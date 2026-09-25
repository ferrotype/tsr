//! Phase 2 C0.3 corpus row: the S08 P5 observation (diagnostic phases, error
//! and type/symbol baselines, public TypeToString) plus the runner sub-tests
//! Phase 2 owns, run after the baseline walk on the same program and checker.
#[path = "../../../tools/s08/p5/baseline/mod.rs"]
mod baseline;
#[path = "../../../tools/s08/p5/corpus.rs"]
mod corpus;
#[path = "../../../tools/s08/p5/errors.rs"]
mod errors;
#[path = "../../../tools/s08/p4/executor.rs"]
mod executor;
#[path = "../../../tools/s08/p5/paths.rs"]
mod paths;
#[path = "../../../tools/phase2/subtests.rs"]
mod subtests;

use serde_json::{json, Value};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::{Arc, Mutex, PoisonError};
use tsr_compiler::{FileCache, Program, ProgramOptions};

/// The location of the last panic, so a fatal row names the code that
/// panicked: production (`crates/`) or this adapter (`tools/`, `examples/`).
static PANIC_LOCATION: Mutex<Option<String>> = Mutex::new(None);

fn last_panic() -> Option<String> {
    PANIC_LOCATION
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone()
}

fn guarded(observe: impl FnOnce() -> Value) -> Value {
    match catch_unwind(AssertUnwindSafe(observe)) {
        Ok(value) => value,
        Err(payload) => {
            let reason = payload
                .downcast_ref::<String>()
                .map(String::as_str)
                .or_else(|| payload.downcast_ref::<&str>().copied())
                .unwrap_or("non-string panic payload");
            json!({"state":"failed","class":"panic","reason":reason,"location":last_panic()})
        }
    }
}

struct Phase2 {
    program: Option<Arc<Program>>,
    trace_resolution: bool,
    subtests: Option<Value>,
}

impl executor::Hooks for Phase2 {
    fn load_program(
        &mut self,
        options: ProgramOptions,
        cache: &mut FileCache,
        counters: &tsr_arena::Counters,
    ) -> Result<Arc<Program>, tsr_compiler::Error> {
        let program = Program::load(options, cache, counters).map(Arc::new)?;
        self.program = Some(program.clone());
        Ok(program)
    }

    // The pinned runner order after types and symbols: module resolution,
    // union ordering, source file parent pointers.
    fn checkpoint(&mut self, op: &mut tsr_checker::Operation<'_>) {
        let Some(program) = self.program.clone() else {
            return;
        };
        let trace = guarded(|| subtests::trace(&program, self.trace_resolution));
        let ordering = guarded(|| subtests::union_ordering(op));
        let parents = guarded(|| subtests::parent_pointers(&program));
        self.subtests =
            Some(json!({"trace":trace,"union_ordering":ordering,"parent_pointers":parents}));
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    std::panic::set_hook(Box::new(|info| {
        *PANIC_LOCATION
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = info
            .location()
            .map(|at| format!("{}:{}", at.file(), at.line()));
        eprintln!("{info}");
    }));
    let args: Vec<_> = std::env::args_os().collect();
    if args.len() != 3 {
        return Err("usage: phase2_checker REQUEST OUTPUT".into());
    }
    let request: Value = serde_json::from_slice(&std::fs::read(&args[1])?)?;
    let mut hooks = Phase2 {
        program: None,
        trace_resolution: request["loading"]["options"]["traceResolution"] == true,
        subtests: None,
    };
    let mut row = corpus::observe_with(&request, &mut FileCache::new(), &mut hooks, false);
    if row.get("fatal").is_some() {
        row["panic_location"] = json!(last_panic());
    }
    row["phase2"] = hooks
        .subtests
        .take()
        .unwrap_or_else(|| json!({"state":"not_reached"}));
    std::fs::write(&args[2], serde_json::to_vec(&row)?)?;
    Ok(())
}
