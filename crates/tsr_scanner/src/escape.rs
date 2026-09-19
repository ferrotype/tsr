use crate::identifier::is_identifier_part;
use crate::number::saturated_radix;
use crate::utilities::{is_digit, is_hex_digit, is_octal_digit};
use crate::{escape_flags as ef, Scanner};
use std::borrow::Cow;
use ts_ast::token_flags as flags;
use ts_diagnostics as diagnostics;
use ts_jsstring::wtf8::{
    decode_utf8, encode_rune, is_high_surrogate, is_low_surrogate, surrogate_pair_to_code_point,
};

impl<'src> Scanner<'src> {
    /// port: tsc/internal/scanner/scanner.go:Scanner.scanEscapeSequence
    pub(crate) fn scan_escape_sequence(&mut self, escape_flags: i32) -> Cow<'src, [u8]> {
        let start = self.state.pos;
        self.state.pos += 1;
        let mut ch = self.char();
        if ch < 0 {
            self.error(diagnostics::Unexpected_end_of_text);
            return Cow::Borrowed(b"");
        }
        self.state.pos += 1;
        let report = escape_flags & ef::REPORT_INVALID_ESCAPE_ERRORS != 0;
        match ch as u8 {
            b'0'..=b'7' => {
                if ch == i32::from(b'0') && !is_digit(self.char()) {
                    return Cow::Borrowed(b"\0");
                }
                if ch <= i32::from(b'3') && is_octal_digit(self.char()) {
                    self.state.pos += 1;
                }
                if is_octal_digit(self.char()) {
                    self.state.pos += 1;
                }
                self.state.token_flags |= flags::CONTAINS_INVALID_ESCAPE;
                if report {
                    let code = saturated_radix(
                        self.slice(start + 1, self.state.pos),
                        8,
                        i64::from(i32::MAX),
                    );
                    let diagnostic = if escape_flags & ef::REGULAR_EXPRESSION != 0
                        && escape_flags & ef::ATOM_ESCAPE == 0
                        && ch != i32::from(b'0')
                    {
                        diagnostics::Octal_escape_sequences_and_backreferences_are_not_allowed_in_a_character_class_If_this_was_intended_as_an_escape_sequence_use_the_syntax_0_instead
                    } else {
                        diagnostics::Octal_escape_sequences_are_not_allowed_Use_the_syntax_0
                    };
                    self.error_at(
                        diagnostic,
                        start,
                        self.state.pos - start,
                        vec![format!("\\x{code:02x}").into()],
                    );
                    return Cow::Owned(encode_rune(code as i32));
                }
                Cow::Borrowed(self.slice(start, self.state.pos))
            }
            b'8' | b'9' => {
                self.state.token_flags |= flags::CONTAINS_INVALID_ESCAPE;
                if report {
                    if escape_flags & ef::REGULAR_EXPRESSION != 0
                        && escape_flags & ef::ATOM_ESCAPE == 0
                    {
                        self.error_at(diagnostics::Decimal_escape_sequences_and_backreferences_are_not_allowed_in_a_character_class,
                            start, self.state.pos - start, vec![]);
                    } else {
                        self.error_at(
                            diagnostics::Escape_sequence_0_is_not_allowed,
                            start,
                            self.state.pos - start,
                            vec![self.slice(start, self.state.pos).into()],
                        );
                    }
                    return Cow::Borrowed(self.slice(start + 1, self.state.pos));
                }
                Cow::Borrowed(self.slice(start, self.state.pos))
            }
            b'b' => Cow::Borrowed(b"\x08"),
            b't' => Cow::Borrowed(b"\t"),
            b'n' => Cow::Borrowed(b"\n"),
            b'v' => Cow::Borrowed(b"\x0b"),
            b'f' => Cow::Borrowed(b"\x0c"),
            b'r' => Cow::Borrowed(b"\r"),
            b'\'' => Cow::Borrowed(b"'"),
            b'"' => Cow::Borrowed(b"\""),
            b'u' => {
                let extended = self.char() == i32::from(b'{');
                self.state.pos -= 2;
                let code_point = self.scan_unicode_escape(report);
                if extended {
                    if escape_flags & ef::ALLOW_EXTENDED_UNICODE_ESCAPE == 0 {
                        self.state.token_flags |= flags::CONTAINS_INVALID_ESCAPE;
                        if report {
                            self.error_at(diagnostics::Unicode_escape_sequences_are_only_available_when_the_Unicode_u_flag_or_the_Unicode_Sets_v_flag_is_set,
                                start, self.state.pos - start, vec![]);
                        }
                    }
                    if code_point < 0 {
                        return Cow::Borrowed(self.slice(start, self.state.pos));
                    }
                    if escape_flags & ef::REGULAR_EXPRESSION == 0 && is_high_surrogate(code_point) {
                        if let Some(combined) = self.scan_low_surrogate_escape(code_point) {
                            return Cow::Owned(encode_rune(combined));
                        }
                    }
                    return Cow::Owned(encode_rune(code_point));
                }
                if code_point < 0 {
                    return Cow::Borrowed(self.slice(start, self.state.pos));
                }
                if is_high_surrogate(code_point) {
                    if escape_flags & ef::REGULAR_EXPRESSION == 0 {
                        if let Some(combined) = self.scan_low_surrogate_escape(code_point) {
                            return Cow::Owned(encode_rune(combined));
                        }
                    } else if escape_flags & ef::ANY_UNICODE_MODE != 0
                        && self.char() == i32::from(b'\\')
                        && self.char_at(1) == i32::from(b'u')
                        && self.char_at(2) != i32::from(b'{')
                    {
                        let saved_pos = self.state.pos;
                        let next = self.scan_unicode_escape(report);
                        if is_low_surrogate(next) {
                            return Cow::Owned(encode_rune(surrogate_pair_to_code_point(
                                code_point, next,
                            )));
                        }
                        // This regexp path restores only pos, unlike scanLowSurrogateEscape.
                        self.state.pos = saved_pos;
                    }
                }
                Cow::Owned(encode_rune(code_point))
            }
            b'x' => {
                while self.state.pos < start + 4 {
                    if !is_hex_digit(self.char()) {
                        self.state.token_flags |= flags::CONTAINS_INVALID_ESCAPE;
                        if report {
                            self.error(diagnostics::Hexadecimal_digit_expected);
                        }
                        return Cow::Borrowed(self.slice(start, self.state.pos));
                    }
                    self.state.pos += 1;
                }
                self.state.token_flags |= flags::HEX_ESCAPE;
                Cow::Owned(encode_rune(saturated_radix(
                    self.slice(start + 2, self.state.pos),
                    16,
                    i64::from(i32::MAX),
                ) as i32))
            }
            b'\r' => {
                if self.char() == i32::from(b'\n') {
                    self.state.pos += 1;
                }
                Cow::Borrowed(b"")
            }
            b'\n' => Cow::Borrowed(b""),
            _ => {
                if ch >= 128 {
                    self.state.pos -= 1;
                    let (rune, size) = decode_utf8(self.tail(self.state.pos));
                    ch = rune;
                    self.state.pos += size as i64;
                }
                if matches!(ch, 0x2028 | 0x2029) {
                    return Cow::Borrowed(b"");
                }
                if escape_flags & ef::ANY_UNICODE_MODE != 0
                    || escape_flags & ef::REGULAR_EXPRESSION != 0
                        && escape_flags & ef::ANNEX_B == 0
                        && is_identifier_part(ch)
                {
                    self.error_at(
                        diagnostics::This_character_cannot_be_escaped_in_a_regular_expression,
                        start,
                        self.state.pos - start,
                        vec![],
                    );
                }
                Cow::Owned(encode_rune(ch))
            }
        }
    }

    /// port: tsc/internal/scanner/scanner.go:Scanner.scanUnicodeEscape
    pub(crate) fn scan_unicode_escape(&mut self, report: bool) -> i32 {
        self.state.pos += 2;
        let start = self.state.pos;
        let extended = self.char() == i32::from(b'{');
        let digits = if extended {
            self.state.pos += 1;
            self.scan_hex_digits(1, true, false)
        } else {
            self.state.token_flags |= flags::UNICODE_ESCAPE;
            self.scan_hex_digits(4, false, false)
        };
        if digits.as_bytes().is_empty() {
            self.state.token_flags |= flags::CONTAINS_INVALID_ESCAPE;
            if report {
                self.error(diagnostics::Hexadecimal_digit_expected);
            }
            return -1;
        }
        let value = saturated_radix(digits.as_bytes(), 16, i64::from(i32::MAX));
        if extended {
            let mut invalid = false;
            if value > 0x10_ffff {
                if report {
                    self.error_at(diagnostics::An_extended_Unicode_escape_value_must_be_between_0x0_and_0x10FFFF_inclusive,
                        start + 1, self.state.pos - start - 1, vec![]);
                }
                invalid = true;
            }
            if self.state.pos >= self.end {
                if report {
                    self.error(diagnostics::Unexpected_end_of_text);
                }
                invalid = true;
            } else if self.char() == i32::from(b'}') {
                self.state.pos += 1;
            } else {
                if report {
                    self.error(diagnostics::Unterminated_Unicode_escape_sequence);
                }
                invalid = true;
            }
            if invalid {
                self.state.token_flags |= flags::CONTAINS_INVALID_ESCAPE;
                return -1;
            }
            self.state.token_flags |= flags::EXTENDED_UNICODE_ESCAPE;
        }
        value as i32
    }

    /// port: tsc/internal/scanner/scanner.go:Scanner.scanLowSurrogateEscape
    pub(crate) fn scan_low_surrogate_escape(&mut self, high: i32) -> Option<i32> {
        if self.char() != i32::from(b'\\') || self.char_at(1) != i32::from(b'u') {
            return None;
        }
        let saved_pos = self.state.pos;
        let saved_flags = self.state.token_flags;
        let low = self.scan_unicode_escape(false);
        if is_low_surrogate(low) {
            return Some(surrogate_pair_to_code_point(high, low));
        }
        self.state.pos = saved_pos;
        self.state.token_flags = saved_flags;
        None
    }
    /// port: tsc/internal/scanner/scanner.go:Scanner.peekUnicodeEscape
    pub(crate) fn peek_unicode_escape(&mut self) -> i32 {
        if self.char_at(1) != i32::from(b'u') {
            return -1;
        }
        let saved_pos = self.state.pos;
        let saved_flags = self.state.token_flags;
        let code_point = self.scan_unicode_escape(false);
        self.state.pos = saved_pos;
        self.state.token_flags = saved_flags;
        code_point
    }
}
