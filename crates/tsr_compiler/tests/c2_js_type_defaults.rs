//! C2: JavaScript implicit-any defaults use identity before instantiation.
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::sync::Arc;
use tsr_arena::{CheckerIdentity, Counters, Generation};
use tsr_checker::CheckerOwner;
use tsr_compiler::{FileCache, Program, ProgramCheckerHost, ProgramOptions};
use tsr_core::{CompilerOptions, ScriptTarget, Tristate};
use tsr_jsstring::JsString;

const FILES: &[(&str, &str)] = &[
    (
        "/classes.d.ts",
        include_str!("fixtures/c2/js_type_defaults/classes.d.ts"),
    ),
    (
        "/control.ts",
        include_str!("fixtures/c2/js_type_defaults/control.ts"),
    ),
    (
        "/use.js",
        include_str!("fixtures/c2/js_type_defaults/use.js"),
    ),
];
const NATIVE: &str = include_str!("fixtures/c2/js_type_defaults/native.json");

#[test]
fn javascript_class_defaults_match_native_identity_and_instantiation_order() {
    let native: Value = serde_json::from_str(NATIVE).unwrap();
    let pin: Value = serde_json::from_str(include_str!("../../../data/upstream.json")).unwrap();
    assert_eq!(native["pin"], pin["pin"]);
    let source_hashes = FILES.iter().map(|(file, source)| {
        json!({"file": file, "sha256": format!("{:x}", Sha256::digest(source.as_bytes()))})
    }).collect::<Vec<_>>();
    assert_eq!(json!(source_hashes), native["sources"]);

    let mut fs = tsr_vfs::MemoryBuilder::new(b"/", true);
    for &(file, source) in FILES {
        fs.insert_loaded(file.as_bytes(), source.as_bytes());
    }
    let counters = Counters::new();
    let program = Arc::new(
        Program::load(
            ProgramOptions {
                config: tsr_tsoptions::ParsedCommandLine::new(
                    CompilerOptions {
                        target: ScriptTarget::ESNEXT,
                        strict: Tristate::TRUE,
                        allow_js: Tristate::TRUE,
                        check_js: Tristate::TRUE,
                        ..Default::default()
                    },
                    [b"/use.js".as_slice(), b"/control.ts".as_slice()]
                        .map(JsString::from_bytes)
                        .to_vec(),
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
    let owner = Arc::new(
        CheckerOwner::for_program(
            CheckerIdentity::new(Generation::new(&counters), &counters),
            &counters,
            Arc::new(ProgramCheckerHost::new(program.clone())),
        )
        .unwrap(),
    );
    let mut op = owner.operation().unwrap();
    let mut observed = Vec::new();
    for &(file, source_text) in FILES {
        let source = program.file(file.as_bytes()).unwrap().source();
        for diagnostic in op.semantic_diagnostics(source).unwrap() {
            assert_eq!(diagnostic.file, Some(source));
            assert_eq!(diagnostic.category, 1);
            assert!(diagnostic.related_information.is_empty());
            let prefix = &source_text[..usize::try_from(diagnostic.loc.pos()).unwrap()];
            let line = prefix.bytes().filter(|&byte| byte == b'\n').count() + 1;
            let column = prefix.rsplit('\n').next().unwrap().encode_utf16().count() + 1;
            let message = String::from_utf8(
                tsr_compiler::diagnostic_writer::flattened(&diagnostic, b"\n").unwrap(),
            )
            .unwrap();
            observed.push(json!({"file": file, "line": line, "column": column,
                "code": diagnostic.code, "message": message}));
        }
    }
    // A structural empty interface is identical to {}, whereas explicit unknown
    // and {} arguments and defaults instantiated from them retain their types.
    assert_eq!(json!(observed), native["diagnostics"]);
}
