//! Immutable program loading, without checker or emitter construction.
//! Every successful `Program::load` represents an executed loader closure;
//! unsupported source operations fail with a named boundary.
mod cache;
mod checker_diagnostics;
mod checker_host;
mod checker_module_specifiers;
mod declaration_diagnostics;
mod include_reason;
mod output_paths;
mod plain_js_errors;
mod program_diagnostics;
mod verify_options;
pub use verify_options::{verify_compiler_options, FileIncludeDiagnostic, OptionVerification};
mod loader;
mod metadata;
mod resolver_host;
pub use cache::{FileCache, ProgramFile};
pub use checker_host::ProgramCheckerHost;
pub use loader::{Error, Program, ProgramOptions, Resolution, TypeResolution};
pub use resolver_host::ProgramResolverHost;
pub use ts_ast::SourceFileMetaData;

#[cfg(test)]
mod boundary_tests;
#[cfg(test)]
mod ownership_tests;
#[cfg(test)]
mod tests;
