use crate::api::{self, Outcome};
use serde_json::{json, Value};
use tsr_core::collections::{MultiMap, Set, SetKeys, SyncMap, SyncSet, Values};

fn strings(action: &Value) -> Result<Vec<String>, String> {
    action["items"]
        .as_array()
        .ok_or("missing items")?
        .iter()
        .map(|v| {
            v.as_str()
                .map(str::to_owned)
                .ok_or_else(|| "item must be string".into())
        })
        .collect()
}
fn sorted(items: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut items: Vec<_> = items.into_iter().collect();
    items.sort();
    items
}
fn guarded(row: &mut Value, value: bool, call: impl FnOnce() -> Value) {
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(call));
    match result {
        Ok(result) => {
            if value {
                row["result"] = result;
            }
            row["panic"] = json!("");
        }
        Err(payload) => {
            let text = payload
                .downcast_ref::<String>()
                .map(String::as_str)
                .or_else(|| payload.downcast_ref::<&str>().copied())
                .unwrap_or("non-error panic");
            let class = if text.contains("nil pointer dereference") {
                "nil_pointer_dereference".into()
            } else if text == "nil interface conversion" {
                "nil_interface_conversion".into()
            } else {
                format!("other:{text}")
            };
            row["panic"] = json!(class);
            if value {
                row["result"] = Value::Null;
            }
        }
    }
}
fn required<T>(value: Option<T>) -> T {
    value.expect("nil pointer dereference")
}
fn key_result(set: Option<&Set<String>>) -> Value {
    let keys = set.and_then(Set::keys);
    json!([
        sorted(keys.iter().flat_map(|keys| keys.iter().cloned())),
        keys.is_none()
    ])
}
fn captured_keys(keys: Option<&SetKeys<String>>) -> Value {
    json!([
        keys.map_or_else(Vec::new, |keys| sorted(keys.read().iter().cloned())),
        keys.is_none()
    ])
}
fn slot(name: &str) -> usize {
    match name {
        "t" => 1,
        "u" => 2,
        _ => 0,
    }
}
fn set_trace(request: &Value) -> Result<Vec<Value>, String> {
    let mut sets: [Option<Set<String>>; 3] = [None, None, None];
    let mut captured = None;
    let mut rows = vec![];
    for action in api::actions(request) {
        let op = api::action_op(action);
        let target = api::action_str(action, "target");
        let i = slot(target);
        let other = api::action_str(action, "other");
        let j = if other.is_empty() {
            usize::from(i != 1)
        } else {
            slot(other)
        };
        let key = api::action_str(action, "key");
        let mut row = json!({"op":op, "target":target});
        match op {
            "new" => sets[i] = Some(Set::default()),
            "new_nil" => sets[i] = None,
            "new_size_hint" => {
                sets[i] = Some(Set::with_capacity(
                    api::action_i64(action, "size").max(0) as usize
                ));
            }
            "new_from_items" => sets[i] = Some(strings(action)?.into_iter().collect()),
            "add" => guarded(&mut row, false, || {
                required(sets[i].as_mut()).insert(key.into());
                Value::Null
            }),
            "add_if_absent" => guarded(&mut row, true, || {
                json!(required(sets[i].as_mut()).insert_if_absent(key.into()))
            }),
            "delete" => guarded(&mut row, false, || {
                required(sets[i].as_mut()).remove(key);
                Value::Null
            }),
            "has" => guarded(&mut row, true, || {
                json!(sets[i].as_ref().is_some_and(|s| s.contains(key)))
            }),
            "len" => guarded(&mut row, true, || {
                json!(sets[i].as_ref().map_or(0, Set::len))
            }),
            "keys" => guarded(&mut row, true, || key_result(sets[i].as_ref())),
            "keys_capture" => guarded(&mut row, true, || {
                captured = sets[i].as_ref().and_then(Set::retain_keys);
                captured_keys(captured.as_ref())
            }),
            "keys_replay" => guarded(&mut row, true, || captured_keys(captured.as_ref())),
            "clear" => guarded(&mut row, false, || {
                if let Some(s) = &mut sets[i] {
                    s.clear();
                }
                Value::Null
            }),
            "clone" => guarded(&mut row, true, || {
                let cloned = sets[i].clone();
                sets[2] = cloned;
                json!(sets[2].is_none())
            }),
            "union" => guarded(&mut row, false, || {
                // Clone only the test operand to express Go's aliasable receiver
                // call through Rust's exclusive mutable borrow.
                let other = sets[j].clone();
                Set::union_into(sets[i].as_mut(), other.as_ref());
                Value::Null
            }),
            "unioned_with" => guarded(&mut row, true, || {
                sets[2] = Set::unioned_with(sets[i].as_ref(), sets[j].as_ref());
                json!(sets[2].is_none())
            }),
            "equals" => guarded(&mut row, true, || {
                json!(Set::equals(sets[i].as_ref(), sets[j].as_ref()))
            }),
            "is_subset_of" => guarded(&mut row, true, || {
                json!(Set::is_subset_of(sets[i].as_ref(), sets[j].as_ref()))
            }),
            "intersects" => guarded(&mut row, true, || {
                json!(Set::intersects(sets[i].as_ref(), sets[j].as_ref()))
            }),
            _ => return Err(format!("unknown Set action {op}")),
        }
        rows.push(row);
    }
    Ok(rows)
}
fn values_result(values: Option<&[String]>) -> Value {
    json!([values.unwrap_or_default(), values.is_none()])
}
fn multi_trace(request: &Value) -> Result<Vec<Value>, String> {
    let mut map: Option<MultiMap<String, String>> = None;
    let mut captured: Option<Values<String>> = None;
    let mut rows = vec![];
    for action in api::actions(request) {
        let op = api::action_op(action);
        let key = api::action_str(action, "key");
        let value = api::action_str(action, "value");
        let mut row = json!({"op":op});
        match op {
            "new" => map = Some(MultiMap::default()),
            "new_size_hint" => {
                map = Some(MultiMap::with_capacity(
                    api::action_i64(action, "size").max(0) as usize,
                ));
            }
            "group_by" => {
                if api::action_str(action, "key_of") != "first_byte" {
                    return Err("unknown group key".into());
                }
                map = Some(MultiMap::group_by(strings(action)?, |value| {
                    value.get(..1).unwrap_or("").to_owned()
                }));
            }
            "add" => guarded(&mut row, false, || {
                required(map.as_mut()).add(key.into(), value.into());
                Value::Null
            }),
            "has" => guarded(&mut row, true, || {
                json!(required(map.as_ref()).contains_key(key))
            }),
            "get" => guarded(&mut row, true, || {
                values_result(required(map.as_ref()).get(key).as_deref())
            }),
            "get_capture" => guarded(&mut row, true, || {
                captured = required(map.as_ref()).retain_values(key);
                values_result(captured.as_ref().map(Values::read).as_deref())
            }),
            "get_replay" => guarded(&mut row, true, || {
                values_result(captured.as_ref().map(Values::read).as_deref())
            }),
            "remove" => guarded(&mut row, false, || {
                required(map.as_mut()).remove(key, &value.to_owned());
                Value::Null
            }),
            "remove_all" => guarded(&mut row, false, || {
                required(map.as_mut()).remove_all(key);
                Value::Null
            }),
            "clear" => guarded(&mut row, false, || {
                required(map.as_mut()).clear();
                Value::Null
            }),
            "len" => guarded(&mut row, true, || json!(required(map.as_ref()).len())),
            "keys" => guarded(&mut row, true, || {
                json!(sorted(required(map.as_ref()).keys().cloned()))
            }),
            "values" => guarded(&mut row, true, || {
                let mut groups: Vec<_> = required(map.as_ref())
                    .values()
                    .map(|v| v.to_vec())
                    .collect();
                groups.sort();
                json!(groups)
            }),
            _ => return Err(format!("unknown MultiMap action {op}")),
        }
        rows.push(row);
    }
    Ok(rows)
}
fn entries(map: std::collections::HashMap<String, Option<String>>) -> Value {
    let mut entries: Vec<_> = map.into_iter().collect();
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    json!(entries)
}
fn sync_trace(request: &Value) -> Result<Vec<Value>, String> {
    let mut map: Option<SyncMap<String, Option<String>>> = None;
    let mut clone = None;
    let mut rows = vec![];
    for action in api::actions(request) {
        let op = api::action_op(action);
        let key = api::action_str(action, "key");
        let value = api::action_str(action, "value");
        let mut row = json!({"op":op});
        match op {
            "new" => {
                map = Some(SyncMap::default());
                clone = None;
            }
            "store" | "store_nil" => guarded(&mut row, false, || {
                required(map.as_ref()).store(key.into(), (op == "store").then(|| value.to_owned()));
                Value::Null
            }),
            "load" => guarded(&mut row, true, || {
                let value = required(map.as_ref()).load(key);
                json!([value, value.is_some()])
            }),
            "load_or_store" | "load_or_store_nil" => guarded(&mut row, true, || {
                json!(required(map.as_ref()).load_or_store(
                    key.into(),
                    (op == "load_or_store").then(|| value.to_owned())
                ))
            }),
            "delete" => guarded(&mut row, false, || {
                required(map.as_ref()).delete(key);
                Value::Null
            }),
            "clear" => guarded(&mut row, false, || {
                required(map.as_ref()).clear();
                Value::Null
            }),
            "size" => guarded(&mut row, true, || json!(required(map.as_ref()).len())),
            "keys" => guarded(&mut row, true, || {
                json!(sorted(required(map.as_ref()).keys()))
            }),
            "clone" => guarded(&mut row, false, || {
                clone = Some(required(map.as_ref()).clone());
                Value::Null
            }),
            "clone_size" => guarded(&mut row, true, || json!(required(clone.as_ref()).len())),
            "clone_keys" => guarded(&mut row, true, || {
                json!(sorted(required(clone.as_ref()).keys()))
            }),
            "to_map" => guarded(&mut row, true, || {
                entries(
                    required(map.as_ref())
                        .try_to_map(|v| v.map(Some).ok_or("nil interface conversion"))
                        .unwrap_or_else(|e| panic!("{e}")),
                )
            }),
            "range" => guarded(&mut row, true, || {
                let mut seen = std::collections::HashMap::new();
                required(map.as_ref()).range(|key, value| {
                    seen.insert(key, value);
                    true
                });
                entries(seen)
            }),
            "range_stop" => guarded(&mut row, true, || {
                let mut count = 0;
                required(map.as_ref()).range(|_, _| {
                    count += 1;
                    count < api::action_i64(action, "stop")
                });
                json!(count)
            }),
            _ => return Err(format!("unknown SyncMap action {op}")),
        }
        rows.push(row);
    }
    Ok(rows)
}
fn sync_set_trace(request: &Value) -> Result<Vec<Value>, String> {
    let mut set: Option<SyncSet<String>> = None;
    let mut rows = vec![];
    for action in api::actions(request) {
        let op = api::action_op(action);
        let key = api::action_str(action, "key").to_owned();
        let mut row = json!({"op":op});
        match op {
            "new" => set = Some(SyncSet::default()),
            "add" => guarded(&mut row, false, || {
                required(set.as_ref()).insert(key);
                Value::Null
            }),
            "add_if_absent" => guarded(&mut row, true, || {
                json!(required(set.as_ref()).insert_if_absent(key))
            }),
            "has" => guarded(&mut row, true, || {
                json!(required(set.as_ref()).contains(&key))
            }),
            "delete" => guarded(&mut row, false, || {
                required(set.as_ref()).remove(&key);
                Value::Null
            }),
            "size" => guarded(&mut row, true, || json!(required(set.as_ref()).len())),
            "is_empty" => guarded(&mut row, true, || json!(required(set.as_ref()).is_empty())),
            "keys" => guarded(&mut row, true, || {
                json!(sorted(required(set.as_ref()).keys()))
            }),
            "to_slice" => guarded(&mut row, true, || {
                json!(sorted(required(set.as_ref()).to_vec()))
            }),
            "range_stop" => guarded(&mut row, true, || {
                let mut count = 0;
                required(set.as_ref()).range(|_| {
                    count += 1;
                    count < api::action_i64(action, "stop")
                });
                json!(count)
            }),
            _ => return Err(format!("unknown SyncSet action {op}")),
        }
        rows.push(row);
    }
    Ok(rows)
}
pub(super) fn observe(request: &Value) -> Outcome {
    let replay = match api::subject(request) {
        "Set" => set_trace,
        "MultiMap" => multi_trace,
        "SyncMap" => sync_trace,
        "SyncSet" => sync_set_trace,
        _ => unreachable!(),
    };
    match replay(request) {
        Ok(rows) => Outcome::Observed(api::ordered(rows)),
        Err(error) => Outcome::Failed(error),
    }
}
