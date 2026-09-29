//! Test-only loader and renderer for the C3 native fixtures. The compiler
//! options come from the recorded native command, so the command is the one
//! authority for what the pinned `tsgo` checked.
#![allow(dead_code)]
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::sync::Arc;
use tsr_arena::{CheckerIdentity, Counters, Generation};
use tsr_checker::CheckerOwner;
use tsr_compiler::{FileCache, Program, ProgramCheckerHost, ProgramOptions};
use tsr_core::{CompilerOptions, ModuleKind, ScriptTarget, Tristate};
use tsr_jsstring::JsString;

pub struct Fixture {
    pub program: Arc<Program>,
    pub owner: Arc<CheckerOwner>,
    pub path: String,
    pub source_text: String,
    pub native: Value,
}

fn options_from_command(command: &[Value]) -> CompilerOptions {
    let mut options = CompilerOptions {
        target: ScriptTarget::ESNEXT,
        ..Default::default()
    };
    let args: Vec<&str> = command.iter().filter_map(Value::as_str).collect();
    let flags = &args[1..args.len() - 1];
    let mut i = 0;
    while i < flags.len() {
        let flag = flags[i];
        let value = flags.get(i + 1).copied().filter(|v| !v.starts_with("--"));
        let tristate = match value {
            Some("false") => Tristate::FALSE,
            _ => Tristate::TRUE,
        };
        match flag {
            "--strict" => options.strict = tristate,
            "--allowJs" => options.allow_js = tristate,
            "--checkJs" => options.check_js = tristate,
            "--skipLibCheck" => options.skip_lib_check = tristate,
            "--exactOptionalPropertyTypes" => options.exact_optional_property_types = tristate,
            "--noUncheckedIndexedAccess" => options.no_unchecked_indexed_access = tristate,
            "--useUnknownInCatchVariables" => options.use_unknown_in_catch_variables = tristate,
            "--verbatimModuleSyntax" => options.verbatim_module_syntax = tristate,
            "--isolatedModules" => options.isolated_modules = tristate,
            "--noFallthroughCasesInSwitch" => options.no_fallthrough_cases_in_switch = tristate,
            "--noLib" => options.no_lib = tristate,
            "--module" => {
                options.module = match value {
                    Some("commonjs") => ModuleKind::COMMON_JS,
                    Some("esnext") => ModuleKind::ESNEXT,
                    other => panic!("unsupported native module {other:?}"),
                }
            }
            "--target" => assert_eq!(value, Some("esnext")),
            "--noEmit" => options.no_emit = tristate,
            "--ignoreConfig" | "--pretty" => {}
            other => panic!("unsupported native flag {other}"),
        }
        i += 1 + usize::from(value.is_some());
    }
    options
}

pub fn load(name: &str, source_text: &str, native_json: &str) -> Fixture {
    let native: Value = serde_json::from_str(native_json).unwrap();
    let pin: Value = serde_json::from_str(include_str!("../../../../data/upstream.json")).unwrap();
    assert_eq!(native["pin"], pin["pin"], "{name}: fixture pin");
    assert_eq!(
        native["source_sha256"],
        format!("{:x}", Sha256::digest(source_text.as_bytes())),
        "{name}: fixture source digest"
    );
    // The recorder refuses unclean runs; the loader refuses a record whose
    // exit code disagrees with its diagnostics or that carries stderr.
    let count = native["diagnostics"].as_array().unwrap().len();
    let exit = native["exit_code"].as_i64().unwrap();
    assert!(
        (exit == 0 && count == 0) || (exit == 2 && count > 0),
        "{name}: exit code {exit} disagrees with {count} recorded diagnostics"
    );
    assert_eq!(
        native["native_stderr"].as_str().unwrap_or(""),
        "",
        "{name}: the native run wrote to stderr"
    );
    let path = format!("/{name}");
    let mut fs = tsr_vfs::MemoryBuilder::new(b"/", true);
    fs.insert_loaded(path.as_bytes(), source_text.as_bytes());
    let counters = Counters::new();
    let options = options_from_command(native["native_command"].as_array().unwrap());
    let program = Arc::new(
        Program::load(
            ProgramOptions {
                config: tsr_tsoptions::ParsedCommandLine::new(
                    options,
                    vec![JsString::from_bytes(path.as_bytes())],
                ),
                host: Arc::new(tsr_bundled::BundledFs::new(Arc::new(fs.finish()))),
                current_directory: JsString::from_bytes(b"/".as_slice()),
                default_library_path: JsString::from_bytes(tsr_bundled::LIB_PATH),
                skip_module_resolution: false,
                single_threaded: tsr_core::Tristate::UNKNOWN,
            },
            &mut FileCache::new(),
            &counters,
        )
        .unwrap(),
    );
    let generation = Generation::new(&counters);
    let owner = Arc::new(
        CheckerOwner::for_program(
            CheckerIdentity::new(generation, &counters),
            &counters,
            Arc::new(ProgramCheckerHost::new(program.clone())),
        )
        .unwrap(),
    );
    Fixture {
        program,
        owner,
        path,
        source_text: source_text.to_string(),
        native,
    }
}

fn position(source_text: &str, offset: usize) -> (usize, usize) {
    let prefix = &source_text[..offset];
    let line = prefix.bytes().filter(|&byte| byte == b'\n').count() + 1;
    let column = prefix.rsplit('\n').next().unwrap().encode_utf16().count() + 1;
    (line, column)
}

/// A diagnostic in the pinned Go oracle's JSON shape (base file name,
/// positions, code, category, localized message, chain and related
/// information), for the fixtures observed through the checker API.
pub fn diagnostic_json(program: &Program, d: &tsr_ast::Diagnostic) -> Value {
    let file = d.file.map(|id| {
        let file = program
            .files()
            .iter()
            .find(|file| file.source() == id)
            .expect("diagnostic source retained");
        let name = file.bound().view().source_file().unwrap().file_name();
        String::from_utf8(name.rsplit(|&c| c == b'/').next().unwrap().to_vec()).unwrap()
    });
    json!({"file":file,"pos":d.loc.pos(),"end":d.loc.end(),"code":d.code,"category":d.category,
        "message":String::from_utf8(tsr_compiler::diagnostic_writer::localized(d).unwrap()).unwrap(),
        "reports_deprecated":d.reports_deprecated,"chain":d.message_chain.iter().map(|d|diagnostic_json(program,d)).collect::<Vec<_>>(),
        "related":d.related_information.iter().map(|d|diagnostic_json(program,d)).collect::<Vec<_>>()})
}

/// The diagnostics of the fixture file the way the pinned command line
/// composes and prints them (`GetDiagnosticsOfAnyProgram`, then
/// `SortAndDeduplicateDiagnostics`): the syntactic diagnostics alone when there
/// are any; otherwise the global diagnostics alone when the checker starts
/// with any; otherwise the file's bind and checker diagnostics with the global
/// diagnostics found while checking. Chains are joined by newlines with
/// two-space indentation, related information is not printed, and a global
/// diagnostic has no line or column.
/// The command line's sort key (`CompareDiagnostics`): file path (empty for a
/// global diagnostic), start, end, code, category, then the message text.
type SortKey = (String, i64, i64, i32, i32, String);

pub fn observed(fixture: &Fixture) -> Vec<Value> {
    let file = fixture.program.file(fixture.path.as_bytes()).unwrap();
    let source = file.source();
    let mut diagnostics = fixture.program.syntactic_diagnostics(Some(file)).unwrap();
    if diagnostics.is_empty() {
        let mut op = fixture.owner.operation().unwrap();
        diagnostics = op.global_diagnostics().unwrap();
        if diagnostics.is_empty() {
            diagnostics = fixture.program.bind_diagnostics(Some(source)).unwrap();
            diagnostics.extend(op.semantic_diagnostics(source).unwrap());
            diagnostics.extend(op.global_diagnostics().unwrap());
        }
    }
    let mut rendered: Vec<(SortKey, Value)> = diagnostics
        .iter()
        .map(|diagnostic| {
            assert!(diagnostic.file.is_none() || diagnostic.file == Some(source));
            assert_eq!(diagnostic.category, 1);
            let message = String::from_utf8(
                tsr_compiler::diagnostic_writer::flattened(diagnostic, b"\n").unwrap(),
            )
            .unwrap();
            let (path, line, column) = if diagnostic.file.is_some() {
                let (line, column) = position(
                    &fixture.source_text,
                    usize::try_from(diagnostic.loc.pos()).unwrap(),
                );
                (fixture.path.clone(), json!(line), json!(column))
            } else {
                (String::new(), Value::Null, Value::Null)
            };
            let key = (
                path,
                diagnostic.loc.pos(),
                diagnostic.loc.end(),
                diagnostic.code,
                diagnostic.category,
                message.clone(),
            );
            (key, json!({"line": line, "column": column, "code": diagnostic.code, "message": message}))
        })
        .collect();
    rendered.sort_by(|a, b| a.0.cmp(&b.0));
    rendered.dedup_by(|a, b| a.1 == b.1);
    rendered.into_iter().map(|(_, value)| value).collect()
}

pub fn assert_fixture(name: &str, source_text: &str, native_json: &str) -> Fixture {
    let fixture = load(name, source_text, native_json);
    assert_eq!(
        json!(observed(&fixture)),
        fixture.native["diagnostics"],
        "{name}: diagnostics differ from the pinned native observation"
    );
    fixture
}
