//! Executes immutable program loading for an independently selected request set.
//! Emits every result, including named unsupported branches and panic payloads.
use serde_json::{json, Value};
use std::{
    io::Write,
    panic::{catch_unwind, AssertUnwindSafe},
};
use ts_compiler as ts_compiler_error;
use ts_compiler::{FileCache, Program, ProgramOptions};
#[path = "../../../tools/s07/program/rust_observation.rs"]
mod observation;
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().collect();
    if args.len() != 3 && !(args.len() == 4 && args[3] == "--verify-options") {
        return Err(
            "usage: program_probe requests.json observations.jsonl [--verify-options]".into(),
        );
    }
    let requests: Vec<Value> = serde_json::from_slice(&std::fs::read(&args[1])?)?;
    let mut output = std::io::BufWriter::new(std::fs::File::create(&args[2])?);
    let counters = ts_arena::Counters::new();
    let mut cache = FileCache::new();
    for (index, request) in requests.iter().enumerate() {
        let id = request["id"].as_str().ok_or("missing ID")?;
        let result = catch_unwind(AssertUnwindSafe(|| {
            match observation::try_load(request, &mut cache, &counters, None) {
                Ok(program) => {
                    if args.len() == 4 {
                        observation::verify_options(id, &program)
                    } else {
                        observation::observe(id, &program)
                    }
                }
                Err(error) => {
                    let identity = if args.len() == 4 { "id" } else { "ID" };
                    json!({identity:id,"Error":format!("{error:?}")})
                }
            }
        }));
        let row = match result {
            Ok(row) => row,
            Err(payload) => {
                let message = payload.downcast_ref::<String>().map_or_else(
                    || {
                        payload
                            .downcast_ref::<&str>()
                            .copied()
                            .unwrap_or("non-string panic")
                    },
                    String::as_str,
                );
                let identity = if args.len() == 4 { "id" } else { "ID" };
                json!({identity:id,"Panic":message})
            }
        };
        serde_json::to_writer(&mut output, &row)?;
        writeln!(output)?;
        output.flush()?;
        cache.prune();
        if index % 100 == 0 {
            eprintln!("program requests {}/{}", index + 1, requests.len());
        }
    }
    Ok(())
}
