use ts_ast::{token_flags as flags, SyntaxKind};
use ts_diagnostics as diagnostics;
use ts_jsstring::wtf8::{decode_utf8, RUNE_ERROR};

use crate::identifier::{get_identifier_token, is_identifier_part, is_identifier_start};
use crate::regexp::{self, reg_exp_flags};
use crate::trivia::is_conflict_marker_trivia;
use crate::utilities::{
    decode_last_utf8, is_line_break, is_white_space_like, is_white_space_single_line,
    token_is_identifier_or_keyword,
};
use crate::{IdentifierVariant, Scanner, TokenValue};

impl Scanner<'_> {
    /// port: tsc/internal/scanner/scanner.go:Scanner.ReScanLessThanToken
    pub fn rescan_less_than_token(&mut self) -> SyntaxKind {
        if self.state.token == SyntaxKind::LessThanLessThanToken {
            self.state.pos = self.state.token_start + 1;
            self.state.token = SyntaxKind::LessThanToken;
        }
        self.state.token
    }
    /// port: tsc/internal/scanner/scanner.go:Scanner.ReScanGreaterThanToken
    pub fn rescan_greater_than_token(&mut self) -> SyntaxKind {
        if self.state.token == SyntaxKind::GreaterThanToken {
            self.rescan_greater_than_token_inner();
        }
        self.state.token
    }
    /// port: tsc/internal/scanner/scanner.go:Scanner.reScanGreaterThanTokenInner
    fn rescan_greater_than_token_inner(&mut self) {
        use SyntaxKind as K;
        self.state.pos = self.state.token_start + 1;
        if self.char() == i32::from(b'>') {
            if self.char_at(1) == i32::from(b'>') {
                if self.char_at(2) == i32::from(b'=') {
                    self.take_token(K::GreaterThanGreaterThanGreaterThanEqualsToken, 3);
                } else {
                    self.take_token(K::GreaterThanGreaterThanGreaterThanToken, 2);
                }
            } else if self.char_at(1) == i32::from(b'=') {
                self.take_token(K::GreaterThanGreaterThanEqualsToken, 2);
            } else {
                self.take_token(K::GreaterThanGreaterThanToken, 1);
            }
        } else if self.char() == i32::from(b'=') {
            self.take_token(K::GreaterThanEqualsToken, 1);
        }
    }
    /// port: tsc/internal/scanner/scanner.go:Scanner.ReScanTemplateToken
    pub fn rescan_template_token(&mut self, tagged: bool) -> SyntaxKind {
        self.state.pos = self.state.token_start;
        self.state.token = self.scan_template_and_set_token_value(!tagged);
        self.state.token
    }
    /// port: tsc/internal/scanner/scanner.go:Scanner.ReScanAsteriskEqualsToken
    pub fn rescan_asterisk_equals_token(&mut self) -> SyntaxKind {
        assert!(
            self.state.token == SyntaxKind::AsteriskEqualsToken,
            "'ReScanAsteriskEqualsToken' should only be called on a '*='"
        );
        self.state.pos = self.state.token_start + 1;
        self.take_token(SyntaxKind::EqualsToken, 0)
    }
    /// Go's omitted reporting argument corresponds to false.
    /// port: tsc/internal/scanner/scanner.go:Scanner.ReScanSlashToken
    pub fn rescan_slash_token(&mut self, report_errors: bool) -> SyntaxKind {
        if !matches!(
            self.state.token,
            SyntaxKind::SlashToken | SyntaxKind::SlashEqualsToken
        ) {
            return self.state.token;
        }
        let body_start = self.state.token_start + 1;
        let mut pos = body_start;
        let mut in_escape = false;
        let mut named_capture_groups = false;
        let mut in_class = false;
        loop {
            if pos >= self.end {
                self.state.token_flags |= flags::UNTERMINATED;
                break;
            }
            let ch = self.text[pos as usize];
            if is_line_break(i32::from(ch)) {
                self.state.token_flags |= flags::UNTERMINATED;
                break;
            } else if in_escape {
                in_escape = false;
            } else if ch == b'/' && !in_class {
                break;
            } else if ch == b'[' {
                in_class = true;
            } else if ch == b'\\' {
                in_escape = true;
            } else if ch == b']' {
                in_class = false;
            } else if !in_class
                && ch == b'('
                && pos + 1 < self.end
                && self.text[pos as usize + 1] == b'?'
                && pos + 2 < self.end
                && self.text[pos as usize + 2] == b'<'
                && (pos + 3 >= self.end || !matches!(self.text[pos as usize + 3], b'=' | b'!'))
            {
                named_capture_groups = true;
            }
            pos += 1;
        }
        let body_end = pos;
        if self.state.token_flags & flags::UNTERMINATED != 0 {
            pos = body_start;
            in_escape = false;
            let mut class_depth = 0;
            let mut in_quantifier = false;
            let mut group_depth = 0;
            while pos < body_end {
                let ch = self.text[pos as usize];
                if in_escape {
                    in_escape = false;
                } else if ch == b'\\' {
                    in_escape = true;
                } else if ch == b'[' {
                    class_depth += 1;
                } else if ch == b']' && class_depth != 0 {
                    class_depth -= 1;
                } else if class_depth == 0 {
                    if ch == b'{' {
                        in_quantifier = true;
                    } else if ch == b'}' && in_quantifier {
                        in_quantifier = false;
                    } else if !in_quantifier {
                        if ch == b'(' {
                            group_depth += 1;
                        } else if ch == b')' && group_depth != 0 {
                            group_depth -= 1;
                        } else if matches!(ch, b')' | b']' | b'}') {
                            break;
                        }
                    }
                }
                pos += 1;
            }
            while pos > body_start {
                let (ch, size) = decode_last_utf8(self.slice(0, pos));
                if is_white_space_like(ch) || ch == i32::from(b';') {
                    pos -= size as i64;
                } else {
                    break;
                }
            }
            self.error_at(
                diagnostics::Unterminated_regular_expression_literal,
                self.state.token_start,
                pos - self.state.token_start,
                vec![],
            );
        } else {
            pos += 1;
            let mut regexp_flags = reg_exp_flags::NONE;
            while pos < self.end {
                let (ch, size) = decode_utf8(self.tail(pos));
                if ch == RUNE_ERROR || !is_identifier_part(ch) {
                    break;
                }
                if report_errors {
                    if let Some(flag) = regexp::flag_for_character(ch) {
                        if regexp_flags & flag != 0 {
                            self.error_at(
                                diagnostics::Duplicate_regular_expression_flag,
                                pos,
                                size as i64,
                                vec![],
                            );
                        } else if (regexp_flags | flag) & reg_exp_flags::ANY_UNICODE_MODE
                            == reg_exp_flags::ANY_UNICODE_MODE
                        {
                            self.error_at(diagnostics::The_Unicode_u_flag_and_the_Unicode_Sets_v_flag_cannot_be_set_simultaneously, pos, size as i64, vec![]);
                        } else {
                            regexp_flags |= flag;
                            regexp::check_flag_availability(self, flag, pos, size as i64);
                        }
                    } else {
                        self.error_at(
                            diagnostics::Unknown_regular_expression_flag,
                            pos,
                            size as i64,
                            vec![],
                        );
                    }
                }
                pos += size as i64;
            }
            if report_errors {
                self.state.pos = body_start;
                let saved_end = self.end;
                let saved_start = self.state.token_start;
                let saved_flags = self.state.token_flags;
                self.end = body_end;
                regexp::scan(self, body_end, regexp_flags, named_capture_groups);
                self.end = saved_end;
                self.state.pos = pos;
                self.state.token_start = saved_start;
                self.state.token_flags = saved_flags;
            } else {
                self.state.pos = pos;
            }
        }
        self.state.pos = pos;
        self.state.token_value = TokenValue::Borrowed(self.slice(self.state.token_start, pos));
        self.take_token(SyntaxKind::RegularExpressionLiteral, 0)
    }
    /// port: tsc/internal/scanner/scanner.go:Scanner.ReScanJsxToken
    pub fn rescan_jsx_token(&mut self, multiline: bool) -> SyntaxKind {
        self.state.pos = self.state.full_start_pos;
        self.state.token_start = self.state.full_start_pos;
        self.state.token = self.scan_jsx_token_ex(multiline);
        self.state.token
    }
    /// port: tsc/internal/scanner/scanner.go:Scanner.ReScanHashToken
    pub fn rescan_hash_token(&mut self) -> SyntaxKind {
        if self.state.token == SyntaxKind::PrivateIdentifier {
            self.state.pos = self.state.token_start + 1;
            self.state.token = SyntaxKind::HashToken;
        }
        self.state.token
    }
    /// port: tsc/internal/scanner/scanner.go:Scanner.ReScanQuestionToken
    pub fn rescan_question_token(&mut self) -> SyntaxKind {
        assert!(
            self.state.token == SyntaxKind::QuestionQuestionToken,
            "'reScanQuestionToken' should only be called on a '??'"
        );
        self.state.pos = self.state.token_start + 1;
        self.take_token(SyntaxKind::QuestionToken, 0)
    }
    /// port: tsc/internal/scanner/scanner.go:Scanner.ScanJsxToken
    pub fn scan_jsx_token(&mut self) -> SyntaxKind {
        self.scan_jsx_token_ex(true)
    }
    /// port: tsc/internal/scanner/scanner.go:Scanner.ScanJsxTokenEx
    pub fn scan_jsx_token_ex(&mut self, multiline: bool) -> SyntaxKind {
        self.state.full_start_pos = self.state.pos;
        self.state.token_start = self.state.pos;
        let ch = self.char();
        if ch < 0 {
            return self.take_token(SyntaxKind::EndOfFile, 0);
        }
        if ch == i32::from(b'<') {
            return if self.char_at(1) == i32::from(b'/') {
                self.take_token(SyntaxKind::LessThanSlashToken, 2)
            } else {
                self.take_token(SyntaxKind::LessThanToken, 1)
            };
        }
        if ch == i32::from(b'{') {
            return self.take_token(SyntaxKind::OpenBraceToken, 1);
        }
        let mut first_non_whitespace = 0;
        loop {
            let (ch, size) = self.char_and_size();
            if size == 0 || ch == i32::from(b'{') {
                break;
            }
            if ch == i32::from(b'<') {
                if is_conflict_marker_trivia(self.text, self.state.pos) {
                    self.consume_conflict_marker();
                    return self.take_token(SyntaxKind::ConflictMarkerTrivia, 0);
                }
                break;
            }
            if ch == i32::from(b'>') {
                self.error_at(
                    diagnostics::Unexpected_token_Did_you_mean_or_gt,
                    self.state.pos,
                    1,
                    vec![],
                );
            } else if ch == i32::from(b'}') {
                self.error_at(
                    diagnostics::Unexpected_token_Did_you_mean_or_rbrace,
                    self.state.pos,
                    1,
                    vec![],
                );
            }
            if is_line_break(ch) && first_non_whitespace == 0 {
                first_non_whitespace = -1;
            } else if !multiline && is_line_break(ch) && first_non_whitespace > 0 {
                break;
            } else if !is_white_space_like(ch) {
                first_non_whitespace = self.state.pos;
            }
            self.state.pos += size as i64;
        }
        self.state.token_value =
            TokenValue::Borrowed(self.slice(self.state.full_start_pos, self.state.pos));
        self.take_token(
            if first_non_whitespace == -1 {
                SyntaxKind::JsxTextAllWhiteSpaces
            } else {
                SyntaxKind::JsxText
            },
            0,
        )
    }
    /// port: tsc/internal/scanner/scanner.go:Scanner.ScanJsxIdentifier
    pub fn scan_jsx_identifier(&mut self) -> SyntaxKind {
        if token_is_identifier_or_keyword(self.state.token) {
            let suffix = self.scan_identifier_parts(IdentifierVariant::Jsx);
            self.append_token_value(suffix.as_bytes());
            self.state.token = get_identifier_token(self.token_value());
        }
        self.state.token
    }
    /// port: tsc/internal/scanner/scanner.go:Scanner.ScanJsxAttributeValue
    pub fn scan_jsx_attribute_value(&mut self) -> SyntaxKind {
        self.state.full_start_pos = self.state.pos;
        loop {
            let (ch, size) = self.char_and_size();
            if size == 0 || !is_white_space_like(ch) {
                break;
            }
            self.state.pos += size as i64;
        }
        self.state.token_start = self.state.pos;
        if matches!(u8::try_from(self.char()), Ok(b'"' | b'\'')) {
            self.state.token_value = self.scan_string(true);
            return self.take_token(SyntaxKind::StringLiteral, 0);
        }
        self.scan()
    }
    /// port: tsc/internal/scanner/scanner.go:Scanner.ReScanJsxAttributeValue
    pub fn rescan_jsx_attribute_value(&mut self) -> SyntaxKind {
        self.state.pos = self.state.full_start_pos;
        self.state.token_start = self.state.full_start_pos;
        self.scan_jsx_attribute_value()
    }
    /// port: tsc/internal/scanner/scanner.go:Scanner.ScanJSDocCommentTextToken
    pub fn scan_jsdoc_comment_text_token(&mut self, in_backticks: bool) -> SyntaxKind {
        self.state.full_start_pos = self.state.pos;
        self.state.token_flags = flags::NONE;
        if self.state.pos >= self.text.len() as i64 {
            return self.take_token(SyntaxKind::EndOfFile, 0);
        }
        self.state.token_start = self.state.pos;
        loop {
            let (ch, size) = self.char_and_size();
            if self.state.pos >= self.text.len() as i64
                || is_line_break(ch)
                || ch == i32::from(b'`')
            {
                break;
            }
            if !in_backticks {
                if ch == i32::from(b'{') {
                    break;
                }
                if ch == i32::from(b'@') && self.state.pos >= 0 {
                    let (previous, _) = decode_last_utf8(self.slice(0, self.state.pos));
                    if is_white_space_single_line(previous) {
                        let (next, _) = decode_utf8(self.tail(self.state.pos + size as i64));
                        if is_identifier_start(next) {
                            break;
                        }
                    }
                }
            }
            self.state.pos += size as i64;
        }
        if self.state.pos == self.state.token_start {
            return self.scan_jsdoc_token();
        }
        self.state.token_value =
            TokenValue::Borrowed(self.slice(self.state.token_start, self.state.pos));
        self.take_token(SyntaxKind::JSDocCommentTextToken, 0)
    }
    /// port: tsc/internal/scanner/scanner.go:Scanner.CanFollowJSDocAt
    pub fn can_follow_jsdoc_at(&self) -> bool {
        if self.state.pos >= self.text.len() as i64 {
            return true;
        }
        let (ch, _) = decode_utf8(self.tail(self.state.pos));
        is_identifier_start(ch) || is_white_space_single_line(ch) || is_line_break(ch)
    }
    /// port: tsc/internal/scanner/scanner.go:Scanner.ScanJSDocToken
    pub fn scan_jsdoc_token(&mut self) -> SyntaxKind {
        use SyntaxKind as K;
        self.state.full_start_pos = self.state.pos;
        self.state.token_flags = flags::NONE;
        if self.state.pos >= self.text.len() as i64 {
            return self.take_token(K::EndOfFile, 0);
        }
        self.state.token_start = self.state.pos;
        let (ch, size) = self.char_and_size();
        self.state.pos += size as i64;
        match u8::try_from(ch).ok() {
            Some(b'\t' | b'\x0b' | b'\x0c' | b' ') => {
                loop {
                    let (ch, size) = self.char_and_size();
                    if size == 0 || !is_white_space_single_line(ch) {
                        break;
                    }
                    self.state.pos += size as i64;
                }
                return self.take_token(K::WhitespaceTrivia, 0);
            }
            Some(b'\r' | b'\n') => {
                if ch == i32::from(b'\r') && self.char() == i32::from(b'\n') {
                    self.state.pos += 1;
                }
                self.state.token_flags |= flags::PRECEDING_LINE_BREAK;
                return self.take_token(K::NewLineTrivia, 0);
            }
            _ => {}
        }
        let token = match u8::try_from(ch).ok() {
            Some(b'@') => Some(K::AtToken),
            Some(b'*') => Some(K::AsteriskToken),
            Some(b'{') => Some(K::OpenBraceToken),
            Some(b'}') => Some(K::CloseBraceToken),
            Some(b'[') => Some(K::OpenBracketToken),
            Some(b']') => Some(K::CloseBracketToken),
            Some(b'(') => Some(K::OpenParenToken),
            Some(b')') => Some(K::CloseParenToken),
            Some(b'<') => Some(K::LessThanToken),
            Some(b'>') => Some(K::GreaterThanToken),
            Some(b'=') => Some(K::EqualsToken),
            Some(b',') => Some(K::CommaToken),
            Some(b'.') => Some(K::DotToken),
            Some(b'`') => Some(K::BacktickToken),
            Some(b'#') => Some(K::HashToken),
            _ => None,
        };
        if let Some(token) = token {
            return self.take_token(token, 0);
        }
        self.state.pos = self.state.token_start;
        if self.scan_identifier(0, IdentifierVariant::Jsx) {
            return self.take_token(get_identifier_token(self.token_value()), 0);
        }
        self.state.pos = self.state.token_start + size as i64;
        self.take_token(K::Unknown, 0)
    }
}
