//! The core/collections leaf group.
use crate::api::Outcome;
use serde_json::Value;

mod cow;
mod ordered;

mod hash;

pub fn observe(request: &Value) -> Option<Outcome> {
    let subject = crate::api::subject(request);
    if subject == "OrderedMap" {
        return Some(ordered::map(request));
    }
    if subject == "OrderedSet" {
        return Some(ordered::set(request));
    }
    if matches!(subject, "CopyOnWriteMap" | "CopyOnWriteSet") {
        return Some(cow::observe(request));
    }
    matches!(subject, "Set" | "MultiMap" | "SyncMap" | "SyncSet").then(|| hash::observe(request))
}
