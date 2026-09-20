//! External consumer smoke check, copied into the isolated package workspace.
use std::sync::Arc;
use tsr_arena::{Counters, Counts};
use tsr_ast::SourceFileParseOptions;
use tsr_core::{CompilerOptions, ScriptKind, Tristate};
use tsr_embed::{FileCache, ProgramOptions, Session};
use tsr_jsstring::{JsString, SourceText};

fn main() {
    let encoded = tsr_embed::parse_and_encode(
        SourceText::from_loaded_bytes(b"const value: string = 1;".as_slice()),
        ScriptKind::TS,
        SourceFileParseOptions {
            file_name: JsString::from_bytes(b"/main.ts".as_slice()),
            path: JsString::from_bytes(b"/main.ts".as_slice()),
            ..Default::default()
        },
    )
    .unwrap();
    assert!(!encoded.is_empty());
    assert_eq!(tsr_bundled::LIBRARIES.len(), 108);
    assert!(tsr_bundled::LIBRARIES
        .iter()
        .all(|(_, bytes)| !bytes.is_empty()));
    assert!(tsr_bundled::COPYRIGHT.contains("Microsoft Corporation"));

    let mut host = tsr_vfs::MemoryBuilder::new(b"/", true);
    host.insert_loaded(b"/main.ts", b"const value: string = 1;".as_slice());
    let counters = Counters::new();
    let session = Session::load(
        ProgramOptions {
            config: tsr_tsoptions::ParsedCommandLine::new(
                CompilerOptions {
                    no_lib: Tristate::TRUE,
                    strict: Tristate::TRUE,
                    ..Default::default()
                },
                vec![JsString::from_bytes(b"/main.ts".as_slice())],
            ),
            host: Arc::new(host.finish()),
            current_directory: JsString::from_bytes(b"/".as_slice()),
            default_library_path: JsString::from_bytes(b"/lib".as_slice()),
            skip_module_resolution: false,
        },
        &mut FileCache::new(),
        &counters,
    )
    .unwrap();
    let retained = {
        let mut operation = session.operation().unwrap();
        let file = session.program().file(b"/main.ts").unwrap();
        let diagnostics = session
            .program()
            .semantic_diagnostics_with_checker(&mut operation, file)
            .unwrap();
        assert_eq!(
            diagnostics.iter().map(|d| d.code).collect::<Vec<_>>(),
            [2322]
        );
        operation
            .retain_type(operation.builtin_type("stringType").unwrap())
            .unwrap()
    };
    drop(session);
    {
        let operation = retained.owner().operation().unwrap();
        let typ = operation.import_type(&retained).unwrap();
        assert_eq!(
            operation.intrinsic_type_name(typ).unwrap().as_bytes(),
            b"string"
        );
    }
    drop(retained);
    assert_eq!(counters.snapshot(), Counts::default());
    println!("packaged parser, bundled assets, checker and retained ownership: pass");
}
