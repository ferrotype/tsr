//! The `internal/core` leaf group: tri-state option semantics, signed source
//! ranges, script kinds and single-wildcard patterns.
//!
//! Each operation drives its production home in `tsr_core` or `tsr_json`.
//!
//! Byte payloads travel as hex in both directions, because a Go observation of
//! a pattern's text cannot carry invalid UTF-8 through `encoding/json` and the
//! byte-domain cases exist precisely to pin those bytes.
//!
//! The numeric arguments are named for what they are (`tristate`, `kind`)
//! rather than taking the generic `value` key: every probe is handed the whole
//! family schedule, and a neighbouring group already decodes `value` as a
//! string into a typed action struct, which a number here would reject.

use std::panic::AssertUnwindSafe;

use serde_json::{json, Value};
use tsr_core::pattern::{find_best_pattern_match, Pattern};
use tsr_core::{ScriptKind, TextRange, Tristate};

use crate::api::{action_i64, action_op, action_str, actions, ordered, subject, Outcome};

pub fn observe(request: &Value) -> Option<Outcome> {
    if !subject(request).starts_with("core.") {
        return None;
    }
    if actions(request).iter().all(|a| action_op(a) == "string") {
        let mut rows = Vec::new();
        for action in actions(request) {
            let value = action_i64(action, "enum_value");
            let text = match subject(request) {
                "core.Tristate" => Tristate(value as u8).to_string(),
                "core.ScriptKind" => ScriptKind(value as i32).to_string(),
                "core.LanguageVariant" => tsr_core::LanguageVariant(value as i32).to_string(),
                "core.ModuleKind" => tsr_core::ModuleKind(value as i32).to_string(),
                "core.ScriptTarget" => tsr_core::ScriptTarget(value as i32).to_string(),
                _ => return None,
            };
            rows.push(json!({"op":"string","enum_value":value,"text":text}));
        }
        return Some(Outcome::Observed(ordered(rows)));
    }
    let replayed = match subject(request) {
        "core.Tristate" => Ok(tristate(actions(request))),
        "core.TextRange" => Ok(text_range(actions(request))),
        "core.ScriptKind" => script_kind(actions(request)),
        "core.Pattern" => pattern(actions(request)),
        "core.TristateJson" => tristate_json(actions(request)),
        "core.TextRangePredicates" => range_predicates(actions(request)),
        _ => return None,
    };
    Some(match replayed {
        Ok(rows) => Outcome::Observed(ordered(rows)),
        Err(problem) => Outcome::Failed(problem),
    })
}

fn tristate_json(trace: &[Value]) -> Result<Vec<Value>, String> {
    let mut rows = Vec::new();
    for action in trace {
        let op = action_op(action);
        rows.push(match op {
            "marshal" => {
                let t = action_i64(action, "tristate");
                let bytes = tsr_json::marshal(&Tristate(t as u8), tsr_json::Options::default())
                    .map_err(|e| e.to_string())?;
                json!({"op":op,"tristate":t,"json":String::from_utf8(bytes).unwrap(),"error":false})
            }
            "unmarshal" | "unmarshal_via_json" => {
                let raw = action_str(action, "raw");
                let mut t = Tristate::UNKNOWN;
                let error = if op == "unmarshal" {
                    t = Tristate::unmarshal_json(raw.as_bytes());
                    false
                } else {
                    // encoding/json v1 validates the whole document before
                    // invoking a custom UnmarshalJSON, unlike the incremental
                    // v2 decoder. Retain the old tristate if validation fails.
                    let mut value = tsr_json::RawValue::default();
                    let result = tsr_json::unmarshal(
                        raw.as_bytes(),
                        &mut value,
                        tsr_json::Options {
                            allow_duplicate_names: Some(true),
                            allow_invalid_utf8: Some(true),
                            ..tsr_json::Options::default()
                        },
                    );
                    if result.is_ok() {
                        t = Tristate::unmarshal_json(&value.0);
                    }
                    result.is_err()
                };
                json!({"op":op,"raw":raw,"result":t.0,"error":error})
            }
            _ => return Err(format!("unknown tristate JSON action {op}")),
        });
    }
    Ok(rows)
}
fn range_predicates(trace: &[Value]) -> Result<Vec<Value>, String> {
    let mut first = TextRange::default();
    let mut second = TextRange::default();
    let mut rows = Vec::new();
    for action in trace {
        let op = action_op(action);
        rows.push(match op {
            "undefined"|"make"|"make2" => {
                let range = match op {
                    "undefined"=>TextRange::undefined(),
                    "make"=>{ first=TextRange::new(action_i64(action,"pos"),action_i64(action,"end"));first },
                    _=>{ second=TextRange::new(action_i64(action,"pos2"),action_i64(action,"end2"));second }
                };
                json!({"op":op,"pos":range.pos(),"end":range.end(),"is_valid":range.is_valid()})
            }
            "contains" => { let pos=action_i64(action,"pos");json!({"op":op,"pos":pos,"contains":first.contains(pos),"contains_inclusive":first.contains_inclusive(pos),"contains_exclusive":first.contains_exclusive(pos)}) }
            "relate"=>json!({"op":op,"contained_by":first.contained_by(second),"overlaps":first.overlaps(second),"intersects":first.intersects(second),"compare":first.compare(second)}),
            "with_pos"|"with_end"=>{let range=if op=="with_pos"{first.with_pos(action_i64(action,"pos"))}else{first.with_end(action_i64(action,"end"))};json!({"op":op,"pos":range.pos(),"end":range.end()})}
            _=>return Err(format!("unknown range action {op}")),
        });
    }
    Ok(rows)
}

fn tristate(trace: &[Value]) -> Vec<Value> {
    let mut rows = Vec::with_capacity(trace.len());
    for action in trace {
        let op = action_op(action);
        let value = action_i64(action, "tristate");
        rows.push(match op {
            "predicates" => {
                let state = Tristate(value as u8);
                json!({
                    "op": op,
                    "tristate": value,
                    "is_true": state.is_true(),
                    "is_false": state.is_false(),
                    "is_unknown": state.is_unknown(),
                    "is_true_or_unknown": state.is_true_or_unknown(),
                    "is_false_or_unknown": state.is_false_or_unknown(),
                })
            }
            "default_if_unknown" => {
                let other = action_i64(action, "tristate_default");
                let result = Tristate(value as u8).default_if_unknown(Tristate(other as u8));
                json!({
                    "op": op,
                    "tristate": value,
                    "tristate_default": other,
                    "result": i64::from(result.0),
                })
            }
            "bool_to_tristate" => {
                let flag = action_bool(action, "flag");
                json!({ "op": op, "flag": flag, "result": i64::from(Tristate::from(flag).0) })
            }
            _ => unsupported(op),
        });
    }
    rows
}

fn text_range(trace: &[Value]) -> Vec<Value> {
    let mut rows = Vec::with_capacity(trace.len());
    for action in trace {
        let op = action_op(action);
        rows.push(match op {
            "new" => {
                let (pos, end) = (action_i64(action, "pos"), action_i64(action, "end"));
                let range = TextRange::new(pos, end);
                json!({
                    "op": op,
                    "pos_in": pos,
                    "end_in": end,
                    "pos": range.pos(),
                    "end": range.end(),
                    "len": range.len(),
                })
            }
            _ => unsupported(op),
        });
    }
    rows
}

fn script_kind(trace: &[Value]) -> Result<Vec<Value>, String> {
    let mut rows = Vec::with_capacity(trace.len());
    for action in trace {
        let op = action_op(action);
        rows.push(match op {
            "from_file_name" | "ensure_from_file_name" => {
                let name = input(action)?;
                let kind = if op == "from_file_name" {
                    ScriptKind::from_file_name(&name)
                } else {
                    ScriptKind::ensure_from_file_name(&name)
                };
                json!({ "op": op, "name_hex": hex(&name), "kind": i64::from(kind.0) })
            }
            "default_extension" => {
                let kind = action_i64(action, "kind");
                let extension = ScriptKind(kind as i32).default_extension();
                json!({
                    "op": op,
                    "kind": kind,
                    "extension": std::str::from_utf8(extension).unwrap_or_default(),
                })
            }
            _ => unsupported(op),
        });
    }
    Ok(rows)
}

fn pattern(trace: &[Value]) -> Result<Vec<Value>, String> {
    // The zero Pattern before any parse, which is the invalid value itself.
    let mut current = Pattern::default();
    let mut rows = Vec::with_capacity(trace.len());
    for action in trace {
        let op = action_op(action);
        rows.push(match op {
            "parse" => {
                current = Pattern::parse(&input(action)?);
                json!({
                    "op": op,
                    "text_hex": hex(&current.text),
                    "star_index": current.star_index as i64,
                    "is_valid": current.is_valid(),
                })
            }
            "is_valid" => json!({ "op": op, "is_valid": current.is_valid() }),
            "matches" => {
                let candidate = input(action)?;
                let (result, panicked) = guarded(AssertUnwindSafe(|| {
                    Value::Bool(current.matches(&candidate))
                }));
                json!({
                    "op": op,
                    "candidate_hex": hex(&candidate),
                    "result": result,
                    "panic": panicked,
                })
            }
            "matched_text" => {
                let candidate = input(action)?;
                let (result, panicked) = guarded(AssertUnwindSafe(|| {
                    Value::String(hex(current.matched_text(&candidate)))
                }));
                json!({
                    "op": op,
                    "candidate_hex": hex(&candidate),
                    "result": result,
                    "panic": panicked,
                })
            }
            "find_best" => {
                let candidate = input(action)?;
                let names = strings(action, "patterns");
                let values = numbers(action, "values");
                // Values are 1-based indices into `patterns`, so the zero value
                // the ported generic returns when nothing matches stays
                // distinguishable from a selected element. An index outside the
                // list is a malformed request, not a pinned behavior, so it
                // fails the case rather than being answered with a Pattern the
                // driver made up; the Go probe rejects the same trace.
                if let Some(index) = values
                    .iter()
                    .find(|value| **value < 1 || **value > names.len() as i64)
                {
                    return Err(format!(
                        "find_best index {index} is outside the {} pattern(s) of a core leaf \
                         action",
                        names.len()
                    ));
                }
                let choose = |value: &i64| Pattern::parse(&names[*value as usize - 1]);
                let (result, panicked) = guarded(AssertUnwindSafe(|| {
                    Value::from(find_best_pattern_match(&values, choose, &candidate))
                }));
                json!({
                    "op": op,
                    "candidate_hex": hex(&candidate),
                    "values": values,
                    "result": result,
                    "panic": panicked,
                })
            }
            _ => unsupported(op),
        });
    }
    Ok(rows)
}

fn unsupported(op: &str) -> Value {
    json!({ "op": op, "unsupported_action": op })
}

fn action_bool(action: &Value, field: &str) -> bool {
    action
        .get(field)
        .and_then(Value::as_bool)
        .unwrap_or_default()
}

fn strings(action: &Value, field: &str) -> Vec<Vec<u8>> {
    action
        .get(field)
        .and_then(Value::as_array)
        .map_or_else(Vec::new, |items| {
            items
                .iter()
                .map(|item| item.as_str().unwrap_or_default().as_bytes().to_vec())
                .collect()
        })
}

fn numbers(action: &Value, field: &str) -> Vec<i64> {
    action
        .get(field)
        .and_then(Value::as_array)
        .map_or_else(Vec::new, |items| {
            items
                .iter()
                .map(|item| item.as_i64().unwrap_or_default())
                .collect()
        })
}

/// An action's single string argument: hex when the case needs a byte string
/// the JSON request cannot carry, plain text otherwise.
fn input(action: &Value) -> Result<Vec<u8>, String> {
    let encoded = action_str(action, "hex");
    if encoded.is_empty() {
        return Ok(action_str(action, "text").as_bytes().to_vec());
    }
    if !encoded.len().is_multiple_of(2) {
        return Err(format!("odd-length hex {encoded:?} in a core leaf action"));
    }
    encoded
        .as_bytes()
        .chunks(2)
        .map(|pair| {
            std::str::from_utf8(pair)
                .ok()
                .and_then(|text| u8::from_str_radix(text, 16).ok())
                .ok_or_else(|| format!("malformed hex {encoded:?} in a core leaf action"))
        })
        .collect()
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

/// Runs `call` and converts a panic into a recorded class, so a case whose
/// subject is the panic boundary observes it instead of aborting the driver.
/// The default hook is silenced for the call: the panic is the expected result
/// here, and its message would otherwise land in the captured stderr.
///
/// Only the class is comparable. Go and Rust word the same failure differently,
/// so an unrecognized panic keeps its text under an `other:` prefix rather than
/// being folded into a class it does not belong to.
fn guarded(call: impl FnOnce() -> Value + std::panic::UnwindSafe) -> (Value, String) {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let result = std::panic::catch_unwind(call);
    std::panic::set_hook(previous);
    match result {
        Ok(value) => (value, String::new()),
        Err(payload) => (Value::Null, classify(payload.as_ref())),
    }
}

fn classify(payload: &(dyn std::any::Any + Send)) -> String {
    let message = payload
        .downcast_ref::<String>()
        .map(String::as_str)
        .or_else(|| payload.downcast_ref::<&str>().copied())
        .unwrap_or("non-string panic payload");
    if message.contains("out of range") || message.contains("out of bounds") {
        // Rust words a slice range and an index panic differently ("out of
        // range for slice of length" against "index out of bounds"), so both
        // are folded here, mirroring the two Go wordings the probe folds.
        "slice_bounds_out_of_range".to_owned()
    } else if message == "candidate does not match pattern" {
        "candidate_does_not_match_pattern".to_owned()
    } else {
        format!("other:{message}")
    }
}
