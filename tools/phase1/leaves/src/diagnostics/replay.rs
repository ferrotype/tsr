use super::{fingerprint, hex, lookup_row};
use crate::api::{self, Outcome};
use serde_json::{json, Map, Value};
use std::any::Any;
use std::panic::{catch_unwind, AssertUnwindSafe};
use tsr_diagnostics as diagnostics;
use tsr_locale::Locale;

pub(crate) fn panic_text(error: &(dyn Any + Send)) -> String {
    error
        .downcast_ref::<String>()
        .cloned()
        .or_else(|| error.downcast_ref::<&str>().map(ToString::to_string))
        .unwrap_or_else(|| "non-error panic".into())
}
fn bytes(action: &Value, field: &str) -> Result<Vec<u8>, String> {
    let encoded = api::action_str(action, &format!("{field}_hex"));
    if encoded.is_empty() {
        Ok(api::action_str(action, field).as_bytes().to_vec())
    } else {
        decode_hex(encoded)
    }
}
fn decode_hex(value: &str) -> Result<Vec<u8>, String> {
    if !value.len().is_multiple_of(2) {
        return Err("odd hex payload".into());
    }
    value
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            std::str::from_utf8(pair)
                .ok()
                .and_then(|pair| u8::from_str_radix(pair, 16).ok())
                .ok_or_else(|| "invalid hex payload".into())
        })
        .collect()
}
fn args(action: &Value) -> Result<Vec<Vec<u8>>, String> {
    if let Some(hex) = action["args_hex"].as_array() {
        hex.iter()
            .map(|v| decode_hex(v.as_str().unwrap()))
            .collect()
    } else {
        Ok(action["args"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|v| v.as_str().unwrap().as_bytes().to_vec())
            .collect())
    }
}
fn any_args(action: &Value) -> Result<Vec<diagnostics::Argument>, String> {
    use diagnostics::Argument as A;
    action["any_args"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|value| {
            Ok(match api::action_str(value, "kind") {
                "string" => A::Bytes(api::action_str(value, "text").as_bytes().to_vec()),
                "string_hex" => A::Bytes(decode_hex(api::action_str(value, "hex"))?),
                "int" => A::Int(api::action_i64(value, "int")),
                "float" => A::Float(value["number"].as_f64().unwrap_or_default()),
                "bool" => A::Bool(value["bool"].as_bool().unwrap_or_default()),
                "null" => A::Null,
                "strings" => A::List(
                    value["list"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .map(|v| A::Bytes(v.as_str().unwrap().as_bytes().to_vec()))
                        .collect(),
                ),
                kind => return Err(format!("unknown argument kind {kind}")),
            })
        })
        .collect()
}
fn selected(row: &mut Map<String, Value>, action: &Value) -> Locale {
    let name = api::action_str(action, "locale");
    let (locale, ok) = if name.is_empty() {
        (Locale::default(), true)
    } else {
        Locale::parse(name)
    };
    row.insert("locale".into(), json!(name));
    row.insert("tag".into(), json!(locale.tag_string()));
    row.insert("tag_parsed".into(), json!(ok));
    row.insert("tag_und".into(), json!(locale.is_default()));
    locale
}
fn result(row: &mut Map<String, Value>, call: impl FnOnce() -> Vec<u8>) {
    let value = catch_unwind(AssertUnwindSafe(call));
    row.insert("panicked".into(), json!(value.is_err()));
    match value {
        Ok(value) => row.insert("result_hex".into(), json!(hex(&value))),
        Err(error) => row.insert(
            "panic_hex".into(),
            json!(hex(panic_text(error.as_ref()).as_bytes())),
        ),
    };
}
fn replay(request: &Value) -> Result<Vec<Value>, String> {
    let mut rows = Vec::new();
    for action in api::actions(request) {
        let op = api::action_op(action);
        let key = api::action_str(action, "key");
        let mut row = Map::from_iter([("op".into(), json!(op))]);
        match op {
            "lookup_bytes" => {
                row.insert("key_hex".into(), action["key_hex"].clone());
                lookup_row(&mut row, diagnostics::by_key_bytes(&bytes(action, "key")?));
            }
            "category_name" | "category_string" => {
                let value = api::action_i64(action, "category") as i32;
                row.insert("category".into(), json!(value));
                if op == "category_string" {
                    row.insert(
                        "string".into(),
                        json!(diagnostics::Category::string_raw(value)),
                    );
                } else {
                    let value = catch_unwind(|| diagnostics::Category::name_raw(value));
                    row.insert("panicked".into(), json!(value.is_err()));
                    match value {
                        Ok(name) => row.insert("name".into(), json!(name)),
                        Err(error) => row.insert(
                            "panic_hex".into(),
                            json!(hex(panic_text(error.as_ref()).as_bytes())),
                        ),
                    };
                }
            }
            "format" | "format_message" => {
                let template = if op == "format" {
                    Some(bytes(action, "text")?)
                } else {
                    row.insert("key".into(), json!(key));
                    let message = diagnostics::by_key(key);
                    row.insert("found".into(), json!(message.is_some()));
                    message.map(|m| m.text.as_bytes().to_vec())
                };
                if let Some(template) = template {
                    let args = args(action)?;
                    row.insert("text_hex".into(), json!(hex(&template)));
                    row.insert("args_count".into(), json!(args.len()));
                    let refs: Vec<_> = args.iter().map(Vec::as_slice).collect();
                    result(&mut row, || diagnostics::format(&template, &refs));
                }
            }
            "localize_message" | "localize_key" | "message_localize" => {
                row.insert("key".into(), json!(key));
                let locale = selected(&mut row, action);
                let message = diagnostics::by_key(key);
                if op != "localize_key" {
                    row.insert("found".into(), json!(message.is_some()));
                    if message.is_none() {
                        rows.push(Value::Object(row));
                        continue;
                    }
                }
                if op == "message_localize" {
                    let args = any_args(action)?;
                    row.insert("args_count".into(), json!(args.len()));
                    result(&mut row, || message.unwrap().localize(&locale, &args));
                } else {
                    let args = args(action)?;
                    row.insert("args_count".into(), json!(args.len()));
                    if op == "localize_message" {
                        row.insert(
                            "translated".into(),
                            json!(diagnostics::localized_messages(&locale)
                                .is_some_and(|t| t.contains_key(key))),
                        );
                    }
                    let refs: Vec<_> = args.iter().map(Vec::as_slice).collect();
                    result(&mut row, || {
                        diagnostics::localize(
                            &locale,
                            if op == "localize_key" { None } else { message },
                            key.as_bytes(),
                            &refs,
                        )
                    });
                }
            }
            "localized_table" => {
                let locale = selected(&mut row, action);
                let table = diagnostics::localized_messages(&locale);
                let mut keys: Vec<_> = table.into_iter().flat_map(|table| table.keys()).collect();
                keys.sort();
                let mut data = Vec::new();
                let mut resolved = 0;
                for key in &keys {
                    if diagnostics::by_key(key).is_some() {
                        resolved += 1;
                    }
                    data.extend_from_slice(key.as_bytes());
                    data.push(0x1f);
                    data.extend_from_slice(table.unwrap()[*key].as_bytes());
                    data.push(0x1e);
                }
                row.insert("table_nil".into(), json!(table.is_none()));
                row.insert("size".into(), json!(keys.len()));
                row.insert("resolved_keys".into(), json!(resolved));
                row.insert("unknown_keys".into(), json!(keys.len() - resolved));
                row.insert("fingerprint".into(), json!(fingerprint(&data)));
                row.insert(
                    "samples".into(),
                    json!(action["keys"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .map(|key| {
                            let value = table.and_then(|table| table.get(key.as_str().unwrap()));
                            json!([
                                key,
                                value.is_some(),
                                hex(value.map_or(b"", String::as_bytes))
                            ])
                        })
                        .collect::<Vec<_>>()),
                );
            }
            "stringify" => {
                let values = diagnostics::stringify_args(&any_args(action)?);
                row.insert("nil_result".into(), json!(values.is_none()));
                row.insert("count".into(), json!(values.as_ref().map_or(0, Vec::len)));
                row.insert(
                    "values_hex".into(),
                    json!(values
                        .into_iter()
                        .flatten()
                        .map(|v| hex(&v))
                        .collect::<Vec<_>>()),
                );
            }
            "adhoc" | "adhoc_localize" => {
                let message = diagnostics::AdHocMessage::new(bytes(action, "text")?);
                if op == "adhoc" {
                    row.insert("code".into(), json!(message.code()));
                    row.insert("category".into(), json!(message.category() as i32));
                    row.insert("key_returned".into(), json!(message.key()));
                    row.insert("text_hex".into(), json!(hex(message.text())));
                } else {
                    let locale = selected(&mut row, action);
                    let args = args(action)?;
                    row.insert("args_count".into(), json!(args.len()));
                    let refs: Vec<_> = args.iter().map(Vec::as_slice).collect();
                    result(&mut row, || message.localize(&locale, &refs));
                }
            }
            _ => return Err(format!("unknown diagnostics action {op}")),
        }
        rows.push(Value::Object(row));
    }
    Ok(rows)
}
pub(super) fn observe(request: &Value) -> Outcome {
    match replay(request) {
        Ok(rows) => Outcome::Observed(json!({"ordered":rows})),
        Err(error) => Outcome::Failed(error),
    }
}
