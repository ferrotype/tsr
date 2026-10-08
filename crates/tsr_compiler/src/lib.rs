//! Immutable program loading, without checker or emitter construction.
//! Every successful `Program::load` represents an executed loader closure;
//! unsupported source operations fail with a named boundary.
mod bind_diagnostics;
mod cache;
mod checked_program;
mod checker_diagnostics;
mod checker_host;
pub use checked_program::{
    filter_no_emit_semantic_diagnostics, CheckedProgram, CheckerCollect, FileCheckers,
};
mod checker_pool;
pub use checker_pool::{
    checker_association_base_weight, checker_association_order, checker_association_policy,
    checker_association_weights, checker_associations_in_order, should_prioritize_source_files,
    CheckerAssociationPlan, CheckerAssociationPolicy, CompilerCheckerPool,
};
mod checker_module_specifiers;
mod content_mapped;
pub use content_mapped::content_mapper_project_diagnostic;
mod declaration_diagnostics;
mod declaration_host;
mod navigation;
pub use declaration_host::ProgramDeclarationHost;
pub use navigation::navigation_module_format;
mod emit_host;
pub use emit_host::EmitHost;
pub mod diagnostic_writer;
pub mod emitter;
mod include_reason;
mod output_paths;
mod plain_js_errors;
mod program_diagnostics;
mod program_emit;
pub use program_emit::{
    combine_emit_results, get_diagnostics_of_any_program, handle_no_emit_options, EmitOnly,
    EmitOptions, EmitResult, FileDiagnostics, ProgramLike, SourceMapEmitResult, WriteFile,
    WriteFileData,
};
mod project_reference_host;
mod project_references;
pub use project_references::{CompilerConfigHost, ResolvedProjectReferenceProvider};
mod syntactic_diagnostics;
mod verify_options;
pub use verify_options::{verify_compiler_options, FileIncludeDiagnostic, OptionVerification};
mod loader;
mod metadata;
mod preload;
mod resolver_host;
pub use cache::{
    CachedMappedProgramFiles, CachedProgramFile, FileCache, MappedFileResult, MappedProgramFiles,
    MappedSourceFileRequest, ProgramFile, SharedSourceFileCache, SourceFileCache,
};
pub use checker_host::ProgramCheckerHost;
pub use loader::{
    Error, LibFile, Program, ProgramHostServices, ProgramOptions, ProgramReuse, Resolution,
    TypeResolution,
};
pub use resolver_host::ProgramResolverHost;
pub use tsr_ast::SourceFileMetaData;
/// The message catalog that `Program::explain_file_include` takes its messages from.
pub use tsr_diagnostics as messages;

#[cfg(test)]
mod boundary_tests;
#[cfg(test)]
mod ownership_tests;
#[cfg(test)]
mod tests;

mod statistics;
