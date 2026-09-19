//! Formatting settings. Upstream keeps them in the language service's utility
//! package (`ls/lsutil/formatcodeoptions.go`); they live here until that crate
//! exists. Parsing them from editor configuration and converting to and from
//! the protocol's options belong to the language service and are not ported.

use ts_core::Tristate;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum IndentStyle {
    None,
    Block,
    #[default]
    Smart,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum SemicolonPreference {
    #[default]
    Ignore,
    Insert,
    Remove,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EditorSettings {
    pub base_indent_size: i64,
    pub indent_size: i64,
    pub tab_size: i64,
    pub new_line_character: Vec<u8>,
    pub convert_tabs_to_spaces: Tristate,
    pub indent_style: IndentStyle,
    pub trim_trailing_whitespace: Tristate,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FormatCodeSettings {
    pub editor: EditorSettings,
    pub insert_space_after_comma_delimiter: Tristate,
    pub insert_space_after_semicolon_in_for_statements: Tristate,
    pub insert_space_before_and_after_binary_operators: Tristate,
    pub insert_space_after_constructor: Tristate,
    pub insert_space_after_keywords_in_control_flow_statements: Tristate,
    pub insert_space_after_function_keyword_for_anonymous_functions: Tristate,
    pub insert_space_after_opening_and_before_closing_nonempty_parenthesis: Tristate,
    pub insert_space_after_opening_and_before_closing_nonempty_brackets: Tristate,
    pub insert_space_after_opening_and_before_closing_nonempty_braces: Tristate,
    pub insert_space_after_opening_and_before_closing_empty_braces: Tristate,
    pub insert_space_after_opening_and_before_closing_template_string_braces: Tristate,
    pub insert_space_after_opening_and_before_closing_jsx_expression_braces: Tristate,
    pub insert_space_after_type_assertion: Tristate,
    pub insert_space_before_function_parenthesis: Tristate,
    pub place_open_brace_on_new_line_for_functions: Tristate,
    pub place_open_brace_on_new_line_for_control_blocks: Tristate,
    pub insert_space_before_type_annotation: Tristate,
    pub indent_multi_line_object_literal_beginning_on_blank_line: Tristate,
    pub semicolons: SemicolonPreference,
    pub indent_switch_case: Tristate,
}

/// `printer.GetDefaultIndentSize`.
const DEFAULT_INDENT_SIZE: i64 = 4;

impl Default for FormatCodeSettings {
    // port: tsc/internal/ls/lsutil/formatcodeoptions.go:GetDefaultFormatCodeSettings
    fn default() -> Self {
        Self {
            editor: EditorSettings {
                base_indent_size: 0,
                indent_size: DEFAULT_INDENT_SIZE,
                tab_size: DEFAULT_INDENT_SIZE,
                new_line_character: b"\n".to_vec(),
                convert_tabs_to_spaces: Tristate::TRUE,
                indent_style: IndentStyle::Smart,
                trim_trailing_whitespace: Tristate::TRUE,
            },
            insert_space_after_constructor: Tristate::FALSE,
            insert_space_after_comma_delimiter: Tristate::TRUE,
            insert_space_after_semicolon_in_for_statements: Tristate::TRUE,
            insert_space_before_and_after_binary_operators: Tristate::TRUE,
            insert_space_after_keywords_in_control_flow_statements: Tristate::TRUE,
            insert_space_after_function_keyword_for_anonymous_functions: Tristate::FALSE,
            insert_space_after_opening_and_before_closing_nonempty_parenthesis: Tristate::FALSE,
            insert_space_after_opening_and_before_closing_nonempty_brackets: Tristate::FALSE,
            insert_space_after_opening_and_before_closing_nonempty_braces: Tristate::TRUE,
            insert_space_after_opening_and_before_closing_empty_braces: Tristate::UNKNOWN,
            insert_space_after_opening_and_before_closing_template_string_braces: Tristate::FALSE,
            insert_space_after_opening_and_before_closing_jsx_expression_braces: Tristate::FALSE,
            insert_space_after_type_assertion: Tristate::UNKNOWN,
            insert_space_before_function_parenthesis: Tristate::FALSE,
            place_open_brace_on_new_line_for_functions: Tristate::FALSE,
            place_open_brace_on_new_line_for_control_blocks: Tristate::FALSE,
            insert_space_before_type_annotation: Tristate::UNKNOWN,
            indent_multi_line_object_literal_beginning_on_blank_line: Tristate::UNKNOWN,
            semicolons: SemicolonPreference::Ignore,
            indent_switch_case: Tristate::TRUE,
        }
    }
}
