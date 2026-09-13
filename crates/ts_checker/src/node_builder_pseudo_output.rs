//! Source-shaped nodes for validated pseudo types, including ordered inference
//! fallback reports when the syntactic structure cannot describe the result.
use super::NodeBuilder;
use crate::Error;
use ts_arena::NodeId;
use ts_ast::NodeListId;
use ts_ast::{Factory, FactoryMethods, SyntaxKind as K};
use ts_nodebuilder::flags as nf;
use ts_printer::{emit_flags, emit_resolver::DeclarationTrackerEvent as Event};
use ts_pseudochecker::{
    PseudoObjectElementData as E, PseudoParameter, PseudoSignature, PseudoType, PseudoTypeData as P,
};

impl NodeBuilder<'_> {
    // port: tsc/internal/checker/pseudotypenodebuilder.go:NodeBuilderImpl.pseudoTypeToNode
    pub(super) fn pseudo_type_to_node(&mut self, pseudo: &PseudoType) -> Result<NodeId, Error> {
        stacker::maybe_grow(128 * 1024, 2 * 1024 * 1024, || {
            self.pseudo_type_to_node_worker(pseudo)
        })
    }
    fn pseudo_type_to_node_worker(&mut self, pseudo: &PseudoType) -> Result<NodeId, Error> {
        match pseudo.as_ref() {
            P::Direct { type_node } => self.reuse_type_node(*type_node),
            P::Inferred {
                expression,
                error_nodes,
                is_signature_return,
            } => {
                let node = *expression;
                let parent = self.checker.ast(node)?.node(node)?.parent();
                if !error_nodes.is_empty() {
                    for &node in error_nodes {
                        self.report(Event::InferenceFallback(node));
                    }
                } else if ts_ast::is_entity_name_expression(self.checker.ast(node)?, node)?
                    && parent
                        .map(|p| {
                            self.checker
                                .ast(p)?
                                .node(p)
                                .map(|n| ts_ast::is_declaration(&n))
                                .map_err(Error::from)
                        })
                        .transpose()?
                        .unwrap_or(false)
                {
                    self.report(Event::InferenceFallback(
                        parent.expect("declaration parent"),
                    ));
                } else {
                    self.report(Event::InferenceFallback(node));
                }
                if *is_signature_return {
                    return self.pseudo_return_node(node);
                }
                if let Some(parent) = parent {
                    let read = self.checker.ast(parent)?.node(parent)?;
                    if read.kind() == K::ReturnStatement {
                        let function = self
                            .checker
                            .containing_body_function(node)?
                            .ok_or(Error::MissingLink("pseudo return container"))?;
                        if matches!(
                            self.checker.ast(function)?.node(function)?.kind().known(),
                            Some(K::GetAccessor | K::SetAccessor)
                        ) {
                            return self.serialize_declaration_type(
                                Some(function),
                                None,
                                None,
                                false,
                            );
                        }
                        return self.pseudo_return_node(function);
                    }
                    if read.kind() == K::ArrowFunction && read.body() == Some(node) {
                        return self.pseudo_return_node(parent);
                    }
                    if ts_ast::is_declaration(&read) {
                        return self.serialize_declaration_type(Some(parent), None, None, false);
                    }
                }
                let ty = self.checker.get_type_of_expression(node)?;
                self.type_node(ty)
            }
            P::NoResult { declaration } => {
                self.report(Event::InferenceFallback(*declaration));
                let read = self.checker.ast(*declaration)?.node(*declaration)?;
                if ts_ast::utilities::is_function_like(Some(&read))
                    && !matches!(read.kind().known(), Some(K::GetAccessor | K::SetAccessor))
                {
                    self.pseudo_return_node(*declaration)
                } else {
                    self.serialize_declaration_type(Some(*declaration), None, None, false)
                }
            }
            P::MaybeConstLocation {
                node,
                const_type,
                regular_type,
            } => {
                let mut is_const = self.checker.is_const_context(*node)?;
                if !is_const && ts_pseudochecker::is_in_const_context(self.checker, *node)? {
                    let context = self.checker.contextual_expression_type(*node)?;
                    if let Some(ty) = self.pseudo_type_to_type(const_type)? {
                        let inference = self.checker.call_inference_at_node(*node)?;
                        let context = context
                            .map(|t| {
                                self.checker
                                    .instantiate_call_contextual_type(t, inference, false)
                            })
                            .transpose()?;
                        is_const = self.checker.literal_of_context(ty, context)?;
                    }
                }
                self.pseudo_type_to_node(if is_const { const_type } else { regular_type })
            }
            P::Union { types } => {
                let mut nodes = Vec::new();
                let mut elided = false;
                let mut has_undefined = false;
                for ty in types {
                    if !self.checker.options.strict_null_checks
                        && matches!(ty.as_ref(), P::Undefined | P::Null)
                    {
                        elided = true;
                        continue;
                    }
                    let node = self.pseudo_type_to_node(ty)?;
                    let mut stack = vec![node];
                    while let Some(node) = stack.pop() {
                        let view = self.ast.view();
                        let read = view.node(node)?;
                        if read.kind() == K::UnionType {
                            let list = read
                                .data_source()
                                .as_union_type_node()
                                .ok_or(ts_arena::Error::InvalidGraph)?
                                .types()
                                .ok_or(Error::MissingLink("pseudo union list"))?;
                            let elements: Vec<_> = view
                                .node_slice(view.list(list)?.nodes())?
                                .iter()
                                .flatten()
                                .collect();
                            stack.extend(elements.into_iter().rev());
                        } else {
                            if read.kind() == K::UndefinedKeyword {
                                if has_undefined {
                                    continue;
                                }
                                has_undefined = true;
                            }
                            nodes.push(node);
                        }
                    }
                }
                match nodes.as_slice() {
                    [] => Ok(self.ast.new_keyword_type_node(
                        if elided {
                            K::AnyKeyword
                        } else {
                            K::NeverKeyword
                        }
                        .into(),
                    )),
                    [node] => Ok(*node),
                    _ => {
                        let list = self.list(nodes)?;
                        Ok(self.ast.new_union_type_node(Some(list)))
                    }
                }
            }
            P::Undefined | P::Null if !self.checker.options.strict_null_checks => {
                Ok(self.ast.new_keyword_type_node(K::AnyKeyword.into()))
            }
            P::Undefined | P::Any | P::String | P::Number | P::BigInt | P::Boolean => {
                let kind = match pseudo.as_ref() {
                    P::Undefined => K::UndefinedKeyword,
                    P::Any => K::AnyKeyword,
                    P::String => K::StringKeyword,
                    P::Number => K::NumberKeyword,
                    P::BigInt => K::BigIntKeyword,
                    _ => K::BooleanKeyword,
                };
                Ok(self.ast.new_keyword_type_node(kind.into()))
            }
            P::Null | P::False | P::True => {
                let kind = match pseudo.as_ref() {
                    P::Null => K::NullKeyword,
                    P::False => K::FalseKeyword,
                    _ => K::TrueKeyword,
                };
                let expression = self.ast.new_keyword_expression(kind.into());
                Ok(self.ast.new_literal_type_node(Some(expression)))
            }
            P::SingleCallSignature(signature) => {
                self.with_pseudo_signature_scope(signature.signature, |b| {
                    let (type_params, params, result) = b.pseudo_signature_nodes(signature)?;
                    Ok(b.ast
                        .new_function_type_node(type_params, Some(params), Some(result)))
                })
            }
            P::Tuple { elements } => {
                let mut nodes = Vec::with_capacity(elements.len());
                for element in elements {
                    nodes.push(self.pseudo_type_to_node(element)?);
                }
                let list = self.list(nodes)?;
                let tuple = self.ast.new_tuple_type_node(Some(list));
                self.emit.add_emit_flags(tuple, emit_flags::SINGLE_LINE);
                Ok(self
                    .ast
                    .new_type_operator_node(K::ReadonlyKeyword.into(), Some(tuple)))
            }
            P::ObjectLiteral { elements } => {
                if elements.is_empty() {
                    let list = self.list(vec![])?;
                    let node = self.ast.new_type_literal_node(Some(list));
                    self.emit.add_emit_flags(node, emit_flags::SINGLE_LINE);
                    return Ok(node);
                }
                let name_parent = self
                    .checker
                    .ast(elements[0].name)?
                    .node(elements[0].name)?
                    .parent()
                    .ok_or(Error::MissingLink("pseudo object element"))?;
                let container = self
                    .checker
                    .ast(name_parent)?
                    .node(name_parent)?
                    .parent()
                    .ok_or(Error::MissingLink("pseudo object literal"))?;
                let is_const = self.checker.is_const_context(container)?;
                let flags = self.flags;
                self.flags |= nf::IN_OBJECT_TYPE_LITERAL;
                let result = (|| {
                    let mut members = Vec::with_capacity(elements.len());
                    for element in elements {
                        let make = |b: &mut Self| {
                            let readonly = is_const
                                || matches!(
                                    element.data,
                                    E::PropertyAssignment { readonly: true, .. }
                                );
                            let modifiers = if readonly {
                                let m = b.ast.new_modifier(K::ReadonlyKeyword.into());
                                Some(b.list(vec![m])?)
                            } else {
                                None
                            };
                            let node = match &element.data {
                                E::PropertyAssignment { ty, .. } => {
                                    let name = b.reuse_name(element.name, false)?;
                                    let ty = b.pseudo_type_to_node(ty)?;
                                    b.ast.new_property_signature_declaration(
                                        modifiers,
                                        name,
                                        None,
                                        Some(ty),
                                        None,
                                    )
                                }
                                E::Method(signature) => {
                                    let name = b.reuse_name(element.name, !is_const)?;
                                    let (type_params, params, ty) =
                                        b.pseudo_signature_nodes(signature)?;
                                    if is_const {
                                        let function = b.ast.new_function_type_node(
                                            type_params,
                                            Some(params),
                                            Some(ty),
                                        );
                                        b.ast.new_property_signature_declaration(
                                            modifiers,
                                            name,
                                            None,
                                            Some(function),
                                            None,
                                        )
                                    } else {
                                        b.ast.new_method_signature_declaration(
                                            modifiers,
                                            name,
                                            None,
                                            type_params,
                                            Some(params),
                                            Some(ty),
                                        )
                                    }
                                }
                                E::SetAccessor { parameter, .. } => {
                                    let name = b.reuse_name(element.name, false)?;
                                    let parameter = b.pseudo_parameter_node(parameter)?;
                                    let parameters = b.list(vec![parameter])?;
                                    b.ast.new_set_accessor_declaration(
                                        None,
                                        name,
                                        None,
                                        Some(parameters),
                                        None,
                                        None,
                                        None,
                                    )
                                }
                                E::GetAccessor { ty, .. } => {
                                    let name = b.reuse_name(element.name, false)?;
                                    let ty = b.pseudo_type_to_node(ty)?;
                                    b.ast.new_get_accessor_declaration(
                                        None,
                                        name,
                                        None,
                                        None,
                                        Some(ty),
                                        None,
                                        None,
                                    )
                                }
                            };
                            // Comment range metadata is attached only when the
                            // original declaration is in this enclosing file.
                            b.pseudo_comment_range(
                                node,
                                b.checker.ast(element.name)?.node(element.name)?.parent(),
                            )?;
                            Ok(node)
                        };
                        let node = if let Some(signature) = element.signature() {
                            self.with_pseudo_signature_scope(signature, make)?
                        } else {
                            make(self)?
                        };
                        members.push(node);
                    }
                    self.list(members)
                })();
                self.flags = flags;
                let list = result?;
                let node = self.ast.new_type_literal_node(Some(list));
                if self.flags & nf::MULTILINE_OBJECT_LITERALS == 0 {
                    self.emit.add_emit_flags(node, emit_flags::SINGLE_LINE);
                }
                Ok(node)
            }
            P::StringLiteral { node } | P::NumericLiteral { node } | P::BigIntLiteral { node } => {
                let literal = self.reuse_node(*node)?;
                Ok(self.ast.new_literal_type_node(literal))
            }
        }
    }

    fn pseudo_return_node(&mut self, declaration: NodeId) -> Result<NodeId, Error> {
        let signature = self.checker.signature_from_declaration(declaration)?;
        Ok(self
            .serialize_signature_return(signature, false)?
            .unwrap_or_else(|| self.ast.new_keyword_type_node(K::AnyKeyword.into())))
    }
    fn with_pseudo_signature_scope<T>(
        &mut self,
        declaration: NodeId,
        action: impl FnOnce(&mut Self) -> Result<T, Error>,
    ) -> Result<T, Error> {
        let signature = self.checker.signature_from_declaration(declaration)?;
        self.with_signature_scope(signature, action)
    }
    fn pseudo_signature_nodes(
        &mut self,
        signature: &PseudoSignature,
    ) -> Result<(Option<NodeListId>, NodeListId, NodeId), Error> {
        let type_params = if signature.type_parameters.is_empty() {
            None
        } else {
            let mut nodes = Vec::new();
            for &node in &signature.type_parameters {
                nodes.push(self.reuse_node(node)?);
            }
            Some(self.optional_node_list(nodes)?)
        };
        let mut parameters = Vec::new();
        for parameter in &signature.parameters {
            parameters.push(self.pseudo_parameter_node(parameter)?);
        }
        let parameters = self.list(parameters)?;
        let result = self.pseudo_type_to_node(&signature.return_type)?;
        Ok((type_params, parameters, result))
    }
    // port: tsc/internal/checker/pseudotypenodebuilder.go:NodeBuilderImpl.pseudoParameterToNode
    fn pseudo_parameter_node(&mut self, parameter: &PseudoParameter) -> Result<NodeId, Error> {
        let rest = parameter
            .rest
            .then(|| self.ast.new_token(K::DotDotDotToken.into()));
        let question = parameter
            .optional
            .then(|| self.ast.new_token(K::QuestionToken.into()));
        let parent = self
            .checker
            .ast(parameter.name)?
            .node(parameter.name)?
            .parent()
            .ok_or(Error::MissingLink("pseudo parameter parent"))?;
        let symbol = self.checker.raw_declaration_symbol(parent)?;
        let name = self.parameter_declaration_name(symbol, parent)?;
        let ty = self.pseudo_type_to_node(&parameter.ty)?;
        let node =
            self.ast
                .new_parameter_declaration(None, rest, Some(name), question, Some(ty), None);
        if self.checker.ast(parent)?.node(parent)?.kind() == K::Parameter {
            self.pseudo_comment_range(node, Some(parent))?;
        }
        Ok(node)
    }
    fn pseudo_comment_range(
        &mut self,
        node: NodeId,
        original: Option<NodeId>,
    ) -> Result<(), Error> {
        let (Some(original), Some(enclosing)) = (original, self.enclosing) else {
            return Ok(());
        };
        let source = ts_ast::utilities::get_source_file_of_node(
            self.checker.ast(original)?,
            Some(original),
        )?;
        if source
            == ts_ast::utilities::get_source_file_of_node(
                self.checker.ast(enclosing)?,
                Some(enclosing),
            )?
        {
            self.emit
                .set_comment_range(node, self.checker.ast(original)?.node(original)?.range());
        }
        Ok(())
    }
}
