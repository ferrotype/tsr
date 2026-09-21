//! Replay scoped actions through the production collections. Nesting the Rust
//! guards represents Go's stack of restoration callbacks; it does not emulate
//! the map or copy-on-write policy.

use crate::api::{self, Outcome};
use serde_json::{json, Value};
use tsr_core::collections::{CopyOnWriteMap, CopyOnWriteSet};

pub(super) fn observe(request: &Value) -> Outcome {
    let actions = api::actions(request);
    if actions.first().map(api::action_op) != Some("new") {
        return Outcome::Failed("scoped collection trace must begin with new".into());
    }
    let mut rows = vec![json!({"op":"new"})];
    let mut index = 1;
    let result = if api::subject(request) == "CopyOnWriteMap" {
        map_actions(
            actions,
            &mut index,
            &mut CopyOnWriteMap::default(),
            0,
            &mut rows,
        )
    } else {
        set_actions(
            actions,
            &mut index,
            &mut CopyOnWriteSet::default(),
            0,
            &mut rows,
        )
    };
    match result {
        Ok(()) => Outcome::Observed(api::ordered(rows)),
        Err(error) => Outcome::Failed(error),
    }
}

fn map_actions(
    actions: &[Value],
    index: &mut usize,
    map: &mut CopyOnWriteMap<String, String>,
    depth: usize,
    rows: &mut Vec<Value>,
) -> Result<(), String> {
    while let Some(action) = actions.get(*index) {
        *index += 1;
        let op = api::action_op(action);
        let key = api::action_str(action, "key");
        let mut row = json!({"op":op, "panic":""});
        match op {
            "get" => {
                let value = map.get(key);
                row["result"] = json!([
                    value.map(String::as_str).unwrap_or_default(),
                    value.is_some()
                ]);
            }
            "has" => row["result"] = json!(map.contains_key(key)),
            "set" => {
                map.insert(key.into(), api::action_str(action, "value").into());
            }
            "enter_scope" => {
                row["depth"] = json!(depth + 1);
                rows.push(row);
                map_actions(actions, index, &mut map.enter_scope(), depth + 1, rows)?;
                continue;
            }
            "exit_scope" if depth > 0 => {
                row["depth"] = json!(depth - 1);
                rows.push(row);
                return Ok(());
            }
            _ => {
                return Err(format!(
                    "unsupported scoped map action {op:?} at depth {depth}"
                ))
            }
        }
        rows.push(row);
    }
    if depth == 0 {
        Ok(())
    } else {
        Err("scoped map trace leaves a scope open".into())
    }
}

fn set_actions(
    actions: &[Value],
    index: &mut usize,
    set: &mut CopyOnWriteSet<String>,
    depth: usize,
    rows: &mut Vec<Value>,
) -> Result<(), String> {
    while let Some(action) = actions.get(*index) {
        *index += 1;
        let op = api::action_op(action);
        let key = api::action_str(action, "key");
        let mut row = json!({"op":op, "panic":""});
        match op {
            "has" => row["result"] = json!(set.contains(key)),
            "add" => {
                set.insert(key.into());
            }
            "enter_scope" => {
                row["depth"] = json!(depth + 1);
                rows.push(row);
                set_actions(actions, index, &mut set.enter_scope(), depth + 1, rows)?;
                continue;
            }
            "exit_scope" if depth > 0 => {
                row["depth"] = json!(depth - 1);
                rows.push(row);
                return Ok(());
            }
            _ => {
                return Err(format!(
                    "unsupported scoped set action {op:?} at depth {depth}"
                ))
            }
        }
        rows.push(row);
    }
    if depth == 0 {
        Ok(())
    } else {
        Err("scoped set trace leaves a scope open".into())
    }
}
