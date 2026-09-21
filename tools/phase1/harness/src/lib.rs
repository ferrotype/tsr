//! Private Phase 1 group-dispatch contract shared by every Rust probe family.
//! Handlers report the missing operation they found; the request label is only
//! the schedule identity and never supplies a missing-operation identity.

use serde_json::{json, Map, Value};

/// What a group module observed for one request.
pub enum Outcome {
    /// The production Rust entry point ran. The payload is the observation.
    Observed(Value),
    /// The production entry point does not exist yet. Preparation records the
    /// gap; it never emulates the algorithm to make a comparison run.
    NotImplemented {
        operation: String,
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
        operation: impl Into<String>,
        go_authority: &'static str,
        intended_signature: &'static str,
        production_home: &'static str,
    ) -> Self {
        Outcome::NotImplemented {
            operation: operation.into(),
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
    action
        .get(field)
        .and_then(Value::as_str)
        .unwrap_or_default()
}

pub fn action_i64(action: &Value, field: &str) -> i64 {
    action
        .get(field)
        .and_then(Value::as_i64)
        .unwrap_or_default()
}

/// Wrap an ordered trace in the shape an order-sensitive case requires: the
/// payload is a list under `ordered`, because the comparison canonicalises
/// with sorted keys and an object's member order would not survive it.
// The vector is moved straight into the JSON array; taking a slice would force
// a clone of every row for no benefit.
#[allow(clippy::needless_pass_by_value)]
pub fn ordered(rows: Vec<Value>) -> Value {
    json!({ "ordered": rows })
}

/// Resolve a group-wide gap against its reviewed entry points. Subject dispatch
/// alone cannot make the caller's arbitrary operation label authoritative.
pub fn missing_for_subject(
    request: &Value,
    operations: &[(&str, &str)],
    authority: &'static str,
    signature: &'static str,
    home: &'static str,
) -> Outcome {
    let requested = request
        .get("operation")
        .and_then(Value::as_str)
        .unwrap_or_default();
    match operations
        .iter()
        .find(|(owner, identity)| *owner == subject(request) && *identity == requested)
    {
        Some((_, identity)) => Outcome::missing(*identity, authority, signature, home),
        None => Outcome::Failed(format!(
            "no reviewed missing operation {requested:?} for subject {:?}",
            subject(request)
        )),
    }
}

/// Serialize a handler result while preserving the separate schedule identity.
pub fn response(request: &Value, outcome: Outcome) -> Map<String, Value> {
    let mut row = Map::new();
    for field in ["case", "operation"] {
        row.insert(
            field.into(),
            Value::String(
                request
                    .get(field)
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
            ),
        );
    }
    match outcome {
        Outcome::Observed(value) => {
            row.insert("result".into(), Value::String("observed".into()));
            row.insert("observation".into(), value);
        }
        Outcome::NotImplemented {
            operation,
            go_authority,
            intended_signature,
            production_home,
        } => {
            row.insert("result".into(), Value::String("not_implemented".into()));
            row.insert(
                "missing_operation".into(),
                json!({
                    "operation": operation,
                    "go_authority": go_authority,
                    "intended_signature": intended_signature,
                    "production_home": production_home,
                }),
            );
        }
        Outcome::Failed(error) => {
            row.insert("result".into(), Value::String("harness_failed".into()));
            row.insert("error".into(), Value::String(error));
        }
    }
    row
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_identity_comes_from_handler_not_schedule() {
        let row = response(
            &json!({"case":"trace", "operation":"caller-label"}),
            Outcome::missing("absent-callee", "Go authority", "signature", "home"),
        );
        assert_eq!(row["operation"], "caller-label");
        assert_eq!(row["missing_operation"]["operation"], "absent-callee");
    }

    #[test]
    fn unrelated_subject_or_identity_cannot_claim_a_reviewed_gap() {
        for request in [
            json!({"subject":"other", "operation":"missing"}),
            json!({"subject":"known", "operation":"other"}),
        ] {
            let result = missing_for_subject(
                &request,
                &[("known", "missing")],
                "authority",
                "signature",
                "home",
            );
            assert_eq!(response(&request, result)["result"], "harness_failed");
        }
    }
}
