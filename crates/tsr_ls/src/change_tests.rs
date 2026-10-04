//! Mapped editing contracts over real parsed, bound and retained programs.
use crate::{change::Tracker, LanguageService};
use std::sync::Arc;
use tsr_ast::{ContentMapperSourceFileInfo, SourceFileParseOptions, SpanSegment};
use tsr_compiler::{CachedProgramFile, FileCache, Program, ProgramFile, SourceFileCache};
use tsr_core::{CancellationToken, TextRange};
use tsr_jsstring::{JsString, PositionEncoding, SourceText};

struct MappedCache;
impl SourceFileCache for MappedCache {
    fn retain(&self, _: &Arc<ProgramFile>) -> Result<Box<dyn Send + Sync>, tsr_compiler::Error> {
        Ok(Box::new(()))
    }
    fn acquire(
        &self,
        source: SourceText,
        kind: tsr_core::ScriptKind,
        options: SourceFileParseOptions,
        counters: &tsr_arena::Counters,
        tracing: Option<&Arc<dyn tsr_checker::TraceSink>>,
    ) -> Result<CachedProgramFile, tsr_compiler::Error> {
        let mut parsed =
            tsr_parser::parse_source_file_with_counters(source, kind, options.clone(), counters);
        if options.file_name.as_bytes().starts_with(b"/virtual") {
            let root = parsed.root();
            parsed
                .builder_mut()
                .source_file_mut(root)?
                .set_content_mapper_info(ContentMapperSourceFileInfo {
                    content_mapper: JsString::from_bytes(b"fixture".as_slice()),
                    parse_options: SourceFileParseOptions {
                        file_name: JsString::from_bytes(b"/original.ts".as_slice()),
                        ..options.clone()
                    },
                    virtual_file_name: options.file_name,
                    canonical_file_name: Some(JsString::from_bytes(b"/original.ts".as_slice())),
                    original_text: SourceText::from_loaded_bytes("/*😀*/alpha; beta;".as_bytes()),
                    span_map: Some(Arc::new(tsr_ast::span_map::SpanMap::new(&[
                        SpanSegment {
                            virtual_start: 0,
                            virtual_end: 4,
                            original_start: 15,
                            original_end: 19,
                            kind: 0,
                            features: SpanSegment::FEATURE_ALL,
                        },
                        SpanSegment {
                            virtual_start: 6,
                            virtual_end: 11,
                            original_start: 8,
                            original_end: 13,
                            kind: 0,
                            features: SpanSegment::FEATURE_ALL,
                        },
                    ]))),
                    ..Default::default()
                });
        }
        Ok(CachedProgramFile {
            file: ProgramFile::bind_parsed(parsed, tracing, None)?,
            retention: Box::new(()),
        })
    }
}
fn program() -> Program {
    let mut fs = tsr_vfs::MemoryBuilder::new(b"/", true);
    let names = [b"/virtual1.ts".as_slice(), b"/virtual2.ts", b"/plain.ts"];
    for name in names {
        fs.insert_loaded(name, b"beta; alpha;".as_slice());
    }
    Program::load(
        tsr_compiler::ProgramOptions {
            config: tsr_tsoptions::ParsedCommandLine::new(
                tsr_core::CompilerOptions {
                    no_lib: tsr_core::Tristate::TRUE,
                    ..Default::default()
                },
                names.iter().map(|n| JsString::from_bytes(*n)).collect(),
            ),
            host: Arc::new(fs.finish()),
            current_directory: JsString::from_bytes(b"/".as_slice()),
            default_library_path: JsString::from_bytes(b"/".as_slice()),
            skip_module_resolution: false,
            single_threaded: tsr_core::Tristate::TRUE,
        },
        &mut FileCache::for_project(Arc::new(MappedCache)),
        &tsr_arena::Counters::new(),
    )
    .unwrap()
}

#[test]
fn mapped_edits_sort_in_original_coordinates_and_drop_the_whole_ambiguous_file() {
    let p = program();
    let a = p.source_file(b"/virtual1.ts").unwrap().source();
    let b = p.source_file(b"/virtual2.ts").unwrap().source();
    let plain = p.source_file(b"/plain.ts").unwrap().source();
    for encoding in [PositionEncoding::Utf8, PositionEncoding::Utf16] {
        let mut service = LanguageService::new(&p, encoding, CancellationToken::new());
        let mut tracker = Tracker::default();
        tracker.replace_text(a, TextRange::new(0, 4), "B".into());
        tracker.replace_text(a, TextRange::new(6, 11), "A".into());
        let result = tracker.finish(&mut service).unwrap();
        let edits = &result.edits[&tsr_lsproto::DocumentUri("file:///original.ts".into())];
        assert_eq!(
            edits
                .iter()
                .map(|e| e.new_text.as_str())
                .collect::<Vec<_>>(),
            ["A", "B"]
        );
        assert_eq!(
            edits[0].range.start.character,
            if encoding == PositionEncoding::Utf8 {
                8
            } else {
                6
            }
        );
        let mut tracker = Tracker::default();
        tracker.replace_text(a, TextRange::new(0, 4), "B".into());
        tracker.replace_text(b, TextRange::new(0, 4), "B".into());
        assert_eq!(
            tracker
                .finish(&mut service)
                .unwrap()
                .edits
                .values()
                .next()
                .unwrap()
                .len(),
            1
        );
        for conflicting in [
            TextRange::new(0, 4),
            TextRange::new(0, 0),
            TextRange::new(4, 6),
        ] {
            let mut tracker = Tracker::default();
            tracker.replace_text(a, TextRange::new(0, 4), "B".into());
            tracker.replace_text(b, conflicting, "other".into());
            tracker.replace_text(plain, TextRange::new(0, 4), "kept".into());
            let result = tracker.finish(&mut service).unwrap();
            // An insertion at the beginning of a replacement is permitted;
            // an overlapping replacement or unmappable span drops that file.
            if conflicting == TextRange::new(0, 0) {
                assert_eq!(result.edits.len(), 2);
            } else {
                assert_eq!(
                    result.unmappable,
                    [tsr_lsproto::DocumentUri("file:///original.ts".into())]
                );
                assert_eq!(result.edits.len(), 1);
            }
        }
        let mut tracker = Tracker::default();
        tracker.replace_text(a, TextRange::new(0, 0), "one".into());
        tracker.replace_text(b, TextRange::new(0, 0), "two".into());
        assert_eq!(tracker.finish(&mut service).unwrap().unmappable.len(), 1);
    }
}

#[test]
fn overlapping_provider_edits_fail_and_canceled_edits_are_not_returned() {
    let p = program();
    let source = p.source_file(b"/plain.ts").unwrap().source();
    let token = CancellationToken::new();
    let mut service = LanguageService::new(&p, PositionEncoding::Utf16, token.clone());
    let mut tracker = Tracker::default();
    tracker.replace_text(source, TextRange::new(0, 4), "x".into());
    tracker.replace_text(source, TextRange::new(1, 3), "y".into());
    assert!(std::panic::catch_unwind(
        std::panic::AssertUnwindSafe(|| tracker.finish(&mut service))
    )
    .is_err());
    let mut tracker = Tracker::default();
    tracker.replace_text(source, TextRange::new(0, 4), "x".into());
    token.cancel();
    assert!(matches!(
        tracker.finish(&mut service),
        Err(crate::Error::Canceled)
    ));
    assert_eq!(
        p.source_file(b"/plain.ts")
            .unwrap()
            .bound()
            .view()
            .source_file()
            .unwrap()
            .text()
            .as_bytes(),
        b"beta; alpha;"
    );
}
