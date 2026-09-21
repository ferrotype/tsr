//! The `config/tsconfigParsing` baseline group: 87 outputs across three pinned
//! entry points.
//!
//! All 87 are recorded gaps, but not the same gap, and the distinction is the
//! useful part of the record. There are two independent reasons, read off the
//! crate rather than off the ledger:
//!
//! 1. **The envelope has no Rust producer, for any of the 87.** The
//!    `Errors::` section is the pinned
//!    `diagnosticwriter.FormatDiagnosticsWithColorAndContext`; the only Rust
//!    counterpart is `tsr_compiler::diagnostic_writer::DiagnosticWriter`,
//!    whose constructor is `new(program: &Program, options: FormattingOptions)`
//!    (crates/tsr_compiler/src/diagnostic_writer/mod.rs:75). A config parse
//!    produces no `Program`, so that writer cannot render a config diagnostic
//!    standing alone, and `tsr_diagnostics` is message data only. The plan
//!    settles what that means for a byte case: "In Phase A, an absent Rust
//!    writer keeps that case `not_implemented` even if structural parity
//!    holds" (docs/PHASE1-implementation-plan.md:750-751).
//!
//! 2. **One of the three entry points is itself absent.** The 40 `json` API
//!    outputs go through the pinned `ParseJsonConfigFileContent`
//!    (tsconfigparsing.go:872), the raw-`any` entry point, which has no Rust
//!    counterpart at all: `crates/tsr_tsoptions` exposes
//!    `parse_json_source_file_config_file_content` (config_parse.rs:691) and
//!    `parse_config_file_text_to_json` (config_text.rs:20), and nothing that
//!    takes a parsed-JSON value. The 40 `jsonSourceFile` and 7 `jsonParse`
//!    outputs do have their parse in Rust; for those, reason 1 is the only one
//!    standing, and F3b closes a strictly smaller gap.
//!
//! Recording that split is the point. A single undifferentiated
//! `not_implemented` across all 87 would say the port is equally far from all
//! three, which is false.
//!
//! Preparation records the gap; it never emulates a renderer or a parse to
//! make a comparison run, and it never reads the frozen bytes.

use crate::api::Outcome;
use serde_json::Value;

/// Shared by all three: the envelope itself, which no Rust code renders.
const ENVELOPE_HOME: &str = "no Rust home: the `Fs::`/`configFileName::`/`CompilerOptions::`/\
     `TypeAcquisition::`/`FileNames::`/`Errors::` envelope has no counterpart, and the \
     `Errors::` section needs a diagnostic writer that does not require a Program \
     (crates/tsr_compiler/src/diagnostic_writer/mod.rs:75 takes one)";

const JSON_API: (&str, &str, &str) = (
    "tsc/internal/tsoptions/tsconfigparsing.go:ParseJsonConfigFileContent, whose result the \
     pinned baselineParseConfigWith (tsc/internal/tsoptions/tsconfigparsing_test.go:1503) \
     renders through getParsedWithJsonApi (:952)",
    "pub fn parse_json_config_file_content(json: &ConfigValue, host: &dyn ParseConfigHost, \
     base_path: &JsString, existing_options: Option<&CompilerOptions>, config_file_name: \
     &JsString, resolution_stack: &[Path], extended_cache: Option<&mut ExtendedConfigCache>) \
     -> ParsedCommandLine -- the raw-JSON entry point, absent; only the source-file one exists",
    "crates/tsr_tsoptions/src/config_parse.rs (the raw-JSON entry point is absent)",
);

const JSON_SOURCE_FILE_API: (&str, &str, &str) = (
    "tsc/internal/tsoptions/tsconfigparsing.go:ParseJsonSourceFileConfigFileContent, whose \
     result the pinned baselineParseConfigWith renders through \
     getParsedWithJsonSourceFileApi (tsc/internal/tsoptions/tsconfigparsing_test.go:1481)",
    "the parse exists as tsr_tsoptions::parse_json_source_file_config_file_content \
     (crates/tsr_tsoptions/src/config_parse.rs:691); what is missing is the baseline envelope \
     over its result, pub fn render_tsconfig_parsing_baseline(parsed: &ParsedCommandLine, \
     host: &dyn ParseConfigHost, config_file_name: &JsString) -> String",
    ENVELOPE_HOME,
);

const JSON_PARSE_API: (&str, &str, &str) = (
    "tsc/internal/tsoptions/tsconfigparsing.go:ParseConfigFileTextToJson, rendered inline by \
     the pinned TestParseConfigFileTextToJson (tsc/internal/tsoptions/tsconfigparsing_test.go:130)",
    "the parse exists as tsr_tsoptions::parse_config_file_text_to_json \
     (crates/tsr_tsoptions/src/config_text.rs:20); what is missing is the \
     `Input::`/`Config::`/`Errors::` envelope over its result, which needs the readable-JSON \
     writer the pinned writeJsonReadableText supplies and a config-scope diagnostic writer",
    ENVELOPE_HOME,
);

pub fn observe(request: &Value) -> Option<Outcome> {
    if crate::api::subject(request) != "tsconfigParsingBaseline" {
        return None;
    }
    let api = request
        .get("api")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let (authority, signature, home) = match api {
        "json" => JSON_API,
        "jsonSourceFile" => JSON_SOURCE_FILE_API,
        "jsonParse" => JSON_PARSE_API,
        other => {
            return Some(Outcome::Failed(format!(
                "no reviewed missing operation for tsconfigParsing api {other:?}"
            )))
        }
    };
    Some(Outcome::missing(
        "tsoptions.tsconfigParsingBaseline",
        authority,
        signature,
        home,
    ))
}
