//! Live-OS action replay. Production calls execute inside a disposable root;
//! the renderer records portable classes, never host paths or expected values.
#[cfg(unix)]
mod fixtures;
#[cfg(unix)]
mod helpers;
#[cfg(unix)]
mod replay;
#[cfg(unix)]
mod walks;
#[cfg(unix)]
mod writes;
use crate::api::{subject, Outcome};
use serde_json::Value;
pub fn observe(request: &Value) -> Option<Outcome> {
    let name = subject(request);
    if !name.starts_with("osvfs.")
        && !name.starts_with("nativepath.")
        && name != "osutil.ProcessIdentity"
    {
        return None;
    }
    #[cfg(unix)]
    return Some(match replay::run(request) {
        Ok(v) => Outcome::Observed(v),
        Err(e) => Outcome::Failed(e),
    });
    #[cfg(not(unix))]
    Some(Outcome::Failed(
        "the native Unix fixture needs a matching host".into(),
    ))
}
