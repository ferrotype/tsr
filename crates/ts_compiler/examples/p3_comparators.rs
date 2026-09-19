//! Executes the frozen residual comparator constructions and the exact native
//! permutations through production Ordering. Expected results stay outside.
mod p3;
use p3::{array, load, text, Result};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, sync::Arc};
use ts_arena::{CheckerIdentity, Counters, Generation};
use ts_checker::CheckerOwner;
use ts_compiler::ProgramCheckerHost;

fn main() -> Result<()> {
    let args: Vec<_> = std::env::args().collect();
    if args.len() != 4 {
        return Err(
            "usage: p3_comparators canonical-requests.json order-inputs.json output.json".into(),
        );
    }
    let request_bytes = std::fs::read(&args[1])?;
    let request_sha256 = format!("{:x}", Sha256::digest(&request_bytes));
    let requests: Value = serde_json::from_slice(&request_bytes)?;
    let inputs: Value = serde_json::from_slice(&std::fs::read(&args[2])?)?;
    if inputs["version"] != 1 || inputs["request_sha256"] != request_sha256 {
        return Err("ordering inputs name a different request capture".into());
    }
    let input_ids: std::collections::BTreeSet<_> = inputs["cases"]
        .as_object()
        .ok_or("cases must be an object")?
        .keys()
        .map(String::as_str)
        .collect();
    let request_ids: std::collections::BTreeSet<_> = array(&requests)?
        .iter()
        .map(|row| text(&row["id"]))
        .collect::<Result<_>>()?;
    if input_ids != request_ids {
        return Err("ordering case inventory mismatch".into());
    }
    let counters = Counters::new();
    let generation = Generation::new(&counters);
    let mut residuals = None;
    let mut foreign = None;
    let mut rows = Vec::new();
    for (i, request) in array(&requests)?.iter().enumerate() {
        let id = text(&request["id"])?;
        let program = load(request, &counters)?;
        let file = program.file(b"/fixture.ts").ok_or("fixture missing")?;
        let ast = file.bound().view().ast();
        let statements = ast
            .node(file.source())?
            .statement_list()
            .ok_or("statements missing")?;
        let statements: Vec<_> = ast
            .node_slice(ast.list(statements)?.nodes())?
            .iter()
            .flatten()
            .collect();
        let mut declarations = BTreeMap::new();
        for &node in &statements {
            if let Some(name) = ast.node(node)?.name() {
                declarations.insert(ast.node_text(name)?.as_bytes().to_vec(), name);
            }
        }
        if i == 0 {
            let first = Arc::new(CheckerOwner::for_program(
                CheckerIdentity::new(generation.clone(), &counters),
                &counters,
                Arc::new(ProgramCheckerHost::new(program.clone())),
            )?);
            let second = Arc::new(CheckerOwner::for_program(
                CheckerIdentity::new(generation.clone(), &counters),
                &counters,
                Arc::new(ProgramCheckerHost::new(program.clone())),
            )?);
            let other = {
                let op = second.operation()?;
                op.builtin_type("anyType")
                    .ok_or("foreign builtin missing")?
            };
            let mut op = first.operation()?;
            residuals = Some(op.observe_residual_comparators(statements[0], statements[1])?);
            foreign = Some(
                match op.compare_type_order(op.builtin_type("anyType"), Some(other)) {
                    Err(error) => json!({"state":"rejected","error":format!("{error:?}")}),
                    Ok(value) => json!({"state":"returned","value":value as i8}),
                },
            );
        }
        let mut observed = Vec::new();
        for group in array(&inputs["cases"][id])? {
            let owner = Arc::new(CheckerOwner::for_program(
                CheckerIdentity::new(generation.clone(), &counters),
                &counters,
                Arc::new(ProgramCheckerHost::new(program.clone())),
            )?);
            let mut op = owner.operation()?;
            // Declaration lookup follows the first source/target schedule at
            // the pin. Relations are tested separately by p3_relations.
            let mut types = BTreeMap::new();
            let result = (|| -> Result<Value> {
                for action in array(&request["actions"])? {
                    for key in ["source", "target"] {
                        let name = text(&action[key])?;
                        if !types.contains_key(name) {
                            let node = declarations[name.as_bytes()];
                            let symbol =
                                op.get_symbol_at_location(node)?.ok_or("symbol missing")?;
                            types.insert(name.to_owned(), op.get_declared_type_of_symbol(symbol)?);
                        }
                    }
                }
                let name = text(&group["type"])?;
                let ty = types[name];
                if op.type_flags(ty)? & ts_checker::type_flags::UNION == 0 {
                    return Err("expected union type".into());
                }
                let original = op.constituents(ty)?;
                let mut pairwise = Vec::new();
                for &a in &original {
                    let mut row = Vec::new();
                    for &b in &original {
                        row.push(op.compare_type_order(Some(a), Some(b))? as i8);
                    }
                    pairwise.push(row);
                }
                let mut permutations = Vec::new();
                for permutation in array(&group["inputs"])? {
                    let input: Vec<usize> = array(permutation)?
                        .iter()
                        .map(|i| {
                            i.as_u64()
                                .and_then(|i| usize::try_from(i).ok())
                                .ok_or("invalid permutation index")
                        })
                        .collect::<std::result::Result<_, _>>()?;
                    let mut inventory = input.clone();
                    inventory.sort_unstable();
                    if inventory != (0..original.len()).collect::<Vec<_>>() {
                        return Err("permutation inventory mismatch".into());
                    }
                    let mut sorted = input.clone();
                    let mut failure = None;
                    sorted.sort_by(|&a, &b| {
                        match op.compare_type_order(Some(original[a]), Some(original[b])) {
                            Ok(value) => value,
                            Err(error) => {
                                failure.get_or_insert(error);
                                std::cmp::Ordering::Equal
                            }
                        }
                    });
                    if let Some(error) = failure {
                        return Err(error.into());
                    }
                    permutations.push(json!({"input":input,"sorted":sorted}));
                }
                Ok(
                    json!({"state":"executed","type":name,"pairwise":pairwise,"permutations":permutations}),
                )
            })();
            observed.push(match result {
                Ok(value) => value,
                Err(error) => {
                    json!({"state":"pending","type":group["type"],"reason":error.to_string()})
                }
            });
        }
        rows.push(json!({"id":id,"ordering":observed}));
    }
    std::fs::write(
        &args[3],
        serde_json::to_vec(
            &json!({"scope":"P3 direct comparator observation; integer signs and identical permutations, no full S08 claim","request_sha256":request_sha256,"native_sha256":inputs["native_sha256"],"residuals":residuals,"foreign_checker":foreign,"rows":rows}),
        )?,
    )?;
    Ok(())
}
