use super::{diagnostics, graph, load, owner, panic_error, queries, text, view, Error, Result};
use serde_json::{json, Value};
use std::sync::{Arc, Barrier};
use ts_arena::{Counters, Generation};
use ts_checker::{CheckerOwner, SymbolRef, TypeRef};
use ts_compiler::{FileCache, Program};
struct Observed {
    row: Value,
    symbol: SymbolRef,
    typ: TypeRef,
}
fn merged(
    program: &Program,
    owner: &Arc<CheckerOwner>,
    request: &Value,
    label: &str,
    path: &str,
) -> Result<Observed> {
    let decl = queries::declaration(program, path, text(&request["symbol"])?)?;
    let node = view(program, decl)?
        .node(decl)?
        .name()
        .ok_or_else(|| Error::Protocol("merge declaration lacks name".into()))?;
    let mut op = owner.operation()?;
    let symbol = op
        .get_symbol_at_location(node)?
        .ok_or_else(|| Error::Protocol("merged symbol missing".into()))?;
    let typ = op.get_declared_type_of_symbol(symbol)?;
    let mut graph = graph::Graph::new(program, label, None)?;
    let value = graph.snapshot(program, Some(&op), Some(symbol.id()))?;
    let base = graph::bound_global(
        program,
        text(&request["shared"])?,
        text(&request["symbol"])?,
    )?;
    Ok(Observed {
        row: json!({"checker":label,"via":path,"symbol":value,"type":queries::type_value(program,&mut op,typ,&mut graph)?,"merged_is_bound_base":symbol.id()==base}),
        symbol,
        typ,
    })
}
fn diagnostic_row(program: &Program, owner: &Arc<CheckerOwner>) -> Result<Value> {
    diagnostics::all(program, &mut owner.operation()?)
}
fn file_identity(a: &Program, b: &Program, path: &str) -> Result<bool> {
    let a = a
        .file(path.as_bytes())
        .ok_or_else(|| Error::Protocol("shared source absent".into()))?;
    let b = b
        .file(path.as_bytes())
        .ok_or_else(|| Error::Protocol("shared source absent".into()))?;
    Ok(std::ptr::eq(a.bound(), b.bound()))
}
fn repeated(mut value: Observed, previous: &Observed) -> Observed {
    value.row["same_symbol_as_previous"] = json!(value.symbol == previous.symbol);
    value.row["same_type_as_previous"] = json!(value.typ == previous.typ);
    value
}
pub fn mode(
    request: &Value,
    mode: &str,
    generation: &Generation,
    counters: &Counters,
) -> Result<Value> {
    let mut cache = FileCache::new();
    let a_program = load(
        &request["files"],
        &request["independent_roots"]["A"],
        &mut cache,
        counters,
    )?;
    let shared = text(&request["shared"])?;
    let base = a_program
        .file(shared.as_bytes())
        .ok_or_else(|| Error::Protocol("shared source absent".into()))?
        .source();
    let before = graph::bound_snapshot(&a_program, base)?;
    let mut output = json!({"mode":mode,"state":"executed","base_before":before});
    let mut observations = Vec::new();
    let after;
    if mode == "single-checker" || mode == "repeated" {
        let program = load(
            &request["files"],
            &request["single_roots"],
            &mut cache,
            counters,
        )?;
        let owner = owner(program.clone(), generation, counters)?;
        output["shared_bound_file_identity"] = json!(file_identity(&a_program, &program, shared)?);
        let paths = if mode == "single-checker" {
            vec![shared]
        } else {
            vec![shared, "/a.ts", "/b.ts", "/b.ts", "/a.ts", shared]
        };
        let mut previous: Option<Observed> = None;
        for path in paths {
            let mut value = merged(&program, &owner, request, "single", path)?;
            value.row["same_symbol_as_previous"] =
                json!(previous.as_ref().is_some_and(|p| p.symbol == value.symbol));
            value.row["same_type_as_previous"] =
                json!(previous.as_ref().is_some_and(|p| p.typ == value.typ));
            observations.push(value.row.clone());
            previous = Some(value);
        }
        output["diagnostics"] = json!({"single":diagnostic_row(&program,&owner)?});
        after = graph::bound_snapshot(&a_program, base)?;
    } else {
        let b_program = load(
            &request["files"],
            &request["independent_roots"]["B"],
            &mut cache,
            counters,
        )?;
        output["shared_bound_file_identity"] =
            json!(file_identity(&a_program, &b_program, shared)?);
        let (a_owner, a_value, b_owner, b_value) = if mode == "concurrent" {
            let barrier = Barrier::new(3);
            std::thread::scope(|scope| -> Result<_> {
                let a = scope.spawn(|| -> Result<_> {
                    barrier.wait();
                    let owner = owner(a_program.clone(), generation, counters)?;
                    let value = merged(&a_program, &owner, request, "A", shared)?;
                    Ok((owner, value))
                });
                let b = scope.spawn(|| -> Result<_> {
                    barrier.wait();
                    let owner = owner(b_program.clone(), generation, counters)?;
                    let value = merged(&b_program, &owner, request, "B", shared)?;
                    Ok((owner, value))
                });
                barrier.wait();
                let (a_owner, a_value) = a
                    .join()
                    .map_err(|payload| panic_error(payload.as_ref()))??;
                let (b_owner, b_value) = b
                    .join()
                    .map_err(|payload| panic_error(payload.as_ref()))??;
                Ok((a_owner, a_value, b_owner, b_value))
            })?
        } else if mode == "separate-checker" {
            let ao = owner(a_program.clone(), generation, counters)?;
            let bo = owner(b_program.clone(), generation, counters)?;
            let av = merged(&a_program, &ao, request, "A", shared)?;
            let bv = merged(&b_program, &bo, request, "B", shared)?;
            (ao, av, bo, bv)
        } else {
            return Err(Error::Protocol("unknown merge mode".into()));
        };
        observations.push(a_value.row.clone());
        observations.push(b_value.row.clone());
        output["different_merged_symbols"] = json!(a_value.symbol.id() != b_value.symbol.id());
        output["different_types"] = json!(a_value.typ != b_value.typ);
        observations.push(
            repeated(
                merged(&b_program, &b_owner, request, "B", shared)?,
                &b_value,
            )
            .row,
        );
        observations.push(
            repeated(
                merged(&a_program, &a_owner, request, "A", shared)?,
                &a_value,
            )
            .row,
        );
        output["diagnostics"] = json!({"A":diagnostic_row(&a_program,&a_owner)?,"B":diagnostic_row(&b_program,&b_owner)?});
        let weak_a = Arc::downgrade(&a_owner);
        let weak_program_a = Arc::downgrade(&a_program);
        drop(a_value);
        drop(a_owner);
        drop(a_program);
        let mut surviving = repeated(
            merged(&b_program, &b_owner, request, "B", shared)?,
            &b_value,
        );
        surviving.row["after_release_A"] =
            json!(weak_a.upgrade().is_none() && weak_program_a.upgrade().is_none());
        observations.push(surviving.row);
        after = graph::bound_snapshot(&b_program, base)?;
    }
    output["base_unchanged"] = json!(output["base_before"] == after);
    output["base_after"] = after;
    output["observations"] = json!(observations);
    Ok(output)
}
