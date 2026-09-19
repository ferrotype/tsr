use std::collections::HashSet;

use ts_core::ScriptTarget;
use ts_diagnostics as diagnostics;
use ts_jsstring::wtf8::{decode_utf8, RUNE_ERROR};

use super::{
    compare_decimal_strings, flag_for_character, reg_exp_flags, RegExpParser, STACK_RED_ZONE,
    STACK_SEGMENT,
};

impl RegExpParser<'_, '_> {
    /// port: tsc/internal/scanner/regexp.go:regExpParser.scanDisjunction
    pub(super) fn scan_disjunction(&mut self, is_in_group: bool) {
        stacker::maybe_grow(STACK_RED_ZONE, STACK_SEGMENT, || {
            self.scan_disjunction_inner(is_in_group);
        });
    }

    // Enter the body's stack frame only after the production growth check.
    #[inline(never)]
    fn scan_disjunction_inner(&mut self, is_in_group: bool) {
        let mut disjunction_names = HashSet::new();
        loop {
            self.named_capturing_groups.push(HashSet::new());
            self.scan_alternative(is_in_group);
            let alternative_names = self
                .named_capturing_groups
                .pop()
                .expect("disjunction pushed its alternative scope");
            disjunction_names.extend(alternative_names);
            if self.char() != i32::from(b'|') {
                break;
            }
            self.inc_pos(1);
        }
        if is_in_group {
            if let Some(parent) = self.named_capturing_groups.last_mut() {
                parent.extend(disjunction_names);
            }
        }
    }

    /// port: tsc/internal/scanner/regexp.go:regExpParser.scanAlternative
    fn scan_alternative(&mut self, is_in_group: bool) {
        let mut previous_term_quantifiable = false;
        while self.pos() < self.end {
            let start = self.pos();
            let ch = self.char();
            // char() observes a source byte (or EOF), never a decoded rune.
            match ch as u8 {
                b'^' | b'$' => {
                    self.inc_pos(1);
                    previous_term_quantifiable = false;
                }
                b'\\' => {
                    self.inc_pos(1);
                    match self.char() as u8 {
                        b'b' | b'B' => {
                            self.inc_pos(1);
                            previous_term_quantifiable = false;
                        }
                        _ => {
                            self.scan_atom_escape();
                            previous_term_quantifiable = true;
                        }
                    }
                }
                b'(' => {
                    self.inc_pos(1);
                    if self.char() == i32::from(b'?') {
                        self.inc_pos(1);
                        match self.char() as u8 {
                            b'=' | b'!' => {
                                self.inc_pos(1);
                                previous_term_quantifiable = !self.any_unicode_mode_or_non_annex_b;
                            }
                            b'<' => {
                                let group_name_start = self.pos();
                                self.inc_pos(1);
                                if matches!(self.char() as u8, b'=' | b'!') {
                                    self.inc_pos(1);
                                    previous_term_quantifiable = false;
                                } else {
                                    self.scan_group_name(false);
                                    self.scan_expected_char(b'>');
                                    if self.scanner.language_version() < ScriptTarget::ES2018 {
                                        self.error(diagnostics::Named_capturing_groups_are_only_available_when_targeting_ES2018_or_later,
                                            group_name_start, self.pos() - group_name_start, vec![]);
                                    }
                                    self.number_of_capturing_groups += 1;
                                    previous_term_quantifiable = true;
                                }
                            }
                            _ => {
                                let flags_start = self.pos();
                                let set_flags = self.scan_pattern_modifiers(reg_exp_flags::NONE);
                                if self.char() == i32::from(b'-') {
                                    self.inc_pos(1);
                                    self.scan_pattern_modifiers(set_flags);
                                    if self.pos() == flags_start + 1 {
                                        self.error(diagnostics::Subpattern_flags_must_be_present_when_there_is_a_minus_sign,
                                            flags_start, self.pos() - flags_start, vec![]);
                                    }
                                }
                                if self.pos() != flags_start
                                    && self.scanner.language_version() < ScriptTarget::ES2025
                                {
                                    self.error(diagnostics::Regular_expression_pattern_modifiers_are_only_available_when_targeting_0_or_later,
                                        flags_start, self.pos() - flags_start, vec!["es2025".into()]);
                                }
                                self.scan_expected_char(b':');
                                previous_term_quantifiable = true;
                            }
                        }
                    } else {
                        self.number_of_capturing_groups += 1;
                        previous_term_quantifiable = true;
                    }
                    self.scan_disjunction(true);
                    self.scan_expected_char(b')');
                }
                b'{' => {
                    self.inc_pos(1);
                    let digits_start = self.pos();
                    self.scan_digits();
                    let min = self.scanner.state.token_value.clone();
                    if !self.any_unicode_mode_or_non_annex_b && min.as_bytes().is_empty() {
                        previous_term_quantifiable = true;
                        continue;
                    }
                    if self.char() == i32::from(b',') {
                        self.inc_pos(1);
                        self.scan_digits();
                        let max = self.scanner.state.token_value.clone();
                        if min.as_bytes().is_empty() {
                            if !max.as_bytes().is_empty() || self.char() == i32::from(b'}') {
                                self.error(
                                    diagnostics::Incomplete_quantifier_Digit_expected,
                                    digits_start,
                                    0,
                                    vec![],
                                );
                            } else {
                                self.error(diagnostics::Unexpected_0_Did_you_mean_to_escape_it_with_backslash,
                                    start, 1, vec!["{".into()]);
                                previous_term_quantifiable = true;
                                continue;
                            }
                        } else if !max.as_bytes().is_empty()
                            && compare_decimal_strings(min.as_bytes(), max.as_bytes()).is_gt()
                            && (self.any_unicode_mode_or_non_annex_b
                                || self.char() == i32::from(b'}'))
                        {
                            self.error(
                                diagnostics::Numbers_out_of_order_in_quantifier,
                                digits_start,
                                self.pos() - digits_start,
                                vec![],
                            );
                        }
                    } else if min.as_bytes().is_empty() {
                        if self.any_unicode_mode_or_non_annex_b {
                            self.error(
                                diagnostics::Unexpected_0_Did_you_mean_to_escape_it_with_backslash,
                                start,
                                1,
                                vec!["{".into()],
                            );
                        }
                        previous_term_quantifiable = true;
                        continue;
                    }
                    if self.char() != i32::from(b'}') {
                        if self.any_unicode_mode_or_non_annex_b {
                            self.error(diagnostics::X_0_expected, self.pos(), 0, vec!["}".into()]);
                            self.inc_pos(-1);
                        } else {
                            previous_term_quantifiable = true;
                            continue;
                        }
                    }
                    self.finish_quantifier(start, previous_term_quantifiable);
                    previous_term_quantifiable = false;
                }
                b'*' | b'+' | b'?' => {
                    self.finish_quantifier(start, previous_term_quantifiable);
                    previous_term_quantifiable = false;
                }
                b'.' => {
                    self.inc_pos(1);
                    previous_term_quantifiable = true;
                }
                b'[' => {
                    self.inc_pos(1);
                    if self.unicode_sets_mode {
                        self.scan_class_set_expression();
                    } else {
                        self.scan_class_ranges();
                        self.pending_low_surrogate = 0;
                    }
                    self.scan_expected_char(b']');
                    previous_term_quantifiable = true;
                }
                b')' if is_in_group => return,
                b')' | b']' | b'}' => {
                    if self.any_unicode_mode_or_non_annex_b || ch == i32::from(b')') {
                        self.error(
                            diagnostics::Unexpected_0_Did_you_mean_to_escape_it_with_backslash,
                            self.pos(),
                            1,
                            vec![vec![ch as u8].into()],
                        );
                    }
                    self.inc_pos(1);
                    previous_term_quantifiable = true;
                }
                b'/' | b'|' => return,
                _ => {
                    self.scan_source_character();
                    previous_term_quantifiable = true;
                }
            }
        }
    }

    fn finish_quantifier(&mut self, start: i64, previous_term_quantifiable: bool) {
        self.inc_pos(1);
        if self.char() == i32::from(b'?') {
            self.inc_pos(1);
        }
        if !previous_term_quantifiable {
            self.error(
                diagnostics::There_is_nothing_available_for_repetition,
                start,
                self.pos() - start,
                vec![],
            );
        }
    }

    /// port: tsc/internal/scanner/regexp.go:regExpParser.scanPatternModifiers
    fn scan_pattern_modifiers(&mut self, mut current_flags: i32) -> i32 {
        while self.pos() < self.end {
            let (ch, size) = decode_utf8(self.scanner.tail(self.pos()));
            if ch == RUNE_ERROR || !crate::is_identifier_part(ch) {
                break;
            }
            if let Some(flag) = flag_for_character(ch) {
                let message = if current_flags & flag != 0 {
                    Some(diagnostics::Duplicate_regular_expression_flag)
                } else if flag & reg_exp_flags::MODIFIERS == 0 {
                    Some(diagnostics::This_regular_expression_flag_cannot_be_toggled_within_a_subpattern)
                } else {
                    current_flags |= flag;
                    None
                };
                if let Some(message) = message {
                    self.error(message, self.pos(), size as i64, vec![]);
                }
            } else {
                self.error(
                    diagnostics::Unknown_regular_expression_flag,
                    self.pos(),
                    size as i64,
                    vec![],
                );
            }
            self.inc_pos(size as i64);
        }
        current_flags
    }
}
