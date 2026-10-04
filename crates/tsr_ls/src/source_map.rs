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

struct MapHost<'a>(&'a tsr_compiler::Program);
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
    pub(crate) fn source_position(&mut self, name: &[u8], pos: i64) -> Option<DocumentPosition> {
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
