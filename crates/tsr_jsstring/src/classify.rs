/// port: tsc/internal/stringutil/util.go:IsDigit
pub fn is_digit(ch: i32) -> bool {
    (i32::from(b'0')..=i32::from(b'9')).contains(&ch)
}
/// port: tsc/internal/stringutil/util.go:IsOctalDigit
pub fn is_octal_digit(ch: i32) -> bool {
    (i32::from(b'0')..=i32::from(b'7')).contains(&ch)
}
/// port: tsc/internal/stringutil/util.go:IsHexDigit
pub fn is_hex_digit(ch: i32) -> bool {
    is_digit(ch)
        || (i32::from(b'a')..=i32::from(b'f')).contains(&ch)
        || (i32::from(b'A')..=i32::from(b'F')).contains(&ch)
}
/// port: tsc/internal/stringutil/util.go:IsASCIILetter
pub fn is_ascii_letter(ch: i32) -> bool {
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
