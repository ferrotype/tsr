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
    let (locale, valid_locale) = tsr_locale::Locale::parse("de-DE");
    assert!(valid_locale);
    assert_eq!(locale.tag_string(), "de-DE");
    let message = tsr_diagnostics::by_code(2322).unwrap();
    let translated = tsr_diagnostics::localize(
        &locale,
        Some(message),
        message.key.as_bytes(),
        &[b"number", b"string"],
    );
    // Pinned de-DE catalog text: verifies the archive carries both locale
    // negotiation tables and generated translated diagnostic messages.
    let expected = "Der Typ \"number\" kann dem Typ \"string\" nicht zugewiesen werden.";
    assert_eq!(translated, expected.as_bytes());

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
        let formatted =
            tsr_compiler::diagnostic_writer::localized_with_locale(&diagnostics[0], &locale)
                .unwrap();
        assert_eq!(formatted, translated);
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
    println!(
        "{}",
        serde_json::json!({
            "encoded_bytes": encoded.len(),
            "libraries": tsr_bundled::LIBRARIES.len(),
            "locale": locale.tag_string(),
            "diagnostic_code": message.code,
            "localized_message": String::from_utf8(translated).unwrap(),
            "owners_returned_to_baseline": counters.snapshot() == Counts::default(),
        })
    );
}
