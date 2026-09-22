//! `debug` group. The Go package is a set of panic-text contracts called by
//! the scanner, parser, binder, core and tsoptions. Rust has no shared helper:
//! each call site composes its message inline (for example
//! crates/tsr_scanner/src/binder_helpers.rs), and PORTS.toml plans the helper
//! in tsr_core with `rust = []`. So every request names the missing helper
//! rather than assembling a message here that no production code would share.

use serde_json::Value;

use crate::api::{subject, Outcome};

const SUBJECT: &str = "debug";
const HOME: &str =
    "a shared tsr_core debug helper (planned in PORTS.toml with rust = []); today each call site \
                    formats its own message";

pub fn observe(request: &Value) -> Option<Outcome> {
    if subject(request) != SUBJECT {
        return None;
    }
    let operation = request
        .get("operation")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let (authority, signature) = match operation {
        "tsc/internal/debug/debug.go:Fail" => (
            "upstream/tsc/internal/debug/debug.go:7-15",
            "fn fail(reason: &str) -> ! panicking with \"Debug failure.\" or \"Debug failure. \" + reason",
        ),
        "tsc/internal/debug/debug.go:FailBadSyntaxKind" => (
            "upstream/tsc/internal/debug/debug.go:17-25",
            "fn fail_bad_syntax_kind(kind: &str, message: Option<&str>) -> ! with the \"\\nNode %s was \
             unexpected.\" suffix",
        ),
        "tsc/internal/debug/debug.go:AssertNever" => (
            "upstream/tsc/internal/debug/debug.go:27-43",
            "fn assert_never(detail: &dyn Debuggable, message: Option<&str>) -> ! preferring KindString, then \
             String, then %v",
        ),
        "tsc/internal/debug/debug.go:Assert" => (
            "upstream/tsc/internal/debug/debug.go:45-61",
            "fn assert(value: bool, message: Option<&str>) panicking with \"False expression.\" or \"False \
             expression: \" + message",
        ),
        other => {
            return Some(Outcome::Failed(format!(
                "no reviewed debug operation {other:?}"
            )))
        }
    };
    Some(Outcome::missing(operation, authority, signature, HOME))
}
