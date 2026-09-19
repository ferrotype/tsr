use crate::tokens::token_is_identifier_or_keyword;
use crate::{Parser, ParserFactory};
use ts_ast::{FactoryMethods, JsString, NodeId, SyntaxKind};
use ts_diagnostics::{self as diagnostics, Message};

impl<F: ParserFactory> Parser<'_, F> {
    pub(crate) fn token_value(&self) -> JsString {
        self.scanner
            .retain_token_value()
            .into_js_string(self.source_owner)
            .expect("scanner token value belongs to parser source")
    }
    /// port: tsc/internal/parser/parser.go:Parser.newIdentifier
    pub(crate) fn new_identifier(&mut self, text: JsString) -> NodeId {
        self.identifier_count = self.identifier_count.wrapping_add(1);
        let is_await = text.as_bytes() == b"await";
        let node = self.factory.new_identifier(text);
        if is_await {
            self.statement_has_await_identifier = true;
        }
        node
    }
    /// port: tsc/internal/parser/parser.go:Parser.createMissingIdentifier
    pub(crate) fn create_missing_identifier(&mut self) -> NodeId {
        let node = self.new_identifier(JsString::default());
        self.finish_node(node, self.node_pos())
    }
    /// port: tsc/internal/parser/parser.go:Parser.parsePrivateIdentifier
    pub(crate) fn parse_private_identifier(&mut self) -> NodeId {
        let pos = self.node_pos();
        let text = self.token_value();
        self.next_token();
        let node = self.factory.new_private_identifier(text);
        self.finish_node(node, pos)
    }
    /// port: tsc/internal/parser/parser.go:Parser.isIdentifier
    pub(crate) fn is_identifier(&self) -> bool {
        if self.token == SyntaxKind::Identifier {
            return true;
        }
        if self.token == SyntaxKind::YieldKeyword && self.in_yield_context()
            || self.token == SyntaxKind::AwaitKeyword && self.in_await_context()
        {
            return false;
        }
        self.token > SyntaxKind::LastReservedWord
    }
    /// port: tsc/internal/parser/parser.go:Parser.isBindingIdentifier
    pub(crate) fn is_binding_identifier(&self) -> bool {
        self.token == SyntaxKind::Identifier || self.token > SyntaxKind::LastReservedWord
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseBindingIdentifier
    pub(crate) fn parse_binding_identifier(&mut self) -> NodeId {
        self.parse_binding_identifier_with_diagnostic(None)
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseBindingIdentifierWithDiagnostic
    pub(crate) fn parse_binding_identifier_with_diagnostic(
        &mut self,
        private_diagnostic: Option<&'static Message>,
    ) -> NodeId {
        let saved = self.statement_has_await_identifier;
        let node = self.create_identifier_with_diagnostic(
            self.is_binding_identifier(),
            None,
            private_diagnostic,
        );
        self.statement_has_await_identifier = saved;
        node
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseOptionalBindingIdentifier
    pub(crate) fn parse_optional_binding_identifier(&mut self) -> Option<NodeId> {
        self.is_binding_identifier()
            .then(|| self.parse_binding_identifier())
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseIdentifierName
    pub(crate) fn parse_identifier_name(&mut self) -> NodeId {
        self.parse_identifier_name_with_diagnostic(None)
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseIdentifierNameWithDiagnostic
    pub(crate) fn parse_identifier_name_with_diagnostic(
        &mut self,
        diagnostic: Option<&'static Message>,
    ) -> NodeId {
        self.create_identifier_with_diagnostic(
            token_is_identifier_or_keyword(self.token),
            diagnostic,
            None,
        )
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseIdentifierNameErrorOnUnicodeEscapeSequence
    pub(crate) fn parse_identifier_name_error_on_unicode_escape_sequence(&mut self) -> NodeId {
        if self.scanner.has_unicode_escape() || self.scanner.has_extended_unicode_escape() {
            self.parse_error_at_current_token(
                diagnostics::Unicode_escape_sequence_cannot_appear_here,
                vec![],
            );
        }
        self.create_identifier(token_is_identifier_or_keyword(self.token))
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseIdentifier
    pub(crate) fn parse_identifier(&mut self) -> NodeId {
        self.parse_identifier_with_diagnostic(None, None)
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseIdentifierWithDiagnostic
    pub(crate) fn parse_identifier_with_diagnostic(
        &mut self,
        diagnostic: Option<&'static Message>,
        private_diagnostic: Option<&'static Message>,
    ) -> NodeId {
        self.create_identifier_with_diagnostic(self.is_identifier(), diagnostic, private_diagnostic)
    }
    /// port: tsc/internal/parser/parser.go:Parser.createIdentifier
    pub(crate) fn create_identifier(&mut self, is_identifier: bool) -> NodeId {
        self.create_identifier_with_diagnostic(is_identifier, None, None)
    }
    /// port: tsc/internal/parser/parser.go:Parser.createIdentifierWithDiagnostic
    pub(crate) fn create_identifier_with_diagnostic(
        &mut self,
        is_identifier: bool,
        diagnostic: Option<&'static Message>,
        private_diagnostic: Option<&'static Message>,
    ) -> NodeId {
        if is_identifier {
            let pos = if self.scanner.has_preceding_jsdoc_leading_asterisks() {
                self.scanner.token_start()
            } else {
                self.node_pos()
            };
            let text = self.token_value();
            self.next_token_without_check();
            let node = self.new_identifier(text);
            return self.finish_node(node, pos);
        }
        if self.token == SyntaxKind::PrivateIdentifier {
            self.parse_error_at_current_token(
                private_diagnostic.unwrap_or(
                    diagnostics::Private_identifiers_are_not_allowed_outside_class_bodies,
                ),
                vec![],
            );
            return self.create_identifier(true);
        }
        let (diagnostic, args) = if let Some(diagnostic) = diagnostic {
            (diagnostic, vec![])
        } else if is_reserved_word(self.token) {
            (
                diagnostics::Identifier_expected_0_is_a_reserved_word_that_cannot_be_used_here,
                vec![JsString::from_bytes(self.scanner.token_text())],
            )
        } else {
            (diagnostics::Identifier_expected, vec![])
        };
        if self.token == SyntaxKind::EndOfFile {
            let pos = self.scanner.token_full_start();
            self.parse_error_at(pos, pos, diagnostic, args);
        } else {
            self.parse_error_at_current_token(diagnostic, args);
        }
        self.create_missing_identifier()
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseKeywordExpression
    pub(crate) fn parse_keyword_expression(&mut self) -> NodeId {
        let pos = self.node_pos();
        let node = self.factory.new_keyword_expression(self.token.into());
        self.next_token();
        self.finish_node(node, pos)
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseLiteralExpression
    pub(crate) fn parse_literal_expression(&mut self) -> NodeId {
        let pos = self.node_pos();
        let text = self.token_value();
        let flags = self.scanner.token_flags();
        let node = match self.token {
            SyntaxKind::StringLiteral => self.factory.new_string_literal(text, flags),
            SyntaxKind::NumericLiteral => self.factory.new_numeric_literal(text, flags),
            SyntaxKind::BigIntLiteral => self.factory.new_big_int_literal(text, flags),
            SyntaxKind::RegularExpressionLiteral => {
                self.factory.new_regular_expression_literal(text, flags)
            }
            SyntaxKind::NoSubstitutionTemplateLiteral => self
                .factory
                .new_no_substitution_template_literal(text, flags),
            _ => panic!("Unhandled case in parseLiteralExpression"),
        };
        self.next_token();
        self.finish_node(node, pos)
    }
    /// port: tsc/internal/parser/parser.go:Parser.isLiteralPropertyName
    pub(crate) fn is_literal_property_name(&self) -> bool {
        token_is_identifier_or_keyword(self.token)
            || matches!(
                self.token,
                SyntaxKind::StringLiteral | SyntaxKind::NumericLiteral | SyntaxKind::BigIntLiteral
            )
    }
    /// port: tsc/internal/parser/parser.go:Parser.isImportAttributeName
    pub(crate) fn is_import_attribute_name(&self) -> bool {
        token_is_identifier_or_keyword(self.token) || self.token == SyntaxKind::StringLiteral
    }
}

/// port: tsc/internal/parser/parser.go:isReservedWord
pub(crate) fn is_reserved_word(kind: SyntaxKind) -> bool {
    (SyntaxKind::FirstReservedWord..=SyntaxKind::LastReservedWord).contains(&kind)
}
