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
            "--module" => {
                options.module = match value {
                    Some("commonjs") => ModuleKind::COMMON_JS,
                    Some("esnext") => ModuleKind::ESNEXT,
                    other => panic!("unsupported native module {other:?}"),
                }
            }
            "--target" => assert_eq!(value, Some("esnext")),
            "--noEmit" | "--ignoreConfig" | "--pretty" => {}
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

/// Syntactic then semantic diagnostics of the fixture file, rendered the way
/// the non-pretty native output records them (chains and related
/// information joined by newlines with two-space indentation).
pub fn observed(fixture: &Fixture) -> Vec<Value> {
    let file = fixture.program.file(fixture.path.as_bytes()).unwrap();
    let source = file.source();
    let mut diagnostics = fixture.program.syntactic_diagnostics(Some(file)).unwrap();
    let mut op = fixture.owner.operation().unwrap();
    diagnostics.extend(op.semantic_diagnostics(source).unwrap());
    diagnostics
        .iter()
        .map(|diagnostic| {
            assert_eq!(diagnostic.file, Some(source));
            assert_eq!(diagnostic.category, 1);
            let (line, column) = position(
                &fixture.source_text,
                usize::try_from(diagnostic.loc.pos()).unwrap(),
            );
            // The pinned non-pretty output prints the message chain and no
            // related information, so the rendering compares chains only.
            let message = String::from_utf8(
                tsr_compiler::diagnostic_writer::flattened(diagnostic, b"\n").unwrap(),
            )
            .unwrap();
            json!({"line": line, "column": column, "code": diagnostic.code, "message": message})
        })
        .collect()
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
