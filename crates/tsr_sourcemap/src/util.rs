use tsr_jsstring::classify::is_line_break;
use tsr_jsstring::wtf8::{decode_utf8, RUNE_ERROR};

use crate::lineinfo::EcmaLineInfo;

/// Tries to find the sourceMappingURL comment at the end of a file. The
/// result borrows the line info's text; it is empty when there is none.
// port: tsc/internal/sourcemap/util.go:TryGetSourceMappingURL
pub fn try_get_source_mapping_url(line_info: Option<&EcmaLineInfo>) -> &[u8] {
    if let Some(line_info) = line_info {
        let mut index = line_info.line_count() - 1;
        while index >= 0 {
            let mut line = line_info.line_text(index);
            index -= 1;
            line = trim_left_func(line, unicode_is_space);
            line = trim_right_func(line, is_line_break);
            if line.is_empty() {
                continue;
            }
            if line.len() < 4
                || !line.starts_with(b"//")
                || line[2] != b'#' && line[2] != b'@'
                || line[3] != b' '
            {
                break;
            }
            if let Some(url) = line[4..].strip_prefix(b"sourceMappingURL=") {
                return trim_right_func(url, unicode_is_space);
            }
        }
    }
    b""
}

/// Go's `unicode.IsSpace`.
fn unicode_is_space(rune: i32) -> bool {
    matches!(
        rune,
        0x09..=0x0d
            | 0x20
            | 0x85
            | 0xa0
            | 0x1680
            | 0x2000..=0x200a
            | 0x2028
            | 0x2029
            | 0x202f
            | 0x205f
            | 0x3000
    )
}

/// Go's `strings.TrimLeftFunc`: runes are decoded as Go decodes UTF-8, so an
/// invalid byte is a one-byte `RuneError`.
pub(crate) fn trim_left_func(text: &[u8], f: fn(i32) -> bool) -> &[u8] {
    let mut start = 0;
    while start < text.len() {
        let (rune, width) = decode_utf8(&text[start..]);
        if !f(rune) {
            break;
        }
        start += width;
    }
    &text[start..]
}

/// Go's `utf8.DecodeLastRuneInString`.
fn decode_last_rune(text: &[u8]) -> (i32, usize) {
    let end = text.len();
    if end == 0 {
        return (RUNE_ERROR, 0);
    }
    let mut start = end - 1;
    if text[start] < 0x80 {
        return (i32::from(text[start]), 1);
    }
    let lim = end.saturating_sub(4);
    while start > lim {
        start -= 1;
        // utf8.RuneStart: not a continuation byte.
        if text[start] & 0xc0 != 0x80 {
            break;
        }
    }
    let (rune, size) = decode_utf8(&text[start..end]);
    if start + size != end {
        return (RUNE_ERROR, 1);
    }
    (rune, size)
}

/// Go's `strings.TrimRightFunc`.
pub(crate) fn trim_right_func(text: &[u8], f: fn(i32) -> bool) -> &[u8] {
    // lastIndexFunc(s, f, false)
    let mut last = None;
    let mut i = text.len();
    while i > 0 {
        let (rune, size) = decode_last_rune(&text[..i]);
        i -= size;
        if !f(rune) {
            last = Some(i);
            break;
        }
    }
    let end = match last {
        Some(i) if text[i] >= 0x80 => i + decode_utf8(&text[i..]).1,
        Some(i) => i + 1,
        None => 0,
    };
    &text[..end]
}
