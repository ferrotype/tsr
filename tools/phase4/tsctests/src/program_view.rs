//! What `TestSys.OnProgram` reads of an incremental program, behind one
//! module: the file's path and name, the cached semantic diagnostics of a
//! testing snapshot by identity, the include reasons and the missing paths.
//!
//! Two of these are not public in the production crates yet, so they refuse
//! with a named [`unsupported`] operation instead of guessing:
//! `tsr_incremental::Snapshot`'s semantic-diagnostics map (the pin's
//! `TestingData.SemanticDiagnosticsPerFile.Load`) and
//! `tsr_compiler::Program`'s include reasons (`Program.GetIncludeReasons`).
//! Nothing calls `OnProgram` before Phase 4 X1 runs a compilation; X1/X2
//! make the two accessors public and replace these bodies.
use crate::execute::unsupported;
use std::collections::BTreeSet;
use tsr_compiler::{Program, ProgramFile};
use tsr_incremental::Snapshot;
use tsr_jsstring::JsString;

/// `file.Path()` and `file.FileName()`.
pub fn path_and_file_name(file: &ProgramFile) -> (JsString, JsString) {
    let view = file.bound().view();
    let source = view
        .source_file()
        .expect("a program file has its source file");
    let options = source.parse_options();
    (options.path.clone(), options.file_name.clone())
}

/// The identity of the cached semantic diagnostics of `path`
/// (`SemanticDiagnosticsPerFile.Load(path)`, compared by pointer), `None`
/// when the snapshot caches none.
pub fn semantic_diagnostics_identity(snapshot: &Snapshot, path: &JsString) -> Option<usize> {
    let _ = (snapshot, path);
    unsupported(
        "tsr_incremental::Snapshot::semantic_diagnostics_per_file is crate-private \
         (TestingData.SemanticDiagnosticsPerFile.Load, Phase 4 X2)",
    )
}

/// The paths of `Program.GetIncludeReasons()`, in path order (the pin
/// iterates its map in random order).
pub fn include_reason_paths(program: &Program) -> BTreeSet<JsString> {
    let _ = program;
    unsupported(
        "tsr_compiler::Program::include_reasons is crate-private \
         (Program.GetIncludeReasons, Phase 4 X2)",
    )
}

/// `Program.IsMissingPath` (a production function, X2's to port into
/// `tsr_compiler`; this is its body over the public accessors): whether a
/// missing file of the program has the path `path`.
pub fn is_missing_path(program: &Program, path: &JsString) -> bool {
    program.missing_files().iter().any(|missing_path| {
        tsr_tspath::to_path(
            missing_path.as_bytes(),
            program.current_directory(),
            program.use_case_sensitive_file_names(),
        ) == *path
    })
}
