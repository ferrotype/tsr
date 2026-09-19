use num_traits::ToPrimitive;
use ts_jsstring::wtf8::decode_utf8;

use crate::{bigint::parse_go_big_int, Number};

/// Go's StringToNumber grammar, including empty input and signed zero.
/// port: tsc/internal/jsnum/string.go:FromString
pub fn from_string(bytes: &[u8]) -> Number {
    let bytes = trim_space(bytes);
    match bytes {
        b"" => return Number::new(0.0),
        b"Infinity" | b"+Infinity" => return Number::new(f64::INFINITY),
        b"-Infinity" => return Number::new(f64::NEG_INFINITY),
        _ => {}
    }
    // Every accepted rune here is ASCII, so rejecting non-ASCII bytes is exact
    // even for malformed sequences that Go decodes one byte at a time.
    if !bytes.iter().all(|&byte| is_number_rune(i32::from(byte))) {
        return Number::new(f64::NAN);
    }
    if let Some(number) = try_parse_int(bytes) {
        return Number::new(number);
    }
    let (bytes, negative) = bytes
        .strip_prefix(b"-")
        .map_or((bytes, false), |rest| (rest, true));
    let bytes = if negative {
        bytes
    } else {
        bytes.strip_prefix(b"+").unwrap_or(bytes)
    };
    if !bytes
        .first()
        .is_some_and(|byte| byte.is_ascii_digit() || *byte == b'.')
    {
        return Number::new(f64::NAN);
    }
    let value = parse_float_string(bytes);
    if value.is_nan() {
        return Number::new(f64::NAN);
    }
    Number::new(value.copysign(if negative { -1.0 } else { 1.0 }))
}

/// port: tsc/internal/jsnum/string.go:isStrWhiteSpace
fn is_str_white_space(rune: i32) -> bool {
    // unicode.Zs from the pinned Go 1.27.1 table, plus the explicit ECMA set.
    matches!(rune, 0x09..=0x0d | 0x20 | 0xa0 | 0x1680 | 0x2000..=0x200a
        | 0x2028 | 0x2029 | 0x202f | 0x205f | 0x3000 | 0xfeff)
}

fn trim_space(mut bytes: &[u8]) -> &[u8] {
    while !bytes.is_empty() {
        let (rune, width) = decode_utf8(bytes);
        if !is_str_white_space(rune) {
            break;
        }
        bytes = &bytes[width..];
    }
    while !bytes.is_empty() {
        let mut start = bytes.len() - 1;
        while start > 0 && bytes[start] & 0xc0 == 0x80 && bytes.len() - start < 4 {
            start -= 1;
        }
        let (rune, width) = decode_utf8(&bytes[start..]);
        if start + width != bytes.len() || !is_str_white_space(rune) {
            break;
        }
        bytes = &bytes[..start];
    }
    bytes
}

/// None means the signed/fractional/exponent parser should try next; Some(NaN)
/// is a final rejection by the integer grammar, matching Go's `(n, ok)` result.
/// port: tsc/internal/jsnum/string.go:tryParseInt
fn try_parse_int(mut bytes: &[u8]) -> Option<f64> {
    let mut radix = 10;
    let mut prefixed = false;
    if bytes.len() > 2 {
        match &bytes[..2] {
            b"0b" | b"0B" => {
                if !is_all_binary_digits(&bytes[2..]) {
                    return Some(f64::NAN);
                }
                radix = 2;
                prefixed = true;
            }
            b"0o" | b"0O" => {
                if !is_all_octal_digits(&bytes[2..]) {
                    return Some(f64::NAN);
                }
                radix = 8;
                prefixed = true;
            }
            b"0x" | b"0X" => {
                if !is_all_hex_digits(&bytes[2..]) {
                    return Some(f64::NAN);
                }
                radix = 16;
                prefixed = true;
            }
            _ => {}
        }
    }
    let digits = if prefixed {
        &bytes[2..]
    } else {
        bytes = trim_leading_zeros(bytes);
        if !is_all_digits(bytes) {
            return None;
        }
        bytes
    };
    let text = std::str::from_utf8(digits).expect("integer grammar accepted only ASCII digits");
    if let Ok(integer) = i64::from_str_radix(text, radix) {
        return Some(integer as f64);
    }
    Some(
        parse_go_big_int(bytes)
            .and_then(|integer| integer.to_f64())
            .unwrap_or(f64::NAN),
    )
}

/// port: tsc/internal/jsnum/string.go:parseFloatString
fn parse_float_string(bytes: &[u8]) -> f64 {
    let (mut integer, mut fraction, exponent, has_dot, has_exp) =
        if let Some(dot) = bytes.iter().position(|byte| *byte == b'.') {
            let (fraction, exponent, found) = cut_any(&bytes[dot + 1..], b"eE");
            (&bytes[..dot], fraction, exponent, true, found)
        } else {
            let (integer, exponent, found) = cut_any(bytes, b"eE");
            (integer, &b""[..], exponent, false, found)
        };
    let mut normalized = Vec::with_capacity(bytes.len() + 3);
    if integer.is_empty() {
        if (has_dot && fraction.is_empty()) || (has_exp && exponent.is_empty()) {
            return f64::NAN;
        }
        normalized.push(b'0');
    } else {
        integer = trim_leading_zeros(integer);
        if !is_all_digits(integer) {
            return f64::NAN;
        }
        normalized.extend_from_slice(integer);
    }
    if has_dot {
        normalized.push(b'.');
        if fraction.is_empty() {
            normalized.push(b'0');
        } else {
            fraction = trim_trailing_zeros(fraction);
            if !is_all_digits(fraction) {
                return f64::NAN;
            }
            normalized.extend_from_slice(fraction);
        }
    }
    if has_exp {
        normalized.push(b'e');
        let exponent = if let Some(rest) = exponent.strip_prefix(b"-") {
            normalized.push(b'-');
            rest
        } else {
            exponent.strip_prefix(b"+").unwrap_or(exponent)
        };
        let exponent = trim_leading_zeros(exponent);
        if !is_all_digits(exponent) {
            return f64::NAN;
        }
        normalized.extend_from_slice(exponent);
    }
    string_to_float64(&normalized)
}

/// The caller's cut set is ASCII e/E; slices remain raw byte views.
/// port: tsc/internal/jsnum/string.go:cutAny
fn cut_any<'a>(bytes: &'a [u8], cutset: &[u8]) -> (&'a [u8], &'a [u8], bool) {
    bytes
        .iter()
        .position(|byte| cutset.contains(byte))
        .map_or((bytes, b"", false), |index| {
            (&bytes[..index], &bytes[index + 1..], true)
        })
}

/// port: tsc/internal/jsnum/string.go:trimLeadingZeros
fn trim_leading_zeros(bytes: &[u8]) -> &[u8] {
    if bytes.starts_with(b"0") {
        let start = bytes
            .iter()
            .position(|byte| *byte != b'0')
            .unwrap_or(bytes.len() - 1);
        &bytes[start..]
    } else {
        bytes
    }
}

/// port: tsc/internal/jsnum/string.go:trimTrailingZeros
fn trim_trailing_zeros(bytes: &[u8]) -> &[u8] {
    if bytes.ends_with(b"0") {
        let end = bytes
            .iter()
            .rposition(|byte| *byte != b'0')
            .map_or(1, |index| index + 1);
        &bytes[..end]
    } else {
        bytes
    }
}

/// Rust's parser returns infinity/zero on range overflow/underflow, matching
/// Go ParseFloat's value when its caller accepts ErrRange.
/// port: tsc/internal/jsnum/string.go:stringToFloat64
fn string_to_float64(bytes: &[u8]) -> f64 {
    std::str::from_utf8(bytes)
        .expect("normalized float grammar is ASCII")
        .parse::<f64>()
        .unwrap_or(f64::NAN)
}

/// port: tsc/internal/jsnum/string.go:isAllDigits
fn is_all_digits(bytes: &[u8]) -> bool {
    bytes.iter().all(u8::is_ascii_digit)
}
/// port: tsc/internal/jsnum/string.go:isAllBinaryDigits
fn is_all_binary_digits(bytes: &[u8]) -> bool {
    bytes.iter().all(|byte| matches!(byte, b'0' | b'1'))
}
/// port: tsc/internal/jsnum/string.go:isAllOctalDigits
fn is_all_octal_digits(bytes: &[u8]) -> bool {
    bytes.iter().all(|byte| matches!(byte, b'0'..=b'7'))
}
/// port: tsc/internal/jsnum/string.go:isAllHexDigits
fn is_all_hex_digits(bytes: &[u8]) -> bool {
    bytes.iter().all(u8::is_ascii_hexdigit)
}
/// port: tsc/internal/jsnum/string.go:isNumberRune
fn is_number_rune(rune: i32) -> bool {
    matches!(rune, 0x30..=0x39 | 0x61..=0x66 | 0x41..=0x46
        | 0x2e | 0x2d | 0x2b | 0x78 | 0x58 | 0x6f | 0x4f)
}
