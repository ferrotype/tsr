use crate::{parse_flags, Parser, ParserFactory, ParsingContext};
use ts_ast::{node_flags, token_flags, FactoryMethods, NodeId, NodeListId, SyntaxKind};
use ts_core::TextRange;
use ts_diagnostics as diagnostics;

impl<F: ParserFactory> Parser<'_, F> {
    /// port: tsc/internal/parser/parser.go:Parser.parseTemplateExpression
    pub(crate) fn parse_template_expression(&mut self, tagged: bool) -> NodeId {
        let pos = self.node_pos();
        let head = self.parse_template_head(tagged);
        let spans = self.parse_template_spans(tagged);
        let node = self
            .factory
            .new_template_expression(Some(head), Some(spans));
        self.finish_node(node, pos)
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseTemplateSpans
    pub(crate) fn parse_template_spans(&mut self, tagged: bool) -> NodeListId {
        let pos = self.node_pos();
        let mut list = vec![];
        loop {
            let span = self.parse_template_span(tagged);
            list.push(span);
            let literal = self
                .factory
                .node(span)
                .data_source()
                .as_template_span()
                .expect("template span payload")
                .literal()
                .expect("parsed template literal");
            if self.factory.node(literal).kind() != SyntaxKind::TemplateMiddle {
                break;
            }
        }
        self.new_node_list(TextRange::new(pos, self.node_pos()), list)
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseTemplateSpan
    pub(crate) fn parse_template_span(&mut self, tagged: bool) -> NodeId {
        let pos = self.node_pos();
        let expression = self.parse_expression_allow_in();
        let literal = self.parse_literal_of_template_span(tagged);
        let node = self
            .factory
            .new_template_span(Some(expression), Some(literal));
        self.finish_node(node, pos)
    }
    /// port: tsc/internal/parser/parser.go:Parser.parsePrimaryExpression
    pub(crate) fn parse_primary_expression(&mut self) -> NodeId {
        let token = self.token;
        match token {
            SyntaxKind::NoSubstitutionTemplateLiteral => {
                if self.scanner.token_flags() & token_flags::IS_INVALID != 0 {
                    self.re_scan_template_token(false);
                }
                return self.parse_literal_expression();
            }
            SyntaxKind::NumericLiteral | SyntaxKind::BigIntLiteral | SyntaxKind::StringLiteral => {
                return self.parse_literal_expression();
            }
            SyntaxKind::ThisKeyword
            | SyntaxKind::SuperKeyword
            | SyntaxKind::NullKeyword
            | SyntaxKind::TrueKeyword
            | SyntaxKind::FalseKeyword => return self.parse_keyword_expression(),
            SyntaxKind::OpenParenToken => return self.parse_parenthesized_expression(),
            SyntaxKind::OpenBracketToken => return self.parse_array_literal_expression(),
            SyntaxKind::OpenBraceToken => return self.parse_object_literal_expression(),
            SyntaxKind::AsyncKeyword
                if self.look_ahead(|p| {
                    p.next_token() == SyntaxKind::FunctionKeyword && !p.has_preceding_line_break()
                }) =>
            {
                return self.parse_function_expression();
            }
            SyntaxKind::AtToken => return self.parse_decorated_expression(),
            SyntaxKind::ClassKeyword => return self.parse_class_expression(),
            SyntaxKind::FunctionKeyword => return self.parse_function_expression(),
            SyntaxKind::NewKeyword => return self.parse_new_expression_or_new_dot_target(),
            SyntaxKind::SlashToken | SyntaxKind::SlashEqualsToken => {
                if self.re_scan_slash_token() == SyntaxKind::RegularExpressionLiteral {
                    return self.parse_literal_expression();
                }
            }
            SyntaxKind::TemplateHead => return self.parse_template_expression(false),
            SyntaxKind::PrivateIdentifier => return self.parse_private_identifier(),
            _ => {}
        }
        self.parse_identifier_with_diagnostic(Some(diagnostics::Expression_expected), None)
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseParenthesizedExpression
    pub(crate) fn parse_parenthesized_expression(&mut self) -> NodeId {
        let pos = self.node_pos();
        let jsdoc = self.jsdoc_scanner_info();
        self.parse_expected(SyntaxKind::OpenParenToken);
        let expression = self.parse_expression_allow_in();
        self.parse_expected(SyntaxKind::CloseParenToken);
        let node = self.factory.new_parenthesized_expression(Some(expression));
        self.finish_node(node, pos);
        self.with_js_doc(node, jsdoc);
        node
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseArrayLiteralExpression
    pub(crate) fn parse_array_literal_expression(&mut self) -> NodeId {
        let pos = self.node_pos();
        let open_pos = self.scanner.token_start();
        let open = self.parse_expected(SyntaxKind::OpenBracketToken);
        let multiline = self.has_preceding_line_break();
        let elements = self.parse_delimited_list(
            ParsingContext::ArrayLiteralMembers,
            Self::parse_argument_or_array_literal_element,
        );
        self.parse_expected_matching_brackets(
            SyntaxKind::OpenBracketToken,
            SyntaxKind::CloseBracketToken,
            open,
            open_pos,
        );
        let node = self
            .factory
            .new_array_literal_expression(elements, multiline);
        self.finish_node(node, pos)
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseObjectLiteralExpression
    pub(crate) fn parse_object_literal_expression(&mut self) -> NodeId {
        let pos = self.node_pos();
        let open_pos = self.scanner.token_start();
        let open = self.parse_expected(SyntaxKind::OpenBraceToken);
        let multiline = self.has_preceding_line_break();
        let properties = self.parse_delimited_list(
            ParsingContext::ObjectLiteralMembers,
            Self::parse_object_literal_element,
        );
        self.parse_expected_matching_brackets(
            SyntaxKind::OpenBraceToken,
            SyntaxKind::CloseBraceToken,
            open,
            open_pos,
        );
        let node = self
            .factory
            .new_object_literal_expression(properties, multiline);
        self.finish_node(node, pos)
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseObjectLiteralElement
    pub(crate) fn parse_object_literal_element(&mut self) -> NodeId {
        let pos = self.node_pos();
        let jsdoc = self.jsdoc_scanner_info();
        if self.parse_optional(SyntaxKind::DotDotDotToken) {
            let expression = self.parse_assignment_expression_or_higher();
            let node = self.factory.new_spread_assignment(Some(expression));
            self.finish_node(node, pos);
            self.with_js_doc(node, jsdoc);
            return node;
        }
        let modifiers = self.parse_modifiers_ex(true, false, false);
        if self.parse_contextual_modifier(SyntaxKind::GetKeyword) {
            return self.parse_accessor_declaration(
                pos,
                jsdoc,
                modifiers,
                SyntaxKind::GetAccessor,
                parse_flags::NONE,
            );
        }
        if self.parse_contextual_modifier(SyntaxKind::SetKeyword) {
            return self.parse_accessor_declaration(
                pos,
                jsdoc,
                modifiers,
                SyntaxKind::SetAccessor,
                parse_flags::NONE,
            );
        }
        let asterisk = self.parse_optional_token(SyntaxKind::AsteriskToken);
        let identifier = self.is_identifier();
        let name = self.parse_property_name();
        let postfix = self
            .parse_optional_token(SyntaxKind::QuestionToken)
            .or_else(|| self.parse_optional_token(SyntaxKind::ExclamationToken));
        if asterisk.is_some()
            || matches!(
                self.token,
                SyntaxKind::OpenParenToken | SyntaxKind::LessThanToken
            )
        {
            return self
                .parse_method_declaration(pos, jsdoc, modifiers, asterisk, name, postfix, None);
        }
        let node = if identifier && self.token != SyntaxKind::ColonToken {
            let equals = self.parse_optional_token(SyntaxKind::EqualsToken);
            let initializer = equals.map(|_| {
                self.do_in_context(
                    node_flags::DISALLOW_IN_CONTEXT,
                    false,
                    Self::parse_assignment_expression_or_higher,
                )
            });
            self.factory.new_shorthand_property_assignment(
                modifiers,
                Some(name),
                postfix,
                None,
                equals,
                initializer,
            )
        } else {
            self.parse_expected(SyntaxKind::ColonToken);
            let initializer = self.do_in_context(
                node_flags::DISALLOW_IN_CONTEXT,
                false,
                Self::parse_assignment_expression_or_higher,
            );
            self.factory.new_property_assignment(
                modifiers,
                Some(name),
                postfix,
                None,
                Some(initializer),
            )
        };
        self.finish_node(node, pos);
        self.with_js_doc(node, jsdoc);
        node
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseFunctionExpression
    pub(crate) fn parse_function_expression(&mut self) -> NodeId {
        let saved = self.context_flags;
        self.set_context_flags(node_flags::DECORATOR_CONTEXT, false);
        let pos = self.node_pos();
        let jsdoc = self.jsdoc_scanner_info();
        let modifiers = self.parse_modifiers();
        self.parse_expected(SyntaxKind::FunctionKeyword);
        let asterisk = self.parse_optional_token(SyntaxKind::AsteriskToken);
        let generator = asterisk.is_some();
        let asynchronous = self.modifier_list_has_async(modifiers);
        let signature_flags = if generator { parse_flags::YIELD } else { 0 }
            | if asynchronous { parse_flags::AWAIT } else { 0 };
        let context = if generator {
            node_flags::YIELD_CONTEXT
        } else {
            0
        } | if asynchronous {
            node_flags::AWAIT_CONTEXT
        } else {
            0
        };
        let name = if context == 0 {
            self.parse_optional_binding_identifier()
        } else {
            self.do_in_context(context, true, Self::parse_optional_binding_identifier)
        };
        let type_parameters = self.parse_type_parameters();
        let parameters = self.parse_parameters(signature_flags);
        let return_type = self.parse_return_type(SyntaxKind::ColonToken, false);
        let body = self.parse_function_block(signature_flags, None);
        self.context_flags = saved;
        let node = self.factory.new_function_expression(
            modifiers,
            asterisk,
            name,
            type_parameters,
            parameters,
            return_type,
            None,
            Some(body),
        );
        self.finish_node(node, pos);
        self.with_js_doc(node, jsdoc);
        self.check_js_syntax(node)
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseDecoratedExpression
    pub(crate) fn parse_decorated_expression(&mut self) -> NodeId {
        let pos = self.node_pos();
        let jsdoc = self.jsdoc_scanner_info();
        let modifiers = self.parse_modifiers_ex(true, false, false);
        if self.token == SyntaxKind::ClassKeyword {
            return self.parse_class_declaration_or_expression(
                pos,
                jsdoc,
                modifiers,
                SyntaxKind::ClassExpression,
            );
        }
        self.parse_error_at(
            self.node_pos(),
            self.node_pos(),
            diagnostics::Expression_expected,
            vec![],
        );
        let node = self.factory.new_missing_declaration(modifiers);
        self.finish_node(node, pos)
    }
}
