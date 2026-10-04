use crate::{
    highlight_syntax::syntax_highlights,
    references::{ReferenceOptions, SearchState},
    syntax::Syntax,
    LanguageService, Result,
};
use std::collections::HashSet;
use tsr_ast::{span_map::FEATURE_DOCUMENT_HIGHLIGHTS, SyntaxKind as K};
use tsr_checker::Operation;
use tsr_core::TextRange;
use tsr_lsproto as lsp;

impl LanguageService<'_> {
    // port: tsc/internal/ls/documenthighlights.go:LanguageService.ProvideDocumentHighlights
    pub fn document_highlights(
        &mut self,
        c: &mut Operation<'_>,
        params: &lsp::DocumentHighlightParams,
    ) -> Result<lsp::DocumentHighlightsOrNull> {
        let documents = self.multi_document_highlights(
            c,
            &lsp::MultiDocumentHighlightParams {
                text_document: params.text_document.clone(),
                position: params.position.clone(),
                files_to_search: Vec::new(),
            },
        )?;
        let mut result = Vec::new();
        for document in documents
            .multi_document_highlights
            .into_iter()
            .flat_map(|v| *v)
            .flatten()
        {
            if document.uri == params.text_document.uri {
                result.extend(document.highlights);
            }
        }
        Ok(lsp::DocumentHighlightsOrNull {
            document_highlights: Some(Box::new(result)),
        })
    }
    // port: tsc/internal/ls/documenthighlights.go:LanguageService.ProvideMultiDocumentHighlights
    pub fn multi_document_highlights(
        &mut self,
        c: &mut Operation<'_>,
        params: &lsp::MultiDocumentHighlightParams,
    ) -> Result<lsp::MultiDocumentHighlightsOrNull> {
        let source = self.file(&params.text_document.uri)?;
        let positions = self.converters.from_lsp_position_for_source_file(
            self.program,
            source,
            &params.position,
            FEATURE_DOCUMENT_HIGHLIGHTS,
        )?;
        let mut result: Vec<Option<Box<lsp::MultiDocumentHighlight>>> = Vec::new();
        let mut seen = HashSet::new();
        for position in positions {
            self.check_canceled()?;
            if !position.mapped.fidelity.is_single_segment() {
                continue;
            }
            let source = position.script;
            let view = self.view(source)?;
            let mut syntax = Syntax::new(view, source)?;
            let node = syntax
                .nav()
                .get_touching_property_name(i64::from(position.mapped.position))?;
            let mut direct = None;
            if let Some(parent) = view.node(node)?.parent() {
                let p = view.node(parent)?;
                if p.kind() == K::JsxClosingElement
                    || p.kind() == K::JsxOpeningElement && p.tag_name() == Some(node)
                {
                    let mut ranges = Vec::new();
                    if let Some(element) = p.parent() {
                        if let Some(d) = view.node(element)?.data_source().as_jsx_element() {
                            for id in [d.opening_element(), d.closing_element()]
                                .into_iter()
                                .flatten()
                            {
                                ranges.push(TextRange::new(
                                    syntax.start(id)?,
                                    i64::from(view.node(id)?.end()),
                                ));
                            }
                        }
                    }
                    direct = Some(ranges);
                }
            }
            let mut documents = Vec::new();
            if direct.is_none() {
                let mut files = Vec::new();
                let mut names = HashSet::new();
                for uri in &params.files_to_search {
                    if names.insert(uri.file_name()) {
                        if let Some(f) = self.program.source_file(uri.file_name().as_bytes()) {
                            files.push(f.source());
                        }
                    }
                }
                if files.is_empty() {
                    files.push(source);
                }
                let groups = SearchState::new(self, c, files.clone(), ReferenceOptions::default())
                    .for_node(node, i64::from(position.mapped.position))?;
                for file in files {
                    let uri = lsp::DocumentUri::from_file_name(
                        self.source(file)?.original_file_name()?.as_bytes(),
                    );
                    let mut highlights = Vec::new();
                    for group in &groups {
                        for entry in &group.entries {
                            if entry.source != file {
                                continue;
                            }
                            if let Some(loc) =
                                self.entry_location(entry, FEATURE_DOCUMENT_HIGHLIGHTS)?
                            {
                                let kind = if self.entry_write(entry)? {
                                    lsp::DocumentHighlightKind::WRITE
                                } else {
                                    lsp::DocumentHighlightKind::READ
                                };
                                highlights.push(Some(Box::new(lsp::DocumentHighlight {
                                    range: loc.range,
                                    kind: Some(Box::new(kind)),
                                })));
                            }
                        }
                    }
                    if !highlights.is_empty() {
                        documents.push(lsp::MultiDocumentHighlight { uri, highlights });
                    }
                }
                if documents.is_empty() {
                    direct = Some(syntax_highlights(&mut syntax, c, node)?);
                }
            }
            if let Some(ranges) = direct {
                let mut highlights = Vec::new();
                for range in ranges {
                    let (range, fidelity) =
                        self.range(source, range, FEATURE_DOCUMENT_HIGHLIGHTS)?;
                    if !fidelity.is_none() {
                        highlights.push(Some(Box::new(lsp::DocumentHighlight {
                            range,
                            kind: Some(Box::new(lsp::DocumentHighlightKind::READ)),
                        })));
                    }
                }
                if !highlights.is_empty() {
                    documents.push(lsp::MultiDocumentHighlight {
                        uri: params.text_document.uri.clone(),
                        highlights,
                    });
                }
            }
            for document in documents {
                let index = result
                    .iter()
                    .position(|r| r.as_ref().is_some_and(|r| r.uri == document.uri))
                    .unwrap_or_else(|| {
                        result.push(Some(Box::new(lsp::MultiDocumentHighlight {
                            uri: document.uri.clone(),
                            highlights: Vec::new(),
                        })));
                        result.len() - 1
                    });
                for h in document.highlights.into_iter().flatten() {
                    let r = &h.range;
                    if seen.insert((
                        document.uri.0.clone(),
                        r.start.line,
                        r.start.character,
                        r.end.line,
                        r.end.character,
                    )) {
                        result[index].as_mut().unwrap().highlights.push(Some(h));
                    }
                }
            }
        }
        Ok(lsp::MultiDocumentHighlightsOrNull {
            multi_document_highlights: Some(Box::new(result)),
        })
    }
}
