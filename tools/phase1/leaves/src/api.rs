//! The contract every leaf group module answers.
//!
//! One module per coverage group, each owning its own file. `main` tries them
//! in turn and takes the first that claims the request, so adding a group is a
//! new module plus one line in `main`.

use serde_json::{json, Value};

/// What a group module observed for one request.
pub enum Outcome {
    /// The production Rust entry point ran. The payload is the observation.
    Observed(Value),
    /// The production entry point does not exist yet. Preparation records the
    /// gap; it never emulates the algorithm to make a comparison run.
    NotImplemented {
        go_authority: &'static str,
        intended_signature: &'static str,
        production_home: &'static str,
    },
    /// The driver claimed the request but could not answer it. Never a
    /// semantic result: a harness failure invalidates the capture.
    Failed(String),
}

impl Outcome {
    pub fn missing(
        go_authority: &'static str,
        intended_signature: &'static str,
        production_home: &'static str,
    ) -> Self {
        Outcome::NotImplemented {
            go_authority,
            intended_signature,
            production_home,
        }
    }
}

/// Read a request's declared subject.
pub fn subject(request: &Value) -> &str {
    request
        .get("subject")
        .and_then(Value::as_str)
        .unwrap_or_default()
}

/// The ordered actions of a trace request, empty when the case is not a trace.
pub fn actions(request: &Value) -> &[Value] {
    request
        .get("actions")
        .and_then(Value::as_array)
        .map_or(&[], |items| items.as_slice())
}

pub fn action_op(action: &Value) -> &str {
    action.get("op").and_then(Value::as_str).unwrap_or_default()
}

pub fn action_str<'a>(action: &'a Value, field: &str) -> &'a str {
    action.get(field).and_then(Value::as_str).unwrap_or_default()
}

pub fn action_i64(action: &Value, field: &str) -> i64 {
    action.get(field).and_then(Value::as_i64).unwrap_or_default()
}

/// Wrap an ordered trace in the shape an order-sensitive case requires: the
/// payload is a list under `ordered`, because the comparison canonicalises
/// with sorted keys and an object's member order would not survive it.
pub fn ordered(rows: Vec<Value>) -> Value {
    json!({ "ordered": rows })
}
