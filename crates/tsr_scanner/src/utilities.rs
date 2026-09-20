use std::borrow::Cow;

use tsr_ast::{IdentifierData, SyntaxKind};
use tsr_core::LanguageVariant;
use tsr_jsstring::line_map::compute_ecma_line_starts;
use tsr_jsstring::wtf8::{decode_utf8, RUNE_ERROR};

use crate::identifier::{is_identifier_part_ex, is_identifier_start, keyword};

pub(crate) fn is_digit(ch: i32) -> bool {
    (i32::from(b'0')..=i32::from(b'9')).contains(&ch)
}
pub(crate) fn is_octal_digit(ch: i32) -> bool {
    (i32::from(b'0')..=i32::from(b'7')).contains(&ch)
}
pub(crate) fn is_hex_digit(ch: i32) -> bool {
    is_digit(ch)
        || (i32::from(b'a')..=i32::from(b'f')).contains(&ch)
        || (i32::from(b'A')..=i32::from(b'F')).contains(&ch)
}
pub(crate) fn is_ascii_letter(ch: i32) -> bool {
    (i32::from(b'a')..=i32::from(b'z')).contains(&ch)
        || (i32::from(b'A')..=i32::from(b'Z')).contains(&ch)
}

/// port: tsc/internal/stringutil/util.go:IsLineBreak
pub fn is_line_break(ch: i32) -> bool {
    matches!(ch, 0x0a | 0x0d | 0x2028 | 0x2029)
}

/// port: tsc/internal/stringutil/util.go:IsWhiteSpaceSingleLine
pub fn is_white_space_single_line(ch: i32) -> bool {
    matches!(
        ch,
        0x20 | 0x09 | 0x0b | 0x0c | 0x85 | 0xa0 | 0x1680 | 0x2000
            ..=0x200b | 0x202f | 0x205f | 0x3000 | 0xfeff
    )
}
/// port: tsc/internal/stringutil/util.go:IsWhiteSpaceLike
pub fn is_white_space_like(ch: i32) -> bool {
    is_white_space_single_line(ch) || is_line_break(ch)
}

/// Go DecodeLastRune: a malformed final byte consumes exactly one byte.
pub(crate) fn decode_last_utf8(bytes: &[u8]) -> (i32, usize) {
    let Some(&last) = bytes.last() else {
        return (RUNE_ERROR, 0);
    };
    if last < 0x80 {
        return (i32::from(last), 1);
    }
    let end = bytes.len();
    let mut start = end - 1;
    let lower = end.saturating_sub(4);
    while start > lower && bytes[start] & 0xc0 == 0x80 {
        start -= 1;
    }
    let (rune, width) = decode_utf8(&bytes[start..]);
    if start + width == end {
        (rune, width)
    } else {
        (RUNE_ERROR, 1)
    }
}

/// port: tsc/internal/scanner/utilities.go:tokenIsIdentifierOrKeyword
pub(crate) fn token_is_identifier_or_keyword(token: SyntaxKind) -> bool {
    token as u16 >= SyntaxKind::Identifier as u16
}
/// port: tsc/internal/scanner/utilities.go:IdentifierToKeywordKind
pub fn identifier_to_keyword_kind(node: &IdentifierData) -> SyntaxKind {
    keyword(node.text.as_bytes())
}

/// port: tsc/internal/scanner/utilities.go:normalizeJSDocTypeSourceText
pub fn normalize_jsdoc_type_source_text(text: &[u8]) -> Cow<'_, [u8]> {
    let starts = compute_ecma_line_starts(text);
    if starts.len() == 1 {
        return Cow::Borrowed(strip_leading_jsdoc_comment(text));
    }
    let mut result = Vec::with_capacity(text.len());
    for (i, &start) in starts.iter().enumerate() {
        if i > 0 {
            result.push(b'\n');
        }
        let end = starts.get(i + 1).map_or(text.len(), |&end| end as usize);
        let mut line = &text[start as usize..end];
        loop {
            let (rune, size) = decode_last_utf8(line);
            if !is_line_break(rune) {
                break;
            }
            line = &line[..line.len() - size];
        }
        result.extend_from_slice(strip_leading_jsdoc_comment(line));
    }
    Cow::Owned(result)
}

fn trim_start_space(mut bytes: &[u8]) -> &[u8] {
    loop {
        let (rune, size) = decode_utf8(bytes);
        if size == 0 || !is_white_space_like(rune) {
            return bytes;
        }
        bytes = &bytes[size..];
    }
}

/// port: tsc/internal/scanner/utilities.go:stripLeadingJSDocComment
fn strip_leading_jsdoc_comment(line: &[u8]) -> &[u8] {
    let line = trim_start_space(line);
    trim_start_space(line.strip_prefix(b"*").unwrap_or(line))
}

/// port: tsc/internal/scanner/utilities.go:IsIdentifierText
pub fn is_identifier_text(name: &[u8], variant: LanguageVariant) -> bool {
    let (ch, mut pos) = decode_utf8(name);
    if !is_identifier_start(ch) {
        return false;
    }
    while pos < name.len() {
        let (ch, size) = decode_utf8(&name[pos..]);
        if !is_identifier_part_ex(ch, variant) {
            return false;
        }
        pos += size;
    }
    true
}
/// port: tsc/internal/scanner/utilities.go:IsIntrinsicJsxName
pub fn is_intrinsic_jsx_name(name: &[u8]) -> bool {
    !name.is_empty() && (name[0].is_ascii_lowercase() || name.contains(&b'-'))
}
