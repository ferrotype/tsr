//! Historical S10 frozen-subset adapter. This deliberately uses the internal
//! walker and does not certify the public embedding boundary (Phase 7 R4).
use std::io::{self, BufRead, Write};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let input = io::stdin();
    let mut output = io::BufWriter::new(io::stdout().lock());
    for line in input.lock().lines() {
        let request = serde_json::from_str(&line?)?;
        let row = s10_corpus::observe(&request, tsr_embed::Session::load);
        serde_json::to_writer(&mut output, &row)?;
        output.write_all(b"\n")?;
        output.flush()?;
    }
    Ok(())
}
