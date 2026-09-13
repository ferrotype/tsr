//! Anonymous class/function serialization preserves native value accessibility,
//! structural expansion and the diagnostics attached to private base members.
use super::NodeBuilder;
use crate::{object_flags as of, signature_flags as sigf, type_flags as tf, Error, TypeId};
use ts_arena::{NodeId, SymbolId};
use ts_ast::{modifier_flags as mf, symbol_flags as sf, JsString, SyntaxKind as K};
use ts_nodebuilder::flags as nf;
use ts_printer::emit_resolver::{DeclarationTrackerEvent as Event, SymbolAccessibility as Access};

/// Native CompositeSymbolIdentity distinguishes class constructor objects from
/// instance types, and source-node identities from a symbol's instantiations.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum SymbolIdentity {
    Node(NodeId),
    Symbol { constructor: bool, symbol: SymbolId },
}

impl NodeBuilder<'_> {
    // port: tsc/internal/checker/symbolaccessibility.go:Checker.IsValueSymbolAccessible
    pub(super) fn value_symbol_accessible(&mut self, symbol: SymbolId) -> Result<bool, Error> {
        Ok(self
            .checker
            .emit_symbol_accessible(Some(symbol), self.enclosing, sf::VALUE, false, true)?
            .accessibility
            == Access::Accessible)
    }

    // port: tsc/internal/checker/nodebuilderimpl.go:isClassInstanceSide
    fn class_instance_side(&mut self, ty: TypeId, symbol: SymbolId) -> Result<bool, Error> {
        if self.checker.symbol(symbol)?.flags() & sf::CLASS == 0 {
            return Ok(false);
        }
        Ok(ty == self.checker.declared_interface_type(symbol)?
            || self.checker.types.flags(ty)? & tf::OBJECT != 0
                && self.checker.types.object_flags(ty)? & of::IS_CLASS_INSTANCE_CLONE != 0)
    }

    // port: tsc/internal/checker/nodebuilderimpl.go:NodeBuilderImpl.shouldWriteTypeOfFunctionSymbol
    fn should_write_type_of_function(
        &mut self,
        mut symbol: SymbolId,
        ty: TypeId,
    ) -> Result<(bool, SymbolId), Error> {
        let flags = self.checker.symbol(symbol)?.flags();
        let declarations = self.checker.symbol_declarations(symbol)?.to_vec();
        let mut static_method = false;
        if flags & sf::METHOD != 0
            && ts_scanner::is_identifier_text(
                self.checker.symbol(symbol)?.name_bytes(),
                ts_core::LanguageVariant::STANDARD,
            )
        {
            for declaration in declarations.iter().flatten().copied() {
                let view = self.checker.ast(declaration)?;
                let read = view.node(declaration)?;
                if read.modifier_flags(view)? & mf::STATIC != 0 {
                    let name = ts_ast::get_name_of_declaration(view, Some(declaration))?;
                    if !match name {
                        Some(name) => self.reuse_late_bindable_name(name)?,
                        None => false,
                    } {
                        static_method = true;
                        break;
                    }
                }
            }
        }
        let mut nonlocal_function = false;
        let mut function_expression = false;
        let mut parent_symbol_missing = false;
        if flags & sf::FUNCTION != 0 {
            if self.checker.symbol(symbol)?.parent().is_some() {
                nonlocal_function = true;
            } else {
                for declaration in declarations.into_iter().flatten() {
                    let read = self.checker.ast(declaration)?.node(declaration)?;
                    let Some(parent) = read.parent() else {
                        continue;
                    };
                    let parent_kind = self.checker.ast(parent)?.node(parent)?.kind();
                    if matches!(parent_kind.known(), Some(K::SourceFile | K::ModuleBlock)) {
                        nonlocal_function = true;
                        break;
                    }
                    if matches!(
                        read.kind().known(),
                        Some(K::FunctionExpression | K::ArrowFunction)
                    ) && parent_kind == K::VariableDeclaration
                    {
                        let mut node = Some(parent);
                        let mut matches = true;
                        for kind in [K::VariableDeclarationList, K::VariableStatement] {
                            node = node
                                .map(|id| {
                                    self.checker
                                        .ast(id)?
                                        .node(id)
                                        .map(|read| read.parent())
                                        .map_err(Error::from)
                                })
                                .transpose()?
                                .flatten();
                            if !match node {
                                Some(id) => self.checker.ast(id)?.node(id)?.kind() == kind,
                                None => false,
                            } {
                                matches = false;
                                break;
                            }
                        }
                        if matches {
                            let container = node
                                .map(|id| {
                                    self.checker
                                        .ast(id)?
                                        .node(id)
                                        .map(|read| read.parent())
                                        .map_err(Error::from)
                                })
                                .transpose()?
                                .flatten();
                            if match container {
                                Some(id) => matches!(
                                    self.checker.ast(id)?.node(id)?.kind().known(),
                                    Some(K::SourceFile | K::ModuleBlock)
                                ),
                                None => false,
                            } {
                                nonlocal_function = true;
                                function_expression = true;
                                break;
                            }
                        }
                    }
                }
            }
        }
        if !static_method && !nonlocal_function {
            return Ok((false, symbol));
        }
        if function_expression {
            if let Some(declaration) = self.checker.symbol(symbol)?.value_declaration() {
                if let Some(parent) = self.checker.ast(declaration)?.node(declaration)?.parent() {
                    if Some(parent) != self.enclosing {
                        // A binding-pattern declaration has no symbol; upstream then
                        // carries a nil symbol, which is trivially accessible.
                        match self.checker.raw_declaration_symbol(parent)? {
                            Some(parent_symbol) => {
                                symbol = self.checker.get_merged_symbol(parent_symbol);
                            }
                            None => parent_symbol_missing = true,
                        }
                    }
                }
            }
        }
        let use_typeof = (self.flags & nf::USE_TYPE_OF_FUNCTION != 0 || self.visited.contains(&ty))
            && (self.flags & nf::USE_STRUCTURAL_FALLBACK == 0
                || parent_symbol_missing
                || self.value_symbol_accessible(symbol)?);
        if use_typeof && parent_symbol_missing {
            return Err(Error::MissingLink("function expression variable symbol"));
        }
        Ok((use_typeof, symbol))
    }

    // port: tsc/internal/checker/nodebuilderimpl.go:NodeBuilderImpl.shouldEmitTypeOfSymbol
    fn should_emit_type_of_symbol(
        &mut self,
        ty: TypeId,
        symbol: SymbolId,
        meaning: u32,
    ) -> Result<(bool, SymbolId), Error> {
        let read = self.checker.symbol(symbol)?;
        let flags = read.flags();
        let value = read.value_declaration();
        if flags & sf::CLASS != 0 && self.checker.class_base_type_variable(symbol)?.is_none() {
            let expand = if self.flags & nf::WRITE_CLASS_EXPRESSION_AS_TYPE_LITERAL != 0 {
                match value {
                    Some(value)
                        if ts_ast::utilities::is_class_like(
                            &self.checker.ast(value)?.node(value)?,
                        ) =>
                    {
                        self.checker.ast(value)?.node(value)?.kind() != K::ClassDeclaration
                            || self
                                .checker
                                .emit_symbol_accessible(
                                    Some(symbol),
                                    self.enclosing,
                                    meaning,
                                    false,
                                    true,
                                )?
                                .accessibility
                                != Access::Accessible
                    }
                    _ => false,
                }
            } else {
                false
            };
            if !expand {
                return Ok((true, symbol));
            }
        }
        if flags & (sf::ENUM | sf::VALUE_MODULE) != 0 {
            return Ok((true, symbol));
        }
        self.should_write_type_of_function(symbol, ty)
    }

    // port: tsc/internal/checker/nodebuilderimpl.go:getTypeAliasForTypeLiteral
    fn alias_for_recursive_literal(&mut self, symbol: SymbolId) -> Result<Option<SymbolId>, Error> {
        if self.checker.symbol(symbol)?.flags() & sf::TYPE_LITERAL == 0 {
            return Ok(None);
        }
        let Some(declaration) = self
            .checker
            .symbol_declarations(symbol)?
            .iter()
            .flatten()
            .next()
        else {
            return Ok(None);
        };
        let mut node = self.checker.ast(declaration)?.node(declaration)?.parent();
        while let Some(id) = node {
            let read = self.checker.ast(id)?.node(id)?;
            if read.kind() != K::ParenthesizedType {
                return if read.kind() == K::TypeAliasDeclaration {
                    self.checker.get_symbol_of_declaration(id)
                } else {
                    Ok(None)
                };
            }
            node = read.parent();
        }
        Ok(None)
    }

    // port: tsc/internal/checker/nodebuilderimpl.go:NodeBuilderImpl.createAnonymousTypeNodeEx
    pub(super) fn anonymous_type_node(&mut self, ty: TypeId) -> Result<NodeId, Error> {
        let record = *self.checker.types.get(ty)?;
        let Some(symbol) = record.symbol else {
            return self.object_type_members_node(ty);
        };
        if record.object_flags & of::INSTANTIATION_EXPRESSION_TYPE != 0 {
            if let Some(existing) = self.checker.types.instantiation_expression(ty)?.node {
                if self.checker.ast(existing)?.node(existing)?.kind() == K::TypeQuery
                    && self.checker.get_type_from_type_node(existing)? == ty
                {
                    if self.visited.contains(&ty) {
                        return Ok(self.elided_type());
                    }
                    self.visited.push(ty);
                    let result = self.try_reuse_existing_type_node(existing, ty, None, None);
                    self.visited.pop();
                    if let Some(node) = result? {
                        return Ok(node);
                    }
                }
            }
            if self.visited.contains(&ty) {
                return Ok(self.elided_type());
            }
            return self.visit_object_type(ty);
        }
        let meaning = if self.class_instance_side(ty, symbol)? {
            sf::TYPE
        } else {
            sf::VALUE
        };
        let (write_symbol, symbol) = self.should_emit_type_of_symbol(ty, symbol, meaning)?;
        if write_symbol {
            return self.symbol_type_node_with_meaning(symbol, meaning);
        }
        if self.visited.contains(&ty) {
            if let Some(alias) = self.alias_for_recursive_literal(symbol)? {
                return self.type_reference(alias, &[]);
            }
            return Ok(self.elided_type());
        }
        self.visit_object_type(ty)
    }

    // Symbol-depth checking is independent of exact TypeId visits because instantiations can
    // revisit a symbol with an unbounded sequence of distinct types.
    // port: tsc/internal/checker/nodebuilderimpl.go:NodeBuilderImpl.visitAndTransformType
    fn visit_object_type(&mut self, ty: TypeId) -> Result<NodeId, Error> {
        self.visit_transform_type(ty, Self::object_type_members_node)
    }

    // port: tsc/internal/checker/nodebuilderimpl.go:NodeBuilderImpl.symbolToTypeNode
    pub(super) fn symbol_type_node_with_meaning(
        &mut self,
        symbol: SymbolId,
        meaning: u32,
    ) -> Result<NodeId, Error> {
        if meaning == sf::TYPE {
            return self.type_reference(symbol, &[]);
        }
        if self.name_external_module(symbol)? {
            return self.module_type_node(symbol, true, &[]);
        }
        self.track_symbol(symbol, meaning)?;
        self.approximate_length += 6;
        self.symbol_type_node_from_chain(symbol, meaning, None)
    }

    // port: tsc/internal/checker/nodebuilderimpl.go:NodeBuilderImpl.typeReferenceToTypeNode
    pub(super) fn inaccessible_class_reference(&mut self, ty: TypeId) -> Result<bool, Error> {
        if self.flags & nf::WRITE_CLASS_EXPRESSION_AS_TYPE_LITERAL == 0 {
            return Ok(false);
        }
        let Some(symbol) = self.checker.types.get(ty)?.symbol else {
            return Ok(false);
        };
        let Some(declaration) = self.checker.symbol(symbol)?.value_declaration() else {
            return Ok(false);
        };
        Ok(
            ts_ast::utilities::is_class_like(&self.checker.ast(declaration)?.node(declaration)?)
                && !self.value_symbol_accessible(symbol)?,
        )
    }

    // port: tsc/internal/checker/nodebuilderimpl.go:NodeBuilderImpl.createTypeNodesFromResolvedType
    pub(super) fn class_expansion_property(&mut self, symbol: SymbolId) -> Result<bool, Error> {
        if self.flags & nf::WRITE_CLASS_EXPRESSION_AS_TYPE_LITERAL == 0 {
            return Ok(true);
        }
        let read = self.checker.symbol(symbol)?;
        if read.flags() & sf::PROTOTYPE != 0 {
            return Ok(false);
        }
        let name = read.name_to_owned();
        let private_identifier = name.as_bytes().starts_with(b"\xfe#");
        if self.checker.rest_declaration_modifiers(symbol)? & (mf::PRIVATE | mf::PROTECTED) != 0 {
            self.report(Event::PrivateInBaseOfClassExpression(name));
        }
        if private_identifier {
            let read = self.checker.symbol(symbol)?;
            let name = if let Some(declaration) = read.value_declaration() {
                JsString::from_bytes(
                    ts_ast::symbol_name(&read, self.checker.ast(declaration)?)?.as_bytes(),
                )
            } else {
                read.name_to_owned()
            };
            self.report(Event::PrivateInBaseOfClassExpression(name));
        }
        Ok(true)
    }

    // port: tsc/internal/checker/nodebuilderimpl.go:NodeBuilderImpl.getResolvedTypeWithoutAbstractConstructSignatures
    pub(super) fn without_abstract_constructors(&mut self, ty: TypeId) -> Result<TypeId, Error> {
        let data = self.checker.types.structured(ty)?;
        if let Some(cached) = data.object_type_without_abstract_construct_signatures {
            return Ok(cached);
        }
        let signatures = data.signatures.clone().unwrap_or_default();
        let calls = data.call_signature_count as usize;
        if calls == signatures.len() {
            return Ok(ty);
        }
        let constructors = signatures[calls..]
            .iter()
            .copied()
            .filter_map(|signature| match self.checker.signatures.get(signature) {
                Ok(data) if data.flags & sigf::ABSTRACT == 0 => Some(Ok(signature)),
                Ok(_) => None,
                Err(error) => Some(Err(error)),
            })
            .collect::<Result<Vec<_>, _>>()?;
        let result = if constructors.len() == signatures.len() - calls {
            ty
        } else {
            let indexes = data.index_infos.clone().unwrap_or_default();
            self.checker.new_anonymous_type(
                self.checker.types.get(ty)?.symbol,
                data.members,
                &signatures[..calls],
                &constructors,
                &indexes,
            )?
        };
        self.checker
            .types
            .structured_mut(ty)?
            .object_type_without_abstract_construct_signatures = Some(result);
        self.checker
            .types
            .structured_mut(result)?
            .object_type_without_abstract_construct_signatures = Some(result);
        Ok(result)
    }
}
