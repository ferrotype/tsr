use crate::identifier::get_identifier_token;
use crate::trivia::{is_conflict_marker_trivia, scan_conflict_marker_trivia};
use crate::utilities::{is_digit, is_line_break, is_white_space_single_line};
use crate::{IdentifierVariant, Scanner, TokenValue};
use std::sync::Arc;
use ts_ast::{token_flags as flags, CommentDirective, CommentDirectiveKind, SyntaxKind};
use ts_core::{LanguageVariant, TextRange};
use ts_diagnostics as diagnostics;
use ts_jsstring::wtf8::RUNE_ERROR;

impl Scanner<'_> {
    pub(crate) fn take_token(&mut self, token: SyntaxKind, width: i64) -> SyntaxKind {
        self.state.pos += width;
        self.state.token = token;
        token
    }

    pub(crate) fn consume_conflict_marker(&mut self) {
        self.error_at(
            diagnostics::Merge_conflict_marker_encountered,
            self.state.pos,
            7,
            vec![],
        );
        self.state.pos = scan_conflict_marker_trivia(self.text, self.state.pos);
    }

    /// port: tsc/internal/scanner/scanner.go:Scanner.Scan
    pub fn scan(&mut self) -> SyntaxKind {
        use SyntaxKind as K;
        self.state.full_start_pos = self.state.pos;
        self.state.token_flags = flags::NONE;
        loop {
            let ch = self.char();
            self.state.token_start = self.state.pos;
            if ch < 0 {
                return self.take_token(K::EndOfFile, 0);
            }
            match ch as u8 {
                b'\t' | b'\x0b' | b'\x0c' | b' ' => {
                    self.state.pos += 1;
                    if self.skip_trivia {
                        continue;
                    }
                    loop {
                        let (rune, size) = self.char_and_size();
                        if !is_white_space_single_line(rune) {
                            break;
                        }
                        self.state.pos += size as i64;
                    }
                    return self.take_token(K::WhitespaceTrivia, 0);
                }
                b'\n' | b'\r' => {
                    self.state.token_flags |= flags::PRECEDING_LINE_BREAK;
                    if self.skip_trivia {
                        self.state.pos += 1;
                        self.scan_ascii_while(|b| b == b' ' || (b'\t'..=b'\r').contains(&b));
                        continue;
                    }
                    let size = if ch == i32::from(b'\r') && self.char_at(1) == i32::from(b'\n') {
                        2
                    } else {
                        1
                    };
                    return self.take_token(K::NewLineTrivia, size);
                }
                b'\'' | b'"' => {
                    self.state.token_value = self.scan_string(false);
                    return self.take_token(K::StringLiteral, 0);
                }
                b'`' => {
                    let token = self.scan_template_and_set_token_value(false);
                    return self.take_token(token, 0);
                }
                b'/' => {
                    if self.char_at(1) == i32::from(b'/') {
                        self.state.pos += 2;
                        loop {
                            self.scan_ascii_while(|b| b != b'\n' && b != b'\r');
                            let (rune, size) = self.char_and_size();
                            if size == 0 || is_line_break(rune) {
                                break;
                            }
                            self.state.pos += size as i64;
                        }
                        self.process_comment_directive(
                            self.state.token_start,
                            self.state.pos,
                            false,
                        );
                        if self.skip_trivia {
                            continue;
                        }
                        return self.take_token(K::SingleLineCommentTrivia, 0);
                    }
                    if self.char_at(1) == i32::from(b'*') {
                        self.state.pos += 2;
                        let jsdoc =
                            self.char() == i32::from(b'*') && self.char_at(1) != i32::from(b'/');
                        let mut closed = false;
                        let mut last_line_start = self.state.token_start;
                        loop {
                            self.scan_ascii_while(|b| !matches!(b, b'*' | b'\n' | b'\r'));
                            let (rune, size) = self.char_and_size();
                            if size == 0 {
                                break;
                            }
                            if rune == i32::from(b'*') && self.char_at(1) == i32::from(b'/') {
                                self.state.pos += 2;
                                closed = true;
                                break;
                            }
                            self.state.pos += size as i64;
                            if is_line_break(rune) {
                                last_line_start = self.state.pos;
                                self.state.token_flags |= flags::PRECEDING_LINE_BREAK;
                            }
                        }
                        if jsdoc {
                            self.state.token_flags |= flags::PRECEDING_JSDOC_COMMENT;
                            self.scan_jsdoc_comment_for_tags(
                                self.slice(self.state.token_start, self.state.pos),
                            );
                        }
                        self.process_comment_directive(last_line_start, self.state.pos, true);
                        if !closed {
                            self.error(diagnostics::Asterisk_Slash_expected);
                        }
                        if self.skip_trivia {
                            continue;
                        }
                        if !closed {
                            self.state.token_flags |= flags::UNTERMINATED;
                        }
                        return self.take_token(K::MultiLineCommentTrivia, 0);
                    }
                    return if self.char_at(1) == i32::from(b'=') {
                        self.take_token(K::SlashEqualsToken, 2)
                    } else {
                        self.take_token(K::SlashToken, 1)
                    };
                }
                b'0' => {
                    if matches!(u8::try_from(self.char_at(1)), Ok(b'X' | b'x')) {
                        let start = self.state.pos;
                        self.state.pos += 2;
                        let mut digits = self.scan_hex_digits(1, true, true);
                        if digits.as_bytes().is_empty() {
                            self.error(diagnostics::Hexadecimal_digit_expected);
                            digits = TokenValue::Static(b"0");
                        }
                        if let Some(value) = self.hex_number_cache.get(digits.as_bytes()) {
                            self.state.token_value = value.clone();
                        } else {
                            let raw = self.slice(start, self.state.pos);
                            self.state.token_value =
                                if raw.starts_with(b"0x") && &raw[2..] == digits.as_bytes() {
                                    TokenValue::Borrowed(raw)
                                } else {
                                    let mut value = b"0x".to_vec();
                                    value.extend_from_slice(digits.as_bytes());
                                    value.into()
                                };
                            self.hex_number_cache
                                .insert(digits, self.state.token_value.clone());
                        }
                        self.state.token_flags |= flags::HEX_SPECIFIER;
                        let token = self.scan_big_int_suffix();
                        return self.take_token(token, 0);
                    }
                    if matches!(u8::try_from(self.char_at(1)), Ok(b'B' | b'b' | b'O' | b'o')) {
                        let binary = matches!(u8::try_from(self.char_at(1)), Ok(b'B' | b'b'));
                        self.state.pos += 2;
                        let mut digits =
                            self.scan_binary_or_octal_digits(if binary { 2 } else { 8 });
                        if digits.is_empty() {
                            self.error(if binary {
                                diagnostics::Binary_digit_expected
                            } else {
                                diagnostics::Octal_digit_expected
                            });
                            digits.push(b'0');
                        }
                        let mut value = if binary {
                            b"0b".to_vec()
                        } else {
                            b"0o".to_vec()
                        };
                        value.extend_from_slice(&digits);
                        self.state.token_value = value.into();
                        self.state.token_flags |= if binary {
                            flags::BINARY_SPECIFIER
                        } else {
                            flags::OCTAL_SPECIFIER
                        };
                        let token = self.scan_big_int_suffix();
                        return self.take_token(token, 0);
                    }
                    let token = self.scan_number();
                    return self.take_token(token, 0);
                }
                b'1'..=b'9' => {
                    let token = self.scan_number();
                    return self.take_token(token, 0);
                }
                b'.' => {
                    if is_digit(self.char_at(1)) {
                        let token = self.scan_number();
                        return self.take_token(token, 0);
                    }
                    return if self.char_at(1) == i32::from(b'.')
                        && self.char_at(2) == i32::from(b'.')
                    {
                        self.take_token(K::DotDotDotToken, 3)
                    } else {
                        self.take_token(K::DotToken, 1)
                    };
                }
                b'#' => {
                    if self.char_at(1) == i32::from(b'!') {
                        if self.state.pos == 0 {
                            self.state.pos += 2;
                            loop {
                                let (rune, size) = self.char_and_size();
                                if size == 0 || is_line_break(rune) {
                                    break;
                                }
                                self.state.pos += size as i64;
                            }
                            continue;
                        }
                        self.error_at(
                            diagnostics::X_can_only_be_used_at_the_start_of_a_file,
                            self.state.pos,
                            2,
                            vec![],
                        );
                        return self.take_token(K::Unknown, 2);
                    }
                    if !self.scan_identifier(1, IdentifierVariant::Standard) {
                        self.error_at(
                            diagnostics::Invalid_character,
                            self.state.pos - 1,
                            1,
                            vec![],
                        );
                        self.state.token_value = TokenValue::Static(b"#");
                    }
                    return self.take_token(K::PrivateIdentifier, 0);
                }
                b'<' | b'=' | b'>' | b'|' => {
                    if self.char_at(1) == ch && is_conflict_marker_trivia(self.text, self.state.pos)
                    {
                        self.consume_conflict_marker();
                        if self.skip_trivia {
                            continue;
                        }
                        return self.take_token(K::ConflictMarkerTrivia, 0);
                    }
                    let (kind, size) = self.punctuation(ch as u8);
                    return self.take_token(kind, size);
                }
                b'*' => {
                    let (kind, size) = self.punctuation(b'*');
                    self.state.pos += size;
                    if kind == K::AsteriskToken
                        && self.state.skip_jsdoc_leading_asterisks != 0
                        && self.state.token_flags & flags::PRECEDING_JSDOC_LEADING_ASTERISKS == 0
                        && self.state.token_flags & flags::PRECEDING_LINE_BREAK != 0
                    {
                        self.state.token_flags |= flags::PRECEDING_JSDOC_LEADING_ASTERISKS;
                        continue;
                    }
                    return self.take_token(kind, 0);
                }
                b'!' | b'%' | b'&' | b'(' | b')' | b'+' | b',' | b'-' | b':' | b';' | b'?'
                | b'[' | b']' | b'^' | b'{' | b'}' | b'~' | b'@' => {
                    let (kind, size) = self.punctuation(ch as u8);
                    return self.take_token(kind, size);
                }
                b'\\' => {
                    if self.scan_identifier(0, IdentifierVariant::Standard) {
                        return self.take_token(get_identifier_token(self.token_value()), 0);
                    }
                    self.scan_invalid_character();
                    return self.state.token;
                }
                _ => {
                    if self.scan_identifier(0, IdentifierVariant::Standard) {
                        return self.take_token(get_identifier_token(self.token_value()), 0);
                    }
                    let (rune, size) = self.char_and_size();
                    if rune == RUNE_ERROR {
                        self.error_at(diagnostics::File_appears_to_be_binary, 0, 0, vec![]);
                        self.state.pos = self.text.len() as i64;
                        return self.take_token(K::NonTextFileMarkerTrivia, 0);
                    }
                    if is_white_space_single_line(rune) {
                        self.state.pos += size as i64;
                        if rune == 0x85 || self.skip_trivia {
                            continue;
                        }
                        loop {
                            let (rune, size) = self.char_and_size();
                            if !is_white_space_single_line(rune) {
                                break;
                            }
                            self.state.pos += size as i64;
                        }
                        return self.take_token(K::WhitespaceTrivia, 0);
                    }
                    if is_line_break(rune) {
                        self.state.token_flags |= flags::PRECEDING_LINE_BREAK;
                        self.state.pos += size as i64;
                        continue;
                    }
                    self.scan_invalid_character();
                    return self.state.token;
                }
            }
        }
    }

    fn punctuation(&self, byte: u8) -> (SyntaxKind, i64) {
        use SyntaxKind as K;
        let next = self.char_at(1);
        let third = self.char_at(2);
        let equals = next == i32::from(b'=');
        match byte {
            b'!' => {
                if equals {
                    if third == i32::from(b'=') {
                        (K::ExclamationEqualsEqualsToken, 3)
                    } else {
                        (K::ExclamationEqualsToken, 2)
                    }
                } else {
                    (K::ExclamationToken, 1)
                }
            }
            b'%' => {
                if equals {
                    (K::PercentEqualsToken, 2)
                } else {
                    (K::PercentToken, 1)
                }
            }
            b'^' => {
                if equals {
                    (K::CaretEqualsToken, 2)
                } else {
                    (K::CaretToken, 1)
                }
            }
            b'+' => {
                if equals {
                    (K::PlusEqualsToken, 2)
                } else if next == i32::from(b'+') {
                    (K::PlusPlusToken, 2)
                } else {
                    (K::PlusToken, 1)
                }
            }
            b'-' => {
                if equals {
                    (K::MinusEqualsToken, 2)
                } else if next == i32::from(b'-') {
                    (K::MinusMinusToken, 2)
                } else {
                    (K::MinusToken, 1)
                }
            }
            b'*' => {
                if equals {
                    (K::AsteriskEqualsToken, 2)
                } else if next == i32::from(b'*') {
                    if third == i32::from(b'=') {
                        (K::AsteriskAsteriskEqualsToken, 3)
                    } else {
                        (K::AsteriskAsteriskToken, 2)
                    }
                } else {
                    (K::AsteriskToken, 1)
                }
            }
            b'&' => {
                if next == i32::from(b'&') {
                    if third == i32::from(b'=') {
                        (K::AmpersandAmpersandEqualsToken, 3)
                    } else {
                        (K::AmpersandAmpersandToken, 2)
                    }
                } else if equals {
                    (K::AmpersandEqualsToken, 2)
                } else {
                    (K::AmpersandToken, 1)
                }
            }
            b'|' => {
                if next == i32::from(b'|') {
                    if third == i32::from(b'=') {
                        (K::BarBarEqualsToken, 3)
                    } else {
                        (K::BarBarToken, 2)
                    }
                } else if equals {
                    (K::BarEqualsToken, 2)
                } else {
                    (K::BarToken, 1)
                }
            }
            b'?' => {
                if next == i32::from(b'.') && !is_digit(third) {
                    (K::QuestionDotToken, 2)
                } else if next == i32::from(b'?') {
                    if third == i32::from(b'=') {
                        (K::QuestionQuestionEqualsToken, 3)
                    } else {
                        (K::QuestionQuestionToken, 2)
                    }
                } else {
                    (K::QuestionToken, 1)
                }
            }
            b'<' => {
                if next == i32::from(b'<') {
                    if third == i32::from(b'=') {
                        (K::LessThanLessThanEqualsToken, 3)
                    } else {
                        (K::LessThanLessThanToken, 2)
                    }
                } else if equals {
                    (K::LessThanEqualsToken, 2)
                } else if self.language_variant == LanguageVariant::JSX
                    && next == i32::from(b'/')
                    && third != i32::from(b'*')
                {
                    (K::LessThanSlashToken, 2)
                } else {
                    (K::LessThanToken, 1)
                }
            }
            b'=' => {
                if equals {
                    if third == i32::from(b'=') {
                        (K::EqualsEqualsEqualsToken, 3)
                    } else {
                        (K::EqualsEqualsToken, 2)
                    }
                } else if next == i32::from(b'>') {
                    (K::EqualsGreaterThanToken, 2)
                } else {
                    (K::EqualsToken, 1)
                }
            }
            b'>' => (K::GreaterThanToken, 1),
            b'(' => (K::OpenParenToken, 1),
            b')' => (K::CloseParenToken, 1),
            b',' => (K::CommaToken, 1),
            b':' => (K::ColonToken, 1),
            b';' => (K::SemicolonToken, 1),
            b'[' => (K::OpenBracketToken, 1),
            b']' => (K::CloseBracketToken, 1),
            b'{' => (K::OpenBraceToken, 1),
            b'}' => (K::CloseBraceToken, 1),
            b'~' => (K::TildeToken, 1),
            b'@' => (K::AtToken, 1),
            _ => unreachable!("punctuation dispatch requires an operator byte"),
        }
    }

    /// port: tsc/internal/scanner/scanner.go:Scanner.scanInvalidCharacter
    fn scan_invalid_character(&mut self) {
        let (_, size) = self.char_and_size();
        self.error_at(
            diagnostics::Invalid_character,
            self.state.pos,
            size as i64,
            vec![],
        );
        self.take_token(SyntaxKind::Unknown, size as i64);
    }

    /// port: tsc/internal/scanner/scanner.go:Scanner.processCommentDirective
    fn process_comment_directive(&mut self, start: i64, end: i64, multiline: bool) {
        let mut pos = start;
        if multiline {
            while pos < end && matches!(self.text[pos as usize], b' ' | b'\t') {
                pos += 1;
            }
            while pos < end && matches!(self.text[pos as usize], b'/' | b'*') {
                pos += 1;
            }
        } else {
            pos += 2;
            while pos < end && self.text[pos as usize] == b'/' {
                pos += 1;
            }
        }
        while pos < end && matches!(self.text[pos as usize], b' ' | b'\t') {
            pos += 1;
        }
        if pos >= end || self.text[pos as usize] != b'@' {
            return;
        }
        pos += 1;
        let kind = if self.tail(pos).starts_with(b"ts-expect-error") {
            CommentDirectiveKind::ExpectError
        } else if self.tail(pos).starts_with(b"ts-ignore") {
            CommentDirectiveKind::Ignore
        } else {
            return;
        };
        let directives = self
            .state
            .comment_directives
            .get_or_insert_with(|| Arc::new(Vec::new()));
        Arc::make_mut(directives).push(CommentDirective {
            loc: TextRange::new(start, end),
            kind,
        });
    }

    /// port: tsc/internal/scanner/scanner.go:Scanner.scanJSDocCommentForTags
    fn scan_jsdoc_comment_for_tags(&mut self, mut text: &[u8]) {
        while let Some(index) = text.iter().position(|&b| b == b'@') {
            text = &text[index + 1..];
            if self.state.token_flags & flags::PRECEDING_JSDOC_WITH_DEPRECATED == 0
                && has_jsdoc_tag(text, &[b"deprecated"])
            {
                self.state.token_flags |= flags::PRECEDING_JSDOC_WITH_DEPRECATED;
            }
            if self.state.token_flags & flags::PRECEDING_JSDOC_WITH_SEE_OR_LINK == 0
                && has_jsdoc_tag(text, &[b"see", b"link", b"linkcode", b"linkplain"])
            {
                self.state.token_flags |= flags::PRECEDING_JSDOC_WITH_SEE_OR_LINK;
            }
            let both =
                flags::PRECEDING_JSDOC_WITH_DEPRECATED | flags::PRECEDING_JSDOC_WITH_SEE_OR_LINK;
            if self.state.token_flags & both == both {
                return;
            }
        }
    }
}

/// port: tsc/internal/scanner/scanner.go:hasJSDocTag
fn has_jsdoc_tag(text: &[u8], tags: &[&[u8]]) -> bool {
    tags.iter().any(|tag| {
        text.strip_prefix(*tag).is_some_and(|rest| {
            rest.is_empty() || matches!(rest[0], b' ' | b'\t' | b'\n' | b'\r' | b'}' | b'*')
        })
    })
}
