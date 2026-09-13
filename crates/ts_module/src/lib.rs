//! Snapshot-scoped module resolution. Unsupported source branches are errors,
//! never a successful unresolved result. See `Error::Unsupported` for the
//! remaining package-map and project-reference boundaries.
mod resolver;
pub use resolver::{
    get_conditions, get_types_package_name, is_relative, mangle_scoped_package_name,
    resolve_config, resolve_package_directory, Error, PackageContents, PackageId, PackageJson,
    Probe, ResolvedModule, Resolver,
};

mod diagnostic;
pub use diagnostic::resolution_diagnostic;
mod paths;
mod type_references;
pub use type_references::{
    effective_type_roots, ResolvedTypeReferenceDirective, INFERRED_TYPES_CONTAINING_FILE,
};

mod package_maps;
pub use package_maps::is_applicable_versioned_types_key;

pub mod package_json;

mod trace;
pub use trace::{DiagAndArgs, TraceArg};

mod config_mapper;
pub use config_mapper::resolve_content_mapper_manifest;
