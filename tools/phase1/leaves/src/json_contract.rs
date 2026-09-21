//! Input construction and observation only. Typed and streaming operations use
//! the same production JSON implementation as config/options serialization.
mod codec;
use crate::api::{self, Outcome};
use serde_json::Value;
pub fn observe(request: &Value) -> Option<Outcome> {
    if !api::subject(request).starts_with("json.") {
        return None;
    }
    Some(match codec::replay(request) {
        Ok(rows) => Outcome::Observed(api::ordered(rows)),
        Err(e) => Outcome::Failed(e),
    })
}
