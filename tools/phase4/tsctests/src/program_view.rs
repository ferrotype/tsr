//! Narrow read-only observations of the production program's inclusion and
//! incremental diagnostics caches. The returned cache identity retains its
//! entry; no raw pointer or mutable cache crosses this boundary.
use std::collections::BTreeSet;
use tsr_compiler::{Program, ProgramFile};
use tsr_incremental::{CachedDiagnosticsIdentity, Snapshot};
use tsr_jsstring::JsString;

pub fn path_and_file_name(file: &ProgramFile) -> (JsString, JsString) {
    let view = file.bound().view();
    let source = view.source_file().expect("a program file has its source file");
    let options = source.parse_options();
    (options.path.clone(), options.file_name.clone())
}

pub fn semantic_diagnostics_identity(snapshot: &Snapshot, path: &JsString) -> Option<CachedDiagnosticsIdentity> {
    snapshot.cached_semantic_diagnostics_identity(path)
}

pub fn include_reason_paths(program: &Program) -> BTreeSet<JsString> {
    program.include_reason_paths().cloned().collect()
}

pub fn is_missing_path(program: &Program, path: &JsString) -> bool {
    program.is_missing_path(path)
}
