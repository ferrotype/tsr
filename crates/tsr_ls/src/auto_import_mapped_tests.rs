//! Mapped auto-imports are offered only when every additional edit is writable.
use crate::{syntax::Syntax, CompletionOptions, LanguageService};
use std::sync::Arc;
use tsr_ast::{ContentMapperSourceFileInfo, SourceFileParseOptions, SpanSegment};
use tsr_compiler::{CachedProgramFile, FileCache, Program, ProgramFile, SourceFileCache};
use tsr_core::{CancellationToken, CompilerOptions, Tristate};
use tsr_jsstring::{JsString, PositionEncoding, SourceText};
use tsr_lsproto as lsp;
const ORIGINAL: &str = "const value = help;\n";
const HEADER: &str = "/* synthesized header */\n";
fn js(text: &str) -> JsString {
    JsString::from_bytes(text.as_bytes())
}
struct MappedCache(bool);
impl SourceFileCache for MappedCache {
    fn retain(&self, _: &Arc<ProgramFile>) -> Result<Box<dyn Send + Sync>, tsr_compiler::Error> {
        Ok(Box::new(()))
    }
    fn acquire(
        &self,
        text: SourceText,
        kind: tsr_core::ScriptKind,
        options: SourceFileParseOptions,
        counters: &tsr_arena::Counters,
        tracing: Option<&Arc<dyn tsr_checker::TraceSink>>,
    ) -> Result<CachedProgramFile, tsr_compiler::Error> {
        let mut parsed =
            tsr_parser::parse_source_file_with_counters(text, kind, options.clone(), counters);
        parsed
            .root_source_file_mut()?
            .set_content_mapper_info(ContentMapperSourceFileInfo {
                content_mapper: js("fixture"),
                parse_options: options.clone(),
                virtual_file_name: options.file_name,
                original_text: SourceText::from_loaded_bytes(ORIGINAL.as_bytes()),
                span_map: Some(Arc::new(tsr_ast::span_map::SpanMap::new(&if self.0 {
                    vec![SpanSegment {
                        virtual_start: HEADER.len() as i32,
                        virtual_end: (HEADER.len() + ORIGINAL.len()) as i32,
                        original_start: 0,
                        original_end: ORIGINAL.len() as i32,
                        kind: tsr_ast::span_map::KIND_VERBATIM,
                        features: SpanSegment::FEATURE_ALL,
                    }]
                } else {
                    vec![]
                }))),
                ..Default::default()
            });
        Ok(CachedProgramFile {
            file: ProgramFile::bind_parsed(parsed, tracing, None)?,
            retention: Box::new(()),
        })
    }
}
fn program(writable: bool) -> Program {
    let mut fs = tsr_vfs::MemoryBuilder::new(b"/", true);
    fs.insert_loaded(b"/main.ts", format!("{HEADER}{ORIGINAL}").as_bytes());
    Program::load(
        tsr_compiler::ProgramOptions {
            config: tsr_tsoptions::ParsedCommandLine::new(
                CompilerOptions {
                    no_lib: Tristate::TRUE,
                    ..Default::default()
                },
                vec![js("/main.ts")],
            ),
            host: Arc::new(fs.finish()),
            current_directory: js("/"),
            default_library_path: js("/"),
            skip_module_resolution: false,
            single_threaded: Tristate::TRUE,
        },
        &mut FileCache::for_project(Arc::new(MappedCache(writable))),
        &tsr_arena::Counters::new(),
    )
    .unwrap()
}
fn candidates() -> lsp::CompletionList {
    lsp::CompletionList {
        items: vec![
            Some(Box::new(lsp::CompletionItem {
                label: "helper".into(),
                data: Some(Box::new(lsp::CompletionItemData {
                    auto_import: Some(Box::new(lsp::AutoImportFix {
                        kind: lsp::AutoImportFixKind::ADD_NEW,
                        import_kind: lsp::ImportKind::NAMED,
                        name: "helper".into(),
                        module_specifier: "./dep".into(),
                        ..Default::default()
                    })),
                    ..Default::default()
                })),
                ..Default::default()
            })),
            Some(Box::new(lsp::CompletionItem {
                label: "local".into(),
                ..Default::default()
            })),
        ],
        ..Default::default()
    }
}
#[test]
fn mapped_auto_import_skips_synthesized_header_and_retains_safe_original_edit() {
    let p = program(true);
    let file = p.source_file(b"/main.ts").unwrap();
    let mut syntax = Syntax::new(file.bound().view().ast(), file.source()).unwrap();
    let mut service = LanguageService::new(&p, PositionEncoding::Utf16, CancellationToken::new());
    let mut list = candidates();
    service
        .filter_content_mapped_auto_imports(&mut syntax, &CompletionOptions::default(), &mut list)
        .unwrap();
    assert_eq!(list.items.len(), 2);
    let helper = list.items[0].as_ref().unwrap();
    let edits = helper.additional_text_edits.as_deref().unwrap();
    assert_eq!(edits.len(), 1);
    let edit = edits[0].as_ref().unwrap();
    assert_eq!(
        edit.range.start,
        lsp::Position {
            line: 0,
            character: 0
        }
    );
    assert_eq!(edit.range.end, edit.range.start);
    assert_eq!(
        format!("{}{ORIGINAL}", edit.new_text),
        "import { helper } from \"./dep\";\n\nconst value = help;\n"
    );
    assert_eq!(
        helper.detail.as_deref().map(String::as_str),
        Some("Add import from \"./dep\"")
    );
}
#[test]
fn mapped_auto_import_drops_unwritable_import_and_keeps_non_import_candidate() {
    let p = program(false);
    let file = p.source_file(b"/main.ts").unwrap();
    let mut syntax = Syntax::new(file.bound().view().ast(), file.source()).unwrap();
    let mut service = LanguageService::new(&p, PositionEncoding::Utf16, CancellationToken::new());
    let mut list = candidates();
    service
        .filter_content_mapped_auto_imports(&mut syntax, &CompletionOptions::default(), &mut list)
        .unwrap();
    assert_eq!(list.items.len(), 1);
    assert_eq!(list.items[0].as_ref().unwrap().label, "local");
}
