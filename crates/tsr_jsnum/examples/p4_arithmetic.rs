//! Native-probe companion; inputs/outputs are IEEE-754 bits so no formatter or
//! JSON number conversion can hide a last-bit difference.
use std::io::{self, BufRead};
use ts_jsnum::Number;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    for line in io::stdin().lock().lines() {
        let line = line?;
        let mut fields = line.split_whitespace();
        let base: u64 = fields.next().ok_or("missing base")?.parse()?;
        let exponent: u64 = fields.next().ok_or("missing exponent")?.parse()?;
        if fields.next().is_some() {
            return Err("extra arithmetic input".into());
        }
        let result =
            Number::new(f64::from_bits(base)).exponentiate(Number::new(f64::from_bits(exponent)));
        println!("{}", result.value().to_bits());
    }
    Ok(())
}
