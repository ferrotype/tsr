//! One result row per scenario. A row is never blank: every scenario ends in
//! exactly one state.
//!
//! - `completed`: the runner finished; the transcript is the rendered
//!   baseline (`sha256`, `bytes`), and `unexpected_diff` is what the pin
//!   reports with `t.Errorf` (`null` when nothing).
//! - `unsupported`: the Rust command line refused a named `operation`.
//! - `failed`: a `panic` (with its `reason` and `location`, `null` when the
//!   hook saw none) or a `harness` defect (the scenario does not replay as
//!   recorded, or the runner named another baseline).
//!
//! Every row carries how far the transcript got (`progress`) and the digest
//! and size of the transcript as far as it got (`transcript`), which the
//! binary writes beside the rows.
use crate::execute::Unsupported;
use crate::runner::{Progress, Transcript};
use crate::scenario::{hex_digest, Scenario};
use serde_json::{json, Value};
use std::any::Any;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::{Mutex, Once, PoisonError};

/// The row format's version.
pub const ROW_VERSION: u64 = 1;

/// The panics the hook saw, newest last: message and location. A panic in a
/// work-group thread reaches the scenario's thread by `resume_unwind`, which
/// does not run the hook again, so a caught payload is matched back to its
/// location by message.
static PANICS: Mutex<Vec<(String, Option<String>)>> = Mutex::new(Vec::new());
const PANICS_KEPT: usize = 256;

/// The text of a panic payload, `None` for an [`Unsupported`] refusal.
fn payload_message(payload: &(dyn Any + Send)) -> Option<String> {
    if payload.is::<Unsupported>() {
        return None;
    }
    Some(
        payload
            .downcast_ref::<String>()
            .map(String::as_str)
            .or_else(|| payload.downcast_ref::<&str>().copied())
            .unwrap_or("non-string panic payload")
            .to_owned(),
    )
}

/// Records every panic's location; prints the panic unless it is a named
/// refusal.
pub fn install_panic_hook() {
    static INSTALL: Once = Once::new();
    INSTALL.call_once(|| {
        std::panic::set_hook(Box::new(|info| {
            let location = info
                .location()
                .map(|at| format!("{}:{}", at.file(), at.line()));
            if let Some(message) = payload_message(info.payload()) {
                let mut panics = PANICS.lock().unwrap_or_else(PoisonError::into_inner);
                if panics.len() == PANICS_KEPT {
                    panics.remove(0);
                }
                panics.push((message, location));
                eprintln!("{info}");
            }
        }));
    });
}

fn panic_location(message: &str) -> Option<String> {
    PANICS
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .iter()
        .rev()
        .find(|(seen, _)| seen == message)
        .and_then(|(_, location)| location.clone())
}

/// The state of a run that unwound.
pub fn unwound(payload: &(dyn Any + Send)) -> Value {
    if let Some(refusal) = payload.downcast_ref::<Unsupported>() {
        return json!({"state": "unsupported", "operation": refusal.operation});
    }
    let reason = payload_message(payload).expect("a payload that is not a refusal has a message");
    let location = panic_location(&reason);
    json!({"state": "failed", "class": "panic", "reason": reason, "location": location})
}

fn harness_failure(reason: &str) -> Value {
    json!({"state": "failed", "class": "harness", "reason": reason, "location": null})
}

fn progress_json(progress: &Progress, edits: usize) -> Value {
    json!({
        "stage": if progress.stage.is_empty() { "setup" } else { progress.stage },
        "commands": progress.commands,
        "edits_completed": progress.edits_completed,
        "edits": edits,
    })
}

/// Runs `scenario` and returns its row and its transcript as far as it got.
pub fn run_scenario(scenario: &Scenario) -> (Value, Vec<u8>) {
    install_panic_hook();
    let mut transcript = Transcript::default();
    let outcome = catch_unwind(AssertUnwindSafe(|| {
        let input = scenario.to_tsc_input();
        scenario.check_initial_state(&input)?;
        let result = input.run(&scenario.scenario, &mut transcript);
        if result.baseline_path != scenario.id {
            return Err(format!(
                "the runner named the baseline {}, the recording {}",
                result.baseline_path, scenario.id
            ));
        }
        Ok(result)
    }));
    let state = match outcome {
        Ok(Ok(result)) => {
            let unexpected = (!result.unexpected_diff.is_empty())
                .then(|| String::from_utf8_lossy(&result.unexpected_diff).into_owned());
            json!({"state": "completed", "sha256": hex_digest(&transcript.text),
                   "bytes": transcript.text.len(), "unexpected_diff": unexpected})
        }
        Ok(Err(reason)) => harness_failure(&reason),
        Err(payload) => unwound(payload.as_ref()),
    };
    let mut row = json!({
        "version": ROW_VERSION,
        "id": scenario.id,
        "family": scenario.family,
        "baseline": scenario.id,
        "scenario_sha256": scenario.sha256,
        "progress": progress_json(&transcript.progress, scenario.edits.len()),
        "transcript": {"sha256": hex_digest(&transcript.text), "bytes": transcript.text.len()},
    });
    for (key, value) in state.as_object().expect("a state is an object") {
        row[key] = value.clone();
    }
    (row, transcript.text)
}
