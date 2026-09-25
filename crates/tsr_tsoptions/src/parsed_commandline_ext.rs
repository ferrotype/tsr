//! `ParsedCommandLine` methods of `tsoptions/parsedcommandline.go` beyond
//! `parsed_accessors.rs`.
//!
//! Witnessed by the `tsoptions` group of the Phase 1 operation tables
//! (`docs/PHASE1-mutation-witnesses.md`, section 9).
use crate::ParsedCommandLine;

impl ParsedCommandLine {
    /// `directory_path` is already a `tspath.Path`; each wildcard directory is
    /// reduced with `to_path` and compared as a path, recursively or exactly.
    /// port: tsc/internal/tsoptions/parsedcommandline.go:ParsedCommandLine.PossiblyMatchesDirectoryName
    pub fn possibly_matches_directory_name(&self, directory_path: &tsr_tspath::Path) -> bool {
        for (wildcard_dir, recursive) in self.wildcard_directories().into_iter().flatten() {
            let wildcard_dir_path = tsr_tspath::Path::from_bytes(
                tsr_tspath::to_path(
                    wildcard_dir.as_bytes(),
                    self.current_directory(),
                    self.use_case_sensitive_file_names(),
                )
                .as_bytes()
                .to_vec(),
            );
            if *recursive {
                if wildcard_dir_path.contains_path(directory_path) {
                    return true;
                }
            } else if wildcard_dir_path == *directory_path {
                return true;
            }
        }
        false
    }
}
