//! Phase 2 C6.7 assignment witnesses, the Rust side of
//! `scripts/phase2_assignments.py compare`: for every input line, the
//! association plan of the compiler checker pool.
//!
//! A `program` line carries a corpus row's request (the frozen loading request
//! and the native harness inputs the config parse reads); the program loads as
//! the corpus loads it, in the concurrent mode (the program's own
//! single-threaded setting unknown), and the plan is the pool's for its
//! checker count. A
//! `synthetic` line carries per-file inputs and a checker count, and the plan
//! is computed from them directly. Output is one JSON line per input line.
#[path = "../../../tools/s08/p4/config.rs"]
mod config;
#[allow(dead_code)]
#[path = "../../../tools/s07/program/rust_observation.rs"]
mod observation;

use serde_json::{json, Value};
use std::io::{BufRead, Write};
use std::sync::Arc;
use tsr_compiler as ts_compiler_error;
use tsr_compiler::{
    CheckerAssociationPlan, CompilerCheckerPool, FileCache, Program, ProgramOptions,
};

fn plan_json(plan: &CheckerAssociationPlan) -> Value {
    json!({
        "checker_count": plan.checker_count,
        "node_counts": plan.node_counts,
        "text_lengths": plan.text_lengths,
        "import_counts": plan.import_counts,
        "is_declaration_file": plan.is_declaration_file,
        "adjacency": plan.adjacency.iter().map(|adjacent| {
            let mut adjacent = adjacent.clone();
            adjacent.sort_unstable();
            adjacent
        }).collect::<Vec<_>>(),
        "policy": plan.policy.map(|policy| json!({
            "prioritize_source_files": policy.prioritize_source_files,
            "source_file_weight_multiplier": policy.source_file_weight_multiplier,
            "balance_penalty_multiplier": policy.balance_penalty_multiplier,
        })),
        "file_weights": plan.file_weights,
        "order": plan.order,
        "associations": plan.associations,
    })
}

fn numbers(value: &Value) -> Vec<i64> {
    value
        .as_array()
        .expect("number array")
        .iter()
        .map(|number| number.as_i64().expect("integer"))
        .collect()
}

fn synthetic(line: &Value) -> Value {
    let adjacency = line["adjacency"]
        .as_array()
        .expect("adjacency")
        .iter()
        .map(|adjacent| {
            numbers(adjacent)
                .into_iter()
                .map(|index| usize::try_from(index).expect("file index"))
                .collect()
        })
        .collect();
    let plan = CheckerAssociationPlan::compute(
        usize::try_from(line["checker_count"].as_u64().expect("checker count")).expect("count"),
        numbers(&line["node_counts"]),
        numbers(&line["text_lengths"]),
        numbers(&line["import_counts"]),
        line["is_declaration_file"]
            .as_array()
            .expect("declaration flags")
            .iter()
            .map(|flag| flag.as_bool().expect("flag"))
            .collect(),
        adjacency,
    );
    json!({"id": line["id"], "state": "computed", "plan": plan_json(&plan)})
}

fn program(line: &Value) -> Value {
    let request = &line["request"];
    let parsed = match config::parse(request) {
        Ok(parsed) => parsed,
        Err(error) => {
            return json!({"id": line["id"], "state": "config_failed", "reason": error.to_string()})
        }
    };
    // The concurrent-mode harness leaves the program's setting unknown, so
    // the compiler option decides (harnessutil.createProgram).
    let options = observation::program_options(&request["loading"], parsed);
    let counters = tsr_arena::Counters::new();
    let program = match Program::load(options, &mut FileCache::new(), &counters) {
        Ok(program) => Arc::new(program),
        Err(tsr_compiler::Error::Unsupported(reason)) => {
            return json!({"id": line["id"], "state": "unsupported", "reason": reason});
        }
        Err(error) => {
            return json!({"id": line["id"], "state": "load_failed", "reason": error.to_string()})
        }
    };
    let pool = CompilerCheckerPool::new(program.clone(), &counters);
    let files = program
        .files()
        .iter()
        .map(|file| {
            let source = file
                .bound()
                .view()
                .source_file()
                .expect("bound source file");
            String::from_utf8_lossy(source.parse_options().file_name.as_bytes()).into_owned()
        })
        .collect::<Vec<_>>();
    match CheckerAssociationPlan::for_program(&program, pool.checker_count()) {
        Ok(plan) => {
            json!({"id": line["id"], "state": "computed", "files": files, "plan": plan_json(&plan)})
        }
        Err(error) => {
            json!({"id": line["id"], "state": "plan_failed", "reason": error.to_string()})
        }
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let stdin = std::io::stdin();
    let mut stdout = std::io::BufWriter::new(std::io::stdout().lock());
    for line in stdin.lock().lines() {
        let line: Value = serde_json::from_str(&line?)?;
        let result = match line["kind"].as_str() {
            Some("synthetic") => synthetic(&line),
            Some("program") => program(&line),
            _ => return Err("unknown input kind".into()),
        };
        serde_json::to_writer(&mut stdout, &result)?;
        stdout.write_all(b"\n")?;
    }
    stdout.flush()?;
    Ok(())
}
