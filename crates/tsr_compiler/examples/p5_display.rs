#[path = "../../../tools/s08/p5/display.rs"]
mod display;
use sha2::{Digest, Sha256};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args_os().collect();
    if args.len() != 3 {
        return Err("usage: p5_display REQUESTS OUTPUT".into());
    }
    let raw = std::fs::read(&args[1])?;
    let request = serde_json::from_slice(&raw)?;
    let mut result = display::observe(&request)?;
    result["request_sha256"] = serde_json::json!(format!("{:x}", Sha256::digest(&raw)));
    std::fs::write(&args[2], serde_json::to_vec(&result)?)?;
    Ok(())
}
