//! Ordered traces use the production containers. Go nullable receivers are
//! represented by Option: nil-tolerant methods use its iterator/clone/default
//! idioms, and methods requiring a receiver report the native nil-pointer
//! class. Domain algorithms, live iteration and diff ordering live in core.

use crate::api::{self, Outcome};
use serde_json::{json, Value};
use tsr_core::collections::{MapChange, OrderedMap, OrderedSet};

const NIL: &str = "nil_pointer_dereference";
type ResultValue = Result<Value, &'static str>;

fn required<T>(value: Option<T>) -> Result<T, &'static str> {
    value.ok_or(NIL)
}

fn result(row: &mut Value, outcome: ResultValue) {
    let (value, panic) = match outcome {
        Ok(value) => (value, ""),
        Err(panic) => (Value::Null, panic),
    };
    row["result"] = value;
    row["panic"] = json!(panic);
}

fn effect(row: &mut Value, outcome: Result<(), &'static str>) {
    row["panic"] = json!(outcome.err().unwrap_or_default());
}

fn entries(map: Option<&OrderedMap<String, String>>) -> Value {
    json!(map
        .into_iter()
        .flat_map(OrderedMap::entries)
        .collect::<Vec<_>>())
}

fn from_entries(action: &Value) -> OrderedMap<String, String> {
    action
        .get("entries")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|pair| {
            (
                pair.get(0)
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
                pair.get(1)
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
            )
        })
        .collect()
}

pub(super) fn map(request: &Value) -> Outcome {
    match map_trace(api::actions(request)) {
        Ok(rows) => Outcome::Observed(api::ordered(rows)),
        Err(error) => Outcome::Failed(error),
    }
}

fn render_json(encoded: Result<Vec<u8>, tsr_json::Error>) -> Value {
    match encoded {
        Ok(bytes) => json!([
            "bytes",
            String::from_utf8(bytes).expect("encoded JSON is UTF-8")
        ]),
        Err(error) => json!(["error", error.to_string()]),
    }
}

fn map_trace(actions: &[Value]) -> Result<Vec<Value>, String> {
    let mut map: Option<OrderedMap<String, String>> = None;
    let mut clone = None;
    let mut rows = Vec::new();
    for action in actions {
        let op = api::action_op(action);
        let key = api::action_str(action, "key");
        let value = api::action_str(action, "value");
        let mut row = json!({"op":op});
        match op {
            "new" => map = Some(OrderedMap::default()),
            "new_nil" => map = None,
            "new_size_hint" => {
                map = Some(OrderedMap::with_capacity(
                    usize::try_from(api::action_i64(action, "size"))
                        .map_err(|_| "invalid size hint")?,
                ));
            }
            "set" => effect(
                &mut row,
                required(map.as_mut()).map(|m| {
                    m.insert(key.into(), value.into());
                }),
            ),
            "clone_set" => effect(
                &mut row,
                required(clone.as_mut()).map(|m: &mut OrderedMap<String, String>| {
                    m.insert(key.into(), value.into());
                }),
            ),
            "marshal" => {
                let encoded = tsr_json::marshal(&map, tsr_json::Options::default());
                result(&mut row, Ok(render_json(encoded)));
            }
            "marshal_int_keys" => {
                let mut numbered = OrderedMap::<isize, String>::default();
                for pair in action["entries"].as_array().ok_or("missing entries")? {
                    let key = pair[0]
                        .as_str()
                        .ok_or("string key")?
                        .parse::<isize>()
                        .map_err(|e| e.to_string())?;
                    numbered.insert(key, pair[1].as_str().ok_or("string value")?.into());
                }
                result(
                    &mut row,
                    Ok(render_json(tsr_json::marshal(
                        &numbered,
                        tsr_json::Options::default(),
                    ))),
                );
            }
            "unmarshal" => {
                let source = api::action_str(action, "source");
                row["source"] = json!(source);
                let outcome = required(map.as_mut()).map(|map| {
                    let result =
                        tsr_json::unmarshal(source.as_bytes(), map, tsr_json::Options::default());
                    let category = match result {
                        Ok(()) => json!(["none", ""]),
                        Err(e)
                            if e.to_string()
                                .contains("cannot unmarshal non-object JSON value into Map") =>
                        {
                            json!([
                                "non_object",
                                "cannot unmarshal non-object JSON value into Map"
                            ])
                        }
                        Err(e) if e.is_unexpected_eof() => json!(["truncated", ""]),
                        Err(_) => json!(["decoder", ""]),
                    };
                    json!([category, entries(Some(map))])
                });
                result(&mut row, outcome);
            }
            "get" => result(
                &mut row,
                required(map.as_ref()).map(|m| {
                    let value = m.get(key);
                    json!([
                        value.map(String::as_str).unwrap_or_default(),
                        value.is_some()
                    ])
                }),
            ),
            "get_or_zero" => result(
                &mut row,
                required(map.as_ref()).map(|m| json!(m.get_or_default(key))),
            ),
            "delete" => result(
                &mut row,
                required(map.as_mut()).map(|m| {
                    let value = m.remove(key);
                    json!([value.as_deref().unwrap_or_default(), value.is_some()])
                }),
            ),
            "has" => result(
                &mut row,
                required(map.as_ref()).map(|m| json!(m.contains_key(key))),
            ),
            "size" => result(&mut row, Ok(json!(map.as_ref().map_or(0, OrderedMap::len)))),
            "keys" | "early_stop_keys" => {
                let limit = if op == "early_stop_keys" {
                    api::action_i64(action, "stop").max(0) as usize
                } else {
                    usize::MAX
                };
                result(
                    &mut row,
                    Ok(json!(map
                        .iter()
                        .flat_map(OrderedMap::keys)
                        .take(limit)
                        .collect::<Vec<_>>())),
                );
            }
            "entries" => result(&mut row, Ok(entries(map.as_ref()))),
            "clone_entries" => result(&mut row, Ok(entries(clone.as_ref()))),
            "values" => result(
                &mut row,
                Ok(json!(map
                    .iter()
                    .flat_map(OrderedMap::values)
                    .collect::<Vec<_>>())),
            ),
            "clear" => effect(&mut row, required(map.as_mut()).map(OrderedMap::clear)),
            "clone" => {
                clone.clone_from(&map);
                effect(&mut row, Ok(()));
            }
            "clone_is_nil" => result(&mut row, Ok(json!(map.clone().is_none()))),
            "from_list" => {
                map = Some(from_entries(action));
                result(&mut row, Ok(entries(map.as_ref())));
            }
            "entry_at" => {
                let index = api::action_i64(action, "index");
                row["index"] = json!(index);
                let index = isize::try_from(index).map_err(|_| "entry index exceeds Go int")?;
                result(
                    &mut row,
                    required(map.as_ref()).map(|m| {
                        m.entry_at(index).map_or_else(
                            || json!(["", "", false]),
                            |(key, value)| json!([key, value, true]),
                        )
                    }),
                );
            }
            "values_while_growing" => {
                let mut seen = Vec::new();
                if let Some(m) = map.as_mut() {
                    m.visit_entries_mut(|m, _, item| {
                        seen.push(item);
                        if seen.len() == 1 {
                            m.insert(key.into(), value.into());
                        }
                        seen.len() <= 64
                    });
                }
                result(&mut row, Ok(json!(seen)));
            }
            "diff" | "diff_func" => {
                let other = from_entries(action);
                let equality = if op == "diff" {
                    "exact"
                } else {
                    api::action_str(action, "equality")
                };
                if op == "diff_func" {
                    row["equality"] = json!(equality);
                }
                let equal: fn(&String, &String) -> bool = match equality {
                    "exact" => PartialEq::eq,
                    "length" => |a, b| a.len() == b.len(),
                    "always" => |_, _| true,
                    "never" => |_, _| false,
                    _ => return Err(format!("unknown ordered-map equality {equality:?}")),
                };
                let mut calls = Vec::new();
                let mut emit = |change| {
                    calls.push(match change {
                        MapChange::Added(k, v) => json!(["added", k, v]),
                        MapChange::Removed(k, v) => json!(["removed", k, v]),
                        MapChange::Modified(k, a, b) => json!(["modified", k, a, b]),
                    });
                };
                let output = match map.as_ref() {
                    None if !other.is_empty() => Err(NIL),
                    None => Ok(()),
                    Some(m) => {
                        if op == "diff" {
                            m.diff(&other, &mut emit);
                        } else {
                            m.diff_by(&other, equal, &mut emit);
                        }
                        Ok(())
                    }
                };
                result(&mut row, output.map(|()| json!(calls)));
            }
            _ => return Err(format!("unsupported ordered-map action {op:?}")),
        }
        rows.push(row);
    }
    Ok(rows)
}

pub(super) fn set(request: &Value) -> Outcome {
    match set_trace(api::actions(request)) {
        Ok(rows) => Outcome::Observed(api::ordered(rows)),
        Err(error) => Outcome::Failed(error),
    }
}

fn set_trace(actions: &[Value]) -> Result<Vec<Value>, String> {
    let mut set: Option<OrderedSet<String>> = None;
    let mut clone = None;
    let mut rows = Vec::new();
    for action in actions {
        let op = api::action_op(action);
        let key = api::action_str(action, "key");
        let mut row = json!({"op":op});
        match op {
            "new" => set = Some(OrderedSet::default()),
            "new_nil" => set = None,
            "new_size_hint" => {
                set = Some(OrderedSet::with_capacity(
                    usize::try_from(api::action_i64(action, "size"))
                        .map_err(|_| "invalid size hint")?,
                ));
            }
            "add" => effect(
                &mut row,
                required(set.as_mut()).map(|s| {
                    s.insert(key.into());
                }),
            ),
            "clone_add" => effect(
                &mut row,
                required(clone.as_mut()).map(|s: &mut OrderedSet<String>| {
                    s.insert(key.into());
                }),
            ),
            "delete" => result(
                &mut row,
                required(set.as_mut()).map(|s| json!(s.remove(key))),
            ),
            "has" => result(
                &mut row,
                required(set.as_ref()).map(|s| json!(s.contains(key))),
            ),
            "size" => result(&mut row, required(set.as_ref()).map(|s| json!(s.len()))),
            "clone_size" => result(&mut row, required(clone.as_ref()).map(|s| json!(s.len()))),
            "values" => result(
                &mut row,
                required(set.as_ref()).map(|s| json!(s.values().collect::<Vec<_>>())),
            ),
            "clone_values" => result(
                &mut row,
                required(clone.as_ref()).map(|s| json!(s.values().collect::<Vec<_>>())),
            ),
            "clear" => effect(&mut row, required(set.as_mut()).map(OrderedSet::clear)),
            "clone" => effect(
                &mut row,
                required(set.as_ref()).map(|s| {
                    clone = Some(s.clone());
                }),
            ),
            "early_stop_values" => result(
                &mut row,
                required(set.as_ref()).map(|s| {
                    json!(s
                        .values()
                        .take(api::action_i64(action, "stop").max(0) as usize)
                        .collect::<Vec<_>>())
                }),
            ),
            _ => return Err(format!("unsupported ordered-set action {op:?}")),
        }
        rows.push(row);
    }
    Ok(rows)
}
