use ts_ast::SyntaxKind;
use ts_core::LanguageVariant;
use ts_jsstring::wtf8::{encode_rune, is_high_surrogate};

use crate::tables_generated::{keyword_kind, IDENTIFIER_PART, IDENTIFIER_START, KEYWORDS, TOKENS};
use crate::utilities::{is_ascii_letter, is_digit};
use crate::{IdentifierVariant, Scanner, TokenValue};

fn in_ranges(ch: i32, ranges: &[(u32, u32, u32)]) -> bool {
    let Ok(ch) = u32::try_from(ch) else {
        return false;
    };
    let index = ranges.partition_point(|&(_, end, _)| end < ch);
    ranges
        .get(index)
        .is_some_and(|&(start, end, stride)| ch >= start && ch <= end && (ch - start) % stride == 0)
}

pub(crate) fn keyword(bytes: &[u8]) -> SyntaxKind {
    keyword_kind(bytes).map_or(SyntaxKind::Unknown, |kind| {
        SyntaxKind::from_u16(kind).expect("pinned keyword kind")
    })
}

/// port: tsc/internal/scanner/scanner.go:GetIdentifierToken
pub fn get_identifier_token(text: &[u8]) -> SyntaxKind {
    if (2..=12).contains(&text.len()) && text[0].is_ascii_lowercase() {
        let kind = keyword(text);
        if kind != SyntaxKind::Unknown {
            return kind;
        }
    }
    SyntaxKind::Identifier
}

/// port: tsc/internal/scanner/scanner.go:IsValidIdentifier
pub fn is_valid_identifier(text: &[u8]) -> bool {
    if text.is_empty() {
        return false;
    }
    let mut pos = 0;
    while pos < text.len() {
        let (ch, size) = ts_jsstring::wtf8::decode_utf8(&text[pos..]);
        if if pos == 0 {
            !is_identifier_start(ch)
        } else {
            !is_identifier_part(ch)
        } {
            return false;
        }
        pos += size;
    }
    true
}
/// port: tsc/internal/scanner/scanner.go:isWordCharacter
pub(crate) fn is_word_character(ch: i32) -> bool {
    is_ascii_letter(ch) || is_digit(ch) || ch == i32::from(b'_')
}
/// port: tsc/internal/scanner/scanner.go:IsIdentifierStart
pub fn is_identifier_start(ch: i32) -> bool {
    is_ascii_letter(ch)
        || ch == i32::from(b'_')
        || ch == i32::from(b'$')
        || ch > 127 && in_ranges(ch, IDENTIFIER_START)
}
/// port: tsc/internal/scanner/scanner.go:IsIdentifierPart
pub fn is_identifier_part(ch: i32) -> bool {
    is_identifier_part_ex(ch, LanguageVariant::STANDARD)
}
/// port: tsc/internal/scanner/scanner.go:IsIdentifierPartEx
pub fn is_identifier_part_ex(ch: i32, variant: LanguageVariant) -> bool {
    is_word_character(ch)
        || ch == i32::from(b'$')
        || ch > 127 && in_ranges(ch, IDENTIFIER_PART)
        || variant == LanguageVariant::JSX && ch == i32::from(b'-')
}
/// port: tsc/internal/scanner/scanner.go:TokenToString
pub fn token_to_string(token: SyntaxKind) -> &'static str {
    TOKENS
        .iter()
        .find_map(|&(text, kind)| (kind == token as u16).then_some(text))
        .unwrap_or("")
}
/// port: tsc/internal/scanner/scanner.go:StringToToken
pub fn string_to_token(text: &[u8]) -> SyntaxKind {
    TOKENS
        .binary_search_by(|&(candidate, _)| candidate.as_bytes().cmp(text))
        .ok()
        .map_or(SyntaxKind::Unknown, |index| {
            SyntaxKind::from_u16(TOKENS[index].1).expect("pinned token kind")
        })
}
/// Go's result is unordered; this returns the same set in lexical order.
/// port: tsc/internal/scanner/scanner.go:GetViableKeywordSuggestions
pub fn get_viable_keyword_suggestions() -> Vec<&'static str> {
    KEYWORDS
        .iter()
        .filter_map(|&(text, _)| (text.len() > 2).then_some(text))
        .collect()
}

impl<'src> Scanner<'src> {
    pub(crate) fn append_token_value(&mut self, suffix: &[u8]) {
        if !suffix.is_empty() {
            let mut value = Vec::with_capacity(self.token_value().len() + suffix.len());
            value.extend_from_slice(self.token_value());
            value.extend_from_slice(suffix);
            self.state.token_value = value.into();
        }
    }

    /// port: tsc/internal/scanner/scanner.go:Scanner.scanIdentifier
    pub(crate) fn scan_identifier(
        &mut self,
        prefix_length: i64,
        variant: IdentifierVariant,
    ) -> bool {
        let start = self.state.pos;
        self.state.pos += prefix_length;
        let identifier_start = self.state.pos;
        let mut ch = self.char();
        if variant != IdentifierVariant::Jsx
            && (is_ascii_letter(ch) || ch == i32::from(b'_') || ch == i32::from(b'$'))
        {
            self.state.pos += 1;
            self.scan_ascii_while(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'$');
            ch = self.char();
            if ch < 128 && ch != i32::from(b'\\') {
                self.state.token_value = TokenValue::Borrowed(self.slice(start, self.state.pos));
                return true;
            }
            self.state.pos = identifier_start;
        }
        let (mut ch, mut size) = self.char_and_size();
        if is_identifier_start(ch) {
            let language = if variant == IdentifierVariant::Jsx {
                LanguageVariant::JSX
            } else {
                LanguageVariant::STANDARD
            };
            loop {
                self.state.pos += size as i64;
                (ch, size) = self.char_and_size();
                if !is_identifier_part_ex(ch, language) {
                    break;
                }
            }
            self.state.token_value = TokenValue::Borrowed(self.slice(start, self.state.pos));
            if ch == i32::from(b'\\') {
                let suffix = self.scan_identifier_parts(variant);
                self.append_token_value(suffix.as_bytes());
            }
            return true;
        }
        if ch == i32::from(b'\\') {
            if let Some(escaped) = self.scan_identifier_escape(
                is_identifier_start,
                variant == IdentifierVariant::RegExpGroupName,
            ) {
                let mut value = self.slice(start, identifier_start).to_vec();
                value.extend_from_slice(&encode_rune(escaped));
                value.extend_from_slice(self.scan_identifier_parts(variant).as_bytes());
                self.state.token_value = value.into();
                return true;
            }
        }
        false
    }

    /// port: tsc/internal/scanner/scanner.go:Scanner.scanIdentifierParts
    pub(crate) fn scan_identifier_parts(&mut self, variant: IdentifierVariant) -> TokenValue<'src> {
        let mut result: Option<Vec<u8>> = None;
        let mut start = self.state.pos;
        let language = if variant == IdentifierVariant::Jsx {
            LanguageVariant::JSX
        } else {
            LanguageVariant::STANDARD
        };
        loop {
            let (ch, size) = self.char_and_size();
            if is_identifier_part_ex(ch, language) {
                self.state.pos += size as i64;
                continue;
            }
            if ch == i32::from(b'\\') {
                let escape_start = self.state.pos;
                if let Some(escaped) = self.scan_identifier_escape(
                    |rune| is_identifier_part_ex(rune, language),
                    variant == IdentifierVariant::RegExpGroupName,
                ) {
                    let result = result.get_or_insert_with(Vec::new);
                    result.extend_from_slice(self.slice(start, escape_start));
                    result.extend_from_slice(&encode_rune(escaped));
                    start = self.state.pos;
                    continue;
                }
            }
            break;
        }
        let tail = self.slice(start, self.state.pos);
        if let Some(mut result) = result {
            result.extend_from_slice(tail);
            result.into()
        } else {
            TokenValue::Borrowed(tail)
        }
    }

    /// port: tsc/internal/scanner/scanner.go:Scanner.scanIdentifierEscape
    fn scan_identifier_escape(
        &mut self,
        valid: impl Fn(i32) -> bool,
        allow_pair: bool,
    ) -> Option<i32> {
        let escaped = self.peek_unicode_escape();
        if escaped >= 0 && valid(escaped) {
            return Some(self.scan_unicode_escape(true));
        }
        if allow_pair && self.char_at(2) != i32::from(b'{') && is_high_surrogate(escaped) {
            let saved_pos = self.state.pos;
            let saved_flags = self.state.token_flags;
            self.scan_unicode_escape(false);
            if self.char_at(2) != i32::from(b'{') {
                if let Some(combined) = self.scan_low_surrogate_escape(escaped) {
                    if valid(combined) {
                        return Some(combined);
                    }
                }
            }
            self.state.pos = saved_pos;
            self.state.token_flags = saved_flags;
        }
        None
    }
}

#[cfg(test)]
#[path = "identifier_tests.rs"]
mod tests;
