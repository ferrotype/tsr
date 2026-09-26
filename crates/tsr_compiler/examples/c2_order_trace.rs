//! ADR 0010 source-program diagnostic driver, separate from corpus authority.
#[path = "../tests/support/c2_order_program.rs"]
mod program;
fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let request = serde_json::from_slice(&std::fs::read(std::env::var("C2_ORDER_REQUEST")?)?)?;
    let tracing = std::env::var("C2_ORDER_TRACE").is_ok_and(|value| value == "1");
    let observed = program::run(request, tracing)?;
    std::fs::write(
        std::env::var("C2_ORDER_OUTPUT")?,
        serde_json::to_vec(&observed)?,
    )?;
    Ok(())
}
