use crate::api::{self, action_str as text};
use serde_json::{json, Value};
use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    rc::Rc,
};
use tsr_core::{helpers as h, slices::SharedSlice};

type Slice = SharedSlice<String>;
fn items(action: &Value) -> Result<Vec<String>, String> {
    action["items"].as_array().map_or(Ok(vec![]), |items| {
        items
            .iter()
            .map(|v| {
                v.as_str()
                    .map(str::to_owned)
                    .ok_or_else(|| "items must be strings".into())
            })
            .collect()
    })
}
fn flag(action: &Value, key: &str) -> bool {
    action[key].as_bool().unwrap_or(false)
}
fn slice_result(row: &mut Value, slice: &Slice) {
    row["values"] = json!(&*slice.read());
    row["nil"] = json!(slice.is_nil());
    row["len"] = json!(slice.len());
}
fn predicate(name: &str, arg: &str, value: &str) -> bool {
    match name {
        "always" => true,
        "never" => false,
        "nonempty" => !value.is_empty(),
        "empty" => value.is_empty(),
        "len_gt_1" => value.len() > 1,
        "equals_value" => value == arg,
        _ => unreachable!(),
    }
}
fn mapper(name: &str, value: &str) -> String {
    match name {
        "upper" => value.to_uppercase(),
        "upper_if_a" if value == "a" => "A".into(),
        "upper_if_c" if value == "c" => "C".into(),
        "identity" | "upper_if_a" | "upper_if_c" => value.into(),
        _ => unreachable!(),
    }
}
fn slices(request: &Value) -> Result<Vec<Value>, String> {
    let mut regs: HashMap<String, Slice> = HashMap::new();
    let mut rows = vec![];
    for a in api::actions(request) {
        let op = api::action_op(a);
        let target = text(a, "target");
        let other = text(a, "other");
        let value = text(a, "value");
        let rule = text(a, "rule");
        let pred = text(a, "predicate");
        let index = api::action_i64(a, "index");
        let mut row = json!({"op":op});
        if !pred.is_empty()
            && ![
                "always",
                "never",
                "nonempty",
                "empty",
                "len_gt_1",
                "equals_value",
            ]
            .contains(&pred)
        {
            return Err("unknown predicate".into());
        }
        if !rule.is_empty()
            && ![
                "identity",
                "upper",
                "upper_if_a",
                "upper_if_c",
                "with_index",
                "identity_index",
                "keep_nonempty",
                "keep_all",
                "drop_all",
                "split_chars",
                "single",
                "empty",
            ]
            .contains(&rule)
        {
            return Err("unknown mapper".into());
        }
        let get = |name: &str| {
            regs.get(name)
                .cloned()
                .ok_or_else(|| format!("unknown register {name}"))
        };
        if matches!(rule, "upper" | "split_chars") && get(target)?.iter().any(|v| !v.is_ascii()) {
            return Err("this fixture callback requires ASCII; add a byte/rune-aware callback before extending its domain".into());
        }
        let mut calls = 0;
        match op {
            "set" => {
                let s = if flag(a, "nil") {
                    Slice::default()
                } else {
                    Slice::from_vec(items(a)?)
                };
                row["target"] = json!(target);
                slice_result(&mut row, &s);
                regs.insert(target.into(), s);
            }
            "alias" | "slice_from" | "slice_to" => {
                let s = get(other)?;
                let s = if op == "alias" {
                    s
                } else {
                    let i = usize::try_from(index).map_err(|_| "negative slice index")?;
                    if i > s.len() {
                        return Err("slice index out of range".into());
                    }
                    row["index"] = json!(index);
                    s.slice(if op == "slice_from" { i..s.len() } else { 0..i })
                };
                row["target"] = json!(target);
                row["other"] = json!(other);
                slice_result(&mut row, &s);
                regs.insert(target.into(), s);
            }
            "read" => {
                row["target"] = json!(target);
                slice_result(&mut row, &get(target)?);
            }
            "mutate" => {
                let s = regs.get_mut(target).ok_or("unknown target")?;
                let i = usize::try_from(index).map_err(|_| "negative index")?;
                if i >= s.len() {
                    return Err("mutate index out of range".into());
                }
                s.set(i, value.into());
                row["target"] = json!(target);
                row["index"] = json!(index);
                row["value"] = json!(value);
            }
            "same" => {
                row["target"] = json!(target);
                row["other"] = json!(other);
                row["same"] = json!(get(target)?.same(&get(other)?));
            }
            "some" | "find" | "find_last" | "find_index" => {
                let s = get(target)?;
                let values = s.read();
                let p = |v: &String| {
                    calls += 1;
                    predicate(pred, value, v)
                };
                if op == "find_index" {
                    row["index"] = json!(h::find_index(&values, p));
                } else {
                    row["result"] = match op {
                        "some" => json!(h::some(&values, p)),
                        "find" => json!(h::find(&values, p)),
                        _ => json!(h::find_last(&values, p)),
                    };
                }
                row["predicate"] = json!(pred);
                row["calls"] = json!(calls);
            }
            "first_or_nil" | "last_or_nil" => {
                let s = get(target)?;
                row["target"] = json!(target);
                row["result"] = json!(if op == "first_or_nil" {
                    h::first_or_zero(&s.read())
                } else {
                    h::last_or_zero(&s.read())
                });
            }
            _ => {
                let result = match op {
                    "filter" => {
                        row["predicate"] = json!(pred);
                        let s = get(target)?;
                        h::filter(&s, |v| {
                            calls += 1;
                            predicate(pred, value, v)
                        })
                    }
                    "deduplicate" => h::deduplicate(&get(target)?),
                    "append_if_unique" => {
                        row["value"] = json!(value);
                        h::append_if_unique(&get(target)?, value.into())
                    }
                    "concatenate" => {
                        row["target"] = json!(target);
                        row["other"] = json!(other);
                        h::concatenate(&get(target)?, &get(other)?)
                    }
                    "map" | "same_map" => {
                        row["rule"] = json!(rule);
                        let f = |v: &String| {
                            calls += 1;
                            mapper(rule, v)
                        };
                        let s = get(target)?;
                        if op == "map" {
                            h::map(&s, f)
                        } else {
                            h::same_map(&s, f)
                        }
                    }
                    "map_index" => {
                        row["rule"] = json!(rule);
                        h::map_index(&get(target)?, |v, i| {
                            calls += 1;
                            if rule == "with_index" {
                                format!("{v}:{i}")
                            } else {
                                v.clone()
                            }
                        })
                    }
                    "map_filtered" => {
                        row["rule"] = json!(rule);
                        h::map_filtered(&get(target)?, |v| {
                            calls += 1;
                            (rule == "keep_all" || rule == "keep_nonempty" && !v.is_empty())
                                .then(|| v.clone())
                        })
                    }
                    "flat_map" => {
                        row["rule"] = json!(rule);
                        h::flat_map(&get(target)?, |v| {
                            calls += 1;
                            match rule {
                                "empty" => vec![],
                                "single" => vec![v.clone()],
                                "split_chars" => v
                                    .bytes()
                                    .map(|b| {
                                        String::from_utf8(vec![b]).expect("ASCII split fixture")
                                    })
                                    .collect(),
                                _ => unreachable!(),
                            }
                        })
                    }
                    "flatten_register" => {
                        row["target"] = json!(target);
                        h::flatten(&[get(target)?])
                    }
                    "flatten" => {
                        let groups = a["groups"].as_array().ok_or("missing groups")?;
                        row["groups"] = json!(groups.len());
                        let groups = groups
                            .iter()
                            .map(|g| {
                                if g.is_null() {
                                    Ok(Slice::default())
                                } else {
                                    let mut action = json!({});
                                    action["items"] = g.clone();
                                    items(&action).map(Slice::from_vec)
                                }
                            })
                            .collect::<Result<Vec<_>, String>>()?;
                        h::flatten(&groups)
                    }
                    _ => return Err(format!("unknown slice operation {op}")),
                };
                if matches!(
                    op,
                    "filter" | "map" | "map_index" | "map_filtered" | "flat_map" | "same_map"
                ) {
                    row["calls"] = json!(calls);
                }
                slice_result(&mut row, &result);
                regs.insert("result".into(), result);
            }
        }
        rows.push(row);
    }
    Ok(rows)
}
#[derive(Debug)]
struct SuppliedError(String);
fn guard(row: &mut Value, f: impl FnOnce() -> Value) {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)) {
        Ok(v) => {
            row["result"] = v;
            row["panic"] = json!("");
            row["payload"] = json!("");
            row["message"] = json!("");
        }
        Err(e) => {
            row["result"] = Value::Null;
            if let Some(e) = e.downcast_ref::<SuppliedError>() {
                row["panic"] = json!("error_value");
                row["payload"] = json!("error");
                row["message"] = json!(e.0);
            } else {
                let text = e
                    .downcast_ref::<&str>()
                    .copied()
                    .or_else(|| e.downcast_ref::<String>().map(String::as_str))
                    .unwrap_or("");
                row["panic"] = json!(if text == "phase1: memoized create failed" {
                    "create_panicked"
                } else {
                    "unclassified_runtime_panic"
                });
                row["payload"] = json!("string");
                row["message"] = json!("");
            }
        }
    }
}
fn scalar(request: &Value) -> Result<Vec<Value>, String> {
    let mut memoized: Option<Box<dyn FnMut() -> String>> = None;
    let calls = Rc::new(Cell::new(0));
    let mut box_value: Option<Rc<RefCell<String>>> = None;
    let mut single: Option<Vec<Rc<RefCell<String>>>> = None;
    let mut boxes: HashMap<String, Option<Rc<String>>> = HashMap::new();
    let mut seqs: Vec<Option<Vec<String>>> = vec![];
    let mut rows = vec![];
    for a in api::actions(request) {
        let op = api::action_op(a);
        let value = text(a, "value");
        let target = text(a, "target");
        let other = text(a, "other");
        let fallback = text(a, "fallback");
        let index = api::action_i64(a, "index");
        let mut row = json!({"op":op});
        match (api::subject(request), op) {
            ("helpers.SingleElementSlice", "new_box") => {
                box_value = Some(Rc::new(RefCell::new(value.into())));
                row["value"] = json!(value);
            }
            ("helpers.SingleElementSlice", "clear_box") => box_value = None,
            (_, "single") => {
                single = h::single_element_slice(box_value.clone());
                row["values"] = json!(single
                    .iter()
                    .flatten()
                    .map(|p| p.borrow().clone())
                    .collect::<Vec<_>>());
                row["nil"] = json!(single.is_none());
                row["len"] = json!(single.as_ref().map_or(0, Vec::len));
            }
            (_, "mutate_element") => {
                let p = single
                    .as_ref()
                    .and_then(|s| s.get(index as usize))
                    .ok_or("invalid element index")?;
                *p.borrow_mut() = value.into();
                row["index"] = json!(index);
                row["value"] = json!(value);
            }
            (_, "read_box") => row["box"] = json!(box_value.as_ref().map(|p| p.borrow().clone())),
            (_, "memoize" | "memoize_panicking") => {
                calls.set(0);
                let calls = calls.clone();
                let value = value.to_owned();
                let should_panic = op == "memoize_panicking";
                let mut failed = false;
                memoized = Some(Box::new(h::memoize(move || {
                    calls.set(calls.get() + 1);
                    if should_panic && !failed {
                        failed = true;
                        panic!("phase1: memoized create failed");
                    }
                    value.clone()
                })));
                row["value"] = json!(text(a, "value"));
            }
            (_, "call") => {
                let f = memoized.as_mut().ok_or("call before memoize")?;
                guard(&mut row, || json!(f()));
                row["calls"] = json!(calls.get());
            }
            (_, "must") => {
                let failed = flag(a, "flag");
                row["value"] = json!(value);
                row["fails"] = json!(failed);
                row["message_in"] = json!(text(a, "message"));
                guard(&mut row, || {
                    json!(h::must(if failed {
                        Err(SuppliedError(text(a, "message").into()))
                    } else {
                        Ok(value)
                    }))
                });
            }
            (_, "or_else" | "if_else") => {
                row["value"] = json!(value);
                row["fallback"] = json!(fallback);
                row["result"] = json!(if op == "or_else" {
                    h::or_else(value, fallback)
                } else {
                    row["flag"] = json!(flag(a, "flag"));
                    h::if_else(flag(a, "flag"), value, fallback)
                });
            }
            (_, "reset_seqs") => seqs.clear(),
            (_, "push_seq") => {
                let items = items(a)?;
                row["items"] = json!(items);
                seqs.push(Some(items));
            }
            (_, "push_nil_seq") => seqs.push(None),
            (_, "collect") => {
                let values: Vec<_> =
                    h::concatenate_seq(seqs.iter().map(|s| s.as_ref().map(|s| s.iter())))
                        .take(if index > 0 {
                            index as usize
                        } else {
                            usize::MAX
                        })
                        .collect();
                row["values"] = json!(values);
                row["visited"] = json!(values.len());
                row["stopped"] = json!(index > 0 && values.len() >= index as usize);
                row["limit"] = json!(index);
            }
            (_, "new_box" | "new_nil_box") => {
                boxes.insert(
                    target.into(),
                    (op == "new_box").then(|| Rc::new(value.into())),
                );
                row["target"] = json!(target);
                if op == "new_box" {
                    row["value"] = json!(value);
                }
            }
            (_, "strings") => {
                row["value"] = json!(value);
                row["other"] = json!(fallback);
                row["equal"] = json!(h::comparable_values_equal(&value, &fallback));
            }
            (_, "pointers" | "pairs") => {
                let a = boxes
                    .get(target)
                    .ok_or("unknown first box")?
                    .as_ref()
                    .map(Rc::as_ptr);
                let b = boxes
                    .get(other)
                    .ok_or("unknown second box")?
                    .as_ref()
                    .map(Rc::as_ptr);
                row["target"] = json!(target);
                row["other"] = json!(other);
                row["equal"] = json!(if op == "pairs" {
                    row["value"] = json!(value);
                    h::comparable_values_equal(&(value, a), &(value, b))
                } else {
                    h::comparable_values_equal(&a, &b)
                });
            }
            _ => return Err(format!("unknown helper operation {op}")),
        }
        rows.push(row);
    }
    Ok(rows)
}
pub(super) fn replay(request: &Value) -> Result<Vec<Value>, String> {
    if api::subject(request) == "helpers.Slices" {
        slices(request)
    } else {
        scalar(request)
    }
}
