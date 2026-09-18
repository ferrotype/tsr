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
