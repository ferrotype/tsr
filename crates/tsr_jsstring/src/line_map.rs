//! ECMAScript and LSP line maps retain their distinct terminators and signed TextPos.

use crate::wtf8::decode_utf8;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LspLineMap {
    pub line_starts: Vec<i32>,
    pub ascii_only: bool,
}

impl LspLineMap {
    /// port: tsc/internal/ls/lsconv/linemap.go:ComputeLSPLineStarts
    pub fn new(text: &[u8]) -> Self {
        Self::with_source_length(text, text.len())
    }

    // Keep the narrowing conversion at the scan boundary. The separate length
    // also lets tests exercise wrapped TextPos limits without allocating GiBs.
    fn with_source_length(text: &[u8], source_length: usize) -> Self {
        let mut line_starts = Vec::with_capacity(line_capacity(text));
        let mut ascii_only = true;
        let text_len = source_length as i32;
        let mut byte_offset = 0_i32;
        let mut line_start = 0;
        while byte_offset < text_len {
            let byte = text[byte_offset as usize];
            if byte.is_ascii() {
                byte_offset = byte_offset.wrapping_add(1);
                if byte == b'\r' {
                    if byte_offset < text_len && text[byte_offset as usize] == b'\n' {
                        byte_offset = byte_offset.wrapping_add(1);
                    }
                    line_starts.push(line_start);
                    line_start = byte_offset;
                } else if byte == b'\n' {
                    line_starts.push(line_start);
                    line_start = byte_offset;
                }
            } else {
                byte_offset =
                    byte_offset.wrapping_add(decode_utf8(&text[byte_offset as usize..]).1 as i32);
                ascii_only = false;
            }
        }
        line_starts.push(line_start);
        Self {
            line_starts,
            ascii_only,
        }
    }

    /// port: tsc/internal/ls/lsconv/linemap.go:LSPLineMap.ComputeIndexOfLineStart
    pub fn compute_index_of_line_start(&self, target: i32) -> usize {
        let index = self.line_starts.partition_point(|&start| start < target);
        if self.line_starts.get(index) != Some(&target) && index > 0 {
            index - 1
        } else {
            index
        }
    }
}

/// port: tsc/internal/core/core.go:ComputeECMALineStarts
pub fn compute_ecma_line_starts(text: &[u8]) -> Vec<i32> {
    let mut starts = Vec::with_capacity(line_capacity(text));
    starts.extend(compute_ecma_line_starts_seq(text));
    starts
}

/// Includes the final line, even for empty text or a trailing terminator.
///
/// port: tsc/internal/core/core.go:ComputeECMALineStartsSeq
pub fn compute_ecma_line_starts_seq(text: &[u8]) -> impl Iterator<Item = i32> + '_ {
    ecma_line_starts_with_source_length(text, text.len())
}

fn ecma_line_starts_with_source_length(
    text: &[u8],
    source_length: usize,
) -> impl Iterator<Item = i32> + '_ {
    let text_len = source_length as i32;
    let mut byte_offset = 0_i32;
    let mut line_start = 0;
    let mut finished = false;
    std::iter::from_fn(move || {
        if finished {
            return None;
        }
        while byte_offset < text_len {
            let byte = text[byte_offset as usize];
            let line_break = if byte.is_ascii() {
                byte_offset = byte_offset.wrapping_add(1);
                if byte == b'\r' && byte_offset < text_len && text[byte_offset as usize] == b'\n' {
                    byte_offset = byte_offset.wrapping_add(1);
                }
                byte == b'\r' || byte == b'\n'
            } else {
                let (rune, width) = decode_utf8(&text[byte_offset as usize..]);
                byte_offset = byte_offset.wrapping_add(width as i32);
                is_line_break(rune)
            };
            if line_break {
                let previous_start = line_start;
                line_start = byte_offset;
                return Some(previous_start);
            }
        }
        finished = true;
        Some(line_start)
    })
}

/// This core helper clamps the line index to zero; the scanner's otherwise
/// similar `compute_line_of_position` returns -1 for a negative position.
///
/// port: tsc/internal/core/core.go:PositionToLineAndByteOffset
pub fn position_to_line_and_byte_offset(position: isize, line_starts: &[i32]) -> (isize, isize) {
    let line = line_starts
        .partition_point(|&start| start as isize <= position)
        .saturating_sub(1);
    (
        line as isize,
        position.wrapping_sub(line_starts[line] as isize),
    )
}

/// Counts standard UTF-8 decoding, including one unit per malformed byte.
/// A complete WTF-8 surrogate sentinel therefore counts as three units here.
///
/// port: tsc/internal/core/core.go:UTF16Len
pub fn utf16_len(text: &[u8]) -> isize {
    let Some(mut byte_offset) = text.iter().position(|byte| !byte.is_ascii()) else {
        return text.len() as isize;
    };
    let mut count = byte_offset as isize;
    while byte_offset < text.len() {
        let (rune, width) = decode_utf8(&text[byte_offset..]);
        byte_offset += width;
        count += utf16_rune_len(rune) as isize;
    }
    count
}

pub(crate) fn utf16_rune_len(rune: i32) -> i32 {
    // Every rune supplied here comes from standard UTF-8 decoding, so it is
    // either a Unicode scalar value or U+FFFD, never a surrogate or negative.
    if rune >= 0x10000 {
        2
    } else {
        1
    }
}

pub(crate) fn is_line_break(rune: i32) -> bool {
    matches!(rune, 0x0A | 0x0D | 0x2028 | 0x2029)
}

#[expect(
    clippy::naive_bytecount,
    reason = "This is only a capacity hint; the leaf crate needs no bytecount dependency."
)]
fn line_capacity(text: &[u8]) -> usize {
    text.iter().filter(|&&byte| byte == b'\n').count() + 1
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn line_scans_keep_signed_textpos_limits_after_source_length_narrowing() {
        // Model the cast of a large source length while supplying only its prefix.
        // Negative limits must not read even the first byte or classify its encoding.
        for length in [i32::MAX as usize + 1, u32::MAX as usize] {
            let lsp = LspLineMap::with_source_length(b"\n\xff", length);
            assert_eq!(lsp.line_starts, [0]);
            assert!(lsp.ascii_only);
            assert_eq!(
                ecma_line_starts_with_source_length(b"\n\xff", length).collect::<Vec<_>>(),
                [0]
            );
        }
        if let Ok(wrap) = usize::try_from(1_u64 << 32) {
            for (length, expected) in [(wrap, vec![0]), (wrap + 1, vec![0, 1])] {
                let lsp = LspLineMap::with_source_length(b"\n\xff", length);
                assert_eq!(lsp.line_starts, expected);
                assert!(lsp.ascii_only);
                assert_eq!(
                    ecma_line_starts_with_source_length(b"\n\xff", length).collect::<Vec<_>>(),
                    expected
                );
            }
        }
    }
}
