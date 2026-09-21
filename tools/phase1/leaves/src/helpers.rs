//! The generic core helper leaf group: the 25 operations of
//! `tsc/internal/core/core.go` that the rest of the compiler is written in.
//!
//! Twenty-four of them are recorded gaps. The workspace carries exactly one
//! `port:` annotation for any of them -- `crates/tsr_tsoptions/src/config_json.rs:22`
//! for `StringifyJson` -- and the other twenty-four are spelled out at each
//! call site as Rust iterator adapters rather than as named functions, so
//! there is no entry point to drive. Preparation records that; it never
//! emulates the algorithm here to make a comparison run.
//!
//! `StringifyJson` is driven for real, over exactly the domain its port
//! covers: that port is specialised twice, to `ConfigValue` rather than Go's
//! `any` and to the compact arm, because it takes no prefix or indent. A trace
//! that asks for an indent leaves the domain and is recorded as the gap it is.
//!
//! What each gap record has to carry is not the Go signature transliterated.
//! It is the part of the pinned body a Rust port silently drops, which for
//! this family is almost always one of four things: whether the result is nil
//! or a non-nil empty slice (they marshal as `null` and `[]`), whether the
//! result IS an input slice rather than a copy of one, how many times the
//! callback actually ran, or what the panic boundary is. The Go probe's trace
//! pins the answer; the `intended_signature` here is what the port must carry
//! to match it.

use crate::api::{action_op, action_str, actions, ordered, subject, Outcome};
use serde_json::{json, Value};
use tsr_jsstring::JsString;
use tsr_tsoptions::{stringify_json_indent, ConfigValue};

mod replay;

fn vocabulary(subject: &str) -> Option<&'static [&'static str]> {
    Some(match subject {
        "helpers.Slices" => &[
            "set",
            "alias",
            "slice_from",
            "slice_to",
            "read",
            "mutate",
            "same",
            "filter",
            "deduplicate",
            "append_if_unique",
            "concatenate",
            "map",
            "map_index",
            "map_filtered",
            "flat_map",
            "flatten",
            "flatten_register",
            "same_map",
            "some",
            "find",
            "find_last",
            "find_index",
            "first_or_nil",
            "last_or_nil",
        ],
        "helpers.SingleElementSlice" => &[
            "new_box",
            "clear_box",
            "single",
            "mutate_element",
            "read_box",
        ],
        "helpers.Memoize" => &["memoize", "memoize_panicking", "call"],
        "helpers.Must" => &["must"],
        "helpers.Values" => &["or_else", "if_else"],
        "helpers.ConcatenateSeq" => &["reset_seqs", "push_seq", "push_nil_seq", "collect"],
        "helpers.ComparableValues" => &["new_box", "new_nil_box", "strings", "pointers", "pairs"],
        "helpers.StringifyJson" => &["stringify"],
        _ => return None,
    })
}

fn hex(value: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(value.len() * 2);
    for byte in value {
        out.push(char::from(DIGITS[usize::from(byte >> 4)]));
        out.push(char::from(DIGITS[usize::from(byte & 0x0f)]));
    }
    out
}

/// Decode an action's byte argument: hex when the case needs bytes a JSON
/// string could not carry, plain text otherwise. Malformed hex is a malformed
/// request, not an observation.
fn input_bytes(action: &Value) -> Result<Vec<u8>, String> {
    let encoded = action_str(action, "hex");
    if encoded.is_empty() {
        return Ok(action_str(action, "text").as_bytes().to_vec());
    }
    if !encoded.len().is_multiple_of(2) {
        return Err(format!("action carries malformed hex {encoded:?}"));
    }
    let mut out = Vec::with_capacity(encoded.len() / 2);
    let raw = encoded.as_bytes();
    for pair in raw.chunks_exact(2) {
        let digit = |byte: u8| match byte {
            b'0'..=b'9' => Ok(byte - b'0'),
            b'a'..=b'f' => Ok(byte - b'a' + 10),
            b'A'..=b'F' => Ok(byte - b'A' + 10),
            _ => Err(format!("action carries malformed hex {encoded:?}")),
        };
        out.push(digit(pair[0])? << 4 | digit(pair[1])?);
    }
    Ok(out)
}

fn config_strings(action: &Value) -> Vec<ConfigValue> {
    action
        .get("items")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(|item| ConfigValue::String(JsString::from_bytes(item.as_bytes().to_vec())))
                .collect()
        })
        .unwrap_or_default()
}

/// The value a stringify action marshals. `Array(None)` is the port's model of
/// a Go typed nil slice -- convert_options.rs:190 builds exactly that where the
/// pinned parser has a nil `[]string` -- and `Null` is the nil interface; the
/// two are different documents (`[]` and `null`), which is half of what the
/// case pins.
fn stringify_input(action: &Value) -> Result<ConfigValue, String> {
    Ok(match action_str(action, "shape") {
        "string" => ConfigValue::String(JsString::from_bytes(input_bytes(action)?)),
        "strings" => ConfigValue::Array(Some(config_strings(action))),
        "strings_empty" => ConfigValue::Array(Some(Vec::new())),
        "strings_nil" => ConfigValue::Array(None),
        "nil_any" => ConfigValue::Null,
        "nested" => ConfigValue::Array(Some(
            action
                .get("groups")
                .and_then(Value::as_array)
                .map(|groups| {
                    groups
                        .iter()
                        .map(|group| {
                            group.as_array().map_or(ConfigValue::Array(None), |items| {
                                ConfigValue::Array(Some(
                                    items
                                        .iter()
                                        .filter_map(Value::as_str)
                                        .map(|item| {
                                            ConfigValue::String(JsString::from_bytes(
                                                item.as_bytes().to_vec(),
                                            ))
                                        })
                                        .collect(),
                                ))
                            })
                        })
                        .collect()
                })
                .unwrap_or_default(),
        )),
        other => return Err(format!("stringify action carries unknown shape {other:?}")),
    })
}

fn stringify(trace: &[Value]) -> Result<Vec<Value>, String> {
    let mut rows = Vec::with_capacity(trace.len());
    for action in trace {
        let input = stringify_input(action)?;
        let observed = stringify_json_indent(
            &input,
            action_str(action, "prefix"),
            action_str(action, "indent"),
        );
        rows.push(json!({
            "op": action_op(action),
            "shape": action_str(action, "shape"),
            "prefix": action_str(action, "prefix"),
            "indent": action_str(action, "indent"),
            "result_hex": hex(observed.as_deref().unwrap_or_default()),
            "error": observed.is_err(),
        }));
    }
    Ok(rows)
}

pub fn observe(request: &Value) -> Option<Outcome> {
    let declared = subject(request);
    let known = vocabulary(declared)?;
    for action in actions(request) {
        let op = action_op(action);
        if !known.contains(&op) {
            return Some(Outcome::Failed(format!(
                "unsupported action {op:?} for subject {declared:?}"
            )));
        }
    }
    if declared == "helpers.StringifyJson" {
        return Some(match stringify(actions(request)) {
            Ok(rows) => Outcome::Observed(ordered(rows)),
            Err(problem) => Outcome::Failed(problem),
        });
    }
    Some(match replay::replay(request) {
        Ok(rows) => Outcome::Observed(ordered(rows)),
        Err(problem) => Outcome::Failed(problem),
    })
}
