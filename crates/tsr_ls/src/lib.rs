//! Language service operations over a retained compiler snapshot.
pub mod converters;
mod definition;
mod folding;
mod linked_editing;
mod selection_ranges;
mod semantic_tokens;
mod symbols;
mod syntax;
#[cfg(test)]
mod tests;
pub use folding::FoldingOptions;
pub use semantic_tokens::{TOKEN_MODIFIERS, TOKEN_TYPES};
pub use symbols::workspace_symbols;

use converters::{Converters, Script};
use tsr_ast::{AstView, NodeId, SourceFileRead};
use tsr_compiler::{diagnostic_writer::DiagnosticSources, Program};
use tsr_core::{CancellationToken, TextRange};
use tsr_lsproto as lsp;

#[derive(Debug)]
pub enum Error {
    Compiler(tsr_compiler::Error),
    Checker(tsr_checker::Error),
    Navigation(tsr_astnav::Error),
    Canceled,
    MissingFile(String),
}
impl From<tsr_arena::Error> for Error {
    fn from(value: tsr_arena::Error) -> Self {
        Self::Compiler(value.into())
    }
}
impl From<tsr_compiler::Error> for Error {
    fn from(value: tsr_compiler::Error) -> Self {
        Self::Compiler(value)
    }
}
impl From<tsr_checker::Error> for Error {
    fn from(value: tsr_checker::Error) -> Self {
        Self::Checker(value)
    }
}
impl From<tsr_astnav::Error> for Error {
    fn from(value: tsr_astnav::Error) -> Self {
        Self::Navigation(value)
    }
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Compiler(e) => e.fmt(f),
            Self::Checker(e) => e.fmt(f),
            Self::Navigation(e) => e.fmt(f),
            Self::Canceled => f.write_str("request canceled"),
            Self::MissingFile(name) => write!(f, "file not found: {name}"),
        }
    }
}
impl std::error::Error for Error {}
pub type Result<T> = std::result::Result<T, Error>;

/// One language-service request. The caller retains its program snapshot and
/// checker lease for the entire request; nodes and coordinate maps never cross
/// into a later snapshot. Syntax-only queries do not acquire a checker.
pub struct LanguageService<'a> {
    program: &'a Program,
    converters: Converters,
    cancellation: CancellationToken,
}
impl<'a> LanguageService<'a> {
    pub fn new(
        program: &'a Program,
        encoding: tsr_jsstring::PositionEncoding,
        cancellation: CancellationToken,
    ) -> Self {
        Self {
            program,
            converters: Converters::new(encoding),
            cancellation,
        }
    }
    fn check_canceled(&self) -> Result<()> {
        if self.cancellation.is_canceled() {
            Err(Error::Canceled)
        } else {
            Ok(())
        }
    }
    fn file(&self, uri: &lsp::DocumentUri) -> Result<NodeId> {
        self.program
            .source_file(uri.file_name().as_bytes())
            .map(tsr_compiler::ProgramFile::source)
            .ok_or_else(|| Error::MissingFile(uri.0.clone()))
    }
    fn view(&self, node: NodeId) -> Result<AstView<'a>> {
        self.program
            .file_of_node(node)
            .map(|file| file.bound().view().ast())
            .ok_or_else(|| tsr_arena::Error::WrongOwner.into())
    }
    fn source(&self, source: NodeId) -> Result<SourceFileRead<'a>> {
        Ok(self.program.diagnostic_source(source)?)
    }
    fn range(
        &mut self,
        source: NodeId,
        range: TextRange,
        feature: i32,
    ) -> Result<(lsp::Range, tsr_ast::span_map::Fidelity)> {
        let file = self.source(source)?;
        let original_name = file.original_file_name()?;
        let script = Script {
            file_name: file.file_name(),
            text: file.text().as_bytes(),
            original_file_name: original_name.as_bytes(),
            original_text: file.original_text(),
            span_map: file.span_map(),
        };
        Ok(self
            .converters
            .to_lsp_range_for_feature(&script, range, feature))
    }
    fn unrestricted_range(
        &mut self,
        source: NodeId,
        range: TextRange,
    ) -> Result<(lsp::Range, tsr_ast::span_map::Fidelity)> {
        let file = self.source(source)?;
        let original_name = file.original_file_name()?;
        let script = Script {
            file_name: file.file_name(),
            text: file.text().as_bytes(),
            original_file_name: original_name.as_bytes(),
            original_text: file.original_text(),
            span_map: file.span_map(),
        };
        Ok(self.converters.to_lsp_range(&script, range))
    }
}
