#[path = "../../../tools/s08/p5/error_requests.rs"]
mod error_requests;
#[path = "../../../tools/s08/p5/errors.rs"]
mod errors;
use sha2::{Digest, Sha256};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args_os().collect();
    if args.len() != 3 {
        return Err("usage: p5_errors REQUEST OUTPUT".into());
    }
    let raw = std::fs::read(&args[1])?;
    let mut result = error_requests::observe(&serde_json::from_slice(&raw)?)?;
    result["request_sha256"] = serde_json::json!(format!("{:x}", Sha256::digest(&raw)));
    std::fs::write(&args[2], serde_json::to_vec(&result)?)?;
    Ok(())
}
