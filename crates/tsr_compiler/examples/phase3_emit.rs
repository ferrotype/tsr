//! Phase 3 corpus row (`scripts/phase3_corpus.py`): loads the request's
//! program as the pin's harness loads its pre-emit program, records the
//! reprint witness, then runs what the pin's runner runs for its `output`,
//! `sourcemap` and `sourcemap record` sub-tests (`tools/phase3/harness/
//! emit.rs`): the first compilation's post-emit program and its emit, the
//! harness's output ordering, and the three baseline writers with the
//! declaration re-compilation and the `noCheck` repeat. A domain the native
//! runner disables is `disabled` with the request's reason.
//!
//!     phase3_emit REQUEST OUTPUT [--texts]
//!
//! `--texts` keeps the bytes (`text_hex`) of each reprint, each emitted file
//! and each composed baseline: a debugging aid, never used by the corpus.
#[path = "../../../tools/phase3/harness/baselines.rs"]
#[allow(dead_code)]
mod baselines;
#[path = "../../../tools/phase3/harness/declaration_program.rs"]
#[allow(dead_code)]
mod declaration_program;
#[path = "../../../tools/phase3/harness/emit.rs"]
mod emit;
#[path = "../../../tools/s08/p5/errors.rs"]
#[allow(dead_code)]
mod errors;
#[path = "../../../tools/s08/p4/executor.rs"]
#[allow(dead_code)]
mod executor;
#[path = "../../../tools/s08/p5/paths.rs"]
mod paths;
#[path = "../../../tools/phase3/harness/program_view.rs"]
mod program_view;
#[path = "../../../tools/phase3/harness/reprint.rs"]
mod reprint;

use serde_json::{json, Value};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::{Mutex, PoisonError};
use tsr_compiler::{CheckedProgram, FileCache};

/// The location of the last panic, so a fatal row names the code that
/// panicked: production (`crates/`) or this adapter (`tools/`, `examples/`).
static PANIC_LOCATION: Mutex<Option<String>> = Mutex::new(None);

fn last_panic() -> Option<String> {
    PANIC_LOCATION
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone()
}

const BASELINE_DOMAINS: [&str; 3] = ["output", "sourcemap", "sourcemap_record"];

/// Every emit domain is `failure`, except an `output` the runner disables.
fn all_failed(request: &Value, row: &mut Value, failure: &Value) {
    row["emit"] = failure.clone();
    for name in BASELINE_DOMAINS {
        row[name] = failure.clone();
    }
    disable_output(request, row);
}

fn disable_output(request: &Value, row: &mut Value) {
    let output = &request["output"];
    if output["state"] == "disabled" {
        row["output"] = json!({"state":"disabled","reason":output["reason"]});
    }
}

/// The emit domains of a row whose pre-emit program loaded.
fn emit_domains(
    request: &Value,
    scope: executor::ContentMapperScope,
    pre: &CheckedProgram,
    cache: &mut FileCache,
    texts: bool,
    row: &mut Value,
) {
    let inputs = match emit::RequestInputs::parse(request) {
        Ok(inputs) => inputs,
        Err(failure) => return all_failed(request, row, &failure),
    };
    let first = catch_unwind(AssertUnwindSafe(|| {
        emit::compile_files(request, Some(pre), inputs.capture_suggestions(), cache)
    }));
    // The first compilation's content-mapper host closes with it.
    drop(scope);
    let first = match first {
        Ok(Ok(first)) => first,
        Ok(Err(failure)) => return all_failed(request, row, &failure),
        Err(payload) => {
            return all_failed(
                request,
                row,
                &emit::panic_failure(payload.as_ref(), last_panic().as_deref()),
            );
        }
    };
    let facts = match program_view::ProgramFacts::new(first.post.program().clone()) {
        Ok(facts) => facts,
        Err(failure) => {
            return all_failed(request, row, &executor::failure(failure, "harness"));
        }
    };
    let recorded = emit::recorded(&first);
    let result = match emit::compilation_result(&facts, &first, &recorded) {
        Ok(result) => result,
        Err(failure) => {
            return all_failed(request, row, &executor::failure(failure, "harness"));
        }
    };
    row["emit"] = emit::emit_json(&first, &result, texts);
    let mut compilations = vec![first.counts("first")];
    let writer_inputs = inputs.writer_inputs();
    row["output"] = if request["output"]["state"] == "disabled" {
        json!({"state":"disabled","reason":request["output"]["reason"]})
    } else {
        emit::guarded(&last_panic, || {
            emit::output(
                request,
                &writer_inputs,
                &result,
                cache,
                &mut compilations,
                texts,
            )
        })
    };
    row["compilations"] = json!(compilations);
    row["sourcemap"] = emit::guarded(&last_panic, || {
        emit::sourcemap(&writer_inputs, &result, texts)
    });
    row["sourcemap_record"] = emit::guarded(&last_panic, || {
        emit::sourcemap_record(&writer_inputs, &result, texts)
    });
}

fn observe(request: &Value, texts: bool) -> Value {
    let mut row = json!({"version":1,"id":request["id"],"acceptance_tier":request["acceptance_tier"],
        "mode":request["mode"],"compilations":[]});
    let mut cache = FileCache::new();
    // The harness's content mappers, when the configuration declares them,
    // serve the first compilation's two programs.
    let scope = executor::content_mapper_scope(request, tsr_contentmappertest::new_spawner());
    let loaded = scope.and_then(|scope| {
        executor::load_fresh_checked(request, &mut cache).map(|pre| (scope, pre))
    });
    match loaded {
        Ok((scope, pre)) => {
            let program = pre.program();
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
            if request["emit"] == true {
                emit_domains(request, scope, &pre, &mut cache, texts, &mut row);
            }
        }
        Err(failure) => {
            row["load"] = failure.clone();
            row["reprint"] = if request["reprint"] == true {
                json!({"state":"not_reached","reason":"the program did not load"})
            } else {
                json!({"state":"not_requested"})
            };
            if request["emit"] == true {
                all_failed(request, &mut row, &failure);
            }
        }
    }
    if request["emit"] != true {
        row["emit"] = json!({"state":"not_requested"});
        for name in BASELINE_DOMAINS {
            row[name] = json!({"state":"not_requested"});
        }
    }
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
