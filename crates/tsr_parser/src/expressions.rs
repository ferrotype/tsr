use crate::tokens::{is_keyword_or_punctuation, token_text};
use crate::{Parser, ParserFactory};
use ts_ast::{node_flags, FactoryMethods, NodeId, SyntaxKind};
use ts_core::LanguageVariant;
use ts_diagnostics as diagnostics;

impl<F: ParserFactory> Parser<'_, F> {
    /// port: tsc/internal/parser/parser.go:Parser.parseExpression
    pub(crate) fn parse_expression(&mut self) -> NodeId {
        let saved = self.context_flags;
        self.context_flags &= !node_flags::DECORATOR_CONTEXT;
        let pos = self.node_pos();
        let mut expression = self.parse_assignment_expression_or_higher();
        while let Some(operator) = self.parse_optional_token(SyntaxKind::CommaToken) {
            let right = self.parse_assignment_expression_or_higher();
            expression = self.make_binary_expression(expression, operator, right, pos);
        }
        self.context_flags = saved;
        expression
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseExpressionAllowIn
    pub(crate) fn parse_expression_allow_in(&mut self) -> NodeId {
        self.do_in_context(
            node_flags::DISALLOW_IN_CONTEXT,
            false,
            Self::parse_expression,
        )
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseAssignmentExpressionOrHigher
    pub(crate) fn parse_assignment_expression_or_higher(&mut self) -> NodeId {
        self.parse_assignment_expression_or_higher_worker(true)
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseAssignmentExpressionOrHigherWorker
    pub(crate) fn parse_assignment_expression_or_higher_worker(
        &mut self,
        allow_return_type: bool,
    ) -> NodeId {
        crate::recursion::guarded(|| {
            if self.is_yield_expression() {
                return self.parse_yield_expression();
            }
            if let Some(arrow) =
                self.try_parse_parenthesized_arrow_function_expression(allow_return_type)
            {
                return arrow;
            }
            if let Some(arrow) =
                self.try_parse_async_simple_arrow_function_expression(allow_return_type)
            {
                return arrow;
            }
            let pos = self.node_pos();
            let jsdoc = self.jsdoc_scanner_info();
            let expression =
                self.parse_binary_expression_or_higher(ts_ast::operator_precedence::LOWEST);
            if self.factory.node(expression).kind() == SyntaxKind::Identifier
                && self.token == SyntaxKind::EqualsGreaterThanToken
            {
                return self.parse_simple_arrow_function_expression(
                    pos,
                    expression,
                    allow_return_type,
                    jsdoc,
                    None,
                );
            }
            if self.is_left_hand_side_expression(expression)
                && ts_ast::is_assignment_operator(self.re_scan_greater_than_token().into())
            {
                let operator = self.parse_token_node();
                let right = self.parse_assignment_expression_or_higher_worker(allow_return_type);
                return self.make_binary_expression(expression, operator, right, pos);
            }
            self.parse_conditional_expression_rest(expression, pos, allow_return_type)
        })
    }
    /// port: tsc/internal/parser/parser.go:Parser.isYieldExpression
    pub(crate) fn is_yield_expression(&mut self) -> bool {
        self.token == SyntaxKind::YieldKeyword
            && (self.in_yield_context()
                || self
                    .look_ahead(Self::next_token_is_identifier_or_keyword_or_literal_on_same_line))
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseYieldExpression
    pub(crate) fn parse_yield_expression(&mut self) -> NodeId {
        let pos = self.node_pos();
        self.next_token();
        let (asterisk, expression) = if !self.has_preceding_line_break()
            && (self.token == SyntaxKind::AsteriskToken || self.is_start_of_expression())
        {
            let asterisk = self.parse_optional_token(SyntaxKind::AsteriskToken);
            (asterisk, Some(self.parse_assignment_expression_or_higher()))
        } else {
            (None, None)
        };
        let node = self.factory.new_yield_expression(asterisk, expression);
        self.finish_node(node, pos)
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseConditionalExpressionRest
    pub(crate) fn parse_conditional_expression_rest(
        &mut self,
        left: NodeId,
        pos: i64,
        allow_return_type: bool,
    ) -> NodeId {
        let Some(question) = self.parse_optional_token(SyntaxKind::QuestionToken) else {
            return left;
        };
        let when_true = self.do_in_context(node_flags::DISALLOW_IN_CONTEXT, false, |p| {
            p.parse_assignment_expression_or_higher_worker(false)
        });
        let colon = self.parse_expected_token(SyntaxKind::ColonToken);
        let when_false = if self.node_is_missing(Some(colon)) {
            self.create_missing_identifier()
        } else {
            self.parse_assignment_expression_or_higher_worker(allow_return_type)
        };
        let node = self.factory.new_conditional_expression(
            Some(left),
            Some(question),
            Some(when_true),
            Some(colon),
            Some(when_false),
        );
        self.finish_node(node, pos)
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseBinaryExpressionOrHigher
    pub(crate) fn parse_binary_expression_or_higher(&mut self, precedence: i32) -> NodeId {
        crate::recursion::guarded(|| {
            let pos = self.node_pos();
            let left = self.parse_unary_expression_or_higher();
            self.parse_binary_expression_rest(precedence, left, pos)
        })
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseBinaryExpressionRest
    pub(crate) fn parse_binary_expression_rest(
        &mut self,
        precedence: i32,
        mut left: NodeId,
        pos: i64,
    ) -> NodeId {
        let mut last = left;
        loop {
            let operator = self.re_scan_greater_than_token();
            let new_precedence = ts_ast::get_binary_operator_precedence(operator.into());
            if !should_consume_binary_operator(operator, new_precedence, precedence)
                || operator == SyntaxKind::InKeyword && self.in_disallow_in_context()
            {
                break;
            }
            if matches!(
                operator,
                SyntaxKind::AsKeyword | SyntaxKind::SatisfiesKeyword
            ) {
                if self.has_preceding_line_break() {
                    break;
                }
                self.next_token();
                let last_precedence = {
                    let node = self.factory.node(last);
                    if node.kind() == SyntaxKind::BinaryExpression {
                        let operator = node
                            .data_source()
                            .as_binary_expression()
                            .expect("binary kind has binary payload")
                            .operator_token()
                            .expect("parser binary has operator");
                        ts_ast::get_binary_operator_precedence(self.factory.node(operator).kind())
                    } else {
                        ts_ast::operator_precedence::HIGHEST
                    }
                };
                let r#type = self.parse_type();
                left = if operator == SyntaxKind::SatisfiesKeyword {
                    self.make_satisfies_expression(left, r#type)
                } else {
                    self.make_as_expression(left, r#type)
                };
                let next = self.re_scan_greater_than_token();
                if should_consume_binary_operator(
                    next,
                    ts_ast::get_binary_operator_precedence(next.into()),
                    last_precedence,
                ) {
                    break;
                }
            } else {
                let operator = self.parse_token_node();
                let right = self.parse_binary_expression_or_higher(new_precedence);
                left = self.make_binary_expression(left, operator, right, pos);
                last = left;
            }
        }
        left
    }
    /// port: tsc/internal/parser/parser.go:Parser.makeSatisfiesExpression
    pub(crate) fn make_satisfies_expression(
        &mut self,
        expression: NodeId,
        r#type: NodeId,
    ) -> NodeId {
        let pos = self.factory.node(expression).range().pos();
        let node = self
            .factory
            .new_satisfies_expression(Some(expression), Some(r#type));
        self.finish_node(node, pos);
        self.check_js_syntax(node)
    }
    /// port: tsc/internal/parser/parser.go:Parser.makeAsExpression
    pub(crate) fn make_as_expression(&mut self, expression: NodeId, r#type: NodeId) -> NodeId {
        let pos = self.factory.node(expression).range().pos();
        let node = self
            .factory
            .new_as_expression(Some(expression), Some(r#type));
        self.finish_node(node, pos);
        self.check_js_syntax(node)
    }
    /// port: tsc/internal/parser/parser.go:Parser.makeBinaryExpression
    pub(crate) fn make_binary_expression(
        &mut self,
        left: NodeId,
        operator: NodeId,
        right: NodeId,
        pos: i64,
    ) -> NodeId {
        let node =
            self.factory
                .new_binary_expression(None, Some(left), None, Some(operator), Some(right));
        self.finish_node(node, pos)
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseUnaryExpressionOrHigher
    pub(crate) fn parse_unary_expression_or_higher(&mut self) -> NodeId {
        if self.is_update_expression() {
            let pos = self.node_pos();
            let update = self.parse_update_expression();
            if self.token == SyntaxKind::AsteriskAsteriskToken {
                return self.parse_binary_expression_rest(
                    ts_ast::get_binary_operator_precedence(self.token.into()),
                    update,
                    pos,
                );
            }
            return update;
        }
        let unary_operator = self.token;
        let unary = self.parse_simple_unary_expression();
        if self.token == SyntaxKind::AsteriskAsteriskToken {
            let (kind, range) = {
                let node = self.factory.node(unary);
                (node.kind(), node.range())
            };
            let pos = ts_scanner::skip_trivia(self.source_text, range.pos());
            if kind == SyntaxKind::TypeAssertionExpression {
                self.parse_error_at(pos, range.end(), diagnostics::A_type_assertion_expression_is_not_allowed_in_the_left_hand_side_of_an_exponentiation_expression_Consider_enclosing_the_expression_in_parentheses, vec![]);
            } else {
                assert!(
                    is_keyword_or_punctuation(unary_operator),
                    "Debug failure. False expression."
                );
                self.parse_error_at(pos, range.end(), diagnostics::An_unary_expression_with_the_0_operator_is_not_allowed_in_the_left_hand_side_of_an_exponentiation_expression_Consider_enclosing_the_expression_in_parentheses, vec![token_text(unary_operator)]);
            }
        }
        unary
    }
    /// port: tsc/internal/parser/parser.go:Parser.isUpdateExpression
    pub(crate) fn is_update_expression(&self) -> bool {
        match self.token {
            SyntaxKind::PlusToken
            | SyntaxKind::MinusToken
            | SyntaxKind::TildeToken
            | SyntaxKind::ExclamationToken
            | SyntaxKind::DeleteKeyword
            | SyntaxKind::TypeOfKeyword
            | SyntaxKind::VoidKeyword
            | SyntaxKind::AwaitKeyword => false,
            SyntaxKind::LessThanToken => self.language_variant == LanguageVariant::JSX,
            _ => true,
        }
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseUpdateExpression
    pub(crate) fn parse_update_expression(&mut self) -> NodeId {
        let pos = self.node_pos();
        if matches!(
            self.token,
            SyntaxKind::PlusPlusToken | SyntaxKind::MinusMinusToken
        ) {
            let operator = self.token;
            self.next_token();
            let operand = self.parse_left_hand_side_expression_or_higher();
            let node = self
                .factory
                .new_prefix_unary_expression(operator.into(), Some(operand));
            return self.finish_node(node, pos);
        }
        if self.language_variant == LanguageVariant::JSX
            && self.token == SyntaxKind::LessThanToken
            && self.look_ahead(Self::next_token_is_identifier_or_keyword_or_greater_than)
        {
            return self
                .parse_jsx_element_or_self_closing_element_or_fragment(true, -1, None, false);
        }
        let expression = self.parse_left_hand_side_expression_or_higher();
        if matches!(
            self.token,
            SyntaxKind::PlusPlusToken | SyntaxKind::MinusMinusToken
        ) && !self.has_preceding_line_break()
        {
            let operator = self.token;
            self.next_token();
            let node = self
                .factory
                .new_postfix_unary_expression(Some(expression), operator.into());
            return self.finish_node(node, pos);
        }
        expression
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseSimpleUnaryExpression
    pub(crate) fn parse_simple_unary_expression(&mut self) -> NodeId {
        crate::recursion::guarded(|| {
            let token = self.token;
            match token {
                SyntaxKind::PlusToken
                | SyntaxKind::MinusToken
                | SyntaxKind::TildeToken
                | SyntaxKind::ExclamationToken => self.parse_prefix_unary_expression(),
                SyntaxKind::DeleteKeyword => self.parse_delete_expression(),
                SyntaxKind::TypeOfKeyword => self.parse_type_of_expression(),
                SyntaxKind::VoidKeyword => self.parse_void_expression(),
                SyntaxKind::LessThanToken if self.language_variant == LanguageVariant::JSX => {
                    self.parse_jsx_element_or_self_closing_element_or_fragment(true, -1, None, true)
                }
                SyntaxKind::LessThanToken => self.parse_type_assertion(),
                SyntaxKind::AwaitKeyword if self.is_await_expression() => {
                    self.parse_await_expression()
                }
                _ => self.parse_update_expression(),
            }
        })
    }
    /// port: tsc/internal/parser/parser.go:Parser.parsePrefixUnaryExpression
    pub(crate) fn parse_prefix_unary_expression(&mut self) -> NodeId {
        let pos = self.node_pos();
        let operator = self.token;
        self.next_token();
        let operand = self.parse_simple_unary_expression();
        let node = self
            .factory
            .new_prefix_unary_expression(operator.into(), Some(operand));
        self.finish_node(node, pos)
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseDeleteExpression
    pub(crate) fn parse_delete_expression(&mut self) -> NodeId {
        let pos = self.node_pos();
        self.next_token();
        let expression = self.parse_simple_unary_expression();
        let node = self.factory.new_delete_expression(Some(expression));
        self.finish_node(node, pos)
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseTypeOfExpression
    pub(crate) fn parse_type_of_expression(&mut self) -> NodeId {
        let pos = self.node_pos();
        self.next_token();
        let expression = self.parse_simple_unary_expression();
        let node = self.factory.new_type_of_expression(Some(expression));
        self.finish_node(node, pos)
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseVoidExpression
    pub(crate) fn parse_void_expression(&mut self) -> NodeId {
        let pos = self.node_pos();
        self.next_token();
        let expression = self.parse_simple_unary_expression();
        let node = self.factory.new_void_expression(Some(expression));
        self.finish_node(node, pos)
    }
    /// port: tsc/internal/parser/parser.go:Parser.isAwaitExpression
    pub(crate) fn is_await_expression(&mut self) -> bool {
        self.token == SyntaxKind::AwaitKeyword
            && (self.in_await_context()
                || self
                    .look_ahead(Self::next_token_is_identifier_or_keyword_or_literal_on_same_line))
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseAwaitExpression
    pub(crate) fn parse_await_expression(&mut self) -> NodeId {
        let pos = self.node_pos();
        self.next_token();
        let expression = self.parse_simple_unary_expression();
        let node = self.factory.new_await_expression(Some(expression));
        self.finish_node(node, pos)
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseTypeAssertion
    pub(crate) fn parse_type_assertion(&mut self) -> NodeId {
        assert!(
            self.language_variant != LanguageVariant::JSX,
            "Debug failure. False expression: Type assertions should never be parsed in JSX; they should be parsed as comparisons or JSX elements/fragments."
        );
        let pos = self.node_pos();
        self.parse_expected(SyntaxKind::LessThanToken);
        let r#type = self.parse_type();
        self.parse_expected(SyntaxKind::GreaterThanToken);
        let expression = self.parse_simple_unary_expression();
        let node = self
            .factory
            .new_type_assertion(Some(r#type), Some(expression));
        self.finish_node(node, pos)
    }
    /// port: tsc/internal/parser/parser.go:Parser.isBinaryOperator
    pub(crate) fn is_binary_operator(&self) -> bool {
        !(self.in_disallow_in_context() && self.token == SyntaxKind::InKeyword)
            && ts_ast::get_binary_operator_precedence(self.token.into())
                != ts_ast::operator_precedence::INVALID
    }
    /// port: tsc/internal/parser/parser.go:Parser.isStartOfExpression
    pub(crate) fn is_start_of_expression(&mut self) -> bool {
        self.is_start_of_left_hand_side_expression()
            || matches!(
                self.token,
                SyntaxKind::PlusToken
                    | SyntaxKind::MinusToken
                    | SyntaxKind::TildeToken
                    | SyntaxKind::ExclamationToken
                    | SyntaxKind::DeleteKeyword
                    | SyntaxKind::TypeOfKeyword
                    | SyntaxKind::VoidKeyword
                    | SyntaxKind::PlusPlusToken
                    | SyntaxKind::MinusMinusToken
                    | SyntaxKind::LessThanToken
                    | SyntaxKind::AwaitKeyword
                    | SyntaxKind::YieldKeyword
                    | SyntaxKind::PrivateIdentifier
                    | SyntaxKind::AtToken
            )
            || self.is_binary_operator()
            || self.is_identifier()
    }
    /// port: tsc/internal/parser/parser.go:Parser.isStartOfLeftHandSideExpression
    pub(crate) fn is_start_of_left_hand_side_expression(&mut self) -> bool {
        match self.token {
            SyntaxKind::ThisKeyword
            | SyntaxKind::SuperKeyword
            | SyntaxKind::NullKeyword
            | SyntaxKind::TrueKeyword
            | SyntaxKind::FalseKeyword
            | SyntaxKind::NumericLiteral
            | SyntaxKind::BigIntLiteral
            | SyntaxKind::StringLiteral
            | SyntaxKind::NoSubstitutionTemplateLiteral
            | SyntaxKind::TemplateHead
            | SyntaxKind::OpenParenToken
            | SyntaxKind::OpenBracketToken
            | SyntaxKind::OpenBraceToken
            | SyntaxKind::FunctionKeyword
            | SyntaxKind::ClassKeyword
            | SyntaxKind::NewKeyword
            | SyntaxKind::SlashToken
            | SyntaxKind::SlashEqualsToken
            | SyntaxKind::Identifier => true,
            SyntaxKind::ImportKeyword => self.look_ahead(|p| {
                matches!(
                    p.next_token(),
                    SyntaxKind::OpenParenToken | SyntaxKind::LessThanToken | SyntaxKind::DotToken
                )
            }),
            _ => self.is_identifier(),
        }
    }
    /// port: tsc/internal/parser/parser.go:Parser.isStartOfExpressionStatement
    pub(crate) fn is_start_of_expression_statement(&mut self) -> bool {
        !matches!(
            self.token,
            SyntaxKind::OpenBraceToken
                | SyntaxKind::FunctionKeyword
                | SyntaxKind::ClassKeyword
                | SyntaxKind::AtToken
        ) && self.is_start_of_expression()
    }
    /// port: tsc/internal/ast/utilities.go:NodeIsMissing
    pub(crate) fn node_is_missing(&self, node: Option<NodeId>) -> bool {
        node.is_none_or(|id| {
            let node = self.factory.node(id);
            node.range().is_empty()
                && node.range().pos() >= 0
                && node.kind() != SyntaxKind::EndOfFile
        })
    }
    /// port: tsc/internal/ast/utilities.go:NodeIsPresent
    pub(crate) fn node_is_present(&self, node: Option<NodeId>) -> bool {
        !self.node_is_missing(node)
    }
    /// port: tsc/internal/ast/utilities.go:IsLeftHandSideExpression
    pub(crate) fn is_left_hand_side_expression(&self, mut node: NodeId) -> bool {
        loop {
            let data = self.factory.node(node);
            if data.kind() == SyntaxKind::PartiallyEmittedExpression {
                node = data
                    .data_source()
                    .as_partially_emitted_expression()
                    .expect("partially emitted payload")
                    .expression()
                    .expect("expression exists");
                continue;
            }
            return matches!(
                data.kind().known(),
                Some(
                    SyntaxKind::PropertyAccessExpression
                        | SyntaxKind::ElementAccessExpression
                        | SyntaxKind::NewExpression
                        | SyntaxKind::CallExpression
                        | SyntaxKind::JsxElement
                        | SyntaxKind::JsxSelfClosingElement
                        | SyntaxKind::JsxFragment
                        | SyntaxKind::TaggedTemplateExpression
                        | SyntaxKind::ArrayLiteralExpression
                        | SyntaxKind::ParenthesizedExpression
                        | SyntaxKind::ObjectLiteralExpression
                        | SyntaxKind::ClassExpression
                        | SyntaxKind::FunctionExpression
                        | SyntaxKind::Identifier
                        | SyntaxKind::PrivateIdentifier
                        | SyntaxKind::RegularExpressionLiteral
                        | SyntaxKind::NumericLiteral
                        | SyntaxKind::BigIntLiteral
                        | SyntaxKind::StringLiteral
                        | SyntaxKind::NoSubstitutionTemplateLiteral
                        | SyntaxKind::TemplateExpression
                        | SyntaxKind::FalseKeyword
                        | SyntaxKind::NullKeyword
                        | SyntaxKind::ThisKeyword
                        | SyntaxKind::TrueKeyword
                        | SyntaxKind::SuperKeyword
                        | SyntaxKind::NonNullExpression
                        | SyntaxKind::ExpressionWithTypeArguments
                        | SyntaxKind::MetaProperty
                        | SyntaxKind::ImportKeyword
                        | SyntaxKind::MissingDeclaration
                )
            );
        }
    }
}

/// port: tsc/internal/parser/parser.go:shouldConsumeBinaryOperator
fn should_consume_binary_operator(
    operator: SyntaxKind,
    operator_precedence: i32,
    current_precedence: i32,
) -> bool {
    operator_precedence > current_precedence
        || operator_precedence == current_precedence
            && operator == SyntaxKind::AsteriskAsteriskToken
}
