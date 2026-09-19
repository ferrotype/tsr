//! Entry points for `tools/s09/format-harness`. They drive crate-private parts
//! the way the native oracle's bridge does (`tools/s09/format_oracle`), so the
//! two can be compared row for row. Not part of the formatter's API.

use crate::{
    scanner::{FormattingScanner, TextRangeWithKind},
    Error, FormatFile,
};

fn range(value: TextRangeWithKind) -> String {
    format!(
        "{},{},{}",
        value.kind as u16,
        value.loc.pos(),
        value.loc.end()
    )
}

fn ranges(values: &[TextRangeWithKind]) -> String {
    values
        .iter()
        .map(|value| range(*value))
        .collect::<Vec<_>>()
        .join(";")
}

/// Drives the formatting scanner over the whole file. The container of each
/// token is the token-level node navigation finds at the token's start, which
/// gives the rescan predicates realistic input. A failure where upstream panics
/// ends the probe with a final `!` row.
pub fn scan(file: &mut FormatFile<'_, '_>, row: &mut dyn FnMut(&str)) {
    if let Err(error) = scan_rows(file, row) {
        row(&format!("K|!{error}"));
    }
}

fn scan_rows(file: &mut FormatFile<'_, '_>, row: &mut dyn FnMut(&str)) -> Result<(), Error> {
    let view = file.view;
    let state = view.source_file(file.source)?;
    let text = state.text().as_bytes();
    let mut scanner = FormattingScanner::new(text, state.language_variant, 0, text.len() as i64);
    scanner.advance();
    while scanner.is_on_token() {
        let container = file
            .navigator()
            .get_token_at_position(scanner.scanner_token_start())?;
        let info = scanner.read_token_info(view, container)?;
        row(&format!(
            "K|{}|{}|{}|{}|{}",
            range(info.token),
            view.node(container)?.kind().raw(),
            u8::from(scanner.last_trailing_trivia_was_new_line()),
            ranges(&info.leading_trivia),
            ranges(&info.trailing_trivia)
        ));
        scanner.advance();
    }
    if scanner.is_on_eof() {
        row(&format!(
            "E|{}|{}",
            range(scanner.read_eof_token_range()?),
            ranges(scanner.current_leading_trivia())
        ));
    } else {
        row("E|-");
    }
    Ok(())
}

/// The settings variants the oracle names, mirrored here by name.
pub fn variant(name: &str) -> Option<crate::FormatCodeSettings> {
    use ts_core::Tristate;
    let mut settings = crate::FormatCodeSettings::default();
    match name {
        "default" => {}
        "tabs" => settings.editor.convert_tabs_to_spaces = Tristate::FALSE,
        "two" => {
            settings.editor.indent_size = 2;
            settings.editor.tab_size = 2;
        }
        "dense" => {
            settings.insert_space_after_constructor = Tristate::TRUE;
            settings.insert_space_after_function_keyword_for_anonymous_functions = Tristate::TRUE;
            settings.insert_space_after_opening_and_before_closing_nonempty_parenthesis =
                Tristate::TRUE;
            settings.insert_space_after_opening_and_before_closing_nonempty_brackets =
                Tristate::TRUE;
            settings.insert_space_after_opening_and_before_closing_empty_braces = Tristate::TRUE;
            settings.insert_space_after_opening_and_before_closing_template_string_braces =
                Tristate::TRUE;
            settings.insert_space_after_opening_and_before_closing_jsx_expression_braces =
                Tristate::TRUE;
            settings.insert_space_after_type_assertion = Tristate::TRUE;
            settings.insert_space_before_function_parenthesis = Tristate::TRUE;
            settings.insert_space_before_type_annotation = Tristate::TRUE;
            settings.place_open_brace_on_new_line_for_functions = Tristate::TRUE;
            settings.place_open_brace_on_new_line_for_control_blocks = Tristate::TRUE;
            settings.semicolons = crate::SemicolonPreference::Insert;
        }
        "terse" => {
            settings.insert_space_after_comma_delimiter = Tristate::FALSE;
            settings.insert_space_after_semicolon_in_for_statements = Tristate::FALSE;
            settings.insert_space_before_and_after_binary_operators = Tristate::FALSE;
            settings.insert_space_after_keywords_in_control_flow_statements = Tristate::FALSE;
            settings.insert_space_after_opening_and_before_closing_nonempty_braces =
                Tristate::FALSE;
            settings.indent_switch_case = Tristate::FALSE;
            settings.editor.trim_trailing_whitespace = Tristate::FALSE;
            settings.semicolons = crate::SemicolonPreference::Remove;
        }
        _ => return None,
    }
    Some(settings)
}

struct Item {
    span: TextRangeWithKind,
    parent: ts_arena::NodeId,
}

fn is_comment(kind: ts_ast::SyntaxKind) -> bool {
    matches!(
        kind,
        ts_ast::SyntaxKind::SingleLineCommentTrivia | ts_ast::SyntaxKind::MultiLineCommentTrivia
    )
}

/// Every token of the file with the comments of its leading and trailing trivia
/// around it. An item's parent is the parent of the token-level node navigation
/// finds at the token's start, or that node itself when it has none.
fn items(file: &mut FormatFile<'_, '_>) -> Result<Vec<Item>, Error> {
    let view = file.view;
    let state = view.source_file(file.source)?;
    let text = state.text().as_bytes();
    let mut scanner = FormattingScanner::new(text, state.language_variant, 0, text.len() as i64);
    let mut out = Vec::new();
    scanner.advance();
    while scanner.is_on_token() {
        let container = file
            .navigator()
            .get_token_at_position(scanner.scanner_token_start())?;
        let parent = view.node(container)?.parent().unwrap_or(container);
        let info = scanner.read_token_info(view, container)?;
        let comments = |trivia: &[TextRangeWithKind]| {
            trivia
                .iter()
                .filter(|item| is_comment(item.kind))
                .map(|&span| Item { span, parent })
                .collect::<Vec<_>>()
        };
        out.extend(comments(&info.leading_trivia));
        out.push(Item {
            span: info.token,
            parent,
        });
        out.extend(comments(&info.trailing_trivia));
        scanner.advance();
    }
    Ok(out)
}

fn common_ancestor(
    file: &FormatFile<'_, '_>,
    a: ts_arena::NodeId,
    b: ts_arena::NodeId,
) -> Result<ts_arena::NodeId, Error> {
    let mut seen = std::collections::HashSet::new();
    let mut current = Some(a);
    while let Some(node) = current {
        seen.insert(node);
        current = file.node(node)?.parent();
    }
    let mut current = Some(b);
    while let Some(node) = current {
        if seen.contains(&node) {
            return Ok(node);
        }
        current = file.node(node)?.parent();
    }
    Ok(file.source)
}

/// Which rules apply between every pair of adjacent items, in the context of
/// their lowest common ancestor. A failure where upstream panics while
/// answering one pair becomes that row's value.
pub fn rules(
    file: &mut FormatFile<'_, '_>,
    options: crate::FormatCodeSettings,
    row: &mut dyn FnMut(&str),
) {
    let items = match items(file) {
        Ok(items) => items,
        Err(error) => {
            row(&format!("R|!{error}"));
            Vec::new()
        }
    };
    let mut context =
        crate::FormattingContext::new(file, crate::FormatRequestKind::FormatDocument, options);
    for pair in items.windows(2) {
        let (current, next) = (&pair[0], &pair[1]);
        let head = format!("R|{}|{}|", range(current.span), range(next.span));
        let answer = (|| -> Result<String, Error> {
            let common = common_ancestor(context.file, current.parent, next.parent)?;
            context.update_context(current.span, current.parent, next.span, next.parent, common);
            let names: Vec<&str> = crate::rulesmap::get_rules(&mut context)?
                .iter()
                .map(|rule| rule.debug_name)
                .collect();
            Ok(format!(
                "{}|{}",
                context.file.node(common)?.kind().raw(),
                names.join(",")
            ))
        })();
        match answer {
            Ok(text) => row(&format!("{head}{text}")),
            Err(error) => row(&format!("{head}!{error}")),
        }
    }
}

/// Every non-empty bucket of the rules map, in order.
pub fn rules_map(row: &mut dyn FnMut(&str)) {
    let map = crate::rulesmap::get_rules_map();
    let row_length = ts_ast::SyntaxKind::LastToken as usize + 1;
    for index in 0..map.buckets() {
        let names: Vec<&str> = map.bucket(index).map(|rule| rule.debug_name).collect();
        if !names.is_empty() {
            row(&format!(
                "B|{}|{}|{}",
                index / row_length,
                index % row_length,
                names.join(",")
            ));
        }
    }
}

/// The indentation at every line start, with and without the close-brace
/// assumption, and at every eighth of the given positions.
pub fn indent(
    file: &mut FormatFile<'_, '_>,
    options: &crate::FormatCodeSettings,
    positions: &[i64],
    row: &mut dyn FnMut(&str),
) {
    let answer = |file: &mut FormatFile<'_, '_>, position: i64, assume: bool| {
        match crate::indent::get_indentation(file, position, options, assume) {
            Ok(value) => value.to_string(),
            Err(error) => format!("!{error}"),
        }
    };
    let line_starts: Vec<i64> = match file.view.source_file(file.source) {
        Ok(state) => state
            .ecma_line_map()
            .iter()
            .map(|&p| i64::from(p))
            .collect(),
        Err(error) => {
            row(&format!("L|!{error:?}"));
            Vec::new()
        }
    };
    for position in line_starts {
        for assume in [false, true] {
            let text = answer(file, position, assume);
            row(&format!("L|{position}|{}|{text}", u8::from(assume)));
        }
    }
    for &position in positions.iter().step_by(8) {
        let text = answer(file, position, false);
        row(&format!("I|{position}|{text}"));
    }
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes.iter().fold(String::new(), |mut out, byte| {
        let _ = write!(out, "{byte:02x}");
        out
    })
}

fn edits(list: Result<Vec<ts_core::TextChange>, Error>) -> String {
    match list {
        Ok(list) => list
            .iter()
            .map(|edit| {
                format!(
                    "{},{},{}",
                    edit.range.pos(),
                    edit.range.end(),
                    hex(&edit.new_text)
                )
            })
            .collect::<Vec<_>>()
            .join(";"),
        Err(error) => format!("!{error}"),
    }
}

const MAX_ENTER_LINES: usize = 64;
const MAX_TRIGGERS: usize = 24;

/// The entry points other than `format_document`: on Enter at the first line
/// starts, a selection between sampled positions, and the three
/// typed-character entry points after the first occurrences of their character.
pub fn entry(
    file: &mut FormatFile<'_, '_>,
    context: &crate::FormatContext,
    positions: &[i64],
    row: &mut dyn FnMut(&str),
) {
    let (line_starts, text): (Vec<i64>, Vec<u8>) = match file.view.source_file(file.source) {
        Ok(state) => (
            state
                .ecma_line_map()
                .iter()
                .map(|&p| i64::from(p))
                .collect(),
            state.text().as_bytes().to_vec(),
        ),
        Err(error) => {
            row(&format!("N|!{error:?}"));
            return;
        }
    };
    for &position in line_starts.iter().take(MAX_ENTER_LINES) {
        let answer = edits(crate::format_on_enter(file, context, position));
        row(&format!("N|{position}|{answer}"));
    }
    for index in (0..positions.len()).step_by(16) {
        let start = positions[index];
        let end = positions
            .get(index + 16)
            .copied()
            .unwrap_or(text.len() as i64);
        let answer = edits(crate::format_selection(file, context, start, end));
        row(&format!("S|{start}|{end}|{answer}"));
    }
    type Run = fn(
        &mut FormatFile<'_, '_>,
        &crate::FormatContext,
        i64,
    ) -> Result<Vec<ts_core::TextChange>, Error>;
    let triggers: [(u8, &str, Run); 3] = [
        (b';', "M", crate::format_on_semicolon),
        (b'{', "O", crate::format_on_opening_curly),
        (b'}', "C", crate::format_on_closing_curly),
    ];
    for (character, name, run) in triggers {
        let found = text
            .iter()
            .enumerate()
            .filter(|&(_, &byte)| byte == character)
            .take(MAX_TRIGGERS);
        for (index, _) in found {
            let position = index as i64 + 1;
            let answer = edits(run(file, context, position));
            row(&format!("{name}|{position}|{answer}"));
        }
    }
}
