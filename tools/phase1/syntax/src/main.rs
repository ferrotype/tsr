//! Phase 1 F4a: the Rust side of the syntax schedule and of the syntax, AST,
//! navigation and evaluator requests.
//!
//! A private harness, following tools/phase1/config. It calls production APIs
//! where they exist and emits an explicit `not_implemented` row naming the
//! missing operation where they do not. It never emulates a missing algorithm
//! to make a comparison run, and it never reads an expected result.
//!
//! `phase1_syntax --schedule <probe-requests.json> <rows.jsonl>` answers the
//! corpus syntax schedule, one program per row.

use std::error::Error;

mod schedule;

fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<String> = std::env::args().collect();
    match args.as_slice() {
        [_, flag, input, output] if flag == "--schedule" => schedule::run(input, output),
        _ => Err("usage: phase1_syntax --schedule <probe-requests.json> <rows.jsonl>".into()),
    }
}
