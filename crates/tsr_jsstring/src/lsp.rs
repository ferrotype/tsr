//! LSP conversions clamp offsets and stop before a partial UTF-16 surrogate pair.

use crate::line_map::{utf16_len, utf16_rune_len, LspLineMap};
use crate::wtf8::decode_utf8;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PositionEncoding {
    Utf8,
    Utf16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LspPosition {
    pub line: u32,
    pub character: u32,
}

/// Conversion for a raw, non-content-mapped script. The caller supplies the
/// corresponding source line map, just as the upstream converter's cache does.
/// The wire's unsigned fields are cast to signed `TextPos` before conversion.
///
/// port: tsc/internal/ls/lsconv/converters.go:Converters.lineAndCharacterToPosition
pub fn lsp_line_and_character_to_position(
    text: &[u8],
    line_map: &LspLineMap,
    position: LspPosition,
    encoding: PositionEncoding,
) -> i32 {
    let line = position.line as i32;
    let character = position.character as i32;
    if line as isize >= line_map.line_starts.len() as isize {
        return text.len() as i32;
    }
    let start = line_map.line_starts[line as usize];
    let line_end = line_map
        .line_starts
        .get(line as usize + 1)
        .copied()
        .unwrap_or(text.len() as i32);
    if line_map.ascii_only || encoding == PositionEncoding::Utf8 {
        return start.max(start.wrapping_add(character).min(line_end));
    }
    let mut count = 0_i32;
    let mut byte_offset = start as isize;
    let end = line_end as isize;
    while byte_offset < end {
        let (rune, width) = decode_utf8(&text[byte_offset as usize..]);
        let units = utf16_rune_len(rune);
        if count.wrapping_add(units) > character {
            break;
        }
        count = count.wrapping_add(units);
        byte_offset = byte_offset.wrapping_add(width as isize);
    }
    byte_offset as i32
}

/// The absolute offset is clamped first. Counting a byte prefix deliberately
/// treats a cut UTF-8 sequence as one replacement rune for each remaining byte.
///
/// port: tsc/internal/ls/lsconv/converters.go:Converters.positionToLineAndCharacter
pub fn lsp_position_to_line_and_character(
    text: &[u8],
    line_map: &LspLineMap,
    position: i32,
    encoding: PositionEncoding,
) -> LspPosition {
    let position = position.min(text.len() as i32).max(0);
    let line = line_map.compute_index_of_line_start(position);
    let start = line_map.line_starts[line];
    let character = if line_map.ascii_only || encoding == PositionEncoding::Utf8 {
        position.wrapping_sub(start)
    } else {
        utf16_len(&text[start as usize..position as usize]) as i32
    };
    LspPosition {
        line: line as u32,
        character: character as u32,
    }
}
