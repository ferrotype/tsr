//! Expression-directed argument error elaboration, kept separate from overload
//! selection because its extra type resolutions are observable only on failure.

use crate::{type_flags as tf, CheckerState, Error, RelationKind, TypeId};
use ts_arena::NodeId;
use ts_ast::{Diagnostic, SyntaxKind as K};
use ts_diagnostics as messages;

impl CheckerState {
    // port: tsc/internal/checker/checker.go:Checker.maybeAddMissingAwaitInfo
    pub(crate) fn maybe_add_missing_await_info(
        &mut self,
        node: Option<NodeId>,
        source: TypeId,
        target: TypeId,
        relation: RelationKind,
        report: bool,
        output: &mut [Diagnostic],
    ) -> Result<(), Error> {
        let Some(node) = node.filter(|_| report && !output.is_empty()) else {
            return Ok(());
        };
        if self.awaited_type_of_promise(target)?.is_some() {
            return Ok(());
        }
        if let Some(awaited) = self.awaited_type_of_promise(source)? {
            if self.is_type_related_to(awaited, target, relation)? {
                output[0]
                    .related_information
                    .push(std::sync::Arc::new(self.diagnostic_for_node(
                        Some(node),
                        messages::Did_you_forget_to_use_await,
                        vec![],
                    )?));
            }
        }
        Ok(())
    }

    // port: tsc/internal/checker/checker.go:Checker.invocationErrorDetails
    pub(crate) fn call_invocation_error(
        &mut self,
        node: NodeId,
        ty: TypeId,
        construct: bool,
    ) -> Result<(), Error> {
        let expression = ts_ast::utilities_middle::get_invoked_expression(self.ast(node)?, node)?
            .ok_or(Error::MissingLink("invocation expression"))?;
        let awaited = self.awaited_type(ty)?;
        let missing_await = match awaited {
            Some(awaited) => !self.signatures_of_type(awaited, construct)?.is_empty(),
            None => false,
        };
        let target = if self.node(node)?.kind() == K::CallExpression
            && self.node(expression)?.kind() == K::PropertyAccessExpression
        {
            self.node(expression)?
                .name()
                .ok_or(Error::MissingLink("invocation property name"))?
        } else {
            expression
        };
        let text = self.type_to_string(
            ty,
            crate::type_format_flags::ALLOW_UNIQUE_ES_SYMBOL_TYPE
                | crate::type_format_flags::USE_ALIAS_DEFINED_OUTSIDE_CURRENT_SCOPE,
        )?;
        let no_signatures = if construct {
            messages::Type_0_has_no_construct_signatures
        } else {
            messages::Type_0_has_no_call_signatures
        };
        let mut diagnostic = None;
        if self.types.flags(ty)? & tf::UNION != 0 {
            let mut has_signatures = false;
            for part in self.types.compound_types(ty)?.to_vec() {
                if self.signatures_of_type(part, construct)?.is_empty() {
                    if diagnostic.is_none() {
                        let part_text = self.type_to_string(
                            part,
                            crate::type_format_flags::ALLOW_UNIQUE_ES_SYMBOL_TYPE
                                | crate::type_format_flags::USE_ALIAS_DEFINED_OUTSIDE_CURRENT_SCOPE,
                        )?;
                        let detail =
                            self.diagnostic_for_node(Some(target), no_signatures, vec![part_text])?;
                        diagnostic = Some(ts_ast::Diagnostic::chain(
                            Some(std::sync::Arc::new(detail)),
                            if construct {
                                messages::Not_all_constituents_of_type_0_are_constructable
                            } else {
                                messages::Not_all_constituents_of_type_0_are_callable
                            },
                            vec![text.clone()],
                        ));
                    }
                    if has_signatures {
                        break;
                    }
                } else {
                    has_signatures = true;
                    if diagnostic.is_some() {
                        break;
                    }
                }
            }
            if !has_signatures {
                diagnostic = Some(self.diagnostic_for_node(
                    Some(target),
                    if construct {
                        messages::No_constituent_of_type_0_is_constructable
                    } else {
                        messages::No_constituent_of_type_0_is_callable
                    },
                    vec![text.clone()],
                )?);
            }
            if diagnostic.is_none() {
                diagnostic = Some(self.diagnostic_for_node(Some(target), if construct { messages::Each_member_of_the_union_type_0_has_construct_signatures_but_none_of_those_signatures_are_compatible_with_each_other } else { messages::Each_member_of_the_union_type_0_has_signatures_but_none_of_those_signatures_are_compatible_with_each_other }, vec![text])?);
            }
        } else {
            diagnostic = Some(self.diagnostic_for_node(Some(target), no_signatures, vec![text])?);
        }
        let getter = if self.node(node)?.kind() == K::CallExpression
            && self
                .source_list(node, self.node(node)?.argument_list())?
                .is_empty()
        {
            match self
                .query
                .resolved_symbols
                .try_get(expression)
                .copied()
                .flatten()
            {
                Some(symbol) => {
                    self.symbol(symbol)?.flags() & ts_ast::symbol_flags::GET_ACCESSOR != 0
                }
                None => false,
            }
        } else {
            false
        };
        let head = if getter {
            messages::This_expression_is_not_callable_because_it_is_a_get_accessor_Did_you_mean_to_use_it_without
        } else if construct {
            messages::This_expression_is_not_constructable
        } else {
            messages::This_expression_is_not_callable
        };
        let mut diagnostic =
            ts_ast::Diagnostic::chain(diagnostic.map(std::sync::Arc::new), head, vec![]);
        if let Some(related) = self.module_invocation_error_related(ty, construct)? {
            diagnostic
                .related_information
                .push(std::sync::Arc::new(related));
        }
        if missing_await {
            diagnostic
                .related_information
                .push(std::sync::Arc::new(self.diagnostic_for_node(
                    Some(expression),
                    messages::Did_you_forget_to_use_await,
                    vec![],
                )?));
        }
        if self.node(node)?.kind() == K::CallExpression
            && self
                .source_list(node, self.node(node)?.argument_list())?
                .len()
                == 1
        {
            let view = self.ast(node)?;
            let source = ts_ast::utilities::get_source_file_of_node(view, Some(node))?
                .ok_or(Error::MissingLink("invocation source"))?;
            let source_data = view.source_file(source)?;
            let text = source_data.text();
            let end = ts_scanner::skip_trivia_ex(
                text.as_bytes(),
                i64::from(view.node(expression)?.end()),
                Some(&ts_scanner::SkipTriviaOptions {
                    stop_after_line_break: true,
                    ..Default::default()
                }),
            );
            if text
                .as_bytes()
                .get(end as usize - 1)
                .is_some_and(|byte| matches!(byte, b'\r' | b'\n'))
            {
                diagnostic.related_information.push(std::sync::Arc::new(
                    self.diagnostic_for_node(
                        Some(expression),
                        messages::Are_you_missing_a_semicolon,
                        vec![],
                    )?,
                ));
            }
        }
        self.add_diagnostic(diagnostic)?;
        Ok(())
    }

    // port: tsc/internal/checker/relater.go:Checker.checkTypeRelatedToAndOptionallyElaborate
    pub(crate) fn check_expression_related_with_elaboration(
        &mut self,
        source: TypeId,
        target: TypeId,
        relation: RelationKind,
        error_node: Option<NodeId>,
        expression: Option<NodeId>,
        head: Option<&'static ts_diagnostics::Message>,
    ) -> Result<bool, Error> {
        let mut diagnostics = Vec::new();
        let result = self.collect_expression_relation_errors(
            source,
            target,
            relation,
            error_node,
            expression,
            head,
            &mut diagnostics,
        )?;
        for diagnostic in diagnostics {
            self.add_diagnostic(diagnostic)?;
        }
        Ok(result)
    }

    #[allow(
        clippy::too_many_arguments,
        reason = "Parameters preserve the upstream operation and its independently selected checking modes"
    )]
    pub(crate) fn collect_expression_relation_errors(
        &mut self,
        source: TypeId,
        target: TypeId,
        relation: RelationKind,
        error_node: Option<NodeId>,
        expression: Option<NodeId>,
        head: Option<&'static ts_diagnostics::Message>,
        output: &mut Vec<Diagnostic>,
    ) -> Result<bool, Error> {
        if self.is_type_related_to(source, target, relation)? {
            return Ok(true);
        }
        if let Some(error_node) = error_node {
            let elaborated = match expression {
                Some(expression) => {
                    self.elaborate_call_error(expression, source, target, relation, head, output)?
                }
                None => false,
            };
            if !elaborated {
                let (_, diagnostic) =
                    self.check_type_related_ex(source, target, relation, Some(error_node), head)?;
                if let Some(diagnostic) = diagnostic {
                    output.push(diagnostic);
                }
            }
        }
        Ok(false)
    }

    // port: tsc/internal/checker/relater.go:Checker.elaborateError
    pub(crate) fn elaborate_call_error(
        &mut self,
        node: NodeId,
        source: TypeId,
        target: TypeId,
        relation: RelationKind,
        head: Option<&'static ts_diagnostics::Message>,
        output: &mut Vec<Diagnostic>,
    ) -> Result<bool, Error> {
        if self.call_target_has_conditional(target)?
            || self.program()?.host.options().no_check == ts_core::Tristate::TRUE
        {
            return Ok(false);
        }
        for construct in [true, false] {
            for signature in self.signatures_of_type(source, construct)? {
                let returned = self.return_type_of_signature(signature)?;
                if self.types.flags(returned)? & (tf::ANY | tf::NEVER) == 0
                    && self.is_type_related_to(returned, target, relation)?
                {
                    let (_, diagnostic) =
                        self.check_type_related_ex(source, target, relation, Some(node), head)?;
                    if let Some(mut diagnostic) = diagnostic {
                        diagnostic.related_information.push(std::sync::Arc::new(
                            self.diagnostic_for_node(
                                Some(node),
                                if construct {
                                    messages::Did_you_mean_to_use_new_with_this_expression
                                } else {
                                    messages::Did_you_mean_to_call_this_expression
                                },
                                vec![],
                            )?,
                        ));
                        output.push(diagnostic);
                        return Ok(true);
                    }
                }
            }
        }
        let read = self.node(node)?;
        match read.kind().known() {
            Some(K::ParenthesizedExpression | K::JsxExpression) => {
                let inner = read
                    .expression()
                    .ok_or(Error::MissingLink("argument elaboration parentheses"))?;
                self.elaborate_call_error(inner, source, target, relation, head, output)
            }
            Some(K::ArrowFunction) => {
                self.elaborate_call_arrow(node, source, target, relation, output)
            }
            Some(K::AsExpression) => {
                if ts_ast::utilities_middle::is_const_assertion(self.ast(node)?, &read)? {
                    let inner = read
                        .expression()
                        .ok_or(Error::MissingLink("const assertion expression"))?;
                    self.elaborate_call_error(inner, source, target, relation, head, output)
                } else {
                    Ok(false)
                }
            }
            Some(K::BinaryExpression) => {
                let data = read
                    .data_source()
                    .as_binary_expression()
                    .ok_or(ts_arena::Error::InvalidGraph)?;
                let operator = data
                    .operator_token()
                    .ok_or(Error::MissingLink("elaborated binary operator"))?;
                let right = data
                    .right()
                    .ok_or(Error::MissingLink("elaborated binary right"))?;
                if matches!(
                    self.node(operator)?.kind().known(),
                    Some(K::EqualsToken | K::CommaToken)
                ) {
                    self.elaborate_call_error(right, source, target, relation, head, output)
                } else {
                    Ok(false)
                }
            }
            Some(K::ObjectLiteralExpression) => {
                self.elaborate_object_error(node, source, target, relation, output)
            }
            Some(K::ArrayLiteralExpression) => {
                self.elaborate_array_error(node, source, target, relation, output)
            }
            _ => Ok(false),
        }
    }

    fn call_target_has_conditional(&self, ty: TypeId) -> Result<bool, Error> {
        if self.types.flags(ty)? & tf::CONDITIONAL != 0 {
            return Ok(true);
        }
        if self.types.flags(ty)? & tf::INTERSECTION != 0 {
            for &part in self.types.compound_types(ty)?.iter() {
                if self.call_target_has_conditional(part)? {
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }

    // port: tsc/internal/checker/relater.go:Checker.elaborateArrowFunction
    fn elaborate_call_arrow(
        &mut self,
        node: NodeId,
        source: TypeId,
        target: TypeId,
        relation: RelationKind,
        output: &mut Vec<Diagnostic>,
    ) -> Result<bool, Error> {
        let read = self.node(node)?;
        let body = read
            .body()
            .ok_or(Error::MissingLink("elaborated arrow body"))?;
        if self.node(body)?.kind() == K::Block {
            return Ok(false);
        }
        for parameter in self.source_list(node, read.parameter_list())? {
            if self.node(parameter)?.type_node().is_some() {
                return Ok(false);
            }
        }
        let Some(source_signature) = self.call_single_signature(source)? else {
            return Ok(false);
        };
        let target_signatures = self.signatures_of_type(target, false)?;
        if target_signatures.is_empty() {
            return Ok(false);
        }
        let source_return = self.return_type_of_signature(source_signature)?;
        let mut target_returns = Vec::with_capacity(target_signatures.len());
        for signature in target_signatures {
            target_returns.push(self.return_type_of_signature(signature)?);
        }
        let target_return = self.get_union_type(&target_returns)?;
        if self.is_type_related_to(source_return, target_return, relation)? {
            return Ok(false);
        }
        if self.elaborate_call_error(body, source_return, target_return, relation, None, output)? {
            return Ok(true);
        }
        let (_, diagnostic) =
            self.check_type_related_ex(source_return, target_return, relation, Some(body), None)?;
        let Some(mut diagnostic) = diagnostic else {
            return Ok(false);
        };
        if let Some(symbol) = self.types.get(target)?.symbol {
            if let Some(Some(declaration)) = self.symbol_declarations(symbol)?.first() {
                diagnostic.related_information.push(std::sync::Arc::new(
                    self.diagnostic_for_node(
                        Some(declaration),
                        messages::The_expected_type_comes_from_the_return_type_of_this_signature,
                        vec![],
                    )?,
                ));
            }
        }
        if !self.body_function_flags(node)?.0
            && self
                .constituent_property(source_return, b"then", false)?
                .is_none()
        {
            let promised = self.create_promise_type(source_return)?;
            if self.is_type_related_to(promised, target_return, relation)? {
                diagnostic.related_information.push(std::sync::Arc::new(
                    self.diagnostic_for_node(
                        Some(node),
                        messages::Did_you_mean_to_mark_this_function_as_async,
                        vec![],
                    )?,
                ));
            }
        }
        output.push(diagnostic);
        Ok(true)
    }
}
