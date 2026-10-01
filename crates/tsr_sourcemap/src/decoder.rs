use tsr_jsstring::JsString;

use crate::generator::{NameIndex, SourceIndex};
use crate::{Error, Utf16Offset};

/// One decoded segment. The pinned decoder hands out arena-allocated
/// pointers; a mapping is a small value here.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Mapping {
    pub generated_line: isize,
    pub generated_character: Utf16Offset,
    pub source_index: SourceIndex,
    pub source_line: isize,
    pub source_character: Utf16Offset,
    pub name_index: NameIndex,
}

impl Mapping {
    // port: tsc/internal/sourcemap/decoder.go:Mapping.Equals
    pub fn equals(&self, other: &Mapping) -> bool {
        std::ptr::eq(self, other)
            || self.generated_line == other.generated_line
                && self.generated_character == other.generated_character
                && self.source_index == other.source_index
                && self.source_line == other.source_line
                && self.source_character == other.source_character
                && self.name_index == other.name_index
    }

    // port: tsc/internal/sourcemap/decoder.go:Mapping.IsSourceMapping
    pub fn is_source_mapping(&self) -> bool {
        self.source_index != MISSING_SOURCE
            && self.source_line != MISSING_LINE_OR_COLUMN
            && self.source_character != MISSING_UTF16_COLUMN
    }
}

pub const MISSING_SOURCE: SourceIndex = -1;
pub const MISSING_NAME: NameIndex = -1;
pub const MISSING_LINE_OR_COLUMN: isize = -1;
pub const MISSING_UTF16_COLUMN: Utf16Offset = -1;

/// Walks a `mappings` string, yielding one [`Mapping`] per segment. The
/// iteration stops at the end or at the first malformed segment; [`error`]
/// then tells the two apart.
///
/// [`error`]: MappingsDecoder::error
#[derive(Clone, Debug)]
pub struct MappingsDecoder {
    mappings: JsString,
    done: bool,
    pos: usize,
    generated_line: isize,
    generated_character: Utf16Offset,
    source_index: SourceIndex,
    source_line: isize,
    source_character: Utf16Offset,
    name_index: NameIndex,
    error: Option<Error>,
}

// port: tsc/internal/sourcemap/decoder.go:DecodeMappings
pub fn decode_mappings(mappings: JsString) -> MappingsDecoder {
    MappingsDecoder {
        mappings,
        done: false,
        pos: 0,
        generated_line: 0,
        generated_character: 0,
        source_index: 0,
        source_line: 0,
        source_character: 0,
        name_index: 0,
        error: None,
    }
}

impl MappingsDecoder {
    // port: tsc/internal/sourcemap/decoder.go:MappingsDecoder.MappingsString
    pub fn mappings_string(&self) -> &[u8] {
        self.mappings.as_bytes()
    }

    /// The byte offset into the mappings string.
    // port: tsc/internal/sourcemap/decoder.go:MappingsDecoder.Pos
    pub fn pos(&self) -> isize {
        self.pos as isize
    }

    // port: tsc/internal/sourcemap/decoder.go:MappingsDecoder.Error
    pub fn error(&self) -> Option<&Error> {
        self.error.as_ref()
    }

    /// The accumulated decoder state, with every field present.
    // port: tsc/internal/sourcemap/decoder.go:MappingsDecoder.State
    pub fn state(&self) -> Mapping {
        self.capture_mapping(true /*hasSource*/, true /*hasName*/)
    }

    /// The remaining mappings. Stopping early leaves the decoder where the
    /// consumer stopped, as breaking out of the pinned sequence does.
    // port: tsc/internal/sourcemap/decoder.go:MappingsDecoder.Values
    pub fn values(&mut self) -> impl Iterator<Item = Mapping> + '_ {
        std::iter::from_fn(move || self.next())
    }

    // port: tsc/internal/sourcemap/decoder.go:MappingsDecoder.captureMapping
    fn capture_mapping(&self, has_source: bool, has_name: bool) -> Mapping {
        Mapping {
            generated_line: self.generated_line,
            generated_character: self.generated_character,
            source_index: if has_source {
                self.source_index
            } else {
                MISSING_SOURCE
            },
            source_line: if has_source {
                self.source_line
            } else {
                MISSING_LINE_OR_COLUMN
            },
            source_character: if has_source {
                self.source_character
            } else {
                MISSING_UTF16_COLUMN
            },
            name_index: if has_name {
                self.name_index
            } else {
                MISSING_NAME
            },
        }
    }

    // port: tsc/internal/sourcemap/decoder.go:MappingsDecoder.stopIterating
    fn stop_iterating(&mut self) -> Option<Mapping> {
        self.done = true;
        None
    }

    // port: tsc/internal/sourcemap/decoder.go:MappingsDecoder.setError
    fn set_error(&mut self, err: &'static str) {
        self.error = Some(Error::new(err));
    }

    // port: tsc/internal/sourcemap/decoder.go:MappingsDecoder.setErrorAndStopIterating
    fn set_error_and_stop_iterating(&mut self, err: &'static str) -> Option<Mapping> {
        self.set_error(err);
        self.stop_iterating()
    }

    // port: tsc/internal/sourcemap/decoder.go:MappingsDecoder.hasReportedError
    fn has_reported_error(&self) -> bool {
        self.error.is_some()
    }

    // port: tsc/internal/sourcemap/decoder.go:MappingsDecoder.isSourceMappingSegmentEnd
    fn is_source_mapping_segment_end(&self) -> bool {
        let mappings = self.mappings.as_bytes();
        self.pos == mappings.len() || mappings[self.pos] == b',' || mappings[self.pos] == b';'
    }

    // port: tsc/internal/sourcemap/decoder.go:MappingsDecoder.base64VLQFormatDecode
    fn base64_vlq_format_decode(&mut self) -> isize {
        let mut more_digits = true;
        let mut shift_count: u32 = 0;
        let mut value: isize = 0;
        while more_digits {
            let mappings = self.mappings.as_bytes();
            if self.pos >= mappings.len() {
                self.set_error("Error in decoding base64VLQFormatDecode, past the mapping string");
                return -1;
            }

            // 6 digit number
            let current_byte = base64_format_decode(mappings[self.pos]);
            if current_byte == -1 {
                self.set_error("Invalid character in VLQ");
                return -1;
            }

            // If msb is set, we still have more bits to continue
            more_digits = (current_byte & 32) != 0;

            // least significant 5 bits are the next msbs in the final value.
            // Go's shift by 64 or more yields 0 rather than panicking.
            value |= (current_byte & 31).checked_shl(shift_count).unwrap_or(0);
            shift_count = shift_count.saturating_add(5);
            self.pos += 1;
        }

        // Least significant bit if 1 represents negative and rest of the msb is actual absolute value
        if (value & 1) == 0 {
            // + number
            value >>= 1;
        } else {
            // - number
            value >>= 1;
            value = value.wrapping_neg();
        }

        value
    }
}

impl Iterator for MappingsDecoder {
    type Item = Mapping;

    /// The next mapping, or `None` once the string is consumed or a segment
    /// is malformed (Go's `done`).
    // port: tsc/internal/sourcemap/decoder.go:MappingsDecoder.Next
    fn next(&mut self) -> Option<Mapping> {
        while !self.done && self.pos < self.mappings.len() {
            let ch = self.mappings.as_bytes()[self.pos];
            if ch == b';' {
                // new line
                self.generated_line = self.generated_line.wrapping_add(1);
                self.generated_character = 0;
                self.pos += 1;
                continue;
            }

            if ch == b',' {
                // Next entry is on same line - no action needed
                self.pos += 1;
                continue;
            }

            let mut has_source = false;
            let mut has_name = false;
            self.generated_character = self
                .generated_character
                .wrapping_add(self.base64_vlq_format_decode());
            if self.has_reported_error() {
                return self.stop_iterating();
            }
            if self.generated_character < 0 {
                return self.set_error_and_stop_iterating("Invalid generatedCharacter found");
            }

            if !self.is_source_mapping_segment_end() {
                has_source = true;

                self.source_index = self
                    .source_index
                    .wrapping_add(self.base64_vlq_format_decode());
                if self.has_reported_error() {
                    return self.stop_iterating();
                }
                if self.source_index < 0 {
                    return self.set_error_and_stop_iterating("Invalid sourceIndex found");
                }
                if self.is_source_mapping_segment_end() {
                    return self.set_error_and_stop_iterating(
                        "Unsupported Format: No entries after sourceIndex",
                    );
                }

                self.source_line = self
                    .source_line
                    .wrapping_add(self.base64_vlq_format_decode());
                if self.has_reported_error() {
                    return self.stop_iterating();
                }
                if self.source_line < 0 {
                    return self.set_error_and_stop_iterating("Invalid sourceLine found");
                }
                if self.is_source_mapping_segment_end() {
                    return self.set_error_and_stop_iterating(
                        "Unsupported Format: No entries after sourceLine",
                    );
                }

                self.source_character = self
                    .source_character
                    .wrapping_add(self.base64_vlq_format_decode());
                if self.has_reported_error() {
                    return self.stop_iterating();
                }
                if self.source_character < 0 {
                    return self.set_error_and_stop_iterating("Invalid sourceCharacter found");
                }

                if !self.is_source_mapping_segment_end() {
                    has_name = true;
                    self.name_index = self
                        .name_index
                        .wrapping_add(self.base64_vlq_format_decode());
                    if self.has_reported_error() {
                        return self.stop_iterating();
                    }
                    if self.name_index < 0 {
                        return self.set_error_and_stop_iterating("Invalid nameIndex found");
                    }

                    if !self.is_source_mapping_segment_end() {
                        return self.set_error_and_stop_iterating(
                            "Unsupported Error Format: Entries after nameIndex",
                        );
                    }
                }
            }

            return Some(self.capture_mapping(has_source, has_name));
        }

        self.stop_iterating()
    }
}

// port: tsc/internal/sourcemap/decoder.go:base64FormatDecode
fn base64_format_decode(ch: u8) -> isize {
    match ch {
        b'A'..=b'Z' => isize::from(ch - b'A'),
        b'a'..=b'z' => isize::from(ch - b'a' + 26),
        b'0'..=b'9' => isize::from(ch - b'0' + 52),
        b'+' => 62,
        b'/' => 63,
        _ => -1,
    }
}
