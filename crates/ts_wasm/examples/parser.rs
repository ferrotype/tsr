//! Native encoding control for the wasm byte-boundary fixtures.
use serde_json::{json, Value};
use std::io::{self, BufRead, Write};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut output = io::BufWriter::new(io::stdout().lock());
    for line in io::stdin().lock().lines() {
        let input: Value = serde_json::from_str(&line?)?;
        let source: Vec<u8> = serde_json::from_value(input["source"].clone())?;
        let name: Vec<u8> = serde_json::from_value(input["name"].clone())?;
        let kind = i32::try_from(input["kind"].as_i64().ok_or("script kind")?)?;
        let jsx = input["jsx"].as_bool().unwrap_or(false);
        let force = input["force"].as_bool().unwrap_or(false);
        let counts = ts_wasm::parse(&source, &name, kind, jsx, force);
        let bytes = ts_wasm::parse_and_encode(&source, &name, kind, jsx, force)
            .expect("native parser control must encode");
        serde_json::to_writer(&mut output, &json!({"counts":counts,"bytes":bytes}))?;
        output.write_all(b"\n")?;
    }
    Ok(())
}
