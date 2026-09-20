use std::io::{self, BufRead, Write};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Streaming transport keeps the complete frozen corpus out of the child's
    // memory. Each row loads, queries and drops a separate public API session.
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
