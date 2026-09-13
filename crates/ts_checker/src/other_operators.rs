//! Non-arithmetic operator diagnostics use source syntax as well as type facts.
use crate::{type_flags as tf, CheckerState, Error, RelationKind, TypeId};
use ts_arena::NodeId;
use ts_ast::{JsString, SyntaxKind as K};
use ts_diagnostics as d;
fn required<T>(value: Option<T>, name: &'static str) -> Result<T, Error> {
    value.ok_or(Error::MissingLink(name))
}
impl CheckerState {
    // port: tsc/internal/checker/checker.go:Checker.checkInExpression
    pub(crate) fn check_in_expression(
        &mut self,
        left: NodeId,
        right: NodeId,
        a: TypeId,
        b: TypeId,
    ) -> Result<TypeId, Error> {
        if a == self.builtins.silent_never_type || b == self.builtins.silent_never_type {
            return Ok(self.builtins.silent_never_type);
        }
        if self.ast(left)?.node(left)?.kind() == K::PrivateIdentifier {
            self.check_private_in_operand(left, b)?;
        } else {
            let checked = self.check_non_null_type(a, left)?;
            self.check_assignable_at(checked, self.builtins.string_number_symbol_type, left)?;
        }
        let checked = self.check_non_null_type(b, right)?;
        let (related, diagnostic) = self.check_type_related_ex(
            checked,
            self.builtins.non_primitive_type,
            RelationKind::Assignable,
            Some(right),
            None,
        )?;
        if let Some(diagnostic) = diagnostic {
            self.add_diagnostic(diagnostic)?;
        }
        if related && self.any_type(b, &mut |c, t| c.empty_object_intersection(t))? {
            let display = self.type_to_string(b, crate::type_display::DEFAULT_FLAGS)?;
            self.error_at(Some(right),d::Type_0_may_represent_a_primitive_value_which_is_not_permitted_as_the_right_operand_of_the_in_operator,vec![display])?;
        }
        Ok(self.builtins.boolean_type)
    }
    // port: tsc/internal/checker/checker.go:Checker.hasEmptyObjectIntersection
    fn empty_object_intersection(&mut self, ty: TypeId) -> Result<bool, Error> {
        if ty == self.builtins.unknown_empty_object_type {
            return Ok(true);
        }
        if self.types.flags(ty)? & tf::INTERSECTION == 0 {
            return Ok(false);
        }
        let constrained = self.base_constraint_of_type(ty)?.unwrap_or(ty);
        self.is_empty_anonymous_object_type(constrained)
    }
    // port: tsc/internal/checker/checker.go:Checker.checkNullishCoalesceOperands
    pub(crate) fn check_nullish_operands(
        &mut self,
        left: NodeId,
        right: NodeId,
    ) -> Result<(), Error> {
        let parent = required(self.ast(left)?.node(left)?.parent(), "coalescing parent")?;
        let grandparent = self.ast(parent)?.node(parent)?.parent();
        let grand = grandparent
            .map(|node| {
                self.ast(node)?
                    .node(node)
                    .map(|read| read.kind() == K::BinaryExpression)
                    .map_err(Error::from)
            })
            .transpose()?
            .unwrap_or(false);
        if grand {
            let node = grandparent.unwrap();
            let read = self.ast(node)?.node(node)?;
            let data = read
                .data_source()
                .as_binary_expression()
                .ok_or(ts_arena::Error::InvalidGraph)?;
            let operand = required(data.left(), "coalescing enclosing left")?;
            let operator = required(data.operator_token(), "coalescing enclosing operator")?;
            if self.ast(operand)?.node(operand)?.kind() == K::BinaryExpression
                && self.ast(operator)?.node(operator)?.kind() == K::BarBarToken
            {
                self.nullish_mixing_error(operand, b"??", b"||")?;
            }
        } else if self.ast(left)?.node(left)?.kind() == K::BinaryExpression {
            let token = required(
                self.ast(left)?
                    .node(left)?
                    .data_source()
                    .as_binary_expression()
                    .ok_or(ts_arena::Error::InvalidGraph)?
                    .operator_token(),
                "left coalescing operator",
            )?;
            let token = self.ast(token)?.node(token)?.kind();
            if matches!(
                token.known(),
                Some(K::BarBarToken | K::AmpersandAmpersandToken)
            ) {
                self.nullish_mixing_error(
                    left,
                    if token == K::BarBarToken {
                        b"||"
                    } else {
                        b"&&"
                    },
                    b"??",
                )?;
            }
        } else if self.ast(right)?.node(right)?.kind() == K::BinaryExpression {
            let token = required(
                self.ast(right)?
                    .node(right)?
                    .data_source()
                    .as_binary_expression()
                    .ok_or(ts_arena::Error::InvalidGraph)?
                    .operator_token(),
                "right coalescing operator",
            )?;
            if self.ast(token)?.node(token)?.kind() == K::AmpersandAmpersandToken {
                self.nullish_mixing_error(right, b"??", b"&&")?;
            }
        }
        let left = self.skip_operator_outer(left)?;
        match self.syntactic_nullishness(left)? {
            1 => {
                self.error_at(Some(left), d::This_expression_is_always_nullish, vec![])?;
            }
            2 => {
                self.error_at(
                    Some(left),
                    d::Right_operand_of_is_unreachable_because_the_left_operand_is_never_nullish,
                    vec![],
                )?;
            }
            _ => {}
        }
        Ok(())
    }
    fn nullish_mixing_error(&mut self, node: NodeId, a: &[u8], b: &[u8]) -> Result<(), Error> {
        self.grammar_error_node(
            node,
            d::X_0_and_1_operations_cannot_be_mixed_without_parentheses,
            vec![JsString::from_bytes(a), JsString::from_bytes(b)],
        )?;
        Ok(())
    }
    fn skip_operator_outer(&self, mut node: NodeId) -> Result<NodeId, Error> {
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
                        | K::PartiallyEmittedExpression
                )
            ) {
                node = required(read.expression(), "outer operator expression")?;
            } else {
                return Ok(node);
            }
        }
    }
    // port: tsc/internal/checker/checker.go:Checker.getSyntacticNullishnessSemantics
    fn syntactic_nullishness(&mut self, node: NodeId) -> Result<u8, Error> {
        let node = self.skip_operator_outer(node)?;
        let read = self.ast(node)?.node(node)?;
        Ok(match read.kind().known() {
            Some(
                K::AwaitExpression
                | K::CallExpression
                | K::TaggedTemplateExpression
                | K::ElementAccessExpression
                | K::MetaProperty
                | K::NewExpression
                | K::PropertyAccessExpression
                | K::YieldExpression
                | K::ThisKeyword,
            ) => 3,
            Some(K::BinaryExpression) => {
                let data = read
                    .data_source()
                    .as_binary_expression()
                    .ok_or(ts_arena::Error::InvalidGraph)?;
                let left = required(data.left(), "nullish left")?;
                let right = required(data.right(), "nullish right")?;
                let token = required(data.operator_token(), "nullish operator")?;
                match self.ast(token)?.node(token)?.kind().known() {
                    Some(
                        K::BarBarToken
                        | K::BarBarEqualsToken
                        | K::AmpersandAmpersandToken
                        | K::AmpersandAmpersandEqualsToken,
                    ) => 3,
                    Some(K::CommaToken | K::EqualsToken) => self.syntactic_nullishness(right)?,
                    Some(K::QuestionQuestionToken | K::QuestionQuestionEqualsToken) => {
                        let left = self.syntactic_nullishness(left)?;
                        left & 2
                            | if left & 1 != 0 {
                                self.syntactic_nullishness(right)?
                            } else {
                                0
                            }
                    }
                    _ => 2,
                }
            }
            Some(K::ConditionalExpression) => {
                let data = read
                    .data_source()
                    .as_conditional_expression()
                    .ok_or(ts_arena::Error::InvalidGraph)?;
                let a = required(data.when_true(), "nullish true")?;
                let b = required(data.when_false(), "nullish false")?;
                self.syntactic_nullishness(a)? | self.syntactic_nullishness(b)?
            }
            Some(K::NullKeyword) => 1,
            Some(K::Identifier) => {
                if self.resolved_value_symbol(node)? == self.builtins.undefined_symbol {
                    1
                } else {
                    3
                }
            }
            _ => 2,
        })
    }
    // port: tsc/internal/checker/checker.go:Checker.checkBinaryLikeExpression
    pub(crate) fn check_comma_expression(
        &mut self,
        left: NodeId,
        right: NodeId,
        result: TypeId,
    ) -> Result<TypeId, Error> {
        if self.program()?.host.options().allow_unreachable_code != ts_core::Tristate::TRUE
            && self.side_effect_free(left)?
            && !self.indirect_call(left, right)?
        {
            let view = self.ast(left)?;
            let source = ts_ast::utilities::get_source_file_of_node(view, Some(left))?
                .ok_or(Error::MissingLink("comma source"))?;
            let file = view.source_file(source)?;
            let start =
                ts_scanner::skip_trivia(file.text().as_bytes(), view.node(left)?.pos().into());
            let in_jsx_diagnostic = file.diagnostics().iter().any(|diagnostic| {
                diagnostic.code == d::JSX_expressions_must_have_one_parent_element.code
                    && (diagnostic.loc.pos() <= start && start < diagnostic.loc.end())
            });
            if !in_jsx_diagnostic {
                self.error_at(
                    Some(left),
                    d::Left_side_of_comma_operator_is_unused_and_has_no_side_effects,
                    vec![],
                )?;
            }
        }
        Ok(result)
    }
    // port: tsc/internal/checker/checker.go:Checker.isSideEffectFree
    fn side_effect_free(&self, mut node: NodeId) -> Result<bool, Error> {
        while self.ast(node)?.node(node)?.kind() == K::ParenthesizedExpression {
            node = required(
                self.ast(node)?.node(node)?.expression(),
                "side effect parentheses",
            )?;
        }
        let read = self.ast(node)?.node(node)?;
        Ok(match read.kind().known() {
            Some(
                K::Identifier
                | K::StringLiteral
                | K::RegularExpressionLiteral
                | K::TaggedTemplateExpression
                | K::TemplateExpression
                | K::NoSubstitutionTemplateLiteral
                | K::NumericLiteral
                | K::BigIntLiteral
                | K::TrueKeyword
                | K::FalseKeyword
                | K::NullKeyword
                | K::UndefinedKeyword
                | K::FunctionExpression
                | K::ClassExpression
                | K::ArrowFunction
                | K::ArrayLiteralExpression
                | K::ObjectLiteralExpression
                | K::TypeOfExpression
                | K::NonNullExpression
                | K::JsxSelfClosingElement
                | K::JsxElement,
            ) => true,
            Some(K::ConditionalExpression) => {
                let data = read
                    .data_source()
                    .as_conditional_expression()
                    .ok_or(ts_arena::Error::InvalidGraph)?;
                self.side_effect_free(required(data.when_true(), "side effect true")?)?
                    && self.side_effect_free(required(data.when_false(), "side effect false")?)?
            }
            Some(K::BinaryExpression) => {
                let data = read
                    .data_source()
                    .as_binary_expression()
                    .ok_or(ts_arena::Error::InvalidGraph)?;
                !ts_ast::is_assignment_operator(
                    self.ast(node)?
                        .node(required(data.operator_token(), "side effect operator")?)?
                        .kind(),
                ) && self.side_effect_free(required(data.left(), "side effect left")?)?
                    && self.side_effect_free(required(data.right(), "side effect right")?)?
            }
            Some(K::PrefixUnaryExpression) => matches!(
                read.data_source()
                    .as_prefix_unary_expression()
                    .ok_or(ts_arena::Error::InvalidGraph)?
                    .operator()
                    .known(),
                Some(K::ExclamationToken | K::PlusToken | K::MinusToken | K::TildeToken)
            ),
            _ => false,
        })
    }
    // port: tsc/internal/checker/checker.go:Checker.isIndirectCall
    fn indirect_call(&self, left: NodeId, right: NodeId) -> Result<bool, Error> {
        let read = self.ast(left)?.node(left)?;
        if read.kind() != K::NumericLiteral || self.ast(left)?.node_text(left)?.as_bytes() != b"0" {
            return Ok(false);
        }
        let binary = required(read.parent(), "indirect binary")?;
        let Some(parent) = self.ast(binary)?.node(binary)?.parent() else {
            return Ok(false);
        };
        if self.ast(parent)?.node(parent)?.kind() != K::ParenthesizedExpression {
            return Ok(false);
        }
        let Some(call) = self.ast(parent)?.node(parent)?.parent() else {
            return Ok(false);
        };
        let read = self.ast(call)?.node(call)?;
        if !(read.kind() == K::CallExpression && read.expression() == Some(parent)
            || read.kind() == K::TaggedTemplateExpression)
        {
            return Ok(false);
        }
        let read = self.ast(right)?.node(right)?;
        Ok(matches!(
            read.kind().known(),
            Some(K::PropertyAccessExpression | K::ElementAccessExpression)
        ) || read.kind() == K::Identifier
            && self.ast(right)?.node_text(right)?.as_bytes() == b"eval")
    }

    // port: tsc/internal/checker/checker.go:Checker.checkNaNEquality
    pub(crate) fn check_nan_equality(
        &mut self,
        node: NodeId,
        operator: ts_ast::NodeKind,
        left: NodeId,
        right: NodeId,
    ) -> Result<(), Error> {
        let left_nan = self.global_nan_expression(left)?;
        let right_nan = self.global_nan_expression(right)?;
        if !left_nan && !right_nan {
            return Ok(());
        }
        let equals = matches!(
            operator.known(),
            Some(K::EqualsEqualsToken | K::EqualsEqualsEqualsToken)
        );
        let index = self.error_at(
            Some(node),
            d::This_condition_will_always_return_0,
            vec![JsString::from_bytes(if equals {
                b"false".as_slice()
            } else {
                b"true".as_slice()
            })],
        )?;
        if left_nan && right_nan {
            return Ok(());
        }
        let location = if left_nan { right } else { left };
        let mut expression = location;
        while self.ast(expression)?.node(expression)?.kind() == K::ParenthesizedExpression {
            expression = required(
                self.ast(expression)?.node(expression)?.expression(),
                "NaN parentheses",
            )?;
        }
        let name = if ts_ast::is_entity_name_expression(self.ast(expression)?, expression)? {
            self.entity_name_text(expression)?
        } else {
            JsString::from_bytes(b"...".as_slice())
        };
        let mut suggestion = if equals {
            b"Number.isNaN(".to_vec()
        } else {
            b"!Number.isNaN(".to_vec()
        };
        suggestion.extend_from_slice(name.as_bytes());
        suggestion.push(b')');
        if let Some(index) = index {
            let related = self.diagnostic_for_node(
                Some(location),
                d::Did_you_mean_0,
                vec![JsString::from_bytes(suggestion)],
            )?;
            self.add_related_diagnostic(index, related)?;
        }
        Ok(())
    }
    // port: tsc/internal/checker/checker.go:Checker.isGlobalNaN
    fn global_nan_expression(&mut self, mut node: NodeId) -> Result<bool, Error> {
        while self.ast(node)?.node(node)?.kind() == K::ParenthesizedExpression {
            node = required(self.ast(node)?.node(node)?.expression(), "NaN parentheses")?;
        }
        if self.ast(node)?.node(node)?.kind() != K::Identifier
            || self.ast(node)?.node_text(node)?.as_bytes() != b"NaN"
        {
            return Ok(false);
        }
        let global =
            self.lookup_symbol(self.builtins.globals, b"NaN", ts_ast::symbol_flags::VALUE)?;
        Ok(global.is_some() && global == Some(self.resolved_value_symbol(node)?))
    }
    // port: tsc/internal/checker/utilities.go:isLiteralExpressionOfObject
    pub(crate) fn object_literal_equality_operand(&self, node: NodeId) -> Result<bool, Error> {
        Ok(matches!(
            self.ast(node)?.node(node)?.kind().known(),
            Some(
                K::ObjectLiteralExpression
                    | K::ArrayLiteralExpression
                    | K::RegularExpressionLiteral
                    | K::FunctionExpression
                    | K::ClassExpression
            )
        ))
    }
}
