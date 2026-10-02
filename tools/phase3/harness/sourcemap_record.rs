//! The harness's source-map record (`CompilationResult.GetSourceMapRecord`
//! and `sourcemap_recorder.go`): per emitted map, its header, then every
//! generated line with markers under the spans the map records, the source
//! text each span covers and the decoded positions, validated against a
//! second decoding of the same `mappings` string.
//!
//! Columns are UTF-16 code units, as the map counts them; texts and slices
//! are bytes. The pin slices the source with `ComputePositionOfLineAndUTF16
//! Character` and measures the generated line in bytes, and both are kept.
use super::{CompilationResult, Failure, TestFile};
use tsr_jsstring::JsString;
use tsr_sourcemap::{Mapping, MappingsDecoder, RawSourceMap, MISSING_NAME};

const SEPARATOR: &[u8] = b"===================================================================";
const FILE_SEPARATOR: &[u8] =
    b"-------------------------------------------------------------------";

/// Go's `writerAggregator`.
#[derive(Default)]
struct WriterAggregator {
    out: Vec<u8>,
}

impl WriterAggregator {
    fn write(&mut self, bytes: &[u8]) {
        self.out.extend_from_slice(bytes);
    }
    // source: tsc/internal/testutil/harnessutil/sourcemap_recorder.go:writerAggregator.WriteLine
    fn write_line(&mut self, bytes: &[u8]) {
        self.out.extend_from_slice(bytes);
        self.out.extend_from_slice(b"\r\n");
    }
}

/// Go's `sourceMapSpanWithDecodeErrors`.
#[derive(Clone)]
struct SpanWithDecodeErrors {
    source_map_span: Mapping,
    decode_errors: Vec<Vec<u8>>,
}

/// Go's `decodedMapping`.
struct DecodedMapping {
    source_map_span: Mapping,
    error: Option<&'static str>,
}

/// Go's `sourceMapDecoder`.
struct SourceMapDecoder {
    source_map_mappings: JsString,
    mappings: MappingsDecoder,
}

impl SourceMapDecoder {
    // source: tsc/internal/testutil/harnessutil/sourcemap_recorder.go:newSourceMapDecoder
    fn new(source_map: &RawSourceMap) -> Self {
        Self {
            source_map_mappings: source_map.mappings.clone(),
            mappings: tsr_sourcemap::decode_mappings(source_map.mappings.clone()),
        }
    }

    // source: tsc/internal/testutil/harnessutil/sourcemap_recorder.go:sourceMapDecoder.decodeNextEncodedSourceMapSpan
    fn decode_next_encoded_source_map_span(&mut self) -> DecodedMapping {
        match self.mappings.next() {
            Some(value) => DecodedMapping {
                source_map_span: value,
                error: None,
            },
            None => DecodedMapping {
                error: Some(
                    self.mappings
                        .error()
                        .map_or("No encoded entry found", tsr_sourcemap::Error::message),
                ),
                source_map_span: self.mappings.state(),
            },
        }
    }

    // source: tsc/internal/testutil/harnessutil/sourcemap_recorder.go:sourceMapDecoder.hasCompletedDecoding
    fn has_completed_decoding(&self) -> bool {
        self.mappings.pos() == self.source_map_mappings.len() as isize
    }

    // source: tsc/internal/testutil/harnessutil/sourcemap_recorder.go:sourceMapDecoder.getRemainingDecodeString
    fn remaining_decode_string(&self) -> &[u8] {
        &self.source_map_mappings.as_bytes()[self.mappings.pos() as usize..]
    }
}

/// Go's `sourceMapSpanWriter`.
struct SourceMapSpanWriter<'r, 'f> {
    recorder: &'r mut WriterAggregator,
    source_map_sources: Vec<JsString>,
    source_map_names: Vec<JsString>,
    js_file: TestFile<'f>,
    js_line_map: Vec<i32>,
    ts_code: Vec<u8>,
    ts_line_map: Vec<i32>,
    spans_on_single_line: Vec<SpanWithDecodeErrors>,
    prev_written_source_pos: isize,
    next_js_line_to_write: isize,
    span_marker_continues: bool,
    decoder: SourceMapDecoder,
}

/// An index the pin takes without a check; out of range it panics.
fn at<T>(items: &[T], index: isize, what: &str) -> Result<usize, Failure> {
    usize::try_from(index)
        .ok()
        .filter(|&i| i < items.len())
        .ok_or_else(|| {
            Failure::Runtime(format!(
                "index out of range [{index}] with length {} ({what})",
                items.len()
            ))
        })
}

impl<'r, 'f> SourceMapSpanWriter<'r, 'f> {
    // source: tsc/internal/testutil/harnessutil/sourcemap_recorder.go:newSourceMapSpanWriter
    fn new(
        recorder: &'r mut WriterAggregator,
        source_map: &RawSourceMap,
        js_file: TestFile<'f>,
    ) -> Result<Self, Failure> {
        let js_line_map = tsr_jsstring::line_map::compute_ecma_line_starts(js_file.content);
        recorder.write_line(SEPARATOR);
        recorder.write_line(&[b"JsFile: ", source_map.file.as_bytes()].concat());
        let line_info = tsr_sourcemap::create_ecma_line_info(
            JsString::from_bytes(js_file.content),
            js_line_map.clone(),
        );
        recorder.write_line(
            &[
                b"mapUrl: ".as_slice(),
                tsr_sourcemap::try_get_source_mapping_url(Some(&line_info)),
            ]
            .concat(),
        );
        recorder.write_line(&[b"sourceRoot: ", source_map.source_root.as_bytes()].concat());
        let sources: Vec<&[u8]> = source_map.sources.iter().map(JsString::as_bytes).collect();
        recorder.write_line(&[b"sources: ".as_slice(), &sources.join(b",".as_slice())].concat());
        if let Some(sources_content) = source_map
            .sources_content
            .as_ref()
            .filter(|content| !content.is_empty())
        {
            let content = tsr_json::marshal(sources_content, tsr_json::Options::default())
                .map_err(|error| {
                    Failure::Runtime(format!("json.Marshal of sourcesContent: {error}"))
                })?;
            recorder.write_line(&[b"sourcesContent: ".as_slice(), &content].concat());
        }
        recorder.write_line(SEPARATOR);
        Ok(Self {
            recorder,
            source_map_sources: source_map.sources.clone(),
            source_map_names: source_map.names.clone(),
            js_file,
            js_line_map,
            ts_code: Vec::new(),
            ts_line_map: Vec::new(),
            spans_on_single_line: Vec::new(),
            prev_written_source_pos: 0,
            next_js_line_to_write: 0,
            span_marker_continues: false,
            decoder: SourceMapDecoder::new(source_map),
        })
    }

    // source: tsc/internal/testutil/harnessutil/sourcemap_recorder.go:sourceMapSpanWriter.getSourceMapSpanString
    fn source_map_span_string(&self, map_entry: &Mapping, get_absent_name_index: bool) -> Vec<u8> {
        let mut map_string = format!(
            "Emitted({}, {})",
            map_entry.generated_line + 1,
            map_entry.generated_character + 1
        )
        .into_bytes();
        if map_entry.is_source_mapping() {
            map_string.extend(
                format!(
                    " Source({}, {}) + SourceIndex({})",
                    map_entry.source_line + 1,
                    map_entry.source_character + 1,
                    map_entry.source_index
                )
                .into_bytes(),
            );
            if map_entry.name_index >= 0
                && (map_entry.name_index as usize) < self.source_map_names.len()
            {
                map_string.extend_from_slice(b" name (");
                map_string.extend_from_slice(
                    self.source_map_names[map_entry.name_index as usize].as_bytes(),
                );
                map_string.push(b')');
            } else if map_entry.name_index != MISSING_NAME || get_absent_name_index {
                map_string.extend(format!(" nameIndex ({})", map_entry.name_index).into_bytes());
            }
        }
        map_string
    }

    // source: tsc/internal/testutil/harnessutil/sourcemap_recorder.go:sourceMapSpanWriter.recordSourceMapSpan
    fn record_source_map_span(&mut self, source_map_span: Mapping) -> Result<(), Failure> {
        // verify the decoded span is same as the new span
        let decode_result = self.decoder.decode_next_encoded_source_map_span();
        let mut decode_errors = Vec::new();
        if decode_result.error.is_some() || !decode_result.source_map_span.equals(&source_map_span)
        {
            if let Some(error) = decode_result.error {
                decode_errors.push(
                    [
                        b"!!^^ !!^^ There was decoding error in the sourcemap at this location: "
                            .as_slice(),
                        error.as_bytes(),
                    ]
                    .concat(),
                );
            } else {
                decode_errors.push(b"!!^^ !!^^ The decoded span from sourcemap's mapping entry does not match what was encoded for this span:".to_vec());
            }
            decode_errors.push(
                [
                    b"!!^^ !!^^ Decoded span from sourcemap's mappings entry: ".as_slice(),
                    &self.source_map_span_string(&decode_result.source_map_span, true),
                    b" Span encoded by the emitter:",
                    &self.source_map_span_string(&source_map_span, true),
                ]
                .concat(),
            );
        }

        if self.spans_on_single_line.first().is_some_and(|first| {
            first.source_map_span.generated_line != source_map_span.generated_line
        }) {
            // On different line from the one that we have been recording till now,
            self.write_recorded_spans()?;
            self.spans_on_single_line.clear();
        }
        self.spans_on_single_line.push(SpanWithDecodeErrors {
            source_map_span,
            decode_errors,
        });
        Ok(())
    }

    // source: tsc/internal/testutil/harnessutil/sourcemap_recorder.go:sourceMapSpanWriter.recordNewSourceFileSpan
    fn record_new_source_file_span(
        &mut self,
        source_map_span: Mapping,
        new_source_file_code: &[u8],
    ) -> Result<(), Failure> {
        let mut continues_line = false;
        // The pin compares the first span's generated character with the new
        // span's generated line ("!!! char == line seems like a bug in Strada?").
        if self.spans_on_single_line.first().is_some_and(|first| {
            first.source_map_span.generated_character == source_map_span.generated_line
        }) {
            self.write_recorded_spans()?;
            self.spans_on_single_line.clear();
            self.next_js_line_to_write -= 1; // walk back one line to reprint the line
            continues_line = true;
        }

        self.record_source_map_span(source_map_span)?;

        if self.spans_on_single_line.len() != 1 {
            return Err(Failure::Assertion("expected a single span".into()));
        }

        self.recorder.write_line(FILE_SEPARATOR);
        if continues_line {
            let mut line = b"emittedFile:".to_vec();
            line.extend_from_slice(self.js_file.unit_name);
            line.extend(
                format!(
                    " ({}, {})",
                    source_map_span.generated_line + 1,
                    source_map_span.generated_character + 1
                )
                .into_bytes(),
            );
            self.recorder.write_line(&line);
        } else {
            self.recorder
                .write_line(&[b"emittedFile:", self.js_file.unit_name].concat());
        }
        let source_index = self.spans_on_single_line[0].source_map_span.source_index;
        let source = &self.source_map_sources
            [at(&self.source_map_sources, source_index, "the map's sources")?];
        self.recorder
            .write_line(&[b"sourceFile:", source.as_bytes()].concat());
        self.recorder.write_line(FILE_SEPARATOR);

        self.ts_line_map = tsr_jsstring::line_map::compute_ecma_line_starts(new_source_file_code);
        self.ts_code = new_source_file_code.to_vec();
        self.prev_written_source_pos = 0;
        Ok(())
    }

    // source: tsc/internal/testutil/harnessutil/sourcemap_recorder.go:sourceMapSpanWriter.close
    fn close(&mut self) -> Result<(), Failure> {
        // Write the lines pending on the single line
        self.write_recorded_spans()?;

        if !self.decoder.has_completed_decoding() {
            self.recorder.write_line(
                b"!!!! **** There are more source map entries in the sourceMap's mapping than what was encoded",
            );
            let line = [
                b"!!!! **** Remaining decoded string: ".as_slice(),
                self.decoder.remaining_decode_string(),
            ]
            .concat();
            self.recorder.write_line(&line);
        }

        // write remaining js lines
        self.write_js_file_lines(self.js_line_map.len() as isize)
    }

    // source: tsc/internal/testutil/harnessutil/sourcemap_recorder.go:sourceMapSpanWriter.writeJsFileLines
    fn write_js_file_lines(&mut self, end_js_line: isize) -> Result<(), Failure> {
        while self.next_js_line_to_write < end_js_line {
            let text = text_of_line(
                self.next_js_line_to_write,
                &self.js_line_map,
                self.js_file.content,
            )?;
            self.recorder.write(b">>>");
            self.recorder.write(text);
            self.next_js_line_to_write += 1;
        }
        Ok(())
    }

    // source: tsc/internal/testutil/harnessutil/sourcemap_recorder.go:sourceMapSpanWriter.writeRecordedSpans
    fn write_recorded_spans(&mut self) -> Result<(), Failure> {
        RecordedSpanWriter {
            marker_ids: Vec::new(),
            prev_emitted_col: 0,
            w: self,
        }
        .write_recorded_spans()
    }
}

/// The line's bytes, its terminator included; the first line loses its byte
/// order mark.
// source: tsc/internal/testutil/harnessutil/sourcemap_recorder.go:sourceMapSpanWriter.getTextOfLine
fn text_of_line<'t>(line: isize, line_map: &[i32], code: &'t [u8]) -> Result<&'t [u8], Failure> {
    let start = line_map[at(line_map, line, "a line map")?] as usize;
    let end = if line + 1 < line_map.len() as isize {
        line_map[(line + 1) as usize] as usize
    } else {
        code.len()
    };
    let text = &code[start..end];
    if line == 0 {
        return Ok(tsr_jsstring::text::remove_byte_order_mark(text));
    }
    Ok(text)
}

/// Go's `recordedSpanWriter`: one generated line's spans.
struct RecordedSpanWriter<'w, 'r, 'f> {
    marker_ids: Vec<Vec<u8>>,
    prev_emitted_col: isize,
    w: &'w mut SourceMapSpanWriter<'r, 'f>,
}

#[derive(Clone, Copy)]
enum SpanStep {
    Marker,
    SourceText,
    Details,
}

impl RecordedSpanWriter<'_, '_, '_> {
    // source: tsc/internal/testutil/harnessutil/sourcemap_recorder.go:recordedSpanWriter.getMarkerId
    fn marker_id(&self, marker_index: usize) -> Result<Vec<u8>, Failure> {
        if self.w.span_marker_continues {
            if marker_index != 0 {
                return Err(Failure::Assertion("expected markerIndex to be 0".into()));
            }
            return Ok(b"1->".to_vec());
        }
        let mut marker_id = (marker_index + 1).to_string().into_bytes();
        if marker_id.len() < 2 {
            marker_id.push(b' ');
        }
        marker_id.push(b'>');
        Ok(marker_id)
    }

    // source: tsc/internal/testutil/harnessutil/sourcemap_recorder.go:recordedSpanWriter.iterateSpans
    fn iterate_spans(&mut self, step: SpanStep) -> Result<(), Failure> {
        self.prev_emitted_col = 0;
        for i in 0..self.w.spans_on_single_line.len() {
            let current_span = self.w.spans_on_single_line[i].clone();
            match step {
                SpanStep::Marker => self.write_source_map_marker(&current_span, i)?,
                SpanStep::SourceText => self.write_source_map_source_text(&current_span, i)?,
                SpanStep::Details => self.write_span_details(&current_span, i),
            }
            self.prev_emitted_col = self.w.spans_on_single_line[i]
                .source_map_span
                .generated_character;
        }
        Ok(())
    }

    // source: tsc/internal/testutil/harnessutil/sourcemap_recorder.go:recordedSpanWriter.writeSourceMapIndent
    fn write_source_map_indent(&mut self, indent_length: isize, indent_prefix: &[u8]) {
        self.w.recorder.write(indent_prefix);
        for _ in 0..indent_length.max(0) {
            self.w.recorder.write(b" ");
        }
    }

    // source: tsc/internal/testutil/harnessutil/sourcemap_recorder.go:recordedSpanWriter.writeSourceMapMarker
    fn write_source_map_marker(
        &mut self,
        current_span: &SpanWithDecodeErrors,
        index: usize,
    ) -> Result<(), Failure> {
        self.write_source_map_marker_ex(
            index,
            current_span.source_map_span.generated_character,
            false,
        )
    }

    // source: tsc/internal/testutil/harnessutil/sourcemap_recorder.go:recordedSpanWriter.writeSourceMapMarkerEx
    fn write_source_map_marker_ex(
        &mut self,
        index: usize,
        end_column: isize,
        end_continues: bool,
    ) -> Result<(), Failure> {
        let marker_id = self.marker_id(index)?;
        self.marker_ids.push(marker_id.clone());
        self.write_source_map_indent(self.prev_emitted_col, &marker_id);
        for _ in self.prev_emitted_col..end_column {
            self.w.recorder.write(b"^");
        }
        if end_continues {
            self.w.recorder.write(b"->");
        }
        self.w.recorder.write_line(b"");
        self.w.span_marker_continues = end_continues;
        Ok(())
    }

    // source: tsc/internal/testutil/harnessutil/sourcemap_recorder.go:recordedSpanWriter.writeSourceMapSourceText
    fn write_source_map_source_text(
        &mut self,
        current_span: &SpanWithDecodeErrors,
        index: usize,
    ) -> Result<(), Failure> {
        // Convert UTF-16 character offset from the source map to a byte position.
        if self.w.ts_line_map.is_empty() {
            // ComputePositionOfLineAndUTF16Character indexes lineStarts[0].
            return Err(Failure::Runtime(
                "index out of range [0] with length 0 (no source file recorded before a span)"
                    .into(),
            ));
        }
        let source_pos =
            tsr_jsstring::scanner_positions::compute_position_of_line_and_utf16_character(
                &self.w.ts_line_map,
                current_span.source_map_span.source_line,
                current_span.source_map_span.source_character,
                &self.w.ts_code,
                true, /*allowEdits*/
            );
        let source_text = if self.w.prev_written_source_pos < source_pos {
            // Position that goes forward, get text
            self.w.ts_code[self.w.prev_written_source_pos as usize..source_pos as usize].to_vec()
        } else {
            Vec::new()
        };

        // If there are decode errors, write
        let marker = self.marker_ids[index].clone();
        for decode_error in &current_span.decode_errors {
            self.write_source_map_indent(self.prev_emitted_col, &marker);
            self.w.recorder.write_line(decode_error);
        }

        let ts_code_line_map = tsr_jsstring::line_map::compute_ecma_line_starts(&source_text);
        for i in 0..ts_code_line_map.len() {
            if i == 0 {
                self.write_source_map_indent(self.prev_emitted_col, &marker);
            } else {
                self.write_source_map_indent(self.prev_emitted_col, b"  >");
            }
            let line = text_of_line(i as isize, &ts_code_line_map, &source_text)?;
            self.w.recorder.write(line);
            if i == ts_code_line_map.len() - 1 {
                self.w.recorder.write_line(b"");
            }
        }

        self.w.prev_written_source_pos = source_pos;
        Ok(())
    }

    // source: tsc/internal/testutil/harnessutil/sourcemap_recorder.go:recordedSpanWriter.writeSpanDetails
    fn write_span_details(&mut self, current_span: &SpanWithDecodeErrors, index: usize) {
        let details = self
            .w
            .source_map_span_string(&current_span.source_map_span, false);
        let line = [self.marker_ids[index].as_slice(), &details].concat();
        self.w.recorder.write_line(&line);
    }

    // source: tsc/internal/testutil/harnessutil/sourcemap_recorder.go:recordedSpanWriter.writeRecordedSpans
    fn write_recorded_spans(&mut self) -> Result<(), Failure> {
        let Some(first) = self.w.spans_on_single_line.first() else {
            return Ok(());
        };
        let current_js_line = first.source_map_span.generated_line;

        // Write js line
        self.w.write_js_file_lines(current_js_line + 1)?;

        // Emit markers
        self.iterate_spans(SpanStep::Marker)?;

        // The pin reads the next line here ("TODO: Strada is wrong here, we
        // should be looking at `currentJsLine`"), and measures it in bytes.
        let js_file_text_len = text_of_line(
            current_js_line + 1,
            &self.w.js_line_map,
            self.w.js_file.content,
        )?
        .len() as isize;
        if self.prev_emitted_col < js_file_text_len - 1 {
            // There is remaining text on this line that will be part of next source span so write marker that continues
            let count = self.w.spans_on_single_line.len();
            self.write_source_map_marker_ex(count, js_file_text_len - 1, true)?;
        }

        // Emit Source text
        self.iterate_spans(SpanStep::SourceText)?;

        // Emit column number etc
        self.iterate_spans(SpanStep::Details)?;

        self.w.recorder.write_line(b"---");
        Ok(())
    }
}

/// The source-map record of a compilation: empty when the emit produced no
/// source map.
// source: tsc/internal/testutil/harnessutil/harnessutil.go:CompilationResult.GetSourceMapRecord
pub fn get_source_map_record(c: &CompilationResult<'_>) -> Result<Vec<u8>, Failure> {
    let Some(source_maps) = c.source_maps.as_ref().filter(|maps| !maps.is_empty()) else {
        return Ok(Vec::new());
    };

    let mut recorder = WriterAggregator::default();
    for source_map_data in source_maps {
        // Go's *ast.SourceFile identity: a file's path in the program.
        let mut prev_source_file: Option<Vec<u8>> = None;

        let current_file = if tsr_tspath::is_declaration_file_name(&source_map_data.generated_file)
        {
            c.dts.get(&source_map_data.generated_file)
        } else {
            c.js.get(&source_map_data.generated_file)
        }
        .copied()
        .ok_or_else(|| {
            Failure::Runtime(format!(
                "nil pointer dereference: no output {} for a source map",
                String::from_utf8_lossy(&source_map_data.generated_file)
            ))
        })?;

        let mut writer =
            SourceMapSpanWriter::new(&mut recorder, &source_map_data.source_map, current_file)?;
        let mut mapper =
            tsr_sourcemap::decode_mappings(source_map_data.source_map.mappings.clone());
        for decoded_source_mapping in mapper.values() {
            if !decoded_source_mapping.is_source_mapping() {
                writer.record_source_map_span(decoded_source_mapping)?;
                continue;
            }
            let names = &source_map_data.input_source_file_names;
            let name = &names[at(
                names,
                decoded_source_mapping.source_index,
                "InputSourceFileNames",
            )?];
            let current_source_file = c.program.source_file(name.as_bytes());
            let current_identity = current_source_file.map(|file| file.path.to_vec());
            // The pin's branches, the equal case first.
            if current_identity == prev_source_file {
                writer.record_source_map_span(decoded_source_mapping)?;
            } else {
                if let Some(current_source_file) = current_source_file {
                    writer.record_new_source_file_span(
                        decoded_source_mapping,
                        current_source_file.original_text,
                    )?;
                }
                prev_source_file = current_identity;
            }
        }
        writer.close()?;
    }
    Ok(recorder.out)
}
