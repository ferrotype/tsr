//! Replay frozen relation actions. Body/JSDoc dependencies and the full P0
//! display protocol remain explicit; this output is not E2 acceptance evidence.
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, sync::Arc};
use ts_arena::{CheckerIdentity, Counters, Generation};
use ts_checker::{CheckerOwner, RelationKind};
use ts_compiler::{Program, ProgramCheckerHost};

mod p3;
use p3::{array, load, text, Result};
fn diagnostic_payload(program: &Program, d: &ts_ast::Diagnostic) -> Result<Value> {
    let file = if let Some(file) = d.file {
        let file = program
            .files()
            .iter()
            .find(|entry| entry.source() == file)
            .ok_or("diagnostic file missing")?;
        Some(String::from_utf8(
            file.bound().view().source_file()?.file_name().to_vec(),
        )?)
    } else {
        None
    };
    let hex = |bytes: &[u8]| {
        use std::fmt::Write as _;
        let mut text = String::with_capacity(bytes.len() * 2);
        for byte in bytes {
            write!(&mut text, "{byte:02x}").expect("formatting into String is infallible");
        }
        text
    };
    Ok(
        json!({"file":file,"pos":d.loc.pos(),"end":d.loc.end(),"code":d.code,"category":d.category,"key_hex":hex(d.message_key.as_bytes()),"text_hex":hex(d.message_text.as_bytes()),
        "args":if d.message_args.is_empty() { Value::Null } else { json!(d.message_args.iter().map(|v| String::from_utf8(v.as_bytes().to_vec())).collect::<std::result::Result<Vec<_>,_>>()?) },
        "chain":d.message_chain.iter().map(|d|diagnostic_payload(program,d)).collect::<Result<Vec<_>>>()?,"related":d.related_information.iter().map(|d|diagnostic_payload(program,d)).collect::<Result<Vec<_>>>()?}),
    )
}
fn group(
    program: &Arc<Program>,
    actions: &[Value],
    generation: &Generation,
    counters: &Counters,
) -> Result<Value> {
    let owner = Arc::new(CheckerOwner::for_program(
        CheckerIdentity::new(generation.clone(), counters),
        counters,
        Arc::new(ProgramCheckerHost::new(program.clone())),
    )?);
    let mut op = owner.operation()?;
    let file = program.file(b"/fixture.ts").ok_or("fixture missing")?;
    let ast = file.bound().view().ast();
    let statements = ast
        .node(file.source())?
        .statement_list()
        .ok_or("statements missing")?;
    let mut declarations = BTreeMap::new();
    for node in ast
        .node_slice(ast.list(statements)?.nodes())?
        .iter()
        .flatten()
    {
        if let Some(name) = ast.node(node)?.name() {
            declarations.insert(ast.node_text(name)?.as_bytes().to_vec(), node);
        }
    }
    let before_lookup = op.relation_state();
    let mut types = BTreeMap::new();
    for action in actions {
        for key in ["source", "target"] {
            let name = text(&action[key])?;
            if !types.contains_key(name) {
                let node = *declarations
                    .get(name.as_bytes())
                    .ok_or("declaration missing")?;
                let symbol = op
                    .get_symbol_at_location(
                        ast.node(node)?.name().ok_or("declaration name missing")?,
                    )?
                    .ok_or("symbol missing")?;
                types.insert(name.to_owned(), op.get_declared_type_of_symbol(symbol)?);
            }
        }
    }
    let mut observations = Vec::new();
    for action in actions {
        let mode = match text(&action["mode"])? {
            "identity" => RelationKind::Identity,
            "assignable" => RelationKind::Assignable,
            "subtype" => RelationKind::Subtype,
            "strict_subtype" => RelationKind::StrictSubtype,
            "comparable" => RelationKind::Comparable,
            _ => return Err("unknown relation".into()),
        };
        let before = op.relation_state();
        let node = if action["report_errors"] == true {
            Some(
                *declarations
                    .get(text(&action["source"])?.as_bytes())
                    .ok_or("declaration missing")?,
            )
        } else {
            None
        };
        let (result, calls, diagnostic) = op.observe_type_relation(
            types[text(&action["source"])?],
            types[text(&action["target"])?],
            mode,
            node,
        )?;
        let diagnostics = diagnostic
            .iter()
            .map(|d| diagnostic_payload(program, d))
            .collect::<Result<Vec<_>>>()?;
        observations.push(json!({"action":action,"result":result,"ternary_calls":calls,"before":before,"after":op.relation_state(),"diagnostics":diagnostics}));
    }
    Ok(
        json!({"before_lookup":before_lookup,"actions":observations,"state":"observed","display_state":"pending"}),
    )
}
fn main() -> Result<()> {
    let args: Vec<_> = std::env::args().collect();
    if args.len() != 3 {
        return Err("usage: p3_relations canonical-requests.json observations.json".into());
    }
    let request_bytes = std::fs::read(&args[1])?;
    let request_sha256 = format!("{:x}", Sha256::digest(&request_bytes));
    let requests: Value = serde_json::from_slice(&request_bytes)?;
    let counters = Counters::new();
    let generation = Generation::new(&counters);
    let mut rows = Vec::new();
    for request in array(&requests)? {
        let program = load(request, &counters)?;
        let mut groups = Vec::new();
        let actions = array(&request["actions"])?;
        let mut start = 0;
        while start < actions.len() {
            let mut end = start + 1;
            while end < actions.len() && actions[end]["mode"] == actions[start]["mode"] {
                end += 1;
            }
            let result = group(&program, &actions[start..end], &generation, &counters);
            let value = match result {
                Ok(value) => value,
                Err(error) => json!({"state":"pending","reason":error.to_string()}),
            };
            groups.push(json!({"mode":actions[start]["mode"],"observations":value}));
            start = end;
        }
        eprintln!(
            "{}: {} observed / {} groups",
            text(&request["id"])?,
            groups
                .iter()
                .filter(|g| g["observations"]["state"] == "observed")
                .count(),
            groups.len()
        );
        rows.push(json!({"id":request["id"],"groups":groups}));
    }
    std::fs::write(
        &args[2],
        serde_json::to_vec(
            &json!({"version":1,"request_sha256":request_sha256,"scope":"P4 relation actions including body/JSDoc; full P0 display/final-state contract pending","rows":rows}),
        )?,
    )?;
    Ok(())
}
