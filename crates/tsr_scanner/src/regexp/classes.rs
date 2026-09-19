use std::borrow::Cow;

use ts_diagnostics as diagnostics;
use ts_jsstring::wtf8::decode_rune;

use super::{Atom, ClassSetExpressionType, RegExpParser, STACK_RED_ZONE, STACK_SEGMENT};

impl<'src> RegExpParser<'_, 'src> {
    /// port: tsc/internal/scanner/regexp.go:regExpParser.scanClassRanges
    pub(super) fn scan_class_ranges(&mut self) {
        self.assert_after(b'[');
        self.pending_low_surrogate = 0;
        if self.char() == i32::from(b'^') {
            self.inc_pos(1);
        }
        while self.pos() < self.end {
            if self.is_class_content_exit(self.char()) {
                return;
            }
            let min_start = self.pos();
            let min = self.scan_class_atom();
            if self.char() == i32::from(b'-') {
                self.inc_pos(1);
                if self.is_class_content_exit(self.char()) {
                    return;
                }
                if min.is_empty() && self.any_unicode_mode_or_non_annex_b {
                    self.error(diagnostics::A_character_class_range_must_not_be_bounded_by_another_character_class,
                        min_start, self.pos() - 1 - min_start, vec![]);
                }
                let max_start = self.pos();
                let max = self.scan_class_atom();
                if max.is_empty() && self.any_unicode_mode_or_non_annex_b {
                    self.error(diagnostics::A_character_class_range_must_not_be_bounded_by_another_character_class,
                        max_start, self.pos() - max_start, vec![]);
                    continue;
                }
                if min.is_empty() {
                    continue;
                }
                self.check_range_order(&min, &max, min_start);
            }
        }
    }

    fn check_range_order(&mut self, min: &[u8], max: &[u8], start: i64) {
        let (min_value, min_size) = decode_rune(min);
        let (max_value, max_size) = decode_rune(max);
        if min.len() == min_size && max.len() == max_size && min_value > max_value {
            self.error(
                diagnostics::Range_out_of_order_in_character_class,
                start,
                self.pos() - start,
                vec![],
            );
        }
    }

    /// port: tsc/internal/scanner/regexp.go:regExpParser.scanClassSetExpression
    pub(super) fn scan_class_set_expression(&mut self) {
        stacker::maybe_grow(STACK_RED_ZONE, STACK_SEGMENT, || {
            self.scan_class_set_expression_inner();
        });
    }

    #[inline(never)]
    fn scan_class_set_expression_inner(&mut self) {
        self.assert_after(b'[');
        let complement = self.char() == i32::from(b'^');
        if complement {
            self.inc_pos(1);
        }
        let mut expression_may_contain_strings = false;
        if self.is_class_content_exit(self.char()) {
            return;
        }
        let mut start = self.pos();
        let mut operand: Atom<'src> = Cow::Borrowed(b"");
        if self.at_set_operator() {
            self.error(
                diagnostics::Expected_a_class_set_operand,
                self.pos(),
                0,
                vec![],
            );
            self.may_contain_strings = false;
        } else {
            operand = self.scan_class_set_operand();
        }
        match self.char() as u8 {
            b'-' if self.pos() + 1 < self.end
                && self.char_at(self.pos() + 1) == i32::from(b'-') =>
            {
                if complement && self.may_contain_strings {
                    self.negated_strings_error(start);
                }
                expression_may_contain_strings = self.may_contain_strings;
                self.scan_class_set_sub_expression(ClassSetExpressionType::Subtraction);
                self.may_contain_strings = !complement && expression_may_contain_strings;
                return;
            }
            b'&' if self.pos() + 1 < self.end
                && self.char_at(self.pos() + 1) == i32::from(b'&') =>
            {
                self.scan_class_set_sub_expression(ClassSetExpressionType::Intersection);
                if complement && self.may_contain_strings {
                    self.negated_strings_error(start);
                }
                expression_may_contain_strings = self.may_contain_strings;
                self.may_contain_strings = !complement && expression_may_contain_strings;
                return;
            }
            // Go's '-'/'&' cases with no doubled operator skip the default arm.
            b'-' | b'&' => {}
            _ => {
                if complement && self.may_contain_strings {
                    self.negated_strings_error(start);
                }
                expression_may_contain_strings = self.may_contain_strings;
            }
        }
        while self.pos() < self.end {
            match self.char() as u8 {
                b'-' => {
                    self.inc_pos(1);
                    let ch = self.char();
                    if self.is_class_content_exit(ch) {
                        self.may_contain_strings = !complement && expression_may_contain_strings;
                        return;
                    }
                    if ch == i32::from(b'-') {
                        self.inc_pos(1);
                        self.mixed_operators_error(self.pos() - 2, 2);
                        start = self.pos() - 2;
                        operand = Cow::Borrowed(self.slice(start, self.pos()));
                        continue;
                    }
                    if operand.is_empty() {
                        self.error(diagnostics::A_character_class_range_must_not_be_bounded_by_another_character_class,
                            start, self.pos() - 1 - start, vec![]);
                    }
                    let second_start = self.pos();
                    let second_operand = self.scan_class_set_operand();
                    if complement && self.may_contain_strings {
                        self.negated_strings_error(second_start);
                    }
                    expression_may_contain_strings |= self.may_contain_strings;
                    if second_operand.is_empty() {
                        self.error(diagnostics::A_character_class_range_must_not_be_bounded_by_another_character_class,
                            second_start, self.pos() - second_start, vec![]);
                    } else if !operand.is_empty() {
                        self.check_range_order(&operand, &second_operand, start);
                    }
                }
                b'&' if self.pos() + 1 < self.end
                    && self.char_at(self.pos() + 1) == i32::from(b'&') =>
                {
                    start = self.pos();
                    self.inc_pos(2);
                    self.mixed_operators_error(self.pos() - 2, 2);
                    if self.char() == i32::from(b'&') {
                        self.error(
                            diagnostics::Unexpected_0_Did_you_mean_to_escape_it_with_backslash,
                            self.pos(),
                            1,
                            vec!["&".into()],
                        );
                        self.inc_pos(1);
                    }
                    operand = Cow::Borrowed(self.slice(start, self.pos()));
                    continue;
                }
                _ => {}
            }
            if self.is_class_content_exit(self.char()) {
                break;
            }
            start = self.pos();
            if self.at_set_operator() {
                self.mixed_operators_error(self.pos(), 2);
                self.inc_pos(2);
                operand = Cow::Borrowed(self.slice(start, self.pos()));
            } else {
                operand = self.scan_class_set_operand();
                if complement && self.may_contain_strings {
                    self.negated_strings_error(start);
                }
            }
            expression_may_contain_strings |= self.may_contain_strings;
        }
        self.may_contain_strings = !complement && expression_may_contain_strings;
    }

    fn at_set_operator(&self) -> bool {
        self.pos() + 1 < self.end && matches!(self.slice(self.pos(), self.pos() + 2), b"--" | b"&&")
    }

    fn negated_strings_error(&mut self, start: i64) {
        self.error(diagnostics::Anything_that_would_possibly_match_more_than_a_single_character_is_invalid_inside_a_negated_character_class,
            start, self.pos() - start, vec![]);
    }

    fn mixed_operators_error(&mut self, start: i64, length: i64) {
        self.error(diagnostics::Operators_must_not_be_mixed_within_a_character_class_Wrap_it_in_a_nested_class_instead,
            start, length, vec![]);
    }

    /// port: tsc/internal/scanner/regexp.go:regExpParser.scanClassSetSubExpression
    fn scan_class_set_sub_expression(&mut self, expression_type: ClassSetExpressionType) {
        let mut expression_may_contain_strings = self.may_contain_strings;
        while self.pos() < self.end {
            if self.is_class_content_exit(self.char()) {
                break;
            }
            match self.char() as u8 {
                b'-' => {
                    self.inc_pos(1);
                    if self.char() == i32::from(b'-') {
                        self.inc_pos(1);
                        if expression_type != ClassSetExpressionType::Subtraction {
                            self.mixed_operators_error(self.pos() - 2, 2);
                        }
                    } else {
                        self.mixed_operators_error(self.pos() - 1, 1);
                    }
                }
                b'&' => {
                    self.inc_pos(1);
                    if self.char() == i32::from(b'&') {
                        self.inc_pos(1);
                        if expression_type != ClassSetExpressionType::Intersection {
                            self.mixed_operators_error(self.pos() - 2, 2);
                        }
                        if self.char() == i32::from(b'&') {
                            self.error(
                                diagnostics::Unexpected_0_Did_you_mean_to_escape_it_with_backslash,
                                self.pos(),
                                1,
                                vec!["&".into()],
                            );
                            self.inc_pos(1);
                        }
                    } else {
                        self.error(
                            diagnostics::Unexpected_0_Did_you_mean_to_escape_it_with_backslash,
                            self.pos() - 1,
                            1,
                            vec!["&".into()],
                        );
                    }
                }
                _ => {
                    let expected = match expression_type {
                        ClassSetExpressionType::Subtraction => "--",
                        ClassSetExpressionType::Intersection => "&&",
                    };
                    self.error(
                        diagnostics::X_0_expected,
                        self.pos(),
                        0,
                        vec![expected.into()],
                    );
                }
            }
            if self.is_class_content_exit(self.char()) {
                self.error(
                    diagnostics::Expected_a_class_set_operand,
                    self.pos(),
                    0,
                    vec![],
                );
                break;
            }
            self.scan_class_set_operand();
            if expression_type == ClassSetExpressionType::Intersection {
                expression_may_contain_strings &= self.may_contain_strings;
            }
        }
        self.may_contain_strings = expression_may_contain_strings;
    }

    /// port: tsc/internal/scanner/regexp.go:regExpParser.scanClassSetOperand
    fn scan_class_set_operand(&mut self) -> Atom<'src> {
        self.may_contain_strings = false;
        match self.char() as u8 {
            b'[' => {
                self.inc_pos(1);
                self.scan_class_set_expression();
                self.scan_expected_char(b']');
                Cow::Borrowed(b"")
            }
            b'\\' => {
                self.inc_pos(1);
                if self.scan_character_class_escape() {
                    return Cow::Borrowed(b"");
                }
                if self.char() == i32::from(b'q') {
                    self.inc_pos(1);
                    if self.char() == i32::from(b'{') {
                        self.inc_pos(1);
                        self.scan_class_string_disjunction_contents();
                        self.scan_expected_char(b'}');
                        Cow::Borrowed(b"")
                    } else {
                        self.error(diagnostics::X_q_must_be_followed_by_string_alternatives_enclosed_in_braces,
                            self.pos() - 2, 2, vec![]);
                        Cow::Borrowed(b"q")
                    }
                } else {
                    self.inc_pos(-1);
                    self.scan_class_set_character()
                }
            }
            _ => self.scan_class_set_character(),
        }
    }

    /// port: tsc/internal/scanner/regexp.go:regExpParser.scanClassStringDisjunctionContents
    fn scan_class_string_disjunction_contents(&mut self) {
        self.assert_after(b'{');
        let mut character_count = 0_i64;
        while self.pos() < self.end {
            match self.char() as u8 {
                b'}' => {
                    if character_count != 1 {
                        self.may_contain_strings = true;
                    }
                    return;
                }
                b'|' => {
                    if character_count != 1 {
                        self.may_contain_strings = true;
                    }
                    character_count = 0;
                    self.inc_pos(1);
                }
                _ => {
                    self.scan_class_set_character();
                    character_count += 1;
                }
            }
        }
    }

    /// port: tsc/internal/scanner/regexp.go:regExpParser.scanClassSetCharacter
    fn scan_class_set_character(&mut self) -> Atom<'src> {
        let ch = self.char();
        if ch == i32::from(b'\\') {
            self.inc_pos(1);
            match self.char() as u8 {
                b'b' => {
                    self.inc_pos(1);
                    return Cow::Borrowed(b"\x08");
                }
                b'&' | b'-' | b'!' | b'#' | b'%' | b',' | b':' | b';' | b'<' | b'=' | b'>'
                | b'@' | b'`' | b'~' => {
                    let start = self.pos();
                    self.inc_pos(1);
                    return Cow::Borrowed(self.slice(start, self.pos()));
                }
                _ => return self.scan_character_escape(false),
            }
        } else if self.pos() + 1 < self.end
            && ch == self.char_at(self.pos() + 1)
            && matches!(
                ch as u8,
                b'&' | b'!'
                    | b'#'
                    | b'%'
                    | b'*'
                    | b'+'
                    | b','
                    | b'.'
                    | b':'
                    | b';'
                    | b'<'
                    | b'='
                    | b'>'
                    | b'?'
                    | b'@'
                    | b'`'
                    | b'~'
            )
        {
            self.error(diagnostics::A_character_class_must_not_contain_a_reserved_double_punctuator_Did_you_mean_to_escape_it_with_backslash,
                self.pos(), 2, vec![]);
            self.inc_pos(2);
            return Cow::Borrowed(self.slice(self.pos() - 2, self.pos()));
        }
        if matches!(
            ch as u8,
            b'/' | b'(' | b')' | b'[' | b']' | b'{' | b'}' | b'-' | b'|'
        ) {
            self.error(
                diagnostics::Unexpected_0_Did_you_mean_to_escape_it_with_backslash,
                self.pos(),
                1,
                vec![vec![ch as u8].into()],
            );
            let start = self.pos();
            self.inc_pos(1);
            return Cow::Borrowed(self.slice(start, self.pos()));
        }
        self.scan_source_character()
    }

    /// port: tsc/internal/scanner/regexp.go:regExpParser.scanClassAtom
    fn scan_class_atom(&mut self) -> Atom<'src> {
        if self.char() == i32::from(b'\\') {
            self.inc_pos(1);
            match self.char() as u8 {
                b'b' => {
                    self.inc_pos(1);
                    Cow::Borrowed(b"\x08")
                }
                b'-' => {
                    self.inc_pos(1);
                    Cow::Borrowed(b"-")
                }
                _ => {
                    if self.scan_character_class_escape() {
                        Cow::Borrowed(b"")
                    } else {
                        self.scan_character_escape(false)
                    }
                }
            }
        } else {
            self.scan_source_character()
        }
    }
}
