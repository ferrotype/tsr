//! Regular expression literals (`checkRegularExpressionLiteral` in
//! `tsc/internal/checker/checker.go`; `checkGrammarRegularExpressionLiteral` in
//! `grammarchecks.go`). The literal's type is the global `RegExp`; the grammar
//! check re-scans the literal with the scanner's regular expression checker so
//! flag and pattern errors the parser deferred are reported once per node.

use crate::{node_check_flags as nc, CheckerState, Error, TypeId};
use std::sync::{Arc, Mutex};
use ts_arena::NodeId;
use ts_ast::{Diagnostic, SyntaxKind};
use ts_core::TextRange;
use ts_diagnostics::Category;
use ts_jsstring::JsString;
use ts_scanner::{DiagnosticArgument, ScannerDiagnostic};

fn argument(argument: DiagnosticArgument) -> JsString {
    match argument {
        DiagnosticArgument::String(bytes) => JsString::from_bytes(bytes),
        DiagnosticArgument::Integer(value) => JsString::from_bytes(value.to_string().into_bytes()),
    }
}

impl CheckerState {
    // port: tsc/internal/checker/checker.go:Checker.checkRegularExpressionLiteral
    pub(crate) fn check_regular_expression_literal(
        &mut self,
        node: NodeId,
    ) -> Result<TypeId, Error> {
        let flags = self.emit_checks.node_flags.get_or_default(node);
        if *flags & nc::TYPE_CHECKED == 0 {
            *flags |= nc::TYPE_CHECKED;
            self.check_grammar_regular_expression_literal(node)?;
        }
        self.query
            .global_types
            .get("RegExp")
            .copied()
            .ok_or(Error::MissingLink("globalRegExpType"))
    }

    /// Re-scans the literal with errors enabled. Upstream folds a message-category
    /// diagnostic at the same range into the previous error as related
    /// information (spelling suggestions) and drops any other report at the same
    /// position; a file with parse diagnostics is not re-scanned.
    // port: tsc/internal/checker/grammarchecks.go:Checker.checkGrammarRegularExpressionLiteral
    fn check_grammar_regular_expression_literal(&mut self, node: NodeId) -> Result<bool, Error> {
        let source = ts_ast::utilities::get_source_file_of_node(self.ast(node)?, Some(node))?
            .ok_or(Error::MissingLink("regular expression source file"))?;
        let collected: Vec<ScannerDiagnostic> = {
            let view = self.ast(source)?;
            let file = view.source_file(source)?;
            if !file.diagnostics().is_empty() {
                return Ok(false);
            }
            let target = self.program()?.host.options().emit_script_target();
            let variant = file.language_variant;
            let position = i64::from(view.node(node)?.pos());
            let sink = Arc::new(Mutex::new(Vec::new()));
            let collector = Arc::clone(&sink);
            let mut scanner = ts_scanner::Scanner::new();
            scanner.set_script_target(target);
            scanner.set_language_variant(variant);
            scanner.set_on_error(Some(Box::new(move |diagnostic| {
                collector
                    .lock()
                    .expect("regular expression diagnostics")
                    .push(diagnostic);
            })));
            scanner.set_text(view.source().as_bytes());
            scanner.reset_token_state(position);
            scanner.scan();
            let token = scanner.rescan_slash_token(true);
            debug_assert_eq!(token, SyntaxKind::RegularExpressionLiteral);
            drop(scanner);
            let mut collected = sink.lock().expect("regular expression diagnostics");
            std::mem::take(&mut *collected)
        };
        // (stored index, position, length) of the last error added.
        let mut last: Option<(Option<usize>, i64, i64)> = None;
        for diagnostic in collected {
            let range = TextRange::new(diagnostic.start, diagnostic.start + diagnostic.length);
            let args: Vec<JsString> = diagnostic.args.into_iter().map(argument).collect();
            match last {
                Some((index, position, length))
                    if diagnostic.message.category == Category::Message
                        && diagnostic.start == position
                        && diagnostic.length == length =>
                {
                    // For providing spelling suggestions.
                    if let Some(index) = index {
                        self.add_related_diagnostic(
                            index,
                            Diagnostic::new(None, range, diagnostic.message, args),
                        )?;
                    }
                }
                Some((_, position, _)) if diagnostic.start == position => {}
                _ => {
                    let index = self.add_diagnostic(Diagnostic::new(
                        Some(source),
                        range,
                        diagnostic.message,
                        args,
                    ))?;
                    last = Some((index, diagnostic.start, diagnostic.length));
                }
            }
        }
        Ok(last.is_some())
    }
}
