//! Replay frozen relation actions. The default retains the P3 scoped protocol;
//! --deep-runtime observes complete P0 groups for E2 on a 512 KiB worker stack.
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, sync::Arc};
use ts_arena::{CheckerIdentity, Counters, Generation};
use ts_checker::{CheckerOwner, RelationKind};
use ts_compiler::{Program, ProgramCheckerHost};

mod p3;
use p3::{array, load, text, Result};
fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut text = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        write!(&mut text, "{byte:02x}").expect("formatting into String is infallible");
    }
    text
}
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
    deep_runtime: bool,
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
    if deep_runtime {
        let mut display = BTreeMap::new();
        let mut alias_display = BTreeMap::new();
        let mut flags = BTreeMap::new();
        for (name, typ) in &types {
            use ts_checker::type_format_flags as ff;
            display.insert(
                name,
                hex(op
                    .type_to_string(
                        *typ,
                        ff::ALLOW_UNIQUE_ES_SYMBOL_TYPE
                            | ff::USE_ALIAS_DEFINED_OUTSIDE_CURRENT_SCOPE,
                    )?
                    .as_bytes()),
            );
            flags.insert(name, op.type_flags(*typ)?);
            alias_display.insert(
                name,
                hex(op
                    .type_to_string_at(
                        *typ,
                        Some(declarations[name.as_bytes()]),
                        ff::IN_TYPE_ALIAS,
                    )?
                    .as_bytes()),
            );
            // The frozen deep fixtures have no unions. Refuse a future changed
            // fixture instead of silently omitting its ordering observation.
            if op.type_flags(*typ)? & ts_checker::type_flags::UNION != 0 {
                return Err("deep fixture acquired a union ordering obligation".into());
            }
        }
        return Ok(json!({"before_lookup":before_lookup,"actions":observations,
            "display_hex":display,"in_alias_display_hex":alias_display,"type_flags":flags,
            "union_ordering":[],"final":op.relation_state()}));
    }
    Ok(
        json!({"before_lookup":before_lookup,"actions":observations,"state":"observed","display_state":"pending"}),
    )
}
fn observe(request_bytes: &[u8], deep_runtime: bool) -> Result<Value> {
    let request_sha256 = format!("{:x}", Sha256::digest(request_bytes));
    let requests: Value = serde_json::from_slice(request_bytes)?;
    let counters = Counters::new();
    let generation = Generation::new(&counters);
    let mut rows = Vec::new();
    for request in array(&requests)? {
        if deep_runtime
            && !matches!(
                text(&request["id"])?,
                "deep-relation-64"
                    | "deep-relation-256"
                    | "deep-relation-1024"
                    | "deep-relation-4096"
            )
        {
            continue;
        }
        let program = load(request, &counters)?;
        let mut groups = Vec::new();
        let actions = array(&request["actions"])?;
        let mut start = 0;
        while start < actions.len() {
            let mut end = start + 1;
            while end < actions.len() && actions[end]["mode"] == actions[start]["mode"] {
                end += 1;
            }
            let result = group(
                &program,
                &actions[start..end],
                &generation,
                &counters,
                deep_runtime,
            );
            let value = match result {
                Ok(value) => value,
                Err(error) => json!({"state":"pending","reason":error.to_string()}),
            };
            groups.push(if deep_runtime {
                value
            } else {
                json!({"mode":actions[start]["mode"],"observations":value})
            });
            start = end;
        }
        eprintln!(
            "{}: {} observed / {} groups",
            text(&request["id"])?,
            groups
                .iter()
                .filter(|g| if deep_runtime {
                    g["actions"].is_array()
                } else {
                    g["observations"]["state"] == "observed"
                })
                .count(),
            groups.len()
        );
        if deep_runtime {
            let owner = Arc::new(CheckerOwner::for_program(
                CheckerIdentity::new(generation.clone(), &counters),
                &counters,
                Arc::new(ProgramCheckerHost::new(program.clone())),
            )?);
            let mut op = owner.operation()?;
            let source = program
                .file(b"/fixture.ts")
                .ok_or("fixture missing")?
                .source();
            let diagnostics = op
                .semantic_diagnostics(source)?
                .iter()
                .map(|d| diagnostic_payload(&program, d))
                .collect::<Result<Vec<_>>>()?;
            let global = op
                .global_diagnostics()?
                .iter()
                .map(|d| diagnostic_payload(&program, d))
                .collect::<Result<Vec<_>>>()?;
            rows.push(json!({"id":request["id"],"groups":groups,"diagnostics":diagnostics,"global_diagnostics":global,"state":"executed"}));
        } else {
            rows.push(json!({"id":request["id"],"groups":groups}));
        }
    }
    Ok(
        json!({"version":1,"request_sha256":request_sha256,"deep_runtime":deep_runtime,
        "stack_bytes":if deep_runtime { Some(512 * 1024) } else { None },"rows":rows}),
    )
}
fn main() -> Result<()> {
    let args: Vec<_> = std::env::args().collect();
    if args.len() != 3 && !(args.len() == 4 && args[3] == "--deep-runtime") {
        return Err(
            "usage: p3_relations canonical-requests.json observations.json [--deep-runtime]".into(),
        );
    }
    let request_bytes = std::fs::read(&args[1])?;
    let observed = if args.len() == 4 {
        std::thread::Builder::new()
            .stack_size(512 * 1024)
            .spawn(move || observe(&request_bytes, true).map_err(|e| e.to_string()))?
            .join()
            .map_err(|_| "deep-runtime worker panicked")??
    } else {
        observe(&request_bytes, false)?
    };
    std::fs::write(&args[2], serde_json::to_vec(&observed)?)?;
    Ok(())
}
