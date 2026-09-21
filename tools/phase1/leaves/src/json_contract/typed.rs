//! Adapter for the typed marshal surface. Raw-token and streaming actions
//! continue through the explicit missing-operation path in the parent module.
use crate::api::{self, Outcome};
use serde_json::{json, Value};
use std::collections::HashMap;
use tsr_json::{Encode, Encoder, Error, Options};
use tsr_jsstring::JsString;

pub(super) fn observe(request: &Value) -> Option<Outcome> {
    if !matches!(api::subject(request), "json.Marshal" | "json.MarshalIndent") {
        return None;
    }
    if api::actions(request).iter().any(|a| {
        !matches!(api::action_op(a), "marshal" | "marshal_indent")
            || !matches!(
                value_kind(request, a),
                "string" | "map" | "sample" | "float"
            )
    }) {
        return None;
    }
    Some(match replay(request) {
        Ok(rows) => Outcome::Observed(api::ordered(rows)),
        Err(error) => Outcome::Failed(error),
    })
}
fn fallback<'a>(a: &'a Value, field: &str, default: &'a str) -> &'a str {
    let value = api::action_str(a, field);
    if value.is_empty() {
        default
    } else {
        value
    }
}
fn value_kind<'a>(request: &'a Value, action: &'a Value) -> &'a str {
    fallback(
        action,
        "value_kind",
        api::action_str(&request["value"], "kind"),
    )
}
fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}
fn decode_hex(s: &str) -> Result<Vec<u8>, String> {
    if !s.len().is_multiple_of(2) || !s.is_ascii() {
        return Err("invalid hex input".into());
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).map_err(|e| e.to_string()))
        .collect()
}
// The sample struct is the native probe's declared input schema, not a JSON
// implementation: member writing and omission decisions feed the shared codec.
struct Sample {
    count: Option<i64>,
    items: Vec<String>,
    populated: bool,
}
impl Encode for Sample {
    fn encode(&self, out: &mut Encoder<'_>) -> Result<(), Error> {
        let name = if self.populated { "a" } else { "" };
        let extra = "e";
        let mut fields: Vec<(&[u8], &dyn Encode)> = vec![
            (b"name", &name),
            (b"count", &self.count),
            (b"items", &self.items),
        ];
        if self.populated {
            fields.push((b"extra", &extra));
        }
        out.object(fields)
    }
}
fn replay(request: &Value) -> Result<Vec<Value>, String> {
    let mut options = Options::default();
    for option in request["options"].as_array().into_iter().flatten() {
        match api::action_str(option, "option") {
            "Deterministic" => {
                options.deterministic = option["enabled"]
                    .as_bool()
                    .ok_or("enabled must be boolean")?;
            }
            "AllowDuplicateNames" => {
                options.allow_duplicate_names = option["enabled"]
                    .as_bool()
                    .ok_or("enabled must be boolean")?;
            }
            "WithIndent" => options.indent = Some(api::action_str(option, "indent")),
            name => return Err(format!("unknown JSON option {name:?}")),
        }
    }
    let mut rows = Vec::new();
    for action in api::actions(request) {
        let kind = value_kind(request, action);
        let text = fallback(
            action,
            "value_text",
            api::action_str(&request["value"], "text"),
        );
        let mut float = None;
        let input: Box<dyn Encode> = match kind {
            "string" => Box::new(JsString::from_bytes(decode_hex(fallback(
                action,
                "input_hex",
                api::action_str(request, "input_hex"),
            ))?)),
            "float" => {
                let value = text.parse::<f64>().map_err(|e| e.to_string())?;
                float = Some(value);
                Box::new(value)
            }
            "map" => {
                let mut map = HashMap::<String, String>::new();
                for pair in request["value"]["entries"]
                    .as_array()
                    .ok_or("missing map entries")?
                {
                    let pair = pair
                        .as_array()
                        .filter(|a| a.len() == 2)
                        .ok_or("bad map entry")?;
                    map.insert(
                        pair[0].as_str().ok_or("string key")?.into(),
                        pair[1].as_str().ok_or("string value")?.into(),
                    );
                }
                Box::new(map)
            }
            "sample" => {
                if !matches!(text, "" | "zero" | "empty_slice" | "populated") {
                    return Err(format!("unknown sample {text:?}"));
                }
                let populated = text == "populated";
                Box::new(Sample {
                    count: populated.then_some(7),
                    items: if populated {
                        vec!["x".into(), "y".into()]
                    } else {
                        vec![]
                    },
                    populated,
                })
            }
            _ => return Err(format!("unsupported typed value {kind}")),
        };
        let op = api::action_op(action);
        let mut row = json!({"op":op,"value_kind":kind});
        let result = if op == "marshal_indent" {
            row["prefix"] = action["prefix"].clone();
            row["indent"] = action["indent"].clone();
            tsr_json::marshal_indent(
                input.as_ref(),
                api::action_str(action, "prefix"),
                api::action_str(action, "indent"),
            )
        } else {
            tsr_json::marshal(input.as_ref(), options.clone())
        };
        let output = match result {
            Ok(bytes) => {
                row["ok"] = json!(true);
                bytes
            }
            Err(Error::NonFiniteNumber) if kind == "float" => {
                // Normalise this Rust error into the Go probe's top-level
                // float64 error schema. No nested/raw error positions claimed.
                let value = float.ok_or("missing float input")?;
                let spelling = if value.is_nan() {
                    "NaN"
                } else if value.is_sign_negative() {
                    "-Inf"
                } else {
                    "+Inf"
                };
                let cause = format!("unsupported value: {spelling}");
                row["ok"] = json!(false);
                row["error"] = json!(format!("json: cannot marshal from Go float64: {cause}"));
                row["error_cause"] = json!(cause);
                row["error_class"] = json!("SemanticError");
                row["error_go_type"] = json!("float64");
                row["error_json_kind"] = json!("invalid");
                row["error_byte_offset"] = json!(0);
                row["error_json_pointer"] = json!("");
                row["error_unexpected_eof"] = json!(false);
                Vec::new()
            }
            Err(error) => {
                return Err(format!(
                    "typed JSON error has no native projection: {error}"
                ))
            }
        };
        row["output_hex"] = json!(hex(&output));
        row["output_len"] = json!(output.len());
        if let Ok(text) = std::str::from_utf8(&output) {
            row["output_text"] = json!(text);
        }
        rows.push(row);
    }
    Ok(rows)
}
