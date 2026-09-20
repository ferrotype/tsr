//! Literal-content escaping over arbitrary bytes, with upstream quote and flag rules.

use crate::wtf8::{decode_rune, is_surrogate, RUNE_ERROR};

const BACKSLASH: i32 = b'\\' as i32;
const DOLLAR: i32 = b'$' as i32;
const CARRIAGE_RETURN: i32 = b'\r' as i32;
const LINE_FEED: i32 = b'\n' as i32;
const DOUBLE_QUOTE: i32 = b'"' as i32;
const SINGLE_QUOTE: i32 = b'\'' as i32;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum QuoteChar {
    Single = b'\'',
    Double = b'"',
    Backtick = b'`',
}

/// The first two flags affect this leaf worker; the other two are consumed by
/// the future original-source/printer integration and are inert in the worker.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct LiteralEscapeFlags(u8);

impl LiteralEscapeFlags {
    pub const NONE: Self = Self(0);
    pub const NEVER_ASCII_ESCAPE: Self = Self(1);
    pub const JSX_ATTRIBUTE_ESCAPE: Self = Self(2);
    pub const TERMINATE_UNTERMINATED_LITERALS: Self = Self(4);
    pub const ALLOW_NUMERIC_SEPARATOR: Self = Self(8);

    pub fn from_bits(bits: u8) -> Option<Self> {
        (bits & !15 == 0).then_some(Self(bits))
    }
    pub fn bits(self) -> u8 {
        self.0
    }
    pub fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }
}

impl std::ops::BitOr for LiteralEscapeFlags {
    type Output = Self;
    fn bitor(self, rhs: Self) -> Self {
        Self(self.0 | rhs.0)
    }
}

/// port: tsc/internal/printer/utilities.go:encodeJsxCharacterEntity
fn encode_jsx_character_entity(output: &mut Vec<u8>, rune: i32) {
    output.extend_from_slice(format!("&#x{rune:X};").as_bytes());
}

/// port: tsc/internal/printer/utilities.go:encodeUtf16EscapeSequence
fn encode_utf16_escape_sequence(output: &mut Vec<u8>, rune: i32) {
    output.extend_from_slice(format!("\\u{rune:04X}").as_bytes());
}

fn canonical_escape(rune: i32) -> Option<&'static [u8]> {
    match char::from_u32(rune as u32)? {
        '\t' => Some(br"\t"),
        '\u{000b}' => Some(br"\v"),
        '\u{000c}' => Some(br"\f"),
        '\u{0008}' => Some(br"\b"),
        '\r' => Some(br"\r"),
        '\n' => Some(br"\n"),
        '\\' => Some(br"\\"),
        '"' => Some(b"\\\""),
        '\'' => Some(br"\'"),
        '`' => Some(br"\`"),
        '$' => Some(br"\$"),
        '\u{2028}' => Some(br"\u2028"),
        '\u{2029}' => Some(br"\u2029"),
        '\u{0085}' => Some(br"\u0085"),
        _ => None,
    }
}

/// Escape literal contents, without surrounding quotes.
/// port: tsc/internal/printer/utilities.go:escapeStringWorker
pub fn escape_string_with_flags(
    bytes: &[u8],
    quote: QuoteChar,
    flags: LiteralEscapeFlags,
) -> Vec<u8> {
    let mut output = Vec::with_capacity(bytes.len());
    let mut copied_until = 0;
    let mut offset = 0;
    let jsx = flags.contains(LiteralEscapeFlags::JSX_ATTRIBUTE_ESCAPE);
    let never_ascii_escape = flags.contains(LiteralEscapeFlags::NEVER_ASCII_ESCAPE);
    while offset < bytes.len() {
        let (mut rune, mut width) = decode_rune(&bytes[offset..]);
        let mut escape = is_surrogate(rune) || rune == RUNE_ERROR && width == 1;
        match rune {
            BACKSLASH => {
                if !jsx {
                    escape = true;
                }
            }
            DOLLAR => {
                if quote == QuoteChar::Backtick && bytes.get(offset + 1) == Some(&b'{') {
                    escape = true;
                }
            }
            value
                if value == i32::from(quote as u8)
                    || matches!(value, 0x2028 | 0x2029 | 0x85 | CARRIAGE_RETURN) =>
            {
                escape = true;
            }
            LINE_FEED => {
                if quote != QuoteChar::Backtick {
                    escape = true;
                }
            }
            _ => {
                if rune <= 0x1f || !never_ascii_escape && rune > 0x7f {
                    escape = true;
                }
            }
        }
        if escape {
            output.extend_from_slice(&bytes[copied_until..offset]);
            if jsx {
                match rune {
                    0 => output.extend_from_slice(b"&#0;"),
                    DOUBLE_QUOTE => output.extend_from_slice(b"&quot;"),
                    SINGLE_QUOTE => output.extend_from_slice(b"&apos;"),
                    _ => encode_jsx_character_entity(&mut output, rune),
                }
            } else if rune == CARRIAGE_RETURN
                && quote == QuoteChar::Backtick
                && bytes.get(offset + 1) == Some(&b'\n')
            {
                width += 1;
                output.extend_from_slice(br"\r\n");
            } else if rune > 0xffff {
                rune -= 0x10000;
                encode_utf16_escape_sequence(&mut output, (rune >> 10) + 0xd800);
                encode_utf16_escape_sequence(&mut output, (rune & 0x3ff) + 0xdc00);
            } else if is_surrogate(rune) {
                encode_utf16_escape_sequence(&mut output, rune);
            } else if rune == 0 {
                if bytes.get(offset + 1).is_some_and(u8::is_ascii_digit) {
                    output.extend_from_slice(br"\x00");
                } else {
                    output.extend_from_slice(br"\0");
                }
            } else if let Some(escaped) = canonical_escape(rune) {
                output.extend_from_slice(escaped);
            } else {
                encode_utf16_escape_sequence(&mut output, rune);
            }
            copied_until = offset + width;
        }
        offset += width;
    }
    output.extend_from_slice(&bytes[copied_until..]);
    output
}

/// port: tsc/internal/printer/utilities.go:EscapeString
pub fn escape_string(bytes: &[u8], quote: QuoteChar) -> Vec<u8> {
    escape_string_with_flags(bytes, quote, LiteralEscapeFlags::NEVER_ASCII_ESCAPE)
}

/// port: tsc/internal/printer/utilities.go:escapeNonAsciiString
pub fn escape_non_ascii_string(bytes: &[u8], quote: QuoteChar) -> Vec<u8> {
    escape_string_with_flags(bytes, quote, LiteralEscapeFlags::NONE)
}

/// port: tsc/internal/printer/utilities.go:escapeJsxAttributeString
pub fn escape_jsx_attribute_string(bytes: &[u8], quote: QuoteChar) -> Vec<u8> {
    escape_string_with_flags(
        bytes,
        quote,
        LiteralEscapeFlags::JSX_ATTRIBUTE_ESCAPE | LiteralEscapeFlags::NEVER_ASCII_ESCAPE,
    )
}
