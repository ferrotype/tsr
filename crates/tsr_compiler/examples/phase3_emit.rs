//! Phase 3 T0 corpus row (`scripts/phase3_corpus.py`): loads the request's
//! program as the pin's harness loads its post-emit program and records the
//! reprint witness and the emit domains. `Program.Emit` is T8's, so every emit
//! domain reports `unsupported` until then; a domain the native runner
//! disables is `disabled` with the request's reason.
//!
//!     phase3_emit REQUEST OUTPUT [--texts]
//!
//! `--texts` keeps each reprint's bytes (`text_hex`): a debugging aid, never
//! used by the corpus.
#[path = "../../../tools/s08/p4/executor.rs"]
#[allow(dead_code)]
mod executor;
#[path = "../../../tools/phase3/harness/reprint.rs"]
mod reprint;

use serde_json::{json, Value};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::{Mutex, PoisonError};
use tsr_compiler::FileCache;

/// The location of the last panic, so a fatal row names the code that
/// panicked: production (`crates/`) or this adapter (`tools/`, `examples/`).
static PANIC_LOCATION: Mutex<Option<String>> = Mutex::new(None);

fn last_panic() -> Option<String> {
    PANIC_LOCATION
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone()
}

const EMIT_UNSUPPORTED: &str = "Program.Emit is Phase 3 T8";

fn emit_domains(request: &Value, row: &mut Value) {
    if request["emit"] != true {
        for name in ["emit", "output", "sourcemap", "sourcemap_record"] {
            row[name] = json!({"state":"not_requested"});
        }
        return;
    }
    let unsupported = || executor::failure(EMIT_UNSUPPORTED, "unsupported");
    row["emit"] = unsupported();
    let output = &request["output"];
    row["output"] = if output["state"] == "disabled" {
        json!({"state":"disabled","reason":output["reason"]})
    } else {
        unsupported()
    };
    row["sourcemap"] = unsupported();
    row["sourcemap_record"] = unsupported();
}

fn observe(request: &Value, texts: bool) -> Value {
    let mut row = json!({"version":1,"id":request["id"],"acceptance_tier":request["acceptance_tier"],
        "mode":request["mode"]});
    // The harness's content mappers, when the configuration declares them,
    // serve the program for the whole observation.
    let scope = executor::content_mapper_scope(request, tsr_contentmappertest::new_spawner());
    let loaded = scope.and_then(|scope| {
        executor::load_fresh_checked(request, &mut FileCache::new()).map(|checked| (scope, checked))
    });
    match loaded {
        Ok((_scope, checked)) => {
            let program = checked.program();
            let libs = program
                .files()
                .iter()
                .filter(|file| {
                    file.bound()
                        .view()
                        .source_file()
                        .is_ok_and(|source| program.is_lib(source.parse_options().path.as_bytes()))
                })
                .count();
            row["load"] =
                json!({"state":"executed","files":program.files().len(),"libraries":libs});
            row["reprint"] = if request["reprint"] == true {
                reprint::observe(program, texts, &last_panic)
            } else {
                json!({"state":"not_requested"})
            };
        }
        Err(failure) => {
            row["load"] = failure;
            row["reprint"] = if request["reprint"] == true {
                json!({"state":"not_reached","reason":"the program did not load"})
            } else {
                json!({"state":"not_requested"})
            };
        }
    }
    emit_domains(request, &mut row);
    row
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    std::panic::set_hook(Box::new(|info| {
        *PANIC_LOCATION
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = info
            .location()
            .map(|at| format!("{}:{}", at.file(), at.line()));
        eprintln!("{info}");
    }));
    let args: Vec<_> = std::env::args_os().collect();
    let texts = args.len() == 4 && args[3] == "--texts";
    if args.len() != 3 && !texts {
        return Err("usage: phase3_emit REQUEST OUTPUT [--texts]".into());
    }
    let request: Value = serde_json::from_slice(&std::fs::read(&args[1])?)?;
    let row = match catch_unwind(AssertUnwindSafe(|| observe(&request, texts))) {
        Ok(row) => row,
        Err(payload) => {
            let reason = payload
                .downcast_ref::<String>()
                .map(String::as_str)
                .or_else(|| payload.downcast_ref::<&str>().copied())
                .unwrap_or("non-string panic payload");
            json!({"version":1,"id":request["id"],"acceptance_tier":request["acceptance_tier"],
                "fatal":executor::failure(reason,"panic"),"panic_location":last_panic()})
        }
    };
    std::fs::write(&args[2], serde_json::to_vec(&row)?)?;
    Ok(())
}
