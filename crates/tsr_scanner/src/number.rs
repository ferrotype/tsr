use ts_ast::{token_flags as flags, SyntaxKind};
use ts_diagnostics as diagnostics;

use crate::identifier::is_identifier_start;
use crate::utilities::{is_digit, is_hex_digit, is_octal_digit};
use crate::{IdentifierVariant, Scanner, TokenValue};

/// The scanner deliberately ignores strconv.ParseInt's range error.
pub(crate) fn saturated_radix(bytes: &[u8], base: i64, maximum: i64) -> i64 {
    bytes.iter().fold(0_i64, |value, &byte| {
        let digit = if byte.is_ascii_digit() {
            i64::from(byte - b'0')
        } else {
            i64::from(byte.to_ascii_lowercase() - b'a') + 10
        };
        value
            .checked_mul(base)
            .and_then(|n| n.checked_add(digit))
            .unwrap_or(maximum)
            .min(maximum)
    })
}

impl<'src> Scanner<'src> {
    fn normalize_number_value(&mut self) {
        let value = ts_jsnum::from_string(self.token_value())
            .to_string()
            .into_bytes();
        if value != self.token_value() {
            self.state.token_value = value.into();
        }
    }

    /// port: tsc/internal/scanner/scanner.go:Scanner.scanNumber
    pub(crate) fn scan_number(&mut self) -> SyntaxKind {
        let mut start = self.state.pos;
        let fixed_part;
        if self.char() == i32::from(b'0') {
            self.state.pos += 1;
            if self.char() == i32::from(b'_') {
                self.state.token_flags |=
                    flags::CONTAINS_SEPARATOR | flags::CONTAINS_INVALID_SEPARATOR;
                self.error_at(
                    diagnostics::Numeric_separators_are_not_allowed_here,
                    self.state.pos,
                    1,
                    vec![],
                );
                self.state.pos = start;
                fixed_part = self.scan_number_fragment();
            } else {
                let (digits, is_octal) = self.scan_digits();
                if digits.is_empty() {
                    fixed_part = TokenValue::Static(b"0");
                } else if !is_octal {
                    self.state.token_flags |= flags::CONTAINS_LEADING_ZERO;
                    fixed_part = TokenValue::Borrowed(digits);
                } else {
                    let value = saturated_radix(digits, 8, i64::MAX);
                    self.state.token_value = value.to_string().into_bytes().into();
                    self.state.token_flags |= flags::OCTAL;
                    let with_minus = self.state.token == SyntaxKind::MinusToken;
                    let replacement = format!("{}0o{value:o}", if with_minus { "-" } else { "" });
                    if with_minus {
                        start -= 1;
                    }
                    self.error_at(
                        diagnostics::Octal_literals_are_not_allowed_Use_the_syntax_0,
                        start,
                        self.state.pos - start,
                        vec![replacement.into()],
                    );
                    return SyntaxKind::NumericLiteral;
                }
            }
        } else {
            fixed_part = self.scan_number_fragment();
        }
        let fixed_part_end = self.state.pos;
        let mut fractional_part = TokenValue::default();
        let mut exponent_preamble: &[u8] = b"";
        let mut exponent_part = TokenValue::default();
        if self.char() == i32::from(b'.') {
            self.state.pos += 1;
            fractional_part = self.scan_number_fragment();
        }
        let mut end = self.state.pos;
        if matches!(u8::try_from(self.char()), Ok(b'E' | b'e')) {
            self.state.pos += 1;
            self.state.token_flags |= flags::SCIENTIFIC;
            if matches!(u8::try_from(self.char()), Ok(b'+' | b'-')) {
                self.state.pos += 1;
            }
            let start_numeric_part = self.state.pos;
            exponent_part = self.scan_number_fragment();
            if exponent_part.as_bytes().is_empty() {
                self.error(diagnostics::Digit_expected);
            } else {
                exponent_preamble = self.slice(end, start_numeric_part);
                end = self.state.pos;
            }
        }
        if self.state.token_flags & flags::CONTAINS_SEPARATOR != 0 {
            let mut value = fixed_part.as_bytes().to_vec();
            if !fractional_part.as_bytes().is_empty() {
                value.push(b'.');
                value.extend_from_slice(fractional_part.as_bytes());
            }
            if !exponent_part.as_bytes().is_empty() {
                value.extend_from_slice(exponent_preamble);
                value.extend_from_slice(exponent_part.as_bytes());
            }
            self.state.token_value = value.into();
        } else {
            self.state.token_value = TokenValue::Borrowed(self.slice(start, end));
        }
        if self.state.token_flags & flags::CONTAINS_LEADING_ZERO != 0 {
            self.error_at(
                diagnostics::Decimals_with_leading_zeros_are_not_allowed,
                start,
                self.state.pos - start,
                vec![],
            );
            self.normalize_number_value();
            return SyntaxKind::NumericLiteral;
        }
        let result = if fixed_part_end == self.state.pos {
            self.scan_big_int_suffix()
        } else {
            self.normalize_number_value();
            SyntaxKind::NumericLiteral
        };
        let (ch, _) = self.char_and_size();
        if is_identifier_start(ch) {
            let id_start = self.state.pos;
            let id = self.scan_identifier_parts(IdentifierVariant::Standard);
            if result != SyntaxKind::BigIntLiteral
                && id.as_bytes().len() == 1
                && self.text[id_start as usize] == b'n'
            {
                if self.state.token_flags & flags::SCIENTIFIC != 0 {
                    self.error_at(
                        diagnostics::A_bigint_literal_cannot_use_exponential_notation,
                        start,
                        self.state.pos - start,
                        vec![],
                    );
                    return result;
                }
                if fixed_part_end < id_start {
                    self.error_at(
                        diagnostics::A_bigint_literal_must_be_an_integer,
                        start,
                        self.state.pos - start,
                        vec![],
                    );
                    return result;
                }
            }
            self.error_at(
                diagnostics::An_identifier_or_keyword_cannot_immediately_follow_a_numeric_literal,
                id_start,
                self.state.pos - id_start,
                vec![],
            );
            self.state.pos = id_start;
        }
        result
    }

    /// port: tsc/internal/scanner/scanner.go:Scanner.scanNumberFragment
    fn scan_number_fragment(&mut self) -> TokenValue<'src> {
        let mut start = self.state.pos;
        let mut allow_separator = false;
        let mut previous_separator = false;
        let mut result = Vec::new();
        loop {
            let before = self.state.pos;
            self.scan_ascii_while(|b| b.is_ascii_digit());
            if self.state.pos > before {
                allow_separator = true;
                previous_separator = false;
            }
            if self.char() != i32::from(b'_') {
                break;
            }
            self.state.token_flags |= flags::CONTAINS_SEPARATOR;
            if allow_separator {
                allow_separator = false;
                previous_separator = true;
                result.extend_from_slice(self.slice(start, self.state.pos));
            } else {
                self.state.token_flags |= flags::CONTAINS_INVALID_SEPARATOR;
                let diagnostic = if previous_separator {
                    diagnostics::Multiple_consecutive_numeric_separators_are_not_permitted
                } else {
                    diagnostics::Numeric_separators_are_not_allowed_here
                };
                self.error_at(diagnostic, self.state.pos, 1, vec![]);
            }
            self.state.pos += 1;
            start = self.state.pos;
        }
        if previous_separator {
            self.state.token_flags |= flags::CONTAINS_INVALID_SEPARATOR;
            self.error_at(
                diagnostics::Numeric_separators_are_not_allowed_here,
                self.state.pos - 1,
                1,
                vec![],
            );
        }
        let tail = self.slice(start, self.state.pos);
        if result.is_empty() {
            TokenValue::Borrowed(tail)
        } else {
            result.extend_from_slice(tail);
            result.into()
        }
    }

    /// port: tsc/internal/scanner/scanner.go:Scanner.scanDigits
    fn scan_digits(&mut self) -> (&'src [u8], bool) {
        let start = self.state.pos;
        let mut octal = true;
        while is_digit(self.char()) {
            if !is_octal_digit(self.char()) {
                octal = false;
            }
            self.state.pos += 1;
        }
        (self.slice(start, self.state.pos), octal)
    }

    /// port: tsc/internal/scanner/scanner.go:Scanner.scanHexDigits
    pub(crate) fn scan_hex_digits(
        &mut self,
        minimum: i64,
        many: bool,
        separators: bool,
    ) -> TokenValue<'src> {
        let start = self.state.pos;
        let mut count = 0;
        let mut allow_separator = false;
        let mut previous_separator = false;
        while count < minimum || many {
            let ch = self.char();
            if is_hex_digit(ch) {
                allow_separator = separators;
                previous_separator = false;
                count += 1;
            } else if separators && ch == i32::from(b'_') {
                self.state.token_flags |= flags::CONTAINS_SEPARATOR;
                if allow_separator {
                    allow_separator = false;
                    previous_separator = true;
                } else {
                    let diagnostic = if previous_separator {
                        diagnostics::Multiple_consecutive_numeric_separators_are_not_permitted
                    } else {
                        diagnostics::Numeric_separators_are_not_allowed_here
                    };
                    self.error_at(diagnostic, self.state.pos, 1, vec![]);
                }
            } else {
                break;
            }
            self.state.pos += 1;
        }
        if previous_separator {
            self.error_at(
                diagnostics::Numeric_separators_are_not_allowed_here,
                self.state.pos - 1,
                1,
                vec![],
            );
        }
        if count < minimum {
            return TokenValue::default();
        }
        let original = self.slice(start, self.state.pos);
        if let Some(cached) = self.hex_digit_cache.get(original) {
            return cached.clone();
        }
        let changed = original
            .iter()
            .any(|b| b.is_ascii_uppercase() || *b == b'_');
        let value = if changed {
            original
                .iter()
                .filter(|&&b| b != b'_' || self.state.token_flags & flags::CONTAINS_SEPARATOR == 0)
                .map(u8::to_ascii_lowercase)
                .collect::<Vec<_>>()
                .into()
        } else {
            TokenValue::Borrowed(original)
        };
        self.hex_digit_cache
            .insert(TokenValue::Borrowed(original), value.clone());
        value
    }

    /// port: tsc/internal/scanner/scanner.go:Scanner.scanBinaryOrOctalDigits
    pub(crate) fn scan_binary_or_octal_digits(&mut self, base: i32) -> Vec<u8> {
        let mut result = Vec::new();
        let mut allow_separator = false;
        let mut previous_separator = false;
        loop {
            let ch = self.char();
            if is_digit(ch) && ch - i32::from(b'0') < base {
                result.push(ch as u8);
                allow_separator = true;
                previous_separator = false;
            } else if ch == i32::from(b'_') {
                self.state.token_flags |= flags::CONTAINS_SEPARATOR;
                if allow_separator {
                    allow_separator = false;
                    previous_separator = true;
                } else {
                    let diagnostic = if previous_separator {
                        diagnostics::Multiple_consecutive_numeric_separators_are_not_permitted
                    } else {
                        diagnostics::Numeric_separators_are_not_allowed_here
                    };
                    self.error_at(diagnostic, self.state.pos, 1, vec![]);
                }
            } else {
                break;
            }
            self.state.pos += 1;
        }
        if previous_separator {
            self.error_at(
                diagnostics::Numeric_separators_are_not_allowed_here,
                self.state.pos - 1,
                1,
                vec![],
            );
        }
        result
    }

    /// port: tsc/internal/scanner/scanner.go:Scanner.scanBigIntSuffix
    pub(crate) fn scan_big_int_suffix(&mut self) -> SyntaxKind {
        if self.char() == i32::from(b'n') {
            self.append_token_value(b"n");
            if self.state.token_flags & flags::BINARY_OR_OCTAL_SPECIFIER != 0 {
                let mut value = ts_jsnum::parse_pseudo_big_int(self.token_value());
                value.push(b'n');
                self.state.token_value = value.into();
            }
            self.state.pos += 1;
            return SyntaxKind::BigIntLiteral;
        }
        if let Some(value) = self.number_cache.get(self.token_value()) {
            self.state.token_value = value.clone();
        } else {
            let original = self.state.token_value.clone();
            self.normalize_number_value();
            self.number_cache
                .insert(original, self.state.token_value.clone());
        }
        SyntaxKind::NumericLiteral
    }
}
