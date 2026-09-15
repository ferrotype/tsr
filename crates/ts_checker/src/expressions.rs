//! Expression checking and value references. The source AST remains immutable;
//! resolution and flow links are local to the checker operation's owner.

use crate::{type_flags as tf, CheckerState, Error, TypeId};
use ts_arena::{NodeId, SymbolId};
use ts_ast::{node_flags as nf, symbol_flags as sf, JsString, SyntaxKind as K};

fn required<T>(value: Option<T>, context: &'static str) -> Result<T, Error> {
    value.ok_or(Error::MissingLink(context))
}

impl CheckerState {
    // port: tsc/internal/checker/checker.go:Checker.isNullOrUndefined
    pub(crate) fn null_or_undefined_expression(&mut self, mut node: NodeId) -> Result<bool, Error> {
        while self.ast(node)?.node(node)?.kind() == K::ParenthesizedExpression {
            node = required(
                self.ast(node)?.node(node)?.expression(),
                "nullish parentheses",
            )?;
        }
        match self.ast(node)?.node(node)?.kind().known() {
            Some(K::NullKeyword) => Ok(true),
            Some(K::Identifier) => {
                Ok(self.resolved_value_symbol(node)? == self.builtins.undefined_symbol)
            }
            _ => Ok(false),
        }
    }

    // port: tsc/internal/checker/checker.go:Checker.getContextFreeTypeOfExpression
    pub(crate) fn get_context_free_type_of_expression(
        &mut self,
        node: NodeId,
    ) -> Result<TypeId, Error> {
        if let Some(&ty) = self.query.context_free_types.get(&node) {
            return Ok(ty);
        }
        self.calls.contexts.push(crate::calls::ArgumentContext {
            node,
            ty: self.builtins.any_type,
            inference: None,
        });
        let result = self.check_expression_ex(node, 4);
        self.calls.contexts.pop();
        let ty = result?;
        self.query.context_free_types.insert(node, ty);
        Ok(ty)
    }
    // port: tsc/internal/checker/checker.go:Checker.getTypeOfExpression
    pub(crate) fn get_type_of_expression(&mut self, node: NodeId) -> Result<TypeId, Error> {
        if let Some(ty) = self.quick_type_of_expression(node)? {
            return Ok(ty);
        }
        if let Some(&ty) = self.flow.expression_cache.get(&node) {
            return Ok(ty);
        }
        let before = self.flow.invocation_count;
        let ty = self.check_expression_ex(node, 64)?;
        if before != self.flow.invocation_count {
            self.flow.expression_cache.insert(node, ty);
        }
        Ok(ty)
    }

    // port: tsc/internal/checker/checker.go:Checker.getQuickTypeOfExpression
    pub(crate) fn quick_type_of_expression(
        &mut self,
        node: NodeId,
    ) -> Result<Option<TypeId>, Error> {
        let mut expression = node;
        while self.ast(expression)?.node(expression)?.kind() == K::ParenthesizedExpression {
            expression = required(
                self.ast(expression)?.node(expression)?.expression(),
                "quick parenthesized expression",
            )?;
        }
        let read = self.ast(expression)?.node(expression)?;
        if read.kind() == K::AwaitExpression {
            let operand = required(read.expression(), "quick await operand")?;
            return match self.quick_type_of_expression(operand)? {
                Some(ty) => self.awaited_type(ty),
                None => Ok(None),
            };
        }
        if matches!(
            read.kind().known(),
            Some(K::CallExpression | K::NewExpression)
        ) {
            let construct = read.kind() == K::NewExpression;
            let optional_chain = read.flags() & nf::OPTIONAL_CHAIN != 0;
            let callee = required(read.expression(), "quick call callee")?;
            if !construct
                && (matches!(
                    self.ast(callee)?.node(callee)?.kind().known(),
                    Some(K::SuperKeyword)
                ) || crate::external_resolution::is_import_call(self.ast(expression)?, &read)?
                    || ts_ast::utilities_middle::is_require_call(
                        self.ast(expression)?,
                        &self.ast(expression)?.node(expression)?,
                        true,
                    )?
                    || self.is_symbol_or_symbol_for_call(expression)?)
            {
                return Ok(None);
            }
            let ty = self.check_expression(callee)?;
            let non_optional = if optional_chain {
                self.optional_expression_type(ty, callee)?
            } else {
                ty
            };
            let ty = if optional_chain {
                ty
            } else {
                self.check_non_null_type(ty, callee)?
            };
            // getSingleSignature(t, kind, allowMembers=true): one object type
            // with exactly one signature of the kind and none of the other kind.
            let single = if self.types.flags(ty)? & tf::OBJECT != 0 {
                let calls = self.signatures_of_type(ty, false)?;
                let constructs = self.signatures_of_type(ty, true)?;
                let (wanted, other) = if construct {
                    (constructs, calls)
                } else {
                    (calls, constructs)
                };
                (wanted.len() == 1 && other.is_empty()).then(|| wanted[0])
            } else {
                None
            };
            if let Some(signature) = single.filter(|&signature| {
                self.signatures
                    .get(signature)
                    .is_ok_and(|s| s.type_parameters.as_ref().is_none_or(|p| p.is_empty()))
            }) {
                let result = self.return_type_of_signature(signature)?;
                return if optional_chain {
                    self.propagate_optional_type_marker(result, expression, non_optional != ty)
                        .map(Some)
                } else {
                    Ok(Some(result))
                };
            }
            return Ok(None);
        }
        if matches!(
            read.kind().known(),
            Some(K::AsExpression | K::TypeAssertionExpression)
        ) {
            let annotation = required(read.type_node(), "assertion type")?;
            if ts_ast::utilities_middle::is_const_type_reference(
                self.ast(annotation)?,
                &self.ast(annotation)?.node(annotation)?,
            )? {
                return Ok(None);
            }
            return self.get_type_from_type_node(annotation).map(Some);
        }
        let read = self.ast(node)?.node(node)?;
        if ts_ast::utilities::is_literal_expression(&read)
            || matches!(read.kind().known(), Some(K::TrueKeyword | K::FalseKeyword))
        {
            return self.check_expression(node).map(Some);
        }

        Ok(None)
    }

    // port: tsc/internal/checker/checker.go:Checker.getResolvedSymbol
    pub(crate) fn resolved_value_symbol(&mut self, node: NodeId) -> Result<SymbolId, Error> {
        if let Some(Some(symbol)) = self.query.resolved_symbols.try_get(node) {
            return Ok(*symbol);
        }
        let missing = ts_ast::node_is_missing(Some(&self.ast(node)?.node(node)?));
        let symbol = if missing {
            None
        } else {
            let name = self.ast(node)?.node_text(node)?.into_js_string();
            let message = self.cannot_find_name_diagnostic(node)?;
            let write_only = ts_ast::utilities::is_write_only_access(self.ast(node)?, node)?;
            self.resolve_name(
                Some(node),
                name.as_bytes(),
                sf::VALUE | sf::EXPORT_VALUE,
                Some(message),
                !write_only,
            )?
        }
        .unwrap_or(self.builtins.unknown_symbol);
        *self.query.resolved_symbols.get_or_default(node) = Some(symbol);
        Ok(symbol)
    }

    // port: tsc/internal/checker/checker.go:Checker.checkIdentifier
    pub(crate) fn check_identifier(&mut self, node: NodeId) -> Result<TypeId, Error> {
        self.check_value_identifier(node)
    }

    // port: tsc/internal/checker/checker.go:Checker.checkQualifiedName
    // port: tsc/internal/checker/checker.go:Checker.checkPropertyAccessExpressionOrQualifiedName
    pub(crate) fn check_property_expression(&mut self, node: NodeId) -> Result<TypeId, Error> {
        self.check_property_access(node)
    }

    // port: tsc/internal/checker/checker.go:Checker.getTypeFromTypeQueryNode
    pub(crate) fn source_type_query(&mut self, node: NodeId) -> Result<TypeId, Error> {
        let ty = self.check_instantiation_expression(node)?;
        let ty = self.widened_type(ty)?;
        self.get_regular_type_of_literal_type(ty)
    }

    // port: tsc/internal/checker/checker.go:Checker.checkTruthinessExpression
    pub(crate) fn check_truthiness_expression(&mut self, node: NodeId) -> Result<TypeId, Error> {
        self.check_truthiness_expression_ex(node, 0)
    }

    fn check_truthiness_expression_ex(&mut self, node: NodeId, mode: u32) -> Result<TypeId, Error> {
        let ty = self.check_expression_ex(node, mode)?;
        self.check_truthiness_type(ty, node)?;
        Ok(ty)
    }

    // port: tsc/internal/checker/checker.go:Checker.checkTruthinessOfType
    pub(crate) fn check_truthiness_type(&mut self, ty: TypeId, node: NodeId) -> Result<(), Error> {
        if self.types.flags(ty)? & tf::VOID != 0 {
            self.error_at(
                Some(node),
                ts_diagnostics::An_expression_of_type_void_cannot_be_tested_for_truthiness,
                vec![],
            )?;
        } else {
            let semantics = self.syntactic_truthiness(node)?;
            if semantics != 3 {
                self.error_at(
                    Some(node),
                    if semantics == 1 {
                        ts_diagnostics::This_kind_of_expression_is_always_truthy
                    } else {
                        ts_diagnostics::This_kind_of_expression_is_always_falsy
                    },
                    vec![],
                )?;
            }
        }
        Ok(())
    }

    // port: tsc/internal/checker/checker.go:Checker.getSyntacticTruthySemantics
    fn syntactic_truthiness(&mut self, mut node: NodeId) -> Result<u8, Error> {
        loop {
            let read = self.ast(node)?.node(node)?;
            if matches!(
                read.kind().known(),
                Some(
                    K::ParenthesizedExpression
                        | K::AsExpression
                        | K::TypeAssertionExpression
                        | K::NonNullExpression
                        | K::SatisfiesExpression
                )
            ) {
                node = required(read.expression(), "outer expression")?;
            } else {
                break;
            }
        }
        let read = self.ast(node)?.node(node)?;
        Ok(match read.kind().known() {
            Some(K::NumericLiteral) => {
                if matches!(self.ast(node)?.node_text(node)?.as_bytes(), b"0" | b"1") {
                    3
                } else {
                    1
                }
            }
            Some(
                K::ArrayLiteralExpression
                | K::ArrowFunction
                | K::BigIntLiteral
                | K::ClassExpression
                | K::FunctionExpression
                | K::JsxElement
                | K::JsxSelfClosingElement
                | K::ObjectLiteralExpression
                | K::RegularExpressionLiteral,
            ) => 1,
            Some(K::VoidExpression | K::NullKeyword) => 2,
            Some(K::StringLiteral | K::NoSubstitutionTemplateLiteral) => {
                if self.ast(node)?.node_text(node)?.as_bytes().is_empty() {
                    2
                } else {
                    1
                }
            }
            Some(K::Identifier) => {
                if self.resolved_value_symbol(node)? == self.builtins.undefined_symbol {
                    2
                } else {
                    3
                }
            }
            Some(K::ConditionalExpression) => {
                let data = read
                    .data_source()
                    .as_conditional_expression()
                    .ok_or(ts_arena::Error::InvalidGraph)?;
                let a = required(data.when_true(), "conditional true")?;
                let b = required(data.when_false(), "conditional false")?;
                self.syntactic_truthiness(a)? | self.syntactic_truthiness(b)?
            }
            _ => 3,
        })
    }

    // port: tsc/internal/checker/checker.go:Checker.checkTypeOfExpression
    pub(crate) fn typeof_result_type(&self) -> TypeId {
        self.builtins.typeof_type
    }

    // port: tsc/internal/checker/checker.go:Checker.checkConditionalExpression
    pub(crate) fn check_conditional_expression(&mut self, node: NodeId) -> Result<TypeId, Error> {
        let read = self.ast(node)?.node(node)?;
        let data = read
            .data_source()
            .as_conditional_expression()
            .ok_or(ts_arena::Error::InvalidGraph)?;
        let condition = required(data.condition(), "conditional condition")?;
        let a = required(data.when_true(), "conditional true")?;
        let b = required(data.when_false(), "conditional false")?;
        let mode = self.expression_mode;
        let ty = self.check_truthiness_expression_ex(condition, mode)?;
        self.check_known_truthy_guard(condition, ty, Some(a))?;
        let a = self.check_expression_ex(a, mode)?;
        let b = self.check_expression_ex(b, mode)?;
        self.get_union_type_ex(&[a, b], crate::UnionReduction::Subtype, None, None)
    }

    // port: tsc/internal/checker/checker.go:Checker.checkBinaryLikeExpression
    pub(crate) fn check_binary_expression(&mut self, node: NodeId) -> Result<TypeId, Error> {
        let read = self.ast(node)?.node(node)?;
        let data = read
            .data_source()
            .as_binary_expression()
            .ok_or(ts_arena::Error::InvalidGraph)?;
        let left = required(data.left(), "binary left")?;
        let right = required(data.right(), "binary right")?;
        let op = required(data.operator_token(), "binary operator")?;
        let operator = self.ast(op)?.node(op)?.kind();
        let mode = self.expression_mode;
        if operator == K::EqualsToken
            && matches!(
                self.ast(left)?.node(left)?.kind().known(),
                Some(K::ObjectLiteralExpression | K::ArrayLiteralExpression)
            )
        {
            let source = self.check_expression_ex(right, mode)?;
            return self.check_destructuring_assignment(
                left,
                source,
                mode,
                self.ast(right)?.node(right)?.kind() == K::ThisKeyword,
            );
        }
        let a = self.check_expression_ex(left, mode)?;
        let b = self.check_expression_ex(right, mode)?;
        match operator.known() {
            Some(
                K::AmpersandAmpersandToken
                | K::BarBarToken
                | K::QuestionQuestionToken
                | K::AmpersandAmpersandEqualsToken
                | K::BarBarEqualsToken
                | K::QuestionQuestionEqualsToken,
            ) => self.logical_binary(left, right, operator, a, b),
            Some(
                K::LessThanToken
                | K::GreaterThanToken
                | K::LessThanEqualsToken
                | K::GreaterThanEqualsToken,
            ) => self.relational_binary(node, left, right, operator, a, b),
            Some(K::InstanceOfKeyword) => self.check_instanceof_expression(left, right, a, b, mode),
            Some(K::CommaToken) => self.check_comma_expression(left, right, b),
            Some(K::InKeyword) => self.check_in_expression(left, right, a, b),
            Some(K::EqualsToken) => {
                self.assignment_operator(left, right, operator, a, b)?;
                Ok(b)
            }
            Some(
                K::EqualsEqualsToken
                | K::EqualsEqualsEqualsToken
                | K::ExclamationEqualsToken
                | K::ExclamationEqualsEqualsToken,
            ) => {
                if mode & 64 != 0 {
                    return Ok(self.builtins.boolean_type);
                }
                let equals = matches!(
                    operator.known(),
                    Some(K::EqualsEqualsToken | K::EqualsEqualsEqualsToken)
                );
                if self.object_literal_equality_operand(left)?
                    || self.object_literal_equality_operand(right)?
                {
                    let js = self.ast(left)?.node(left)?.flags()
                        & ts_ast::node_flags::JAVA_SCRIPT_FILE
                        != 0;
                    if !js
                        || matches!(
                            operator.known(),
                            Some(K::EqualsEqualsEqualsToken | K::ExclamationEqualsEqualsToken)
                        )
                    {
                        self.error_at(Some(node),ts_diagnostics::This_condition_will_always_return_0_since_JavaScript_compares_objects_by_reference_not_value,vec![JsString::from_bytes(if equals {b"false".as_slice()}else{b"true".as_slice()})])?;
                    }
                }
                self.check_nan_equality(node, operator, left, right)?;
                let nullable = self.types.flags(a)? & tf::NULLABLE != 0
                    || self.types.flags(b)? & tf::NULLABLE != 0;
                if !nullable && !self.types_comparable(a, b)? {
                    self.report_equality_operator_error(a, b, node)?;
                }
                Ok(self.builtins.boolean_type)
            }
            Some(
                K::PlusToken
                | K::PlusEqualsToken
                | K::MinusToken
                | K::MinusEqualsToken
                | K::AsteriskToken
                | K::AsteriskEqualsToken
                | K::AsteriskAsteriskToken
                | K::AsteriskAsteriskEqualsToken
                | K::SlashToken
                | K::SlashEqualsToken
                | K::PercentToken
                | K::PercentEqualsToken
                | K::LessThanLessThanToken
                | K::LessThanLessThanEqualsToken
                | K::GreaterThanGreaterThanToken
                | K::GreaterThanGreaterThanEqualsToken
                | K::GreaterThanGreaterThanGreaterThanToken
                | K::GreaterThanGreaterThanGreaterThanEqualsToken
                | K::BarToken
                | K::BarEqualsToken
                | K::AmpersandToken
                | K::AmpersandEqualsToken
                | K::CaretToken
                | K::CaretEqualsToken,
            ) => self.arithmetic_binary(node, left, right, op, a, b),
            _ => Err(Error::Unsupported("checkBinaryLikeExpression: operator")),
        }
    }

    /// The equality case of reportOperatorError: literal base types when
    /// those are also unrelated, error-display names, and the await hint.
    // port: tsc/internal/checker/checker.go:Checker.reportOperatorError
    // port: tsc/internal/checker/checker.go:Checker.getBaseTypesIfUnrelated
    // port: tsc/internal/checker/checker.go:Checker.errorAndMaybeSuggestAwait
    fn report_equality_operator_error(
        &mut self,
        left: TypeId,
        right: TypeId,
        node: NodeId,
    ) -> Result<(), Error> {
        let mut would_work_with_await = false;
        if let (Some(awaited_left), Some(awaited_right)) = (
            self.awaited_type_no_alias(left)?,
            self.awaited_type_no_alias(right)?,
        ) {
            would_work_with_await = !(awaited_left == left && awaited_right == right)
                && self.equality_comparable(awaited_left, awaited_right)?;
        }
        let (mut effective_left, mut effective_right) = (left, right);
        if !would_work_with_await {
            let left_base = self.base_literal_type(left)?;
            let right_base = self.base_literal_type(right)?;
            if !self.equality_comparable(left_base, right_base)? {
                effective_left = left_base;
                effective_right = right_base;
            }
        }
        let (left_name, right_name) =
            self.type_names_for_error_display(effective_left, effective_right)?;
        let mut diagnostic = self.diagnostic_for_node(
            Some(node),
            ts_diagnostics::This_comparison_appears_to_be_unintentional_because_the_types_0_and_1_have_no_overlap,
            vec![left_name, right_name],
        )?;
        if would_work_with_await {
            diagnostic
                .related_information
                .push(std::sync::Arc::new(self.diagnostic_for_node(
                    Some(node),
                    ts_diagnostics::Did_you_forget_to_use_await,
                    vec![],
                )?));
        }
        self.add_diagnostic(diagnostic)?;
        Ok(())
    }

    // port: tsc/internal/checker/checker.go:Checker.isTypeEqualityComparableTo
    fn equality_comparable(&mut self, left: TypeId, right: TypeId) -> Result<bool, Error> {
        let comparable = |this: &mut Self, source: TypeId, target: TypeId| -> Result<bool, Error> {
            Ok(this.types.flags(target)? & tf::NULLABLE != 0
                || this.is_type_related_to(
                    source,
                    target,
                    crate::relater::RelationKind::Comparable,
                )?)
        };
        Ok(comparable(self, left, right)? || comparable(self, right, left)?)
    }
}
