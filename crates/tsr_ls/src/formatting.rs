//! LSP formatting over the production formatter and exact source projections.
use crate::{syntax::Syntax, LanguageService, Result, Script};
use tsr_ast::{span_map, NodeId, SpanSegment};
use tsr_core::{TextChange, TextRange};
use tsr_format::{FormatCodeSettings, FormatContext, FormatFile};
use tsr_lsproto as lsp;

// port: tsc/internal/ls/lsutil/formatcodeoptions.go:FromLSFormatOptions
fn settings(base: &FormatCodeSettings, options: &lsp::FormattingOptions) -> FormatCodeSettings {
    let mut result = base.clone();
    result.editor.tab_size = i64::from(options.tab_size);
    result.editor.indent_size = i64::from(options.tab_size);
    result.editor.convert_tabs_to_spaces = options.insert_spaces.into();
    if let Some(trim) = options.trim_trailing_whitespace.as_deref() {
        result.editor.trim_trailing_whitespace = (*trim).into();
    }
    result
}

struct MappedRange {
    source: NodeId,
    segment: SpanSegment,
    range: TextRange,
}

// port: tsc/internal/ls/format.go:nonOverlappingFormattingRanges
fn non_overlapping(mut candidates: Vec<MappedRange>) -> Vec<MappedRange> {
    candidates.sort_by_key(|c| (c.range.pos(), std::cmp::Reverse(c.range.end())));
    let mut result: Vec<MappedRange> = Vec::new();
    for mut candidate in candidates {
        if let Some(previous) = result.last() {
            candidate.range = TextRange::new(
                candidate.range.pos().max(previous.range.end()),
                candidate.range.end(),
            );
        }
        if candidate.range.pos() < candidate.range.end() {
            result.push(candidate);
        }
    }
    result
}

impl LanguageService<'_> {
    // port: tsc/internal/ls/format.go:LanguageService.toLSProtoTextEdits
    fn formatting_edits(
        &mut self,
        source: NodeId,
        changes: Vec<TextChange>,
    ) -> Result<lsp::TextEditsOrNull> {
        let mut edits = Vec::with_capacity(changes.len());
        for change in changes {
            self.check_canceled()?;
            let (range, fidelity) = self.unrestricted_range(source, change.range)?;
            if !fidelity.is_exact() {
                return Ok(lsp::TextEditsOrNull::default());
            }
            // Formatter edits contain only whitespace, line endings and punctuation.
            edits.push(Some(Box::new(lsp::TextEdit {
                range,
                new_text: String::from_utf8(change.new_text).expect("formatter emits UTF-8"),
            })));
        }
        Ok(lsp::TextEditsOrNull {
            text_edits: Some(Box::new(edits)),
        })
    }

    fn format_changes(
        &self,
        source: NodeId,
        options: &FormatCodeSettings,
        range: Option<TextRange>,
        key: Option<(i64, &str)>,
    ) -> Result<Vec<TextChange>> {
        self.check_canceled()?;
        let mut syntax = Syntax::new(self.view(source)?, source)?;
        if let Some((position, _)) = key {
            if syntax.in_comment(position)? {
                return Ok(Vec::new());
            }
        }
        let context = FormatContext::new(options.clone(), &options.editor.new_line_character);
        let mut file = FormatFile {
            view: syntax.view,
            source,
            jsdoc: &mut syntax.docs,
        };
        let result = match key {
            Some((pos, "{")) => tsr_format::format_on_opening_curly(&mut file, &context, pos),
            Some((pos, "}")) => tsr_format::format_on_closing_curly(&mut file, &context, pos),
            Some((pos, ";")) => tsr_format::format_on_semicolon(&mut file, &context, pos),
            Some((pos, "\n")) => tsr_format::format_on_enter(&mut file, &context, pos),
            Some(_) => Ok(Vec::new()),
            None => match range {
                Some(range) => {
                    tsr_format::format_selection(&mut file, &context, range.pos(), range.end())
                }
                None => tsr_format::format_document(&mut file, &context),
            },
        }?;
        self.check_canceled()?;
        Ok(result)
    }

    // port: tsc/internal/ls/format.go:LanguageService.getFormattingEditsForMappedRange
    fn format_mapped(
        &mut self,
        source: NodeId,
        options: &FormatCodeSettings,
        range: TextRange,
    ) -> Result<lsp::TextEditsOrNull> {
        let mut sources = vec![source];
        sources.extend(
            self.source(source)?
                .supplemental_source_files()?
                .iter()
                .flatten()
                .copied(),
        );
        let mut candidates = Vec::new();
        for source in sources {
            let file = self.source(source)?;
            let Some(map) = file.span_map() else { continue };
            for segment in map.segments() {
                if segment.kind != span_map::KIND_VERBATIM
                    || segment.features & span_map::FEATURE_FORMATTING == 0
                {
                    continue;
                }
                let start = range.pos().max(i64::from(segment.original_start));
                let end = range.end().min(i64::from(segment.original_end));
                if start < end {
                    candidates.push(MappedRange {
                        source,
                        segment: *segment,
                        range: TextRange::new(start, end),
                    });
                }
            }
        }
        let mut edits = Vec::new();
        for candidate in non_overlapping(candidates) {
            let shift = i64::from(candidate.segment.virtual_start)
                - i64::from(candidate.segment.original_start);
            let range =
                TextRange::new(candidate.range.pos() + shift, candidate.range.end() + shift);
            for change in self.format_changes(candidate.source, options, Some(range), None)? {
                if change.range.pos() < range.pos() || change.range.end() > range.end() {
                    continue;
                }
                let (range, fidelity) =
                    self.range(candidate.source, change.range, span_map::FEATURE_FORMATTING)?;
                if fidelity.is_exact() {
                    edits.push(lsp::TextEdit {
                        range,
                        new_text: String::from_utf8(change.new_text)
                            .expect("formatter emits UTF-8"),
                    });
                }
            }
        }
        edits.sort_by(|a, b| {
            (
                a.range.start.line,
                a.range.start.character,
                a.range.end.line,
                a.range.end.character,
                &a.new_text,
            )
                .cmp(&(
                    b.range.start.line,
                    b.range.start.character,
                    b.range.end.line,
                    b.range.end.character,
                    &b.new_text,
                ))
        });
        Ok(lsp::TextEditsOrNull {
            text_edits: (!edits.is_empty())
                .then(|| Box::new(edits.into_iter().map(|edit| Some(Box::new(edit))).collect())),
        })
    }

    // port: tsc/internal/ls/format.go:LanguageService.ProvideFormatDocument
    pub fn format_document(
        &mut self,
        params: &lsp::DocumentFormattingParams,
        base: &FormatCodeSettings,
        enabled: bool,
    ) -> Result<lsp::TextEditsOrNull> {
        if !enabled {
            return Ok(lsp::TextEditsOrNull::default());
        }
        let source = self.file(&params.text_document.uri)?;
        let options = settings(
            base,
            params
                .options
                .as_deref()
                .expect("validated formatting options"),
        );
        let file = self.source(source)?;
        if !file.content_mapper().is_empty() {
            return self.format_mapped(
                source,
                &options,
                TextRange::new(0, file.original_text().len() as i64),
            );
        }
        let changes = self.format_changes(source, &options, None, None)?;
        self.formatting_edits(source, changes)
    }

    // port: tsc/internal/ls/format.go:LanguageService.ProvideFormatDocumentRange
    pub fn format_range(
        &mut self,
        params: &lsp::DocumentRangeFormattingParams,
        base: &FormatCodeSettings,
        enabled: bool,
    ) -> Result<lsp::TextEditsOrNull> {
        if !enabled {
            return Ok(lsp::TextEditsOrNull::default());
        }
        let source = self.file(&params.text_document.uri)?;
        let options = settings(
            base,
            params
                .options
                .as_deref()
                .expect("validated formatting options"),
        );
        let file = self.source(source)?;
        if !file.content_mapper().is_empty() {
            let original = file.original_file_name()?;
            let script = Script {
                file_name: file.file_name(),
                text: file.text().as_bytes(),
                original_file_name: original.as_bytes(),
                original_text: file.original_text(),
                span_map: file.span_map(),
            };
            let range = self
                .converters
                .from_lsp_range_to_original(&script, &params.range);
            return self.format_mapped(source, &options, range);
        }
        let ranges = self.converters.from_lsp_range_for_source_file(
            self.program,
            source,
            &params.range,
            span_map::FEATURE_FORMATTING,
        )?;
        let [projection] = ranges.as_slice() else {
            return Ok(lsp::TextEditsOrNull::default());
        };
        if !projection.mapped.fidelity.is_exact() {
            return Ok(lsp::TextEditsOrNull::default());
        }
        let changes = self.format_changes(
            projection.script,
            &options,
            Some(projection.mapped.span),
            None,
        )?;
        self.formatting_edits(projection.script, changes)
    }

    // port: tsc/internal/ls/format.go:LanguageService.ProvideFormatDocumentOnType
    pub fn format_on_type(
        &mut self,
        params: &lsp::DocumentOnTypeFormattingParams,
        base: &FormatCodeSettings,
        enabled: bool,
    ) -> Result<lsp::TextEditsOrNull> {
        if !enabled {
            return Ok(lsp::TextEditsOrNull::default());
        }
        let source = self.file(&params.text_document.uri)?;
        let options = settings(
            base,
            params
                .options
                .as_deref()
                .expect("validated formatting options"),
        );
        let positions = self.converters.from_lsp_position_for_source_file(
            self.program,
            source,
            &params.position,
            span_map::FEATURE_FORMATTING,
        )?;
        let [projection] = positions.as_slice() else {
            return Ok(lsp::TextEditsOrNull::default());
        };
        if !projection.mapped.fidelity.is_exact() {
            return Ok(lsp::TextEditsOrNull::default());
        }
        let changes = self.format_changes(
            projection.script,
            &options,
            None,
            Some((i64::from(projection.mapped.position), &params.ch)),
        )?;
        self.formatting_edits(projection.script, changes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    // source: tsc/internal/ls/format_test.go:TestNonOverlappingFormattingRanges
    #[test]
    fn formatting_ranges_sort_trim_and_prefer_the_longest() {
        let program = crate::tests::program(b"/index.ts", b"");
        let source = program.source_file(b"/index.ts").unwrap().source();
        for (candidates, expected) in [
            (vec![(10, 15), (0, 5)], vec![(0, 5), (10, 15)]),
            (vec![(0, 10), (0, 20)], vec![(0, 20)]),
            (vec![(5, 15), (0, 20)], vec![(0, 20)]),
            (vec![(5, 15), (0, 10)], vec![(0, 10), (10, 15)]),
        ] {
            let mapped = candidates
                .into_iter()
                .map(|(start, end)| MappedRange {
                    source,
                    segment: SpanSegment::default(),
                    range: TextRange::new(start, end),
                })
                .collect();
            assert_eq!(
                non_overlapping(mapped)
                    .iter()
                    .map(|r| (r.range.pos(), r.range.end()))
                    .collect::<Vec<_>>(),
                expected,
            );
        }
    }

    // source: tsc/internal/ls/format_test.go:TestGetFormattingEditsAfterKeystroke_EmptyFile
    // source: tsc/internal/ls/format_test.go:TestGetFormattingEditsAfterKeystroke_SimpleStatement
    #[test]
    fn newline_formatting_accepts_empty_files_and_unterminated_statements() {
        for text in [b"".as_slice(), b"const x = 1"] {
            let program = crate::tests::program(b"/index.ts", text);
            let source = program.source_file(b"/index.ts").unwrap().source();
            let service = LanguageService::new(
                &program,
                tsr_jsstring::PositionEncoding::Utf16,
                tsr_core::CancellationToken::new(),
            );
            service
                .format_changes(
                    source,
                    &FormatCodeSettings::default(),
                    None,
                    Some((text.len() as i64, "\n")),
                )
                .unwrap();
        }
    }

    // source: tsc/internal/ls/format_test.go:TestGetFormattingEditsForRange_FunctionBody
    #[test]
    fn formatting_function_body_ranges_uses_the_containing_node() {
        for (text, start, end) in [
            ("function foo() {\n    return (1  + 2);\n}", 21, 38),
            ("function\nf() {\n}", 9, 13),
            ("function f() {\n  \n}", 15, 17),
            ("function f() {\n}", 15, 15),
        ] {
            let program = crate::tests::program(b"/index.ts", text.as_bytes());
            let source = program.source_file(b"/index.ts").unwrap().source();
            let service = LanguageService::new(
                &program,
                tsr_jsstring::PositionEncoding::Utf16,
                tsr_core::CancellationToken::new(),
            );
            service
                .format_changes(
                    source,
                    &FormatCodeSettings::default(),
                    Some(TextRange::new(start, end)),
                    None,
                )
                .unwrap();
        }
    }
}
