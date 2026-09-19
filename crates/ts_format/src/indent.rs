//! The smart indenter (`format/indent.go`).

use crate::lsutil::position_belongs_to_node;
use crate::settings::{FormatCodeSettings, IndentStyle};
use crate::util::{get_line_start_position_for_position, range_is_on_one_line};
use crate::{debug_assert, Error, FormatFile};
use ts_arena::NodeId;
use ts_ast::{utilities, utilities_middle, NodeListId, SyntaxKind as K};
use ts_core::TextRange;
use ts_jsstring::wtf8::decode_utf8;
use ts_scanner::CommentRange;

/// Upstream's "no actual indentation" answer is `-1`; here it is `None`.
type Actual = Option<i64>;

fn kind_of(file: &FormatFile<'_, '_>, node: NodeId) -> Result<ts_ast::NodeKind, Error> {
    Ok(file.node(node)?.kind())
}

fn parent_kind(file: &FormatFile<'_, '_>, node: NodeId) -> Result<Option<ts_ast::NodeKind>, Error> {
    match file.node(node)?.parent() {
        Some(parent) => Ok(Some(kind_of(file, parent)?)),
        None => Ok(None),
    }
}

fn list_loc(file: &FormatFile<'_, '_>, list: NodeListId) -> Result<TextRange, Error> {
    Ok(file.view.list(list)?.loc())
}

fn list_nodes(file: &FormatFile<'_, '_>, list: NodeListId) -> Result<Vec<NodeId>, Error> {
    let nodes = file.view.list(list)?.nodes();
    Ok(file.view.node_slice(nodes)?.iter().flatten().collect())
}

fn contained_by(inner: TextRange, outer: TextRange) -> bool {
    inner.pos() >= outer.pos() && inner.end() <= outer.end()
}

// port: tsc/internal/format/indent.go:GetIndentationForNode
pub fn get_indentation_for_node(
    file: &mut FormatFile<'_, '_>,
    node: NodeId,
    ignore_actual_indentation_range: Option<TextRange>,
    options: &FormatCodeSettings,
) -> Result<i64, Error> {
    let (line, character) = get_start_line_and_character_for_node(file, node)?;
    get_indentation_for_node_worker(
        file,
        node,
        line,
        character,
        ignore_actual_indentation_range,
        0,
        false,
        options,
    )
}

/// The indentation expected at a position: `SmartIndenter.getIndentation`.
// port: tsc/internal/format/indent.go:GetIndentation
pub fn get_indentation(
    file: &mut FormatFile<'_, '_>,
    position: i64,
    options: &FormatCodeSettings,
    assume_new_line_before_close_brace: bool,
) -> Result<i64, Error> {
    let length = file.view.source_file(file.source)?.text().as_bytes().len() as i64;
    if position > length {
        return Ok(options.editor.base_indent_size); // past EOF
    }

    // With no indentation at all there is nothing to compute.
    if options.editor.indent_style == IndentStyle::None {
        return Ok(0);
    }

    let preceding_token = file
        .navigator()
        .find_preceding_token_ex(position, None, true)?;

    let enclosing_comment = get_range_of_enclosing_comment(file, position, preceding_token)?;
    if let Some(comment) = enclosing_comment {
        if comment.kind == K::MultiLineCommentTrivia {
            return get_comment_indent(file, position, options, comment);
        }
    }

    let Some(preceding_token) = preceding_token else {
        return Ok(options.editor.base_indent_size);
    };

    // No indentation inside string, regular expression and template literals.
    let preceding_kind = kind_of(file, preceding_token)?;
    if is_string_or_regular_expression_or_template_literal(preceding_kind) {
        let token_start = file.token_pos(preceding_token)?;
        if token_start <= position && position < i64::from(file.node(preceding_token)?.end()) {
            return Ok(0);
        }
    }

    let line_at_position = file.line_of(position)?;

    // Object literals indent like blocks: wherever the `{` sits, even in the
    // middle of a line, what follows treats it as the start of that line,
    // leading whitespace included.
    let current_token = file.navigator().get_token_at_position(position)?;
    let current_parent_kind = parent_kind(file, current_token)?;
    let is_object_literal = kind_of(file, current_token)? == K::OpenBraceToken
        && current_parent_kind.is_some_and(|kind| kind == K::ObjectLiteralExpression);
    if options.editor.indent_style == IndentStyle::Block || is_object_literal {
        return get_block_indent(file, position, options);
    }

    let preceding_parent = file.node(preceding_token)?.parent();
    if preceding_kind == K::CommaToken {
        if let Some(parent) = preceding_parent {
            if kind_of(file, parent)? != K::BinaryExpression {
                // A comma separating list items: derive the indentation from
                // the previous item.
                if let Some(actual) = get_actual_indentation_for_list_item_before_comma(
                    file,
                    preceding_token,
                    options,
                )? {
                    return Ok(actual);
                }
            }
        }
    }

    let container_list = get_list_by_position(file, position, preceding_parent)?;
    // Use the list position if the preceding token is before any list item.
    if let Some(list) = container_list {
        let token_range = file.node(preceding_token)?.range();
        if !contained_by(token_range, list_loc(file, list)?) {
            let use_the_same_base_indentation = current_parent_kind
                .is_some_and(|kind| kind == K::FunctionExpression || kind == K::ArrowFunction);
            let indent_size = if use_the_same_base_indentation {
                0
            } else {
                options.editor.indent_size
            };
            return Ok(
                match get_actual_indentation_for_list_start_line(file, Some(list), options)? {
                    Some(actual) => actual + indent_size,
                    None => indent_size,
                },
            );
        }
    }

    get_smart_indent(
        file,
        position,
        preceding_token,
        line_at_position,
        assume_new_line_before_close_brace,
        options,
    )
}

// port: tsc/internal/format/span.go:isStringOrRegularExpressionOrTemplateLiteral
pub(crate) fn is_string_or_regular_expression_or_template_literal(kind: ts_ast::NodeKind) -> bool {
    kind == K::StringLiteral
        || kind == K::RegularExpressionLiteral
        || utilities_middle::is_template_literal_kind(kind)
}

// port: tsc/internal/format/indent.go:getCommentIndent
fn get_comment_indent(
    file: &mut FormatFile<'_, '_>,
    position: i64,
    options: &FormatCodeSettings,
    comment: CommentRange,
) -> Result<i64, Error> {
    let previous_line = file.line_of(position)? - 1;
    let comment_start_line = file.line_of(comment.loc.pos())?;

    debug_assert(comment_start_line >= 0)?;

    if previous_line <= comment_start_line {
        let start = file.line_start(comment_start_line)?;
        return find_first_non_whitespace_column(file, start, position, options);
    }

    let start_position_of_line = file.line_start(previous_line)?;
    let (character, column) = find_first_non_whitespace_character_and_column(
        file,
        start_position_of_line,
        position,
        options,
    )?;

    if column == 0 {
        return Ok(column);
    }

    let state = file.view.source_file(file.source)?;
    let text = state.text().as_bytes();
    let index = start_position_of_line + character;
    let Some(&first) = usize::try_from(index)
        .ok()
        .and_then(|index| text.get(index))
    else {
        return Err(Error::Assertion(format!(
            "runtime error: index out of range [{index}] with length {}",
            text.len()
        )));
    };
    Ok(if first == b'*' { column - 1 } else { column })
}

// port: tsc/internal/format/indent.go:getRangeOfEnclosingComment
pub(crate) fn get_range_of_enclosing_comment(
    file: &mut FormatFile<'_, '_>,
    position: i64,
    preceding_token: Option<NodeId>,
) -> Result<Option<CommentRange>, Error> {
    let mut token_at_position = file.navigator().get_token_at_position(position)?;
    let jsdoc = utilities::find_ancestor(file.view, Some(token_at_position), |node| {
        node.kind() == K::JSDoc
    })?;
    if let Some(jsdoc) = jsdoc {
        // Upstream dereferences the parent without a check.
        token_at_position = file.node(jsdoc)?.parent().ok_or_else(|| {
            Error::Assertion(
                "runtime error: invalid memory address or nil pointer dereference".into(),
            )
        })?;
    }
    let token_start = file
        .navigator()
        .get_start_of_node(token_at_position, false)?;
    let (token_kind, token_pos, token_end) = {
        let read = file.node(token_at_position)?;
        (read.kind(), i64::from(read.pos()), i64::from(read.end()))
    };
    if token_start <= position && position < token_end {
        return Ok(None);
    }

    // Between two consecutive tokens every comment is either trailing on the
    // former or leading on the latter, never both.
    let preceding_end = match preceding_token {
        Some(token) => Some(i64::from(file.node(token)?.end())),
        None => None,
    };
    let state = file.view.source_file(file.source)?;
    let text = state.text().as_bytes();
    let length = text.len() as i64;
    let trailing = preceding_end
        .into_iter()
        .flat_map(|end| ts_scanner::get_trailing_comment_ranges(text, end));
    // port: tsc/internal/format/indent.go:getLeadingCommentRangesOfNode
    let leading = (token_kind != K::JsxText)
        .then(|| ts_scanner::get_leading_comment_ranges(text, token_pos))
        .into_iter()
        .flatten();
    for comment in trailing.chain(leading) {
        let (pos, end) = (comment.loc.pos(), comment.loc.end());
        if (pos < position && position < end)
            || (position == end
                && (comment.kind == K::SingleLineCommentTrivia || position == length))
        {
            return Ok(Some(comment));
        }
    }
    Ok(None)
}

// port: tsc/internal/format/indent.go:getBlockIndent
fn get_block_indent(
    file: &mut FormatFile<'_, '_>,
    position: i64,
    options: &FormatCodeSettings,
) -> Result<i64, Error> {
    // Move backwards to a line with a non-whitespace character, then find the
    // first non-whitespace character of that line.
    let mut current = position;
    {
        let state = file.view.source_file(file.source)?;
        let text = state.text().as_bytes();
        while current > 0 {
            let (ch, size) = decode_utf8(&text[current as usize..]);
            if !ts_scanner::is_white_space_like(ch) {
                break;
            }
            current -= size as i64;
        }
    }
    let line_start = get_line_start_position_for_position(current, file)?;
    find_first_non_whitespace_column(file, line_start, current, options)
}

// port: tsc/internal/format/indent.go:getActualIndentationForListItemBeforeComma
fn get_actual_indentation_for_list_item_before_comma(
    file: &mut FormatFile<'_, '_>,
    comma_token: NodeId,
    options: &FormatCodeSettings,
) -> Result<Actual, Error> {
    if file.node(comma_token)?.parent().is_none() {
        return Ok(None);
    }
    let Some(containing_list) = get_containing_list(file, comma_token)? else {
        return Ok(None);
    };
    let nodes = list_nodes(file, containing_list)?;
    match nodes.iter().position(|&node| node == comma_token) {
        Some(index) if index > 0 => {
            derive_actual_indentation_from_list(file, containing_list, index - 1, options)
        }
        _ => Ok(None),
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum NextTokenKind {
    Unknown,
    OpenBrace,
    CloseBrace,
}

// port: tsc/internal/format/indent.go:nextTokenIsCurlyBraceOnSameLineAsCursor
fn next_token_is_curly_brace_on_same_line_as_cursor(
    file: &mut FormatFile<'_, '_>,
    preceding_token: NodeId,
    current: NodeId,
    line_at_position: i64,
) -> Result<NextTokenKind, Error> {
    let Some(next_token) = file.navigator().find_next_token(preceding_token, current)? else {
        return Ok(NextTokenKind::Unknown);
    };
    let kind = kind_of(file, next_token)?;
    if kind == K::OpenBraceToken {
        // Open braces are always indented at the parent level.
        return Ok(NextTokenKind::OpenBrace);
    }
    if kind == K::CloseBraceToken {
        // Close braces are indented at the parent level when they share the
        // cursor's line.
        if line_at_position == get_start_line_for_node(file, next_token)? {
            return Ok(NextTokenKind::CloseBrace);
        }
    }
    Ok(NextTokenKind::Unknown)
}

// port: tsc/internal/format/indent.go:getSmartIndent
fn get_smart_indent(
    file: &mut FormatFile<'_, '_>,
    position: i64,
    preceding_token: NodeId,
    line_at_position: i64,
    assume_new_line_before_close_brace: bool,
    options: &FormatCodeSettings,
) -> Result<i64, Error> {
    // Starting from the preceding token, find a node that includes the position
    // and can contribute to indentation, and compute the indentation inside it.
    let mut previous: Option<NodeId> = None;
    let mut current = Some(preceding_token);

    while let Some(node) = current {
        if position_belongs_to_node(file, node, position)?
            && should_indent_child_node(file, options, node, previous, true, true)?
        {
            let (current_start_line, current_start_character) =
                get_start_line_and_character_for_node(file, node)?;
            let next = next_token_is_curly_brace_on_same_line_as_cursor(
                file,
                preceding_token,
                node,
                line_at_position,
            )?;
            let indentation_delta = if next == NextTokenKind::Unknown {
                if line_at_position == current_start_line {
                    0
                } else {
                    options.editor.indent_size
                }
            } else if assume_new_line_before_close_brace && next == NextTokenKind::CloseBrace {
                // A code fix is about to be inserted before the close brace.
                options.editor.indent_size
            } else {
                0
            };
            return get_indentation_for_node_worker(
                file,
                node,
                current_start_line,
                current_start_character,
                None,
                indentation_delta,
                true,
                options,
            );
        }

        // A list item takes its indentation from the list. Parent and child
        // sharing a line is not considered yet: in `function foo(a` followed by
        // a new line, `a` shares its parent's line but indentation is expected.
        if let Some(actual) = get_actual_indentation_for_list_item(file, node, options, true)? {
            return Ok(actual);
        }

        previous = Some(node);
        current = file.node(node)?.parent();
    }
    // No parent was found: the base indentation of the file.
    Ok(options.editor.base_indent_size)
}

// port: tsc/internal/format/indent.go:getIndentationForNodeWorker
#[allow(
    clippy::too_many_arguments,
    reason = "the upstream signature; the pairs of line and character travel together through the loop"
)]
fn get_indentation_for_node_worker(
    file: &mut FormatFile<'_, '_>,
    mut current: NodeId,
    mut current_start_line: i64,
    mut current_start_character: i64,
    ignore_actual_indentation_range: Option<TextRange>,
    mut indentation_delta: i64,
    is_next_child: bool,
    options: &FormatCodeSettings,
) -> Result<i64, Error> {
    let mut parent = file.node(current)?.parent();

    // Walk up the tree and collect indentation for parent-child pairs. None is
    // added when parent and child start on the same line, or when the parent is
    // an `if` and the child starts on the line of its `else`.
    while let Some(parent_node) = parent {
        let use_actual_indentation = match ignore_actual_indentation_range {
            Some(range) => {
                let start = file.token_pos(current)?;
                start < range.pos() || start > range.end()
            }
            None => true,
        };

        let (containing_list_or_parent_start_line, containing_list_or_parent_start_character) =
            get_containing_list_or_parent_start(file, parent_node, current)?;
        let parent_and_child_share_line = containing_list_or_parent_start_line
            == current_start_line
            || child_starts_on_the_same_line_with_else_in_if_statement(
                file,
                parent_node,
                current,
                current_start_line,
            )?;

        if use_actual_indentation {
            // A list indents its children when they begin on a later line than
            // the list itself. What decides is the relationship between the
            // list and its *first* item.
            let first_list_child = match get_containing_list(file, current)? {
                Some(list) => list_nodes(file, list)?.first().copied(),
                None => None,
            };
            let list_indents_child = match first_list_child {
                Some(first) => {
                    get_start_line_for_node(file, first)? > containing_list_or_parent_start_line
                }
                None => false,
            };
            if let Some(actual) =
                get_actual_indentation_for_list_item(file, current, options, list_indents_child)?
            {
                return Ok(actual + indentation_delta);
            }

            // Try the actual indentation of the node in the source text.
            if let Some(actual) = get_actual_indentation_for_node(
                file,
                current,
                parent_node,
                current_start_line,
                current_start_character,
                parent_and_child_share_line,
                options,
            )? {
                return Ok(actual + indentation_delta);
            }
        }

        // Indent when the parent wants its content indented and the two do not
        // start on the same line.
        if should_indent_child_node(
            file,
            options,
            parent_node,
            Some(current),
            true,
            is_next_child,
        )? && !parent_and_child_share_line
        {
            indentation_delta += options.editor.indent_size;
        }

        // A call argument's parent is the call expression, not the argument
        // list, so the call's start is spoofed to the list's. Comparing that
        // spoofed start with the call's own parent would add a level for an
        // IIFE, so at an argument the true start is restored after the
        // argument's indentation is applied.
        let use_true_start = is_argument_and_start_line_overlaps_expression_being_called(
            file,
            parent_node,
            current,
            current_start_line,
        )?;

        current = parent_node;
        parent = file.node(current)?.parent();

        if use_true_start {
            (current_start_line, current_start_character) =
                get_start_line_and_character_for_node(file, current)?;
        } else {
            current_start_line = containing_list_or_parent_start_line;
            current_start_character = containing_list_or_parent_start_character;
        }
    }

    Ok(indentation_delta + options.editor.base_indent_size)
}

/// `None` when the node's actual indentation should not be used, as for a
/// nested expression.
// port: tsc/internal/format/indent.go:getActualIndentationForNode
fn get_actual_indentation_for_node(
    file: &mut FormatFile<'_, '_>,
    current: NodeId,
    parent: NodeId,
    current_line: i64,
    current_character: i64,
    parent_and_child_share_line: bool,
    options: &FormatCodeSettings,
) -> Result<Actual, Error> {
    // Statements and declarations use their actual indentation when the parent
    // is the source file, whose children are not indented unless the user did
    // it, or when parent and child are on different lines.
    let read = file.node(current)?;
    let use_actual_indentation = (ts_ast::is_declaration(&read)
        || utilities::is_statement_but_not_declaration(&read))
        && (kind_of(file, parent)? == K::SourceFile || !parent_and_child_share_line);

    if !use_actual_indentation {
        return Ok(None);
    }

    find_column_for_first_non_whitespace_character_in_line(
        file,
        current_line,
        current_character,
        options,
    )
    .map(Some)
}

// port: tsc/internal/format/indent.go:isArgumentAndStartLineOverlapsExpressionBeingCalled
fn is_argument_and_start_line_overlaps_expression_being_called(
    file: &mut FormatFile<'_, '_>,
    parent: NodeId,
    child: NodeId,
    child_start_line: i64,
) -> Result<bool, Error> {
    let read = file.node(parent)?;
    if read.kind() != K::CallExpression || !arguments(file, parent)?.contains(&child) {
        return Ok(false);
    }
    let expression = read.expression().ok_or(ts_arena::Error::InvalidGraph)?;
    let expression_end = i64::from(file.node(expression)?.end());
    Ok(file.line_of(expression_end)? == child_start_line)
}

/// `node.Arguments()`: empty when the call has no argument list.
fn arguments(file: &FormatFile<'_, '_>, node: NodeId) -> Result<Vec<NodeId>, Error> {
    match file.node(node)?.argument_list() {
        Some(list) => list_nodes(file, list),
        None => Ok(Vec::new()),
    }
}

// port: tsc/internal/format/indent.go:getActualIndentationForListItem
fn get_actual_indentation_for_list_item(
    file: &mut FormatFile<'_, '_>,
    node: NodeId,
    options: &FormatCodeSettings,
    list_indents_child: bool,
) -> Result<Actual, Error> {
    if parent_kind(file, node)?.is_some_and(|kind| kind == K::VariableDeclarationList) {
        // A variable declaration list has no wrapping tokens.
        return Ok(None);
    }
    let Some(containing_list) = get_containing_list(file, node)? else {
        return Ok(None);
    };
    let nodes = list_nodes(file, containing_list)?;
    if let Some(index) = nodes.iter().position(|&element| element == node) {
        if let Some(result) =
            derive_actual_indentation_from_list(file, containing_list, index, options)?
        {
            return Ok(Some(result));
        }
    }
    let delta = if list_indents_child {
        options.editor.indent_size
    } else {
        0
    };
    Ok(Some(
        match get_actual_indentation_for_list_start_line(file, Some(containing_list), options)? {
            Some(actual) => actual + delta,
            None => delta,
        },
    ))
}

// port: tsc/internal/format/indent.go:getActualIndentationForListStartLine
fn get_actual_indentation_for_list_start_line(
    file: &mut FormatFile<'_, '_>,
    list: Option<NodeListId>,
    options: &FormatCodeSettings,
) -> Result<Actual, Error> {
    let Some(list) = list else {
        return Ok(None);
    };
    let (line, character) = file.line_and_offset(list_loc(file, list)?.pos())?;
    find_column_for_first_non_whitespace_character_in_line(file, line, character, options).map(Some)
}

// port: tsc/internal/format/indent.go:deriveActualIndentationFromList
fn derive_actual_indentation_from_list(
    file: &mut FormatFile<'_, '_>,
    list: NodeListId,
    index: usize,
    options: &FormatCodeSettings,
) -> Result<Actual, Error> {
    let nodes = list_nodes(file, list)?;
    debug_assert(index < nodes.len())?;

    // Walk toward the start of the list while the items share one line. Where
    // item [i - 1] ends on another line than item [i] starts, the answer is the
    // column of the first non-whitespace character on item [i]'s line.
    let (mut line, mut character) = get_start_line_and_character_for_node(file, nodes[index])?;

    for &item in nodes[..=index].iter().rev() {
        if kind_of(file, item)? == K::CommaToken {
            continue;
        }
        // Skip items that end on the line of the current element.
        let previous_end_line = file.line_of(i64::from(file.node(item)?.end()))?;
        if previous_end_line != line {
            return find_column_for_first_non_whitespace_character_in_line(
                file, line, character, options,
            )
            .map(Some);
        }
        (line, character) = get_start_line_and_character_for_node(file, item)?;
    }
    Ok(None)
}

// port: tsc/internal/format/indent.go:findColumnForFirstNonWhitespaceCharacterInLine
fn find_column_for_first_non_whitespace_character_in_line(
    file: &mut FormatFile<'_, '_>,
    line: i64,
    character: i64,
    options: &FormatCodeSettings,
) -> Result<i64, Error> {
    let line_start = file.line_start(line)?;
    find_first_non_whitespace_column(file, line_start, line_start + character, options)
}

// port: tsc/internal/format/indent.go:FindFirstNonWhitespaceColumn
pub fn find_first_non_whitespace_column(
    file: &FormatFile<'_, '_>,
    start: i64,
    end: i64,
    options: &FormatCodeSettings,
) -> Result<i64, Error> {
    Ok(find_first_non_whitespace_character_and_column(file, start, end, options)?.1)
}

/// The character is the byte index from the start of the line. The column is
/// the position after expanding tabs: in `"0\t2$"` with a tab size of 4 the `$`
/// has character 3.
///
/// A tab advances the column by `tab_size + column % tab_size`, as upstream
/// writes it, rather than to the next tab stop.
// port: tsc/internal/format/indent.go:findFirstNonWhitespaceCharacterAndColumn
pub(crate) fn find_first_non_whitespace_character_and_column(
    file: &FormatFile<'_, '_>,
    start: i64,
    end: i64,
    options: &FormatCodeSettings,
) -> Result<(i64, i64), Error> {
    let state = file.view.source_file(file.source)?;
    let text = state.text().as_bytes();
    let tab_size = options.editor.tab_size;
    let mut column = 0;
    let mut position = start;
    while position < end {
        let Some(rest) = usize::try_from(position).ok().and_then(|at| text.get(at..)) else {
            return Err(Error::Assertion(format!(
                "runtime error: slice bounds out of range [{position}:{}]",
                text.len()
            )));
        };
        let (ch, size) = decode_utf8(rest);
        if !ts_scanner::is_white_space_single_line(ch) {
            break;
        }
        if ch == i32::from(b'\t') {
            if tab_size > 0 {
                column += tab_size + (column % tab_size);
            }
        } else {
            column += 1;
        }
        position += size as i64;
    }
    Ok((position - start, column))
}

// port: tsc/internal/format/indent.go:childStartsOnTheSameLineWithElseInIfStatement
pub(crate) fn child_starts_on_the_same_line_with_else_in_if_statement(
    file: &mut FormatFile<'_, '_>,
    parent: NodeId,
    child: NodeId,
    child_start_line: i64,
) -> Result<bool, Error> {
    let read = file.node(parent)?;
    let is_else = read
        .data_source()
        .as_if_statement()
        .is_some_and(|statement| statement.else_statement() == Some(child));
    if !is_else {
        return Ok(false);
    }
    let child_pos = i64::from(file.node(child)?.pos());
    let else_keyword = file.navigator().find_preceding_token(child_pos)?;
    debug_assert(else_keyword.is_some())?;
    let Some(else_keyword) = else_keyword else {
        return Ok(false);
    };
    Ok(get_start_line_for_node(file, else_keyword)? == child_start_line)
}

// port: tsc/internal/format/indent.go:getStartLineAndCharacterForNode
pub(crate) fn get_start_line_and_character_for_node(
    file: &mut FormatFile<'_, '_>,
    node: NodeId,
) -> Result<(i64, i64), Error> {
    let start = file.token_pos(node)?;
    file.line_and_offset(start)
}

// port: tsc/internal/format/indent.go:getStartLineForNode
fn get_start_line_for_node(file: &mut FormatFile<'_, '_>, node: NodeId) -> Result<i64, Error> {
    let start = file.token_pos(node)?;
    file.line_of(start)
}

// port: tsc/internal/format/indent.go:GetContainingList
pub fn get_containing_list(
    file: &mut FormatFile<'_, '_>,
    node: NodeId,
) -> Result<Option<NodeListId>, Error> {
    let read = file.node(node)?;
    let Some(parent) = read.parent() else {
        return Ok(None);
    };
    let start = file.token_pos(node)?;
    get_list_by_range(file, start, i64::from(read.end()), parent)
}

// port: tsc/internal/format/indent.go:getListByPosition
fn get_list_by_position(
    file: &mut FormatFile<'_, '_>,
    position: i64,
    node: Option<NodeId>,
) -> Result<Option<NodeListId>, Error> {
    match node {
        Some(node) => get_list_by_range(file, position, position, node),
        None => Ok(None),
    }
}

// port: tsc/internal/format/indent.go:getListByRange
fn get_list_by_range(
    file: &mut FormatFile<'_, '_>,
    start: i64,
    end: i64,
    node: NodeId,
) -> Result<Option<NodeListId>, Error> {
    let range = TextRange::new(start, end);
    let read = file.node(node)?;
    let candidates: [Option<NodeListId>; 2] = match read.kind().known() {
        Some(K::TypeReference) => [read.type_argument_list(), None],
        Some(K::ObjectLiteralExpression) => [read.property_list(), None],
        Some(K::TypeLiteral) => [read.member_list(), None],
        Some(
            K::FunctionDeclaration
            | K::FunctionExpression
            | K::ArrowFunction
            | K::MethodDeclaration
            | K::MethodSignature
            | K::CallSignature
            | K::Constructor
            | K::ConstructorType
            | K::ConstructSignature,
        ) => [read.type_parameter_list(), read.parameter_list()],
        Some(K::GetAccessor) => [read.parameter_list(), None],
        Some(
            K::ClassDeclaration
            | K::ClassExpression
            | K::InterfaceDeclaration
            | K::TypeAliasDeclaration
            | K::JSDocTemplateTag,
        ) => [read.type_parameter_list(), None],
        Some(K::NewExpression | K::CallExpression) => {
            [read.type_argument_list(), read.argument_list()]
        }
        Some(K::VariableDeclarationList) => [
            read.data_source()
                .as_variable_declaration_list()
                .and_then(|list| list.declarations()),
            None,
        ],
        Some(
            K::ArrayLiteralExpression
            | K::ObjectBindingPattern
            | K::ArrayBindingPattern
            | K::NamedImports
            | K::NamedExports,
        ) => [read.element_list(), None],
        // Upstream wonders whether this should panic; Strada does not.
        _ => [None, None],
    };
    for list in candidates.into_iter().flatten() {
        if get_list(file, list, range, node)? {
            return Ok(Some(list));
        }
    }
    Ok(None)
}

// port: tsc/internal/format/indent.go:getList
fn get_list(
    file: &mut FormatFile<'_, '_>,
    list: NodeListId,
    range: TextRange,
    node: NodeId,
) -> Result<bool, Error> {
    let loc = list_loc(file, list)?;
    Ok(contained_by(range, get_visual_list_range(file, node, loc)?))
}

/// The list's range widened over the trivia around it: from the end of the
/// token before it to the start of the token after it. Strada got this from the
/// services' child list; upstream and this port use the scanner.
// port: tsc/internal/format/indent.go:getVisualListRange
fn get_visual_list_range(
    file: &mut FormatFile<'_, '_>,
    _node: NodeId,
    list: TextRange,
) -> Result<TextRange, Error> {
    let prior_end = match file.navigator().find_preceding_token(list.pos())? {
        Some(prior) => i64::from(file.node(prior)?.end()),
        None => list.pos(),
    };
    let state = file.view.source_file(file.source)?;
    let scanner = ts_scanner::get_scanner_for_source_file(&state, list.end());
    let next_start = if scanner.token() == K::EndOfFile {
        list.end()
    } else {
        scanner.token_start()
    };
    Ok(TextRange::new(prior_end, next_start))
}

// port: tsc/internal/format/indent.go:getContainingListOrParentStart
fn get_containing_list_or_parent_start(
    file: &mut FormatFile<'_, '_>,
    parent: NodeId,
    child: NodeId,
) -> Result<(i64, i64), Error> {
    let start = match get_containing_list(file, child)? {
        Some(list) => list_loc(file, list)?.pos(),
        None => file.token_pos(parent)?,
    };
    file.line_and_offset(start)
}

// port: tsc/internal/format/indent.go:isControlFlowEndingStatement
fn is_control_flow_ending_statement(kind: ts_ast::NodeKind, parent: ts_ast::NodeKind) -> bool {
    matches!(
        kind.known(),
        Some(K::ReturnStatement | K::ThrowStatement | K::ContinueStatement | K::BreakStatement)
    ) && parent != K::Block
}

/// Whether the parent indents the child by an explicit rule. With
/// `is_next_child` the question is about a hypothetical child *after* this one.
/// `with_source` is false where upstream passes no source file.
// port: tsc/internal/format/indent.go:ShouldIndentChildNode
pub fn should_indent_child_node(
    file: &FormatFile<'_, '_>,
    settings: &FormatCodeSettings,
    parent: NodeId,
    child: Option<NodeId>,
    with_source: bool,
    is_next_child: bool,
) -> Result<bool, Error> {
    if !node_will_indent_child(file, settings, parent, child, with_source, false)? {
        return Ok(false);
    }
    let Some(child) = child else {
        return Ok(true);
    };
    Ok(!(is_next_child
        && is_control_flow_ending_statement(kind_of(file, child)?, kind_of(file, parent)?)))
}

// port: tsc/internal/format/indent.go:NodeWillIndentChild
pub fn node_will_indent_child(
    file: &FormatFile<'_, '_>,
    settings: &FormatCodeSettings,
    parent: NodeId,
    child: Option<NodeId>,
    with_source: bool,
    indent_by_default: bool,
) -> Result<bool, Error> {
    let child_kind = match child {
        Some(child) => kind_of(file, child)?.known(),
        None => Some(K::Unknown),
    };
    let parent_read = file.node(parent)?;
    let parent_kind = parent_read.kind();

    Ok(match parent_kind.known() {
        Some(
            K::ExpressionStatement
            | K::ClassDeclaration
            | K::ClassExpression
            | K::InterfaceDeclaration
            | K::EnumDeclaration
            | K::TypeAliasDeclaration
            | K::ArrayLiteralExpression
            | K::Block
            | K::ModuleBlock
            | K::ObjectLiteralExpression
            | K::TypeLiteral
            | K::MappedType
            | K::TupleType
            | K::ParenthesizedExpression
            | K::PropertyAccessExpression
            | K::CallExpression
            | K::NewExpression
            | K::VariableStatement
            | K::ExportAssignment
            | K::ReturnStatement
            | K::ConditionalExpression
            | K::ArrayBindingPattern
            | K::ObjectBindingPattern
            | K::JsxOpeningElement
            | K::JsxOpeningFragment
            | K::JsxSelfClosingElement
            | K::JsxExpression
            | K::MethodSignature
            | K::CallSignature
            | K::ConstructSignature
            | K::Parameter
            | K::FunctionType
            | K::ConstructorType
            | K::ParenthesizedType
            | K::TaggedTemplateExpression
            | K::AwaitExpression
            | K::NamedExports
            | K::NamedImports
            | K::ExportSpecifier
            | K::ImportSpecifier
            | K::PropertyDeclaration
            | K::CaseClause
            | K::DefaultClause,
        ) => true,
        Some(K::CaseBlock) => settings.indent_switch_case.is_true_or_unknown(),
        Some(K::VariableDeclaration | K::PropertyAssignment | K::BinaryExpression) => {
            if let Some(child) = child {
                if settings
                    .indent_multi_line_object_literal_beginning_on_blank_line
                    .is_false_or_unknown()
                    && with_source
                    && child_kind == Some(K::ObjectLiteralExpression)
                {
                    return range_is_on_one_line(file.node(child)?.range(), file);
                }
                if parent_kind == K::BinaryExpression
                    && with_source
                    && child_kind == Some(K::JsxElement)
                {
                    let state = file.view.source_file(file.source)?;
                    let text = state.text().as_bytes();
                    let parent_start = ts_scanner::skip_trivia(text, i64::from(parent_read.pos()));
                    let child_start =
                        ts_scanner::skip_trivia(text, i64::from(file.node(child)?.pos()));
                    return Ok(file.line_of(parent_start)? != file.line_of(child_start)?);
                }
            }
            parent_kind != K::BinaryExpression || indent_by_default
        }
        Some(
            K::DoStatement
            | K::WhileStatement
            | K::ForInStatement
            | K::ForOfStatement
            | K::ForStatement
            | K::IfStatement
            | K::FunctionDeclaration
            | K::FunctionExpression
            | K::MethodDeclaration
            | K::Constructor
            | K::GetAccessor
            | K::SetAccessor,
        ) => child_kind != Some(K::Block),
        Some(K::ArrowFunction) => {
            if let Some(child) = child {
                if with_source && child_kind == Some(K::ParenthesizedExpression) {
                    return range_is_on_one_line(file.node(child)?.range(), file);
                }
            }
            child_kind != Some(K::Block)
        }
        Some(K::ExportDeclaration) => child_kind != Some(K::NamedExports),
        Some(K::ImportDeclaration) => {
            if child_kind != Some(K::ImportClause) {
                return Ok(true);
            }
            let bindings = match child {
                Some(child) => file
                    .node(child)?
                    .data_source()
                    .as_import_clause()
                    .and_then(|clause| clause.named_bindings()),
                None => None,
            };
            match bindings {
                Some(bindings) => kind_of(file, bindings)? != K::NamedImports,
                None => false,
            }
        }
        Some(K::JsxElement) => child_kind != Some(K::JsxClosingElement),
        Some(K::JsxFragment) => child_kind != Some(K::JsxClosingFragment),
        Some(K::IntersectionType | K::UnionType | K::SatisfiesExpression) => {
            !matches!(
                child_kind,
                Some(K::TypeLiteral | K::TupleType | K::MappedType)
            ) && indent_by_default
        }
        Some(K::TryStatement) => child_kind != Some(K::Block) && indent_by_default,
        // No explicit rule: the default decides.
        _ => indent_by_default,
    })
}

/// A multi-line conditional indents its `whenTrue` and `whenFalse` branches,
/// unless the branches themselves span lines and apply their own indentation,
/// as an object literal opened on the condition's line does. That case shows as
/// `whenTrue` starting on the line the condition ends, and `whenFalse` starting
/// on the line `whenTrue` ends; the extra level must then be dropped.
// port: tsc/internal/format/indent.go:childIsUnindentedBranchOfConditionalExpression
pub(crate) fn child_is_unindented_branch_of_conditional_expression(
    file: &mut FormatFile<'_, '_>,
    parent: NodeId,
    child: NodeId,
    child_start_line: i64,
) -> Result<bool, Error> {
    let read = file.node(parent)?;
    let data = read.data_source();
    let Some(conditional) = data.as_conditional_expression() else {
        return Ok(false);
    };
    let (when_true, when_false) = (conditional.when_true(), conditional.when_false());
    if when_true != Some(child) && when_false != Some(child) {
        return Ok(false);
    }
    let condition = conditional
        .condition()
        .ok_or(ts_arena::Error::InvalidGraph)?;
    let condition_end_line = file.line_of(i64::from(file.node(condition)?.end()))?;
    if when_true == Some(child) {
        return Ok(child_start_line == condition_end_line);
    }
    // On the `whenFalse` side the `whenTrue` side decides: if that one was
    // indented, `whenFalse` must be too.
    let when_true = when_true.ok_or(ts_arena::Error::InvalidGraph)?;
    let true_start_line = get_start_line_for_node(file, when_true)?;
    let true_end_line = file.line_of(i64::from(file.node(when_true)?.end()))?;
    Ok(condition_end_line == true_start_line && true_end_line == child_start_line)
}

// port: tsc/internal/format/indent.go:argumentStartsOnSameLineAsPreviousArgument
pub(crate) fn argument_starts_on_same_line_as_previous_argument(
    file: &mut FormatFile<'_, '_>,
    parent: NodeId,
    child: NodeId,
    child_start_line: i64,
) -> Result<bool, Error> {
    let kind = kind_of(file, parent)?;
    if kind != K::CallExpression && kind != K::NewExpression {
        return Ok(false);
    }
    let arguments = arguments(file, parent)?;
    // Not an argument, or the first one: there is no previous node to look at.
    let Some(index) = arguments.iter().position(|&node| node == child) else {
        return Ok(false);
    };
    if index == 0 {
        return Ok(false);
    }
    let previous_end = i64::from(file.node(arguments[index - 1])?.end());
    Ok(child_start_line == file.line_of(previous_end)?)
}
