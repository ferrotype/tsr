use crate::tokens::{token_is_identifier_or_keyword, token_text};
use crate::{Parser, ParserFactory, ParsingContext};
use ts_ast::{JsString, NodeId, NodeListId, SyntaxKind};
use ts_core::TextRange;
use ts_diagnostics as diagnostics;

#[cfg(test)]
#[path = "lists_tests.rs"]
mod tests;

impl<F: ParserFactory> Parser<'_, F> {
    /// port: tsc/internal/parser/parser.go:Parser.parseListIndex
    pub(crate) fn parse_list_index(
        &mut self,
        kind: ParsingContext,
        mut parse_element: impl FnMut(&mut Self, usize) -> NodeId,
    ) -> crate::list_buffer::ListBuffer {
        let saved_contexts = self.parsing_contexts;
        self.parsing_contexts |= 1 << kind as u8;
        let mut outer_reparse_list = std::mem::take(&mut self.reparse_list);
        let mut list = self.factory.start_list_buffer();
        while !self.is_list_terminator(kind) {
            if self.is_list_element(kind, false) {
                let element = parse_element(self, list.len());
                for reparsed in self.reparse_list.drain(..) {
                    let node = self.factory.node(reparsed);
                    if (ts_ast::is_js_type_alias_declaration(&node)
                        || ts_ast::is_js_import_declaration(&node))
                        && !matches!(
                            kind,
                            ParsingContext::SourceElements | ParsingContext::BlockStatements
                        )
                    {
                        outer_reparse_list.push(reparsed);
                    } else {
                        list.push(reparsed);
                    }
                }
                list.push(element);
                continue;
            }
            if self.abort_parsing_list_or_move_to_next_token(kind) {
                break;
            }
        }
        self.reparse_list = outer_reparse_list;
        self.parsing_contexts = saved_contexts;
        list
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseList
    pub(crate) fn parse_list(
        &mut self,
        kind: ParsingContext,
        mut parse_element: impl FnMut(&mut Self) -> NodeId,
    ) -> NodeListId {
        let pos = self.node_pos();
        let nodes = self.parse_list_index(kind, |parser, _| parse_element(parser));
        self.new_parsed_node_list(TextRange::new(pos, self.node_pos()), nodes)
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseDelimitedList
    pub(crate) fn parse_delimited_list<N: Into<Option<NodeId>>>(
        &mut self,
        kind: ParsingContext,
        mut parse_element: impl FnMut(&mut Self) -> N,
    ) -> Option<NodeListId> {
        let pos = self.node_pos();
        let saved_contexts = self.parsing_contexts;
        self.parsing_contexts |= 1 << kind as u8;
        let mut list = self.factory.start_list_buffer();
        loop {
            if self.is_list_element(kind, false) {
                let start = self.node_pos();
                let Some(element) = parse_element(self).into() else {
                    self.parsing_contexts = saved_contexts;
                    return None;
                };
                list.push(element);
                if self.parse_optional(SyntaxKind::CommaToken) {
                    continue;
                }
                if self.is_list_terminator(kind) {
                    break;
                }
                if self.token != SyntaxKind::CommaToken && kind == ParsingContext::EnumMembers {
                    self.parse_error_at_current_token(
                        diagnostics::An_enum_member_name_must_be_followed_by_a_or,
                        vec![],
                    );
                } else {
                    self.parse_expected(SyntaxKind::CommaToken);
                }
                if matches!(
                    kind,
                    ParsingContext::ObjectLiteralMembers | ParsingContext::ImportAttributes
                ) && self.token == SyntaxKind::SemicolonToken
                    && !self.has_preceding_line_break()
                {
                    self.next_token();
                }
                if start == self.node_pos() {
                    self.next_token();
                }
                continue;
            }
            if self.is_list_terminator(kind) || self.abort_parsing_list_or_move_to_next_token(kind)
            {
                break;
            }
        }
        self.parsing_contexts = saved_contexts;
        Some(self.new_parsed_node_list(TextRange::new(pos, self.node_pos()), list))
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseBracketedList
    pub(crate) fn parse_bracketed_list<N: Into<Option<NodeId>>>(
        &mut self,
        kind: ParsingContext,
        parse_element: impl FnMut(&mut Self) -> N,
        opening: SyntaxKind,
        closing: SyntaxKind,
    ) -> Option<NodeListId> {
        if self.parse_expected(opening) {
            let result = self.parse_delimited_list(kind, parse_element);
            self.parse_expected(closing);
            result
        } else {
            Some(self.create_missing_list())
        }
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseEmptyNodeList
    pub(crate) fn parse_empty_node_list(&mut self) -> NodeListId {
        self.new_node_list(TextRange::new(self.node_pos(), self.node_pos()), vec![])
    }
    /// port: tsc/internal/parser/parser.go:Parser.createMissingList
    pub(crate) fn create_missing_list(&mut self) -> NodeListId {
        let list = self.parse_empty_node_list();
        self.factory.mark_list_missing(list);
        list
    }
    /// port: tsc/internal/parser/parser.go:Parser.abortParsingListOrMoveToNextToken
    pub(crate) fn abort_parsing_list_or_move_to_next_token(
        &mut self,
        kind: ParsingContext,
    ) -> bool {
        self.parsing_context_errors(kind);
        if self.is_in_some_parsing_context() {
            return true;
        }
        self.next_token();
        false
    }
    /// port: tsc/internal/parser/parser.go:Parser.isInSomeParsingContext
    pub(crate) fn is_in_some_parsing_context(&mut self) -> bool {
        assert!(
            self.parsing_contexts != 0,
            "Debug failure. False expression: Missing parsing context"
        );
        for kind in ParsingContext::ALL {
            if self.parsing_contexts & (1 << kind as u8) != 0
                && (self.is_list_element(kind, true) || self.is_list_terminator(kind))
            {
                return true;
            }
        }
        false
    }
    /// port: tsc/internal/parser/parser.go:Parser.parsingContextErrors
    pub(crate) fn parsing_context_errors(&mut self, kind: ParsingContext) {
        use ParsingContext as P;
        let mut args = vec![];
        let message = match kind {
            P::SourceElements if self.token == SyntaxKind::DefaultKeyword => {
                args.push(JsString::from_bytes(b"export".as_slice()));
                diagnostics::X_0_expected
            }
            P::SourceElements | P::BlockStatements => {
                diagnostics::Declaration_or_statement_expected
            }
            P::SwitchClauses => diagnostics::X_case_or_default_expected,
            P::SwitchClauseStatements => diagnostics::Statement_expected,
            P::RestProperties | P::TypeMembers => diagnostics::Property_or_signature_expected,
            P::ClassMembers => {
                diagnostics::Unexpected_token_A_constructor_method_accessor_or_property_was_expected
            }
            P::EnumMembers => diagnostics::Enum_member_expected,
            P::HeritageClauseElement => diagnostics::Expression_expected,
            P::VariableDeclarations if ts_ast::is_keyword_kind(self.token.into()) => {
                args.push(token_text(self.token));
                diagnostics::X_0_is_not_allowed_as_a_variable_declaration_name
            }
            P::VariableDeclarations => diagnostics::Variable_declaration_expected,
            P::ObjectBindingElements => diagnostics::Property_destructuring_pattern_expected,
            P::ArrayBindingElements => diagnostics::Array_element_destructuring_pattern_expected,
            P::ArgumentExpressions => diagnostics::Argument_expression_expected,
            P::ObjectLiteralMembers => diagnostics::Property_assignment_expected,
            P::ArrayLiteralMembers => diagnostics::Expression_or_comma_expected,
            P::Parameters if ts_ast::is_keyword_kind(self.token.into()) => {
                args.push(token_text(self.token));
                diagnostics::X_0_is_not_allowed_as_a_parameter_name
            }
            P::Parameters | P::JSDocParameters => diagnostics::Parameter_declaration_expected,
            P::TypeParameters => diagnostics::Type_parameter_declaration_expected,
            P::TypeArguments => diagnostics::Type_argument_expected,
            P::TupleElementTypes => diagnostics::Type_expected,
            P::HeritageClauses => diagnostics::Unexpected_token_expected,
            P::ImportOrExportSpecifiers if self.token == SyntaxKind::FromKeyword => {
                args.push(JsString::from_bytes(b"}".as_slice()));
                diagnostics::X_0_expected
            }
            P::ImportOrExportSpecifiers | P::JsxAttributes | P::JsxChildren | P::JSDocComment => {
                diagnostics::Identifier_expected
            }
            P::ImportAttributes => diagnostics::Identifier_or_string_literal_expected,
        };
        self.parse_error_at_current_token(message, args);
    }
    /// port: tsc/internal/parser/parser.go:Parser.isListElement
    pub(crate) fn is_list_element(&mut self, kind: ParsingContext, recovery: bool) -> bool {
        use ParsingContext as P;
        use SyntaxKind as K;
        match kind {
            P::SourceElements | P::BlockStatements | P::SwitchClauseStatements => {
                !(self.token == K::SemicolonToken && recovery) && self.is_start_of_statement()
            }
            P::SwitchClauses => matches!(self.token, K::CaseKeyword | K::DefaultKeyword),
            P::TypeMembers => self.look_ahead(Self::scan_type_member_start),
            P::ClassMembers => {
                self.look_ahead(Self::scan_class_member_start)
                    || self.token == K::SemicolonToken && !recovery
            }
            P::EnumMembers => self.token == K::OpenBracketToken || self.is_literal_property_name(),
            P::ObjectLiteralMembers => {
                matches!(
                    self.token,
                    K::OpenBracketToken | K::AsteriskToken | K::DotDotDotToken | K::DotToken
                ) || self.is_literal_property_name()
            }
            P::RestProperties => self.is_literal_property_name(),
            P::ObjectBindingElements => {
                matches!(self.token, K::OpenBracketToken | K::DotDotDotToken)
                    || self.is_literal_property_name()
            }
            P::ImportAttributes => self.is_import_attribute_name(),
            P::HeritageClauseElement => {
                if self.token == K::OpenBraceToken {
                    return self.is_valid_heritage_clause_object_literal();
                }
                let start = if recovery {
                    self.is_identifier()
                } else {
                    self.is_start_of_left_hand_side_expression()
                };
                start && !self.is_heritage_clause_extends_or_implements_keyword()
            }
            P::VariableDeclarations => {
                self.is_binding_identifier_or_private_identifier_or_pattern()
            }
            P::ArrayBindingElements => {
                matches!(self.token, K::CommaToken | K::DotDotDotToken)
                    || self.is_binding_identifier_or_private_identifier_or_pattern()
            }
            P::TypeParameters => {
                matches!(self.token, K::InKeyword | K::ConstKeyword) || self.is_identifier()
            }
            P::ArrayLiteralMembers if matches!(self.token, K::CommaToken | K::DotToken) => true,
            P::ArrayLiteralMembers | P::ArgumentExpressions => {
                self.token == K::DotDotDotToken || self.is_start_of_expression()
            }
            P::Parameters => self.is_start_of_parameter(false),
            P::JSDocParameters => self.is_start_of_parameter(true),
            P::TypeArguments | P::TupleElementTypes => {
                self.token == K::CommaToken || self.is_start_of_type(false)
            }
            P::HeritageClauses => self.is_heritage_clause(),
            P::ImportOrExportSpecifiers => {
                if self.token == K::FromKeyword
                    && self.look_ahead(|p| p.next_token() == K::StringLiteral)
                {
                    return false;
                }
                self.token == K::StringLiteral || token_is_identifier_or_keyword(self.token)
            }
            P::JsxAttributes => {
                token_is_identifier_or_keyword(self.token) || self.token == K::OpenBraceToken
            }
            P::JsxChildren | P::JSDocComment => true,
        }
    }
    /// port: tsc/internal/parser/parser.go:Parser.isListTerminator
    pub(crate) fn is_list_terminator(&mut self, kind: ParsingContext) -> bool {
        use ParsingContext as P;
        use SyntaxKind as K;
        if self.token == K::EndOfFile {
            return true;
        }
        match kind {
            P::BlockStatements
            | P::SwitchClauses
            | P::TypeMembers
            | P::ClassMembers
            | P::EnumMembers
            | P::ObjectLiteralMembers
            | P::ObjectBindingElements
            | P::ImportOrExportSpecifiers
            | P::ImportAttributes => self.token == K::CloseBraceToken,
            P::SwitchClauseStatements => matches!(
                self.token,
                K::CloseBraceToken | K::CaseKeyword | K::DefaultKeyword
            ),
            P::HeritageClauseElement => matches!(
                self.token,
                K::OpenBraceToken | K::ExtendsKeyword | K::ImplementsKeyword
            ),
            P::VariableDeclarations => {
                self.can_parse_semicolon()
                    || matches!(
                        self.token,
                        K::InKeyword | K::OfKeyword | K::EqualsGreaterThanToken
                    )
            }
            P::TypeParameters => matches!(
                self.token,
                K::GreaterThanToken
                    | K::OpenParenToken
                    | K::OpenBraceToken
                    | K::ExtendsKeyword
                    | K::ImplementsKeyword
            ),
            P::ArgumentExpressions => matches!(self.token, K::CloseParenToken | K::SemicolonToken),
            P::ArrayLiteralMembers | P::TupleElementTypes | P::ArrayBindingElements => {
                self.token == K::CloseBracketToken
            }
            P::JSDocParameters | P::Parameters | P::RestProperties => {
                matches!(self.token, K::CloseParenToken | K::CloseBracketToken)
            }
            P::TypeArguments => self.token != K::CommaToken,
            P::HeritageClauses => matches!(self.token, K::OpenBraceToken | K::CloseBraceToken),
            P::JsxAttributes => matches!(self.token, K::GreaterThanToken | K::SlashToken),
            P::JsxChildren => {
                self.token == K::LessThanToken
                    && self.look_ahead(|p| p.next_token() == K::SlashToken)
            }
            _ => false,
        }
    }
}

impl ParsingContext {
    pub(crate) const ALL: [Self; 26] = [
        Self::SourceElements,
        Self::BlockStatements,
        Self::SwitchClauses,
        Self::SwitchClauseStatements,
        Self::TypeMembers,
        Self::ClassMembers,
        Self::EnumMembers,
        Self::HeritageClauseElement,
        Self::VariableDeclarations,
        Self::ObjectBindingElements,
        Self::ArrayBindingElements,
        Self::ArgumentExpressions,
        Self::ObjectLiteralMembers,
        Self::JsxAttributes,
        Self::JsxChildren,
        Self::ArrayLiteralMembers,
        Self::Parameters,
        Self::JSDocParameters,
        Self::RestProperties,
        Self::TypeParameters,
        Self::TypeArguments,
        Self::TupleElementTypes,
        Self::HeritageClauses,
        Self::ImportOrExportSpecifiers,
        Self::ImportAttributes,
        Self::JSDocComment,
    ];
}
