//! `tsr-testrunner`: the suite runner `scripts/parity.py` drives.
//!
//! ```text
//! tsr-testrunner --suite compiler|transpile [--mode single|concurrent] [--root DIR] list
//! tsr-testrunner --suite compiler|transpile [--mode single|concurrent] [--root DIR] run --id ID --local DIR
//! ```
//!
//! `--suite compiler` is the pin's `TestCompilerBaselines`: both the
//! `compiler` and `conformance` test directories. `list` prints every
//! variant id the pin's runner would run (its skip list and
//! `SkipUnsupportedCompilerOptions` applied), one per line. `run` runs one
//! variant and prints one result line per sub-test (`tsr_testrunner::result`);
//! differing baselines are written under `--local`. The root defaults to
//! the current directory and must hold `upstream/tsc/testdata`.
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use tsr_testrunner::baseline::Roots;
use tsr_testrunner::enumerate::{self, CompilerTestType, Variant};
use tsr_testrunner::result::{Outcome, Report};
use tsr_testrunner::{compiler_runner, transpile_runner, Mode, Stop, TestData};

struct Arguments {
    suite: String,
    mode: Mode,
    root: PathBuf,
    command: Command,
}

enum Command {
    List,
    Run { id: String, local: PathBuf },
}

fn usage(message: &str) -> ! {
    eprintln!("tsr-testrunner: {message}");
    eprintln!("usage: tsr-testrunner --suite compiler|transpile [--mode single|concurrent] [--root DIR] (list | run --id ID --local DIR)");
    std::process::exit(2)
}

fn parse_arguments() -> Arguments {
    let mut suite = None;
    let mut mode = Mode::Single;
    let mut root = std::env::current_dir()
        .unwrap_or_else(|error| usage(&format!("current directory: {error}")));
    let mut command = None;
    let mut id = None;
    let mut local = None;
    let mut arguments = std::env::args().skip(1);
    while let Some(argument) = arguments.next() {
        let mut value = |name: &str| {
            arguments
                .next()
                .unwrap_or_else(|| usage(&format!("{name} needs a value")))
        };
        match argument.as_str() {
            "--suite" => suite = Some(value("--suite")),
            "--mode" => {
                mode = match value("--mode").as_str() {
                    "single" => Mode::Single,
                    "concurrent" => Mode::Concurrent,
                    other => usage(&format!("unknown mode {other}")),
                }
            }
            "--root" => root = PathBuf::from(value("--root")),
            "--id" => id = Some(value("--id")),
            "--local" => local = Some(PathBuf::from(value("--local"))),
            "list" => command = Some("list"),
            "run" => command = Some("run"),
            other => usage(&format!("unknown argument {other}")),
        }
    }
    let suite = suite.unwrap_or_else(|| usage("--suite is required"));
    if !matches!(suite.as_str(), "compiler" | "transpile") {
        usage(&format!("unknown suite {suite}"));
    }
    let command = match command {
        Some("list") => Command::List,
        Some("run") => Command::Run {
            id: id.unwrap_or_else(|| usage("run needs --id")),
            local: local.unwrap_or_else(|| usage("run needs --local")),
        },
        _ => usage("list or run is required"),
    };
    Arguments {
        suite,
        mode,
        root,
        command,
    }
}

/// Every variant of the suite the pin's runner runs, with the test files
/// read once; skipped variants are left out, fatal ones kept (they fail).
fn list(suite: &str, testdata: &TestData) -> Result<Vec<Variant>, Stop> {
    let mut variants = Vec::new();
    if suite == "transpile" {
        for file in transpile_runner::transpile_test_files(testdata)? {
            let content = read(&file)?;
            variants.extend(transpile_runner::transpile_variants(&file, &content)?);
        }
        return Ok(variants);
    }
    for kind in [CompilerTestType::Regression, CompilerTestType::Conformance] {
        for file in enumerate::compiler_test_files(testdata, kind)? {
            let content = read(&file)?;
            for variant in enumerate::compiler_variants(&file, kind, &content)? {
                match prepare(variant.clone(), &content) {
                    Ok(_) | Err(Stop::Fatal(_)) => variants.push(variant),
                    Err(Stop::Skip(_)) => {}
                }
            }
        }
    }
    Ok(variants)
}

/// A test file's text as the pin's runners read it (a byte order mark
/// hides a first-line directive otherwise).
fn read(file: &Path) -> Result<Vec<u8>, Stop> {
    enumerate::read_test_file(file)
}

/// `newCompilerTest` up to the compilation; a panic there is the pin's
/// `RecoverAndFail`: the variant fails instead of the process.
fn prepare(variant: Variant, content: &[u8]) -> Result<compiler_runner::Prepared, Stop> {
    let file = variant.file.display().to_string();
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        compiler_runner::prepare(variant, content)
    })) {
        Ok(prepared) => prepared,
        Err(payload) => Err(Stop::fatal(format!(
            "Panic on compiler test {file}:\n{}",
            compiler_runner::panic_message(payload.as_ref())
        ))),
    }
}

fn run(arguments: &Arguments, id: &str, local: &Path, testdata: &TestData) -> Result<Report, Stop> {
    let variant = enumerate::find_variant(testdata, id)?
        .ok_or_else(|| Stop::fatal(format!("no variant {id} in suite {}", arguments.suite)))?;
    let content = read(&variant.file)?;
    let roots = Roots::new(testdata.reference(), local.to_path_buf());
    let mut report = Report::default();
    if variant.suite == "transpile" {
        transpile_runner::run_transpile_test(&variant, &content, &roots, &mut report);
        return Ok(report);
    }
    match prepare(variant.clone(), &content) {
        Ok(prepared) => compiler_runner::run_single_config_test(
            prepared,
            testdata,
            &roots,
            arguments.mode,
            &mut report,
        ),
        Err(Stop::Skip(reason)) => report.subtest(&variant.id(), "test", Outcome::skip(reason)),
        Err(Stop::Fatal(reason)) => report.subtest(&variant.id(), "test", Outcome::fail(reason)),
    }
    Ok(report)
}

fn main() -> ExitCode {
    let arguments = parse_arguments();
    let testdata = TestData::in_repository(&arguments.root);
    if !testdata.path().is_dir() {
        usage(&format!(
            "{} is not a directory; pass --root",
            testdata.path().display()
        ));
    }
    let outcome = match &arguments.command {
        Command::List => list(&arguments.suite, &testdata).map(|variants| {
            let mut ids: Vec<String> = variants.iter().map(Variant::id).collect();
            ids.sort();
            ids.dedup();
            for id in ids {
                println!("{id}");
            }
        }),
        Command::Run { id, local } => run(&arguments, id, local, &testdata).and_then(|report| {
            report
                .write(&mut std::io::stdout().lock())
                .map_err(|error| Stop::fatal(format!("stdout: {error}")))
        }),
    };
    match outcome {
        Ok(()) => ExitCode::SUCCESS,
        Err(stop) => {
            eprintln!("tsr-testrunner: {stop}");
            ExitCode::FAILURE
        }
    }
}
