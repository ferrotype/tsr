//! Explicit type arguments on value expressions filter and instantiate callable
//! signatures while preserving object members. Each union constituent decides
//! independently whether its signatures accept the supplied argument list.
use crate::{object_flags as of, type_flags as tf, CheckerState, Error, SignatureId, TypeId};
use ts_arena::NodeId;
use ts_ast::{node_flags as nf, Diagnostic, JsString, SyntaxKind as K};
use ts_diagnostics as d;

struct Resolution {
    node: NodeId,
    arguments: Vec<NodeId>,
    applicable: bool,
    non_applicable: Option<TypeId>,
}

impl CheckerState {
    // port: tsc/internal/checker/checker.go:Checker.checkExpressionWithTypeArguments
    pub(crate) fn check_instantiation_expression(&mut self, node: NodeId) -> Result<TypeId, Error> {
        let read = self.ast(node)?.node(node)?;
        let value_expression = read.kind() == K::ExpressionWithTypeArguments;
        let expression = if value_expression {
            read.expression()
        } else {
            read.data_source()
                .as_type_query_node()
                .ok_or(Error::MissingLink("instantiation type query"))?
                .expr_name()
        }
        .ok_or(Error::MissingLink("instantiation expression"))?;
        if value_expression
            && self.ast(expression)?.node(expression)?.kind() == K::ImportKeyword
            && self.ast(node)?.node(node)?.type_argument_list().is_some()
        {
            self.grammar_error_node(node,d::This_use_of_import_is_invalid_import_calls_can_be_written_but_they_must_have_parentheses_and_cannot_have_type_arguments,vec![])?;
        } else {
            self.check_grammar_type_arguments(node)?;
        }
        for argument in self.source_list(node, self.ast(node)?.node(node)?.type_argument_list())? {
            self.check_source_element(argument)?;
        }
        if value_expression {
            let mut parent = self.ast(node)?.node(node)?.parent();
            while let Some(id) = parent {
                if self.ast(id)?.node(id)?.kind() != K::ParenthesizedExpression {
                    break;
                }
                parent = self.ast(id)?.node(id)?.parent();
            }
            if let Some(parent) = parent {
                if self.ast(parent)?.node(parent)?.kind() == K::BinaryExpression {
                    let read = self.ast(parent)?.node(parent)?;
                    let data = read
                        .data_source()
                        .as_binary_expression()
                        .ok_or(Error::MissingLink("instantiation binary parent"))?;
                    let operator = data
                        .operator_token()
                        .ok_or(Error::MissingLink("instantiation binary operator"))?;
                    let right = data.right();
                    if self.ast(operator)?.node(operator)?.kind() == K::InstanceOfKeyword
                        && ts_ast::utilities::is_node_descendant_of(
                            self.ast(node)?,
                            Some(node),
                            right,
                        )?
                    {
                        self.error_at(Some(node),d::The_right_hand_side_of_an_instanceof_expression_must_not_be_an_instantiation_expression,vec![])?;
                    }
                }
            }
        }
        let ty = self.check_expression(expression)?;
        self.instantiation_expression_type(ty, node)
    }

    // port: tsc/internal/checker/checker.go:Checker.getInstantiationExpressionType
    pub(crate) fn instantiation_expression_type(
        &mut self,
        ty: TypeId,
        node: NodeId,
    ) -> Result<TypeId, Error> {
        let list = self.ast(node)?.node(node)?.type_argument_list();
        if ty == self.builtins.silent_never_type || self.is_error_type(ty)? || list.is_none() {
            return Ok(ty);
        }
        if let Some(&result) = self.calls.instantiation_expressions.get(&(node, ty)) {
            return Ok(result);
        }
        let mut state = Resolution {
            node,
            arguments: self.source_list(node, list)?,
            applicable: false,
            non_applicable: None,
        };
        let result = self.instantiate_value_type(ty, &mut state)?;
        self.calls
            .instantiation_expressions
            .insert((node, ty), result);
        let error = if state.applicable {
            state.non_applicable
        } else {
            Some(ty)
        };
        if let Some(error) = error {
            let view = self.ast(node)?;
            let source = ts_ast::utilities::get_source_file_of_node(view, Some(node))?
                .ok_or(Error::MissingLink("instantiation source"))?;
            let range = view
                .list(list.ok_or(Error::MissingLink("instantiation arguments"))?)?
                .loc();
            let start =
                ts_scanner::skip_trivia(view.source_file(source)?.text().as_bytes(), range.pos());
            let text = self.type_to_string(error, crate::type_display::DEFAULT_FLAGS)?;
            self.add_diagnostic(Diagnostic::new(
                Some(source),
                ts_core::TextRange::new(start, range.end()),
                d::Type_0_has_no_signatures_for_which_the_type_argument_list_is_applicable,
                vec![text],
            ))?;
        }
        Ok(result)
    }

    fn instantiate_value_type(
        &mut self,
        ty: TypeId,
        state: &mut Resolution,
    ) -> Result<TypeId, Error> {
        let (mut has_signatures, mut applicable) = (false, false);
        let result =
            self.instantiate_value_part(ty, state, &mut has_signatures, &mut applicable)?;
        state.applicable |= applicable;
        if has_signatures && !applicable && state.non_applicable.is_none() {
            state.non_applicable = Some(ty);
        }
        Ok(result)
    }

    fn instantiate_value_part(
        &mut self,
        ty: TypeId,
        state: &mut Resolution,
        has_signatures: &mut bool,
        applicable: &mut bool,
    ) -> Result<TypeId, Error> {
        let flags = self.types.flags(ty)?;
        if flags & tf::OBJECT != 0 {
            self.resolve_type_members(ty)?;
            let structured = self.types.structured(ty)?;
            let members = structured.members;
            let signature_ids = structured.signatures.clone();
            let index_infos = structured.index_infos.clone();
            let count = structured.call_signature_count as usize;
            let signatures = signature_ids.as_deref().unwrap_or_default();
            let calls =
                self.instantiate_value_signatures(&signatures[..count], &state.arguments)?;
            let constructors =
                self.instantiate_value_signatures(&signatures[count..], &state.arguments)?;
            *has_signatures |= !signatures.is_empty();
            *applicable |= !calls.is_empty() || !constructors.is_empty();
            if calls != signatures[..count] || constructors != signatures[count..] {
                let original = self
                    .types
                    .get(ty)?
                    .symbol
                    .ok_or(Error::MissingLink("instantiated value symbol"))?;
                let declarations = self.symbol(original)?.declarations();
                let symbol = self.new_symbol(
                    0,
                    JsString::from_bytes(b"\xfeinstantiationExpression".as_slice()),
                )?;
                self.symbol_mut(symbol)?.declarations = declarations;
                let result = self.new_object_type(
                    of::ANONYMOUS | of::INSTANTIATION_EXPRESSION_TYPE,
                    Some(symbol),
                )?;
                self.set_structured_type_members(
                    result,
                    members,
                    &calls,
                    &constructors,
                    index_infos.as_deref().unwrap_or_default(),
                )?;
                self.types.instantiation_expression_mut(result)?.node = Some(state.node);
                return Ok(result);
            }
        } else if flags & tf::INSTANTIABLE_NON_PRIMITIVE != 0 {
            if let Some(constraint) = self.base_constraint_of_type(ty)? {
                let result =
                    self.instantiate_value_part(constraint, state, has_signatures, applicable)?;
                if result != constraint {
                    return Ok(result);
                }
            }
        } else if flags & tf::UNION != 0 {
            return self
                .map_type(ty, &mut |checker, part| {
                    checker.instantiate_value_type(part, state).map(Some)
                })?
                .ok_or(Error::MissingLink("instantiation expression union"));
        } else if flags & tf::INTERSECTION != 0 {
            let mut types = Vec::new();
            for part in self.types.compound_types(ty)?.to_vec() {
                types.push(self.instantiate_value_part(part, state, has_signatures, applicable)?);
            }
            return self.get_intersection_type(&types);
        }
        Ok(ty)
    }

    fn instantiate_value_signatures(
        &mut self,
        signatures: &[SignatureId],
        nodes: &[NodeId],
    ) -> Result<Vec<SignatureId>, Error> {
        let mut filtered = Vec::new();
        for &signature in signatures {
            let parameters = self
                .signatures
                .get(signature)?
                .type_parameters
                .clone()
                .unwrap_or_else(|| [].into());
            if !parameters.is_empty()
                && (nodes.is_empty()
                    || nodes.len() >= self.min_type_argument_count(&parameters)?
                        && nodes.len() <= parameters.len())
            {
                filtered.push(signature);
            }
        }
        for signature in &mut filtered {
            if let Some(arguments) = self.call_type_arguments(*signature, nodes, true)? {
                let js = self
                    .signatures
                    .get(*signature)?
                    .declaration
                    .map(|node| {
                        self.ast(node)?
                            .node(node)
                            .map(|read| read.flags() & nf::JAVA_SCRIPT_FILE != 0)
                            .map_err(Error::from)
                    })
                    .transpose()?
                    .unwrap_or(false);
                *signature = self.signature_instantiation(*signature, &arguments, js)?;
            }
        }
        Ok(filtered)
    }
}
