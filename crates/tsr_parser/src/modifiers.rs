use crate::{Parser, ParserFactory};
use ts_ast::{node_flags, FactoryMethods, NodeId, NodeListId, SyntaxKind as K};
use ts_core::TextRange;
use ts_diagnostics as diag;

impl<F: ParserFactory> Parser<'_, F> {
    /// port: tsc/internal/parser/parser.go:Parser.parseModifiers
    pub(crate) fn parse_modifiers(&mut self) -> Option<NodeListId> {
        self.parse_modifiers_ex(false, false, false)
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseModifiersEx
    pub(crate) fn parse_modifiers_ex(
        &mut self,
        allow_decorators: bool,
        permit_const: bool,
        stop_static_block: bool,
    ) -> Option<NodeListId> {
        let mut leading_modifier = false;
        let mut trailing_decorator = false;
        let mut trailing_modifier = false;
        let mut static_modifier = false;
        let pos = self.node_pos();
        let mut list = Vec::new();
        loop {
            if allow_decorators && self.token == K::AtToken && !trailing_modifier {
                list.push(self.parse_decorator());
                if leading_modifier {
                    trailing_decorator = true;
                }
            } else {
                let Some(modifier) =
                    self.try_parse_modifier(static_modifier, permit_const, stop_static_block)
                else {
                    break;
                };
                if self.factory.node(modifier).kind() == K::StaticKeyword {
                    static_modifier = true;
                }
                list.push(modifier);
                if trailing_decorator {
                    trailing_modifier = true;
                } else {
                    leading_modifier = true;
                }
            }
        }
        if list.is_empty() {
            None
        } else {
            Some(self.new_modifier_list(TextRange::new(pos, self.node_pos()), list))
        }
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseDecorator
    pub(crate) fn parse_decorator(&mut self) -> NodeId {
        let pos = self.node_pos();
        self.parse_expected(K::AtToken);
        let expression = self.do_in_context(
            node_flags::DECORATOR_CONTEXT,
            true,
            Self::parse_decorator_expression,
        );
        let node = self.factory.new_decorator(Some(expression));
        self.finish_node(node, pos)
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseDecoratorExpression
    pub(crate) fn parse_decorator_expression(&mut self) -> NodeId {
        if self.in_await_context() && self.token == K::AwaitKeyword {
            let pos = self.node_pos();
            let expression =
                self.parse_identifier_with_diagnostic(Some(diag::Expression_expected), None);
            self.next_token();
            let member = self.parse_member_expression_rest(pos, expression, true);
            return self.parse_call_expression_rest(pos, member);
        }
        self.parse_left_hand_side_expression_or_higher()
    }
    /// port: tsc/internal/parser/parser.go:Parser.tryParseModifier
    pub(crate) fn try_parse_modifier(
        &mut self,
        seen_static: bool,
        permit_const: bool,
        stop_static_block: bool,
    ) -> Option<NodeId> {
        let pos = self.node_pos();
        let kind = self.token;
        if self.token == K::ConstKeyword && permit_const {
            if !self.look_ahead(Self::next_token_is_on_same_line_and_can_follow_modifier) {
                return None;
            }
            self.next_token();
        } else if (self.token == K::StaticKeyword
            && (stop_static_block && self.look_ahead(Self::next_token_is_open_brace)
                || seen_static))
            || !self.parse_any_contextual_modifier()
        {
            return None;
        }
        let node = self.factory.new_modifier(kind.into());
        Some(self.finish_node(node, pos))
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseContextualModifier
    pub(crate) fn parse_contextual_modifier(&mut self, token: K) -> bool {
        let state = self.mark();
        if self.token == token && self.next_token_can_follow_modifier() {
            self.commit(state);
            return true;
        }
        self.rewind(state);
        false
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseAnyContextualModifier
    pub(crate) fn parse_any_contextual_modifier(&mut self) -> bool {
        let state = self.mark();
        if ts_ast::is_modifier_kind(self.token.into()) && self.next_token_can_follow_modifier() {
            self.commit(state);
            return true;
        }
        self.rewind(state);
        false
    }
    /// port: tsc/internal/parser/parser.go:Parser.nextTokenCanFollowModifier
    pub(crate) fn next_token_can_follow_modifier(&mut self) -> bool {
        match self.token {
            K::ConstKeyword => self.next_token() == K::EnumKeyword,
            K::ExportKeyword => {
                self.next_token();
                if self.token == K::DefaultKeyword {
                    return self.look_ahead(Self::next_token_can_follow_default_keyword);
                }
                if self.token == K::TypeKeyword {
                    return self.look_ahead(Self::next_token_can_follow_export_modifier);
                }
                self.can_follow_export_modifier()
            }
            K::DefaultKeyword => self.next_token_can_follow_default_keyword(),
            K::StaticKeyword => {
                self.next_token();
                self.can_follow_modifier()
            }
            K::GetKeyword | K::SetKeyword => {
                self.next_token();
                self.can_follow_get_or_set_keyword()
            }
            _ => self.next_token_is_on_same_line_and_can_follow_modifier(),
        }
    }
    /// port: tsc/internal/parser/parser.go:Parser.nextTokenCanFollowDefaultKeyword
    pub(crate) fn next_token_can_follow_default_keyword(&mut self) -> bool {
        match self.next_token() {
            K::ClassKeyword | K::FunctionKeyword | K::InterfaceKeyword | K::AtToken => true,
            K::AbstractKeyword => self.look_ahead(Self::next_token_is_class_keyword_on_same_line),
            K::AsyncKeyword => self.look_ahead(Self::next_token_is_function_keyword_on_same_line),
            _ => false,
        }
    }
    /// port: tsc/internal/parser/parser.go:Parser.nextTokenIsIdentifierOrKeywordOrGreaterThan
    pub(crate) fn next_token_is_identifier_or_keyword_or_greater_than(&mut self) -> bool {
        crate::tokens::token_is_identifier_or_keyword_or_greater_than(self.next_token())
    }
    /// port: tsc/internal/parser/parser.go:Parser.nextTokenIsIdentifierOrKeywordOrLiteralOnSameLine
    pub(crate) fn next_token_is_identifier_or_keyword_or_literal_on_same_line(&mut self) -> bool {
        (self.next_token_is_identifier_or_keyword()
            || matches!(
                self.token,
                K::NumericLiteral | K::BigIntLiteral | K::StringLiteral
            ))
            && !self.has_preceding_line_break()
    }
    /// port: tsc/internal/parser/parser.go:Parser.nextTokenIsClassKeywordOnSameLine
    pub(crate) fn next_token_is_class_keyword_on_same_line(&mut self) -> bool {
        self.next_token() == K::ClassKeyword && !self.has_preceding_line_break()
    }
    /// port: tsc/internal/parser/parser.go:Parser.nextTokenIsFunctionKeywordOnSameLine
    pub(crate) fn next_token_is_function_keyword_on_same_line(&mut self) -> bool {
        self.next_token() == K::FunctionKeyword && !self.has_preceding_line_break()
    }
    /// port: tsc/internal/parser/parser.go:Parser.nextTokenCanFollowExportModifier
    pub(crate) fn next_token_can_follow_export_modifier(&mut self) -> bool {
        self.next_token();
        self.can_follow_export_modifier()
    }
    /// port: tsc/internal/parser/parser.go:Parser.canFollowExportModifier
    pub(crate) fn can_follow_export_modifier(&self) -> bool {
        self.token == K::AtToken
            || !matches!(
                self.token,
                K::AsteriskToken | K::AsKeyword | K::OpenBraceToken
            ) && self.can_follow_modifier()
    }
    /// port: tsc/internal/parser/parser.go:Parser.canFollowModifier
    pub(crate) fn can_follow_modifier(&self) -> bool {
        matches!(
            self.token,
            K::OpenBracketToken | K::OpenBraceToken | K::AsteriskToken | K::DotDotDotToken
        ) || self.is_literal_property_name()
    }
    /// port: tsc/internal/parser/parser.go:Parser.canFollowGetOrSetKeyword
    pub(crate) fn can_follow_get_or_set_keyword(&self) -> bool {
        self.token == K::OpenBracketToken || self.is_literal_property_name()
    }
    /// port: tsc/internal/parser/parser.go:Parser.nextTokenIsOnSameLineAndCanFollowModifier
    pub(crate) fn next_token_is_on_same_line_and_can_follow_modifier(&mut self) -> bool {
        self.next_token();
        !self.has_preceding_line_break() && self.can_follow_modifier()
    }
    /// port: tsc/internal/parser/parser.go:Parser.nextTokenIsOpenBrace
    pub(crate) fn next_token_is_open_brace(&mut self) -> bool {
        self.next_token() == K::OpenBraceToken
    }
}
