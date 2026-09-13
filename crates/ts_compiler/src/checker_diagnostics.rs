//! Program diagnostic selection and directives surround the checker operation.
use crate::{Error, Program, ProgramFile};
use std::collections::BTreeMap;
use ts_ast::{CommentDirective, CommentDirectiveKind, Diagnostic, SourceFileRead};
use ts_checker::Operation;
use ts_core::{ScriptKind, Tristate};
use ts_jsstring::scanner_positions::compute_line_of_position;

impl Program {
    /// Source selection for semantic and suggestion diagnostics. Project references
    /// are absent because the loader rejects nonempty reference configurations.
    // port: tsc/internal/compiler/program.go:Program.SkipTypeChecking
    // port: tsc/internal/compiler/program.go:Program.canIncludeBindAndCheckDiagnostics
    pub fn skip_type_checking(
        &self,
        file: &ProgramFile,
        ignore_no_check: bool,
    ) -> Result<bool, Error> {
        let source = file.bound().view().source_file()?;
        let retained = self
            .file(source.parse_options().path.as_bytes())
            .ok_or(ts_arena::Error::WrongOwner)?;
        if retained.source() != file.source() {
            return Err(ts_arena::Error::WrongOwner.into());
        }
        let options = self.options();
        if !ignore_no_check && options.no_check.is_true()
            || options.skip_lib_check.is_true() && source.is_declaration_file
            || options.skip_default_lib_check.is_true()
                && self.is_lib(source.parse_options().path.as_bytes())
            || source
                .check_js_directive
                .is_some_and(|directive| !directive.enabled)
        {
            return Ok(true);
        }
        if matches!(source.script_kind, ScriptKind::TS | ScriptKind::TSX) {
            return Ok(false);
        }
        let js = matches!(source.script_kind, ScriptKind::JS | ScriptKind::JSX);
        let checked = source
            .check_js_directive
            .map_or(options.check_js.is_true(), |directive| directive.enabled);
        Ok(!(js && checked
            || ts_ast::utilities_middle::is_plain_js_file(Some(&source), options.check_js)))
    }

    /// Bind and checker diagnostics after per-file selection and directive filtering.
    /// The caller keeps the checker operation so query and checking caches share one generation.
    // port: tsc/internal/compiler/program.go:Program.getBindAndCheckDiagnosticsWithChecker
    pub fn bind_and_check_diagnostics_with_checker(
        &self,
        operation: &mut Operation<'_>,
        file: &ProgramFile,
    ) -> Result<Vec<Diagnostic>, Error> {
        if self.skip_type_checking(file, false)? {
            return Ok(Vec::new());
        }
        let source = file.bound().view().source_file()?;
        let mut diagnostics = source.bind_diagnostics().to_vec();
        diagnostics.extend(operation.semantic_diagnostics(file.source())?);
        if ts_ast::utilities_middle::is_plain_js_file(Some(&source), self.options().check_js) {
            diagnostics
                .retain(|diagnostic| super::plain_js_errors::is_plain_js_error(diagnostic.code));
            return Ok(diagnostics);
        }
        if matches!(source.script_kind, ScriptKind::JS | ScriptKind::JSX)
            && source
                .check_js_directive
                .map_or(self.options().check_js == Tristate::TRUE, |directive| {
                    directive.enabled
                })
        {
            diagnostics.extend_from_slice(source.jsdoc_diagnostics());
        }
        let (mut diagnostics, directives) = preceding_directives(&source, diagnostics)?;
        for directive in directives.into_values() {
            if directive.kind == CommentDirectiveKind::ExpectError {
                diagnostics.push(Diagnostic::new(
                    Some(file.source()),
                    directive.loc,
                    ts_diagnostics::Unused_ts_expect_error_directive,
                    Vec::new(),
                ));
            }
        }
        apply_mapped_directives(file.source(), &source, diagnostics)
    }

    /// Native semantic diagnostics include the include processor's file diagnostics
    /// after checking, while noEmit filtering applies only to bind/check diagnostics.
    // port: tsc/internal/compiler/program.go:Program.getSemanticDiagnosticsWithChecker
    pub fn semantic_diagnostics_with_checker(
        &self,
        operation: &mut Operation<'_>,
        file: &ProgramFile,
    ) -> Result<Vec<Diagnostic>, Error> {
        let mut diagnostics = self.bind_and_check_diagnostics_with_checker(operation, file)?;
        if self.options().no_emit.is_true() {
            diagnostics.retain(|diagnostic| !diagnostic.skipped_on_no_emit);
        }
        if !self.skip_type_checking(file, false)? {
            let source = file.bound().view().source_file()?;
            let include = self
                .include_diagnostics_for_file(source.parse_options().path.as_bytes())?
                .to_vec();
            diagnostics.extend(preceding_directives(&source, include)?.0);
        }
        Ok(diagnostics)
    }
}

// port: tsc/internal/compiler/program.go:Program.getDiagnosticsWithPrecedingDirectives
fn preceding_directives(
    source: &SourceFileRead<'_>,
    diagnostics: Vec<Diagnostic>,
) -> Result<(Vec<Diagnostic>, BTreeMap<isize, CommentDirective>), Error> {
    let directives = source.comment_directives()?;
    let mut by_line = BTreeMap::new();
    if directives.is_empty() {
        return Ok((diagnostics, by_line));
    }
    let starts = source.ecma_line_map();
    for directive in directives.iter() {
        let line = compute_line_of_position(starts, directive.loc.pos() as isize);
        by_line.insert(line, *directive);
    }
    let text = source.text().as_bytes();
    let mut filtered = Vec::with_capacity(diagnostics.len());
    for diagnostic in diagnostics {
        let mut line = compute_line_of_position(starts, diagnostic.loc.pos() as isize) - 1;
        let mut ignored = false;
        while line >= 0 {
            if let Some(directive) = by_line.get_mut(&line) {
                directive.kind = CommentDirectiveKind::Ignore;
                ignored = true;
                break;
            }
            if !comment_or_blank_line(text, starts[line as usize] as usize) {
                break;
            }
            line -= 1;
        }
        if !ignored {
            filtered.push(diagnostic);
        }
    }
    Ok((filtered, by_line))
}

// port: tsc/internal/compiler/program.go:isCommentOrBlankLine
fn comment_or_blank_line(text: &[u8], mut pos: usize) -> bool {
    while matches!(text.get(pos), Some(b' ' | b'\t')) {
        pos += 1;
    }
    pos == text.len()
        || matches!(text.get(pos), Some(b'\r' | b'\n'))
        || text.get(pos..pos.saturating_add(2)) == Some(b"//")
}

// port: tsc/internal/compiler/program.go:applyContentMapperDiagnosticDirectives
fn apply_mapped_directives(
    source_id: ts_ast::NodeId,
    source: &SourceFileRead<'_>,
    diagnostics: Vec<Diagnostic>,
) -> Result<Vec<Diagnostic>, Error> {
    let directives = source.diagnostic_directives()?;
    if directives.is_empty() {
        return Ok(diagnostics);
    }
    let mut used = vec![false; directives.len()];
    let mut filtered = Vec::with_capacity(diagnostics.len());
    for diagnostic in diagnostics {
        let mut suppressed = false;
        if diagnostic.source.is_empty() {
            for (index, directive) in directives.iter().enumerate() {
                if diagnostic.loc.pos() >= directive.virtual_range.pos()
                    && diagnostic.loc.pos() < directive.virtual_range.end()
                {
                    used[index] = true;
                    suppressed = true;
                    break;
                }
            }
        }
        if !suppressed {
            filtered.push(diagnostic);
        }
    }
    for (index, directive) in directives.iter().enumerate() {
        // MappedDiagnosticDirectivePolicyExpect is the pinned wire value 1.
        if directive.policy == 1 && !used[index] {
            filtered.push(Diagnostic::external(
                Some(source_id),
                directive.original_range,
                directive.source.clone(),
                ts_diagnostics::Category::Error as i32,
                directive.unused_code,
                directive.unused_message_text.clone(),
            ));
        }
    }
    Ok(filtered)
}
