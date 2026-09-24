//! The `syntax` oracle of the Phase 1 mutation witnesses
//! (docs/PHASE1-mutation-witnesses.md): the corpus syntax schedule, one
//! loaded program per row, speaking the mutation driver's trace and kill
//! protocol (`tools/phase1/mutation/driver/src/jobs.rs`).
//!
//! `PHASE1_MUTATION=trace phase1_syntax --requests FILE --out FILE [--frames
//! FILE] [--dump-dir DIR]` and `PHASE1_MUTATION=kill phase1_syntax --requests
//! FILE --base FILE --dump-dir DIR < jobs`. Without the variable the harness is
//! unchanged; in a repository build every mutant guard is absent, so this mode
//! is inert there too.
//!
//! A request line is a schedule probe (`id`, `request`, optionally `load`,
//! `guard`, `path`) plus `request_sha256`, the committed
//! `loading_request_sha256`: sha256 of `s07_subset.json_bytes(request)`. Each
//! row loads its program with a fresh file cache through the schedule's own
//! `observe`, so production (the load and `syntactic_diagnostics`) and
//! observation (file names, structured diagnostics, rendering) are the
//! schedule's. The single operation is `program` (`ok` when the row loaded and
//! was observed, else the row's state); the compared stages are the fields of
//! `scripts/phase1_syntax.py` `COMPARED`, each digested as
//! `sha256(canonical(value))` (`phase1_mutation_go.syntax_row`). A row's
//! frames are its schedule row, as one JSON line.

use std::collections::BTreeMap;
use std::panic::{catch_unwind, AssertUnwindSafe};

use serde_json::{json, Value};

use crate::canonical;
use crate::jobs::{self, Request, RowOutput, Rows};
use crate::schedule;
use tsr_compiler::FileCache;

/// The row's single operation: the program loaded and was observed
/// (`phase1_mutation_go.ORACLES["syntax"].operations`).
pub(crate) const OPERATION: &str = "program";
/// `scripts/phase1_syntax.py` `COMPARED`.
pub(crate) const COMPARED: [&str; 5] = [
    "files",
    "file_names_sha256",
    "syntactic",
    "plain_hex",
    "pretty_hex",
];

struct Syntax;

fn sha256_canonical(value: &Value) -> Result<String, String> {
    let mut bytes = Vec::new();
    canonical::write(&mut bytes, value, false).map_err(|error| error.to_string())?;
    Ok(jobs::sha256_hex(&bytes))
}

impl Rows for Syntax {
    fn name(&self) -> &'static str {
        "syntax"
    }

    fn compared(&self) -> &'static [&'static str] {
        &COMPARED
    }

    fn observe_counts(&self) -> bool {
        false
    }

    fn dumps_kills(&self) -> bool {
        true
    }

    fn request(&self, index: usize, line: &[u8]) -> Result<Request, String> {
        let mut value: Value = serde_json::from_slice(line).map_err(|error| error.to_string())?;
        let object = value.as_object_mut().ok_or("a request line is an object")?;
        if let Some(unknown) = object.keys().find(|key| {
            !["id", "request_sha256", "request", "load", "guard", "path"].contains(&key.as_str())
        }) {
            return Err(format!("unknown request field {unknown:?}"));
        }
        let claimed = object
            .remove("request_sha256")
            .and_then(|digest| digest.as_str().map(str::to_owned))
            .ok_or("missing request_sha256")?;
        if object.get("load").is_some_and(|load| load != &json!(true)) {
            return Err("only loaded schedule rows are syntax oracle rows".into());
        }
        let id = object
            .get("id")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
            .ok_or("missing id")?
            .to_owned();
        let request = object.get("request").ok_or("missing request")?;
        if request["id"].as_str() != Some(id.as_str()) {
            return Err("the loading request names another id".into());
        }
        let mut bytes = Vec::new();
        canonical::write_json_bytes(&mut bytes, request, false)
            .map_err(|error| error.to_string())?;
        bytes.push(b'\n');
        if jobs::sha256_hex(&bytes) != claimed {
            return Err("request_sha256 does not match the loading request".into());
        }
        let source_bytes = request["files"]
            .as_object()
            .ok_or("request without files")?
            .values()
            .map(|text| text.as_str().map_or(0, |hex| hex.len() / 2))
            .sum();
        Ok(Request {
            index,
            id,
            sha256: claimed,
            value,
            source_bytes,
        })
    }

    fn execute(&self, request: &Request, capture: bool) -> RowOutput {
        let counters = tsr_arena::Counters::new();
        let mut cache = FileCache::new();
        let mut row = catch_unwind(AssertUnwindSafe(|| {
            schedule::observe(&request.value["request"], &mut cache, &counters)
        }))
        .unwrap_or_else(
            |payload| json!({"state": "panic", "panic": schedule::panic_text(&*payload)}),
        );
        row["id"] = json!(request.id);
        // `phase1_mutation_go.syntax_row`: an observed row is `ok`, any other
        // row has its state as the outcome and its detail as the message.
        let state = row["state"].as_str().unwrap_or_default().to_owned();
        let outcome = if state == "observed" { "ok" } else { &state };
        let mut output = RowOutput {
            outcomes: BTreeMap::from([(OPERATION.to_owned(), outcome.to_owned())]),
            ..RowOutput::default()
        };
        if state == "observed" {
            for field in COMPARED {
                match sha256_canonical(&row[field]) {
                    Ok(digest) => {
                        output.digests.insert(field.to_owned(), digest);
                    }
                    Err(error) => {
                        output.error.get_or_insert(error);
                    }
                }
            }
        } else {
            let detail = ["panic", "error", "operation"]
                .iter()
                .find_map(|field| row[field].as_str())
                .unwrap_or_default();
            output
                .messages
                .insert(OPERATION.to_owned(), crate::hex(detail.as_bytes()));
        }
        if capture {
            match serde_json::to_vec(&row) {
                Ok(mut bytes) => {
                    bytes.push(b'\n');
                    output.frames = Some(bytes);
                }
                Err(error) => {
                    output.error.get_or_insert(error.to_string());
                }
            }
        }
        output
    }
}

/// Runs the mutation mode `mode` (`trace` or `kill`) on a parser worker, so
/// parsing and binding run inline on the thread whose switch state records
/// reach.
pub(crate) fn main(mode: &str, args: &[String]) -> Result<(), String> {
    // Stage panics are row outcomes; the default hook would repeat them for
    // every mutant.
    std::panic::set_hook(Box::new(|_| {}));
    let options = jobs::options(args, &["requests", "out", "frames", "dump-dir", "base"])?;
    std::thread::scope(|scope| {
        match tsr_parser::spawn_parser_worker(scope, || jobs::run_mode(&Syntax, mode, &options)) {
            Ok(worker) => worker.join().unwrap_or_else(|payload| {
                Err(format!(
                    "mutation mode panic: {}",
                    jobs::panic_message(payload.as_ref())
                ))
            }),
            Err(error) => Err(format!("could not start the parser worker: {error}")),
        }
    })
}

#[cfg(test)]
mod tests {
    use super::{Syntax, COMPARED, OPERATION};
    use crate::jobs::{run_row, Rows};
    use serde_json::json;

    #[test]
    fn requests_are_authenticated_by_the_loading_request_digest() {
        let request = json!({
            "case_sensitive": true, "cwd": "/.src",
            "files": {"/.src/a.ts": crate::hex(b"let x = ;")},
            "id": "compiler/a.ts#configuration=0",
            "options": {"noErrorTruncation": true, "paths": {"z/*": ["z"], "a/*": ["a"]}},
            "roots": ["/.src/a.ts"], "skip_module_resolution": false, "symlinks": {},
        });
        let mut bytes = Vec::new();
        crate::canonical::write_json_bytes(&mut bytes, &request, false).unwrap();
        bytes.push(b'\n');
        let line = json!({
            "id": "compiler/a.ts#configuration=0", "load": true, "guard": true,
            "path": "tsc/testdata/tests/cases/compiler/a.ts", "request": request,
            "request_sha256": crate::jobs::sha256_hex(&bytes),
        });
        let parsed = Syntax
            .request(4, &serde_json::to_vec(&line).unwrap())
            .unwrap();
        assert_eq!(
            (parsed.index, parsed.id.as_str(), parsed.source_bytes),
            (4, "compiler/a.ts#configuration=0", 9)
        );
        let mut forged = line.clone();
        forged["request"]["files"]["/.src/a.ts"] = json!(crate::hex(b"let y = ;"));
        assert!(Syntax
            .request(0, &serde_json::to_vec(&forged).unwrap())
            .is_err());
        let mut unloaded = line;
        unloaded["load"] = json!(false);
        assert!(Syntax
            .request(0, &serde_json::to_vec(&unloaded).unwrap())
            .is_err());

        let row = run_row(&Syntax, &parsed, true, 0);
        let output = row.output;
        assert!(output.error.is_none(), "{:?}", output.error);
        assert_eq!(
            output.outcomes.get(OPERATION).map(String::as_str),
            Some("ok"),
            "{:?}",
            output.messages
        );
        assert_eq!(output.outcomes.len(), 1);
        assert_eq!(output.digests.len(), COMPARED.len());
        let frame: serde_json::Value =
            serde_json::from_slice(output.frames.as_deref().unwrap()).unwrap();
        assert_eq!(frame["state"], "observed");
        assert!(
            !frame["syntactic"].as_array().unwrap().is_empty(),
            "`let x = ;` has a syntactic diagnostic"
        );
    }
}
