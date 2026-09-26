//! Test-only loader and diagnostic renderer shared by the C2 native fixtures.
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::sync::Arc;
use tsr_arena::{CheckerIdentity, Counters, Generation};
use tsr_checker::CheckerOwner;
use tsr_compiler::{FileCache, Program, ProgramCheckerHost, ProgramOptions};
use tsr_core::{CompilerOptions, ScriptTarget, Tristate};
use tsr_jsstring::JsString;

pub fn assert_native_diagnostics(source_text: &str, native_json: &str, path: &str) {
    let native: Value = serde_json::from_str(native_json).unwrap();
    let pin: Value = serde_json::from_str(include_str!("../../../../data/upstream.json")).unwrap();
    assert_eq!(native["pin"], pin["pin"]);
    assert_eq!(
        native["source_sha256"],
        format!("{:x}", Sha256::digest(source_text.as_bytes()))
    );
    let mut fs = tsr_vfs::MemoryBuilder::new(b"/", true);
    fs.insert_loaded(path.as_bytes(), source_text.as_bytes());
    let counters = Counters::new();
    let program = Arc::new(
        Program::load(
            ProgramOptions {
                config: tsr_tsoptions::ParsedCommandLine::new(
                    CompilerOptions {
                        target: ScriptTarget::ESNEXT,
                        strict: Tristate::TRUE,
                        ..Default::default()
                    },
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
    let mut op = owner.operation().unwrap();
    let source = program.file(path.as_bytes()).unwrap().source();
    let diagnostics = op.semantic_diagnostics(source).unwrap();
    let observed = diagnostics
        .iter()
        .map(|diagnostic| {
            assert_eq!(diagnostic.file, Some(source));
            assert_eq!(diagnostic.category, 1);
            assert!(diagnostic.related_information.is_empty());
            let position = usize::try_from(diagnostic.loc.pos()).unwrap();
            let prefix = &source_text[..position];
            let line = prefix.bytes().filter(|&byte| byte == b'\n').count() + 1;
            let column = prefix.rsplit('\n').next().unwrap().encode_utf16().count() + 1;
            let message = String::from_utf8(
                tsr_compiler::diagnostic_writer::flattened(diagnostic, b"\n").unwrap(),
            )
            .unwrap();
            json!({"line": line, "column": column, "code": diagnostic.code, "message": message})
        })
        .collect::<Vec<_>>();
    assert_eq!(json!(observed), native["diagnostics"]);
}
