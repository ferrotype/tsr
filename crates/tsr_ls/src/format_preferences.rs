//! Formatting configuration shared by completion and edit requests.
use std::collections::HashMap;
use tsr_core::Tristate;
use tsr_format::{FormatCodeSettings, IndentStyle, SemicolonPreference};
use tsr_lsproto::Any;

/// Apply one raw or language-specific configuration layer. The caller orders
/// editor defaults, languages, raw/unstable fields, and stable fields as at the pin.
/// Source: ls/lsutil/userpreferences.go:setFieldFromValue and formatcodeoptions.go.
#[allow(
    clippy::implicit_hasher,
    reason = "The protocol Any object uses this concrete map, including nested configuration layers"
)]
pub fn apply_format_settings(
    fields: &HashMap<String, Any>,
    raw: bool,
    out: &mut FormatCodeSettings,
) {
    let fields = if raw {
        fields
    } else {
        let Some(Any::Object(format)) = fields.get("format") else {
            return;
        };
        format
    };
    for (name, target) in [
        (
            "convertTabsToSpaces",
            &mut out.editor.convert_tabs_to_spaces,
        ),
        (
            "trimTrailingWhitespace",
            &mut out.editor.trim_trailing_whitespace,
        ),
        (
            "insertSpaceAfterCommaDelimiter",
            &mut out.insert_space_after_comma_delimiter,
        ),
        (
            "insertSpaceAfterSemicolonInForStatements",
            &mut out.insert_space_after_semicolon_in_for_statements,
        ),
        (
            "insertSpaceBeforeAndAfterBinaryOperators",
            &mut out.insert_space_before_and_after_binary_operators,
        ),
        (
            "insertSpaceAfterConstructor",
            &mut out.insert_space_after_constructor,
        ),
        (
            "insertSpaceAfterKeywordsInControlFlowStatements",
            &mut out.insert_space_after_keywords_in_control_flow_statements,
        ),
        (
            "insertSpaceAfterFunctionKeywordForAnonymousFunctions",
            &mut out.insert_space_after_function_keyword_for_anonymous_functions,
        ),
        (
            "insertSpaceAfterOpeningAndBeforeClosingNonemptyParenthesis",
            &mut out.insert_space_after_opening_and_before_closing_nonempty_parenthesis,
        ),
        (
            "insertSpaceAfterOpeningAndBeforeClosingNonemptyBrackets",
            &mut out.insert_space_after_opening_and_before_closing_nonempty_brackets,
        ),
        (
            "insertSpaceAfterOpeningAndBeforeClosingNonemptyBraces",
            &mut out.insert_space_after_opening_and_before_closing_nonempty_braces,
        ),
        (
            "insertSpaceAfterOpeningAndBeforeClosingEmptyBraces",
            &mut out.insert_space_after_opening_and_before_closing_empty_braces,
        ),
        (
            "insertSpaceAfterOpeningAndBeforeClosingTemplateStringBraces",
            &mut out.insert_space_after_opening_and_before_closing_template_string_braces,
        ),
        (
            "insertSpaceAfterOpeningAndBeforeClosingJsxExpressionBraces",
            &mut out.insert_space_after_opening_and_before_closing_jsx_expression_braces,
        ),
        (
            "insertSpaceAfterTypeAssertion",
            &mut out.insert_space_after_type_assertion,
        ),
        (
            "insertSpaceBeforeFunctionParenthesis",
            &mut out.insert_space_before_function_parenthesis,
        ),
        (
            "placeOpenBraceOnNewLineForFunctions",
            &mut out.place_open_brace_on_new_line_for_functions,
        ),
        (
            "placeOpenBraceOnNewLineForControlBlocks",
            &mut out.place_open_brace_on_new_line_for_control_blocks,
        ),
        (
            "insertSpaceBeforeTypeAnnotation",
            &mut out.insert_space_before_type_annotation,
        ),
        (
            "indentMultiLineObjectLiteralBeginningOnBlankLine",
            &mut out.indent_multi_line_object_literal_beginning_on_blank_line,
        ),
        ("indentSwitchCase", &mut out.indent_switch_case),
    ] {
        if let Some(value) = fields.get(name).filter(|v| !matches!(v, Any::Null)) {
            *target = match value {
                Any::Boolean(value) => Tristate::from(*value),
                _ => Tristate::UNKNOWN,
            };
        }
    }
    for (name, target) in [
        ("baseIndentSize", &mut out.editor.base_indent_size),
        ("indentSize", &mut out.editor.indent_size),
        ("tabSize", &mut out.editor.tab_size),
    ] {
        if let Some(Any::Number(value)) = fields.get(name) {
            *target = *value as i64;
        }
    }
    if let Some(Any::String(value)) = fields.get("newLineCharacter") {
        out.editor.new_line_character = value.as_bytes().to_vec();
    }
    if let Some(value) = fields
        .get("indentStyle")
        .filter(|v| !matches!(v, Any::Null))
    {
        out.editor.indent_style = match value {
            Any::String(value) if value.eq_ignore_ascii_case("none") => IndentStyle::None,
            Any::String(value) if value.eq_ignore_ascii_case("block") => IndentStyle::Block,
            Any::Number(value) if (*value as i64) == 0 => IndentStyle::None,
            Any::Number(value) if (*value as i64) == 1 => IndentStyle::Block,
            _ => IndentStyle::Smart,
        };
    }
    if let Some(value) = fields.get("semicolons").filter(|v| !matches!(v, Any::Null)) {
        out.semicolons = match value {
            Any::String(value) if value.eq_ignore_ascii_case("insert") => {
                SemicolonPreference::Insert
            }
            Any::String(value) if value.eq_ignore_ascii_case("remove") => {
                SemicolonPreference::Remove
            }
            _ => SemicolonPreference::Ignore,
        };
    }
}
