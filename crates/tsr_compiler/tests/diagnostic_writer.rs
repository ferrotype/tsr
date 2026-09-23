#[path = "../../../tools/s08/p5/error_requests.rs"]
mod error_requests;
#[path = "../../../tools/s08/p5/errors.rs"]
mod errors;
#[path = "../../../tools/s08/p5/paths.rs"]
mod paths;
use serde_json::Value;

#[test]
fn native_plain_pretty_and_error_baseline_bytes_match() {
    let request =
        serde_json::from_str(include_str!("../../../data/s08/p5/errors/requests.json")).unwrap();
    let expected: Value = serde_json::from_str(include_str!(
        "../../../data/s08/p5/errors/observations.json"
    ))
    .unwrap();
    let actual = error_requests::observe(&request).unwrap();
    assert_eq!(
        actual["cases"].as_array().unwrap().len(),
        expected["cases"].as_array().unwrap().len()
    );
    for (e, a) in expected["cases"]
        .as_array()
        .unwrap()
        .iter()
        .zip(actual["cases"].as_array().unwrap())
    {
        assert_eq!(a["state"], "executed", "{}: {a}", e["id"]);
        assert_eq!(a, e, "{}", e["id"]);
    }
}

#[test]
fn deep_message_flattening_uses_an_explicit_stack() {
    std::thread::Builder::new()
        .stack_size(512 * 1024)
        .spawn(|| {
            use std::sync::Arc;
            use tsr_ast::Diagnostic;
            use tsr_jsstring::JsString;
            fn message() -> Diagnostic {
                Diagnostic::external(
                    None,
                    tsr_core::TextRange::new(-1, -1),
                    JsString::default(),
                    1,
                    9999,
                    JsString::from_bytes(b"x".as_slice()),
                )
            }
            let depth = 1000usize;
            let mut node = Arc::new(message());
            for _ in 0..depth {
                let mut parent = message();
                parent.message_chain.push(node);
                node = Arc::new(parent);
            }
            let output = tsr_compiler::diagnostic_writer::flattened(&node, b"\n").unwrap();
            assert_eq!(output.len(), depth * (depth + 1) + 2 * depth + 1);
            assert!(output.ends_with(b" x"));
            // Avoid testing Arc's recursively derived drop instead of the writer.
            let mut pending = vec![node];
            while let Some(node) = pending.pop() {
                let mut node = Arc::try_unwrap(node).unwrap();
                pending.append(&mut node.message_chain);
            }
        })
        .unwrap()
        .join()
        .unwrap();
}

#[test]
fn mapped_diagnostics_select_text_without_mutating_the_ast() {
    use tsr_ast::{
        AstFile, ContentMapperSourceFileInfo, Diagnostic, NodeId, SourceFileParseOptions,
        SourceFileRead, SpanSegment,
    };
    use tsr_compiler::{
        diagnostic_writer::{DiagnosticSources, DiagnosticWriter, FormattingOptions},
        Error,
    };
    use tsr_core::{ScriptKind, TextRange};
    use tsr_jsstring::{JsString, SourceText};
    struct Sources(AstFile, NodeId);
    impl DiagnosticSources for Sources {
        fn diagnostic_source(&self, id: NodeId) -> Result<SourceFileRead<'_>, Error> {
            if id != self.1 {
                return Err(Error::Unsupported("foreign test source"));
            }
            Ok(self.0.view().source_file(id)?)
        }
    }
    let mut parsed = tsr_parser::parse_source_file(
        SourceText::from_bytes(b"xxxfoo".as_slice()),
        ScriptKind::TS,
        SourceFileParseOptions {
            file_name: JsString::from_bytes(b"/mapped.ts".as_slice()),
            ..Default::default()
        },
    );
    let root = parsed.root();
    parsed
        .builder_mut()
        .source_file_mut(root)
        .unwrap()
        .set_content_mapper_info(ContentMapperSourceFileInfo {
            content_mapper: JsString::from_bytes(b"test-mapper".as_slice()),
            original_text: SourceText::from_bytes(b"\nfoo".as_slice()),
            span_map: Some(tsr_ast::span_map::new(&[SpanSegment {
                virtual_start: 3,
                virtual_end: 6,
                original_start: 1,
                original_end: 4,
                kind: 0,
                features: 0,
            }])),
            ..Default::default()
        });
    let sources = Sources(parsed.publish_unbound(), root);
    let mut writer = DiagnosticWriter::from_sources(
        &sources,
        FormattingOptions {
            current_directory: b"/".to_vec(),
            ..Default::default()
        },
    );
    let mut diagnostic = Diagnostic::external(
        Some(root),
        TextRange::new(3, 6),
        JsString::default(),
        1,
        9999,
        JsString::from_bytes(b"problem".as_slice()),
    );
    assert_eq!(
        writer.format(&[&diagnostic], false).unwrap(),
        b"mapped.ts(2,1): error TS9999: problem\n"
    );
    let pretty = writer.format(&[&diagnostic], true).unwrap();
    assert!(pretty.windows(3).any(|bytes| bytes == b"foo"));
    assert!(!pretty.windows(6).any(|bytes| bytes == b"xxxfoo"));
    assert_eq!(diagnostic.loc, TextRange::new(3, 6));
    diagnostic.loc = TextRange::new(0, 2);
    let rendered = writer.format(&[&diagnostic], false).unwrap();
    assert!(rendered.starts_with(b"mapped.ts(1,1): error TS9999: problem\n  "));
    assert!(rendered
        .windows(b"test-mapper".len())
        .any(|bytes| bytes == b"test-mapper"));
    assert_eq!(diagnostic.message_chain.len(), 0);
    diagnostic.source = JsString::from_bytes(b"external".as_slice());
    diagnostic.loc = TextRange::new(1, 4);
    assert_eq!(
        writer.format(&[&diagnostic], false).unwrap(),
        b"mapped.ts(2,1): error external9999: problem\n"
    );
    let retained = writer.file(&diagnostic).unwrap().unwrap();
    let foreign = tsr_parser::parse_source_file(
        SourceText::from_bytes(b"foo".as_slice()),
        ScriptKind::TS,
        SourceFileParseOptions {
            file_name: JsString::from_bytes(b"/foreign.ts".as_slice()),
            ..Default::default()
        },
    );
    diagnostic.file = Some(foreign.root());
    assert!(writer.file(&diagnostic).is_err());
    drop(writer);
    drop(sources);
    assert_eq!(retained.text(), b"\nfoo");
}

#[test]
fn request_locale_applies_to_chains_status_and_summary() {
    use std::sync::Arc;
    use tsr_ast::Diagnostic;
    use tsr_compiler::diagnostic_writer::{
        localized, localized_with_locale, DiagnosticWriter, FormattingOptions,
    };
    use tsr_jsstring::JsString;
    let sources =
        tsr_tsoptions::ParsedCommandLine::new(tsr_core::CompilerOptions::default(), vec![]);
    let mut diagnostic = Diagnostic::compiler(
        tsr_diagnostics::Unknown_compiler_option_0,
        vec![JsString::from_bytes(b"notAnOption".as_slice())],
    );
    diagnostic.message_chain.push(Arc::new(Diagnostic::compiler(
        tsr_diagnostics::Cannot_find_name_0,
        vec![JsString::from_bytes(b"name".as_slice())],
    )));
    let mut writer = DiagnosticWriter::from_sources(
        &sources,
        FormattingOptions {
            locale: tsr_locale::Locale::parse("de-DE").0,
            ..Default::default()
        },
    );
    // Text is from the pinned de-DE catalog; the envelope and indentation are
    // WriteFlattenedDiagnosticMessage / FormatDiagnosticsStatusAndTime.
    let flattened =
        "Unbekannte Compileroption \"notAnOption\".\n  Der Name \"name\" wurde nicht gefunden.";
    assert_eq!(
        writer.flatten(&diagnostic, b"\n").unwrap(),
        flattened.as_bytes()
    );
    assert_eq!(
        writer.status(&diagnostic, b"12:34:56", false).unwrap(),
        format!("12:34:56 - {flattened}").as_bytes()
    );
    assert_eq!(
        writer.format(&[&diagnostic], false).unwrap(),
        format!("error TS5023: {flattened}\n").as_bytes()
    );
    assert_eq!(
        writer.error_summary(&[&diagnostic]).unwrap(),
        "\n1 Fehler gefunden.\n\n".as_bytes()
    );
    assert_eq!(
        localized(&diagnostic).unwrap(),
        b"Unknown compiler option 'notAnOption'."
    );
    assert_eq!(
        localized_with_locale(&diagnostic, &tsr_locale::Locale::parse("zz-ZZ").0).unwrap(),
        localized(&diagnostic).unwrap()
    );
    let external = Diagnostic::external(
        None,
        tsr_core::TextRange::new(-1, -1),
        JsString::default(),
        1,
        9999,
        JsString::from_bytes(b"raw\xfftext".as_slice()),
    );
    assert_eq!(writer.flatten(&external, b"\n").unwrap(), b"raw\xfftext");
}

#[test]
fn translated_error_table_heading_is_not_hardcoded() {
    use tsr_compiler::diagnostic_writer::{DiagnosticWriter, FormattingOptions};
    let sources =
        tsr_tsoptions::ParsedCommandLine::new(tsr_core::CompilerOptions::default(), vec![]);
    let writer = DiagnosticWriter::from_sources(
        &sources,
        FormattingOptions {
            locale: tsr_locale::Locale::parse("ja-JP").0,
            ..Default::default()
        },
    );
    // An empty table still renders its translated heading, as the native leaf
    // writeTabularErrorsDisplay does. This detects the old hardcoded English.
    assert_eq!(
        writer.tabular_errors(&[]).unwrap(),
        "エラーの発生したファイル\n".as_bytes()
    );
}
