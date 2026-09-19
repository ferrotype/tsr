//! Frozen corpus diagnostics followed by native type/symbol and error baselines.
#[path = "../../../tools/s08/p5/baseline/mod.rs"]
mod baseline;
#[path = "../../../tools/s08/p5/corpus.rs"]
mod corpus;
#[path = "../../../tools/s08/p5/errors.rs"]
mod errors;
#[path = "../../../tools/s08/p4/executor.rs"]
mod executor;
#[path = "../../../tools/s08/p5/paths.rs"]
mod paths;
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args_os().collect();
    if args.len() != 3 {
        return Err("usage: p5_inventory REQUEST OUTPUT".into());
    }
    let request = serde_json::from_slice(&std::fs::read(&args[1])?)?;
    std::fs::write(&args[2], serde_json::to_vec(&corpus::observe(&request))?)?;
    Ok(())
}
