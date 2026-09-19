//! Test-only transport for injected hosts. No compiler or language service runs here.
//! See docs/S11.md for the versioned, deliberately bounded protocol.
mod configuration;
mod filesystem;
pub mod framing;
mod protocol;
mod session;
mod streams;
mod wire;

pub use session::{OptionsToken, OptionsUpdate, Session};

/// Drive one connection. Pending operations are owned by the session; EOF drops
/// them together. Input is pumped while callbacks wait, never under a host lock.
pub fn serve<R: std::io::BufRead, W: std::io::Write>(
    reader: &mut R,
    writer: &mut W,
) -> std::io::Result<()> {
    let mut session = Session::default();
    while let Some(bytes) = framing::read(reader)? {
        let message = framing::parse_json(&bytes)?;
        let mut responses = session.receive(&message)?;
        // S11 stub: the endpoint owns application. No configuration round-trip.
        if let Some(update) = session.pending_options() {
            let token = update.token;
            responses.extend(session.complete_options(token, Ok(()))?);
        }
        for response in responses {
            framing::write(writer, response.get().as_bytes())?;
        }
        if session.is_closed() {
            return Ok(());
        }
    }
    if session.has_pending() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::UnexpectedEof,
            "test-host disconnected with pending callbacks",
        ));
    }
    Ok(())
}
