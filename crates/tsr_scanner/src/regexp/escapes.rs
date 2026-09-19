use std::borrow::Cow;

use ts_core::ScriptTarget;
use ts_diagnostics as diagnostics;
use ts_jsstring::wtf8::{code_point_to_surrogate_pair, decode_utf8, encode_rune, RUNE_ERROR};

use crate::{escape_flags, IdentifierVariant};

use super::{Atom, DecimalEscapeValue, GroupNameReference, RegExpParser};

impl<'src> RegExpParser<'_, 'src> {
    /// port: tsc/internal/scanner/regexp.go:regExpParser.scanAtomEscape
    pub(super) fn scan_atom_escape(&mut self) {
        self.assert_after(b'\\');
        match self.char() as u8 {
            b'k' => {
                self.inc_pos(1);
                if self.char() == i32::from(b'<') {
                    self.inc_pos(1);
                    self.scan_group_name(true);
                    self.scan_expected_char(b'>');
                } else if self.any_unicode_mode_or_non_annex_b || self.named_capture_groups {
                    self.error(diagnostics::X_k_must_be_followed_by_a_capturing_group_name_enclosed_in_angle_brackets,
                        self.pos() - 2, 2, vec![]);
                }
            }
            b'q' if self.unicode_sets_mode => {
                self.inc_pos(1);
                self.error(
                    diagnostics::X_q_is_only_available_inside_character_class,
                    self.pos() - 2,
                    2,
                    vec![],
                );
            }
            _ => {
                if !self.scan_character_class_escape() && !self.scan_decimal_escape() {
                    // Go debug.Assert always evaluates this scan, including in release.
                    let escaped = self.scan_character_escape(true);
                    assert!(!escaped.is_empty(), "Debug failure. False expression.");
                }
            }
        }
    }

    /// port: tsc/internal/scanner/regexp.go:regExpParser.scanDecimalEscape
    fn scan_decimal_escape(&mut self) -> bool {
        self.assert_after(b'\\');
        if (i32::from(b'1')..=i32::from(b'9')).contains(&self.char()) {
            let start = self.pos();
            self.scan_digits();
            let value = self
                .scanner
                .state
                .token_value
                .as_bytes()
                .iter()
                .try_fold(0_i64, |value, digit| {
                    value.checked_mul(10)?.checked_add(i64::from(digit - b'0'))
                })
                .unwrap_or(i64::MAX);
            self.decimal_escapes.push(DecimalEscapeValue {
                pos: start,
                end: self.pos(),
                value,
            });
            true
        } else {
            false
        }
    }

    /// port: tsc/internal/scanner/regexp.go:regExpParser.scanCharacterEscape
    pub(super) fn scan_character_escape(&mut self, atom_escape: bool) -> Atom<'src> {
        self.assert_after(b'\\');
        let mut ch = self.char();
        if ch == -1 {
            self.error(
                diagnostics::Undetermined_character_escape,
                self.pos() - 1,
                1,
                vec![],
            );
            return Cow::Borrowed(b"\\");
        }
        match ch as u8 {
            b'c' => {
                self.inc_pos(1);
                ch = self.char();
                if crate::utilities::is_ascii_letter(ch) {
                    self.inc_pos(1);
                    return Cow::Owned(vec![(ch & 0x1f) as u8]);
                }
                if self.any_unicode_mode_or_non_annex_b {
                    self.error(
                        diagnostics::X_c_must_be_followed_by_an_ASCII_letter,
                        self.pos() - 2,
                        2,
                        vec![],
                    );
                } else if atom_escape {
                    self.inc_pos(-1);
                    return Cow::Borrowed(b"\\");
                }
                Cow::Owned(encode_rune(ch))
            }
            b'^' | b'$' | b'/' | b'\\' | b'.' | b'*' | b'+' | b'?' | b'(' | b')' | b'[' | b']'
            | b'{' | b'}' | b'|' => {
                let start = self.pos();
                self.inc_pos(1);
                Cow::Borrowed(self.slice(start, self.pos()))
            }
            _ => {
                self.inc_pos(-1);
                let mut flags = escape_flags::REGULAR_EXPRESSION;
                if self.annex_b {
                    flags |= escape_flags::ANNEX_B;
                }
                if self.any_unicode_mode {
                    flags |= escape_flags::ANY_UNICODE_MODE;
                }
                if atom_escape {
                    flags |= escape_flags::ATOM_ESCAPE;
                }
                self.scanner.scan_escape_sequence(flags)
            }
        }
    }

    /// port: tsc/internal/scanner/regexp.go:regExpParser.scanGroupName
    pub(super) fn scan_group_name(&mut self, is_reference: bool) {
        self.assert_after(b'<');
        self.scanner.state.token_start = self.pos();
        if !self
            .scanner
            .scan_identifier(0, IdentifierVariant::RegExpGroupName)
        {
            self.error(
                diagnostics::Expected_a_capturing_group_name,
                self.pos(),
                0,
                vec![],
            );
            return;
        }
        let name = self.scanner.state.token_value.clone();
        let start = self.scanner.state.token_start;
        if is_reference {
            self.group_name_references.push(GroupNameReference {
                pos: start,
                end: self.pos(),
                name,
            });
        } else if self.named_capturing_groups_contains(name.as_bytes()) {
            self.error(diagnostics::Named_capturing_groups_with_the_same_name_must_be_mutually_exclusive_to_each_other,
                start, self.pos() - start, vec![]);
        } else {
            let target = self.scanner.language_version();
            if self.group_specifiers.contains(name.as_bytes())
                && target >= ScriptTarget::ES2018
                && target < ScriptTarget::ES2025
            {
                self.error(diagnostics::Duplicate_named_capturing_groups_are_only_available_when_targeting_0_or_later,
                    start, self.pos() - start, vec!["es2025".into()]);
            }
            if let Some(scope) = self.named_capturing_groups.last_mut() {
                scope.insert(name.clone());
            }
            self.group_specifiers.insert(name);
        }
    }

    /// port: tsc/internal/scanner/regexp.go:regExpParser.scanSourceCharacter
    pub(super) fn scan_source_character(&mut self) -> Atom<'src> {
        if self.pos() >= self.end {
            return Cow::Borrowed(b"");
        }
        let start = self.pos();
        if !self.any_unicode_mode {
            if self.pending_low_surrogate != 0 {
                let (_, width) = decode_utf8(self.scanner.tail(start));
                self.inc_pos(width as i64);
                let low = std::mem::take(&mut self.pending_low_surrogate);
                return Cow::Owned(encode_rune(low));
            }
            let (ch, width) = decode_utf8(self.scanner.tail(start));
            if ch == RUNE_ERROR || width == 0 {
                self.inc_pos(1);
                // Go string(byte) encodes U+00xx here; it does not copy that byte.
                return Cow::Owned(encode_rune(i32::from(self.slice(start, self.pos())[0])));
            }
            if ch > 0xffff {
                let (high, low) = code_point_to_surrogate_pair(ch);
                self.pending_low_surrogate = low;
                return Cow::Owned(encode_rune(high));
            }
            self.inc_pos(width as i64);
            return Cow::Borrowed(self.slice(start, self.pos()));
        }
        let (ch, width) = decode_utf8(self.scanner.tail(start));
        if width == 0 {
            return Cow::Borrowed(b"");
        }
        self.inc_pos(width as i64);
        if ch == RUNE_ERROR {
            Cow::Borrowed(b"")
        } else {
            Cow::Borrowed(self.slice(start, self.pos()))
        }
    }
}
