use crate::{Parser, ParserFactory};
use std::sync::OnceLock;
use ts_ast::{Diagnostic, JsString, NodeId, SyntaxKind as K};
use ts_core::TextRange;
use ts_diagnostics::Message;
use ts_scanner::{DiagnosticArgument, Scanner};

/// The scanner calls its error callback while scanning. Its parser callback
/// reads no scanner state and only appends a diagnostic; draining before this
/// operation returns preserves that ordering without a self-referential closure.
impl<'src, F: ParserFactory> Parser<'src, F> {
    pub(crate) fn scan_operation<T>(
        &mut self,
        operation: impl FnOnce(&mut Scanner<'src>) -> T,
    ) -> T {
        let result = operation(&mut self.scanner);
        for diagnostic in self.scanner.drain_diagnostics() {
            let args = diagnostic
                .args
                .into_iter()
                .map(|arg| match arg {
                    DiagnosticArgument::String(bytes) => JsString::from_bytes(bytes),
                    DiagnosticArgument::Integer(number) => {
                        JsString::from_bytes(number.to_string().as_bytes())
                    }
                })
                .collect();
            append_error(
                &mut self.diagnostics,
                &mut self.has_parse_error,
                TextRange::new(
                    diagnostic.start,
                    diagnostic.start.wrapping_add(diagnostic.length),
                ),
                diagnostic.message,
                args,
            );
        }
        result
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseErrorAt
    pub(crate) fn parse_error_at(
        &mut self,
        pos: i64,
        end: i64,
        message: &'static Message,
        args: Vec<JsString>,
    ) -> Option<usize> {
        self.parse_error_at_range(TextRange::new(pos, end), message, args)
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseErrorAtCurrentToken
    pub(crate) fn parse_error_at_current_token(
        &mut self,
        message: &'static Message,
        args: Vec<JsString>,
    ) -> Option<usize> {
        self.parse_error_at_range(self.scanner.token_range(), message, args)
    }
    /// port: tsc/internal/parser/parser.go:Parser.parseErrorAtRange
    pub(crate) fn parse_error_at_range(
        &mut self,
        loc: TextRange,
        message: &'static Message,
        args: Vec<JsString>,
    ) -> Option<usize> {
        append_error(
            &mut self.diagnostics,
            &mut self.has_parse_error,
            loc,
            message,
            args,
        )
    }

    /// port: tsc/internal/parser/parser.go:Parser.parseErrorForMissingSemicolonAfter
    pub(crate) fn parse_error_for_missing_semicolon_after(&mut self, node: NodeId) {
        let node_ref = self.factory.node(node);
        if node_ref.kind() == K::TaggedTemplateExpression {
            let template = node_ref
                .data_source()
                .as_tagged_template_expression()
                .expect("tagged template payload")
                .template()
                .expect("parsed template");
            let loc = self.skip_range_trivia(self.factory.node(template).range());
            drop(node_ref);
            self.parse_error_at_range(
                loc,
                ts_diagnostics::Module_declaration_names_may_only_use_or_quoted_strings,
                vec![],
            );
            return;
        }
        let text = if node_ref.kind() == K::Identifier {
            node_ref
                .data_source()
                .as_identifier()
                .expect("identifier payload")
                .text_owned()
        } else {
            JsString::default()
        };
        let loc = node_ref.range();
        drop(node_ref);
        if text.is_empty() {
            self.parse_error_at_current_token(
                ts_diagnostics::X_0_expected,
                vec![JsString::from_bytes(b";".as_slice())],
            );
            return;
        }
        let pos = ts_scanner::skip_trivia(self.source_text, loc.pos());
        match text.as_bytes() {
            b"const" | b"let" | b"var" => {
                self.parse_error_at(
                    pos,
                    loc.end(),
                    ts_diagnostics::Variable_declaration_not_allowed_at_this_location,
                    vec![],
                );
                return;
            }
            b"declare" => return,
            b"interface" => {
                self.parse_error_for_invalid_name(
                    ts_diagnostics::Interface_name_cannot_be_0,
                    ts_diagnostics::Interface_must_be_given_a_name,
                    K::OpenBraceToken,
                );
                return;
            }
            b"is" => {
                self.parse_error_at(pos, self.scanner.token_start(), ts_diagnostics::A_type_predicate_is_only_allowed_in_return_type_position_for_functions_and_methods, vec![]);
                return;
            }
            b"module" | b"namespace" => {
                self.parse_error_for_invalid_name(
                    ts_diagnostics::Namespace_name_cannot_be_0,
                    ts_diagnostics::Namespace_must_be_given_a_name,
                    K::OpenBraceToken,
                );
                return;
            }
            b"type" => {
                self.parse_error_for_invalid_name(
                    ts_diagnostics::Type_alias_name_cannot_be_0,
                    ts_diagnostics::Type_alias_must_be_given_a_name,
                    K::EqualsToken,
                );
                return;
            }
            _ => {}
        }
        let suggestion = ts_scanner::get_spelling_suggestion_for_strings(
            text.as_bytes(),
            viable_keyword_suggestions()
                .iter()
                .map(|word| word.as_bytes()),
        )
        .map(JsString::from_bytes)
        .or_else(|| get_space_suggestion(text.as_bytes()));
        if let Some(suggestion) = suggestion {
            self.parse_error_at(
                pos,
                loc.end(),
                ts_diagnostics::Unknown_keyword_or_identifier_Did_you_mean_0,
                vec![suggestion],
            );
            return;
        }
        if self.token != K::Unknown {
            self.parse_error_at(
                pos,
                loc.end(),
                ts_diagnostics::Unexpected_keyword_or_identifier,
                vec![],
            );
        }
    }

    /// port: tsc/internal/parser/parser.go:Parser.parseErrorForInvalidName
    pub(crate) fn parse_error_for_invalid_name(
        &mut self,
        name_message: &'static Message,
        blank_message: &'static Message,
        token_if_blank: K,
    ) {
        if self.token == token_if_blank {
            self.parse_error_at_current_token(blank_message, vec![]);
        } else {
            let value = self.token_value();
            self.parse_error_at_current_token(name_message, vec![value]);
        }
    }
}

fn viable_keyword_suggestions() -> &'static [&'static str] {
    static CANDIDATES: OnceLock<Vec<&'static str>> = OnceLock::new();
    CANDIDATES.get_or_init(ts_scanner::get_viable_keyword_suggestions)
}

/// port: tsc/internal/parser/parser.go:getSpaceSuggestion
fn get_space_suggestion(text: &[u8]) -> Option<JsString> {
    // S05 freezes lexical candidate order. The Go map order is nondeterministic;
    // the S06 plan records this diagnostic-only deterministic choice explicitly.
    for keyword in viable_keyword_suggestions() {
        if text.len() > keyword.len() + 2 && text.starts_with(keyword.as_bytes()) {
            let mut suggestion = Vec::with_capacity(text.len() + 1);
            suggestion.extend_from_slice(keyword.as_bytes());
            suggestion.push(b' ');
            suggestion.extend_from_slice(&text[keyword.len()..]);
            return Some(JsString::from_bytes(suggestion));
        }
    }
    None
}

fn append_error(
    diagnostics: &mut Vec<Diagnostic>,
    has_parse_error: &mut bool,
    loc: TextRange,
    message: &'static Message,
    args: Vec<JsString>,
) -> Option<usize> {
    let result = if diagnostics
        .last()
        .is_none_or(|last| last.loc.pos() != loc.pos())
    {
        let index = diagnostics.len();
        diagnostics.push(Diagnostic::new(None, loc, message, args));
        Some(index)
    } else {
        None
    };
    // Suppressing a duplicate does not suppress the pending node error bit.
    *has_parse_error = true;
    result
}
