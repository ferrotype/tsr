//! Declaration maps belong to a request's immutable filesystem snapshot.
//! Missing/invalid maps fall back to the declaration; a cycle never recurses.
use crate::{converters::Script, LanguageService, Result};
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};
use tsr_ast::{span_map::Fidelity, SourceFileRead};
use tsr_core::TextRange;
use tsr_jsstring::JsString;
use tsr_lsproto as lsp;
use tsr_sourcemap::{DocumentPosition, DocumentPositionMapper, EcmaLineInfo, Host};

pub(crate) type Maps = HashMap<Vec<u8>, Option<DocumentPositionMapper>>;

pub(crate) struct MapHost<'a>(pub(crate) &'a tsr_compiler::Program);
impl Host for MapHost<'_> {
    fn use_case_sensitive_file_names(&self) -> bool {
        self.0.use_case_sensitive_file_names()
    }
    fn read_file(&self, name: &[u8]) -> Option<JsString> {
        if let Some(file) = self.0.source_file(name) {
            return file
                .bound()
                .view()
                .source_file()
                .ok()
                .map(|f| JsString::from_bytes(f.text().as_bytes()));
        }
        self.0
            .host()
            .read_file(name)
            .ok()
            .flatten()
            .map(|f| JsString::from_bytes(f.text.as_bytes()))
    }
    fn get_ecma_line_info(&self, name: &[u8]) -> Option<Arc<EcmaLineInfo>> {
        let text = self.read_file(name)?;
        let starts = tsr_jsstring::line_map::compute_ecma_line_starts(text.as_bytes());
        Some(Arc::new(tsr_sourcemap::create_ecma_line_info(text, starts)))
    }
}

impl LanguageService<'_> {
    // port: tsc/internal/ls/source_map.go:LanguageService.tryGetSourcePosition
    // port: tsc/internal/ls/source_map.go:LanguageService.tryGetSourcePositionWorker
    pub fn source_position(&mut self, name: &[u8], pos: i64) -> Option<DocumentPosition> {
        let mut current = DocumentPosition {
            file_name: JsString::from_bytes(name),
            pos: pos as isize,
        };
        let mut seen = HashSet::new();
        let mut mapped = false;
        while tsr_tspath::is_declaration_file_name(current.file_name.as_bytes()) {
            if !seen.insert(current.file_name.clone()) {
                return None;
            }
            let mapper = self
                .source_maps
                .entry(current.file_name.as_bytes().to_vec())
                .or_insert_with(|| {
                    tsr_sourcemap::get_document_position_mapper(
                        &MapHost(self.program),
                        current.file_name.as_bytes(),
                    )
                });
            let Some(next) = mapper
                .as_ref()
                .and_then(|m| m.get_source_position(&current))
            else {
                break;
            };
            current = next;
            mapped = true;
        }
        (mapped
            && MapHost(self.program)
                .read_file(current.file_name.as_bytes())
                .is_some())
        .then_some(current)
    }

    /// Maps a source byte offset to its declaration output. Referenced sources
    /// already included through editor redirection stay in their source project.
    // port: tsc/internal/ls/source_map.go:LanguageService.tryGetGeneratedPosition
    // port: tsc/internal/ls/source_map.go:LanguageService.tryGetGeneratedPositionWorker
    pub fn generated_position(
        &mut self,
        name: &[u8],
        byte_position: i64,
    ) -> Option<DocumentPosition> {
        if tsr_tspath::is_declaration_file_name(name)
            || self.program.source_file(name).is_none()
            || self.program.is_source_from_project_reference(
                tsr_tspath::to_path(
                    name,
                    self.program.current_directory(),
                    self.program.use_case_sensitive_file_names(),
                )
                .as_bytes(),
            )
        {
            return None;
        }
        let declaration = self.program.declaration_output_name(name).ok()?;
        let mapper = self
            .source_maps
            .entry(declaration.clone())
            .or_insert_with(|| {
                tsr_sourcemap::get_document_position_mapper(&MapHost(self.program), &declaration)
            });
        let generated = mapper.as_ref()?.get_generated_position(&DocumentPosition {
            file_name: JsString::from_bytes(name),
            pos: byte_position as isize,
        })?;
        MapHost(self.program).read_file(generated.file_name.as_bytes())?;
        Some(generated)
    }

    // port: tsc/internal/ls/source_map.go:LanguageService.getMappedLocation
    fn mapped_location(
        &mut self,
        name: &[u8],
        text: &[u8],
        range: TextRange,
    ) -> (lsp::Location, Fidelity) {
        if let Some(start) = self.source_position(name, range.pos()) {
            let end = self
                .source_position(name, range.end())
                .filter(|p| p.file_name == start.file_name && p.pos >= start.pos)
                .map_or(start.pos as i64 + range.len(), |p| p.pos as i64);
            if let Some(text) = MapHost(self.program).read_file(start.file_name.as_bytes()) {
                let (range, fidelity) = self.converters.to_lsp_range(
                    &Script::plain(start.file_name.as_bytes(), text.as_bytes()),
                    TextRange::new(start.pos as i64, end),
                );
                return (
                    lsp::Location {
                        uri: lsp::DocumentUri::from_file_name(start.file_name.as_bytes()),
                        range,
                    },
                    fidelity,
                );
            }
        }
        let (range, fidelity) = self
            .converters
            .to_lsp_range(&Script::plain(name, text), range);
        (
            lsp::Location {
                uri: lsp::DocumentUri::from_file_name(name),
                range,
            },
            fidelity,
        )
    }

    // port: tsc/internal/ls/source_map.go:LanguageService.sourceFileRangeToLSPLocation
    // port: tsc/internal/ls/source_map.go:LanguageService.sourceFileRangeToLSPLocationForFeature
    pub(crate) fn file_location(
        &mut self,
        file: &SourceFileRead<'_>,
        range: TextRange,
        feature: Option<i32>,
    ) -> Result<(lsp::Location, Fidelity)> {
        if file.content_mapper().is_empty() {
            return Ok(self.mapped_location(file.file_name(), file.text().as_bytes(), range));
        }
        let original = file.original_file_name()?;
        let script = Script {
            file_name: file.file_name(),
            text: file.text().as_bytes(),
            original_file_name: original.as_bytes(),
            original_text: file.original_text(),
            span_map: file.span_map(),
        };
        let (range, fidelity) = match feature {
            Some(f) => self.converters.to_lsp_range_for_feature(&script, range, f),
            None => self.converters.to_lsp_range(&script, range),
        };
        Ok((
            lsp::Location {
                uri: lsp::DocumentUri::from_file_name(original.as_bytes()),
                range,
            },
            fidelity,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tsr_core::{CancellationToken, CompilerOptions, Tristate};

    #[test]
    fn generated_position_uses_declaration_dir_and_requires_loaded_source() {
        let mut fs = tsr_vfs::MemoryBuilder::new(b"/", true);
        fs.insert_loaded(b"/src/index.ts", b"export const value = 1;".as_slice());
        fs.insert_loaded(
            b"/types/index.d.ts",
            b"export declare const value: number;\n//# sourceMappingURL=index.d.ts.map".as_slice(),
        );
        fs.insert_loaded(b"/types/index.d.ts.map", br#"{"version":3,"file":"index.d.ts","sources":["../src/index.ts"],"names":[],"mappings":"qBAAa"}"#.as_slice());
        let program = tsr_compiler::Program::load(
            tsr_compiler::ProgramOptions {
                config: tsr_tsoptions::ParsedCommandLine::new(
                    CompilerOptions {
                        no_lib: Tristate::TRUE,
                        root_dir: JsString::from_bytes(b"/src".as_slice()),
                        out_dir: JsString::from_bytes(b"/out".as_slice()),
                        declaration_dir: JsString::from_bytes(b"/types".as_slice()),
                        ..Default::default()
                    },
                    vec![JsString::from_bytes(b"/src/index.ts".as_slice())],
                ),
                host: Arc::new(fs.finish()),
                current_directory: JsString::from_bytes(b"/".as_slice()),
                default_library_path: JsString::default(),
                skip_module_resolution: false,
                single_threaded: Tristate::TRUE,
            },
            &mut tsr_compiler::FileCache::new(),
            &tsr_arena::Counters::new(),
        )
        .unwrap();
        let mut service = LanguageService::new(
            &program,
            tsr_jsstring::PositionEncoding::Utf16,
            CancellationToken::new(),
        );
        let generated = service.generated_position(b"/src/index.ts", 13).unwrap();
        assert_eq!(generated.file_name.as_bytes(), b"/types/index.d.ts");
        assert_eq!(generated.pos, 21);
        let source = service.source_position(b"/types/index.d.ts", 21).unwrap();
        assert_eq!(source.file_name.as_bytes(), b"/src/index.ts");
        assert_eq!(source.pos, 13);
        assert!(service
            .generated_position(b"/types/index.d.ts", 21)
            .is_none());
        assert!(service.generated_position(b"/not-loaded.ts", 0).is_none());
    }
}
