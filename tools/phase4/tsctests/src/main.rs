//! Runs recorded command-line scenarios through the ported harness.
//!
//!     phase4_tsctests --suite tsc [--root DIR] [--scenarios FILE] list
//!     phase4_tsctests --suite tsc [--root DIR] [--scenarios FILE] run --id ID --local DIR
//!     phase4_tsctests --output DIR [--scenarios FILE] [--jobs N] SELECTOR...
//!
//! `--suite tsc` is the `tsc` suite of `scripts/parity.py`
//! (docs/EVIDENCE-plan.md). `list` prints every scenario id of the
//! inventory, one per line. `run` runs the scenario `ID` and prints its
//! result lines (`phase4_tsctests::suite`), one JSON object per sub-test;
//! a transcript that differs from the reference is written under `--local`.
//! `DIR` defaults to the current directory and must hold
//! `upstream/tsc/testdata`; `FILE` defaults to
//! `DIR/data/phase4/scenarios.json.gz`. The exit status is 0 when the
//! command ran (a failing sub-test is a result line), 2 for bad arguments
//! (an unknown id among them) and 1 when the inventory cannot be read.
//!
//! Without `--suite`, a selector is `all`, a family (`tsc`, `tsbuild`,
//! `tscWatch`, `tsbuildWatch`) or a scenario id (`tsc/commandLine/help.js`,
//! with or without `.js`). `FILE` defaults to the repository's
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
use std::process::ExitCode;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, PoisonError};
use tsr_testrunner::baseline::Roots;
use tsr_testrunner::TestData;

const USAGE: &str =
    "usage: phase4_tsctests --output DIR [--scenarios FILE] [--jobs N] (all|FAMILY|ID)...";
const SUITE_USAGE: &str = "usage: phase4_tsctests --suite tsc [--root DIR] [--scenarios FILE] (list | run --id ID --local DIR)";

/// A scenario's row and its transcript.
type ScenarioResult = (Value, Vec<u8>);

/// What the command line asks for.
#[derive(Debug, PartialEq, Eq)]
enum Invocation {
    /// `--output DIR ... SELECTOR...`: rows and transcripts for a comparison.
    Rows(Arguments),
    /// `--suite tsc ...`: the parity suite's `list` or `run`.
    Suite(SuiteArguments),
}

#[derive(Debug, PartialEq, Eq)]
struct Arguments {
    output: PathBuf,
    scenarios: PathBuf,
    jobs: usize,
    selectors: Vec<String>,
}

#[derive(Debug, PartialEq, Eq)]
struct SuiteArguments {
    /// The repository root: `upstream/tsc/testdata` is below it.
    root: PathBuf,
    scenarios: PathBuf,
    command: SuiteCommand,
}

#[derive(Debug, PartialEq, Eq)]
enum SuiteCommand {
    List,
    Run { id: String, local: PathBuf },
}

/// The invocation `args` (the program name left out) asks for; the error is
/// the message to print, usage included.
fn parse(args: Vec<String>) -> Result<Invocation, String> {
    if args.iter().any(|arg| arg == "--suite") {
        parse_suite(args)
            .map(Invocation::Suite)
            .map_err(|error| format!("{error}\n{SUITE_USAGE}"))
    } else {
        parse_rows(args).map(Invocation::Rows)
    }
}

fn parse_rows(args: Vec<String>) -> Result<Arguments, String> {
    let mut output = None;
    let mut scenarios =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../data/phase4/scenarios.json.gz");
    let mut jobs = std::thread::available_parallelism().map_or(1, |n| n.get().min(8));
    let mut selectors = Vec::new();
    let mut args = args.into_iter();
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

fn parse_suite(args: Vec<String>) -> Result<SuiteArguments, String> {
    let mut suite = None;
    let mut root = None;
    let mut scenarios = None;
    let mut command = None;
    let mut id = None;
    let mut local = None;
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        let mut value = |name: &str| args.next().ok_or_else(|| format!("{name} needs a value"));
        match arg.as_str() {
            "--suite" => suite = Some(value("--suite")?),
            "--root" => root = Some(PathBuf::from(value("--root")?)),
            "--scenarios" => scenarios = Some(PathBuf::from(value("--scenarios")?)),
            "--id" => id = Some(value("--id")?),
            "--local" => local = Some(PathBuf::from(value("--local")?)),
            "list" | "run" if command.is_none() => command = Some(arg),
            "list" | "run" => return Err("one command: list or run".to_owned()),
            _ => return Err(format!("unknown argument {arg}")),
        }
    }
    match suite.as_deref() {
        Some("tsc") => {}
        Some(other) => return Err(format!("unknown suite {other}; this runner has tsc")),
        None => return Err("--suite needs a value".to_owned()),
    }
    let command = match command.as_deref() {
        Some("list") if id.is_none() && local.is_none() => SuiteCommand::List,
        Some("list") => return Err("list takes no --id or --local".to_owned()),
        Some(_) => SuiteCommand::Run {
            id: id.ok_or("run needs --id")?,
            local: local.ok_or("run needs --local")?,
        },
        None => return Err("list or run is required".to_owned()),
    };
    let root = root.unwrap_or_else(|| PathBuf::from("."));
    let scenarios = scenarios.unwrap_or_else(|| root.join("data/phase4/scenarios.json.gz"));
    Ok(SuiteArguments {
        root,
        scenarios,
        command,
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

/// Runs `scenario` on a thread with the work group's stack, as the rows mode
/// runs every scenario.
fn run_on_scenario_thread(scenario: &Scenario) -> ScenarioResult {
    std::thread::scope(|scope| {
        std::thread::Builder::new()
            .name("phase4-scenario".into())
            .stack_size(tsr_core::workgroup::RESERVED_STACK)
            .spawn_scoped(scope, || run_scenario(scenario))
            .expect("a scenario thread starts")
            .join()
            .unwrap_or_else(|payload| std::panic::resume_unwind(payload))
    })
}

fn write_rows(arguments: &Arguments) -> Result<(), String> {
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
        let workers: Vec<_> = (0..arguments.jobs.min(selected.len()))
            .map(|_| {
                std::thread::Builder::new()
                    .name("phase4-scenario".into())
                    .stack_size(tsr_core::workgroup::RESERVED_STACK)
                    .spawn_scoped(scope, worker)
                    .expect("a scenario thread starts")
            })
            .collect();
        // Joined, not dropped, as the work group joins its workers: detaching
        // a finished thread on a reserved stack can fault in glibc.
        let panics: Vec<_> = workers.into_iter().filter_map(|w| w.join().err()).collect();
        if let Some(payload) = panics.into_iter().next() {
            std::panic::resume_unwind(payload);
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

/// How a suite command ended, mapped to the exit status.
enum SuiteFailure {
    /// Bad arguments: exit 2.
    Usage(String),
    /// The inventory or stdout failed: exit 1.
    Failure(String),
}

fn run_suite(arguments: &SuiteArguments) -> Result<(), SuiteFailure> {
    let testdata = TestData::in_repository(&arguments.root);
    if matches!(arguments.command, SuiteCommand::Run { .. }) && !testdata.path().is_dir() {
        return Err(SuiteFailure::Usage(format!(
            "{} is not a directory; pass --root",
            testdata.path().display()
        )));
    }
    let inventory = read_inventory(&arguments.scenarios).map_err(SuiteFailure::Failure)?;
    let mut out = Vec::new();
    match &arguments.command {
        SuiteCommand::List => {
            for scenario in &inventory.scenarios {
                out.extend_from_slice(scenario.id.as_bytes());
                out.push(b'\n');
            }
        }
        SuiteCommand::Run { id, local } => {
            let scenario = inventory
                .scenarios
                .iter()
                .find(|scenario| scenario.id == *id)
                .ok_or_else(|| SuiteFailure::Usage(format!("no scenario {id} in suite tsc")))?;
            install_panic_hook();
            let (row, transcript) = run_on_scenario_thread(scenario);
            let roots = Roots::new(testdata.reference(), local.clone());
            phase4_tsctests::suite::report(&row, &transcript, &roots)
                .write(&mut out)
                .map_err(|error| SuiteFailure::Failure(error.to_string()))?;
        }
    }
    std::io::stdout()
        .lock()
        .write_all(&out)
        .map_err(|error| SuiteFailure::Failure(format!("stdout: {error}")))
}

fn main() -> ExitCode {
    let (status, error) = match parse(std::env::args().skip(1).collect()) {
        Err(message) => (2, Some(message)),
        Ok(Invocation::Rows(arguments)) => match write_rows(&arguments) {
            Ok(()) => (0, None),
            Err(error) => (2, Some(error)),
        },
        Ok(Invocation::Suite(arguments)) => match run_suite(&arguments) {
            Ok(()) => (0, None),
            Err(SuiteFailure::Usage(error)) => (2, Some(format!("{error}\n{SUITE_USAGE}"))),
            Err(SuiteFailure::Failure(error)) => (1, Some(error)),
        },
    };
    if let Some(error) = error {
        eprintln!("phase4_tsctests: {error}");
    }
    ExitCode::from(status)
}

#[cfg(test)]
mod tests {
    use super::{parse, Arguments, Invocation, SuiteArguments, SuiteCommand};
    use std::path::PathBuf;

    fn parse_words(words: &str) -> Result<Invocation, String> {
        parse(words.split_whitespace().map(str::to_owned).collect())
    }

    fn suite(words: &str) -> SuiteArguments {
        match parse_words(words) {
            Ok(Invocation::Suite(arguments)) => arguments,
            other => panic!("{words}: {other:?}"),
        }
    }

    fn suite_error(words: &str) -> String {
        let error = parse_words(words).expect_err(words);
        assert!(error.ends_with(super::SUITE_USAGE), "{error}");
        error
    }

    #[test]
    fn list_defaults_the_root_to_the_current_directory() {
        assert_eq!(
            suite("--suite tsc list"),
            SuiteArguments {
                root: PathBuf::from("."),
                scenarios: PathBuf::from("./data/phase4/scenarios.json.gz"),
                command: SuiteCommand::List,
            }
        );
    }

    #[test]
    fn run_takes_an_id_and_a_local_directory_in_any_order() {
        let expected = SuiteArguments {
            root: PathBuf::from("/repo"),
            scenarios: PathBuf::from("/repo/data/phase4/scenarios.json.gz"),
            command: SuiteCommand::Run {
                id: "tsc/commandLine/help.js".to_owned(),
                local: PathBuf::from("/out/local"),
            },
        };
        assert_eq!(
            suite("--suite tsc --root /repo run --id tsc/commandLine/help.js --local /out/local"),
            expected
        );
        assert_eq!(
            suite("run --local /out/local --id tsc/commandLine/help.js --root /repo --suite tsc"),
            expected
        );
    }

    #[test]
    fn scenarios_overrides_the_inventory_under_the_root() {
        let arguments = suite("--suite tsc --root /repo --scenarios /elsewhere/s.json list");
        assert_eq!(arguments.root, PathBuf::from("/repo"));
        assert_eq!(arguments.scenarios, PathBuf::from("/elsewhere/s.json"));
    }

    #[test]
    fn bad_suite_arguments_are_errors_with_the_suite_usage() {
        assert!(suite_error("--suite compiler list").contains("unknown suite compiler"));
        assert!(suite_error("--suite tsc").contains("list or run is required"));
        assert!(suite_error("--suite tsc run --local L").contains("run needs --id"));
        assert!(suite_error("--suite tsc run --id X").contains("run needs --local"));
        assert!(suite_error("--suite tsc list --id X").contains("list takes no --id"));
        assert!(suite_error("--suite tsc list run").contains("one command"));
        assert!(suite_error("--suite tsc --output O list").contains("unknown argument --output"));
        assert!(suite_error("--suite tsc list all").contains("unknown argument all"));
        assert!(suite_error("--suite tsc run --id X --local").contains("--local needs a value"));
        assert!(suite_error("list --suite").contains("--suite needs a value"));
    }

    #[test]
    fn without_suite_the_rows_mode_parses_as_before() {
        match parse_words("--output out --jobs 3 tsc tsbuild/sample/x.js") {
            Ok(Invocation::Rows(Arguments {
                output,
                scenarios,
                jobs,
                selectors,
            })) => {
                assert_eq!(output, PathBuf::from("out"));
                assert!(scenarios.ends_with("data/phase4/scenarios.json.gz"));
                assert_eq!(jobs, 3);
                assert_eq!(selectors, ["tsc", "tsbuild/sample/x.js"]);
            }
            other => panic!("{other:?}"),
        }
        // `list` and `run` are selectors there, and a missing output is an error.
        assert!(matches!(
            parse_words("--output out list"),
            Ok(Invocation::Rows(Arguments { selectors, .. })) if selectors == ["list"]
        ));
        assert!(parse_words("all").is_err());
        assert!(parse_words("--output out --root r all")
            .unwrap_err()
            .contains("unknown option --root"));
    }
}
