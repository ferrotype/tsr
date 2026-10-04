//! Coordinate and diagnostic conversion for one immutable request snapshot.
use std::collections::HashMap;
use tsr_ast::{
    span_map::{Fidelity, MappedPosition, MappedSpan, SpanMap},
    Diagnostic,
};
use tsr_compiler::diagnostic_writer::{DiagnosticSources, DiagnosticWriter, FormattingOptions};
use tsr_core::TextRange;
use tsr_jsstring::lsp::{lsp_line_and_character_to_position, lsp_position_to_line_and_character};
use tsr_jsstring::{LspLineMap, LspPosition, PositionEncoding};
use tsr_lsproto as lsp;

pub struct Script<'a> {
    pub file_name: &'a [u8],
    pub text: &'a [u8],
    pub original_file_name: &'a [u8],
    pub original_text: &'a [u8],
    pub span_map: Option<&'a SpanMap>,
}
/// The source identity is retained with every projection, including
/// supplemental virtual files which share the same original filename.
pub struct Projection<T> {
    pub script: tsr_ast::NodeId,
    pub mapped: T,
}
impl<'a> Script<'a> {
    pub fn plain(file_name: &'a [u8], text: &'a [u8]) -> Self {
        Self {
            file_name,
            text,
            original_file_name: file_name,
            original_text: text,
            span_map: None,
        }
    }
    fn original(&self) -> Self {
        Self::plain(self.original_file_name, self.original_text)
    }
}

/// Line maps are request-local: the caller must use a new converter for a new
/// snapshot, so no path can accidentally retain a previous document's map.
pub struct Converters {
    encoding: PositionEncoding,
    lines: HashMap<Vec<u8>, LspLineMap>,
}
impl Converters {
    // port: tsc/internal/ls/lsconv/converters.go:NewConverters
    pub fn new(encoding: PositionEncoding) -> Self {
        Self {
            encoding,
            lines: HashMap::new(),
        }
    }
    fn position(&mut self, script: &Script<'_>, offset: i32) -> lsp::Position {
        let map = self
            .lines
            .entry(script.file_name.to_vec())
            .or_insert_with(|| LspLineMap::new(script.text));
        let p = lsp_position_to_line_and_character(script.text, map, offset, self.encoding);
        lsp::Position {
            line: p.line,
            character: p.character,
        }
    }
    fn offset(&mut self, script: &Script<'_>, position: &lsp::Position) -> i32 {
        let map = self
            .lines
            .entry(script.file_name.to_vec())
            .or_insert_with(|| LspLineMap::new(script.text));
        lsp_line_and_character_to_position(
            script.text,
            map,
            LspPosition {
                line: position.line,
                character: position.character,
            },
            self.encoding,
        )
    }
    // port: tsc/internal/ls/lsconv/converters.go:Converters.ToLSPRange
    pub fn to_lsp_range(
        &mut self,
        script: &Script<'_>,
        range: TextRange,
    ) -> (lsp::Range, Fidelity) {
        self.range(script, range, None)
    }
    // port: tsc/internal/ls/lsconv/converters.go:Converters.ToLSPRangeForFeature
    pub fn to_lsp_range_for_feature(
        &mut self,
        script: &Script<'_>,
        range: TextRange,
        feature: i32,
    ) -> (lsp::Range, Fidelity) {
        self.range(script, range, Some(feature))
    }
    fn range(
        &mut self,
        script: &Script<'_>,
        range: TextRange,
        feature: Option<i32>,
    ) -> (lsp::Range, Fidelity) {
        let (range, fidelity) = match (script.span_map, feature) {
            (Some(map), Some(feature)) => map.virtual_to_original_span_for_feature(range, feature),
            (Some(map), None) => map.virtual_to_original_span(range),
            (None, _) => (range, Fidelity::Exact),
        };
        let original = script.original();
        let script = if script.span_map.is_some() {
            &original
        } else {
            script
        };
        (
            lsp::Range {
                start: self.position(script, range.pos() as i32),
                end: self.position(script, range.end() as i32),
            },
            fidelity,
        )
    }
    // port: tsc/internal/ls/lsconv/converters.go:Converters.ToLSPPosition
    pub fn to_lsp_position(
        &mut self,
        script: &Script<'_>,
        position: i32,
    ) -> (lsp::Position, Fidelity) {
        self.mapped_position(script, position, None)
    }
    // port: tsc/internal/ls/lsconv/converters.go:Converters.ToLSPPositionForFeature
    pub fn to_lsp_position_for_feature(
        &mut self,
        script: &Script<'_>,
        position: i32,
        feature: i32,
    ) -> (lsp::Position, Fidelity) {
        self.mapped_position(script, position, Some(feature))
    }
    fn mapped_position(
        &mut self,
        script: &Script<'_>,
        position: i32,
        feature: Option<i32>,
    ) -> (lsp::Position, Fidelity) {
        let (position, fidelity) = match (script.span_map, feature) {
            (Some(map), Some(feature)) => {
                map.virtual_to_original_position_for_feature(position, feature)
            }
            (Some(map), None) => map.virtual_to_original_position(position),
            (None, _) => (position, Fidelity::Exact),
        };
        let original = script.original();
        (
            self.position(
                if script.span_map.is_some() {
                    &original
                } else {
                    script
                },
                position,
            ),
            fidelity,
        )
    }
    // port: tsc/internal/ls/lsconv/converters.go:Converters.ToLSPLocation
    pub fn to_lsp_location(
        &mut self,
        script: &Script<'_>,
        range: TextRange,
    ) -> (lsp::Location, Fidelity) {
        let (range, fidelity) = self.to_lsp_range(script, range);
        (
            lsp::Location {
                uri: lsp::DocumentUri::from_file_name(script.original_file_name),
                range,
            },
            fidelity,
        )
    }
    // port: tsc/internal/ls/lsconv/converters.go:Converters.ToLSPLocationForFeature
    pub fn to_lsp_location_for_feature(
        &mut self,
        script: &Script<'_>,
        range: TextRange,
        feature: i32,
    ) -> (lsp::Location, Fidelity) {
        let (range, fidelity) = self.to_lsp_range_for_feature(script, range, feature);
        (
            lsp::Location {
                uri: lsp::DocumentUri::from_file_name(script.original_file_name),
                range,
            },
            fidelity,
        )
    }
    // port: tsc/internal/ls/lsconv/converters.go:FromLSPRangeToOriginal
    pub fn from_lsp_range_to_original(
        &mut self,
        script: &Script<'_>,
        range: &lsp::Range,
    ) -> TextRange {
        let original = script.original();
        TextRange::new(
            i64::from(self.offset(&original, &range.start)),
            i64::from(self.offset(&original, &range.end)),
        )
    }
    // port: tsc/internal/ls/lsconv/converters.go:FromLSPRange
    pub fn from_lsp_range(
        &mut self,
        script: &Script<'_>,
        range: &lsp::Range,
        feature: i32,
    ) -> Vec<MappedSpan> {
        let range = self.from_lsp_range_to_original(script, range);
        script.span_map.map_or_else(
            || {
                vec![MappedSpan {
                    span: range,
                    fidelity: Fidelity::Exact,
                }]
            },
            |map| map.original_to_virtual_spans(range, feature),
        )
    }
    // port: tsc/internal/ls/lsconv/converters.go:FromLSPPosition
    pub fn from_lsp_position(
        &mut self,
        script: &Script<'_>,
        position: &lsp::Position,
        feature: i32,
    ) -> Vec<MappedPosition> {
        let position = self.offset(&script.original(), position);
        script.span_map.map_or_else(
            || {
                vec![MappedPosition {
                    position,
                    fidelity: Fidelity::Exact,
                }]
            },
            |map| map.original_to_virtual_positions(position, feature),
        )
    }
    /// Convert the feature-enabled intersections of one virtual script.
    pub fn from_lsp_range_intersecting(
        &mut self,
        script: &Script<'_>,
        range: &lsp::Range,
        feature: i32,
    ) -> Vec<MappedSpan> {
        let range = self.from_lsp_range_to_original(script, range);
        script.span_map.map_or_else(
            || {
                vec![MappedSpan {
                    span: range,
                    fidelity: Fidelity::Exact,
                }]
            },
            |map| map.original_to_virtual_intersecting_spans(range, feature),
        )
    }

    // port: tsc/internal/ls/lsconv/converters.go:FromLSPPositionForSourceFile
    pub fn from_lsp_position_for_source_file(
        &mut self,
        sources: &dyn DiagnosticSources,
        file: tsr_ast::NodeId,
        position: &lsp::Position,
        feature: i32,
    ) -> Result<Vec<Projection<MappedPosition>>, tsr_compiler::Error> {
        self.project(sources, file, |c, script| {
            c.from_lsp_position(script, position, feature)
        })
    }
    // port: tsc/internal/ls/lsconv/converters.go:FromLSPRangeForSourceFile
    pub fn from_lsp_range_for_source_file(
        &mut self,
        sources: &dyn DiagnosticSources,
        file: tsr_ast::NodeId,
        range: &lsp::Range,
        feature: i32,
    ) -> Result<Vec<Projection<MappedSpan>>, tsr_compiler::Error> {
        self.project(sources, file, |c, script| {
            c.from_lsp_range(script, range, feature)
        })
    }
    // port: tsc/internal/ls/lsconv/converters.go:FromLSPRangeIntersectingForSourceFile
    pub fn from_lsp_range_intersecting_for_source_file(
        &mut self,
        sources: &dyn DiagnosticSources,
        file: tsr_ast::NodeId,
        range: &lsp::Range,
        feature: i32,
    ) -> Result<Vec<Projection<MappedSpan>>, tsr_compiler::Error> {
        self.project(sources, file, |c, script| {
            c.from_lsp_range_intersecting(script, range, feature)
        })
    }
    fn project<T>(
        &mut self,
        sources: &dyn DiagnosticSources,
        file: tsr_ast::NodeId,
        mut convert: impl FnMut(&mut Self, &Script<'_>) -> Vec<T>,
    ) -> Result<Vec<Projection<T>>, tsr_compiler::Error> {
        let canonical = sources.diagnostic_source(file)?;
        let ids: Vec<_> = std::iter::once(file)
            .chain(
                canonical
                    .supplemental_source_files()?
                    .iter()
                    .flatten()
                    .copied(),
            )
            .collect();
        let mut result = Vec::new();
        for id in ids {
            let source = sources.diagnostic_source(id)?;
            let name = source.original_file_name()?;
            let script = Script {
                file_name: source.file_name(),
                text: source.text().as_bytes(),
                original_file_name: name.as_bytes(),
                original_text: source.original_text(),
                span_map: source.span_map(),
            };
            result.extend(
                convert(self, &script)
                    .into_iter()
                    .map(|mapped| Projection { script: id, mapped }),
            );
        }
        Ok(result)
    }

    // port: tsc/internal/ls/lsconv/converters.go:diagnosticToLSP
    pub fn diagnostic(
        &mut self,
        sources: &dyn DiagnosticSources,
        diagnostic: &Diagnostic,
        options: &DiagnosticOptions,
    ) -> Result<lsp::Diagnostic, tsr_compiler::Error> {
        let mut severity = match diagnostic.category {
            0 => lsp::DiagnosticSeverity::WARNING,
            2 => lsp::DiagnosticSeverity::HINT,
            3 => lsp::DiagnosticSeverity::INFORMATION,
            _ => lsp::DiagnosticSeverity::ERROR,
        };
        if options.style_checks_as_warnings
            && severity == lsp::DiagnosticSeverity::ERROR
            && matches!(
                diagnostic.code,
                6196 | 6133 | 6138 | 6192 | 7027 | 7028 | 7029 | 7030
            )
        {
            severity = lsp::DiagnosticSeverity::WARNING;
        }
        let writer = DiagnosticWriter::from_sources(
            sources,
            FormattingOptions {
                locale: options.locale.clone(),
                ..Default::default()
            },
        );
        let mut related = Vec::new();
        if options.related_information {
            for d in &diagnostic.related_information {
                let file = sources.diagnostic_source(d.file.ok_or(
                    tsr_compiler::Error::Unsupported("related diagnostic without file"),
                )?)?;
                let range = self.diagnostic_range(sources, d)?;
                related.push(lsp::DiagnosticRelatedInformation {
                    location: lsp::Location {
                        uri: lsp::DocumentUri::from_file_name(
                            file.original_file_name()?.as_bytes(),
                        ),
                        range,
                    },
                    message: strict_string(
                        &DiagnosticWriter::from_sources(
                            sources,
                            FormattingOptions {
                                locale: options.locale.clone(),
                                ..Default::default()
                            },
                        )
                        .localized(d)?,
                    )?,
                });
            }
        }
        let mut tags = Vec::new();
        if diagnostic.reports_unnecessary && options.tags.contains(&lsp::DiagnosticTag::UNNECESSARY)
        {
            tags.push(lsp::DiagnosticTag::UNNECESSARY);
        }
        if diagnostic.reports_deprecated && options.tags.contains(&lsp::DiagnosticTag::DEPRECATED) {
            tags.push(lsp::DiagnosticTag::DEPRECATED);
        }
        let code = if options.visual_studio {
            lsp::IntegerOrString {
                string: Some(Box::new(format!("TS{}", diagnostic.code))),
                ..Default::default()
            }
        } else {
            lsp::IntegerOrString {
                integer: Some(Box::new(diagnostic.code)),
                ..Default::default()
            }
        };
        Ok(lsp::Diagnostic {
            range: self.diagnostic_range(sources, diagnostic)?,
            code: Some(Box::new(code)),
            severity: Some(Box::new(severity)),
            source: Some(Box::new(if diagnostic.source.is_empty() {
                "ts".into()
            } else {
                strict_string(diagnostic.source.as_bytes())?
            })),
            message: lsp::StringOrMarkupContent {
                string: Some(Box::new(message_chain(&writer, diagnostic)?)),
                ..Default::default()
            },
            related_information: (!related.is_empty())
                .then(|| Box::new(related.into_iter().map(|d| Some(Box::new(d))).collect())),
            tags: (!tags.is_empty()).then(|| Box::new(tags)),
            ..Default::default()
        })
    }
    // port: tsc/internal/ls/lsconv/converters.go:diagnosticScriptAndRange
    fn diagnostic_range(
        &mut self,
        sources: &dyn DiagnosticSources,
        d: &Diagnostic,
    ) -> Result<lsp::Range, tsr_compiler::Error> {
        let Some(id) = d.file else {
            return Ok(lsp::Range::default());
        };
        let file = sources.diagnostic_source(id)?;
        let original = file.original_text();
        let name = file.original_file_name()?;
        let (range, fidelity) = if d.source.is_empty() {
            file.span_map().map_or((d.loc, Fidelity::Exact), |map| {
                map.virtual_to_original_span(d.loc)
            })
        } else {
            (d.loc, Fidelity::Exact)
        };
        if fidelity.is_none() {
            return Ok(lsp::Range::default());
        }
        Ok(self
            .to_lsp_range(&Script::plain(name.as_bytes(), original), range)
            .0)
    }
}
#[derive(Clone, Default)]
pub struct DiagnosticOptions {
    pub locale: tsr_locale::Locale,
    pub related_information: bool,
    pub tags: Vec<lsp::DiagnosticTag>,
    pub visual_studio: bool,
    pub style_checks_as_warnings: bool,
}
fn strict_string(bytes: &[u8]) -> Result<String, tsr_compiler::Error> {
    std::str::from_utf8(bytes).map(str::to_owned).map_err(|_| {
        tsr_compiler::Error::Unsupported("invalid UTF-8 cannot cross the LSP transport (ADR 0019)")
    })
}

// port: tsc/internal/ls/lsconv/converters.go:messageChainToString
fn message_chain(
    writer: &DiagnosticWriter<'_>,
    diagnostic: &Diagnostic,
) -> Result<String, tsr_compiler::Error> {
    let mut out = Vec::new();
    let mut stack = vec![(diagnostic, 0usize)];
    while let Some((node, depth)) = stack.pop() {
        if depth > 0 {
            out.push(b'\n');
        }
        out.extend(std::iter::repeat_n(b' ', depth * 2));
        out.extend(writer.localized(node)?);
        stack.extend(
            node.message_chain
                .iter()
                .rev()
                .map(|child| (child.as_ref(), depth + 1)),
        );
    }
    strict_string(&out)
}
#[cfg(test)]
mod tests;
