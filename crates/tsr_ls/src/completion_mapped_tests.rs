//! Completion payloads must retain the selected mapped projection across resolve.
use crate::{Error, LanguageService};
use std::sync::Arc;
use tsr_ast::{ContentMapperSourceFileInfo, SourceFileParseOptions};
use tsr_compiler::{CachedProgramFile, FileCache, Program, ProgramFile, SourceFileCache};
use tsr_core::{CancellationToken, CompilerOptions, Tristate};
use tsr_jsstring::{JsString, PositionEncoding, SourceText};
use tsr_lsproto as lsp;

fn js(text: &str) -> JsString {
    JsString::from_bytes(text.as_bytes())
}
struct MappedCache;
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
        let canonical = options.file_name.as_bytes() == b"/component.ts";
        parsed
            .root_source_file_mut()?
            .set_content_mapper_info(ContentMapperSourceFileInfo {
                content_mapper: js("fixture"),
                parse_options: options.clone(),
                virtual_file_name: options.file_name.clone(),
                supplemental_file_names: if canonical {
                    vec![js("/component.ts.0.ts"), js("/component.ts.1.ts")]
                } else {
                    vec![]
                },
                canonical_file_name: (!canonical).then(|| js("/component.ts")),
                ..Default::default()
            });
        Ok(CachedProgramFile {
            file: ProgramFile::bind_parsed(parsed, tracing, None)?,
            retention: Box::new(()),
        })
    }
}
fn program() -> Program {
    let mut fs = tsr_vfs::MemoryBuilder::new(b"/", true);
    for (name, text) in [
        (
            b"/component.ts".as_slice(),
            b"export const canonical = 0;".as_slice(),
        ),
        (b"/component.ts.0.ts", b"export const first = 1;"),
        (b"/component.ts.1.ts", b"export const second = 2;"),
    ] {
        fs.insert_loaded(name, text);
    }
    Program::load(
        tsr_compiler::ProgramOptions {
            config: tsr_tsoptions::ParsedCommandLine::new(
                CompilerOptions {
                    no_lib: Tristate::TRUE,
                    ..Default::default()
                },
                vec![js("/component.ts")],
            ),
            host: Arc::new(fs.finish()),
            current_directory: js("/"),
            default_library_path: js("/"),
            skip_module_resolution: false,
            single_threaded: Tristate::TRUE,
        },
        &mut FileCache::for_project(Arc::new(MappedCache)),
        &tsr_arena::Counters::new(),
    )
    .unwrap()
}

#[test]
fn mapped_completion_payload_selects_its_independent_projection_on_resolve() {
    let p = program();
    let service = LanguageService::new(&p, PositionEncoding::Utf16, CancellationToken::new());
    let canonical = p.source_file(b"/component.ts").unwrap().source();
    assert!(service
        .completion_source_index(canonical)
        .unwrap()
        .is_none());
    for (index, name) in ["/component.ts.0.ts", "/component.ts.1.ts"]
        .into_iter()
        .enumerate()
    {
        let source = p.source_file(name.as_bytes()).unwrap().source();
        assert_eq!(
            service.completion_source_index(source).unwrap().as_deref(),
            Some(&(index as i32))
        );
        let mut list = lsp::CompletionList {
            items: vec![Some(Box::new(lsp::CompletionItem {
                label: "symbol".into(),
                ..Default::default()
            }))],
            ..Default::default()
        };
        service.completion_data(source, 17, &mut list).unwrap();
        let data = list.items[0].as_ref().unwrap().data.as_deref().unwrap();
        assert_eq!(data.file_name, "/component.ts");
        assert_eq!(data.position, 17);
        assert_eq!(
            data.supplemental_file_index.as_deref(),
            Some(&(index as i32))
        );
        assert_eq!(
            service.completion_source(data).unwrap(),
            source,
            "the source used by LSP resolve must keep the supplemental owner"
        );
        assert_ne!(source, canonical);
    }
    let mut data = lsp::CompletionItemData {
        file_name: "/component.ts".into(),
        ..Default::default()
    };
    assert_eq!(service.completion_source(&data).unwrap(), canonical);
    for index in [-1, 2, i32::MAX] {
        data.supplemental_file_index = Some(Box::new(index));
        assert!(
            matches!(service.completion_source(&data), Err(Error::MissingSupplementalFile(value)) if value == index)
        );
    }
}
