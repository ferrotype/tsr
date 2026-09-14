//! Diagnostic-only full frozen-corpus executor. No baseline parity is inferred.
#[path = "../../../tools/s08/p4/executor.rs"]
mod executor;
use executor::failure;
use serde_json::{json, Value};
use std::panic::{catch_unwind, AssertUnwindSafe};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().collect();
    if args.len() != 3 {
        return Err("usage: p4_inventory request.json observation.json".into());
    }
    let request: Value = serde_json::from_slice(&std::fs::read(&args[1])?)?;
    let result = catch_unwind(AssertUnwindSafe(|| {
        executor::observe(&request, |_, _, _, _| executor::BaselineResults {
            type_symbols: json!({"state":"not_implemented","reason":"P5 native baseline walker/display schedule"}),
            errors: json!({"state":"not_requested"}),
        })
    }));
    let row = match result {
        Ok(row) => row,
        Err(payload) => {
            let reason = payload
                .downcast_ref::<String>()
                .map(String::as_str)
                .or_else(|| payload.downcast_ref::<&str>().copied())
                .unwrap_or("non-string panic payload");
            json!({"version":1,"id":request["id"],"acceptance_tier":request["acceptance_tier"],"fatal":failure(reason,"panic")})
        }
    };
    let mut raw = serde_json::to_vec(&row)?;
    raw.push(b'\n');
    std::fs::write(&args[2], raw)?;
    Ok(())
}
