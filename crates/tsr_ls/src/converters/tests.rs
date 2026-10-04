use super::*;
use tsr_ast::span_map::{FEATURE_ALL, FEATURE_HOVER};
use tsr_ast::{ContentMapperSourceFileInfo, SourceFileParseOptions};
use tsr_jsstring::{JsString, SourceText};

struct Sources(tsr_ast::AstFile);
impl DiagnosticSources for Sources {
    fn diagnostic_source(
        &self,
        id: tsr_ast::NodeId,
    ) -> Result<tsr_ast::SourceFileRead<'_>, tsr_compiler::Error> {
        Ok(self.0.view().source_file(id)?)
    }
}

// Source: lsconv/converters_test.go TestConvertersInvalidUTF8 and its UTF-16
// JS-reference test. CRLF contributes one line break; LS ignores U+2028/2029.
#[test]
fn negotiated_coordinates_and_invalid_bytes() {
    for encoding in [PositionEncoding::Utf8, PositionEncoding::Utf16] {
        let mut conv = Converters::new(encoding);
        let script = Script::plain(b"invalid.ts", b"a\x80b\ncd");
        for (offset, line, character) in [
            (0, 0, 0),
            (1, 0, 1),
            (2, 0, 2),
            (3, 0, 3),
            (4, 1, 0),
            (5, 1, 1),
            (6, 1, 2),
        ] {
            let expected = lsp::Position { line, character };
            assert_eq!(conv.to_lsp_position(&script, offset).0, expected);
            assert_eq!(
                conv.from_lsp_position(&script, &expected, FEATURE_ALL)[0].position,
                offset
            );
        }
        let script = Script::plain(b"unicode.ts", "a😀β\r\nz\u{2028}t\r\n".as_bytes());
        let expected = if encoding == PositionEncoding::Utf16 {
            3
        } else {
            5
        };
        assert_eq!(
            conv.to_lsp_position(&script, 5).0,
            lsp::Position {
                line: 0,
                character: expected
            }
        );
        assert_eq!(
            conv.from_lsp_position(
                &script,
                &lsp::Position {
                    line: 0,
                    character: expected
                },
                FEATURE_ALL
            )[0]
            .position,
            5
        );
        assert_eq!(conv.to_lsp_position(&script, 13).0.line, 1);
        assert_eq!(
            conv.to_lsp_position(&script, 16).0,
            lsp::Position {
                line: 2,
                character: 0
            }
        );
    }
}
#[test]
fn canonical_and_supplemental_projection_preserves_script_identity() {
    let mut conv = Converters::new(PositionEncoding::Utf16);
    for (name, text, start) in [
        (b"canonical".as_slice(), " x", 1),
        (b"supplemental", "  x", 2),
    ] {
        let map = SpanMap::new(&[tsr_ast::SpanSegment {
            virtual_start: start,
            virtual_end: start + 1,
            original_start: 0,
            original_end: 1,
            kind: 0,
            features: FEATURE_ALL,
        }]);
        let script = Script {
            file_name: name,
            text: text.as_bytes(),
            original_file_name: b"/component.vue",
            original_text: b"x",
            span_map: Some(&map),
        };
        assert_eq!(
            conv.from_lsp_position(&script, &lsp::Position::default(), FEATURE_HOVER)[0].position,
            start
        );
        let range = TextRange::new(i64::from(start), i64::from(start + 1));
        let (loc, fidelity) = conv.to_lsp_location(&script, range);
        assert_eq!(fidelity, Fidelity::Exact);
        assert_eq!(loc.uri.0, "file:///component.vue");
        assert_eq!(loc.range.end.character, 1);
        assert_eq!(
            conv.from_lsp_range(&script, &loc.range, FEATURE_HOVER)[0].span,
            range
        );
        assert_eq!(
            conv.from_lsp_range_intersecting(&script, &loc.range, FEATURE_HOVER)[0].span,
            range
        );
    }
}

// Pinned TestConvertersSourceFileProjectionExpansion, through the complete
// source-file entry points rather than manually enumerating both scripts.
#[test]
fn source_file_projection_expands_supplementals_with_their_owner_ids() {
    let mut builder = tsr_ast::AstBuilder::new(SourceText::default(), &tsr_arena::Counters::new());
    let options = SourceFileParseOptions {
        file_name: JsString::from_bytes(b"/component.vue".as_slice()),
        ..Default::default()
    };
    let canonical = builder.new_source_file(
        options.clone(),
        SourceText::from_bytes(b" x".as_slice()),
        None,
        None,
    );
    let supplemental = builder.new_source_file(
        options,
        SourceText::from_bytes(b"  x".as_slice()),
        None,
        None,
    );
    let children = builder.source_nodes(vec![Some(supplemental)]).unwrap();
    for (id, start) in [(canonical, 1), (supplemental, 2)] {
        builder
            .source_file_mut(id)
            .unwrap()
            .set_content_mapper_info(ContentMapperSourceFileInfo {
                content_mapper: JsString::from_bytes(b"mapper".as_slice()),
                original_text: SourceText::from_bytes(b"x".as_slice()),
                span_map: Some(std::sync::Arc::new(SpanMap::new(&[tsr_ast::SpanSegment {
                    virtual_start: start,
                    virtual_end: start + 1,
                    original_end: 1,
                    features: FEATURE_ALL,
                    ..Default::default()
                }]))),
                supplemental_source_files: if id == canonical {
                    children
                } else {
                    tsr_ast::SourceNodeSlice::default()
                },
                canonical_source_file: (id == supplemental).then_some(canonical),
                ..Default::default()
            });
    }
    let sources = Sources(builder.complete(canonical).unwrap().publish_unbound());
    let mut converters = Converters::new(PositionEncoding::Utf16);
    let positions = converters
        .from_lsp_position_for_source_file(
            &sources,
            canonical,
            &lsp::Position::default(),
            FEATURE_HOVER,
        )
        .unwrap();
    assert_eq!(
        positions
            .iter()
            .map(|p| (p.script, p.mapped.position))
            .collect::<Vec<_>>(),
        [(canonical, 1), (supplemental, 2)]
    );
    let range = lsp::Range {
        end: lsp::Position {
            line: 0,
            character: 1,
        },
        ..Default::default()
    };
    for spans in [
        converters
            .from_lsp_range_for_source_file(&sources, canonical, &range, FEATURE_HOVER)
            .unwrap(),
        converters
            .from_lsp_range_intersecting_for_source_file(&sources, canonical, &range, FEATURE_HOVER)
            .unwrap(),
    ] {
        assert_eq!(
            spans
                .iter()
                .map(|p| (p.script, p.mapped.span))
                .collect::<Vec<_>>(),
            [
                (canonical, TextRange::new(1, 2)),
                (supplemental, TextRange::new(2, 3))
            ]
        );
    }
}

#[test]
fn diagnostics_preserve_chains_tags_locales_and_client_capabilities() {
    struct NoSources;
    impl DiagnosticSources for NoSources {
        fn diagnostic_source(
            &self,
            _: tsr_ast::NodeId,
        ) -> Result<tsr_ast::SourceFileRead<'_>, tsr_compiler::Error> {
            panic!("global diagnostic cannot read a source")
        }
    }
    let mut diagnostic = Diagnostic::compiler(
        tsr_diagnostics::by_code(6133).unwrap(),
        vec![JsString::from_bytes(b"unused".as_slice())],
    );
    diagnostic.reports_deprecated = true;
    diagnostic
        .message_chain
        .push(std::sync::Arc::new(Diagnostic::external(
            None,
            TextRange::default(),
            JsString::default(),
            3,
            0,
            JsString::from_bytes(b"child detail".as_slice()),
        )));
    let mut converter = Converters::new(PositionEncoding::Utf16);
    let base = converter
        .diagnostic(&NoSources, &diagnostic, &DiagnosticOptions::default())
        .unwrap();
    assert_eq!(
        base.severity.as_deref(),
        Some(&lsp::DiagnosticSeverity::ERROR)
    );
    assert_eq!(base.code.unwrap().integer.as_deref(), Some(&6133));
    assert_eq!(
        base.message.string.as_deref().unwrap(),
        "'unused' is declared but its value is never read.\n  child detail"
    );
    assert!(base.tags.is_none());
    let localized = converter
        .diagnostic(
            &NoSources,
            &diagnostic,
            &DiagnosticOptions {
                locale: tsr_locale::Locale::parse("de").0,
                style_checks_as_warnings: true,
                visual_studio: true,
                tags: vec![
                    lsp::DiagnosticTag::UNNECESSARY,
                    lsp::DiagnosticTag::DEPRECATED,
                ],
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(
        localized.severity.as_deref(),
        Some(&lsp::DiagnosticSeverity::WARNING)
    );
    assert_eq!(localized.code.unwrap().string.as_deref().unwrap(), "TS6133");
    assert_eq!(
        localized.tags.as_deref().unwrap().as_slice(),
        [
            lsp::DiagnosticTag::UNNECESSARY,
            lsp::DiagnosticTag::DEPRECATED
        ]
    );
    assert_ne!(localized.message, base.message);
}
