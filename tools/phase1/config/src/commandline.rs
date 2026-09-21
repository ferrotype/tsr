//! The `tsoptions/commandLineParsing` baseline group: 53 `parseCommandLine`
//! and 27 `parseBuildOptions` outputs.
//!
//! Unlike F2a's carried `config/matchFiles` envelope, these 80 have a real
//! pinned producer, and `tools/phase1/config/commandline_probe_test.go` renders
//! them through the pinned `formatNewBaseline` / `formatNewBaselineBuild`
//! rather than through anything carried. The gap is on this side: the port has
//! no argument-vector parser at all.
//!
//! That is a reading of the crate, not of the ledger. `crates/tsr_tsoptions`
//! carries the generated option declarations and the whole config-file path,
//! but nothing that consumes an `argv`: the closest thing,
//! `fixture_options::apply_fixture_settings`, takes already-split
//! `name`/`value` pairs from a compiler-test fixture comment and explicitly
//! refuses a switch or a response-file argument -- "This is the source bridge's
//! ParseCommandLine([\"--\"+name,value]) error path. A second switch/response
//! argument requires the full CLI operation"
//! (crates/tsr_tsoptions/src/fixture_options.rs:270-275). So the option
//! *semantics* are partly present while the parser the baselines exercise is
//! absent, which is what these rows record.
//!
//! Preparation records the gap; it never emulates the parser to make a
//! comparison run, and it never reads the frozen bytes.

use crate::api::Outcome;
use serde_json::Value;

const PARSE_COMMAND_LINE: (&str, &str, &str, &str) = (
    "tsoptions.parseCommandLineBaseline",
    "tsc/internal/tsoptions/commandlineparser.go:ParseCommandLine, whose result the pinned \
     formatNewBaseline (tsc/internal/tsoptions/commandlineparser_test.go:334) renders. The 53 \
     frozen outputs are written from the test worker in the pinned export_test.go, which shares \
     ParseCommandLine's parseStrings body (commandlineparser.go:134)",
    "pub fn parse_command_line(command_line: &[JsString], host: &dyn ParseConfigHost) -> \
     ParsedCommandLine, carrying the option map in declaration order, the file names and the \
     diagnostics the Errors:: section renders",
    "crates/tsr_tsoptions/src/parse_options.rs, which today parses option VALUES but has no \
     argument-vector parser (absent)",
);

const PARSE_BUILD_COMMAND_LINE: (&str, &str, &str, &str) = (
    "tsoptions.parseBuildOptionsBaseline",
    "tsc/internal/tsoptions/commandlineparser.go:ParseBuildCommandLine, whose result the pinned \
     formatNewBaselineBuild (tsc/internal/tsoptions/commandlineparser_test.go:456) renders",
    "pub fn parse_build_command_line(command_line: &[JsString], host: &dyn ParseConfigHost) -> \
     ParsedBuildCommandLine, a distinct mode carrying build options, the compiler options it \
     still accepts, the project list and the build-only diagnostics",
    "crates/tsr_tsoptions/src/parse_options.rs (absent; BUILD_OPTIONS declarations exist in \
     option_declarations_generated.rs, the parsing mode does not)",
);

pub fn observe(request: &Value) -> Option<Outcome> {
    if crate::api::subject(request) != "commandLineBaseline" {
        return None;
    }
    let requested = request
        .get("operation")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let (operation, authority, signature, home) = match requested {
        "tsoptions.parseCommandLineBaseline" => PARSE_COMMAND_LINE,
        "tsoptions.parseBuildOptionsBaseline" => PARSE_BUILD_COMMAND_LINE,
        other => {
            return Some(Outcome::Failed(format!(
                "no reviewed missing operation for command-line baseline operation {other:?}"
            )))
        }
    };
    Some(Outcome::missing(operation, authority, signature, home))
}
