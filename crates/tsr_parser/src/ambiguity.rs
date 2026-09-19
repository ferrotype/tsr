use crate::{parse_flags, Parser, ParserFactory};
use ts_ast::{node_flags, FactoryMethods, NodeId, NodeListId, SyntaxKind};
use ts_core::{LanguageVariant, Tristate};

impl<F: ParserFactory> Parser<'_, F> {
    /// port: tsc/internal/parser/parser.go:Parser.isParenthesizedArrowFunctionExpression
    pub(crate) fn is_parenthesized_arrow_function_expression(&mut self) -> Tristate {
        if matches!(
            self.token,
            SyntaxKind::OpenParenToken | SyntaxKind::LessThanToken | SyntaxKind::AsyncKeyword
        ) {
            let state = self.mark();
            let result = self.next_is_parenthesized_arrow_function_expression();
            self.rewind(state);
            return result;
        }
        if self.token == SyntaxKind::EqualsGreaterThanToken {
            Tristate::TRUE
        } else {
            Tristate::FALSE
        }
    }
    /// port: tsc/internal/parser/parser.go:Parser.nextIsParenthesizedArrowFunctionExpression
    pub(crate) fn next_is_parenthesized_arrow_function_expression(&mut self) -> Tristate {
        use SyntaxKind as K;
        if self.token == K::AsyncKeyword {
            self.next_token();
            if self.has_preceding_line_break()
                || !matches!(self.token, K::OpenParenToken | K::LessThanToken)
            {
                return Tristate::FALSE;
            }
        }
        let first = self.token;
        let second = self.next_token();
        if first == K::OpenParenToken {
            if second == K::CloseParenToken {
                return if matches!(
                    self.next_token(),
                    K::EqualsGreaterThanToken | K::ColonToken | K::OpenBraceToken
                ) {
                    Tristate::TRUE
                } else {
                    Tristate::FALSE
                };
            }
            if matches!(second, K::OpenBracketToken | K::OpenBraceToken) {
                return Tristate::UNKNOWN;
            }
            if second == K::DotDotDotToken {
                return Tristate::TRUE;
            }
            if ts_ast::is_modifier_kind(second.into())
                && second != K::AsyncKeyword
                && self.look_ahead(|p| {
                    p.next_token();
                    p.is_identifier()
                })
            {
                return if self.next_token() == K::AsKeyword {
                    Tristate::FALSE
                } else {
                    Tristate::TRUE
                };
            }
            if !self.is_identifier() && second != K::ThisKeyword {
                return Tristate::FALSE;
            }
            match self.next_token() {
                K::ColonToken => Tristate::TRUE,
                K::QuestionToken => {
                    if matches!(
                        self.next_token(),
                        K::ColonToken | K::CommaToken | K::EqualsToken | K::CloseParenToken
                    ) {
                        Tristate::TRUE
                    } else {
                        Tristate::FALSE
                    }
                }
                K::CommaToken | K::EqualsToken | K::CloseParenToken => Tristate::UNKNOWN,
                _ => Tristate::FALSE,
            }
        } else {
            assert!(
                first == K::LessThanToken,
                "Debug failure. False expression."
            );
            if !self.is_identifier() && self.token != K::ConstKeyword {
                return Tristate::FALSE;
            }
            if self.language_variant == LanguageVariant::JSX {
                let arrow = self.look_ahead(|p| {
                    p.parse_optional(K::ConstKeyword);
                    match p.next_token() {
                        K::ExtendsKeyword => !matches!(
                            p.next_token(),
                            K::EqualsToken | K::GreaterThanToken | K::SlashToken
                        ),
                        K::CommaToken | K::EqualsToken => true,
                        _ => false,
                    }
                });
                if arrow {
                    Tristate::TRUE
                } else {
                    Tristate::FALSE
                }
            } else {
                Tristate::UNKNOWN
            }
        }
    }
    /// port: tsc/internal/parser/parser.go:Parser.tryParseParenthesizedArrowFunctionExpression
    pub(crate) fn try_parse_parenthesized_arrow_function_expression(
        &mut self,
        allow_return_type: bool,
    ) -> Option<NodeId> {
        let state = self.is_parenthesized_arrow_function_expression();
        if state == Tristate::FALSE {
            return None;
        }
        if state == Tristate::TRUE {
            return self.parse_parenthesized_arrow_function_expression(true, true);
        }
        let state = self.mark();
        let result = self.parse_possible_parenthesized_arrow_function_expression(allow_return_type);
        if result.is_none() {
            self.rewind(state);
        } else {
            self.commit(state);
        }
        result
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseParenthesizedArrowFunctionExpression
    pub(crate) fn parse_parenthesized_arrow_function_expression(
        &mut self,
        allow_ambiguity: bool,
        allow_return_type: bool,
    ) -> Option<NodeId> {
        let pos = self.node_pos();
        let jsdoc = self.jsdoc_scanner_info();
        let modifiers = self.parse_modifiers_for_arrow_function();
        let asynchronous = self.modifier_list_has_async(modifiers);
        let flags = if asynchronous { parse_flags::AWAIT } else { 0 };
        let type_parameters = self.parse_type_parameters();
        let parameters = if self.parse_expected(SyntaxKind::OpenParenToken) {
            let parameters = self.parse_parameters_worker(flags, allow_ambiguity);
            if !allow_ambiguity && parameters.is_none() {
                return None;
            }
            if !self.parse_expected(SyntaxKind::CloseParenToken) && !allow_ambiguity {
                return None;
            }
            parameters
        } else {
            if !allow_ambiguity {
                return None;
            }
            Some(self.create_missing_list())
        };
        let has_return_colon = self.token == SyntaxKind::ColonToken;
        let return_type = self.parse_return_type(SyntaxKind::ColonToken, false);
        if return_type.is_some_and(|node| {
            !allow_ambiguity && self.type_has_arrow_function_blocking_parse_error(node)
        }) {
            return None;
        }
        // Preserve the source's type assertion traversal, even though the local
        // unwrapped result is presently unused by its following condition.
        let mut unwrapped = return_type;
        while let Some(node) = unwrapped {
            let data = self.factory.node(node);
            if data.kind() != SyntaxKind::ParenthesizedType {
                break;
            }
            unwrapped = data
                .data_source()
                .as_parenthesized_type_node()
                .expect("parenthesized type payload")
                .r#type();
        }
        if !allow_ambiguity
            && !matches!(
                self.token,
                SyntaxKind::EqualsGreaterThanToken | SyntaxKind::OpenBraceToken
            )
        {
            return None;
        }
        let last = self.token;
        let arrow = self.parse_expected_token(SyntaxKind::EqualsGreaterThanToken);
        let body = if matches!(
            last,
            SyntaxKind::EqualsGreaterThanToken | SyntaxKind::OpenBraceToken
        ) {
            self.parse_arrow_function_expression_body(asynchronous, allow_return_type)
        } else {
            self.parse_identifier()
        };
        if !allow_return_type && has_return_colon && self.token != SyntaxKind::ColonToken {
            return None;
        }
        let node = self.factory.new_arrow_function(
            modifiers,
            type_parameters,
            parameters,
            return_type,
            None,
            Some(arrow),
            Some(body),
        );
        self.finish_node(node, pos);
        self.with_js_doc(node, jsdoc);
        self.check_js_syntax(node);
        Some(node)
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseModifiersForArrowFunction
    pub(crate) fn parse_modifiers_for_arrow_function(&mut self) -> Option<NodeListId> {
        if self.token != SyntaxKind::AsyncKeyword {
            return None;
        }
        let pos = self.node_pos();
        self.next_token();
        let modifier = self.factory.new_modifier(SyntaxKind::AsyncKeyword.into());
        self.finish_node(modifier, pos);
        let loc = self.factory.node(modifier).range();
        Some(self.new_modifier_list(loc, vec![modifier]))
    }
    /// port: tsc/internal/parser/parser.go:typeHasArrowFunctionBlockingParseError
    pub(crate) fn type_has_arrow_function_blocking_parse_error(&self, mut node: NodeId) -> bool {
        loop {
            let data = self.factory.node(node);
            let (parameters, r#type) = match data.kind().known() {
                Some(SyntaxKind::TypeReference) => {
                    return self.node_is_missing(
                        data.data_source()
                            .as_type_reference_node()
                            .expect("type reference payload")
                            .type_name(),
                    );
                }
                Some(SyntaxKind::FunctionType) => {
                    let d = data
                        .data_source()
                        .as_function_type_node()
                        .expect("function type payload");
                    (d.parameters(), d.r#type())
                }
                Some(SyntaxKind::ConstructorType) => {
                    let d = data
                        .data_source()
                        .as_constructor_type_node()
                        .expect("constructor type payload");
                    (d.parameters(), d.r#type())
                }
                Some(SyntaxKind::ParenthesizedType) => {
                    node = data
                        .data_source()
                        .as_parenthesized_type_node()
                        .expect("parenthesized type payload")
                        .r#type()
                        .expect("parsed parenthesized type");
                    continue;
                }
                _ => return false,
            };
            if parameters.is_some_and(|list| self.factory.read_list(list).is_missing()) {
                return true;
            }
            node = r#type.expect("parsed function type has return type");
        }
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseArrowFunctionExpressionBody
    pub(crate) fn parse_arrow_function_expression_body(
        &mut self,
        asynchronous: bool,
        allow_return_type: bool,
    ) -> NodeId {
        let flags = if asynchronous { parse_flags::AWAIT } else { 0 };
        if self.token == SyntaxKind::OpenBraceToken {
            return self.parse_function_block(flags, None);
        }
        if !matches!(
            self.token,
            SyntaxKind::SemicolonToken | SyntaxKind::FunctionKeyword | SyntaxKind::ClassKeyword
        ) && self.is_start_of_statement()
            && !self.is_start_of_expression_statement()
        {
            return self.parse_function_block(parse_flags::IGNORE_MISSING_OPEN_BRACE | flags, None);
        }
        let saved = self.context_flags;
        self.set_context_flags(node_flags::AWAIT_CONTEXT, asynchronous);
        self.set_context_flags(node_flags::YIELD_CONTEXT, false);
        let node = self.parse_assignment_expression_or_higher_worker(allow_return_type);
        self.context_flags = saved;
        node
    }
    /// port: tsc/internal/parser/parser.go:Parser.parsePossibleParenthesizedArrowFunctionExpression
    pub(crate) fn parse_possible_parenthesized_arrow_function_expression(
        &mut self,
        allow_return_type: bool,
    ) -> Option<NodeId> {
        let pos = self.scanner.token_start();
        if self.not_parenthesized_arrow.contains(&pos) {
            return None;
        }
        let result = self.parse_parenthesized_arrow_function_expression(false, allow_return_type);
        if result.is_none() {
            self.not_parenthesized_arrow.insert(pos);
        }
        result
    }
    /// port: tsc/internal/parser/parser.go:Parser.tryParseAsyncSimpleArrowFunctionExpression
    pub(crate) fn try_parse_async_simple_arrow_function_expression(
        &mut self,
        allow_return_type: bool,
    ) -> Option<NodeId> {
        if self.token == SyntaxKind::AsyncKeyword
            && self.look_ahead(Self::next_is_un_parenthesized_async_arrow_function)
        {
            let pos = self.node_pos();
            let jsdoc = self.jsdoc_scanner_info();
            let asynchronous = self.parse_modifiers_for_arrow_function();
            let expression =
                self.parse_binary_expression_or_higher(ts_ast::operator_precedence::LOWEST);
            return Some(self.parse_simple_arrow_function_expression(
                pos,
                expression,
                allow_return_type,
                jsdoc,
                asynchronous,
            ));
        }
        None
    }
    /// port: tsc/internal/parser/parser.go:Parser.nextIsUnParenthesizedAsyncArrowFunction
    pub(crate) fn next_is_un_parenthesized_async_arrow_function(&mut self) -> bool {
        if self.token != SyntaxKind::AsyncKeyword {
            return false;
        }
        self.next_token();
        if self.has_preceding_line_break()
            || self.token == SyntaxKind::EqualsGreaterThanToken
            || !self.is_identifier()
        {
            return false;
        }
        self.next_token_without_check();
        !self.has_preceding_line_break() && self.token == SyntaxKind::EqualsGreaterThanToken
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseSimpleArrowFunctionExpression
    pub(crate) fn parse_simple_arrow_function_expression(
        &mut self,
        pos: i64,
        identifier: NodeId,
        allow_return_type: bool,
        jsdoc: u8,
        asynchronous: Option<NodeListId>,
    ) -> NodeId {
        assert!(
            self.token == SyntaxKind::EqualsGreaterThanToken,
            "Debug failure. False expression: parseSimpleArrowFunctionExpression should only have been called if we had a =>"
        );
        let parameter =
            self.factory
                .new_parameter_declaration(None, None, Some(identifier), None, None, None);
        let identifier_pos = self.factory.node(identifier).range().pos();
        self.finish_node(parameter, identifier_pos);
        let parameters = self.new_node_list(self.factory.node(parameter).range(), vec![parameter]);
        let arrow = self.parse_expected_token(SyntaxKind::EqualsGreaterThanToken);
        let body =
            self.parse_arrow_function_expression_body(asynchronous.is_some(), allow_return_type);
        let node = self.factory.new_arrow_function(
            asynchronous,
            None,
            Some(parameters),
            None,
            None,
            Some(arrow),
            Some(body),
        );
        self.finish_node(node, pos);
        self.with_js_doc(node, jsdoc);
        node
    }
}
