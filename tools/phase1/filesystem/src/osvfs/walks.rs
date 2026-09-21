use super::helpers::{array, error, int, mode, text, Env};
use serde_json::{json, Value};
use std::{cell::RefCell, collections::BTreeMap, rc::Rc};
use tsr_vfs::{
    iofs::IoError,
    os::{self, LimitedWalk},
    Error, FileSystem, WalkCallback, WalkControl, WalkEntry,
};
type Counts = Rc<RefCell<BTreeMap<String, usize>>>;
#[derive(Default)]
pub struct State {
    counts: Counts,
    held: Option<LimitedWalk<Box<WalkCallback<'static>>>>,
}
fn decide(
    rule: &str,
    d: Option<&WalkEntry>,
    has_error: bool,
) -> (&'static str, Result<WalkControl, Error>) {
    let (v, arg) = rule.split_once(':').unwrap_or((rule, ""));
    let name = d.map_or(b"".as_slice(), |d| d.name.as_bytes());
    match v {
        "skipdir_on" if name == arg.as_bytes() => ("skipdir", Ok(WalkControl::SkipDir)),
        "skipall_on" if name == arg.as_bytes() => ("skipall", Ok(WalkControl::SkipAll)),
        "error_on" if name == arg.as_bytes() => {
            ("error", Err(Error::Unsupported("probe callback refusal")))
        }
        "skipdir_on_error" if has_error => ("skipdir", Ok(WalkControl::SkipDir)),
        "error_on_error" if has_error => {
            ("error", Err(Error::Unsupported("probe callback refusal")))
        }
        _ => ("nil", Ok(WalkControl::Continue)),
    }
}
fn walk_row(e: &Env, p: &[u8], d: Option<&WalkEntry>, err: Option<Error>, decision: &str) -> Value {
    let name = d.map_or("<nil>".into(), |d| {
        String::from_utf8_lossy(d.name.as_bytes()).into_owned()
    });
    let kind = d.map_or(json!("<nil>"), |d| mode(d.info.mode)[1].clone());
    json!([
        e.place(p),
        name,
        d.is_some_and(|d| d.info.directory),
        kind,
        d.is_some_and(|d| p.rsplit(|b| *b == b'/').next() == Some(d.name.as_bytes())),
        error(err),
        decision
    ])
}
fn descend(p: &[u8], level: i64, depth: i64, rows: &mut Vec<Value>) {
    let mut visited = 0;
    let err = os::fs()
        .walk_dir(p, &mut |_, d, _| {
            visited += 1;
            if level < depth && visited == 1 && d.is_some_and(|d| d.info.directory) {
                descend(p, level + 1, depth, rows);
            }
            Ok(WalkControl::Continue)
        })
        .err();
    rows.push(json!([level, visited, error(err)]));
}
pub fn action(e: &Env, a: &Value, state: &mut State) -> Result<Value, String> {
    let op = text(a, "op")?;
    let fs = os::fs();
    Ok(match op {
        "walk" | "walk_literal" => {
            let p = if op == "walk" {
                e.path(text(a, "path")?)
            } else {
                text(a, "literal")?.as_bytes().to_vec()
            };
            let rule = text(a, "decision")?;
            if !matches!(
                rule.split(':').next().unwrap(),
                "nil"
                    | "skipdir_on"
                    | "skipall_on"
                    | "error_on"
                    | "skipdir_on_error"
                    | "error_on_error"
            ) {
                return Err("unknown walk rule".into());
            }
            let mut rows = vec![];
            let err = fs
                .walk_dir(&p, &mut |p, d, err| {
                    let (label, decision) = decide(rule, d, err.is_some());
                    rows.push(walk_row(e, p, d, err, label));
                    decision
                })
                .err();
            json!([error(err), rows.len(), rows])
        }
        "walk_counted" => {
            let p = e.path(text(a, "path")?);
            let label = text(a, "callback")?;
            let others = array(a, "others")?
                .iter()
                .map(|v| v.as_str().ok_or("invalid callback name"))
                .collect::<Result<Vec<_>, _>>()?;
            let before = state.counts.borrow().clone();
            let mut rows = vec![];
            let err = fs
                .walk_dir(&p, &mut |p, d, _| {
                    *state.counts.borrow_mut().entry(label.into()).or_default() += 1;
                    rows.push(json!([e.place(p), d.is_some()]));
                    Ok(WalkControl::Continue)
                })
                .err();
            let counts = state.counts.borrow();
            let untouched = others
                .iter()
                .map(|name| {
                    json!([
                        name,
                        counts.get(*name).copied().unwrap_or(0)
                            == before.get(*name).copied().unwrap_or(0)
                    ])
                })
                .collect::<Vec<_>>();
            json!([
                error(err),
                counts.get(label).copied().unwrap_or(0),
                rows.iter()
                    .all(|r| r[0].as_str().unwrap().starts_with("<root>")),
                untouched,
                rows
            ])
        }
        "walk_nested" => {
            let p = e.path(text(a, "path")?);
            let inner = text(a, "nested")?;
            let at = text(a, "at")?;
            let mut outer_rows = vec![];
            let mut inner_rows = vec![];
            let mut started = 0;
            let err = fs
                .walk_dir(&p, &mut |p, d, _| {
                    outer_rows.push(e.place(p));
                    if d.is_some_and(|d| d.name.as_bytes() == at.as_bytes()) {
                        started += 1;
                        let _ = fs.walk_dir(&e.path(inner), &mut |p, _, _| {
                            inner_rows.push(e.place(p));
                            Ok(WalkControl::Continue)
                        });
                    }
                    Ok(WalkControl::Continue)
                })
                .err();
            json!([
                error(err),
                started,
                outer_rows.len(),
                inner_rows.len(),
                inner_rows
                    .iter()
                    .all(|r| r.starts_with(&format!("<root>/{inner}"))),
                outer_rows,
                inner_rows
            ])
        }
        "walk_reentrant" => {
            let depth = int(a, "depth")?;
            if !(1..=16).contains(&depth) {
                return Err("unsafe recursion depth".into());
            }
            let mut rows = vec![];
            descend(&e.path(text(a, "path")?), 1, depth, &mut rows);
            json!([depth, rows.len(), rows])
        }
        "acquire_wrapper" => {
            let label = text(a, "callback")?.to_owned();
            let counts = state.counts.clone();
            let cb: Box<WalkCallback<'static>> = Box::new(move |p, d, err| {
                *counts.borrow_mut().entry(label.clone()).or_default() += 1;
                Err(IoError::message(format!(
                    "phase1: {label} saw {} d==nil={} err={}",
                    tsr_vfs::iofs::go_quote(p),
                    d.is_none(),
                    err.is_some()
                ))
                .into())
            });
            state.held = Some(LimitedWalk::new(cb));
            let bound = state.held.as_ref().unwrap().is_bound();
            json!([state.held.is_some(), bound, true])
        }
        "invoke_wrapper" => {
            let label = text(a, "callback")?;
            let p = text(a, "literal")?.as_bytes();
            let err = (text(a, "error")? == "yes")
                .then(|| IoError::message("phase1: probe supplied walk error").into());
            if let Some(w) = &mut state.held {
                let before = state.counts.borrow().get(label).copied().unwrap_or(0);
                let returned = w.visit(p, None, err);
                let after = state.counts.borrow().get(label).copied().unwrap_or(0);
                json!([
                    "ok",
                    after - before,
                    returned.err().map_or(String::new(), |e| e.to_string())
                ])
            } else {
                json!(["no_wrapper"])
            }
        }
        "release_wrapper" => {
            if let Some(mut w) = state.held.take() {
                let before = w.is_bound();
                w.clear();
                json!(["ok", before, true, w.is_bound(), true])
            } else {
                json!(["no_wrapper"])
            }
        }
        "counts" => {
            let counts = state.counts.borrow();
            let names = array(a, "callbacks")?;
            json!(names
                .iter()
                .map(|n| {
                    let s = n.as_str().ok_or("invalid callback name")?;
                    Ok(json!([s, counts.get(s).copied().unwrap_or(0)]))
                })
                .collect::<Result<Vec<_>, String>>()?)
        }
        _ => return Err(format!("unknown walk action {op}")),
    })
}
