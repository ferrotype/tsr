//! Per-operation casing and truncation over arbitrary bytes.

use std::borrow::Cow;

use crate::case_tables::{CASED_RANGES, CASE_IGNORABLE_RANGES, CASE_MAPPINGS, SIMPLE_LOWER};
use crate::wtf8::{append_rune, decode_rune, decode_utf8, is_surrogate};

/// port: tsc/internal/stringutil/js_case.go:ToLowerJS
pub fn to_lower_js(bytes: &[u8]) -> Cow<'_, [u8]> {
    if let Some(ascii) = to_lower_ascii(bytes) {
        return ascii;
    }
    let mut result = Vec::with_capacity(bytes.len());
    let mut cased_before = false;
    let mut offset = 0;
    while offset < bytes.len() {
        let (rune, width) = decode_rune(&bytes[offset..]);
        offset += width;
        if is_surrogate(rune) {
            append_rune(&mut result, rune);
        } else if let Ok(index) = CASE_MAPPINGS.binary_search_by_key(&rune, |row| row.0) {
            let (_, lower, _, conditional) = CASE_MAPPINGS[index];
            let mapping = match conditional {
                Some(final_sigma) if is_final_sigma_context(cased_before, bytes, offset) => {
                    final_sigma
                }
                _ => lower,
            };
            result.extend_from_slice(mapping.as_bytes());
        } else {
            append_rune(&mut result, rune);
        }
        if !is_unicode_case_ignorable(rune) {
            cased_before = is_sigma_cased(rune);
        }
    }
    Cow::Owned(result)
}

/// port: tsc/internal/stringutil/js_case.go:ToUpperJS
pub fn to_upper_js(bytes: &[u8]) -> Cow<'_, [u8]> {
    if let Some(ascii) = to_upper_ascii(bytes) {
        return ascii;
    }
    let mut result = Vec::with_capacity(bytes.len());
    let mut offset = 0;
    while offset < bytes.len() {
        let (rune, width) = decode_rune(&bytes[offset..]);
        if is_surrogate(rune) {
            result.extend_from_slice(&bytes[offset..offset + width]);
        } else if let Ok(index) = CASE_MAPPINGS.binary_search_by_key(&rune, |row| row.0) {
            result.extend_from_slice(CASE_MAPPINGS[index].2.as_bytes());
        } else {
            append_rune(&mut result, rune);
        }
        offset += width;
    }
    Cow::Owned(result)
}

/// port: tsc/internal/stringutil/js_case.go:toLowerASCII
fn to_lower_ascii(bytes: &[u8]) -> Option<Cow<'_, [u8]>> {
    let mut needs_mapping = false;
    for byte in bytes {
        if !byte.is_ascii() {
            return None;
        }
        needs_mapping |= byte.is_ascii_uppercase();
    }
    Some(if needs_mapping {
        Cow::Owned(bytes.to_ascii_lowercase())
    } else {
        Cow::Borrowed(bytes)
    })
}

/// port: tsc/internal/stringutil/js_case.go:toUpperASCII
fn to_upper_ascii(bytes: &[u8]) -> Option<Cow<'_, [u8]>> {
    let mut needs_mapping = false;
    for byte in bytes {
        if !byte.is_ascii() {
            return None;
        }
        needs_mapping |= byte.is_ascii_lowercase();
    }
    Some(if needs_mapping {
        Cow::Owned(bytes.to_ascii_uppercase())
    } else {
        Cow::Borrowed(bytes)
    })
}

/// port: tsc/internal/stringutil/js_case.go:isFinalSigmaContext
fn is_final_sigma_context(cased_before: bool, bytes: &[u8], after_offset: usize) -> bool {
    cased_before && !has_sigma_cased_after(bytes, after_offset)
}

/// port: tsc/internal/stringutil/js_case.go:hasSigmaCasedAfter
fn has_sigma_cased_after(bytes: &[u8], start: usize) -> bool {
    let mut offset = start;
    while offset < bytes.len() {
        let (rune, width) = decode_rune(&bytes[offset..]);
        offset += width;
        if is_unicode_case_ignorable(rune) {
            continue;
        }
        return is_sigma_cased(rune);
    }
    false
}

fn in_ranges(rune: i32, ranges: &[(i32, i32, i32)]) -> bool {
    let index = ranges.partition_point(|&(_, last, _)| last < rune);
    ranges.get(index).is_some_and(|&(first, last, stride)| {
        rune >= first && rune <= last && (rune - first) % stride == 0
    })
}

/// port: tsc/internal/stringutil/js_case.go:isSigmaCased
fn is_sigma_cased(rune: i32) -> bool {
    in_ranges(rune, CASED_RANGES)
}

/// port: tsc/internal/stringutil/js_case.go:isUnicodeCaseIgnorable
fn is_unicode_case_ignorable(rune: i32) -> bool {
    in_ranges(rune, CASE_IGNORABLE_RANGES)
}

/// port: tsc/internal/stringutil/util.go:LowerFirstChar
pub fn lower_first_char(bytes: &[u8]) -> Vec<u8> {
    let (rune, width) = decode_utf8(bytes);
    if width == 0 {
        return bytes.to_vec();
    }
    let mut result = Vec::with_capacity(bytes.len() + 2);
    append_rune(&mut result, simple_lower(rune));
    result.extend_from_slice(&bytes[width..]);
    result
}

/// Go strings.ToLower semantics for spelling suggestions, using the pinned
/// toolchain's simple mappings rather than JavaScript full case conversion.
/// Malformed UTF-8 is decoded as width-one replacement runes, as Go does.
pub fn to_lower_go(bytes: &[u8]) -> Vec<u8> {
    if bytes.is_ascii() {
        return bytes.to_ascii_lowercase();
    }
    let mut result = Vec::with_capacity(bytes.len());
    let mut offset = 0;
    while offset < bytes.len() {
        let (rune, width) = decode_utf8(&bytes[offset..]);
        append_rune(&mut result, simple_lower(rune));
        offset += width;
    }
    result
}

fn simple_lower(rune: i32) -> i32 {
    SIMPLE_LOWER
        .binary_search_by_key(&rune, |row| row.0)
        .map_or(rune, |index| SIMPLE_LOWER[index].1)
}

/// port: tsc/internal/stringutil/util.go:TruncateByRunes
pub fn truncate_by_runes(bytes: &[u8], max_length: isize) -> &[u8] {
    if max_length <= 0 {
        return &bytes[..0];
    }
    if bytes.len() < max_length as usize {
        return bytes;
    }
    let mut offset = 0;
    for _ in 0..max_length {
        if offset == bytes.len() {
            break;
        }
        offset += decode_utf8(&bytes[offset..]).1;
    }
    &bytes[..offset]
}
