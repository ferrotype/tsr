//! Scanner line helpers consume whole characters before checking UTF-16 offsets.

use std::cmp::Ordering;

use crate::line_map::{compute_ecma_line_starts, is_line_break, utf16_len, utf16_rune_len};
use crate::wtf8::decode_utf8;

/// port: tsc/internal/scanner/scanner.go:ComputeLineOfPosition
pub fn compute_line_of_position(line_starts: &[i32], position: isize) -> isize {
    let mut low = 0;
    let mut high = line_starts.len() as isize - 1;
    while low <= high {
        let middle = low + ((high - low) >> 1);
        let value = line_starts[middle as usize] as isize;
        match value.cmp(&position) {
            Ordering::Less => low = middle + 1,
            Ordering::Greater => high = middle - 1,
            Ordering::Equal => return middle,
        }
    }
    low - 1
}

/// port: tsc/internal/scanner/scanner.go:GetECMALineOfPosition
pub fn get_ecma_line_of_position(text: &[u8], position: isize) -> isize {
    compute_line_of_position(&compute_ecma_line_starts(text), position)
}

/// port: tsc/internal/scanner/scanner.go:GetECMALineAndUTF16CharacterOfPosition
pub fn get_ecma_line_and_utf16_character_of_position(
    text: &[u8],
    position: isize,
) -> (isize, isize) {
    let line_starts = compute_ecma_line_starts(text);
    let line = compute_line_of_position(&line_starts, position);
    let start = line_starts[line as usize] as usize;
    (line, utf16_len(&text[start..position as usize]))
}

/// port: tsc/internal/scanner/scanner.go:GetECMALineAndByteOffsetOfPosition
pub fn get_ecma_line_and_byte_offset_of_position(text: &[u8], position: isize) -> (isize, isize) {
    let line_starts = compute_ecma_line_starts(text);
    let line = compute_line_of_position(&line_starts, position);
    (
        line,
        position.wrapping_sub(line_starts[line as usize] as isize),
    )
}

/// Returns the byte preceding the first terminator or EOF. An empty first line
/// returns -1; a multi-byte final character returns its final byte's offset.
///
/// port: tsc/internal/scanner/scanner.go:GetECMAEndLinePosition
pub fn get_ecma_end_line_position(text: &[u8], line: isize) -> isize {
    let line_starts = compute_ecma_line_starts(text);
    let mut byte_offset = line_starts[line as usize] as usize;
    loop {
        let (rune, width) = decode_utf8(&text[byte_offset..]);
        if width == 0 || is_line_break(rune) {
            return byte_offset as isize - 1;
        }
        byte_offset += width;
    }
}

/// port: tsc/internal/scanner/scanner.go:GetECMAPositionOfLineAndUTF16Character
pub fn get_ecma_position_of_line_and_utf16_character(
    text: &[u8],
    line: isize,
    character: isize,
) -> isize {
    compute_position_of_line_and_utf16_character(
        &compute_ecma_line_starts(text),
        line,
        character,
        text,
        false,
    )
}

/// port: tsc/internal/scanner/scanner.go:GetECMAPositionOfLineAndByteOffset
pub fn get_ecma_position_of_line_and_byte_offset(
    text: &[u8],
    line: isize,
    byte_offset: isize,
) -> isize {
    compute_position_of_line_and_byte_offset(&compute_ecma_line_starts(text), line, byte_offset)
}

/// port: tsc/internal/scanner/scanner.go:ComputePositionOfLineAndByteOffset
pub fn compute_position_of_line_and_byte_offset(
    line_starts: &[i32],
    line: isize,
    byte_offset: isize,
) -> isize {
    assert_valid_line(line_starts, line);
    (line_starts[line as usize] as isize).wrapping_add(byte_offset)
}

/// Unlike LSP this consumes a whole rune before checking the UTF-16 count, so
/// an offset inside an astral pair rounds forward. Line terminators are part of
/// the scanned line. A nonpositive character returns the line start.
///
/// port: tsc/internal/scanner/scanner.go:ComputePositionOfLineAndUTF16Character
pub fn compute_position_of_line_and_utf16_character(
    line_starts: &[i32],
    mut line: isize,
    character: isize,
    text: &[u8],
    allow_edits: bool,
) -> isize {
    if line < 0 || line >= line_starts.len() as isize {
        if allow_edits {
            if line < 0 {
                line = 0;
            } else {
                line = line_starts.len() as isize - 1;
            }
        } else {
            assert_valid_line(line_starts, line);
        }
    }
    let line_start = line_starts[line as usize] as isize;
    if character > 0 {
        let line_end = line_starts
            .get(line as usize + 1)
            .map_or(text.len() as isize, |&start| start as isize);
        let mut count = 0;
        let mut byte_offset = line_start;
        while byte_offset < line_end {
            if count >= character {
                break;
            }
            let (rune, width) = decode_utf8(&text[byte_offset as usize..]);
            count += utf16_rune_len(rune) as isize;
            byte_offset += width as isize;
        }
        if !allow_edits {
            assert!(
                byte_offset != line_end || count >= character,
                "Bad UTF-16 character offset. Line: {line}, character: {character}."
            );
            assert!(byte_offset <= text.len() as isize);
            return byte_offset;
        }
        return byte_offset.min(text.len() as isize);
    }
    if allow_edits {
        line_start.min(text.len() as isize)
    } else {
        assert!(line_start <= text.len() as isize);
        line_start
    }
}

fn assert_valid_line(line_starts: &[i32], line: isize) {
    assert!(
        line >= 0 && line < line_starts.len() as isize,
        "Bad line number. Line: {line}, lineStarts.length: {}.",
        line_starts.len()
    );
}
