//! Phase 2 C0.7: the Rust side of the three runner sub-tests Phase 2 owns.
//!
//! * Union ordering (`compiler_runner.go:verifyUnionOrdering`): every union
//!   the checker interned must be reproduced by sorting its reversed list and
//!   ten shuffles (pinned seed `NewPCG(1234, 5678)`) with the production
//!   comparator. C0 has one checker per program; C6 runs it per checker.
//! * Source file parent pointers (`verifyParentPointers`): below each
//!   non-default-library source-file root, every node the generated child
//!   visitor reaches has the traversal parent as its recorded parent. The
//!   root itself is not checked. The walk stops at its first failure.
//! * Module-resolution trace (`verifyModuleResolution`,
//!   `harnessutil.TracerForBaselining`): the loader's trace, localized and
//!   sanitized exactly as the baseline tracer writes it.
//!
//! This is declared test code over production entry points; it adds no
//! checker behavior. Verdicts, not counts, are compared with Go.
use serde_json::{json, Value};
use std::collections::hash_map::Entry;
use std::collections::HashMap;
use tsr_compiler::Program;

mod compiler_api {
    pub use crate::compiler_api::{Operation, Program, QueryError, TypeRef};
}
#[path = "../../../../tools/phase7/portable/assertions.rs"]
mod assertions;
pub use assertions::{
    first_failing_node, first_parent_failure, inconsistent_orderings, parent_pointers,
    parent_pointers_with, union_ordering, union_ordering_checkers, union_ordering_checkers_by,
    ParentOf, ParentTree, Pcg, TypeOrder,
};

/// `harnessutil.TracerForBaselining.sanitizeTrace`, including its per-tracer
/// package.json cache (one per compile, as the harness host is).
pub struct Sanitizer {
    current_directory: Vec<u8>,
    case_sensitive: bool,
    package_json: HashMap<Vec<u8>, bool>,
}

impl Sanitizer {
    pub fn new(current_directory: &[u8], case_sensitive: bool) -> Self {
        Self {
            current_directory: current_directory.to_vec(),
            case_sensitive,
            package_json: HashMap::new(),
        }
    }

    fn path(&self, file: &[u8]) -> Vec<u8> {
        tsr_tspath::to_path(file, &self.current_directory, self.case_sensitive)
            .as_bytes()
            .to_vec()
    }

    pub fn sanitize(&mut self, message: &[u8], use_package_json_cache: bool) -> Vec<u8> {
        let version = format!("'{}'", tsr_core::version()).into_bytes();
        if let Some(at) = find(message, &version) {
            let mut result = message[..at].to_vec();
            result.extend_from_slice(b"'FakeTSVersion'");
            result.extend_from_slice(&message[at + version.len()..]);
            return result;
        }
        if let Some(rest) = message
            .strip_suffix(b"' does not exist according to earlier cached lookups.".as_slice())
        {
            let file = rest.strip_prefix(b"File '".as_slice()).unwrap_or(rest);
            if use_package_json_cache {
                let path = self.path(file);
                if self.package_json.contains_key(&path) {
                    return message.to_vec();
                }
                self.package_json.insert(path, false);
            }
            return [b"File '".as_slice(), file, b"' does not exist."].concat();
        }
        if let Some(rest) =
            message.strip_suffix(b"' exists according to earlier cached lookups.".as_slice())
        {
            let file = rest.strip_prefix(b"File '".as_slice()).unwrap_or(rest);
            if use_package_json_cache {
                let path = self.path(file);
                if self.package_json.contains_key(&path) {
                    return message.to_vec();
                }
                self.package_json.insert(path, true);
            }
            return [b"Found 'package.json' at '".as_slice(), file, b"'."].concat();
        }
        if use_package_json_cache {
            if let Some(rest) = message.strip_suffix(b"' does not exist.".as_slice()) {
                let file = rest.strip_prefix(b"File '".as_slice()).unwrap_or(rest);
                let path = self.path(file);
                if let Entry::Vacant(entry) = self.package_json.entry(path) {
                    entry.insert(false);
                    return message.to_vec();
                }
                return [
                    b"File '".as_slice(),
                    file,
                    b"' does not exist according to earlier cached lookups.",
                ]
                .concat();
            }
            if let Some(rest) = message.strip_prefix(b"Found 'package.json' at '".as_slice()) {
                let file = rest.strip_suffix(b"'.".as_slice()).unwrap_or(rest);
                let path = self.path(file);
                if let Entry::Vacant(entry) = self.package_json.entry(path) {
                    entry.insert(true);
                    return message.to_vec();
                }
                return [
                    b"File '".as_slice(),
                    file,
                    b"' exists according to earlier cached lookups.",
                ]
                .concat();
            }
        }
        message.to_vec()
    }
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

/// The `.trace.json` text the pinned harness would baseline for this program.
pub fn trace_text(program: &Program) -> Vec<u8> {
    let mut sanitizer = Sanitizer::new(
        program.current_directory(),
        program.host().use_case_sensitive_file_names(),
    );
    let mut text = Vec::new();
    for entry in program.trace() {
        let args: Vec<Vec<u8>> = entry
            .args
            .iter()
            .map(|arg| match arg {
                tsr_module::TraceArg::Text(value) => value.as_bytes().to_vec(),
                tsr_module::TraceArg::Bool(value) => value.to_string().into_bytes(),
            })
            .collect();
        let refs: Vec<&[u8]> = args.iter().map(Vec::as_slice).collect();
        let message =
            tsr_diagnostics::localize(&tsr_locale::DEFAULT, Some(entry.message), b"", &refs);
        text.extend(sanitizer.sanitize(&message, true));
        text.push(b'\n');
    }
    text
}

pub fn trace(program: &Program, trace_resolution: bool) -> Value {
    if !trace_resolution {
        return json!({"state":"disabled"});
    }
    let text = trace_text(program);
    if text.is_empty() {
        json!({"state":"no_content"})
    } else {
        json!({"state":"content","text_hex":hex(&text)})
    }
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8] = b"0123456789abcdef";
    bytes
        .iter()
        .flat_map(|&b| {
            [
                char::from(DIGITS[usize::from(b >> 4)]),
                char::from(DIGITS[usize::from(b & 15)]),
            ]
        })
        .collect()
}
