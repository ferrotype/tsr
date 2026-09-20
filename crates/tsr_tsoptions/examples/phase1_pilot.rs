//! Cargo target for the Phase 1 F0 pilot observation driver.
//!
//! The driver itself lives under tools/phase1/ with the other Phase 1 adapters,
//! following the s07/s08 convention: a thin example target includes it so the
//! observation code stays beside its manifests and Go bridges.

#[path = "../../../tools/phase1/pilot/rust_observation.rs"]
mod rust_observation;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() != 2 {
        return Err("usage: phase1_pilot requests.json observations.json".into());
    }
    rust_observation::run(&args[0], &args[1])
}
