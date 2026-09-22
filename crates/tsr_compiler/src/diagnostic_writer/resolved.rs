//! Borrowed AST diagnostics retain their original identities. Resolution is a
//! presentation operation; neither locations nor chains in the AST are mutated.
use super::{localized, Diagnostic, DiagnosticWriter, File, Result};
use std::{borrow::Cow, sync::Arc};
use tsr_ast::SourceFileRead;
use tsr_core::TextRange;
use tsr_jsstring::{JsString, SourceText};

#[derive(Clone, Copy)]
pub struct AstDiagnostic<'a>(pub &'a Diagnostic);
impl<'a> AstDiagnostic<'a> {
    /// port: tsc/internal/diagnosticwriter/diagnosticwriter.go:WrapASTDiagnostic
    pub fn new(diagnostic: &'a Diagnostic) -> Self {
        Self(diagnostic)
    }
    /// port: tsc/internal/diagnosticwriter/diagnosticwriter.go:ASTDiagnostic.Source
    pub fn source(self) -> &'a [u8] {
        self.0.source.as_bytes()
    }
    /// port: tsc/internal/diagnosticwriter/diagnosticwriter.go:ASTDiagnostic.RelatedInformation
    pub fn related_information(self) -> Vec<Self> {
        self.0.related_information.iter().map(|d| Self(d)).collect()
    }
}
/// Rust uses the same borrowed display representation for both Go wrapper slices.
/// port: tsc/internal/diagnosticwriter/diagnosticwriter.go:WrapASTDiagnostics
/// port: tsc/internal/diagnosticwriter/diagnosticwriter.go:FromASTDiagnostics
pub fn wrap_diagnostics<'a>(diagnostics: &[&'a Diagnostic]) -> Vec<AstDiagnostic<'a>> {
    diagnostics.iter().map(|d| AstDiagnostic::new(d)).collect()
}
/// Widening the display slice retains wrapper identity; only the result slice is allocated.
/// port: tsc/internal/diagnosticwriter/diagnosticwriter.go:ToDiagnostics
pub fn to_diagnostics<'a, 'd>(diagnostics: &'a [AstDiagnostic<'d>]) -> Vec<&'a AstDiagnostic<'d>> {
    diagnostics.iter().collect()
}
#[derive(Clone, Copy, Debug)]
pub struct ResolvedLocation {
    pub loc: TextRange,
    pub use_original: bool,
    pub synthesized: bool,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileKind {
    Source,
    Original,
    Renamed,
}
impl File {
    fn with_text(
        source: &SourceFileRead<'_>,
        name: JsString,
        text: SourceText,
        kind: FileKind,
    ) -> Self {
        Self {
            name,
            lines: tsr_jsstring::line_map::compute_ecma_line_starts(text.as_bytes()),
            text,
            supplemental: source.is_content_mapper_supplemental(),
            stable_identity: kind == FileKind::Source,
            kind,
        }
    }
    /// port: tsc/internal/diagnosticwriter/diagnosticwriter.go:newOriginalTextFile
    pub fn original(source: &SourceFileRead<'_>, name: JsString) -> Self {
        Self::with_text(
            source,
            name,
            SourceText::from_bytes(source.original_text()),
            FileKind::Original,
        )
    }
    pub fn renamed(source: &SourceFileRead<'_>, name: JsString) -> Self {
        Self::with_text(source, name, source.text().clone(), FileKind::Renamed)
    }
    pub fn from_source(source: &SourceFileRead<'_>) -> Self {
        Self::with_text(
            source,
            source.parse_options().file_name.clone(),
            source.text().clone(),
            FileKind::Source,
        )
    }
    pub fn kind(&self) -> FileKind {
        self.kind
    }
    pub fn line_map(&self) -> &[i32] {
        &self.lines
    }
}
impl DiagnosticWriter<'_> {
    /// port: tsc/internal/diagnosticwriter/diagnosticwriter.go:ASTDiagnostic.resolve
    pub fn resolved_location(&self, d: &Diagnostic) -> Result<ResolvedLocation> {
        let mut result = ResolvedLocation {
            loc: d.loc,
            use_original: false,
            synthesized: false,
        };
        let Some(id) = d.file else { return Ok(result) };
        if !d.source.is_empty() {
            result.use_original = true;
            return Ok(result);
        }
        let source = self.source(id)?;
        if let Some(segments) = source.span_map() {
            let (loc, fidelity) =
                tsr_ast::span_map::virtual_to_original_span(Some(segments), d.loc);
            if fidelity == tsr_ast::span_map::Fidelity::None {
                result.synthesized = true;
            } else {
                result.loc = loc;
                result.use_original = true;
            }
        }
        Ok(result)
    }
    /// port: tsc/internal/diagnosticwriter/diagnosticwriter.go:ASTDiagnostic.MessageChain
    pub fn message_chain<'d>(&self, d: &'d Diagnostic) -> Result<Vec<Cow<'d, Diagnostic>>> {
        let mut chain: Vec<_> = d
            .message_chain
            .iter()
            .map(|d| Cow::Borrowed(d.as_ref()))
            .collect();
        if self.resolved_location(d)?.synthesized {
            let source = self.source(d.file.expect("mapped diagnostic source"))?;
            chain.push(Cow::Owned(Diagnostic::compiler(tsr_diagnostics::This_location_is_in_virtual_code_produced_by_the_content_mapper_0_and_has_no_corresponding_location_in_the_original_file,vec![JsString::from_bytes(source.content_mapper())])));
        }
        Ok(chain)
    }
    /// Flatten display chains, including virtual-code notes, without changing
    /// the stored chain or recursing on the native stack.
    /// port: tsc/internal/diagnosticwriter/diagnosticwriter.go:WriteFlattenedDiagnosticMessage
    pub fn flatten(&self, d: &Diagnostic, new_line: &[u8]) -> Result<Vec<u8>> {
        let mut out = localized(d)?;
        enum Task<'a> {
            Node(&'a Diagnostic, usize),
            Note(Box<Diagnostic>, usize),
        }
        let enqueue = |pending: &mut Vec<_>, value: &Diagnostic, level: usize| -> Result<()> {
            if self.resolved_location(value)?.synthesized {
                let source = self.source(value.file.expect("mapped source"))?;
                pending.push(Task::Note(Box::new(Diagnostic::compiler(tsr_diagnostics::This_location_is_in_virtual_code_produced_by_the_content_mapper_0_and_has_no_corresponding_location_in_the_original_file,vec![JsString::from_bytes(source.content_mapper())])),level));
            }
            Ok(())
        };
        let mut pending = Vec::new();
        enqueue(&mut pending, d, 1)?;
        pending.extend(d.message_chain.iter().rev().map(|d| Task::Node(d, 1)));
        while let Some(task) = pending.pop() {
            let (value, level) = match &task {
                Task::Node(d, l) => (*d, *l),
                Task::Note(d, l) => (d.as_ref(), *l),
            };
            out.extend_from_slice(new_line);
            out.extend(std::iter::repeat_n(b' ', level * 2));
            out.extend_from_slice(&localized(value)?);
            if let Task::Node(value, _) = task {
                enqueue(&mut pending, value, level + 1)?;
                pending.extend(
                    value
                        .message_chain
                        .iter()
                        .rev()
                        .map(|d| Task::Node(d, level + 1)),
                );
            }
        }
        Ok(out)
    }
    /// port: tsc/internal/diagnosticwriter/diagnosticwriter.go:FormatDiagnosticsStatusAndTime
    /// port: tsc/internal/diagnosticwriter/diagnosticwriter.go:FormatDiagnosticsStatusWithColorAndTime
    pub fn status(&self, d: &Diagnostic, time: &[u8], color: bool) -> Result<Vec<u8>> {
        let mut out = Vec::new();
        if color {
            out.push(b'[');
            super::styled(&mut out, time, super::GREY, true);
            out.extend_from_slice(b"] ");
        } else {
            out.extend_from_slice(time);
            out.extend_from_slice(b" - ");
        }
        out.extend_from_slice(&self.flatten(d, &self.options.new_line)?);
        Ok(out)
    }
    /// port: tsc/internal/diagnosticwriter/diagnosticwriter.go:CompareASTDiagnostics
    pub fn compare(
        &self,
        a: AstDiagnostic<'_>,
        b: AstDiagnostic<'_>,
    ) -> Result<std::cmp::Ordering> {
        let mut names = std::collections::HashMap::new();
        let mut pending = vec![a.0, b.0];
        while let Some(d) = pending.pop() {
            if let Some(id) = d.file {
                names.insert(id, self.source(id)?.parse_options().file_name.clone());
            }
            pending.extend(
                d.message_chain
                    .iter()
                    .chain(&d.related_information)
                    .map(AsRef::as_ref),
            );
        }
        Ok(tsr_ast::compare_diagnostics(a.0, b.0, &|id| {
            names
                .get(&id)
                .map(JsString::as_bytes)
                .ok_or(tsr_arena::Error::WrongOwner)
        })?)
    }
    pub fn source_file(&self, id: tsr_ast::NodeId) -> Result<Arc<File>> {
        Ok(Arc::new(File::from_source(&self.source(id)?)))
    }
}
/// The output buffer is supplied so callers can distinguish no clear from an
/// empty status message without having to consult diagnostic internals.
/// port: tsc/internal/diagnosticwriter/diagnosticwriter.go:TryClearScreen
pub fn try_clear_screen(
    out: &mut Vec<u8>,
    d: &Diagnostic,
    options: &tsr_core::CompilerOptions,
) -> bool {
    if !options.preserve_watch_output.is_true()
        && !options.extended_diagnostics.is_true()
        && !options.diagnostics.is_true()
        && [
            tsr_diagnostics::Starting_compilation_in_watch_mode.code,
            tsr_diagnostics::File_change_detected_Starting_incremental_compilation.code,
        ]
        .contains(&d.code)
    {
        out.extend_from_slice(b"\x1b[2J\x1b[3J\x1b[H");
        true
    } else {
        false
    }
}
