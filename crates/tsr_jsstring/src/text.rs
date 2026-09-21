//! Byte-oriented affixes, line splitting and quoting; no implicit UTF-8 repair.
use crate::{classify::is_white_space_like, wtf8::decode_utf8};
use std::borrow::Cow;

/// port: tsc/internal/stringutil/util.go:shouldEscapeForEncodeURI
pub fn should_escape_for_encode_uri(byte: u8) -> bool {
    !(byte.is_ascii_alphanumeric() || b";/?:@&=+$,#-_.!~*'()".contains(&byte))
}
/// port: tsc/internal/stringutil/util.go:EncodeURI
pub fn encode_uri(text: &[u8]) -> Cow<'_, [u8]> {
    if !text.iter().any(|&b| should_escape_for_encode_uri(b)) {
        return Cow::Borrowed(text);
    }
    const HEX: &[u8] = b"0123456789ABCDEF";
    let mut out = Vec::with_capacity(text.len());
    for &b in text {
        if should_escape_for_encode_uri(b) {
            out.extend_from_slice(&[b'%', HEX[(b >> 4) as usize], HEX[(b & 15) as usize]]);
        } else {
            out.push(b);
        }
    }
    Cow::Owned(out)
}
/// port: tsc/internal/stringutil/util.go:getByteOrderMarkLength
pub fn byte_order_mark_length(text: &[u8]) -> usize {
    if text.starts_with(b"\xfe\xff") || text.starts_with(b"\xff\xfe") {
        2
    } else if text.starts_with(b"\xef\xbb\xbf") {
        3
    } else {
        0
    }
}
/// port: tsc/internal/stringutil/util.go:RemoveByteOrderMark
pub fn remove_byte_order_mark(text: &[u8]) -> &[u8] {
    &text[byte_order_mark_length(text)..]
}
/// port: tsc/internal/stringutil/util.go:AddUTF8ByteOrderMark
pub fn add_utf8_byte_order_mark(text: &[u8]) -> Cow<'_, [u8]> {
    if byte_order_mark_length(text) > 0 {
        Cow::Borrowed(text)
    } else {
        let mut out = Vec::with_capacity(text.len() + 3);
        out.extend_from_slice(b"\xef\xbb\xbf");
        out.extend_from_slice(text);
        Cow::Owned(out)
    }
}
/// port: tsc/internal/stringutil/util.go:SplitLines
pub fn split_lines(text: &[u8]) -> Vec<&[u8]> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut i = 0;
    while i < text.len() {
        if matches!(text[i], b'\r' | b'\n') {
            out.push(&text[start..i]);
            i += if text[i] == b'\r' && text.get(i + 1) == Some(&b'\n') {
                2
            } else {
                1
            };
            start = i;
        } else {
            i += 1;
        }
    }
    if start < text.len() {
        out.push(&text[start..]);
    }
    out
}
/// port: tsc/internal/stringutil/util.go:GuessIndentation
pub fn guess_indentation(lines: &[&[u8]]) -> usize {
    const MAX: usize = 0x3fff_ffff;
    let mut indent = MAX;
    for &line in lines {
        if line.is_empty() {
            continue;
        }
        let mut i = 0;
        while i < line.len() && i < indent {
            let (rune, width) = decode_utf8(&line[i..]);
            if !is_white_space_like(rune) {
                break;
            }
            i += width;
        }
        indent = indent.min(i);
        if indent == 0 {
            return 0;
        }
    }
    if indent == MAX {
        0
    } else {
        indent
    }
}
/// port: tsc/internal/stringutil/util.go:StripQuotes
pub fn strip_quotes(text: &[u8]) -> &[u8] {
    if text.len() >= 2 && matches!(text[0], b'\'' | b'"' | b'`') && text.first() == text.last() {
        &text[1..text.len() - 1]
    } else {
        text
    }
}
/// Go's `\\.` removes the backslash before one rune, except LF. It neither
/// interprets escapes nor repairs malformed bytes from the matched substring.
/// port: tsc/internal/stringutil/util.go:UnquoteString
pub fn unquote_string(text: &[u8]) -> Cow<'_, [u8]> {
    let text = strip_quotes(text);
    if !text.contains(&b'\\') {
        return Cow::Borrowed(text);
    }
    let mut out = Vec::with_capacity(text.len());
    let mut i = 0;
    while i < text.len() {
        if text[i] == b'\\' && text.get(i + 1).is_some_and(|&b| b != b'\n') {
            i += 1;
            let (_, width) = decode_utf8(&text[i..]);
            out.extend_from_slice(&text[i..i + width]);
            i += width;
        } else {
            out.push(text[i]);
            i += 1;
        }
    }
    Cow::Owned(out)
}
