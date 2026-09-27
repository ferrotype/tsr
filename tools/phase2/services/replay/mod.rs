//! Phase 2 C5.5: the recorded fourslash services replay.
//!
//! Reads the recorder's raw event stream (`scripts/phase2_services.py record`),
//! rebuilds each recorded program from the test's files and the snapshot texts,
//! gives each recorded checker a fresh Rust checker over it, and replays that
//! checker's calls in the recorded order through the checker's public entry
//! points. Every result is compared with the recorded one: a type, symbol or
//! signature by the fields that build it, the first time its token appears,
//! and by identity afterwards. One JSON line per test reports the outcome of
//! every call by operation.
mod ops;
mod values;

use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, HashMap};
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::rc::Rc;
use std::sync::Arc;
use tsr_arena::{CheckerIdentity, Counters, Generation};
use tsr_checker::CheckerOwner;
use tsr_compiler::{FileCache, Program, ProgramCheckerHost, ProgramOptions};
use tsr_jsstring::JsString;

pub use values::{Outcome, ProgramState, Replay};

type Error = Box<dyn std::error::Error>;

/// Details kept per test: the first calls of each operation that did not match.
const DETAILS_PER_OPERATION: usize = 3;

struct Options {
    input: String,
    output: String,
    shard: usize,
    shards: usize,
    tests: Vec<String>,
}

#[allow(dead_code, reason = "the example's argument parser")]
fn options() -> Result<Options, Error> {
    let mut options = Options {
        input: String::new(),
        output: String::new(),
        shard: 0,
        shards: 1,
        tests: Vec::new(),
    };
    let mut args = std::env::args().skip(1);
    while let Some(flag) = args.next() {
        let mut value = || args.next().ok_or_else(|| format!("{flag} needs a value"));
        match flag.as_str() {
            "--input" => options.input = value()?,
            "--output" => options.output = value()?,
            "--shard" => options.shard = value()?.parse()?,
            "--shards" => options.shards = value()?.parse()?,
            "--test" => options.tests.push(value()?),
            other => return Err(format!("unknown flag {other}").into()),
        }
    }
    if options.input.is_empty() || options.output.is_empty() || options.shard >= options.shards {
        return Err(
            "usage: phase2_services --input RAW --output OUT [--shard K --shards N] [--test NAME]"
                .into(),
        );
    }
    Ok(options)
}

fn hex(value: &str) -> Result<Vec<u8>, Error> {
    if !value.len().is_multiple_of(2) {
        return Err("odd hex length".into());
    }
    (0..value.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&value[i..i + 2], 16).map_err(Into::into))
        .collect()
}

/// One recorded test: its files, the snapshot texts, its programs and checkers.
struct Test {
    name: String,
    files: BTreeMap<String, Vec<u8>>,
    symlinks: BTreeMap<String, String>,
    blobs: HashMap<String, Vec<u8>>,
    programs: HashMap<String, Value>,
    loaded: HashMap<String, Result<Rc<ProgramState>, String>>,
    excluded: HashMap<String, String>,
    checkers: HashMap<String, Checker>,
    report: Report,
}

struct Checker {
    program: String,
    owner: Option<Arc<CheckerOwner>>,
    replay: Option<Replay>,
    /// Why the checker cannot continue: its program failed to load, or a call
    /// panicked and retired it.
    dead: Option<String>,
    excluded: Option<String>,
    calls: usize,
}

#[derive(Default)]
struct Report {
    operations: BTreeMap<String, BTreeMap<&'static str, u64>>,
    details: BTreeMap<String, Vec<Value>>,
    programs: BTreeMap<String, Value>,
    /// Excluded calls by the exclusion's reason.
    exclusions: BTreeMap<String, u64>,
    /// Unsupported calls by operation and reason.
    unsupported: BTreeMap<String, BTreeMap<String, u64>>,
}

impl Report {
    fn count(&mut self, op: &str, outcome: &Outcome, detail: impl FnOnce() -> Value) {
        *self
            .operations
            .entry(op.to_string())
            .or_default()
            .entry(outcome.name())
            .or_default() += 1;
        if let Outcome::Unsupported(reason) = outcome {
            *self
                .unsupported
                .entry(op.to_string())
                .or_default()
                .entry(reason.clone())
                .or_default() += 1;
        }
        if let Outcome::Excluded(reason) = outcome {
            *self.exclusions.entry(reason.clone()).or_default() += 1;
        } else if !matches!(outcome, Outcome::Match) {
            let details = self.details.entry(op.to_string()).or_default();
            if details.len() < DETAILS_PER_OPERATION {
                details.push(detail());
            }
        }
    }
}

/// The program a recorded `program` event describes: the test's files with the
/// snapshot texts the program checks, its roots and its options.
/// The exclusion reason for a program the Rust loader cannot build in the
/// editor's project-reference mode.
const EXCLUDED_REDIRECTION: &str = "Phase 5: the editor's project-reference source redirection";

/// Why the replay does not rebuild a recorded program: a checker program the
/// compiler does not make (the auto-import registry's alias resolver) or a
/// program whose sources come through content mappers. Both are Phase 5's.
fn exclusion(description: &Value) -> Option<String> {
    if description.get("options").is_none() {
        return Some(format!(
            "Phase 5: a {} program",
            description["go_type"].as_str().unwrap_or("non-compiler")
        ));
    }
    if description["content_mappers"]
        .as_array()
        .is_some_and(|list| !list.is_empty())
    {
        return Some("Phase 5: content mappers".to_string());
    }
    None
}

fn load_program(
    test: &Test,
    description: &Value,
    cache: &mut FileCache,
    counters: &Counters,
) -> Result<ProgramState, Error> {
    let cwd = description["cwd"].as_str().unwrap_or("/");
    let case_sensitive = description["case_sensitive"].as_bool().unwrap_or(false);
    let mut texts: BTreeMap<String, Vec<u8>> = test.files.clone();
    let mut expected = Vec::new();
    for file in description["source_files"]
        .as_array()
        .ok_or("program without files")?
    {
        let name = file["name"].as_str().ok_or("file without a name")?;
        expected.push(name.to_string());
        if let Some(sha) = file["text_sha256"].as_str() {
            let text = test
                .blobs
                .get(sha)
                .ok_or_else(|| format!("missing snapshot text {sha}"))?;
            texts.insert(name.to_string(), text.clone());
        }
    }
    let mut fs = tsr_vfs::MemoryBuilder::new(cwd.as_bytes(), case_sensitive);
    for (name, text) in &texts {
        fs.insert_physical(name.as_bytes(), text.clone());
    }
    for (name, target) in &test.symlinks {
        fs.insert_symlink(name.as_bytes(), target.as_bytes());
    }
    let host: Arc<dyn tsr_vfs::FileSystem> =
        Arc::new(tsr_bundled::BundledFs::new(Arc::new(fs.finish())));
    let references = description["project_references"]
        .as_array()
        .is_some_and(|list| !list.is_empty());
    let config = if references {
        // The project system's command line: the config file with its
        // references, whose sources the editor checks in place of outputs.
        let name = description["config_file"].as_str().unwrap_or_default();
        let config_host = tsr_compiler::CompilerConfigHost::new(
            host.clone(),
            JsString::from_bytes(cwd.as_bytes()),
        );
        tsr_tsoptions::get_parsed_command_line_of_config_file(
            name.as_bytes(),
            &tsr_core::CompilerOptions::default(),
            &tsr_tsoptions::ConfigValue::Null,
            &config_host,
        )
        .map_err(|error| format!("config {name}: {error:?}"))?
        .command_line
        .ok_or_else(|| format!("config {name} did not parse"))?
    } else {
        let options = tsr_tsoptions::raw::compiler_options(&description["options"])
            .map_err(|error| format!("options: {error:?}"))?;
        let roots = description["roots"]
            .as_array()
            .map(|roots| {
                roots
                    .iter()
                    .filter_map(Value::as_str)
                    .map(|root| JsString::from_bytes(root.as_bytes()))
                    .collect()
            })
            .unwrap_or_default();
        tsr_tsoptions::ParsedCommandLine::new(options, roots)
    };
    let source_of_references = description["source_files"].as_array().is_some_and(|files| {
        files
            .iter()
            .any(|file| file["from_project_reference"].as_bool() == Some(true))
    });
    let options = |config| ProgramOptions {
        config,
        host: host.clone(),
        current_directory: JsString::from_bytes(cwd.as_bytes()),
        default_library_path: JsString::from_bytes(tsr_bundled::LIB_PATH),
        skip_module_resolution: false,
    };
    match Program::load_with_source_of_project_reference(
        options(config),
        references && source_of_references,
        cache,
        counters,
    ) {
        Ok(program) => ProgramState::new(Arc::new(program), &expected),
        // The editor's project-reference source redirection is outside the
        // Rust loader (`Program::load_with_source_of_project_reference`).
        Err(error)
            if references
                && format!("{error:?}").contains("project-reference source redirection") =>
        {
            Err(format!("{EXCLUDED_REDIRECTION}: {error:?}").into())
        }
        Err(error) => Err(format!("load: {error:?}").into()),
    }
}

fn new_checker(program: &ProgramState) -> Result<Arc<CheckerOwner>, Error> {
    let counters = Counters::new();
    let generation = Generation::new(&counters);
    Ok(Arc::new(CheckerOwner::for_program(
        CheckerIdentity::new(generation, &counters),
        &counters,
        Arc::new(ProgramCheckerHost::new(program.program.clone())),
    )?))
}

fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    payload
        .downcast_ref::<String>()
        .map(String::as_str)
        .or_else(|| payload.downcast_ref::<&str>().copied())
        .unwrap_or("non-string panic payload")
        .to_string()
}

struct Driver {
    cache: FileCache,
    counters: Counters,
    /// The previous test's programs, kept alive so the cache reuses the
    /// default library files while the next test loads.
    keep: Vec<Rc<ProgramState>>,
}

impl Driver {
    fn checker_event(&mut self, test: &mut Test, event: &Value) -> Result<(), Error> {
        let token = event["checker"]
            .as_str()
            .ok_or("checker token")?
            .to_string();
        let program = event["program"]
            .as_str()
            .ok_or("program token")?
            .to_string();
        if !test.loaded.contains_key(&program) {
            let description = test
                .programs
                .get(&program)
                .ok_or_else(|| format!("unknown program {program}"))?
                .clone();
            if let Some(reason) = exclusion(&description) {
                test.report
                    .programs
                    .insert(program.clone(), json!({"excluded": reason}));
                test.excluded.insert(program.clone(), reason);
            }
        }
        if let Some(reason) = test.excluded.get(&program) {
            test.checkers.insert(
                token,
                Checker {
                    program: program.clone(),
                    owner: None,
                    replay: None,
                    dead: None,
                    excluded: Some(reason.clone()),
                    calls: 0,
                },
            );
            return Ok(());
        }
        if !test.loaded.contains_key(&program) {
            let description = test.programs[&program].clone();
            let loaded = catch_unwind(AssertUnwindSafe(|| {
                load_program(test, &description, &mut self.cache, &self.counters)
            }))
            .map_err(|payload| format!("panic: {}", panic_message(payload.as_ref())))
            .and_then(|result| result.map_err(|error| error.to_string()))
            .map(Rc::new);
            let status = match &loaded {
                Ok(state) => state.status(),
                Err(error) if error.starts_with(EXCLUDED_REDIRECTION) => {
                    json!({"excluded": EXCLUDED_REDIRECTION, "detail": error})
                }
                Err(error) => json!({"error": error}),
            };
            test.report.programs.insert(program.clone(), status);
            if let Err(error) = &loaded {
                if error.starts_with(EXCLUDED_REDIRECTION) {
                    test.excluded
                        .insert(program.clone(), EXCLUDED_REDIRECTION.to_string());
                }
            }
            test.loaded.insert(program.clone(), loaded);
            if let Some(reason) = test.excluded.get(&program) {
                test.checkers.insert(
                    token,
                    Checker {
                        program: program.clone(),
                        owner: None,
                        replay: None,
                        dead: None,
                        excluded: Some(reason.clone()),
                        calls: 0,
                    },
                );
                return Ok(());
            }
        }
        let mut checker = Checker {
            program: program.clone(),
            owner: None,
            replay: None,
            dead: None,
            excluded: None,
            calls: 0,
        };
        match &test.loaded[&program] {
            Ok(state) => match catch_unwind(AssertUnwindSafe(|| new_checker(state))) {
                Ok(Ok(owner)) => {
                    checker.owner = Some(owner);
                    checker.replay = Some(Replay::new(state.clone()));
                }
                Ok(Err(error)) => checker.dead = Some(format!("checker: {error}")),
                Err(payload) => {
                    checker.dead = Some(format!(
                        "checker panic: {}",
                        panic_message(payload.as_ref())
                    ));
                }
            },
            Err(error) => checker.dead = Some(format!("program: {error}")),
        }
        test.checkers.insert(token, checker);
        Ok(())
    }

    fn call_event(test: &mut Test, event: &Value) -> Result<(), Error> {
        let token = event["checker"].as_str().ok_or("call without checker")?;
        let op = event["op"].as_str().ok_or("call without op")?;
        let checker = test
            .checkers
            .get_mut(token)
            .ok_or_else(|| format!("unknown checker {token}"))?;
        let index = checker.calls;
        checker.calls += 1;
        if let Some(defs) = event.get("defs").and_then(Value::as_object) {
            if let Some(replay) = checker.replay.as_mut() {
                replay.add_defs(defs);
            }
        }
        let outcome = match (&checker.dead, &checker.owner, checker.replay.as_mut()) {
            _ if checker.excluded.is_some() => {
                Outcome::Excluded(checker.excluded.clone().unwrap_or_default())
            }
            (None, Some(owner), Some(replay)) => {
                let result = catch_unwind(AssertUnwindSafe(|| -> Outcome {
                    let mut operation = match owner.operation() {
                        Ok(operation) => operation,
                        Err(error) => return Outcome::Failed(format!("operation: {error}")),
                    };
                    ops::replay(replay, &mut operation, op, event)
                }));
                match result {
                    Ok(outcome) => outcome,
                    Err(payload) => {
                        let message = panic_message(payload.as_ref());
                        checker.dead = Some(format!("{op} panicked: {message}"));
                        Outcome::Failed(format!("panic: {message}"))
                    }
                }
            }
            (Some(reason), _, _) => Outcome::Skipped(reason.clone()),
            _ => Outcome::Skipped("no checker".to_string()),
        };
        let program = checker.program.clone();
        test.report.count(op, &outcome, || {
            json!({
                "checker": token,
                "program": program,
                "index": index,
                "outcome": outcome.name(),
                "reason": outcome.reason(),
                "args": event["args"],
                "results": event.get("results"),
            })
        });
        Ok(())
    }

    fn finish(&mut self, test: Test, out: &mut impl Write) -> Result<(), Error> {
        let mut operations = Map::new();
        for (op, counts) in &test.report.operations {
            operations.insert(op.clone(), json!(counts));
        }
        let line = json!({
            "test": test.name,
            "programs": test.report.programs,
            "operations": operations,
            "details": test.report.details,
            "exclusions": test.report.exclusions,
            "unsupported": test.report.unsupported,
        });
        serde_json::to_writer(&mut *out, &line)?;
        out.write_all(b"\n")?;
        self.keep = test.loaded.into_values().filter_map(Result::ok).collect();
        self.cache.prune();
        Ok(())
    }
}

#[allow(
    dead_code,
    reason = "the example's entry point; the contract test calls `run`"
)]
pub fn main() -> Result<(), Error> {
    run(&options()?)
}

/// Replays the tests of `input` (this shard's, or the named ones) and writes
/// one report line per test to `output`.
#[allow(
    dead_code,
    reason = "the contract test's entry point; the example calls `main`"
)]
pub fn replay_file(input: &str, output: &str, tests: &[String]) -> Result<(), Error> {
    run(&Options {
        input: input.to_string(),
        output: output.to_string(),
        shard: 0,
        shards: 1,
        tests: tests.to_vec(),
    })
}

fn run(options: &Options) -> Result<(), Error> {
    let input = BufReader::with_capacity(1 << 20, std::fs::File::open(&options.input)?);
    let mut out = BufWriter::new(std::fs::File::create(&options.output)?);
    let mut driver = Driver {
        cache: FileCache::new(),
        counters: Counters::new(),
        keep: Vec::new(),
    };
    let mut test: Option<Test> = None;
    let mut ordinal = 0usize;
    for line in input.split(b'\n') {
        let line = line?;
        if line.is_empty() {
            continue;
        }
        if line.starts_with(br#"{"e":"test""#) {
            if let Some(done) = test.take() {
                driver.finish(done, &mut out)?;
            }
            let event: Value = serde_json::from_slice(&line)?;
            let name = event["name"].as_str().unwrap_or_default().to_string();
            let selected = if options.tests.is_empty() {
                ordinal % options.shards == options.shard
            } else {
                options.tests.contains(&name)
            };
            ordinal += 1;
            if !selected {
                continue;
            }
            let mut files = BTreeMap::new();
            for (file, text) in event["files"].as_object().into_iter().flatten() {
                files.insert(file.clone(), hex(text.as_str().unwrap_or_default())?);
            }
            let symlinks = event["symlinks"]
                .as_object()
                .into_iter()
                .flatten()
                .map(|(name, target)| {
                    (
                        name.clone(),
                        target.as_str().unwrap_or_default().to_string(),
                    )
                })
                .collect();
            test = Some(Test {
                name,
                files,
                symlinks,
                blobs: HashMap::new(),
                programs: HashMap::new(),
                loaded: HashMap::new(),
                excluded: HashMap::new(),
                checkers: HashMap::new(),
                report: Report::default(),
            });
            continue;
        }
        let Some(current) = test.as_mut() else {
            continue;
        };
        if line.starts_with(br#"{"e":"end""#) {
            let done = test.take().expect("current test");
            driver.finish(done, &mut out)?;
            continue;
        }
        let event: Value = serde_json::from_slice(&line)?;
        match event["e"].as_str() {
            Some("blob") => {
                let sha = event["sha256"].as_str().ok_or("blob digest")?.to_string();
                current
                    .blobs
                    .insert(sha, hex(event["text"].as_str().unwrap_or_default())?);
            }
            Some("program") => {
                let token = event["program"]
                    .as_str()
                    .ok_or("program token")?
                    .to_string();
                current.programs.insert(token, event);
            }
            Some("checker") => driver.checker_event(current, &event)?,
            Some("call") => Driver::call_event(current, &event)?,
            other => return Err(format!("unknown event {other:?}").into()),
        }
    }
    if let Some(done) = test.take() {
        driver.finish(done, &mut out)?;
    }
    out.flush()?;
    Ok(())
}
