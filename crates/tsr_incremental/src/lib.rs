//! The incremental program and its build info: the pin's
//! `execute/incremental` package.
//!
//! [`new_program`] wraps a [`tsr_compiler::CheckedProgram`] with the
//! incremental state computed from an old program, if any: either an
//! earlier [`Program`] of the same session or the one
//! [`read_build_info_program`] reads back from a `.tsbuildinfo`. The program
//! checks and emits only what changed, and its emit ends with the build info
//! ([`BuildInfo`], written with the pin's JSON shapes).
//!
//! Where the pin's methods take a context, these take a
//! [`tsr_checker::CheckerRequest`]; a canceled request returns the pin's nil
//! results. The pin's `SyncMap`s are maps behind a mutex, and the work groups
//! it queues per file are [`tsr_core::workgroup::WorkGroup`]s.
mod affected_files_handler;
mod build_info;
mod build_info_to_snapshot;
mod emit_files_handler;
mod host;
mod incremental;
mod json;
mod program;
mod program_to_snapshot;
mod reference_map;
mod snapshot;
mod snapshot_to_build_info;

pub use build_info::{
    content_mapper_identities, is_build_info_file_name_default_library, BuildInfo,
    BuildInfoDiagnostic, BuildInfoDiagnosticsOfFile, BuildInfoEmitSignature, BuildInfoFileId,
    BuildInfoFileIdListId, BuildInfoFileInfo, BuildInfoFilePendingEmit, BuildInfoReferenceMapEntry,
    BuildInfoRepopulateInfo, BuildInfoResolvedRoot, BuildInfoRoot, BuildInfoRootInfoReader,
    BuildInfoSemanticDiagnostic,
};
pub use host::{create_host, get_mtime, CompilerHost, Host, ProgramCompilerHost};
pub use incremental::{new_build_info_reader, read_build_info_program, BuildInfoReader};
pub use json::AnyValue;
pub use program::{new_program, NestedEmitNow, Program, SignatureUpdateKind, TestingData};
pub use snapshot::{
    compute_hash, get_file_emit_kind, CachedDiagnosticsIdentity, FileEmitKind, FileInfo, Path,
    Snapshot,
};

#[cfg(test)]
mod tests;
