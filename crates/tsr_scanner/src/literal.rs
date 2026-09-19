use crate::{escape_flags, Scanner, TokenValue};
use ts_ast::{token_flags as flags, SyntaxKind};
use ts_diagnostics as diagnostics;

impl<'src> Scanner<'src> {
    /// port: tsc/internal/scanner/scanner.go:Scanner.scanString
    pub(crate) fn scan_string(&mut self, jsx_attribute: bool) -> TokenValue<'src> {
        let quote = self.char();
        if quote == i32::from(b'\'') {
            self.state.token_flags |= flags::SINGLE_QUOTE;
        }
        self.state.pos += 1;
        if let Some(length) = self
            .tail(self.state.pos)
            .iter()
            .position(|&byte| i32::from(byte) == quote)
        {
            let value = self.slice(self.state.pos, self.state.pos + length as i64);
            if jsx_attribute || !value.iter().any(|&b| matches!(b, b'\\' | b'\r' | b'\n')) {
                self.state.pos += length as i64 + 1;
                return TokenValue::Borrowed(value);
            }
        }
        let mut result: Option<Vec<u8>> = None;
        let mut start = self.state.pos;
        let value_end;
        loop {
            let ch = self.char();
            if ch < 0 || !jsx_attribute && matches!(u8::try_from(ch), Ok(b'\n' | b'\r')) {
                value_end = self.state.pos;
                self.state.token_flags |= flags::UNTERMINATED;
                self.error(diagnostics::Unterminated_string_literal);
                break;
            }
            if ch == quote {
                value_end = self.state.pos;
                self.state.pos += 1;
                break;
            }
            if ch == i32::from(b'\\') && !jsx_attribute {
                let result = result.get_or_insert_with(Vec::new);
                result.extend_from_slice(self.slice(start, self.state.pos));
                result.extend_from_slice(
                    &self.scan_escape_sequence(escape_flags::STRING | escape_flags::REPORT_ERRORS),
                );
                start = self.state.pos;
                continue;
            }
            self.state.pos += 1;
        }
        let tail = self.slice(start, value_end);
        if let Some(mut result) = result {
            result.extend_from_slice(tail);
            result.into()
        } else {
            TokenValue::Borrowed(tail)
        }
    }

    /// port: tsc/internal/scanner/scanner.go:Scanner.scanTemplateAndSetTokenValue
    pub(crate) fn scan_template_and_set_token_value(&mut self, report: bool) -> SyntaxKind {
        let backtick = self.char() == i32::from(b'`');
        self.state.pos += 1;
        let mut start = self.state.pos;
        let mut result: Option<Vec<u8>> = None;
        let token;
        let value_end;
        loop {
            self.scan_ascii_while(|b| !matches!(b, b'`' | b'$' | b'\\' | b'\r'));
            let ch = self.char();
            if ch < 0 || ch == i32::from(b'`') {
                value_end = self.state.pos;
                if ch == i32::from(b'`') {
                    self.state.pos += 1;
                } else {
                    self.state.token_flags |= flags::UNTERMINATED;
                    self.error(diagnostics::Unterminated_template_literal);
                }
                token = if backtick {
                    SyntaxKind::NoSubstitutionTemplateLiteral
                } else {
                    SyntaxKind::TemplateTail
                };
                break;
            }
            if ch == i32::from(b'$') && self.char_at(1) == i32::from(b'{') {
                value_end = self.state.pos;
                self.state.pos += 2;
                token = if backtick {
                    SyntaxKind::TemplateHead
                } else {
                    SyntaxKind::TemplateMiddle
                };
                break;
            }
            if ch == i32::from(b'\\') {
                let result = result.get_or_insert_with(Vec::new);
                result.extend_from_slice(self.slice(start, self.state.pos));
                result.extend_from_slice(&self.scan_escape_sequence(
                    escape_flags::STRING
                        | if report {
                            escape_flags::REPORT_ERRORS
                        } else {
                            0
                        },
                ));
                start = self.state.pos;
                continue;
            }
            if ch == i32::from(b'\r') {
                let result = result.get_or_insert_with(Vec::new);
                result.extend_from_slice(self.slice(start, self.state.pos));
                self.state.pos += 1;
                if self.char() == i32::from(b'\n') {
                    self.state.pos += 1;
                }
                result.push(b'\n');
                start = self.state.pos;
                continue;
            }
            self.state.pos += 1;
        }
        let tail = self.slice(start, value_end);
        self.state.token_value = if let Some(mut result) = result {
            result.extend_from_slice(tail);
            result.into()
        } else {
            TokenValue::Borrowed(tail)
        };
        token
    }
}
