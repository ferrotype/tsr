//! The declaration re-compilation of the `.js` baseline
//! (`compileDeclarationFiles` → `harnessutil.CompileFilesEx`): a program over
//! the prepared declaration files with the first compilation's options,
//! harness settings, current directory, symlinks, configuration file and
//! content mappers (not its option errors), and the harness's diagnostics
//! of it (`compileFilesWithHost`).
//!
//! The first compilation's loading request (`scripts/phase3_corpus.py`'s
//! request builder) supplies what is shared; the declaration program replaces
//! its files and roots. The pin's post-emit program would emit before its
//! diagnostics are collected; a declaration file has no JavaScript output,
//! so no transform reaches the checker first and the pre-emit collection is
//! the post-emit one.
use super::baselines::{DeclarationCompilationContext, Failure};
use serde_json::{json, Value};
use std::sync::Arc;
use tsr_compiler as ts_compiler_error;
use tsr_compiler::{CheckedProgram, FileCache, Program, ProgramOptions};

// The witness also loads these through the S08 executor, whose modules are
// private to it.
#[path = "../../s08/p4/config.rs"]
#[allow(clippy::duplicate_mod)]
mod config;
#[path = "../../s07/program/rust_observation.rs"]
#[allow(dead_code, clippy::duplicate_mod)]
mod observation;

/// The pin's `testLibFolder`.
const TEST_LIB_FOLDER: &[u8] = b"/.lib";

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut text = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(text, "{byte:02x}");
    }
    text
}

fn utf8(bytes: &[u8], what: &str) -> Result<String, Failure> {
    String::from_utf8(bytes.to_vec())
        .map_err(|_| Failure::Input(format!("{what} is not UTF-8 and cannot be a JSON key")))
}

/// The declaration program's loading request: the first request with the
/// files and roots `CompileFilesEx` derives from the declaration files.
// source: tsc/internal/testutil/harnessutil/harnessutil.go:CompileFilesEx
pub fn loading_request(
    first: &Value,
    context: &DeclarationCompilationContext<'_>,
    harness_options: &Value,
) -> Result<Value, Failure> {
    let current_directory = context.current_directory;
    let options = first
        .get("options")
        .ok_or_else(|| Failure::Input("loading request without options".into()))?;
    let mut program_file_names = Vec::new();
    for file in &context.decl_input_files {
        let file_name = tsr_tspath::absolute(file.unit_name, current_directory);
        if !tsr_tspath::file_extension_is(&file_name, tsr_tspath::EXTENSION_JSON)
            && !tsr_tspath::file_extension_is(&file_name, b".tsbuildinfo")
        {
            program_file_names.push(json!(utf8(&file_name, "a declaration root")?));
        }
    }
    let mut include_lib_dir = context
        .decl_input_files
        .iter()
        .any(|file| file.content.windows(6).any(|window| window == b"/.lib/"));
    if let Some(lib_files) = harness_options["LibFiles"].as_array() {
        for lib_file in lib_files {
            let lib_file = lib_file
                .as_str()
                .ok_or_else(|| Failure::Input("malformed LibFiles".into()))?;
            if lib_file == "lib.d.ts" && options["noLib"] != true {
                continue;
            }
            program_file_names.push(json!(utf8(
                &tsr_tspath::combine(TEST_LIB_FOLDER, &[lib_file.as_bytes()]),
                "a library root"
            )?));
            include_lib_dir = true;
        }
    }

    let mut files = serde_json::Map::new();
    for file in context
        .decl_input_files
        .iter()
        .chain(&context.decl_other_files)
    {
        let file_name = tsr_tspath::absolute(file.unit_name, current_directory);
        files.insert(
            utf8(&file_name, "a declaration file name")?,
            json!(hex(file.content)),
        );
    }
    if include_lib_dir {
        let first_files = first["files"]
            .as_object()
            .ok_or_else(|| Failure::Input("loading request without files".into()))?;
        let mut found = false;
        for (name, content) in first_files {
            if name.starts_with("/.lib/") {
                files.insert(name.clone(), content.clone());
                found = true;
            }
        }
        if !found {
            return Err(Failure::Input(
                "the declaration program needs the test library folder, which the first request does not carry"
                    .into(),
            ));
        }
    }
    let mut request = first.clone();
    request["files"] = Value::Object(files);
    request["roots"] = Value::Array(program_file_names);
    Ok(request)
}

/// The declaration program, loaded and checked, and its harness diagnostics
/// sorted and deduplicated.
pub struct Compiled {
    pub checked: CheckedProgram,
    pub diagnostics: Vec<tsr_ast::Diagnostic>,
}

/// Why the declaration program was not compiled: a harness defect, or the
/// production loader's or checker's error (a refusal stays nameable).
pub enum CompileError {
    Harness(Failure),
    Load(ts_compiler_error::Error),
    Diagnostics(ts_compiler_error::Error),
}

/// `request` is the first compilation's request (`loading`, `error_inputs`,
/// `mode`); `loading` the declaration program's [`loading_request`].
pub fn compile(
    request: &Value,
    loading: &Value,
    capture_suggestions: bool,
    content_mapper_project: Option<Arc<dyn tsr_contentmapper::Project>>,
    cache: &mut FileCache,
) -> Result<Compiled, Failure> {
    compile_checked(
        request,
        loading,
        capture_suggestions,
        content_mapper_project,
        cache,
    )
    .map_err(|error| match error {
        CompileError::Harness(failure) => failure,
        CompileError::Load(error) => Failure::Input(format!("declaration program load: {error:?}")),
        CompileError::Diagnostics(error) => {
            Failure::Input(format!("declaration program diagnostics: {error:?}"))
        }
    })
}

/// [`compile`] with the production error kept.
pub fn compile_checked(
    request: &Value,
    loading: &Value,
    capture_suggestions: bool,
    content_mapper_project: Option<Arc<dyn tsr_contentmapper::Project>>,
    cache: &mut FileCache,
) -> Result<Compiled, CompileError> {
    let parsed = config::parse(request)
        .map_err(|error| CompileError::Harness(Failure::Input(format!("config parse: {error}"))))?;
    let mut options: ProgramOptions = observation::program_options(loading, parsed);
    // compileDeclarationFiles passes the configuration file and its content
    // mappers, not the first parse's option errors.
    options.config.errors = Vec::new();
    options.single_threaded = if request["mode"] == "single" {
        tsr_core::Tristate::TRUE
    } else {
        tsr_core::Tristate::UNKNOWN
    };
    let counters = tsr_arena::Counters::new();
    let program = Program::load_with_content_mapper_project(
        options,
        content_mapper_project,
        cache,
        &counters,
    )
    .map_err(CompileError::Load)?;
    let checked = CheckedProgram::new(Arc::new(program), &counters, None);
    let diagnostics =
        harness_diagnostics(&checked, capture_suggestions).map_err(CompileError::Diagnostics)?;
    Ok(Compiled {
        checked,
        diagnostics,
    })
}

/// The post-emit collection of `compileFilesWithHost`, sorted and
/// deduplicated; the first compilation's pre- and post-emit programs are
/// collected the same way.
// source: tsc/internal/testutil/harnessutil/harnessutil.go:compileFilesWithHost
pub fn harness_diagnostics(
    checked: &CheckedProgram,
    capture_suggestions: bool,
) -> Result<Vec<tsr_ast::Diagnostic>, ts_compiler_error::Error> {
    let program = checked.program();
    let request = tsr_checker::CheckerRequest::default();
    let mut values = program.config_file_parsing_diagnostics();
    values.extend_from_slice(program.program_diagnostics()?);
    values.extend(program.syntactic_diagnostics(None)?);
    values.extend(checked.semantic_diagnostics(&request, None)?);
    values.extend(checked.global_diagnostics()?);
    if program.options().emit_declarations() {
        values.extend(checked.declaration_diagnostics(&request, None)?);
    }
    if capture_suggestions {
        values.extend(checked.suggestion_diagnostics(&request, None)?);
    }
    program.sort_and_deduplicate_diagnostics(&values)
}
