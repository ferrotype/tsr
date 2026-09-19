use crate::{Parser, ParserFactory};
use std::sync::Arc;
use ts_ast::{Diagnostic, FactoryMethods, JsString, NodeId, SyntaxKind};
use ts_core::TextRange;
use ts_diagnostics::{self as diagnostics, Message};
use ts_scanner::token_to_string;

impl<F: ParserFactory> Parser<'_, F> {
    /// port: tsc/internal/parser/parser.go:Parser.nextToken
    pub(crate) fn next_token(&mut self) -> SyntaxKind {
        if ts_ast::is_keyword_kind(self.token.into())
            && (self.scanner.has_unicode_escape() || self.scanner.has_extended_unicode_escape())
        {
            self.parse_error_at_current_token(
                diagnostics::Keywords_cannot_contain_escape_characters,
                vec![],
            );
        }
        self.next_token_without_check()
    }
    /// port: tsc/internal/parser/parser.go:Parser.nextTokenWithoutCheck
    pub(crate) fn next_token_without_check(&mut self) -> SyntaxKind {
        self.token = self.scan_operation(ts_scanner::Scanner::scan);
        self.token
    }
    /// port: tsc/internal/parser/parser.go:Parser.nextTokenJSDoc
    pub(crate) fn next_token_jsdoc(&mut self) -> SyntaxKind {
        self.token = self.scan_operation(ts_scanner::Scanner::scan_jsdoc_token);
        self.token
    }
    /// port: tsc/internal/parser/parser.go:Parser.nextJSDocCommentTextToken
    pub(crate) fn next_jsdoc_comment_text_token(&mut self, in_backticks: bool) -> SyntaxKind {
        self.token =
            self.scan_operation(|scanner| scanner.scan_jsdoc_comment_text_token(in_backticks));
        self.token
    }
    /// port: tsc/internal/parser/parser.go:Parser.reScanLessThanToken
    pub(crate) fn re_scan_less_than_token(&mut self) -> SyntaxKind {
        self.token = self.scan_operation(ts_scanner::Scanner::rescan_less_than_token);
        self.token
    }
    /// port: tsc/internal/parser/parser.go:Parser.reScanGreaterThanToken
    pub(crate) fn re_scan_greater_than_token(&mut self) -> SyntaxKind {
        self.token = self.scan_operation(ts_scanner::Scanner::rescan_greater_than_token);
        self.token
    }
    /// port: tsc/internal/parser/parser.go:Parser.reScanSlashToken
    pub(crate) fn re_scan_slash_token(&mut self) -> SyntaxKind {
        self.token = self.scan_operation(|scanner| scanner.rescan_slash_token(false));
        self.token
    }
    /// port: tsc/internal/parser/parser.go:Parser.reScanTemplateToken
    pub(crate) fn re_scan_template_token(&mut self, tagged: bool) -> SyntaxKind {
        self.token = self.scan_operation(|scanner| scanner.rescan_template_token(tagged));
        self.token
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseOptional
    pub(crate) fn parse_optional(&mut self, kind: SyntaxKind) -> bool {
        if self.token == kind {
            self.next_token();
            true
        } else {
            false
        }
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseExpected
    pub(crate) fn parse_expected(&mut self, kind: SyntaxKind) -> bool {
        self.parse_expected_with_diagnostic(kind, None, true)
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseExpectedWithoutAdvancing
    pub(crate) fn parse_expected_without_advancing(&mut self, kind: SyntaxKind) -> bool {
        self.parse_expected_with_diagnostic(kind, None, false)
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseExpectedWithDiagnostic
    pub(crate) fn parse_expected_with_diagnostic(
        &mut self,
        kind: SyntaxKind,
        message: Option<&'static Message>,
        should_advance: bool,
    ) -> bool {
        if self.token == kind {
            if should_advance {
                self.next_token();
            }
            return true;
        }
        if let Some(message) = message {
            self.parse_error_at_current_token(message, vec![]);
        } else {
            self.parse_error_at_current_token(diagnostics::X_0_expected, vec![token_text(kind)]);
        }
        false
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseExpectedMatchingBrackets
    pub(crate) fn parse_expected_matching_brackets(
        &mut self,
        open_kind: SyntaxKind,
        close_kind: SyntaxKind,
        open_parsed: bool,
        open_position: i64,
    ) {
        if self.token == close_kind {
            self.next_token();
            return;
        }
        let error = self
            .parse_error_at_current_token(diagnostics::X_0_expected, vec![token_text(close_kind)]);
        if open_parsed {
            if let Some(index) = error {
                self.diagnostics[index]
                    .related_information
                    .push(Arc::new(Diagnostic::new(
                        None,
                        TextRange::new(open_position, open_position),
                        diagnostics::The_parser_expected_to_find_a_1_to_match_the_0_token_here,
                        vec![token_text(open_kind), token_text(close_kind)],
                    )));
            }
        }
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseExpectedJSDoc
    pub(crate) fn parse_expected_jsdoc(&mut self, kind: SyntaxKind) -> bool {
        if self.token == kind {
            self.next_token_jsdoc();
            return true;
        }
        assert!(
            is_keyword_or_punctuation(kind),
            "Invalid JSDoc kind: expected keyword or punctuation"
        );
        self.parse_error_at_current_token(diagnostics::X_0_expected, vec![token_text(kind)]);
        false
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseTokenNode
    pub(crate) fn parse_token_node(&mut self) -> NodeId {
        let pos = self.node_pos();
        let kind = self.token;
        self.next_token();
        let node = self.factory.new_token(kind.into());
        self.finish_node(node, pos)
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseOptionalToken
    pub(crate) fn parse_optional_token(&mut self, kind: SyntaxKind) -> Option<NodeId> {
        (self.token == kind).then(|| self.parse_token_node())
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseExpectedToken
    pub(crate) fn parse_expected_token(&mut self, kind: SyntaxKind) -> NodeId {
        self.parse_optional_token(kind).unwrap_or_else(|| {
            self.parse_error_at_current_token(diagnostics::X_0_expected, vec![token_text(kind)]);
            let node = self.factory.new_token(kind.into());
            self.finish_node(node, self.node_pos())
        })
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseOptionalTokenJSDoc
    pub(crate) fn parse_optional_token_jsdoc(&mut self, kind: SyntaxKind) -> Option<NodeId> {
        self.parse_optional_token(kind)
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseExpectedTokenJSDoc
    pub(crate) fn parse_expected_token_jsdoc(&mut self, kind: SyntaxKind) -> NodeId {
        self.parse_optional_token_jsdoc(kind).unwrap_or_else(|| {
            assert!(
                is_keyword_or_punctuation(kind),
                "expected keyword or punctuation"
            );
            self.parse_error_at_current_token(diagnostics::X_0_expected, vec![token_text(kind)]);
            let node = self.factory.new_token(kind.into());
            self.finish_node(node, self.node_pos())
        })
    }
    /// port: tsc/internal/parser/parser.go:Parser.canParseSemicolon
    pub(crate) fn can_parse_semicolon(&self) -> bool {
        matches!(
            self.token,
            SyntaxKind::SemicolonToken | SyntaxKind::CloseBraceToken | SyntaxKind::EndOfFile
        ) || self.has_preceding_line_break()
    }
    /// port: tsc/internal/parser/parser.go:Parser.tryParseSemicolon
    pub(crate) fn try_parse_semicolon(&mut self) -> bool {
        if !self.can_parse_semicolon() {
            return false;
        }
        if self.token == SyntaxKind::SemicolonToken {
            self.next_token();
        }
        true
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseSemicolon
    pub(crate) fn parse_semicolon(&mut self) -> bool {
        self.try_parse_semicolon() || self.parse_expected(SyntaxKind::SemicolonToken)
    }
}

pub(crate) fn token_text(kind: SyntaxKind) -> JsString {
    JsString::from_bytes(token_to_string(kind).as_bytes())
}
/// port: tsc/internal/parser/utilities.go:isKeywordOrPunctuation
pub(crate) fn is_keyword_or_punctuation(kind: SyntaxKind) -> bool {
    ts_ast::is_keyword_kind(kind.into()) || ts_ast::is_punctuation_kind(kind.into())
}
/// port: tsc/internal/parser/utilities.go:tokenIsIdentifierOrKeyword
pub(crate) fn token_is_identifier_or_keyword(kind: SyntaxKind) -> bool {
    kind >= SyntaxKind::Identifier
}
/// port: tsc/internal/parser/utilities.go:tokenIsIdentifierOrKeywordOrGreaterThan
pub(crate) fn token_is_identifier_or_keyword_or_greater_than(kind: SyntaxKind) -> bool {
    kind == SyntaxKind::GreaterThanToken || token_is_identifier_or_keyword(kind)
}
