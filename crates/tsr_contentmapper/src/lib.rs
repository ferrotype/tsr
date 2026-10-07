//! Content mappers: plugins that transform otherwise unsupported file content
//! (a `.vue` file, say) into virtual TypeScript during program construction.
//!
//! A mapper is declared in tsconfig and resolved with its package's manifest
//! by `tsr_tsoptions`. The host here drives the configured mappers: it starts
//! each mapper through a spawner, talks JSON-RPC to it over `tsr_ipc`, and
//! turns its responses into virtual source files with span maps back to the
//! original content. Mappers sharing an identity share one connection.
mod host;
mod host_impl;
mod mapper;
mod protocol;
mod transform;

pub use host::{
    transform_error, DiagnosticDirectiveErrorKind, Error, Host, InitializeError,
    InitializeErrorKind, MappedResult, MapperTimings, OperationTiming, OptionDiagnostic,
    OptionPathSegment, Project, ProjectErrorKind, ProjectSpec, Request, Timings,
    TransformErrorKind, TransformResult,
};
pub use host_impl::{
    new_host, new_host_with_options, HostImpl, HostOptions, Logger, SpawnError, Spawner,
    SpawnerFunc,
};
pub use mapper::{
    declared_json, diagnostic_name, hex, identity, is_supported_virtual_extension,
    marshal_declared_options, transform_identity,
};
pub use protocol::*;
pub use transform::{
    check_supplemental_file_name_collisions, parse_result, transform_and_parse, SourceFiles,
};
pub use tsr_tsoptions::options_json::{
    marshal_compiler_options, option_json, CompilerOptionsJson, OptionValue,
};

#[cfg(test)]
mod tests;
