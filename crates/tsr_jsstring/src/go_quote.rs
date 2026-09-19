//! The `%q` string spelling used in pinned Go panic payloads.
//!
//! This is `strconv.Quote` over arbitrary Go string bytes. Its printable-rune
//! predicate comes from the oracle toolchain, independently of Rust's Unicode
//! version. JavaScript literal escaping has its separate source rules.

use crate::{
    go_print_generated::PRINT_RANGES,
    wtf8::{decode_utf8, RUNE_ERROR},
};

const HEX: &[u8; 16] = b"0123456789abcdef";

fn escape_hex(output: &mut String, prefix: &str, value: i32, digits: u32) {
    output.push_str(prefix);
    for shift in (0..digits).rev() {
        output.push(char::from(HEX[((value >> (shift * 4)) & 15) as usize]));
    }
}

/// Quote bytes as a Go double-quoted string, including malformed UTF-8.
pub fn go_quote(mut bytes: &[u8]) -> String {
    let mut output = String::with_capacity(bytes.len().saturating_add(2));
    output.push('"');
    while !bytes.is_empty() {
        let (rune, width) = decode_utf8(bytes);
        if rune == RUNE_ERROR && width == 1 {
            escape_hex(&mut output, "\\x", i32::from(bytes[0]), 2);
        } else if rune == i32::from(b'"') || rune == i32::from(b'\\') {
            output.push('\\');
            output.push(char::from_u32(rune as u32).expect("ASCII quote or backslash"));
        } else {
            let index = PRINT_RANGES.partition_point(|&(_, end)| end < rune);
            if PRINT_RANGES
                .get(index)
                .is_some_and(|&(start, _)| start <= rune)
            {
                output.push(char::from_u32(rune as u32).expect("decoded UTF-8 scalar"));
            } else {
                match rune {
                    7 => output.push_str("\\a"),
                    8 => output.push_str("\\b"),
                    12 => output.push_str("\\f"),
                    10 => output.push_str("\\n"),
                    13 => output.push_str("\\r"),
                    9 => output.push_str("\\t"),
                    11 => output.push_str("\\v"),
                    0..32 | 0x7f => escape_hex(&mut output, "\\x", rune, 2),
                    0..0x10000 => escape_hex(&mut output, "\\u", rune, 4),
                    _ => escape_hex(&mut output, "\\U", rune, 8),
                }
            }
        }
        bytes = &bytes[width..];
    }
    output.push('"');
    output
}

#[cfg(test)]
mod tests {
    use super::go_quote;

    #[test]
    fn go_quotes_preserve_raw_bytes_controls_and_pinned_unicode() {
        assert_eq!(go_quote(b""), "\"\"");
        assert_eq!(
            go_quote(b"a\0\x07\x08\x0c\n\r\t\x0b\x1f\x7f\\\""),
            "\"a\\x00\\a\\b\\f\\n\\r\\t\\v\\x1f\\x7f\\\\\\\"\""
        );
        assert_eq!(
            go_quote(b"\xed\xa0\x80\xc0\xaf\xff\xef\xbf\xbd"),
            "\"\\xed\\xa0\\x80\\xc0\\xaf\\xff�\""
        );
        assert_eq!(
            go_quote("é 😀\u{a0}\u{200b}\u{2028}\u{feff}\u{1e6c0}\u{1e6df}".as_bytes()),
            "\"é 😀\\u00a0\\u200b\\u2028\\ufeff\u{1e6c0}\\U0001e6df\""
        );
    }
}
