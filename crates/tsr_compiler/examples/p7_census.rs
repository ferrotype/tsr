//! S08 P7 census fixtures (`tools/s08/p7/census-fixtures.json`): load each
//! small program, check it, retain the named declared types as roots and
//! report the structural census (`data/s08/type-footprint.json`).
//! `p7_census requests.json output.json` with the P3 request layout.
use serde_json::{json, Value};
use std::sync::Arc;
use tsr_arena::{CheckerIdentity, Counters, Generation};
use tsr_checker::CheckerOwner;
use tsr_compiler::ProgramCheckerHost;

mod p3;
use p3::{array, load, text, Result};

fn main() -> Result<()> {
    let args: Vec<_> = std::env::args().collect();
    if args.len() != 3 {
        return Err("usage: p7_census requests.json output.json".into());
    }
    let requests: Value = serde_json::from_slice(&std::fs::read(&args[1])?)?;
    let counters = Counters::new();
    let generation = Generation::new(&counters);
    let mut rows = Vec::new();
    for request in array(&requests)? {
        let program = load(request, &counters)?;
        let owner = Arc::new(CheckerOwner::for_program(
            CheckerIdentity::new(generation.clone(), &counters),
            &counters,
            Arc::new(ProgramCheckerHost::new(program.clone())),
        )?);
        let mut op = owner.operation()?;
        let file = program.file(b"/fixture.ts").ok_or("fixture missing")?;
        let source = file.source();
        let diagnostics = op.semantic_diagnostics(source)?.len() + op.global_diagnostics()?.len();
        let ast = file.bound().view().ast();
        let statements = ast
            .node(source)?
            .statement_list()
            .ok_or("statements missing")?;
        let mut roots = Vec::new();
        for name in array(&request["roots"])? {
            let wanted = text(name)?.as_bytes();
            let mut found = None;
            for node in ast
                .node_slice(ast.list(statements)?.nodes())?
                .iter()
                .flatten()
            {
                if let Some(name) = ast.node(node)?.name() {
                    if ast.node_text(name)?.as_bytes() == wanted {
                        found = Some(name);
                    }
                }
            }
            let name = found.ok_or("root declaration missing")?;
            let symbol = op
                .get_symbol_at_location(name)?
                .ok_or("root symbol missing")?;
            roots.push(op.get_declared_type_of_symbol(symbol)?);
        }
        let census = op.census(&roots)?;
        rows.push(
            json!({"id": request["id"], "diagnostics": diagnostics, "roots": roots.len(),
            "types_created": op.type_count(), "symbols_created": op.symbol_count(),
            "signatures_created": op.signature_count(), "census": census}),
        );
    }
    std::fs::write(
        &args[2],
        serde_json::to_vec(&json!({"version": 1, "rows": rows}))?,
    )?;
    Ok(())
}
