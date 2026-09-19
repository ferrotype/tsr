use ts_diagnostics as diagnostics;

use crate::spelling::get_spelling_suggestion_for_strings;
use crate::tables_generated::{
    BINARY_PROPERTIES, GENERAL_CATEGORIES, NON_BINARY_PROPERTIES, SCRIPT_VALUES, STRING_PROPERTIES,
};

use super::RegExpParser;

fn contains(table: &[&str], value: &[u8]) -> bool {
    table
        .binary_search_by(|candidate| candidate.as_bytes().cmp(value))
        .is_ok()
}

fn property_values(name: &[u8]) -> Option<&'static [&'static str]> {
    match name {
        b"General_Category" => Some(GENERAL_CATEGORIES),
        b"Script" | b"Script_Extensions" => Some(SCRIPT_VALUES),
        _ => None,
    }
}

impl<'src> RegExpParser<'_, 'src> {
    /// port: tsc/internal/scanner/regexp.go:regExpParser.scanCharacterClassEscape
    pub(super) fn scan_character_class_escape(&mut self) -> bool {
        self.assert_after(b'\\');
        let start = self.pos() - 1;
        let ch = self.char();
        if matches!(ch as u8, b'd' | b'D' | b's' | b'S' | b'w' | b'W') {
            self.inc_pos(1);
            return true;
        }
        if !matches!(ch as u8, b'P' | b'p') {
            return false;
        }
        let complement = ch == i32::from(b'P');
        self.inc_pos(1);
        if self.char() == i32::from(b'{') {
            self.inc_pos(1);
            let name_or_value_start = self.pos();
            let name_or_value = self.scan_word_characters();
            if self.char() == i32::from(b'=') {
                let property_name = NON_BINARY_PROPERTIES
                    .binary_search_by(|(alias, _)| alias.as_bytes().cmp(name_or_value))
                    .ok()
                    .map(|index| NON_BINARY_PROPERTIES[index].1.as_bytes());
                if self.pos() == name_or_value_start {
                    self.error(
                        diagnostics::Expected_a_Unicode_property_name,
                        self.pos(),
                        0,
                        vec![],
                    );
                } else if property_name.is_none() {
                    self.error(
                        diagnostics::Unknown_Unicode_property_name,
                        name_or_value_start,
                        self.pos() - name_or_value_start,
                        vec![],
                    );
                    if let Some(suggestion) =
                        Self::get_spelling_suggestion_for_unicode_property_name(name_or_value)
                    {
                        self.error(
                            diagnostics::Did_you_mean_0,
                            name_or_value_start,
                            self.pos() - name_or_value_start,
                            vec![suggestion.into()],
                        );
                    }
                }
                self.inc_pos(1);
                let value_start = self.pos();
                let value = self.scan_word_characters();
                if self.pos() == value_start {
                    self.error(
                        diagnostics::Expected_a_Unicode_property_value,
                        self.pos(),
                        0,
                        vec![],
                    );
                } else if let Some(name) = property_name {
                    if property_values(name).is_some_and(|values| !contains(values, value)) {
                        self.error(
                            diagnostics::Unknown_Unicode_property_value,
                            value_start,
                            self.pos() - value_start,
                            vec![],
                        );
                        if let Some(suggestion) =
                            Self::get_spelling_suggestion_for_unicode_property_value(name, value)
                        {
                            self.error(
                                diagnostics::Did_you_mean_0,
                                value_start,
                                self.pos() - value_start,
                                vec![suggestion.into()],
                            );
                        }
                    }
                }
            } else if self.pos() == name_or_value_start {
                self.error(
                    diagnostics::Expected_a_Unicode_property_name_or_value,
                    self.pos(),
                    0,
                    vec![],
                );
            } else if contains(STRING_PROPERTIES, name_or_value) {
                if !self.unicode_sets_mode {
                    self.error(diagnostics::Any_Unicode_property_that_would_possibly_match_more_than_a_single_character_is_only_available_when_the_Unicode_Sets_v_flag_is_set,
                        name_or_value_start, self.pos() - name_or_value_start, vec![]);
                } else if complement {
                    self.error(diagnostics::Anything_that_would_possibly_match_more_than_a_single_character_is_invalid_inside_a_negated_character_class,
                        name_or_value_start, self.pos() - name_or_value_start, vec![]);
                } else {
                    self.may_contain_strings = true;
                }
            } else if !contains(GENERAL_CATEGORIES, name_or_value)
                && !contains(BINARY_PROPERTIES, name_or_value)
            {
                self.error(
                    diagnostics::Unknown_Unicode_property_name_or_value,
                    name_or_value_start,
                    self.pos() - name_or_value_start,
                    vec![],
                );
                if let Some(suggestion) =
                    Self::get_spelling_suggestion_for_unicode_property_name_or_value(name_or_value)
                {
                    self.error(
                        diagnostics::Did_you_mean_0,
                        name_or_value_start,
                        self.pos() - name_or_value_start,
                        vec![suggestion.into()],
                    );
                }
            }
            self.scan_expected_char(b'}');
            if !self.any_unicode_mode {
                self.error(diagnostics::Unicode_property_value_expressions_are_only_available_when_the_Unicode_u_flag_or_the_Unicode_Sets_v_flag_is_set,
                    start, self.pos() - start, vec![]);
            }
        } else if self.any_unicode_mode_or_non_annex_b {
            self.error(diagnostics::X_0_must_be_followed_by_a_Unicode_property_value_expression_enclosed_in_braces,
                self.pos() - 2, 2, vec![vec![ch as u8].into()]);
        } else {
            self.inc_pos(-1);
            return false;
        }
        true
    }

    /// port: tsc/internal/scanner/regexp.go:regExpParser.getSpellingSuggestionForUnicodePropertyName
    fn get_spelling_suggestion_for_unicode_property_name(name: &[u8]) -> Option<&'static [u8]> {
        get_spelling_suggestion_for_strings(
            name,
            NON_BINARY_PROPERTIES
                .iter()
                .map(|(alias, _)| alias.as_bytes()),
        )
    }

    /// port: tsc/internal/scanner/regexp.go:regExpParser.getSpellingSuggestionForUnicodePropertyValue
    fn get_spelling_suggestion_for_unicode_property_value(
        property_name: &[u8],
        value: &[u8],
    ) -> Option<&'static [u8]> {
        get_spelling_suggestion_for_strings(
            value,
            property_values(property_name)?
                .iter()
                .map(|name| name.as_bytes()),
        )
    }

    /// port: tsc/internal/scanner/regexp.go:regExpParser.getSpellingSuggestionForUnicodePropertyNameOrValue
    fn get_spelling_suggestion_for_unicode_property_name_or_value(
        name: &[u8],
    ) -> Option<&'static [u8]> {
        get_spelling_suggestion_for_strings(
            name,
            GENERAL_CATEGORIES
                .iter()
                .chain(BINARY_PROPERTIES)
                .chain(STRING_PROPERTIES)
                .map(|name| name.as_bytes()),
        )
    }

    /// port: tsc/internal/scanner/regexp.go:regExpParser.scanWordCharacters
    fn scan_word_characters(&mut self) -> &'src [u8] {
        let start = self.pos();
        while self.pos() < self.end && crate::identifier::is_word_character(self.char()) {
            self.inc_pos(1);
        }
        self.slice(start, self.pos())
    }
}
