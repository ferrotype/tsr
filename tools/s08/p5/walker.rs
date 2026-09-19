//! Focused walker adapter. This reads inputs, never expected queries or bytes.
use crate::baseline::{self, InputFile};
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;
use tsr_arena::{CheckerIdentity, Counters, Generation};
use tsr_checker::CheckerOwner;
use tsr_compiler::{FileCache, Program, ProgramCheckerHost, ProgramOptions};
use tsr_core::{CompilerOptions, ModuleKind, ScriptTarget, Tristate};
use tsr_jsstring::JsString;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    version: u32,
    scope: String,
    cases: Vec<Case>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Case {
    id: String,
    files: Vec<File>,
    roots: Vec<String>,
    header: String,
    had_errors: bool,
    enabled: bool,
    allow_js: bool,
    #[serde(default)]
    no_check: bool,
    #[serde(default)]
    skip_lib_check: bool,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    name: String,
    content: String,
}

fn run(case: &Case, trace: &mut baseline::Trace) -> Result<Value> {
    if !case.enabled {
        return Ok(
            json!({"state":"executed","types":{"state":"disabled"},"symbols":{"state":"disabled"},"queries":[]}),
        );
    }
    let counters = Counters::new();
    let mut fs = tsr_vfs::MemoryBuilder::new(b"/", true);
    for file in &case.files {
        fs.insert_loaded(file.name.as_bytes(), file.content.as_bytes());
    }
    let program = Arc::new(Program::load(
        ProgramOptions {
            config: tsr_tsoptions::ParsedCommandLine::new(
                CompilerOptions {
                    target: ScriptTarget::ESNEXT,
                    module: ModuleKind::ESNEXT,
                    strict: Tristate::TRUE,
                    no_lib: Tristate::TRUE,
                    no_check: if case.no_check {
                        Tristate::TRUE
                    } else {
                        Tristate::FALSE
                    },
                    skip_lib_check: if case.skip_lib_check {
                        Tristate::TRUE
                    } else {
                        Tristate::FALSE
                    },
                    allow_js: if case.allow_js {
                        Tristate::TRUE
                    } else {
                        Tristate::FALSE
                    },
                    ..Default::default()
                },
                case.roots
                    .iter()
                    .map(|s| JsString::from_bytes(s.as_bytes()))
                    .collect(),
            ),
            host: Arc::new(fs.finish()),
            current_directory: JsString::from_bytes(b"/".as_slice()),
            default_library_path: JsString::from_bytes(b"/no-default-lib".as_slice()),
            skip_module_resolution: false,
        },
        &mut FileCache::new(),
        &counters,
    )?);
    let owner = Arc::new(CheckerOwner::for_program(
        CheckerIdentity::new(Generation::new(&counters), &counters),
        &counters,
        Arc::new(ProgramCheckerHost::new(program.clone())),
    )?);
    let mut op = owner.operation()?;
    for file in program.files() {
        program.semantic_diagnostics_with_checker(&mut op, file)?;
    }
    op.global_diagnostics()?;
    let files: Vec<_> = case
        .files
        .iter()
        .map(|f| InputFile {
            name: f.name.as_bytes(),
            content: f.content.as_bytes(),
        })
        .collect();
    Ok(baseline::generate(
        &program,
        &mut op,
        &files,
        case.header.as_bytes(),
        case.had_errors,
        trace,
    ))
}

pub fn observe(request: &Value) -> Result<Value> {
    let request: Request = serde_json::from_value(request.clone())?;
    if request.version != 1 || request.scope != "native-walker-focused" {
        return Err("unknown walker contract".into());
    }
    let mut cases = Vec::new();
    for case in &request.cases {
        let mut trace = baseline::Trace::default();
        let result =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run(case, &mut trace)));
        let mut row = match result {
            Ok(Ok(row)) => row,
            Ok(Err(error)) => json!({"state":"failed","stage":"setup","reason":error.to_string()}),
            Err(payload) => {
                json!({"state":"failed","stage":"panic","reason":payload.downcast_ref::<String>().map(String::as_str).or_else(||payload.downcast_ref::<&str>().copied()).unwrap_or("non-string panic payload")})
            }
        };
        if row["state"] == "failed" {
            row["queries"] = json!(trace.queries);
            row["active_query"] = trace.active;
        }
        row["id"] = json!(case.id);
        cases.push(row);
    }
    Ok(json!({"version":1,"cases":cases}))
}
