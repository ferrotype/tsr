//! Native corpus phase execution followed by the production type/symbol walker.
#[path = "../../../tools/s08/p5/baseline/mod.rs"]
mod baseline;
#[path = "../../../tools/s08/p4/executor.rs"]
mod executor;
use serde_json::{json, Value};

fn unhex(value: &Value) -> Result<Vec<u8>, &'static str> {
    let text = value.as_str().ok_or("missing hex bytes")?;
    if !text.len().is_multiple_of(2) {
        return Err("odd hex length");
    }
    fn digit(b: u8) -> Result<u8, &'static str> {
        match b {
            b'0'..=b'9' => Ok(b - b'0'),
            b'a'..=b'f' => Ok(b - b'a' + 10),
            _ => Err("noncanonical hex digit"),
        }
    }
    text.as_bytes()
        .chunks_exact(2)
        .map(|p| Ok((digit(p[0])? << 4) | digit(p[1])?))
        .collect()
}

fn diagnostic_presence(phase: &Value) -> Result<bool, &'static str> {
    if phase["state"] != "executed" {
        return Err("requested diagnostics did not complete before baseline walk");
    }
    if let Some(files) = phase["files"].as_array() {
        let mut present = false;
        for file in files {
            present |= diagnostic_presence(&file["result"])?;
        }
        return Ok(present);
    }
    phase["diagnostics"]
        .as_array()
        .map(|d| !d.is_empty())
        .ok_or("diagnostic phase lacks payload")
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args_os().collect();
    if args.len() != 3 {
        return Err("usage: p5_inventory REQUEST OUTPUT".into());
    }
    let raw = std::fs::read(&args[1])?;
    let request: Value = serde_json::from_slice(&raw)?;
    let mut trace = baseline::Trace::default();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        executor::observe(&request, |program, op, phases| {
            let result = (|| {
                let mut had_errors = false;
                for phase in phases
                    .as_object()
                    .ok_or("missing diagnostic phases")?
                    .values()
                {
                    had_errors |= diagnostic_presence(phase)?;
                }
                let contents = request["baseline_inputs"]
                    .as_array()
                    .ok_or("missing native baseline input order")?
                    .iter()
                    .map(|f| Ok((unhex(&f["name_hex"])?, unhex(&f["content_hex"])?)))
                    .collect::<Result<Vec<_>, &str>>()?;
                let files: Vec<_> = contents
                    .iter()
                    .map(|(name, content)| baseline::InputFile { name, content })
                    .collect();
                let header = request["baseline_header"]
                    .as_str()
                    .ok_or("missing baseline header")?;
                Ok(baseline::generate(
                    program,
                    op,
                    &files,
                    header.as_bytes(),
                    had_errors,
                    &mut trace,
                ))
            })();
            result.unwrap_or_else(|reason: &str| executor::failure(reason, "baseline_prerequisite"))
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
            json!({"version":1,"id":request["id"],"acceptance_tier":request["acceptance_tier"],"fatal":executor::failure(reason,"panic"),"queries":trace.queries,"active_query":trace.active})
        }
    };
    std::fs::write(&args[2], serde_json::to_vec(&row)?)?;
    Ok(())
}
