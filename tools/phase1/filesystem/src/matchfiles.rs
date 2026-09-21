//! The carried `config/matchFiles` baseline group.
//!
//! The 142 frozen `config/matchFiles` reference outputs are rendered natively
//! by `tools/phase1/filesystem/matchfiles_probe_test.go`, which carries the
//! test envelope and fills it from the pinned configuration parse. The Rust
//! port has no counterpart: no crate renders that envelope, `tsr_tsoptions`
//! exposes no equivalent of the `json` entry point and no
//! `ParsedCommandLine::wildcard_directories`, so every case here is a recorded
//! gap.
//!
//! Preparation records the gap; it never emulates the renderer to make a
//! comparison run, and it never reads the frozen bytes.

use crate::api::Outcome;
use serde_json::Value;

/// The three pieces the port needs before this group can be observed on the
/// Rust side. Each names the pinned Go authority, the signature the port is
/// expected to carry and the file that does not have it yet.
const MISSING: (&str, &str, &str) = (
    "tsc/internal/tsoptions/tsconfigparsing_test.go:printFS plus \
     tsc/internal/tsoptions/tsconfigparsing.go:ParseJsonConfigFileContent and \
     :ParseJsonSourceFileConfigFileContent, whose results the carried envelope renders",
    "pub fn render_match_files_baseline(inputs: &MatchFilesInputs) -> String, rendering \
     `config:`/`Fs::`/`configFileName::`/`Result`/`Errors::` over a ParsedCommandLine, which \
     first needs parse_json_config_file_content(raw, host, base_path, existing, config_file_name, \
     resolution_stack, extended_cache) -> ParsedCommandLine (the `json` entry point, absent: only \
     parse_json_source_file_config_file_content exists) and \
     ParsedCommandLine::wildcard_directories(&self) -> BTreeMap<JsString, bool> (absent)",
    "crates/tsr_tsoptions/src/config_parse.rs (the `json` entry point and wildcard directories) \
     with the envelope in tools/phase1/filesystem/src/ (both absent)",
);

pub fn observe(request: &Value) -> Option<Outcome> {
    if crate::api::subject(request) != "matchFilesBaseline" {
        return None;
    }
    let (authority, signature, home) = MISSING;
    Some(Outcome::missing(
        "tsoptions.matchFilesBaseline",
        authority,
        signature,
        home,
    ))
}
