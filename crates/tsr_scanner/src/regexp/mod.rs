//! Validation of the pinned scanner's regexp grammar after slash discovery.
//!
//! The enclosing scanner finds the delimiter and flags. This module borrows its
//! cursor and token scratch state; `rescan` restores that temporary state after
//! validation. Intermediate atoms borrow unchanged source bytes.

use std::borrow::Cow;
use std::collections::HashSet;

use ts_core::ScriptTarget;
use ts_diagnostics::{self as diagnostics, Message};

use crate::spelling::get_spelling_suggestion_for_strings;
use crate::{DiagnosticArgument, Scanner, TokenValue};

mod classes;
mod escapes;
mod grammar;
mod properties;

#[cfg(test)]
mod tests;

pub(crate) mod reg_exp_flags {
    pub(crate) const NONE: i32 = 0;
    pub(crate) const HAS_INDICES: i32 = 1 << 0;
    pub(crate) const GLOBAL: i32 = 1 << 1;
    pub(crate) const IGNORE_CASE: i32 = 1 << 2;
    pub(crate) const MULTILINE: i32 = 1 << 3;
    pub(crate) const DOT_ALL: i32 = 1 << 4;
    pub(crate) const UNICODE: i32 = 1 << 5;
    pub(crate) const UNICODE_SETS: i32 = 1 << 6;
    pub(crate) const STICKY: i32 = 1 << 7;
    pub(crate) const ANY_UNICODE_MODE: i32 = UNICODE | UNICODE_SETS;
    pub(crate) const MODIFIERS: i32 = IGNORE_CASE | MULTILINE | DOT_ALL;
}

const STACK_RED_ZONE: usize = 64 * 1024;
const STACK_SEGMENT: usize = 1024 * 1024;

type Atom<'src> = Cow<'src, [u8]>;

struct GroupNameReference<'src> {
    pos: i64,
    end: i64,
    name: TokenValue<'src>,
}

struct DecimalEscapeValue {
    pos: i64,
    end: i64,
    value: i64,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum ClassSetExpressionType {
    Intersection,
    Subtraction,
}

struct RegExpParser<'scanner, 'src> {
    scanner: &'scanner mut Scanner<'src>,
    end: i64,
    any_unicode_mode: bool,
    unicode_sets_mode: bool,
    annex_b: bool,
    any_unicode_mode_or_non_annex_b: bool,
    named_capture_groups: bool,
    may_contain_strings: bool,
    number_of_capturing_groups: i64,
    group_specifiers: HashSet<TokenValue<'src>>,
    group_name_references: Vec<GroupNameReference<'src>>,
    decimal_escapes: Vec<DecimalEscapeValue>,
    named_capturing_groups: Vec<HashSet<TokenValue<'src>>>,
    pending_low_surrogate: i32,
}

pub(crate) fn flag_for_character(character: i32) -> Option<i32> {
    use reg_exp_flags as flags;
    match u8::try_from(character).ok() {
        Some(b'd') => Some(flags::HAS_INDICES),
        Some(b'g') => Some(flags::GLOBAL),
        Some(b'i') => Some(flags::IGNORE_CASE),
        Some(b'm') => Some(flags::MULTILINE),
        Some(b's') => Some(flags::DOT_ALL),
        Some(b'u') => Some(flags::UNICODE),
        Some(b'v') => Some(flags::UNICODE_SETS),
        Some(b'y') => Some(flags::STICKY),
        _ => None,
    }
}

/// port: tsc/internal/scanner/regexp.go:Scanner.checkRegularExpressionFlagAvailability
pub(crate) fn check_flag_availability(scanner: &mut Scanner<'_>, flag: i32, pos: i64, size: i64) {
    let (target, name) = match flag {
        reg_exp_flags::HAS_INDICES => (ScriptTarget::ES2022, "es2022"),
        reg_exp_flags::DOT_ALL => (ScriptTarget::ES2018, "es2018"),
        reg_exp_flags::UNICODE_SETS => (ScriptTarget::ES2024, "es2024"),
        _ => return,
    };
    if scanner.language_version() < target {
        scanner.error_at(
            diagnostics::This_regular_expression_flag_is_only_available_when_targeting_0_or_later,
            pos,
            size,
            vec![name.into()],
        );
    }
}

pub(crate) fn scan(
    scanner: &mut Scanner<'_>,
    body_end: i64,
    flags: i32,
    named_capture_groups: bool,
) {
    RegExpParser {
        scanner,
        end: body_end,
        any_unicode_mode: flags & reg_exp_flags::ANY_UNICODE_MODE != 0,
        unicode_sets_mode: flags & reg_exp_flags::UNICODE_SETS != 0,
        annex_b: true,
        any_unicode_mode_or_non_annex_b: false,
        named_capture_groups,
        may_contain_strings: false,
        number_of_capturing_groups: 0,
        group_specifiers: HashSet::new(),
        group_name_references: Vec::new(),
        decimal_escapes: Vec::new(),
        named_capturing_groups: Vec::new(),
        pending_low_surrogate: 0,
    }
    .run();
}

/// port: tsc/internal/scanner/regexp.go:compareDecimalStrings
fn compare_decimal_strings(mut left: &[u8], mut right: &[u8]) -> std::cmp::Ordering {
    while left.first() == Some(&b'0') {
        left = &left[1..];
    }
    while right.first() == Some(&b'0') {
        right = &right[1..];
    }
    if left.is_empty() {
        left = b"0";
    }
    if right.is_empty() {
        right = b"0";
    }
    left.len().cmp(&right.len()).then_with(|| left.cmp(right))
}

impl<'src> RegExpParser<'_, 'src> {
    /// port: tsc/internal/scanner/regexp.go:regExpParser.pos
    fn pos(&self) -> i64 {
        self.scanner.state.pos
    }

    /// port: tsc/internal/scanner/regexp.go:regExpParser.setPos
    fn set_pos(&mut self, pos: i64) {
        self.scanner.state.pos = pos;
    }

    /// port: tsc/internal/scanner/regexp.go:regExpParser.incPos
    fn inc_pos(&mut self, count: i64) {
        self.set_pos(self.pos().wrapping_add(count));
    }

    /// port: tsc/internal/scanner/regexp.go:regExpParser.char
    fn char(&self) -> i32 {
        self.scanner.char()
    }

    /// port: tsc/internal/scanner/regexp.go:regExpParser.charAt
    fn char_at(&self, pos: i64) -> i32 {
        self.scanner.char_at(pos.wrapping_sub(self.pos()))
    }

    /// port: tsc/internal/scanner/regexp.go:regExpParser.error
    fn error(
        &mut self,
        message: &'static Message,
        pos: i64,
        length: i64,
        args: Vec<DiagnosticArgument>,
    ) {
        self.scanner.error_at(message, pos, length, args);
    }

    /// port: tsc/internal/scanner/regexp.go:regExpParser.text
    fn text(&self) -> &'src [u8] {
        self.scanner.text
    }

    fn slice(&self, start: i64, end: i64) -> &'src [u8] {
        let start = usize::try_from(start).expect("source range start must not be negative");
        let end = usize::try_from(end).expect("source range end must not be negative");
        &self.text()[start..end]
    }

    fn assert_after(&self, expected: u8) {
        assert!(
            self.pos() > 0 && self.slice(self.pos() - 1, self.pos())[0] == expected,
            "Debug failure. False expression."
        );
    }

    /// port: tsc/internal/scanner/regexp.go:regExpParser.namedCapturingGroupsContains
    fn named_capturing_groups_contains(&self, name: &[u8]) -> bool {
        self.named_capturing_groups
            .iter()
            .any(|group| group.contains(name))
    }

    /// port: tsc/internal/scanner/regexp.go:regExpParser.isClassContentExit
    fn is_class_content_exit(&self, character: i32) -> bool {
        character == i32::from(b']') || self.pos() >= self.end
    }

    /// port: tsc/internal/scanner/regexp.go:regExpParser.scanExpectedChar
    fn scan_expected_char(&mut self, expected: u8) {
        if self.char() == i32::from(expected) {
            self.inc_pos(1);
        } else {
            self.error(
                diagnostics::X_0_expected,
                self.pos(),
                0,
                vec![vec![expected].into()],
            );
        }
    }

    /// port: tsc/internal/scanner/regexp.go:regExpParser.scanDigits
    fn scan_digits(&mut self) {
        let start = self.pos();
        while self.pos() < self.end && crate::utilities::is_digit(self.char()) {
            self.inc_pos(1);
        }
        self.scanner.state.token_value = TokenValue::Borrowed(self.slice(start, self.pos()));
    }

    /// port: tsc/internal/scanner/regexp.go:regExpParser.run
    fn run(&mut self) {
        self.any_unicode_mode_or_non_annex_b = self.any_unicode_mode || !self.annex_b;
        self.scan_disjunction(false);
        // The source emits these after grammar diagnostics, in reference order.
        for index in 0..self.group_name_references.len() {
            let reference = &self.group_name_references[index];
            if !self.group_specifiers.contains(reference.name.as_bytes()) {
                let (pos, end, name) = (reference.pos, reference.end, reference.name.clone());
                self.error(
                    diagnostics::There_is_no_capturing_group_named_0_in_this_regular_expression,
                    pos,
                    end - pos,
                    vec![name.as_bytes().into()],
                );
                if let Some(suggestion) = get_spelling_suggestion_for_strings(
                    name.as_bytes(),
                    self.group_specifiers.iter().map(TokenValue::as_bytes),
                ) {
                    let suggestion = suggestion.to_vec();
                    self.error(
                        diagnostics::Did_you_mean_0,
                        pos,
                        end - pos,
                        vec![suggestion.into()],
                    );
                }
            }
        }
        for index in 0..self.decimal_escapes.len() {
            let escape = &self.decimal_escapes[index];
            if escape.value > self.number_of_capturing_groups {
                let (pos, end) = (escape.pos, escape.end);
                if self.number_of_capturing_groups > 0 {
                    self.error(diagnostics::This_backreference_refers_to_a_group_that_does_not_exist_There_are_only_0_capturing_groups_in_this_regular_expression,
                        pos, end - pos, vec![self.number_of_capturing_groups.into()]);
                } else {
                    self.error(diagnostics::This_backreference_refers_to_a_group_that_does_not_exist_There_are_no_capturing_groups_in_this_regular_expression,
                        pos, end - pos, vec![]);
                }
            }
        }
    }
}
