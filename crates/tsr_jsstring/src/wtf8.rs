//! Standard UTF-8 and JavaScript sentinel decoding remain distinct operations.

use std::borrow::Cow;

/// Go's unicode/utf8.RuneError.
pub const RUNE_ERROR: i32 = 0xfffd;

const SURROGATE_UTF8_LEAD: u8 = 0xed;
const SURROGATE_UTF8_SECOND_MIN: u8 = 0xa0;
const UTF8_CONTINUATION_MIN: u8 = 0x80;
const UTF8_CONTINUATION_MAX: u8 = 0xbf;
const UTF8_CONTINUATION_MASK: u8 = 0x3f;

/// Go's unicode/utf8.DecodeRuneInString: invalid input consumes one byte.
/// The empty input returns (RuneError, 0), unlike a malformed nonempty prefix.
pub fn decode_utf8(bytes: &[u8]) -> (i32, usize) {
    let Some(&first) = bytes.first() else {
        return (RUNE_ERROR, 0);
    };
    if first < 0x80 {
        return (i32::from(first), 1);
    }
    let (width, second_min, second_max, mask) = match first {
        0xc2..=0xdf => (2, 0x80, 0xbf, 0x1f),
        0xe0 => (3, 0xa0, 0xbf, 0x0f),
        0xe1..=0xec | 0xee..=0xef => (3, 0x80, 0xbf, 0x0f),
        0xed => (3, 0x80, 0x9f, 0x0f),
        0xf0 => (4, 0x90, 0xbf, 0x07),
        0xf1..=0xf3 => (4, 0x80, 0xbf, 0x07),
        0xf4 => (4, 0x80, 0x8f, 0x07),
        _ => return (RUNE_ERROR, 1),
    };
    if bytes.len() < width || !(second_min..=second_max).contains(&bytes[1]) {
        return (RUNE_ERROR, 1);
    }
    if bytes[2..width]
        .iter()
        .any(|byte| !(UTF8_CONTINUATION_MIN..=UTF8_CONTINUATION_MAX).contains(byte))
    {
        return (RUNE_ERROR, 1);
    }
    let mut rune = i32::from(first & mask);
    for byte in &bytes[1..width] {
        rune = (rune << 6) | i32::from(byte & UTF8_CONTINUATION_MASK);
    }
    (rune, width)
}

/// port: tsc/internal/stringutil/util.go:DecodeJSStringRune
pub fn decode_rune(bytes: &[u8]) -> (i32, usize) {
    if bytes.len() >= 3
        && bytes[0] == SURROGATE_UTF8_LEAD
        && (SURROGATE_UTF8_SECOND_MIN..=UTF8_CONTINUATION_MAX).contains(&bytes[1])
        && (UTF8_CONTINUATION_MIN..=UTF8_CONTINUATION_MAX).contains(&bytes[2])
    {
        return (
            0xd000
                | (i32::from(bytes[1] & UTF8_CONTINUATION_MASK) << 6)
                | i32::from(bytes[2] & UTF8_CONTINUATION_MASK),
            3,
        );
    }
    decode_utf8(bytes)
}

/// port: tsc/internal/stringutil/util.go:IsSurrogate
pub fn is_surrogate(rune: i32) -> bool {
    (0xd800..=0xdfff).contains(&rune)
}
/// port: tsc/internal/stringutil/util.go:IsHighSurrogate
pub fn is_high_surrogate(rune: i32) -> bool {
    (0xd800..=0xdbff).contains(&rune)
}
/// port: tsc/internal/stringutil/util.go:IsLowSurrogate
pub fn is_low_surrogate(rune: i32) -> bool {
    (0xdc00..=0xdfff).contains(&rune)
}

/// port: tsc/internal/stringutil/util.go:SurrogatePairToCodePoint
pub fn surrogate_pair_to_code_point(high: i32, low: i32) -> i32 {
    if is_high_surrogate(high) && is_low_surrogate(low) {
        ((high - 0xd800) << 10) + low - 0xdc00 + 0x10000
    } else {
        RUNE_ERROR
    }
}

/// port: tsc/internal/stringutil/util.go:CodePointToSurrogatePair
pub fn code_point_to_surrogate_pair(rune: i32) -> (i32, i32) {
    if (0x0001_0000..=0x0010_ffff).contains(&rune) {
        let value = rune - 0x10000;
        (0xd800 + (value >> 10), 0xdc00 + (value & 0x3ff))
    } else {
        (RUNE_ERROR, RUNE_ERROR)
    }
}

/// port: tsc/internal/stringutil/util.go:EncodeJSStringRune
pub fn encode_rune(rune: i32) -> Vec<u8> {
    let mut result = Vec::with_capacity(4);
    append_rune(&mut result, rune);
    result
}

pub(crate) fn append_rune(output: &mut Vec<u8>, rune: i32) {
    if is_surrogate(rune) {
        output.extend_from_slice(&[
            SURROGATE_UTF8_LEAD,
            UTF8_CONTINUATION_MIN | ((rune >> 6) as u8 & UTF8_CONTINUATION_MASK),
            UTF8_CONTINUATION_MIN | (rune as u8 & UTF8_CONTINUATION_MASK),
        ]);
    } else {
        let character = char::from_u32(rune as u32).unwrap_or(char::REPLACEMENT_CHARACTER);
        let mut buffer = [0; 4];
        output.extend_from_slice(character.encode_utf8(&mut buffer).as_bytes());
    }
}

#[derive(Clone, Debug)]
pub struct CodePoints<'a> {
    remaining: &'a [u8],
}

pub fn code_points(bytes: &[u8]) -> CodePoints<'_> {
    CodePoints { remaining: bytes }
}

impl Iterator for CodePoints<'_> {
    type Item = i32;
    fn next(&mut self) -> Option<Self::Item> {
        let (rune, width) = decode_rune(self.remaining);
        if width == 0 {
            return None;
        }
        self.remaining = &self.remaining[width..];
        Some(rune)
    }
}

/// port: tsc/internal/stringutil/util.go:CombineSurrogatePairs
pub fn combine_surrogate_pairs(bytes: &[u8]) -> Cow<'_, [u8]> {
    if !bytes.contains(&SURROGATE_UTF8_LEAD) {
        return Cow::Borrowed(bytes);
    }
    let mut output = Vec::with_capacity(bytes.len());
    let mut offset = 0;
    while offset < bytes.len() {
        let (rune, width) = decode_rune(&bytes[offset..]);
        if is_high_surrogate(rune) {
            let (low, low_width) = decode_rune(&bytes[offset + width..]);
            if is_low_surrogate(low) {
                append_rune(&mut output, surrogate_pair_to_code_point(rune, low));
                offset += width + low_width;
                continue;
            }
        }
        output.extend_from_slice(&bytes[offset..offset + width]);
        offset += width;
    }
    Cow::Owned(output)
}
