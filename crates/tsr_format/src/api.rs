//! The formatter's entry points (`format/api.go`).

use crate::context::FormatRequestKind;
use crate::indent::get_indentation_for_node;
use crate::scanner::FormattingScanner;
use crate::settings::FormatCodeSettings;
use crate::span::{
    find_enclosing_node, get_own_or_inherited_delta, get_scan_start_position, ErrorRanges,
    FormatSpanWorker,
};
use crate::util::{
    find_immediately_preceding_token_of_kind, find_outermost_node_within_list_level,
    get_line_start_position_for_position,
};
use crate::{Error, FormatFile};
use tsr_arena::NodeId;
use tsr_ast::SyntaxKind as K;
use tsr_core::{TextChange, TextRange};
use tsr_jsstring::wtf8::decode_utf8;

/// What upstream carries in its `context.Context`: the settings and the host's
/// new line.
#[derive(Clone, Debug)]
pub struct FormatContext {
    pub settings: FormatCodeSettings,
    pub host_new_line: Vec<u8>,
}

impl FormatContext {
    // port: tsc/internal/format/api.go:WithFormatCodeSettings
    pub fn new(settings: FormatCodeSettings, host_new_line: &[u8]) -> Self {
        Self {
            settings,
            host_new_line: host_new_line.to_vec(),
        }
    }

    /// The new line of the settings, else the host's, else a line feed.
    // port: tsc/internal/format/api.go:GetNewLineOrDefaultFromContext
    pub fn new_line(&self) -> &[u8] {
        if !self.settings.editor.new_line_character.is_empty() {
            &self.settings.editor.new_line_character
        } else if !self.host_new_line.is_empty() {
            &self.host_new_line
        } else {
            b"\n"
        }
    }
}

// port: tsc/internal/format/api.go:FormatSpan
pub fn format_span(
    file: &mut FormatFile<'_, '_>,
    context: &FormatContext,
    span: TextRange,
    kind: FormatRequestKind,
) -> Result<Vec<TextChange>, Error> {
    // The smallest node that fully wraps the range, and its initial indentation.
    let enclosing_node = find_enclosing_node(file, span)?;
    let options = &context.settings;
    let scan_start = get_scan_start_position(file, enclosing_node, span)?;
    let initial_indentation = get_indentation_for_node(file, enclosing_node, Some(span), options)?;
    let delta = get_own_or_inherited_delta(file, enclosing_node, options)?;

    let view = file.view;
    let state = view.source_file(file.source)?;
    let errors: Vec<TextRange> = state.diagnostics().iter().map(|error| error.loc).collect();
    let scanner = FormattingScanner::new(
        state.text().as_bytes(),
        state.language_variant,
        scan_start,
        span.end(),
    );
    FormatSpanWorker::new(
        file,
        scanner,
        options.clone(),
        context.new_line().to_vec(),
        span,
        enclosing_node,
        initial_indentation,
        delta,
        kind,
        ErrorRanges::new(&errors, span),
    )
    .execute()
}

/// Formats one node as a selection. The node is assumed to have no errors.
// port: tsc/internal/format/api.go:FormatNodeGivenIndentation
pub fn format_node_given_indentation(
    file: &mut FormatFile<'_, '_>,
    context: &FormatContext,
    node: NodeId,
    language_variant: tsr_core::LanguageVariant,
    initial_indentation: i64,
    delta: i64,
) -> Result<Vec<TextChange>, Error> {
    let range = file.node(node)?.range();
    let view = file.view;
    let state = view.source_file(file.source)?;
    let scanner = FormattingScanner::new(
        state.text().as_bytes(),
        language_variant,
        range.pos(),
        range.end(),
    );
    FormatSpanWorker::new(
        file,
        scanner,
        context.settings.clone(),
        context.new_line().to_vec(),
        range,
        node,
        initial_indentation,
        delta,
        FormatRequestKind::FormatSelection,
        ErrorRanges::none(),
    )
    .execute()
}

// port: tsc/internal/format/api.go:formatNodeLines
fn format_node_lines(
    file: &mut FormatFile<'_, '_>,
    context: &FormatContext,
    node: Option<NodeId>,
    kind: FormatRequestKind,
) -> Result<Vec<TextChange>, Error> {
    let Some(node) = node else {
        return Ok(Vec::new());
    };
    let token_start = file.token_pos(node)?;
    let line_start = get_line_start_position_for_position(token_start, file)?;
    let span = TextRange::new(line_start, i64::from(file.node(node)?.end()));
    format_span(file, context, span, kind)
}

// port: tsc/internal/format/api.go:FormatDocument
pub fn format_document(
    file: &mut FormatFile<'_, '_>,
    context: &FormatContext,
) -> Result<Vec<TextChange>, Error> {
    let end = i64::from(file.node(file.source)?.end());
    format_span(
        file,
        context,
        TextRange::new(0, end),
        FormatRequestKind::FormatDocument,
    )
}

// port: tsc/internal/format/api.go:FormatSelection
pub fn format_selection(
    file: &mut FormatFile<'_, '_>,
    context: &FormatContext,
    start: i64,
    end: i64,
) -> Result<Vec<TextChange>, Error> {
    let line_start = get_line_start_position_for_position(start, file)?;
    format_span(
        file,
        context,
        TextRange::new(line_start, end),
        FormatRequestKind::FormatSelection,
    )
}

/// The span ends at the opening curly, because the brace matched to the one
/// just typed may be wrong until further edits: typing the opening curly of a
/// method body inside a class must not move the class's closing brace.
// port: tsc/internal/format/api.go:FormatOnOpeningCurly
pub fn format_on_opening_curly(
    file: &mut FormatFile<'_, '_>,
    context: &FormatContext,
    position: i64,
) -> Result<Vec<TextChange>, Error> {
    let Some(opening_curly) =
        find_immediately_preceding_token_of_kind(position, K::OpenBraceToken, file)?
    else {
        return Ok(Vec::new());
    };
    let curly_brace_range = file.node(opening_curly)?.parent();
    let Some(outermost_node) = outermost(file, curly_brace_range)? else {
        return Err(Error::Assertion(
            "runtime error: invalid memory address or nil pointer dereference".into(),
        ));
    };
    let token_start = file.token_pos(outermost_node)?;
    let line_start = get_line_start_position_for_position(token_start, file)?;
    format_span(
        file,
        context,
        TextRange::new(line_start, position),
        FormatRequestKind::FormatOnOpeningCurlyBrace,
    )
}

/// `findOutermostNodeWithinListLevel`, which upstream also calls with nil.
fn outermost(file: &FormatFile<'_, '_>, node: Option<NodeId>) -> Result<Option<NodeId>, Error> {
    node.map(|node| find_outermost_node_within_list_level(file, node))
        .transpose()
}

// port: tsc/internal/format/api.go:FormatOnClosingCurly
pub fn format_on_closing_curly(
    file: &mut FormatFile<'_, '_>,
    context: &FormatContext,
    position: i64,
) -> Result<Vec<TextChange>, Error> {
    let preceding_token =
        find_immediately_preceding_token_of_kind(position, K::CloseBraceToken, file)?;
    let node = outermost(file, preceding_token)?;
    format_node_lines(
        file,
        context,
        node,
        FormatRequestKind::FormatOnClosingCurlyBrace,
    )
}

// port: tsc/internal/format/api.go:FormatOnSemicolon
pub fn format_on_semicolon(
    file: &mut FormatFile<'_, '_>,
    context: &FormatContext,
    position: i64,
) -> Result<Vec<TextChange>, Error> {
    let semicolon = find_immediately_preceding_token_of_kind(position, K::SemicolonToken, file)?;
    let node = outermost(file, semicolon)?;
    format_node_lines(file, context, node, FormatRequestKind::FormatOnSemicolon)
}

// port: tsc/internal/format/api.go:FormatOnEnter
pub fn format_on_enter(
    file: &mut FormatFile<'_, '_>,
    context: &FormatContext,
    position: i64,
) -> Result<Vec<TextChange>, Error> {
    let line = file.line_of(position)?;
    if line == 0 {
        return Ok(Vec::new());
    }
    // The start of the previous line.
    let start_pos = file.line_start(line - 1)?;
    // After Enter the cursor is on a new line, which may hold only whitespace.
    // Formatting such a line would remove its indentation as trailing
    // whitespace, so the span ends at the later of the end of the previous line
    // and the last non-whitespace character of the current one.
    let end_of_format_span = {
        let state = file.view.source_file(file.source)?;
        let text = state.text().as_bytes();
        let rest = |at: i64| -> Result<&[u8], Error> {
            usize::try_from(at)
                .ok()
                .and_then(|at| text.get(at..))
                .ok_or_else(|| {
                    Error::Assertion(format!(
                        "runtime error: slice bounds out of range [{at}:{}]",
                        text.len()
                    ))
                })
        };
        let mut end = tsr_scanner::get_ecma_end_line_position(&state, line as isize);
        while end > start_pos {
            let (ch, size) = decode_utf8(rest(end)?);
            // On a multi-byte character, keep backing up.
            if size == 0 || tsr_scanner::is_white_space_single_line(ch) {
                end -= 1;
                continue;
            }
            break;
        }
        // A line break at the end of the span means the current line is not to
        // be touched, so it is left out. Where the line break is two characters
        // the one before it was handled above.
        let (ch, _) = decode_utf8(rest(end)?);
        if tsr_scanner::is_line_break(ch) {
            end -= 1;
        }
        end
    };

    // The end is exclusive.
    format_span(
        file,
        context,
        TextRange::new(start_pos, end_of_format_span + 1),
        FormatRequestKind::FormatOnEnter,
    )
}
