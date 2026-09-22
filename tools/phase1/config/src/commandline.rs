//! The 80 baseline envelopes still need the shared Go-renderer bridge and
//! the ordinary test worker's synthetic declaration input. Production argv
//! parsing is implemented in command_line.rs and exercised by commandlineops.
//! These rows identify the missing *baseline integration*, not an absent parser.

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
    "production parser exists at crates/tsr_tsoptions/src/command_line.rs; missing test-only \
     shared Go envelope bridge and synthetic option-declaration worker entry point",
);

const PARSE_BUILD_COMMAND_LINE: (&str, &str, &str, &str) = (
    "tsoptions.parseBuildOptionsBaseline",
    "tsc/internal/tsoptions/commandlineparser.go:ParseBuildCommandLine, whose result the pinned \
     formatNewBaselineBuild (tsc/internal/tsoptions/commandlineparser_test.go:456) renders",
    "pub fn parse_build_command_line(command_line: &[JsString], host: &dyn ParseConfigHost) -> \
     ParsedBuildCommandLine, a distinct mode carrying build options, the compiler options it \
     still accepts, the project list and the build-only diagnostics",
    "production build parser exists at crates/tsr_tsoptions/src/command_line.rs; missing \
     test-only shared Go envelope bridge",
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
