//! Declaration serialization preserves the native order of semantic inference,
//! syntactic annotation validation, and tracker fallback diagnostics.
use super::NodeBuilder;
use crate::{object_flags as of, type_flags as tf, Error, SignatureId, TypeId};
use tsr_arena::{NodeId, SymbolId};
use tsr_ast::{node_flags, symbol_flags as sf, SyntaxKind as K};
use tsr_nodebuilder::flags as nf;
use tsr_pseudochecker::{PseudoType, PseudoTypeData as P};

impl NodeBuilder<'_> {
    // port: tsc/internal/checker/nodebuilderimpl.go:NodeBuilderImpl.serializeTypeForExpression
    pub(crate) fn serialize_expression_type(
        &mut self,
        expression: NodeId,
    ) -> Result<NodeId, Error> {
        let ty = self.checker.regular_type_of_expression(expression)?;
        let ty = self.checker.widened_type(ty)?;
        let ty = self.checker.instantiate_type(ty, self.mapper)?;
        self.type_node(ty)
    }

    // port: tsc/internal/checker/nodebuilderimpl.go:NodeBuilderImpl.serializeTypeForDeclaration
    pub(crate) fn serialize_declaration_type(
        &mut self,
        declaration: Option<NodeId>,
        ty: Option<TypeId>,
        symbol: Option<SymbolId>,
        try_reuse: bool,
    ) -> Result<NodeId, Error> {
        let declaration = match (declaration, symbol) {
            (Some(node), _) => Some(node),
            (_, Some(symbol)) => self.checker.symbol(symbol)?.value_declaration().or(self
                .checker
                .symbol_declarations(symbol)?
                .first()
                .flatten()),
            _ => None,
        };
        let symbol = match (symbol, declaration) {
            (Some(s), _) => Some(s),
            (_, Some(d)) => self.checker.get_symbol_of_declaration(d)?,
            _ => None,
        };
        let mut ty = match ty {
            Some(ty) => ty,
            None => match symbol {
                None => match declaration {
                    Some(decl)
                        if tsr_ast::utilities_middle::is_variable_like(
                            &self.checker.node(decl)?,
                        ) =>
                    {
                        self.checker
                            .type_for_variable_like_raw(decl, false, 0)?
                            .ok_or(Error::MissingLink("declaration serialization raw type"))?
                    }
                    _ => self.checker.builtins.error_type,
                },
                Some(symbol) => {
                    if let Some(&ty) = self.enclosing_symbol_types.get(&symbol) {
                        ty
                    } else {
                        let flags = self.checker.symbol(symbol)?.flags();
                        let value = if flags & sf::ACCESSOR != 0
                            && declaration
                                .map(|d| {
                                    self.checker
                                        .ast(d)?
                                        .node(d)
                                        .map(|n| n.kind() == K::SetAccessor)
                                        .map_err(Error::from)
                                })
                                .transpose()?
                                .unwrap_or(false)
                        {
                            self.checker.write_type_of_symbol(symbol)?
                        } else if flags & (sf::TYPE_LITERAL | sf::SIGNATURE) == 0 {
                            let ty = self.checker.get_type_of_symbol(symbol)?;
                            self.checker.widen_literal_type(ty)?
                        } else {
                            self.checker.builtins.error_type
                        };
                        self.checker.instantiate_type(value, self.mapper)?
                    }
                }
            },
        };
        let mut requires_undefined = false;
        if let Some(decl) = declaration {
            let kind = self.checker.node(decl)?.kind();
            if matches!(
                kind.known(),
                Some(K::Parameter | K::PropertySignature | K::PropertyDeclaration)
            ) {
                requires_undefined =
                    self.checker
                        .emit_requires_undefined(decl, symbol, self.enclosing)?;
                if requires_undefined && kind == K::Parameter {
                    ty = self.checker.add_type_optionality(ty, false, true)?;
                }
            }
        }
        let saved_flags = self.flags;
        let result = (|| {
            if self.checker.types.flags(ty)? & tf::UNIQUE_ES_SYMBOL != 0
                && self.checker.types.get(ty)?.symbol == symbol
            {
                let same_file = if let (Some(enclosing), Some(symbol)) = (self.enclosing, symbol) {
                    let file = tsr_ast::utilities::get_source_file_of_node(
                        self.checker.ast(enclosing)?,
                        Some(enclosing),
                    )?;
                    let declarations: Vec<_> = self
                        .checker
                        .symbol_declarations(symbol)?
                        .iter()
                        .flatten()
                        .collect();
                    let mut same = false;
                    for node in declarations {
                        if tsr_ast::utilities::get_source_file_of_node(
                            self.checker.ast(node)?,
                            Some(node),
                        )? == file
                        {
                            same = true;
                            break;
                        }
                    }
                    same
                } else {
                    true
                };
                if same_file {
                    self.flags |= nf::ALLOW_UNIQUE_ES_SYMBOL_TYPE;
                }
            }
            let eligible = match declaration {
                Some(decl) => {
                    let read = self.checker.node(decl)?;
                    matches!(read.kind().known(), Some(K::GetAccessor | K::SetAccessor))
                        || tsr_ast::utilities_tail::has_inferred_type(&read)
                            && read.flags() & node_flags::SYNTHESIZED == 0
                            && self.checker.types.get(ty)?.object_flags & of::REQUIRES_WIDENING == 0
                }
                None => false,
            };
            let mut reported_fallback = false;
            if try_reuse && self.enclosing.is_some() && eligible {
                let decl = declaration.expect("eligible declaration");
                let previous = symbol.and_then(|s| self.enclosing_symbol_types.insert(s, ty));
                let attempt = (|| {
                    let accessor = matches!(
                        self.checker.node(decl)?.kind().known(),
                        Some(K::GetAccessor | K::SetAccessor)
                    );
                    let pc = self.checker.pseudo_checker();
                    let mut pseudo = if accessor {
                        pc.get_type_of_accessor(self.checker, decl)?
                    } else {
                        pc.get_type_of_declaration(self.checker, decl)?
                    };
                    if matches!(pseudo.as_ref(), P::NoResult { .. })
                        && self.checker.node(decl)?.kind() == K::BinaryExpression
                    {
                        if let Some(symbol) = symbol {
                            let declarations: Vec<_> = self
                                .checker
                                .symbol_declarations(symbol)?
                                .iter()
                                .flatten()
                                .collect();
                            for node in declarations {
                                let read = self.checker.node(node)?;
                                if read.type_node().is_some()
                                    && !matches!(
                                        read.kind().known(),
                                        Some(K::TypeAliasDeclaration | K::JSTypeAliasDeclaration)
                                    )
                                {
                                    pseudo = pc.get_type_of_declaration(self.checker, node)?;
                                    break;
                                }
                            }
                        }
                    }
                    let report = !self.suppress_inference_fallback;
                    let kind = self.checker.node(decl)?.kind();
                    let optional = !requires_undefined
                        && matches!(
                            kind.known(),
                            Some(K::Parameter | K::PropertySignature | K::PropertyDeclaration)
                        )
                        && self
                            .checker
                            .ast(decl)?
                            .node(decl)?
                            .question_token(self.checker.ast(decl)?)?
                            .is_some();
                    if self.pseudo_type_equivalent(&pseudo, Some(ty), optional, report)? {
                        if let Some(from) = self.pseudo_type_to_type(&pseudo)? {
                            if requires_undefined
                                && self.non_missing_undefined(ty)?
                                && !self.non_missing_undefined(from)?
                            {
                                pseudo = tsr_pseudochecker::union(vec![
                                    pseudo,
                                    tsr_pseudochecker::undefined(),
                                ]);
                            }
                        }
                        return self.pseudo_node_with_fallback(&pseudo, ty).map(Some);
                    }
                    reported_fallback = report
                        && matches!(pseudo.as_ref(), P::Inferred { error_nodes, .. } if !error_nodes.is_empty());
                    let add = if requires_undefined {
                        match self.pseudo_type_to_type(&pseudo)? {
                            Some(from) => !self.non_missing_undefined(from)?,
                            None => !tsr_pseudochecker::could_already_refer_to_undefined_type(
                                self.checker,
                                &pseudo,
                            )?,
                        }
                    } else {
                        false
                    };
                    if add {
                        pseudo =
                            tsr_pseudochecker::union(vec![pseudo, tsr_pseudochecker::undefined()]);
                        if self.pseudo_type_equivalent(&pseudo, Some(ty), false, report)? {
                            reported_fallback = false;
                            return self.pseudo_node_with_fallback(&pseudo, ty).map(Some);
                        }
                    }
                    Ok(None)
                })();
                if let Some(symbol) = symbol {
                    match previous {
                        Some(ty) => {
                            self.enclosing_symbol_types.insert(symbol, ty);
                        }
                        None => {
                            self.enclosing_symbol_types.remove(&symbol);
                        }
                    }
                }
                if let Some(node) = attempt? {
                    return Ok(node);
                }
            }
            let suppress = self.suppress_inference_fallback;
            if reported_fallback {
                self.suppress_inference_fallback = true;
            }
            let result = self.type_node(ty);
            self.suppress_inference_fallback = suppress;
            result
        })();
        self.flags = saved_flags;
        result
    }

    // port: tsc/internal/checker/utilities.go:containsNonMissingUndefinedType
    fn non_missing_undefined(&self, ty: TypeId) -> Result<bool, Error> {
        let first = if self.checker.types.flags(ty)? & tf::UNION != 0 {
            self.checker.types.union(ty)?.types[0]
        } else {
            ty
        };
        Ok(self.checker.types.flags(first)? & tf::UNDEFINED != 0
            && first != self.checker.builtins.missing_type)
    }

    // port: tsc/internal/checker/nodebuilderimpl.go:NodeBuilderImpl.serializeReturnTypeForSignature
    pub(crate) fn serialize_signature_return(
        &mut self,
        signature: SignatureId,
        try_reuse: bool,
    ) -> Result<Option<NodeId>, Error> {
        let flags = self.flags;
        let suppress_any = flags & nf::SUPPRESS_ANY_RETURN_TYPE != 0;
        if suppress_any {
            self.flags &= !nf::SUPPRESS_ANY_RETURN_TYPE;
        }
        let result = (|| {
            let declaration = self.checker.signatures.get(signature)?.declaration;
            let original = if let Some(decl) = declaration {
                (self.checker.node(decl)?.flags() & node_flags::SYNTHESIZED == 0).then_some(decl)
            } else {
                None
            };
            let ty = if let Some(decl) = original {
                let symbol = self.checker.get_symbol_of_declaration(decl)?;
                if let Some(ty) = symbol
                    .and_then(|s| self.enclosing_symbol_types.get(&s))
                    .copied()
                {
                    ty
                } else {
                    let ty = self.checker.return_type_of_signature(signature)?;
                    self.checker.instantiate_type(ty, self.mapper)?
                }
            } else {
                self.checker.return_type_of_signature(signature)?
            };
            if suppress_any && self.checker.types.flags(ty)? & tf::ANY != 0 {
                return Ok(None);
            }
            if let Some(decl) = original.filter(|_| try_reuse && self.enclosing.is_some()) {
                let symbol = self.checker.get_symbol_of_declaration(decl)?;
                let previous = symbol.and_then(|s| self.enclosing_symbol_types.insert(s, ty));
                let attempt = (|| {
                    let pseudo = self
                        .checker
                        .pseudo_checker()
                        .get_return_type_of_signature(self.checker, decl)?;
                    if self.pseudo_type_equivalent(
                        &pseudo,
                        Some(ty),
                        false,
                        !self.suppress_inference_fallback,
                    )? {
                        if let Some(predicate) =
                            self.checker.type_predicate_of_signature(signature)?
                        {
                            if !self.pseudo_return_matches_predicate(&pseudo, predicate)? {
                                if !self.suppress_inference_fallback {
                                    self.report(tsr_printer::emit_resolver::DeclarationTrackerEvent::InferenceFallback(decl));
                                }
                                return Ok(None);
                            }
                        }
                        return self.pseudo_node_with_fallback(&pseudo, ty).map(Some);
                    }
                    Ok(None)
                })();
                if let Some(symbol) = symbol {
                    match previous {
                        Some(ty) => {
                            self.enclosing_symbol_types.insert(symbol, ty);
                        }
                        None => {
                            self.enclosing_symbol_types.remove(&symbol);
                        }
                    }
                }
                if let Some(node) = attempt? {
                    return Ok(Some(node));
                }
            }
            let suppress = self.suppress_inference_fallback;
            self.suppress_inference_fallback = true;
            let result = (|| {
                if let Some(predicate) = self.checker.type_predicate_of_signature(signature)? {
                    let mut data = self.checker.signatures.predicate(predicate)?.clone();
                    if let Some(mapper) = self.mapper {
                        data.t = data
                            .t
                            .map(|t| self.checker.instantiate_type(t, Some(mapper)))
                            .transpose()?;
                    }
                    self.predicate_data_node(data)
                } else {
                    self.type_node(ty)
                }
            })();
            self.suppress_inference_fallback = suppress;
            result.map(Some)
        })();
        self.flags = flags;
        result
    }

    // port: tsc/internal/checker/pseudotypenodebuilder.go:NodeBuilderImpl.pseudoTypeToNodeWithCheckerFallback
    pub(super) fn pseudo_node_with_fallback(
        &mut self,
        pseudo: &PseudoType,
        ty: TypeId,
    ) -> Result<NodeId, Error> {
        let fallback = match pseudo.as_ref() {
            P::Inferred {
                expression,
                error_nodes,
                ..
            } => {
                if !self.suppress_inference_fallback {
                    if error_nodes.is_empty() {
                        self.report(
                            tsr_printer::emit_resolver::DeclarationTrackerEvent::InferenceFallback(
                                *expression,
                            ),
                        );
                    } else {
                        for &node in error_nodes {
                            self.report(tsr_printer::emit_resolver::DeclarationTrackerEvent::InferenceFallback(node));
                        }
                    }
                }
                true
            }
            P::Direct { type_node } => {
                if self.can_reuse_existing_js_type_node(*type_node, ty)? {
                    false
                } else {
                    if !self.suppress_inference_fallback {
                        self.report(
                            tsr_printer::emit_resolver::DeclarationTrackerEvent::InferenceFallback(
                                *type_node,
                            ),
                        );
                    }
                    true
                }
            }
            _ => false,
        };
        if !fallback {
            return self.pseudo_type_to_node(pseudo);
        }
        let old = self.suppress_inference_fallback;
        self.suppress_inference_fallback = true;
        let result = self.type_node(ty);
        self.suppress_inference_fallback = old;
        result
    }
}
