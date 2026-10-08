//! Language service operations over a retained compiler snapshot.
mod api;
pub mod api_server;
mod crossproject;
pub use crossproject::{CrossProjectDefinition, CrossProjectPosition, CrossProjectTargets};
#[cfg(test)]
mod auto_import_mapped_tests;
mod auto_imports;
mod autoinsert;
mod call_declarations;
mod call_hierarchy;
mod call_sites;
mod change;
mod change_delete;
mod change_insert;
mod change_nodes;
mod change_positions;
#[cfg(test)]
mod change_tests;
mod code_actions;
mod code_lens;
mod codefix_class;
mod codefix_imports;
mod codefix_isolated;
mod codefix_isolated_edits;
mod codefix_isolated_spreads;
mod codefix_promote;
mod completion_class_snippets;
mod completion_containers;
mod completion_context;
mod completion_imports;
mod completion_items;
mod completion_jsx;
mod completion_keywords;
mod completion_labels;
mod completion_literals;
#[cfg(test)]
mod completion_mapped_tests;
mod completion_paths;
mod completion_snippets;
mod completion_switch;
mod completions;
#[cfg(test)]
mod edit_tests;
mod file_rename;
mod import_adder;
mod jsdoc_completions;
mod jsdoc_parameters;
mod jsdoc_template;
mod snippet_printer;
mod string_completions;
pub use completion_keywords::compare as compare_completion_entries;
pub use completions::{CompletionOptions, COMPLETION_TRIGGER_CHARACTERS};
pub mod converters;
mod definition;
mod display_parts;
mod documentation;
mod folding;
mod format_preferences;
mod formatting;
mod organize_coalesce;
mod organize_compare;
mod organize_imports;
mod organize_unicode;
pub use format_preferences::apply_format_settings;
pub use organize_compare::OrganizeOptions;
pub use organize_imports::OrganizeMode;
mod highlights;
mod hover;
mod hover_display;
mod import_tracker;
mod inlay_hints;
mod inlay_parts;
mod meaning;
mod reference_helpers;
mod reference_special;
mod references;
mod rename;
pub use rename::{RenameInfo, RenameOptions};
mod source_declarations;
mod source_definition;
mod source_map;
mod vs_references;
pub use code_lens::CodeLensOptions;
mod highlight_syntax;
pub use inlay_hints::{InlayHintsOptions, ParameterNameHints, QuotePreference};
pub mod symbol_display;
pub use hover::HoverOptions;
mod linked_editing;
mod selection_ranges;
mod semantic_tokens;
mod signature_arguments;
mod signature_help;
mod symbols;
pub use signature_help::{
    SignatureHelpOptions, SIGNATURE_HELP_RETRIGGER_CHARACTERS, SIGNATURE_HELP_TRIGGER_CHARACTERS,
};
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
    Printer(tsr_printer::Error),
    Edits(tsr_core::UnappliableEdits),
    Canceled,
    MissingFile(String),
    MissingSourceFile(String),
    MissingSupplementalFile(i32),
}
impl From<tsr_core::UnappliableEdits> for Error {
    fn from(value: tsr_core::UnappliableEdits) -> Self {
        Self::Edits(value)
    }
}
impl From<tsr_printer::Error> for Error {
    fn from(value: tsr_printer::Error) -> Self {
        Self::Printer(value)
    }
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
            Self::Printer(e) => e.fmt(f),
            Self::Edits(e) => write!(f, "unappliable snippet edits: {e:?}"),
            Self::Canceled => f.write_str("request canceled"),
            Self::MissingFile(name) => write!(f, "file not found: {name}"),
            Self::MissingSourceFile(name) => write!(f, "source file not found: {name}"),
            Self::MissingSupplementalFile(index) => {
                write!(f, "supplemental source file index not found: {index}")
            }
        }
    }
}
impl std::error::Error for Error {}
pub type Result<T> = std::result::Result<T, Error>;

/// Runs the semantic part of navigation with the requested file's checker.
/// Source-definition's syntactic fast path never invokes this provider. The
/// caller retains its lease for the callback and releases it afterwards.
pub trait QueryChecker {
    fn with_checker<T>(
        &mut self,
        source: NodeId,
        query: impl FnOnce(&mut tsr_checker::Operation<'_>) -> Result<T>,
    ) -> Result<T>;
}

impl QueryChecker for tsr_checker::Operation<'_> {
    fn with_checker<T>(
        &mut self,
        _source: NodeId,
        query: impl FnOnce(&mut tsr_checker::Operation<'_>) -> Result<T>,
    ) -> Result<T> {
        query(self)
    }
}

/// One language-service request. The caller retains its program snapshot and
/// checker lease for the entire request; nodes and coordinate maps never cross
/// into a later snapshot. Syntax-only queries do not acquire a checker.
pub struct LanguageService<'a> {
    auto_imports: std::sync::Arc<tsr_autoimport::Cache>,
    completion_host: Option<std::sync::Arc<dyn tsr_vfs::FileSystem>>,
    program: &'a Program,
    converters: Converters,
    source_maps: source_map::Maps,
    cancellation: CancellationToken,
    cross_project_targets: Option<CrossProjectTargets>,
    /// While the API server collects completions: the symbol of each
    /// symbol-backed entry by label.
    pub(crate) completion_symbols:
        Option<std::collections::HashMap<String, tsr_checker::SymbolRef>>,
    #[cfg(test)]
    reference_search_count: usize,
}
impl<'a> LanguageService<'a> {
    pub fn new(
        program: &'a Program,
        encoding: tsr_jsstring::PositionEncoding,
        cancellation: CancellationToken,
    ) -> Self {
        Self {
            auto_imports: std::sync::Arc::new(tsr_autoimport::Cache::new(program)),
            completion_host: None,
            program,
            converters: Converters::new(encoding),
            source_maps: source_map::Maps::new(),
            cancellation,
            cross_project_targets: None,
            completion_symbols: None,
            #[cfg(test)]
            reference_search_count: 0,
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
