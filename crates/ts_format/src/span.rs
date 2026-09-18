//! The span worker (`format/span.go`): walks the enclosing node and the token
//! stream together, asks the rules about every adjacent pair of tokens, and
//! records the whitespace and indentation edits.

use crate::context::{FormatRequestKind, FormattingContext};
use crate::indent::{
    argument_starts_on_same_line_as_previous_argument,
    child_is_unindented_branch_of_conditional_expression,
    child_starts_on_the_same_line_with_else_in_if_statement,
    find_first_non_whitespace_character_and_column, find_first_non_whitespace_column,
    is_string_or_regular_expression_or_template_literal, node_will_indent_child,
    should_indent_child_node,
};
use crate::rule::{action, Rule, RuleFlags};
use crate::scanner::{FormattingScanner, TextRangeWithKind, TokenInfo};
use crate::settings::FormatCodeSettings;
use crate::util::{
    get_close_token_for_open_token, get_line_start_position_for_position, get_open_token_for_list,
    is_member_list_element,
};
use crate::{debug_assert, Error, FormatFile};
use std::ops::ControlFlow;
use ts_arena::NodeId;
use ts_ast::{node_flags, utilities, utilities_middle, ChildVisitor, NodeListId, SyntaxKind as K};
use ts_astnav::Visit;
use ts_core::{TextChange, TextRange};
use ts_jsstring::wtf8::decode_utf8;

/// Upstream's zero `TextRangeWithKind`, which stands for "no range yet".
fn no_range() -> TextRangeWithKind {
    TextRangeWithKind::new(0, 0, K::Unknown)
}

fn overlaps(left: TextRange, right: TextRange) -> bool {
    left.pos().max(right.pos()) < left.end().min(right.end())
}

fn contained_by(inner: TextRange, outer: TextRange) -> bool {
    inner.pos() >= outer.pos() && inner.end() <= outer.end()
}

fn is_comment(kind: K) -> bool {
    kind == K::SingleLineCommentTrivia || kind == K::MultiLineCommentTrivia
}

fn out_of_range(index: i64, length: usize) -> Error {
    Error::Assertion(if index < 0 {
        format!("runtime error: index out of range [{index}]")
    } else {
        format!("runtime error: index out of range [{index}] with length {length}")
    })
}

// port: tsc/internal/format/context.go:withTokenStart
pub(crate) fn with_token_start(
    file: &mut FormatFile<'_, '_>,
    node: NodeId,
) -> Result<TextRange, Error> {
    let start = file.token_pos(node)?;
    Ok(TextRange::new(start, i64::from(file.node(node)?.end())))
}

/// Every child `ForEachChild` reaches, with lists flattened.
struct Children<'v> {
    view: ts_ast::AstView<'v>,
    out: Vec<NodeId>,
    error: Option<ts_arena::Error>,
}

impl ChildVisitor for Children<'_> {
    fn visit_node(&mut self, node: NodeId) -> ControlFlow<()> {
        self.out.push(node);
        ControlFlow::Continue(())
    }
    fn visit_list(&mut self, nodes: NodeListId) -> ControlFlow<()> {
        match self.view.list(nodes) {
            Ok(list) => self.visit_node_slice(list.nodes()),
            Err(error) => {
                self.error = Some(error);
                ControlFlow::Break(())
            }
        }
    }
    fn visit_node_slice(&mut self, nodes: ts_ast::NodeSlice) -> ControlFlow<()> {
        match self.view.node_slice(nodes) {
            Ok(read) => {
                self.out.extend(read.iter().flatten());
                ControlFlow::Continue(())
            }
            Err(error) => {
                self.error = Some(error);
                ControlFlow::Break(())
            }
        }
    }
}

/// The smallest node that fully contains the range.
// port: tsc/internal/format/span.go:findEnclosingNode
pub(crate) fn find_enclosing_node(
    file: &mut FormatFile<'_, '_>,
    range: TextRange,
) -> Result<NodeId, Error> {
    let mut node = file.source;
    loop {
        let mut children = Children {
            view: file.view,
            out: Vec::new(),
            error: None,
        };
        let _ = file.node(node)?.for_each_child(&mut children);
        if let Some(error) = children.error {
            return Err(error.into());
        }
        let mut candidate = None;
        for child in children.out {
            if file.node(child)?.flags() & node_flags::REPARSED != 0 {
                continue;
            }
            if contained_by(range, with_token_start(file, child)?) {
                candidate = Some(child);
                break;
            }
        }
        match candidate {
            Some(child) => node = child,
            None => return Ok(node),
        }
    }
}

/// The start of the range may fall inside a comment, where the scanner would
/// not give useful results. Scanning starts at the end of the token before the
/// range instead.
// port: tsc/internal/format/span.go:getScanStartPosition
pub(crate) fn get_scan_start_position(
    file: &mut FormatFile<'_, '_>,
    enclosing_node: NodeId,
    original_range: TextRange,
) -> Result<i64, Error> {
    let start = with_token_start(file, enclosing_node)?.pos();
    let (node_pos, node_end) = {
        let read = file.node(enclosing_node)?;
        (i64::from(read.pos()), i64::from(read.end()))
    };
    if start == original_range.pos() && node_end == original_range.end() {
        return Ok(start);
    }

    // JSDoc is excluded so the scan never starts inside a JSDoc comment.
    let Some(preceding_token) =
        file.navigator()
            .find_preceding_token_ex(original_range.pos(), None, true)?
    else {
        // No preceding token: start from the beginning of the enclosing node.
        return Ok(node_pos);
    };

    // The preceding token ends after the start of the range, as when the range
    // starts in the middle of a literal: start from the enclosing node to
    // handle the whole range.
    let preceding_end = i64::from(file.node(preceding_token)?.end());
    if preceding_end >= original_range.pos() {
        return Ok(node_pos);
    }
    Ok(preceding_end)
}

/// In
///
/// ```text
/// if (a ||
///     b ||$
///     c) {...}
/// ```
///
/// pressing Enter at `$` formats the last two lines. The node enclosing them is
/// the binary expression, whose initial indentation is 0. A binary expression
/// opens no indentation scope, but a parent on the same line may, as the `if`
/// does here. Only parents on the node's own line count: one on another line
/// already contributed its delta to the initial indentation.
// port: tsc/internal/format/span.go:getOwnOrInheritedDelta
pub(crate) fn get_own_or_inherited_delta(
    file: &mut FormatFile<'_, '_>,
    node: NodeId,
    options: &FormatCodeSettings,
) -> Result<i64, Error> {
    let mut previous_line = -1;
    let mut child: Option<NodeId> = None;
    let mut current = Some(node);
    while let Some(n) = current {
        let start = with_token_start(file, n)?.pos();
        let line = file.line_of(start)?;
        if previous_line != -1 && line != previous_line {
            break;
        }
        if should_indent_child_node(file, options, n, child, true, false)? {
            return Ok(options.editor.indent_size);
        }
        previous_line = line;
        child = Some(n);
        current = file.node(n)?.parent();
    }
    Ok(0)
}

/// Whether a range contains a parse error. The ranges asked about increase
/// monotonically, so the index of the last error checked is kept.
// port: tsc/internal/format/span.go:prepareRangeContainsErrorFunction
pub(crate) struct ErrorRanges {
    sorted: Vec<TextRange>,
    index: usize,
}

impl ErrorRanges {
    // port: tsc/internal/format/span.go:rangeHasNoErrors
    pub(crate) fn none() -> Self {
        Self {
            sorted: Vec::new(),
            index: 0,
        }
    }

    pub(crate) fn new(errors: &[TextRange], original_range: TextRange) -> Self {
        // Only the errors that fall in the range.
        let mut sorted: Vec<TextRange> = errors
            .iter()
            .copied()
            .filter(|&error| overlaps(original_range, error))
            .collect();
        sorted.sort_by_key(|error| error.pos());
        Self { sorted, index: 0 }
    }

    fn contains(&mut self, range: TextRange) -> bool {
        loop {
            // Every error in the range was already checked.
            let Some(&error) = self.sorted.get(self.index) else {
                return false;
            };
            // The range ends before the error the index refers to.
            if range.end() <= error.pos() {
                return false;
            }
            if overlaps(range, error) {
                return true;
            }
            self.index += 1;
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LineAction {
    None,
    LineAdded,
    LineRemoved,
}

/// What `indentTriviaItems` does with a single-line comment it indents.
#[derive(Clone, Copy)]
enum SingleLine {
    /// Trivia left over at the end of the range: it is processed as a range
    /// first, since no token carried it.
    Remaining,
    /// Trivia leading a token.
    Leading,
}

type IndenterId = usize;

struct DynamicIndenter {
    node: NodeId,
    node_start_line: i64,
    indentation: i64,
    delta: i64,
}

pub(crate) struct FormatSpanWorker<'a, 'p, 'f, 's> {
    original_range: TextRange,
    enclosing_node: NodeId,
    initial_indentation: i64,
    delta: i64,
    range_contains_error: ErrorRanges,
    new_line: Vec<u8>,

    scanner: FormattingScanner<'s>,
    context: FormattingContext<'a, 'p, 'f>,

    edits: Vec<TextChange>,
    previous_range: TextRangeWithKind,
    previous_range_trivia_end: i64,
    previous_parent: Option<NodeId>,
    previous_range_start_line: i64,

    child_context_node: Option<NodeId>,
    last_indented_line: i64,
    indentation_on_last_indented_line: i64,

    /// Upstream shares indenters by pointer and mutates them in place.
    indenters: Vec<DynamicIndenter>,
}

impl<'a, 'p, 'f, 's> FormatSpanWorker<'a, 'p, 'f, 's> {
    // port: tsc/internal/format/span.go:newFormatSpanWorker
    #[allow(
        clippy::too_many_arguments,
        reason = "the upstream constructor; every argument is one independent fact of the request"
    )]
    pub(crate) fn new(
        file: &'f mut FormatFile<'a, 'p>,
        scanner: FormattingScanner<'s>,
        options: FormatCodeSettings,
        new_line: Vec<u8>,
        original_range: TextRange,
        enclosing_node: NodeId,
        initial_indentation: i64,
        delta: i64,
        request_kind: FormatRequestKind,
        range_contains_error: ErrorRanges,
    ) -> Self {
        Self {
            original_range,
            enclosing_node,
            initial_indentation,
            delta,
            range_contains_error,
            new_line,
            scanner,
            context: FormattingContext::new(file, request_kind, options),
            edits: Vec::new(),
            previous_range: no_range(),
            previous_range_trivia_end: 0,
            previous_parent: None,
            previous_range_start_line: 0,
            child_context_node: None,
            last_indented_line: -1,
            indentation_on_last_indented_line: -1,
            indenters: Vec::new(),
        }
    }

    fn file(&mut self) -> &mut FormatFile<'a, 'p> {
        self.context.file
    }

    fn kind_of(&self, node: NodeId) -> Result<ts_ast::NodeKind, Error> {
        Ok(self.context.file.node(node)?.kind())
    }

    fn range_of(&self, node: NodeId) -> Result<TextRange, Error> {
        Ok(self.context.file.node(node)?.range())
    }

    fn read_token_info(&mut self, node: NodeId) -> Result<TokenInfo, Error> {
        let view = self.context.file.view;
        self.scanner.read_token_info(view, node)
    }

    /// Whether the scanner is on a token that starts inside the range.
    fn on_token_in_range(&self) -> bool {
        self.scanner.is_on_token() && self.scanner.token_full_start() < self.original_range.end()
    }

    // port: tsc/internal/format/span.go:formatSpanWorker.execute
    pub(crate) fn execute(mut self) -> Result<Vec<TextChange>, Error> {
        self.scanner.advance();

        if self.scanner.is_on_token() {
            let enclosing = self.enclosing_node;
            let start = with_token_start(self.file(), enclosing)?.pos();
            let start_line = self.file().line_of(start)?;
            let mut undecorated_start_line = start_line;
            if self.has_decorators(enclosing)? {
                let position = get_non_decorator_token_pos_of_node(self.file(), enclosing)?;
                undecorated_start_line = self.file().line_of(position)?;
            }
            self.process_node(
                enclosing,
                Some(enclosing),
                start_line,
                undecorated_start_line,
                self.initial_indentation,
                self.delta,
            )?;
        }

        // Leading trivia is attached to the token after it and processed with
        // that token. When the range ends inside leading trivia the token is
        // outside the range and never processed, so the trivia is handled here.
        let remaining_trivia = self.scanner.current_leading_trivia().to_vec();
        if !remaining_trivia.is_empty() {
            let mut indentation = self.initial_indentation;
            if node_will_indent_child(
                self.context.file,
                &self.context.options,
                self.enclosing_node,
                None,
                true,
                false,
            )? {
                indentation += self.context.options.editor.indent_size;
            }
            self.indent_trivia_items(&remaining_trivia, indentation, true, SingleLine::Remaining)?;
            if self
                .context
                .options
                .editor
                .trim_trailing_whitespace
                .is_true()
            {
                self.trim_trailing_whitespaces_for_remaining_range(&remaining_trivia)?;
            }
        }

        if self.previous_range != no_range()
            && self.scanner.token_full_start() >= self.original_range.end()
        {
            // Edits come from pairs of contiguous tokens. `process_node` stops
            // at the first token past the end of the range, which can leave out
            // an edit *inside* the range that only shows with one more token:
            // in `x = { }` with the selection ending on the space inside the
            // braces, deleting that space needs the pair `{`, `}`. This block
            // handles that trailing edit.
            let token_info = if self.scanner.is_on_eof() {
                self.scanner.read_eof_token_range()?
            } else if self.scanner.is_on_token() {
                let enclosing = self.enclosing_node;
                self.read_token_info(enclosing)?.token
            } else {
                no_range()
            };

            // The pair must be contiguous. The range may have ended inside a
            // token: formatting stopped on it, leaving the previous range on
            // the token before it while the scanner moved past it. Such a pair
            // straddles the token at the end of the range and would produce
            // bogus edits, and no valid edit exists inside a token anyway.
            if token_info.loc.pos() == self.previous_range_trivia_end {
                let preceding = self
                    .file()
                    .navigator()
                    .find_preceding_token(token_info.loc.end())?;
                let mut parent = match preceding {
                    Some(token) => self.file().node(token)?.parent(),
                    None => None,
                };
                if parent.is_none() {
                    parent = self.previous_parent;
                }
                let line = self.file().line_of(token_info.loc.pos())?;
                self.process_pair(
                    token_info,
                    line,
                    parent,
                    self.previous_range,
                    self.previous_range_start_line,
                    self.previous_parent,
                    parent,
                    None,
                )?;
            }
        }

        Ok(self.edits)
    }

    fn has_decorators(&self, node: NodeId) -> Result<bool, Error> {
        let view = self.context.file.view;
        Ok(utilities_middle::has_decorators(view, &view.node(node)?)?)
    }

    // port: tsc/internal/format/span.go:formatSpanWorker.processChildNode
    #[allow(
        clippy::too_many_arguments,
        reason = "the upstream signature, less the arguments its body never reads"
    )]
    fn process_child_node(
        &mut self,
        node: NodeId,
        child: NodeId,
        mut inherited_indentation: i64,
        parent: NodeId,
        parent_dynamic_indentation: IndenterId,
        parent_start_line: i64,
        undecorated_parent_start_line: i64,
        is_list_item: bool,
        is_first_list_item: bool,
    ) -> Result<i64, Error> {
        let (child_flags, child_range, child_kind) = {
            let read = self.context.file.node(child)?;
            debug_assert(!utilities::node_is_synthesized(&read))?;
            if ts_ast::node_is_missing(Some(&read)) || read.flags() & node_flags::REPARSED != 0 {
                return Ok(inherited_indentation);
            }
            (read.flags(), read.range(), read.kind())
        };
        let child_start_pos = self.file().token_pos(child)?;
        let child_start_line = self.file().line_of(child_start_pos)?;

        let mut undecorated_child_start_line = child_start_line;
        if self.has_decorators(child)? {
            let position = get_non_decorator_token_pos_of_node(self.file(), child)?;
            undecorated_child_start_line = self.file().line_of(position)?;
        }

        let is_error_member_list_element = child_flags & node_flags::THIS_NODE_HAS_ERROR != 0
            && is_member_list_element(self.context.file, parent, child)?;
        // A list item gets its own indentation only when the parent is within
        // the original range.
        let mut child_indentation_amount = -1;

        if !is_error_member_list_element
            && is_list_item
            && contained_by(self.range_of(parent)?, self.original_range)
        {
            child_indentation_amount = self.try_compute_indentation_for_list_item(
                child_start_pos,
                child_range.end(),
                parent_start_line,
                self.original_range,
                inherited_indentation,
            )?;
            if child_indentation_amount != -1 {
                inherited_indentation = child_indentation_amount;
            }
        }

        // The child is outside the target range: do not dive inside.
        if !overlaps(self.original_range, child_range) {
            if child_range.end() < self.original_range.pos() {
                self.scanner.skip_to_end_of(child_range);
            }
            return Ok(inherited_indentation);
        }

        if child_range.is_empty() {
            return Ok(inherited_indentation);
        }

        while self.on_token_in_range() {
            // Process the parent's tokens located before the child's start.
            let token_info = self.read_token_info(node)?;
            if token_info.token.loc.end() > self.original_range.end() {
                return Ok(inherited_indentation);
            }
            if token_info.token.loc.end() > child_start_pos {
                if token_info.token.loc.pos() > child_start_pos {
                    self.scanner.skip_to_start_of(child_range);
                }
                // The scanner is past the beginning of the child.
                break;
            }
            self.consume_token_and_advance_scanner(
                &token_info,
                node,
                parent_dynamic_indentation,
                node,
                false,
            )?;
        }

        if !self.on_token_in_range() {
            return Ok(inherited_indentation);
        }

        if ts_ast::is_token_kind(child_kind) {
            // A token child does not affect indentation: it is processed under
            // the parent's indentation scope.
            let token_info = self.read_token_info(child)?;
            // JSX text should not affect indenting.
            if child_kind != K::JsxText {
                if token_info.token.loc.end() != child_range.end() {
                    return Err(Error::Assertion(
                        "Debug failure. False expression: Token end is child end".into(),
                    ));
                }
                self.consume_token_and_advance_scanner(
                    &token_info,
                    node,
                    parent_dynamic_indentation,
                    child,
                    false,
                )?;
                return Ok(inherited_indentation);
            }
        }

        let effective_parent_start_line = if child_kind == K::Decorator {
            child_start_line
        } else {
            undecorated_parent_start_line
        };
        let (child_indentation, delta) = if is_error_member_list_element {
            (
                self.get_current_indentation_at_position(child_start_pos)?,
                0,
            )
        } else {
            self.compute_indentation(
                child,
                child_start_line,
                child_indentation_amount,
                node,
                parent_dynamic_indentation,
                effective_parent_start_line,
            )?
        };

        let context_node = self.child_context_node;
        self.process_node(
            child,
            context_node,
            child_start_line,
            undecorated_child_start_line,
            child_indentation,
            delta,
        )?;

        self.child_context_node = Some(node);

        if is_first_list_item
            && self.kind_of(parent)? == K::ArrayLiteralExpression
            && inherited_indentation == -1
        {
            inherited_indentation = child_indentation;
        }

        Ok(inherited_indentation)
    }

    // port: tsc/internal/format/span.go:formatSpanWorker.processChildNodes
    fn process_child_nodes(
        &mut self,
        node: NodeId,
        nodes: NodeListId,
        parent: NodeId,
        parent_start_line: i64,
        parent_dynamic_indentation: IndenterId,
    ) -> Result<(), Error> {
        let view = self.context.file.view;
        let list_range = view.list(nodes)?.loc();
        let elements: Vec<NodeId> = view
            .node_slice(view.list(nodes)?.nodes())?
            .iter()
            .flatten()
            .collect();
        debug_assert(!utilities::position_is_synthesized(
            list_range.pos() as isize
        ))?;
        debug_assert(!utilities::position_is_synthesized(
            list_range.end() as isize
        ))?;

        let list_start_token = get_open_token_for_list(self.context.file, parent, nodes)?;

        let mut list_dynamic_indentation = parent_dynamic_indentation;
        let mut start_line = parent_start_line;

        // The list is outside the target range: do not dive inside.
        if !overlaps(self.original_range, list_range) {
            let first_is_reparsed = match elements.first() {
                Some(&first) => view.node(first)?.flags() & node_flags::REPARSED != 0,
                None => false,
            };
            if list_range.end() < self.original_range.pos() && !first_is_reparsed {
                self.scanner.skip_to_end_of(list_range);
            }
            return Ok(());
        }

        if list_start_token != K::Unknown {
            // A list opens its own indentation scope, which includes its start
            // and end tokens.
            while self.on_token_in_range() {
                let token_info = self.read_token_info(parent)?;
                if token_info.token.loc.end() > list_range.pos() {
                    // The scanner is past the beginning of the list.
                    break;
                } else if token_info.token.kind == list_start_token {
                    start_line = self.file().line_of(token_info.token.loc.pos())?;

                    self.consume_token_and_advance_scanner(
                        &token_info,
                        parent,
                        parent_dynamic_indentation,
                        parent,
                        false,
                    )?;

                    // The scanner just processed the list start token, so the
                    // last indentation is the list's: in
                    // `function foo(): {` the members indent from that line.
                    let indentation_on_list_start_token =
                        if self.indentation_on_last_indented_line == -1 {
                            self.get_current_indentation_at_position(token_info.token.loc.pos())?
                        } else {
                            self.indentation_on_last_indented_line
                        };

                    list_dynamic_indentation = self.get_dynamic_indentation(
                        parent,
                        parent_start_line,
                        indentation_on_list_start_token,
                        self.context.options.editor.indent_size,
                    );
                } else {
                    // Tokens before the list belong to the node's own scope.
                    self.consume_token_and_advance_scanner(
                        &token_info,
                        parent,
                        parent_dynamic_indentation,
                        parent,
                        false,
                    )?;
                }
            }
        }

        let mut inherited_indentation = -1;
        for (index, &child) in elements.iter().enumerate() {
            inherited_indentation = self.process_child_node(
                node,
                child,
                inherited_indentation,
                node,
                list_dynamic_indentation,
                start_line,
                start_line,
                true,
                index == 0,
            )?;
        }

        let list_end_token = get_close_token_for_open_token(list_start_token);
        if list_end_token != K::Unknown && self.on_token_in_range() {
            let mut token_info = self.read_token_info(parent)?;
            if token_info.token.kind == K::CommaToken {
                self.consume_token_and_advance_scanner(
                    &token_info,
                    parent,
                    list_dynamic_indentation,
                    parent,
                    false,
                )?;
                if !self.scanner.is_on_token() {
                    return Ok(());
                }
                token_info = self.read_token_info(parent)?;
            }

            // The end token is consumed only while it still belongs to the
            // parent. In `function (x: function) <--` the close paren matches
            // the end token of the function expression but is not its own.
            if token_info.token.kind == list_end_token
                && contained_by(token_info.token.loc, self.range_of(parent)?)
            {
                self.consume_token_and_advance_scanner(
                    &token_info,
                    parent,
                    list_dynamic_indentation,
                    parent,
                    true,
                )?;
            }
        }
        Ok(())
    }

    /// `node.VisitEachChild` under upstream's visitor. Its only hook is
    /// `VisitNodes`, so a modifier list is visited one modifier at a time, as
    /// nodes, while every other list is processed as a list.
    // port: tsc/internal/format/span.go:formatSpanWorker.executeProcessNodeVisitor
    fn execute_process_node_visitor(
        &mut self,
        node: NodeId,
        indenter: IndenterId,
        node_start_line: i64,
        undecorated_node_start_line: i64,
    ) -> Result<(), Error> {
        let view = self.context.file.view;
        let modifiers = view.node(node)?.modifiers();
        for visit in ts_astnav::visit_each_child(view, node)? {
            let children = match visit {
                Visit::Node(child) => vec![child],
                Visit::List(list) if Some(list) == modifiers => view
                    .node_slice(view.list(list)?.nodes())?
                    .iter()
                    .flatten()
                    .collect(),
                Visit::List(list) => {
                    self.process_child_nodes(node, list, node, node_start_line, indenter)?;
                    continue;
                }
            };
            for child in children {
                self.process_child_node(
                    node,
                    child,
                    -1,
                    node,
                    indenter,
                    node_start_line,
                    undecorated_node_start_line,
                    false,
                    false,
                )?;
            }
        }
        Ok(())
    }

    // port: tsc/internal/format/span.go:formatSpanWorker.getCurrentIndentationAtPosition
    fn get_current_indentation_at_position(&mut self, position: i64) -> Result<i64, Error> {
        let start = get_line_start_position_for_position(position, self.context.file)?;
        find_first_non_whitespace_column(self.context.file, start, position, &self.context.options)
    }

    // port: tsc/internal/format/span.go:formatSpanWorker.computeIndentation
    fn compute_indentation(
        &mut self,
        node: NodeId,
        start_line: i64,
        inherited_indentation: i64,
        parent: NodeId,
        parent_dynamic_indentation: IndenterId,
        effective_parent_start_line: i64,
    ) -> Result<(i64, i64), Error> {
        let indent_size = self.context.options.editor.indent_size;
        let delta = if should_indent_child_node(
            self.context.file,
            &self.context.options,
            node,
            None,
            false,
            false,
        )? {
            indent_size
        } else {
            0
        };

        if effective_parent_start_line == start_line {
            // The node is on its parent's line: it inherits the parent's
            // indentation, and pushes its children when either has a delta.
            let indentation = if start_line == self.last_indented_line {
                self.indentation_on_last_indented_line
            } else {
                self.indenters[parent_dynamic_indentation].indentation
            };
            let parent_delta = self.get_delta(parent_dynamic_indentation, Some(node))?;
            return Ok((indentation, indent_size.min(parent_delta + delta)));
        }
        if inherited_indentation != -1 {
            return Ok((inherited_indentation, delta));
        }
        if self.kind_of(node)? == K::OpenParenToken && start_line == self.last_indented_line {
            // Chained method calls: the indentation of the last line and the
            // parent's delta.
            let parent_delta = self.get_delta(parent_dynamic_indentation, Some(node))?;
            return Ok((self.indentation_on_last_indented_line, parent_delta));
        }
        let parent_indentation = self.indenters[parent_dynamic_indentation].indentation;
        if child_starts_on_the_same_line_with_else_in_if_statement(
            self.context.file,
            parent,
            node,
            start_line,
        )? || child_is_unindented_branch_of_conditional_expression(
            self.context.file,
            parent,
            node,
            start_line,
        )? || argument_starts_on_same_line_as_previous_argument(
            self.context.file,
            parent,
            node,
            start_line,
        )? || parent_indentation == -1
        {
            return Ok((parent_indentation, delta));
        }
        let parent_delta = self.get_delta(parent_dynamic_indentation, Some(node))?;
        Ok((parent_indentation + parent_delta, delta))
    }

    /// The indentation of a list element. One outside the range keeps its
    /// actual indentation, which is then inherited downstream; one inside takes
    /// what its predecessors passed on. `-1` when there is no answer.
    // port: tsc/internal/format/span.go:formatSpanWorker.tryComputeIndentationForListItem
    fn try_compute_indentation_for_list_item(
        &mut self,
        start_pos: i64,
        end_pos: i64,
        parent_start_line: i64,
        range: TextRange,
        inherited_indentation: i64,
    ) -> Result<i64, Error> {
        let item = TextRange::new(start_pos, end_pos);
        // Containment is checked too, so zero-length nodes such as JSX text are
        // not missed.
        if overlaps(range, item) || contained_by(item, range) {
            if inherited_indentation != -1 {
                return Ok(inherited_indentation);
            }
        } else {
            let start_line = self.file().line_of(start_pos)?;
            let column = self.get_current_indentation_at_position(start_pos)?;
            if start_line != parent_start_line || start_pos == column {
                // The base indent size wins when it is greater than the
                // indentation of the inherited predecessor.
                return Ok(self.context.options.editor.base_indent_size.max(column));
            }
        }
        Ok(-1)
    }

    // port: tsc/internal/format/span.go:formatSpanWorker.processNode
    fn process_node(
        &mut self,
        node: NodeId,
        context_node: Option<NodeId>,
        node_start_line: i64,
        undecorated_node_start_line: i64,
        indentation: i64,
        delta: i64,
    ) -> Result<(), Error> {
        let range = with_token_start(self.file(), node)?;
        if !overlaps(self.original_range, range) {
            return Ok(());
        }

        let node_dynamic_indentation =
            self.get_dynamic_indentation(node, node_start_line, indentation, delta);

        // In a tree where `a` has children `b`, `c` and `d`, `a` is the context
        // node of all three, except for the leftmost leaf token in `b`, whose
        // context node is somewhere above `a`. The same holds recursively.
        // The context node is set to the parent after every child node, and to
        // the parent of the token after every token.
        self.child_context_node = context_node;

        // Tokens of the node that sit between its children are consumed in
        // `process_child_node`, for the child that follows them.
        self.execute_process_node_visitor(
            node,
            node_dynamic_indentation,
            node_start_line,
            undecorated_node_start_line,
        )?;

        // The tokens of the node that follow its children.
        let node_end = range.end();
        while self.on_token_in_range() {
            let token_info = self.read_token_info(node)?;
            if token_info.token.loc.end() > node_end.min(self.original_range.end()) {
                break;
            }
            self.consume_token_and_advance_scanner(
                &token_info,
                node,
                node_dynamic_indentation,
                node,
                false,
            )?;
        }
        Ok(())
    }

    // port: tsc/internal/format/span.go:formatSpanWorker.processPair
    #[allow(
        clippy::too_many_arguments,
        reason = "the upstream signature: two items, each with its line and parent"
    )]
    fn process_pair(
        &mut self,
        current_item: TextRangeWithKind,
        current_start_line: i64,
        current_parent: Option<NodeId>,
        previous_item: TextRangeWithKind,
        previous_start_line: i64,
        previous_parent: Option<NodeId>,
        context_node: Option<NodeId>,
        dynamic_indentation: Option<IndenterId>,
    ) -> Result<LineAction, Error> {
        let nil = || {
            Error::Assertion(
                "runtime error: invalid memory address or nil pointer dereference".into(),
            )
        };
        let (current_parent, previous_parent, context_node) = (
            current_parent.ok_or_else(nil)?,
            previous_parent.ok_or_else(nil)?,
            context_node.ok_or_else(nil)?,
        );
        self.context.update_context(
            previous_item,
            previous_parent,
            current_item,
            current_parent,
            context_node,
        );

        let rules: Vec<&'static Rule> = crate::rulesmap::get_rules(&mut self.context)?;

        let mut trim_trailing_whitespaces = !self
            .context
            .options
            .editor
            .trim_trailing_whitespace
            .is_false();
        let mut line_action = LineAction::None;

        if rules.is_empty() {
            trim_trailing_whitespaces =
                trim_trailing_whitespaces && current_item.kind != K::EndOfFile;
        } else {
            // In reverse, so that the higher-priority rules, which come first,
            // win a conflict with the lower-priority ones.
            for rule in rules.iter().rev() {
                line_action = self.apply_rule_edits(
                    rule,
                    previous_item,
                    previous_start_line,
                    current_item,
                    current_start_line,
                )?;
                if let Some(indenter) = dynamic_indentation {
                    // A removed line moved the next line up to this one, so it
                    // is not indented in the next pass. An added line moved the
                    // second token down: it is indented in the next pass, and
                    // the indenter learns that the indentation is within the
                    // line.
                    if line_action != LineAction::None
                        && self.file().token_pos(current_parent)? == current_item.loc.pos()
                    {
                        self.recompute_indentation(
                            indenter,
                            line_action == LineAction::LineAdded,
                            context_node,
                        )?;
                    }
                }

                // Trailing whitespace between tokens on different lines is
                // trimmed unless a rule put them on one line.
                trim_trailing_whitespaces = trim_trailing_whitespaces
                    && rule.action & action::DELETE_SPACE == 0
                    && rule.flags != RuleFlags::CanDeleteNewLines;
            }
        }

        if current_start_line != previous_start_line && trim_trailing_whitespaces {
            self.trim_trailing_whitespaces_for_lines(
                previous_start_line,
                current_start_line,
                previous_item,
            )?;
        }

        Ok(line_action)
    }

    // port: tsc/internal/format/span.go:formatSpanWorker.applyRuleEdits
    fn apply_rule_edits(
        &mut self,
        rule: &Rule,
        previous_range: TextRangeWithKind,
        previous_start_line: i64,
        current_range: TextRangeWithKind,
        current_start_line: i64,
    ) -> Result<LineAction, Error> {
        let on_later_line = current_start_line != previous_start_line;
        let (previous_end, current_pos) = (previous_range.loc.end(), current_range.loc.pos());
        match rule.action {
            action::DELETE_SPACE => {
                if previous_end != current_pos {
                    // From the end of the first token up to the second.
                    self.record_delete(previous_end, current_pos - previous_end);
                    if on_later_line {
                        return Ok(LineAction::LineRemoved);
                    }
                }
            }
            action::DELETE_TOKEN => {
                self.record_delete(previous_range.loc.pos(), previous_range.loc.len());
            }
            action::INSERT_NEW_LINE => {
                // On different lines a rule that cannot change the number of
                // new lines has nothing to do: one new line needs no edit, and
                // extra new lines cannot be deleted.
                if rule.flags != RuleFlags::CanDeleteNewLines
                    && previous_start_line != current_start_line
                {
                    return Ok(LineAction::None);
                }
                // No edit when exactly one line feed separates the elements.
                if current_start_line - previous_start_line != 1 {
                    let new_line = self.new_line.clone();
                    self.record_replace(previous_end, current_pos - previous_end, new_line);
                    if !on_later_line {
                        return Ok(LineAction::LineAdded);
                    }
                }
            }
            action::INSERT_SPACE => {
                if rule.flags != RuleFlags::CanDeleteNewLines
                    && previous_start_line != current_start_line
                {
                    return Ok(LineAction::None);
                }
                let pos_delta = current_pos - previous_end;
                let has_space = {
                    let state = self
                        .context
                        .file
                        .view
                        .source_file(self.context.file.source)?;
                    let text = state.text().as_bytes();
                    let Some(rest) = usize::try_from(previous_end)
                        .ok()
                        .and_then(|at| text.get(at..))
                    else {
                        return Err(Error::Assertion(format!(
                            "runtime error: slice bounds out of range [{previous_end}:{}]",
                            text.len()
                        )));
                    };
                    rest.first() == Some(&b' ')
                };
                if pos_delta != 1 || !has_space {
                    self.record_replace(previous_end, pos_delta, b" ".to_vec());
                    if on_later_line {
                        return Ok(LineAction::LineRemoved);
                    }
                }
            }
            action::INSERT_TRAILING_SEMICOLON => self.record_insert(previous_end, b";".to_vec()),
            // Stopping the space actions needs no edit.
            _ => {}
        }
        Ok(LineAction::None)
    }

    // port: tsc/internal/format/span.go:formatSpanWorker.processRange
    fn process_range(
        &mut self,
        range: TextRangeWithKind,
        range_start_line: i64,
        parent: NodeId,
        context_node: Option<NodeId>,
        dynamic_indentation: Option<IndenterId>,
    ) -> Result<LineAction, Error> {
        let range_has_error = self.range_contains_error.contains(range.loc);
        let mut line_action = LineAction::None;
        if !range_has_error {
            if self.previous_range == no_range() {
                // Trim from the beginning of the span up to the current line.
                let original_start = self.original_range.pos();
                let original_start_line = self.file().line_of(original_start)?;
                self.trim_trailing_whitespaces_for_lines(
                    original_start_line,
                    range_start_line,
                    no_range(),
                )?;
            } else {
                line_action = self.process_pair(
                    range,
                    range_start_line,
                    Some(parent),
                    self.previous_range,
                    self.previous_range_start_line,
                    self.previous_parent,
                    context_node,
                    dynamic_indentation,
                )?;
            }
        }

        self.previous_range = range;
        self.previous_range_trivia_end = range.loc.end();
        self.previous_parent = Some(parent);
        self.previous_range_start_line = range_start_line;

        Ok(line_action)
    }

    // port: tsc/internal/format/span.go:formatSpanWorker.processTrivia
    fn process_trivia(
        &mut self,
        trivia: &[TextRangeWithKind],
        parent: NodeId,
        context_node: Option<NodeId>,
        dynamic_indentation: Option<IndenterId>,
    ) -> Result<(), Error> {
        for &item in trivia {
            if is_comment(item.kind) && contained_by(item.loc, self.original_range) {
                let start_line = self.file().line_of(item.loc.pos())?;
                self.process_range(item, start_line, parent, context_node, dynamic_indentation)?;
            }
        }
        Ok(())
    }

    /// Trims the lines after the previous range. Comments are left out: they
    /// were processed already.
    // port: tsc/internal/format/span.go:formatSpanWorker.trimTrailingWhitespacesForRemainingRange
    fn trim_trailing_whitespaces_for_remaining_range(
        &mut self,
        trivias: &[TextRangeWithKind],
    ) -> Result<(), Error> {
        let mut start_pos = if self.previous_range == no_range() {
            self.original_range.pos()
        } else {
            self.previous_range.loc.end()
        };

        for trivia in trivias {
            if is_comment(trivia.kind) {
                if start_pos < trivia.loc.pos() {
                    self.trim_trailing_whitespaces_for_positions(
                        start_pos,
                        trivia.loc.pos() - 1,
                        self.previous_range,
                    )?;
                }
                start_pos = trivia.loc.end() + 1;
            }
        }

        if start_pos < self.original_range.end() {
            self.trim_trailing_whitespaces_for_positions(
                start_pos,
                self.original_range.end(),
                self.previous_range,
            )?;
        }
        Ok(())
    }

    // port: tsc/internal/format/span.go:formatSpanWorker.trimTrailingWitespacesForPositions
    fn trim_trailing_whitespaces_for_positions(
        &mut self,
        start_pos: i64,
        end_pos: i64,
        previous_range: TextRangeWithKind,
    ) -> Result<(), Error> {
        let start_line = self.file().line_of(start_pos)?;
        let end_line = self.file().line_of(end_pos)?;
        self.trim_trailing_whitespaces_for_lines(start_line, end_line + 1, previous_range)
    }

    // port: tsc/internal/format/span.go:formatSpanWorker.trimTrailingWhitespacesForLines
    fn trim_trailing_whitespaces_for_lines(
        &mut self,
        line1: i64,
        line2: i64,
        range: TextRangeWithKind,
    ) -> Result<(), Error> {
        for line in line1..line2 {
            let line_start_position = self.file().line_start(line)?;
            let line_end_position = {
                let state = self
                    .context
                    .file
                    .view
                    .source_file(self.context.file.source)?;
                ts_scanner::get_ecma_end_line_position(&state, line as isize)
            };

            // Whitespace inside comments and template literals is not trimmed.
            if range != no_range()
                && (is_comment(range.kind)
                    || is_string_or_regular_expression_or_template_literal(range.kind.into()))
                && range.loc.pos() <= line_end_position
                && range.loc.end() > line_end_position
            {
                continue;
            }

            let whitespace_start = self
                .get_trailing_whitespace_start_position(line_start_position, line_end_position)?;
            if whitespace_start != -1 {
                if whitespace_start != line_start_position {
                    let state = self
                        .context
                        .file
                        .view
                        .source_file(self.context.file.source)?;
                    let text = state.text().as_bytes();
                    let (ch, _) = decode_utf8(&text[(whitespace_start - 1) as usize..]);
                    debug_assert(!ts_scanner::is_white_space_single_line(ch))?;
                }
                self.record_delete(whitespace_start, line_end_position + 1 - whitespace_start);
            }
        }
        Ok(())
    }

    /// `start` and `end` are the first and the last character of the range.
    // port: tsc/internal/format/span.go:formatSpanWorker.getTrailingWhitespaceStartPosition
    fn get_trailing_whitespace_start_position(&self, start: i64, end: i64) -> Result<i64, Error> {
        let state = self
            .context
            .file
            .view
            .source_file(self.context.file.source)?;
        let text = state.text().as_bytes();
        let mut pos = end;
        while pos >= start {
            let Some(rest) = usize::try_from(pos).ok().and_then(|at| text.get(at..)) else {
                return Err(Error::Assertion(format!(
                    "runtime error: slice bounds out of range [{pos}:{}]",
                    text.len()
                )));
            };
            let (ch, size) = decode_utf8(rest);
            // At the end of the text there is nothing to decode: rewind.
            if size != 0 && !ts_scanner::is_white_space_single_line(ch) {
                break;
            }
            pos -= 1;
        }
        Ok(if pos == end { -1 } else { pos + 1 })
    }

    // port: tsc/internal/format/span.go:formatSpanWorker.insertIndentation
    fn insert_indentation(
        &mut self,
        pos: i64,
        indentation: i64,
        line_added: bool,
    ) -> Result<(), Error> {
        let indentation_string = get_indentation_string(indentation, &self.context.options);
        if line_added {
            // The rules added a new line before the token: the indentation goes
            // at the very beginning of the token.
            self.record_replace(pos, 0, indentation_string);
            return Ok(());
        }
        let (token_start_line, token_start_character) = self.file().line_and_offset(pos)?;
        let start_line_position = self.file().line_start(token_start_line)?;
        if indentation != self.character_to_column(start_line_position, token_start_character)?
            || self.indentation_is_different(&indentation_string, start_line_position)?
        {
            self.record_replace(
                start_line_position,
                token_start_character,
                indentation_string,
            );
        }
        Ok(())
    }

    // port: tsc/internal/format/span.go:formatSpanWorker.characterToColumn
    fn character_to_column(
        &self,
        start_line_position: i64,
        character_in_line: i64,
    ) -> Result<i64, Error> {
        let state = self
            .context
            .file
            .view
            .source_file(self.context.file.source)?;
        let text = state.text().as_bytes();
        let tab_size = self.context.options.editor.tab_size;
        let mut column = 0;
        for offset in 0..character_in_line {
            let index = start_line_position + offset;
            let Some(&byte) = usize::try_from(index).ok().and_then(|at| text.get(at)) else {
                return Err(out_of_range(index, text.len()));
            };
            if byte == b'\t' {
                if tab_size > 0 {
                    column += tab_size - (column % tab_size);
                }
            } else {
                column += 1;
            }
        }
        Ok(column)
    }

    // port: tsc/internal/format/span.go:formatSpanWorker.indentationIsDifferent
    fn indentation_is_different(
        &self,
        indentation_string: &[u8],
        start_line_position: i64,
    ) -> Result<bool, Error> {
        let state = self
            .context
            .file
            .view
            .source_file(self.context.file.source)?;
        let text = state.text().as_bytes();
        let start = start_line_position as usize;
        let end = start + indentation_string.len();
        Ok(end > text.len() || indentation_string != &text[start..end])
    }

    /// Returns whether the next token or trivia is still to be indented.
    // port: tsc/internal/format/span.go:formatSpanWorker.indentTriviaItems
    fn indent_trivia_items(
        &mut self,
        trivia: &[TextRangeWithKind],
        comment_indentation: i64,
        mut indent_next_token_or_trivia: bool,
        single_line: SingleLine,
    ) -> Result<bool, Error> {
        for &item in trivia {
            let trivia_in_range = contained_by(item.loc, self.original_range);
            match item.kind {
                K::MultiLineCommentTrivia => {
                    if trivia_in_range {
                        self.indent_multiline_comment(
                            item.loc,
                            comment_indentation,
                            !indent_next_token_or_trivia,
                            true,
                        )?;
                    }
                    indent_next_token_or_trivia = false;
                }
                K::SingleLineCommentTrivia => {
                    if indent_next_token_or_trivia && trivia_in_range {
                        if let SingleLine::Remaining = single_line {
                            let start_line = self.file().line_of(item.loc.pos())?;
                            let enclosing = self.enclosing_node;
                            self.process_range(item, start_line, enclosing, Some(enclosing), None)?;
                        }
                        self.insert_indentation(item.loc.pos(), comment_indentation, false)?;
                    }
                    indent_next_token_or_trivia = false;
                }
                K::NewLineTrivia => indent_next_token_or_trivia = true,
                _ => {}
            }
        }
        Ok(indent_next_token_or_trivia)
    }

    // port: tsc/internal/format/span.go:formatSpanWorker.indentMultilineComment
    fn indent_multiline_comment(
        &mut self,
        comment_range: TextRange,
        indentation: i64,
        first_line_is_indented: bool,
        indent_final_line: bool,
    ) -> Result<(), Error> {
        // Split the comment in lines.
        let mut start_line = self.file().line_of(comment_range.pos())?;
        let end_line = self.file().line_of(comment_range.end())?;

        if start_line == end_line {
            if !first_line_is_indented {
                // Treated as a single-line comment.
                self.insert_indentation(comment_range.pos(), indentation, false)?;
            }
            return Ok(());
        }

        let mut parts: Vec<TextRange> = Vec::new();
        let mut start_pos = comment_range.pos();
        for line in start_line..end_line {
            let end_of_line = {
                let state = self
                    .context
                    .file
                    .view
                    .source_file(self.context.file.source)?;
                ts_scanner::get_ecma_end_line_position(&state, line as isize)
            };
            parts.push(TextRange::new(start_pos, end_of_line));
            start_pos = self.file().line_start(line + 1)?;
        }

        if indent_final_line {
            parts.push(TextRange::new(start_pos, comment_range.end()));
        }

        if parts.is_empty() {
            return Ok(());
        }

        let start_line_pos = self.file().line_start(start_line)?;
        let (non_whitespace_in_first_part_character, non_whitespace_in_first_part_column) =
            find_first_non_whitespace_character_and_column(
                self.context.file,
                start_line_pos,
                parts[0].pos(),
                &self.context.options,
            )?;

        let mut start_index = 0;
        if first_line_is_indented {
            start_index = 1;
            start_line += 1;
        }

        // Shift every part by the delta.
        let delta = indentation - non_whitespace_in_first_part_column;
        for (index, part) in parts.iter().enumerate().skip(start_index) {
            let start_line_pos = self.file().line_start(start_line)?;
            let (non_whitespace_character, non_whitespace_column) = if index == 0 {
                (
                    non_whitespace_in_first_part_character,
                    non_whitespace_in_first_part_column,
                )
            } else {
                find_first_non_whitespace_character_and_column(
                    self.context.file,
                    part.pos(),
                    part.end(),
                    &self.context.options,
                )?
            };
            let new_indentation = non_whitespace_column + delta;
            if new_indentation > 0 {
                let indentation_string =
                    get_indentation_string(new_indentation, &self.context.options);
                self.record_replace(start_line_pos, non_whitespace_character, indentation_string);
            } else {
                self.record_delete(start_line_pos, non_whitespace_character);
            }
            start_line += 1;
        }
        Ok(())
    }

    // port: tsc/internal/format/span.go:formatSpanWorker.recordDelete
    fn record_delete(&mut self, start: i64, length: i64) {
        if length != 0 {
            self.edits.push(create_text_change_from_start_length(
                start,
                length,
                Vec::new(),
            ));
        }
    }

    // port: tsc/internal/format/span.go:formatSpanWorker.recordReplace
    fn record_replace(&mut self, start: i64, length: i64, new_text: Vec<u8>) {
        if length != 0 || !new_text.is_empty() {
            self.edits.push(create_text_change_from_start_length(
                start, length, new_text,
            ));
        }
    }

    // port: tsc/internal/format/span.go:formatSpanWorker.recordInsert
    fn record_insert(&mut self, start: i64, text: Vec<u8>) {
        if !text.is_empty() {
            self.edits
                .push(create_text_change_from_start_length(start, 0, text));
        }
    }

    // port: tsc/internal/format/span.go:formatSpanWorker.consumeTokenAndAdvanceScanner
    fn consume_token_and_advance_scanner(
        &mut self,
        current_token_info: &TokenInfo,
        parent: NodeId,
        dynamic_indentation: IndenterId,
        container: NodeId,
        is_list_end_token: bool,
    ) -> Result<(), Error> {
        let last_trivia_was_new_line = self.scanner.last_trailing_trivia_was_new_line();
        let mut indent_token = false;
        let token = current_token_info.token;

        if !current_token_info.leading_trivia.is_empty() {
            let context_node = self.child_context_node;
            self.process_trivia(
                &current_token_info.leading_trivia,
                parent,
                context_node,
                Some(dynamic_indentation),
            )?;
        }

        let mut line_action = LineAction::None;
        let is_token_in_range = contained_by(token.loc, self.original_range);
        let token_start_line = self.file().line_of(token.loc.pos())?;

        if is_token_in_range {
            let range_has_error = self.range_contains_error.contains(token.loc);
            // `process_range` overwrites the previous range with this one.
            let save_previous_range = self.previous_range;
            let context_node = self.child_context_node;
            line_action = self.process_range(
                token,
                token_start_line,
                parent,
                context_node,
                Some(dynamic_indentation),
            )?;
            // Nothing is indented when the token overlaps an error.
            if !range_has_error {
                indent_token = if line_action != LineAction::None {
                    line_action == LineAction::LineAdded
                } else if save_previous_range == no_range() {
                    // With no previous range Strada compares the line with
                    // `undefined`, which never equals it.
                    last_trivia_was_new_line
                } else {
                    // Indent only when the previous range ends on another line
                    // than the token starts.
                    let previous_end_line = self.file().line_of(save_previous_range.loc.end())?;
                    last_trivia_was_new_line && token_start_line != previous_end_line
                };
            }
        }

        if let Some(last) = current_token_info.trailing_trivia.last() {
            self.previous_range_trivia_end = last.loc.end();
            // A trailing comment that extends past the original range is not
            // processed as trivia. The end is capped before it, so the trailing
            // edit in `execute` does not pair across unprocessed comment text.
            for trivia in &current_token_info.trailing_trivia {
                if is_comment(trivia.kind) && !contained_by(trivia.loc, self.original_range) {
                    self.previous_range_trivia_end = trivia.loc.pos();
                    break;
                }
            }
            let context_node = self.child_context_node;
            self.process_trivia(
                &current_token_info.trailing_trivia,
                parent,
                context_node,
                Some(dynamic_indentation),
            )?;
        }

        if indent_token {
            let mut token_indentation = -1;
            if is_token_in_range && !self.range_contains_error.contains(token.loc) {
                token_indentation = self.get_indentation_for_token(
                    dynamic_indentation,
                    token_start_line,
                    token.kind,
                    container,
                    is_list_end_token,
                )?;
            }
            let mut indent_next_token_or_trivia = true;
            if !current_token_info.leading_trivia.is_empty() {
                let comment_indentation = self.get_indentation_for_comment(
                    dynamic_indentation,
                    token.kind,
                    token_indentation,
                    container,
                )?;
                indent_next_token_or_trivia = self.indent_trivia_items(
                    &current_token_info.leading_trivia,
                    comment_indentation,
                    indent_next_token_or_trivia,
                    SingleLine::Leading,
                )?;
            }

            // The token is indented only in the target range and clear of
            // errors.
            if token_indentation != -1 && indent_next_token_or_trivia {
                self.insert_indentation(
                    token.loc.pos(),
                    token_indentation,
                    line_action == LineAction::LineAdded,
                )?;
                self.last_indented_line = token_start_line;
                self.indentation_on_last_indented_line = token_indentation;
            }
        }

        self.scanner.advance();
        self.child_context_node = Some(parent);
        Ok(())
    }

    // port: tsc/internal/format/span.go:formatSpanWorker.getDynamicIndentation
    fn get_dynamic_indentation(
        &mut self,
        node: NodeId,
        node_start_line: i64,
        indentation: i64,
        delta: i64,
    ) -> IndenterId {
        self.indenters.push(DynamicIndenter {
            node,
            node_start_line,
            indentation,
            delta,
        });
        self.indenters.len() - 1
    }

    // port: tsc/internal/format/span.go:dynamicIndenter.getIndentationForComment
    fn get_indentation_for_comment(
        &mut self,
        indenter: IndenterId,
        kind: K,
        token_indentation: i64,
        container: NodeId,
    ) -> Result<i64, Error> {
        // A comment before the token that closes an indentation scope takes the
        // indentation of the scope.
        if matches!(
            kind,
            K::CloseBraceToken | K::CloseBracketToken | K::CloseParenToken
        ) {
            return Ok(
                self.indenters[indenter].indentation + self.get_delta(indenter, Some(container))?
            );
        }
        if token_indentation != -1 {
            return Ok(token_indentation);
        }
        Ok(self.indenters[indenter].indentation)
    }

    /// The delta of a `>` that ends a list is suppressed by the caller, so
    /// that `>` as a binary operator can still be indented.
    // port: tsc/internal/format/span.go:dynamicIndenter.getIndentationForToken
    fn get_indentation_for_token(
        &mut self,
        indenter: IndenterId,
        line: i64,
        kind: K,
        container: NodeId,
        suppress_delta: bool,
    ) -> Result<i64, Error> {
        if !suppress_delta && self.should_add_delta(indenter, line, kind, container)? {
            return Ok(
                self.indenters[indenter].indentation + self.get_delta(indenter, Some(container))?
            );
        }
        Ok(self.indenters[indenter].indentation)
    }

    /// Zero when the node explicitly prevents indentation of the child.
    // port: tsc/internal/format/span.go:dynamicIndenter.getDelta
    fn get_delta(&self, indenter: IndenterId, child: Option<NodeId>) -> Result<i64, Error> {
        let state = &self.indenters[indenter];
        Ok(
            if node_will_indent_child(
                self.context.file,
                &self.context.options,
                state.node,
                child,
                true,
                true,
            )? {
                state.delta
            } else {
                0
            },
        )
    }

    // port: tsc/internal/format/span.go:dynamicIndenter.recomputeIndentation
    fn recompute_indentation(
        &mut self,
        indenter: IndenterId,
        line_added: bool,
        parent: NodeId,
    ) -> Result<(), Error> {
        let node = self.indenters[indenter].node;
        let indent_size = self.context.options.editor.indent_size;
        if should_indent_child_node(
            self.context.file,
            &self.context.options,
            parent,
            Some(node),
            true,
            false,
        )? {
            let own = should_indent_child_node(
                self.context.file,
                &self.context.options,
                node,
                None,
                false,
                false,
            )?;
            let state = &mut self.indenters[indenter];
            if line_added {
                state.indentation += indent_size;
            } else {
                state.indentation -= indent_size;
            }
            state.delta = if own { indent_size } else { 0 };
        }
        Ok(())
    }

    // port: tsc/internal/format/span.go:dynamicIndenter.shouldAddDelta
    fn should_add_delta(
        &mut self,
        indenter: IndenterId,
        line: i64,
        kind: K,
        container: NodeId,
    ) -> Result<bool, Error> {
        let container_kind = self.kind_of(container)?;
        match kind {
            // Braces, `else` and the `while` of a `do` statement have the
            // indentation of the parent.
            K::OpenBraceToken
            | K::CloseBraceToken
            | K::CloseParenToken
            | K::ElseKeyword
            | K::WhileKeyword
            | K::AtToken => return Ok(false),
            K::SlashToken | K::GreaterThanToken => {
                if matches!(
                    container_kind.known(),
                    Some(K::JsxOpeningElement | K::JsxClosingElement | K::JsxSelfClosingElement)
                ) {
                    return Ok(false);
                }
            }
            K::OpenBracketToken | K::CloseBracketToken if container_kind != K::MappedType => {
                return Ok(false);
            }
            _ => {}
        }
        let (node, node_start_line) = {
            let state = &self.indenters[indenter];
            (state.node, state.node_start_line)
        };
        // The first token of the node, on the node's own line, uses the node's
        // indentation. So does the first token after a list of decorators.
        if node_start_line == line {
            return Ok(false);
        }
        if !self.has_decorators(node)? {
            return Ok(true);
        }
        Ok(kind != get_first_non_decorator_token_of_node(self.context.file, node)?)
    }
}

// port: tsc/internal/format/span.go:getNonDecoratorTokenPosOfNode
fn get_non_decorator_token_pos_of_node(
    file: &mut FormatFile<'_, '_>,
    node: NodeId,
) -> Result<i64, Error> {
    let view = file.view;
    let read = view.node(node)?;
    let mut last_decorator = None;
    if utilities_middle::has_decorators(view, &read)? {
        for modifier in modifier_nodes(file, node)?.into_iter().rev() {
            if view.node(modifier)?.kind() == K::Decorator {
                last_decorator = Some(modifier);
                break;
            }
        }
    }
    let Some(last_decorator) = last_decorator else {
        return Ok(with_token_start(file, node)?.pos());
    };
    let end = i64::from(view.node(last_decorator)?.end());
    let state = view.source_file(file.source)?;
    Ok(ts_scanner::skip_trivia(state.text().as_bytes(), end))
}

/// `node.ModifierNodes()`: empty when the node has no modifier list.
fn modifier_nodes(file: &FormatFile<'_, '_>, node: NodeId) -> Result<Vec<NodeId>, Error> {
    let view = file.view;
    match view.node(node)?.modifiers() {
        Some(list) => Ok(view
            .node_slice(view.list(list)?.nodes())?
            .iter()
            .flatten()
            .collect()),
        None => Ok(Vec::new()),
    }
}

// port: tsc/internal/format/span.go:getFirstNonDecoratorTokenOfNode
fn get_first_non_decorator_token_of_node(
    file: &FormatFile<'_, '_>,
    node: NodeId,
) -> Result<K, Error> {
    let view = file.view;
    let read = view.node(node)?;
    if utilities::can_have_modifiers(&read) {
        let modifiers = modifier_nodes(file, node)?;
        let mut first_decorator = None;
        for (index, &modifier) in modifiers.iter().enumerate() {
            if view.node(modifier)?.kind() == K::Decorator {
                first_decorator = Some(index);
                break;
            }
        }
        // Upstream slices from the index without checking that one was found.
        let Some(first_decorator) = first_decorator else {
            return Err(Error::Assertion(
                "runtime error: slice bounds out of range [-1:]".into(),
            ));
        };
        for &modifier in &modifiers[first_decorator..] {
            let modifier = view.node(modifier)?;
            if utilities::is_modifier(&modifier) {
                return Ok(modifier.kind().known().unwrap_or(K::Unknown));
            }
        }
    }

    let name_kind = |file: &FormatFile<'_, '_>| -> Result<K, Error> {
        Ok(
            match ts_ast::get_name_of_declaration(file.view, Some(node))? {
                Some(name) => file.view.node(name)?.kind().known().unwrap_or(K::Unknown),
                None => K::Unknown,
            },
        )
    };
    Ok(match read.kind().known() {
        Some(K::ClassDeclaration) => K::ClassKeyword,
        Some(K::InterfaceDeclaration) => K::InterfaceKeyword,
        Some(K::FunctionDeclaration) => K::FunctionKeyword,
        // Upstream answers with the declaration kind here, not the keyword.
        Some(K::EnumDeclaration) => K::EnumDeclaration,
        Some(K::GetAccessor) => K::GetKeyword,
        Some(K::SetAccessor) => K::SetKeyword,
        Some(K::MethodDeclaration) => {
            let has_asterisk = read
                .data_source()
                .as_method_declaration()
                .is_some_and(|method| method.asterisk_token().is_some());
            if has_asterisk {
                K::AsteriskToken
            } else {
                name_kind(file)?
            }
        }
        Some(K::PropertyDeclaration | K::Parameter) => name_kind(file)?,
        _ => K::Unknown,
    })
}

// port: tsc/internal/format/span.go:getIndentationString
pub(crate) fn get_indentation_string(indentation: i64, options: &FormatCodeSettings) -> Vec<u8> {
    if options.editor.convert_tabs_to_spaces.is_true() {
        return vec![b' '; usize::try_from(indentation).unwrap_or(0)];
    }
    let tab_size = options.editor.tab_size;
    if tab_size == 0 {
        return Vec::new();
    }
    // `math.Floor` of the quotient.
    let tabs = indentation.div_euclid(tab_size);
    let spaces = indentation - tabs * tab_size;
    let mut out = vec![b'\t'; usize::try_from(tabs).unwrap_or(0)];
    out.extend(std::iter::repeat_n(
        b' ',
        usize::try_from(spaces).unwrap_or(0),
    ));
    out
}

// port: tsc/internal/format/span.go:createTextChangeFromStartLength
fn create_text_change_from_start_length(start: i64, length: i64, new_text: Vec<u8>) -> TextChange {
    TextChange {
        range: TextRange::new(start, start + length),
        new_text,
    }
}
