//! Runs recorded command-line scenarios through the ported harness.
//!
//!     phase4_tsctests --output DIR [--scenarios FILE] [--jobs N] SELECTOR...
//!
//! A selector is `all`, a family (`tsc`, `tsbuild`, `tscWatch`,
//! `tsbuildWatch`) or a scenario id (`tsc/commandLine/help.js`, with or
//! without `.js`). `FILE` defaults to the repository's
//! `data/phase4/scenarios.json.gz`. The binary writes, under `DIR`:
//!
//! - `baselines/<id>`: each scenario's transcript as far as it got;
//! - `rows.jsonl`: one row per selected scenario, in inventory order (the
//!   format of `phase4_tsctests::row`);
//! - `summary.json`: the inventory's pin and size, the selection, and the
//!   rows by state and family.
//!
//! It exits nonzero only when it cannot run (bad arguments, an unreadable or
//! malformed inventory, an output it cannot write); a scenario's outcome is
//! its row.
use phase4_tsctests::row::{install_panic_hook, run_scenario};
use phase4_tsctests::scenario::{read_inventory, Scenario, FAMILIES};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, PoisonError};

const USAGE: &str =
    "usage: phase4_tsctests --output DIR [--scenarios FILE] [--jobs N] (all|FAMILY|ID)...";

/// A scenario's row and its transcript.
type ScenarioResult = (Value, Vec<u8>);

struct Arguments {
    output: PathBuf,
    scenarios: PathBuf,
    jobs: usize,
    selectors: Vec<String>,
}

fn arguments() -> Result<Arguments, String> {
    let mut output = None;
    let mut scenarios =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../data/phase4/scenarios.json.gz");
    let mut jobs = std::thread::available_parallelism().map_or(1, |n| n.get().min(8));
    let mut selectors = Vec::new();
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let mut value = |name: &str| {
            args.next()
                .ok_or_else(|| format!("{name} needs a value\n{USAGE}"))
        };
        match arg.as_str() {
            "--output" => output = Some(PathBuf::from(value("--output")?)),
            "--scenarios" => scenarios = PathBuf::from(value("--scenarios")?),
            "--jobs" => {
                jobs = value("--jobs")?
                    .parse()
                    .ok()
                    .filter(|&jobs| jobs > 0)
                    .ok_or_else(|| format!("--jobs needs a positive integer\n{USAGE}"))?;
            }
            _ if arg.starts_with("--") => return Err(format!("unknown option {arg}\n{USAGE}")),
            _ => selectors.push(arg),
        }
    }
    let output = output.ok_or_else(|| USAGE.to_owned())?;
    if selectors.is_empty() {
        return Err(USAGE.to_owned());
    }
    Ok(Arguments {
        output,
        scenarios,
        jobs,
        selectors,
    })
}

/// The selected scenarios, in inventory order; an unknown selector is an
/// error.
fn select<'a>(
    scenarios: &'a [Scenario],
    selectors: &[String],
) -> Result<Vec<&'a Scenario>, String> {
    let mut chosen = vec![false; scenarios.len()];
    for selector in selectors {
        // An id is the reference path; `.js` may be left out.
        let id = match selector.strip_suffix(".js") {
            Some(_) => selector.clone(),
            None => format!("{selector}.js"),
        };
        let mut matched = false;
        for (index, scenario) in scenarios.iter().enumerate() {
            if selector == "all" || scenario.family == *selector || scenario.id == id {
                chosen[index] = true;
                matched = true;
            }
        }
        if !matched {
            return Err(format!("no scenario matches {selector}"));
        }
    }
    Ok(scenarios
        .iter()
        .zip(chosen)
        .filter_map(|(scenario, chosen)| chosen.then_some(scenario))
        .collect())
}

fn write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("{}: {error}", parent.display()))?;
    }
    std::fs::write(path, bytes).map_err(|error| format!("{}: {error}", path.display()))
}

fn run() -> Result<(), String> {
    let arguments = arguments()?;
    let inventory = read_inventory(&arguments.scenarios)?;
    let selected = select(&inventory.scenarios, &arguments.selectors)?;
    install_panic_hook();

    let rows: Vec<Mutex<Option<ScenarioResult>>> =
        selected.iter().map(|_| Mutex::new(None)).collect();
    let next = AtomicUsize::new(0);
    let worker = || loop {
        let index = next.fetch_add(1, Ordering::Relaxed);
        let Some(scenario) = selected.get(index) else {
            break;
        };
        let result = run_scenario(scenario);
        *rows[index].lock().unwrap_or_else(PoisonError::into_inner) = Some(result);
    };
    std::thread::scope(|scope| {
        for _ in 0..arguments.jobs.min(selected.len()) {
            std::thread::Builder::new()
                .name("phase4-scenario".into())
                .stack_size(tsr_core::workgroup::RESERVED_STACK)
                .spawn_scoped(scope, worker)
                .expect("a scenario thread starts");
        }
    });

    let mut lines = Vec::new();
    let mut by_state: BTreeMap<String, BTreeMap<String, usize>> = BTreeMap::new();
    for (scenario, row) in selected.iter().zip(rows) {
        let (row, transcript) = row
            .into_inner()
            .unwrap_or_else(PoisonError::into_inner)
            .expect("every selected scenario ran");
        write(
            &arguments.output.join("baselines").join(&scenario.id),
            &transcript,
        )?;
        let state = row["state"].as_str().expect("a row has a state").to_owned();
        *by_state
            .entry(state)
            .or_default()
            .entry(scenario.family.clone())
            .or_default() += 1;
        serde_json::to_writer(&mut lines, &row).map_err(|error| error.to_string())?;
        lines.push(b'\n');
    }
    write(&arguments.output.join("rows.jsonl"), &lines)?;
    let summary = json!({
        "version": 1,
        "pin": inventory.pin,
        "inventory": {"scenarios": inventory.scenarios.len(), "orphan_references": inventory.orphan_references},
        "selectors": arguments.selectors,
        "rows": selected.len(),
        "families": FAMILIES,
        "states": by_state,
    });
    let mut text = serde_json::to_vec_pretty(&summary).map_err(|error| error.to_string())?;
    text.push(b'\n');
    write(&arguments.output.join("summary.json"), &text)?;
    std::io::stdout()
        .write_all(&text)
        .map_err(|error| error.to_string())
}

fn main() {
    if let Err(error) = run() {
        eprintln!("phase4_tsctests: {error}");
        std::process::exit(2);
    }
}
