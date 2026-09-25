//! `projectReferences` group. Each request is a small in-memory workspace. Its
//! tsconfig is parsed for real by `tsr_tsoptions` through the compiler's own
//! config host, and the program is loaded by the production `Program` loader,
//! which reads every referenced config itself. The actions observe the same
//! program as the Go probe's four actions: the loader's closure (`load`), the
//! reference walk (`graph`), the verifier's writes (`verify`) and include
//! explanations built by `Program::explain_file_include` (`explain`).
//!
//! The observation is the actions' ordered answers, one flat object each.
//! Diagnostics travel as the probe's positional arrays, [file, pos, end, code,
//! category, key, args, text, chain, related].

use std::sync::Arc;

use serde_json::{json, Value};
use tsr_compiler::{CompilerConfigHost, FileCache, Program, ProgramOptions};
use tsr_core::{CompilerOptions, Tristate};
use tsr_jsstring::JsString;
use tsr_tsoptions::ConfigValue;
use tsr_vfs::MemoryBuilder;

use crate::api::{action_op, actions, subject, Outcome};
use crate::observation;

const SUBJECT: &str = "projectReferences";

pub fn observe(request: &Value) -> Option<Outcome> {
    (subject(request) == SUBJECT).then(|| match run(request) {
        Ok(value) => Outcome::Observed(value),
        Err(Stop::Missing(operation)) => Outcome::missing(
            operation,
            "tsc/internal/compiler/program.go:NewProgram",
            "Program::load_with_source_of_project_reference(options, use_source, cache, counters)",
            "crates/tsr_compiler/src/loader.rs",
        ),
        Err(Stop::Failed(error)) => Outcome::Failed(error),
    })
}

/// A named loader boundary is a missing operation, never a harness failure.
enum Stop {
    Missing(&'static str),
    Failed(String),
}

impl From<String> for Stop {
    fn from(error: String) -> Self {
        Stop::Failed(error)
    }
}

impl From<&str> for Stop {
    fn from(error: &str) -> Self {
        Stop::Failed(error.to_owned())
    }
}

fn text(value: &[u8]) -> &str {
    std::str::from_utf8(value).expect("fixture output is valid UTF-8")
}

fn strings(values: &[JsString]) -> Value {
    values
        .iter()
        .map(|value| json!(text(value.as_bytes())))
        .collect()
}

/// The loader bridge's named diagnostic as the probe's positional array.
fn positional(d: &Value) -> Value {
    let list = |key: &str| match &d[key] {
        Value::Array(items) => Value::Array(items.iter().map(positional).collect()),
        _ => json!([]),
    };
    let args = match &d["Args"] {
        Value::Array(items) => Value::Array(items.clone()),
        _ => json!([]),
    };
    json!([
        d["File"],
        d["Pos"],
        d["End"],
        d["Code"],
        d["Category"],
        d["Key"],
        args,
        d["Text"],
        list("Chain"),
        list("Related")
    ])
}

fn diagnostics<'a>(
    list: impl IntoIterator<Item = &'a tsr_ast::Diagnostic>,
    program: &Program,
) -> Value {
    list.into_iter()
        .map(|d| positional(&observation::diagnostic(d, program)))
        .collect()
}

struct Workspace {
    host: Arc<dyn tsr_vfs::FileSystem>,
    cwd: JsString,
}

fn workspace(spec: &Value) -> Result<Workspace, String> {
    let cwd = spec["cwd"].as_str().ok_or("workspace has no cwd")?;
    let mut fs = MemoryBuilder::new(
        cwd.as_bytes(),
        spec["case_sensitive"]
            .as_bool()
            .ok_or("workspace has no case_sensitive")?,
    );
    for (name, value) in spec["files"].as_object().ok_or("workspace has no files")? {
        let text = value.as_str().ok_or("file text is not a string")?;
        fs.insert_physical(name.as_bytes(), text.as_bytes().to_vec());
    }
    if let Some(links) = spec["symlinks"].as_object() {
        for (name, target) in links {
            let target = target.as_str().ok_or("symlink target is not a string")?;
            fs.insert_symlink(name.as_bytes(), target.as_bytes());
        }
    }
    Ok(Workspace {
        host: Arc::new(tsr_bundled::BundledFs::new(Arc::new(fs.finish()))),
        cwd: JsString::from_bytes(cwd.as_bytes()),
    })
}

fn load(request: &Value) -> Result<Program, Stop> {
    let spec = request.get("workspace").ok_or("request has no workspace")?;
    let workspace = workspace(spec)?;
    let config_host = CompilerConfigHost::new(workspace.host.clone(), workspace.cwd.clone());
    let project = spec["project"].as_str().ok_or("workspace has no project")?;
    let read = tsr_tsoptions::get_parsed_command_line_of_config_file(
        project.as_bytes(),
        &CompilerOptions::default(),
        &ConfigValue::Null,
        &config_host,
    )
    .map_err(|e| format!("config read failed: {e:?}"))?;
    let mut config = read
        .command_line
        .ok_or_else(|| format!("config {project} was not read"))?;
    if spec["suppress_output_path_check"].as_bool() == Some(true) {
        // Not a tsconfig option: the request sets it on the parsed options.
        config.options.suppress_output_path_check = Tristate::TRUE;
    }
    Program::load_with_source_of_project_reference(
        ProgramOptions {
            config,
            host: workspace.host,
            current_directory: workspace.cwd,
            default_library_path: JsString::from_bytes(tsr_bundled::LIB_PATH),
            skip_module_resolution: false,
        },
        spec["use_source_of_project_reference"].as_bool() == Some(true),
        &mut FileCache::new(),
        &tsr_arena::Counters::new(),
    )
    .map_err(|e| match e {
        tsr_compiler::Error::Unsupported(operation) => Stop::Missing(operation),
        e => Stop::Failed(format!("program load failed: {e:?}")),
    })
}

fn load_action(program: &Program) -> Result<Value, String> {
    let config = program.config();
    let mut files = Vec::new();
    for file in program.files() {
        let view = file.bound().view();
        let source = view.source_file().map_err(|e| format!("{e:?}"))?;
        let options = source.parse_options();
        let (name, path) = (options.file_name.as_bytes(), options.path.as_bytes());
        let meta = program
            .metadata(path)
            .ok_or("program file without metadata")?;
        files.push(json!([
            text(name),
            text(path),
            program.is_lib(path),
            meta.implied_node_format.0,
            text(meta.package_json_type.as_bytes()),
            text(meta.package_json_directory.as_bytes()),
            source.external_module_indicator.is_some(),
            text(program.source_of_project_reference_if_output_included(path, name)),
            program
                .redirect_for_resolution(path, name)
                .map(|config| text(config.config_name().as_bytes()).to_owned()),
        ]));
    }
    let resolutions: Vec<Value> = program
        .resolutions()
        .iter()
        .map(|r| {
            let result = &r.result;
            json!([
                text(r.file.as_bytes()),
                text(r.name.as_bytes()),
                r.mode.0,
                text(result.resolved_file_name.as_bytes()),
                text(result.original_path.as_bytes()),
                text(result.extension.as_bytes()),
                result.is_external_library_import,
                text(result.package_id.text().as_bytes()),
            ])
        })
        .collect();
    let type_resolutions: Vec<Value> = program
        .type_resolutions()
        .iter()
        .map(|r| {
            let result = &r.result;
            json!([
                text(r.file.as_bytes()),
                text(r.name.as_bytes()),
                r.mode.0,
                text(result.resolved_file_name.as_bytes()),
                result.primary,
                result.is_external_library_import,
            ])
        })
        .collect();
    let trace: Vec<Value> = program
        .trace()
        .iter()
        .map(|entry| {
            let args: Vec<_> = entry
                .args
                .iter()
                .map(|arg| match arg {
                    tsr_module::TraceArg::Text(value) => text(value.as_bytes()).to_owned(),
                    tsr_module::TraceArg::Bool(value) => value.to_string(),
                })
                .collect();
            json!(format!("{}:[{}]", entry.message.code, args.join(" ")))
        })
        .collect();
    Ok(json!({
        "root_files": strings(&config.root_file_names),
        "reference_paths": strings(config.resolved_project_reference_paths()),
        "config_errors": diagnostics(&config.config_file_parsing_diagnostics(), program),
        "files": files,
        "missing": strings(program.missing_files()),
        "resolutions": resolutions,
        "type_resolutions": type_resolutions,
        "include_diagnostics": diagnostics(
            program
                .include_diagnostics()
                .map_err(|e| format!("include diagnostics failed: {e:?}"))?,
            program
        ),
        "trace": trace,
    }))
}

fn graph_action(program: &Program) -> Value {
    let mut walk = Vec::new();
    let result = program.range_resolved_project_reference(|path, config, parent, index| {
        walk.push(json!([
            text(path),
            config.map(|config| text(config.config_name().as_bytes()).to_owned()),
            text(parent.config_name().as_bytes()),
            index,
        ]));
        true
    });
    json!({"walk": walk, "result": result})
}

fn verify_action(program: &Program) -> Result<Value, String> {
    let verification = program.option_verification();
    let blocked: Vec<Value> = verification
        .blocked_output_paths
        .iter()
        .map(|path| json!(text(path.as_bytes())))
        .collect();
    let program_diagnostics = program
        .program_diagnostics()
        .map_err(|e| format!("program diagnostics failed: {e:?}"))?;
    // The verifier's file-bound include requests are in the per-file buckets.
    let mut files = Vec::new();
    for file in program.files() {
        let source = file
            .bound()
            .view()
            .source_file()
            .map_err(|e| format!("{e:?}"))?;
        let options = source.parse_options();
        if program.is_lib(options.path.as_bytes()) {
            continue;
        }
        let bucket = program
            .include_diagnostics_for_file(options.path.as_bytes())
            .map_err(|e| format!("include diagnostics failed: {e:?}"))?;
        files.push(json!([
            text(options.file_name.as_bytes()),
            diagnostics(bucket, program)
        ]));
    }
    Ok(json!({
        "raw": diagnostics(&verification.diagnostics, program),
        "blocked": blocked,
        "program": diagnostics(program_diagnostics, program),
        "files": files,
    }))
}

fn explain_action(program: &Program, request: &Value) -> Result<Value, String> {
    let names = request["explain_files"]
        .as_array()
        .ok_or("explain requires explain_files")?;
    let mut result = Vec::new();
    for name in names {
        let name = name.as_str().ok_or("explain file is not a string")?;
        let path = tsr_tspath::to_path(
            name.as_bytes(),
            program.current_directory(),
            program.host().use_case_sensitive_file_names(),
        );
        let diagnostic = program
            .explain_file_include(
                path.as_bytes(),
                tsr_compiler::messages::File_0_not_found,
                vec![JsString::from_bytes(name.as_bytes())],
            )
            .map_err(|e| format!("explain {name} failed: {e:?}"))?;
        result.push(json!([
            name,
            positional(&observation::diagnostic(&diagnostic, program))
        ]));
    }
    Ok(json!({ "explanations": result }))
}

fn run(request: &Value) -> Result<Value, Stop> {
    let program = load(request)?;
    let mut observed = Vec::new();
    for action in actions(request) {
        let op = action_op(action);
        let value = match op {
            "load" => load_action(&program)?,
            "graph" => graph_action(&program),
            "verify" => verify_action(&program)?,
            "explain" => explain_action(&program, request)?,
            other => return Err(format!("unknown projectReferences action {other:?}").into()),
        };
        let mut value = value;
        value["op"] = json!(op);
        observed.push(value);
    }
    Ok(json!({ "ordered": observed }))
}
