//! Current Rust config observations over the exact requested IDs.
#[path = "../../../tools/s07/config/rust_observation.rs"]
mod observation;
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().collect();
    if args.len() != 3 {
        return Err("usage: s07_config requests.json observations.json".into());
    }
    let requests: Vec<serde_json::Value> = serde_json::from_slice(&std::fs::read(&args[1])?)?;
    let rows = observation::observe_all(&requests)?;
    let mut output = serde_json::to_vec(&rows)?;
    output.push(b'\n');
    std::fs::write(&args[2], output)?;
    Ok(())
}
