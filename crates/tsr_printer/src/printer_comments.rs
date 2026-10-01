//! Comments, source-map positions and the scoped enter/exit operations of
//! `tsc/internal/printer/printer.go` (its "Comments", "Source Maps" and
//! "Scoped operations" sections).
//!
//! Comment state lives on the [`Session`]: upstream keeps it on the printer,
//! where every balanced enter/exit pair restores it, so a printer that is not
//! reentered observes the same values.
//!
//! Source maps are a named boundary. `Printer.Write` receives no generator
//! here, so `sourceMapsDisabled` is always set and the guards upstream checks
//! first answer "no map" before any generator call; the generator calls
//! themselves return [`Error::Unsupported`] until they are wired.

use super::{position_is_synthesized, Session, ViewFactory};
use crate::utilities::{is_jsdoc_like_text, is_pinned_comment, is_recognized_triple_slash_comment};
use crate::{emit_flags as ef, EmitFlags, EmitTextWriter, Error, SynthesizedComment};
use tsr_ast::{NodeId, NodeKind, SyntaxKind as K};
use tsr_core::TextRange;
use tsr_scanner::CommentRange;

/// `tokenEmitFlags`.
pub(crate) mod token_emit_flags {
    pub type TokenEmitFlags = u32;
    pub const NONE: TokenEmitFlags = 0;
    pub const NO_COMMENTS: TokenEmitFlags = 1 << 0;
    pub const INDENT_LEADING_COMMENTS: TokenEmitFlags = 1 << 1;
    pub const NO_SOURCE_MAPS: TokenEmitFlags = 1 << 2;
}
use token_emit_flags as tef;

/// `commentState`: what a node's leading comments changed, restored by its
/// trailing comments.
#[derive(Clone, Copy, Debug)]
pub(crate) struct CommentState {
    emit_flags: EmitFlags,
    comment_range: TextRange,
    container_pos: i64,
    container_end: i64,
    declaration_list_container_end: i64,
}

impl CommentState {
    /// `commentStateArena.New()`: the zero state a token's comments return.
    fn zero() -> Self {
        Self {
            emit_flags: ef::NONE,
            comment_range: TextRange::new(0, 0),
            container_pos: 0,
            container_end: 0,
            declaration_list_container_end: 0,
        }
    }
}

/// `sourceMapState`. No value exists while source maps are a boundary: every
/// path that would create one returns [`Error::Unsupported`] first.
#[derive(Debug)]
pub(crate) enum SourceMapState {}

/// `sourcemap.Source` as the printer holds it. `setSourceMapSource` is the
/// source-map boundary and returns while maps are disabled, so no value is ever
/// held.
#[derive(Clone, Copy, Debug)]
pub(crate) enum SourceMapSource {}

/// `printerState`.
#[derive(Debug, Default)]
pub(crate) struct PrinterState {
    comment_state: Option<CommentState>,
    source_map_state: Option<SourceMapState>,
}

/// `commentSeparator`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CommentSeparator {
    None,
    Before,
    After,
}

/// `detachedCommentsInfo`.
#[derive(Clone, Copy, Debug)]
pub(crate) struct DetachedCommentsInfo {
    node_pos: i64,
    detached_comment_end_pos: i64,
}

/// The parts of a node comment emission reads: its kind, emit flags, comment
/// range and, for a node of the tree, its identity (synthetic comments are
/// keyed by it). Upstream also creates a few nodes while printing; those have
/// no identity here, no emit flags and no synthetic comments, as a fresh node
/// upstream has none.
#[derive(Clone, Copy, Debug)]
pub(crate) struct CommentTarget {
    pub(crate) node: Option<NodeId>,
    pub(crate) kind: NodeKind,
    pub(crate) emit_flags: EmitFlags,
    pub(crate) comment_range: TextRange,
}

/// Upstream's `core.Tristate` for `emitLeadingComments`.
#[derive(Clone, Copy, PartialEq, Eq)]
enum TripleSlash {
    Unknown,
    True,
    False,
}

fn go_is_space(rune: i32) -> bool {
    // unicode.IsSpace at the pin.
    matches!(
        rune,
        0x09..=0x0d | 0x20 | 0x85 | 0xa0 | 0x1680 | 0x2000..=0x200a | 0x2028 | 0x2029 | 0x202f
            | 0x205f | 0x3000
    )
}

/// `strings.TrimSpace`: Go's standard decoder from both ends.
fn trim_space(mut bytes: &[u8]) -> &[u8] {
    use tsr_jsstring::wtf8::decode_utf8;
    while !bytes.is_empty() {
        let (rune, width) = decode_utf8(bytes);
        if !go_is_space(rune) {
            break;
        }
        bytes = &bytes[width..];
    }
    while !bytes.is_empty() {
        let end = bytes.len();
        let mut start = end - 1;
        let lower = end.saturating_sub(4);
        while start > lower && bytes[start] & 0xc0 == 0x80 {
            start -= 1;
        }
        let (rune, width) = decode_utf8(&bytes[start..]);
        let (rune, width) = if start + width == end {
            (rune, width)
        } else {
            (0xfffd, 1)
        };
        if !go_is_space(rune) {
            break;
        }
        bytes = &bytes[..end - width];
    }
    bytes
}

fn index(pos: i64) -> usize {
    usize::try_from(pos).expect("text positions are not negative")
}

/// Writes one comment, re-indenting the continuation lines of a multi-line
/// comment relative to the writer's current indentation.
// port: tsc/internal/printer/printer.go:Printer.writeCommentRangeWorker
pub(crate) fn write_comment_range_worker(
    writer: &mut dyn EmitTextWriter,
    text: &[u8],
    line_map: &[i32],
    kind: K,
    loc: TextRange,
) {
    if kind == K::MultiLineCommentTrivia {
        let indent_size = crate::get_default_indent_size() as i64;
        let first_line =
            tsr_jsstring::scanner_positions::compute_line_of_position(line_map, loc.pos() as isize)
                as i64;
        let line_count = line_map.len() as i64;
        let mut first_comment_line_indent = -1;
        let mut pos = loc.pos();
        let mut current_line = first_line;
        while pos < loc.end() {
            let next_line_start = if current_line + 1 == line_count {
                text.len() as i64 + 1
            } else {
                i64::from(line_map[index(current_line + 1)])
            };
            if pos != loc.pos() {
                // If we are not emitting first line, we need to write the spaces to adjust the alignment
                if first_comment_line_indent == -1 {
                    first_comment_line_indent = crate::utilities::calculate_indent(
                        text,
                        i64::from(line_map[index(first_line)]),
                        loc.pos(),
                    );
                }
                // These are number of spaces writer is going to write at current indent
                let current_writer_indent_spacing = writer.get_indent() as i64 * indent_size;
                // Number of spaces we want to be writing
                let spaces_to_emit = current_writer_indent_spacing - first_comment_line_indent
                    + crate::utilities::calculate_indent(text, pos, next_line_start);
                if spaces_to_emit > 0 {
                    let mut number_of_single_spaces_to_emit = spaces_to_emit % indent_size;
                    let indent_size_space_string = crate::text_writer::indent_string(
                        ((spaces_to_emit - number_of_single_spaces_to_emit) / indent_size) as isize,
                        indent_size as isize,
                    );
                    // Write indent size string
                    writer.raw_write(&indent_size_space_string);
                    // Emit the single spaces
                    while number_of_single_spaces_to_emit > 0 {
                        writer.raw_write(b" ");
                        number_of_single_spaces_to_emit -= 1;
                    }
                } else {
                    // No spaces to emit write empty string
                    writer.raw_write(b"");
                }
            }
            // Write the comment line text
            let mut end = loc.end().min(next_line_start);
            let mut scan = pos;
            while scan < end {
                let (ch, size) = tsr_jsstring::wtf8::decode_utf8(&text[index(scan)..index(end)]);
                if size == 0 {
                    break;
                }
                if tsr_jsstring::classify::is_line_break(ch) {
                    end = scan;
                    break;
                }
                scan += size as i64;
            }
            let current_line_text = trim_space(&text[index(pos)..index(end)]);
            if current_line_text.is_empty() {
                // Empty string - make sure we write empty line
                writer.write_line_force(true);
            } else {
                writer.write_comment(current_line_text);
                if end != loc.end() {
                    writer.write_line();
                }
            }
            pos = next_line_start;
            current_line += 1;
        }
    } else {
        // Single line comment of style //....
        writer.write_comment(&text[index(loc.pos())..index(loc.end())]);
    }
}

// port: tsc/internal/printer/printer.go:formatSynthesizedComment
fn format_synthesized_comment(comment: &SynthesizedComment) -> Vec<u8> {
    let text = comment.text.as_bytes();
    let mut out = Vec::with_capacity(text.len() + 4);
    if comment.kind == K::MultiLineCommentTrivia {
        out.extend_from_slice(b"/*");
        out.extend_from_slice(text);
        out.extend_from_slice(b"*/");
    } else {
        out.extend_from_slice(b"//");
        out.extend_from_slice(text);
    }
    out
}

impl Session<'_, '_> {
    fn source_text_bytes(&self) -> Option<&[u8]> {
        self.source_text()
    }

    fn line_of(&self, pos: i64) -> i64 {
        let line_map = self.line_starts().expect("line queries need a source file");
        tsr_jsstring::scanner_positions::compute_line_of_position(line_map, pos as isize) as i64
    }

    /// `EmitContext.ParseNode`: the most original node when it is a parse-tree
    /// node.
    pub(crate) fn parse_node(&self, node: NodeId) -> Option<NodeId> {
        self.printer
            .emit_context
            .parse_node(&ViewFactory(self.view), node)
    }

    /// The node whose synthetic comments and erased type node print with
    /// `node`'s comments: none for a node `parenthesizeExpressionForNoAsi`
    /// updated, as an update copies neither.
    fn comment_identity(&self, node: NodeId) -> Option<NodeId> {
        (!self.is_no_asi_updated(node)).then_some(node)
    }

    /// The comment-relevant parts of a node of the tree.
    pub(crate) fn comment_target(&self, node: NodeId) -> Result<CommentTarget, Error> {
        let read = self.node(node)?;
        Ok(CommentTarget {
            node: self.comment_identity(node),
            kind: read.kind(),
            emit_flags: self.emit_flags(node),
            comment_range: self
                .printer
                .emit_context
                .comment_range(node)
                .unwrap_or_else(|| read.range()),
        })
    }

    //
    // Custom emit behavior
    //

    fn should_emit_comments_of_kind(&self, kind: NodeKind) -> bool {
        !self.comments_disabled && self.current_source.is_some() && kind != K::SourceFile
    }

    // port: tsc/internal/printer/printer.go:Printer.shouldEmitComments
    fn should_emit_comments(&self, node: NodeId) -> Result<bool, Error> {
        Ok(!self.comments_disabled
            && self.current_source.is_some()
            && self.node(node)?.kind() != K::SourceFile)
    }

    // port: tsc/internal/printer/printer.go:Printer.shouldWriteComment
    pub(crate) fn should_write_comment(&self, comment: &CommentRange) -> bool {
        !self.printer.options.only_print_js_doc_style
            || self
                .source_text_bytes()
                .is_some_and(|text| is_jsdoc_like_text(text, comment))
            || self
                .source_text_bytes()
                .is_some_and(|text| is_pinned_comment(text, comment))
    }

    // port: tsc/internal/printer/printer.go:Printer.shouldEmitSourceMaps
    pub(crate) fn should_emit_source_maps(&self, node: Option<NodeId>) -> Result<bool, Error> {
        if self.source_maps_disabled || self.source_map_source.is_none() {
            return Ok(false);
        }
        // A node the printer creates is neither a source file nor in a JSON
        // file.
        let Some(node) = node else {
            return Ok(true);
        };
        let read = self.node(node)?;
        Ok(read.kind() != K::SourceFile && !tsr_ast::utilities::is_in_json_file(&read))
    }

    /// Tokens other than braces have no positions: they are noisy, but coverage
    /// tools need branch braces. Declaration files omit those too.
    // port: tsc/internal/printer/printer.go:Printer.shouldEmitTokenSourceMaps
    fn should_emit_token_source_maps(
        &self,
        token: K,
        _pos: i64,
        context: Option<NodeId>,
        flags: tef::TokenEmitFlags,
    ) -> Result<bool, Error> {
        Ok(flags & tef::NO_SOURCE_MAPS == 0
            && self.should_emit_source_maps(context)?
            && !self.printer.options.omit_brace_source_map_positions
            && (token == K::OpenBraceToken || token == K::CloseBraceToken))
    }

    // port: tsc/internal/printer/printer.go:Printer.shouldEmitLeadingComments
    pub(crate) fn should_emit_leading_comments(&self, node: NodeId) -> bool {
        self.emit_flags(node) & ef::NO_LEADING_COMMENTS == 0
    }

    // port: tsc/internal/printer/printer.go:Printer.shouldEmitTrailingComments
    pub(crate) fn should_emit_trailing_comments(&self, node: Option<NodeId>) -> bool {
        // Upstream reads the flags of a nil node as none.
        node.is_none_or(|node| self.emit_flags(node) & ef::NO_TRAILING_COMMENTS == 0)
    }

    // port: tsc/internal/printer/printer.go:Printer.shouldEmitNestedComments
    #[allow(
        dead_code,
        reason = "unused upstream at the pin; kept with its siblings"
    )]
    pub(crate) fn should_emit_nested_comments(&self, node: NodeId) -> bool {
        self.emit_flags(node) & ef::NO_NESTED_COMMENTS == 0
    }

    /// A file's detached comments are emitted unless its first statement is a
    /// parsed prologue directive, whose own leading comments include them.
    // port: tsc/internal/printer/printer.go:Printer.shouldEmitDetachedComments
    fn should_emit_detached_comments(&self, node: NodeId) -> Result<bool, Error> {
        let read = self.node(node)?;
        if read.kind() != K::SourceFile {
            return Ok(true);
        }
        let statements = self.list_nodes_of(read.statement_list())?;
        Ok(statements.is_empty()
            || !tsr_ast::utilities::is_prologue_directive(self.view, statements[0])?
            || tsr_ast::utilities::node_is_synthesized(&self.node(statements[0])?))
    }

    // port: tsc/internal/printer/printer.go:Printer.hasCommentsAtPosition
    pub(crate) fn has_comments_at_position(&self, pos: i64) -> bool {
        let Some(text) = self.source_text_bytes() else {
            return false;
        };
        if tsr_scanner::get_trailing_comment_ranges(text, pos + 1)
            .next()
            .is_some()
        {
            return true;
        }
        tsr_scanner::get_leading_comment_ranges(text, pos + 1)
            .next()
            .is_some()
    }

    // port: tsc/internal/printer/printer.go:Printer.shouldEmitIndirectCall
    pub(crate) fn should_emit_indirect_call(&self, node: NodeId) -> bool {
        self.emit_flags(node) & ef::INDIRECT_CALL != 0
    }

    //
    // Comments
    //

    // port: tsc/internal/printer/printer.go:Printer.writeCommentRange
    fn write_comment_range(&mut self, comment: &CommentRange) {
        let Some((_, source)) = &self.current_source else {
            return;
        };
        let text = source.text().as_bytes();
        let line_map = source.ecma_line_map();
        write_comment_range_worker(&mut *self.writer, text, line_map, comment.kind, comment.loc);
    }

    // port: tsc/internal/printer/printer.go:Printer.emitCommentsBeforeNode
    pub(crate) fn emit_comments_before_node(
        &mut self,
        node: NodeId,
    ) -> Result<Option<CommentState>, Error> {
        if !self.should_emit_comments(node)? {
            return Ok(None);
        }
        let target = self.comment_target(node)?;
        Ok(Some(self.emit_comments_before_target(&target)))
    }

    /// `emitCommentsBeforeNode` once `shouldEmitComments` held.
    fn emit_comments_before_target(&mut self, target: &CommentTarget) -> CommentState {
        let emit_flags = target.emit_flags;
        let comment_range = target.comment_range;
        let container_pos = self.container_pos;
        let container_end = self.container_end;
        let declaration_list_container_end = self.declaration_list_container_end;

        // Emit leading comments
        self.emit_leading_comments_of_node(target.kind, emit_flags, comment_range);
        self.emit_leading_synthetic_comments_of_node(target.node, emit_flags);
        if emit_flags & ef::NO_NESTED_COMMENTS != 0 {
            self.comments_disabled = true;
        }
        CommentState {
            emit_flags,
            comment_range,
            container_pos,
            container_end,
            declaration_list_container_end,
        }
    }

    // port: tsc/internal/printer/printer.go:Printer.emitCommentsAfterNode
    pub(crate) fn emit_comments_after_node(
        &mut self,
        node: Option<NodeId>,
        kind: NodeKind,
        state: Option<CommentState>,
    ) -> Result<(), Error> {
        let Some(state) = state else {
            return Ok(());
        };
        let emit_flags = state.emit_flags;
        let comment_range = state.comment_range;
        let container_pos = state.container_pos;
        let container_end = state.container_end;
        let declaration_list_container_end = state.declaration_list_container_end;

        // Emit trailing comments
        if emit_flags & ef::NO_NESTED_COMMENTS != 0 {
            self.comments_disabled = false;
        }
        self.emit_trailing_synthetic_comments_of_node(node, emit_flags);
        self.emit_trailing_comments_of_node(
            kind,
            emit_flags,
            comment_range,
            container_pos,
            container_end,
            declaration_list_container_end,
        );

        // Preserve comments from erased type annotation
        if let Some(type_node) = node.and_then(|node| self.printer.emit_context.get_type_node(node))
        {
            let type_range = self.node(type_node)?.range();
            self.emit_trailing_comments_of_node(
                kind,
                emit_flags,
                type_range,
                container_pos,
                container_end,
                declaration_list_container_end,
            );
        }
        Ok(())
    }

    // port: tsc/internal/printer/printer.go:Printer.emitCommentsBeforeToken
    fn emit_comments_before_token(
        &mut self,
        _token: K,
        mut pos: i64,
        context: Option<NodeId>,
        flags: tef::TokenEmitFlags,
    ) -> Result<(Option<CommentState>, i64), Error> {
        if flags & tef::NO_COMMENTS != 0 || self.comments_disabled {
            // Still skip trivia so that the returned pos correctly identifies the token position.
            if let Some(text) = self.source_text_bytes() {
                if !position_is_synthesized(pos) {
                    pos = tsr_scanner::skip_trivia(text, pos);
                }
            }
            return Ok((None, pos));
        }

        let start_pos = pos;
        if let Some(text) = self.source_text_bytes() {
            pos = tsr_scanner::skip_trivia(text, start_pos);
        }

        // A node the printer creates has no parse-tree original.
        let Some(context) = context else {
            return Ok((None, pos));
        };
        let context_read = self.node(context)?;
        let is_similar_node = match self.parse_node(context) {
            Some(node) => self.node(node)?.kind() == context_read.kind(),
            None => false,
        };
        if !is_similar_node {
            return Ok((None, pos));
        }

        if i64::from(context_read.pos()) != start_pos {
            let indent_leading = flags & tef::INDENT_LEADING_COMMENTS != 0;
            let needs_indent = indent_leading
                && self.current_source.is_some()
                && !self.positions_are_on_same_line(start_pos, pos);
            self.increase_indent_if(needs_indent);
            self.emit_leading_comments(start_pos, false);
            self.decrease_indent_if(needs_indent);
        }
        Ok((Some(CommentState::zero()), pos))
    }

    // port: tsc/internal/printer/printer.go:Printer.emitCommentsAfterToken
    fn emit_comments_after_token(
        &mut self,
        _token: K,
        pos: i64,
        context: Option<NodeId>,
        state: Option<CommentState>,
    ) -> Result<(), Error> {
        if state.is_none() {
            return Ok(());
        }
        let context = context.expect("a token comment state has a context node");
        let context_read = self.node(context)?;
        if i64::from(context_read.end()) != pos {
            let is_jsx_expr_context = context_read.kind() == K::JsxExpression;
            self.emit_trailing_comments(
                pos,
                if is_jsx_expr_context {
                    CommentSeparator::None
                } else {
                    CommentSeparator::Before
                },
            );
        }
        Ok(())
    }

    // port: tsc/internal/printer/printer.go:Printer.emitDetachedCommentsBeforeStatementList
    pub(crate) fn emit_detached_comments_before_statement_list(
        &mut self,
        node: NodeId,
        detached_range: TextRange,
    ) -> Result<Option<CommentState>, Error> {
        if !self.should_emit_detached_comments(node)? {
            return Ok(None);
        }
        let emit_flags = self.emit_flags(node);
        let container_pos = self.container_pos;
        let container_end = self.container_end;
        let declaration_list_container_end = self.declaration_list_container_end;
        let skip_leading_comments = position_is_synthesized(detached_range.pos())
            || emit_flags & ef::NO_LEADING_COMMENTS != 0;
        if !skip_leading_comments {
            self.emit_detached_comments_and_update_comments_info(detached_range);
        }
        if emit_flags & ef::NO_NESTED_COMMENTS != 0 {
            self.comments_disabled = true;
        }
        Ok(Some(CommentState {
            emit_flags,
            comment_range: detached_range,
            container_pos,
            container_end,
            declaration_list_container_end,
        }))
    }

    // port: tsc/internal/printer/printer.go:Printer.emitDetachedCommentsAfterStatementList
    pub(crate) fn emit_detached_comments_after_statement_list(
        &mut self,
        _node: NodeId,
        detached_range: TextRange,
        state: Option<CommentState>,
    ) {
        let Some(state) = state else {
            return;
        };
        let emit_flags = state.emit_flags;
        let skip_trailing_comments = self.comments_disabled
            || position_is_synthesized(detached_range.end())
            || emit_flags & ef::NO_TRAILING_COMMENTS != 0;
        if !skip_trailing_comments {
            let has_written_comment = self.emit_leading_comments(detached_range.end(), false);
            if has_written_comment && !self.writer.is_at_start_of_line() {
                self.write_line();
            }
        }
    }

    // port: tsc/internal/printer/printer.go:Printer.emitLeadingCommentsOfNode
    fn emit_leading_comments_of_node(
        &mut self,
        kind: NodeKind,
        emit_flags: EmitFlags,
        comment_range: TextRange,
    ) {
        let pos = comment_range.pos();
        let end = comment_range.end();

        // Save current container state on the stack.
        if (!position_is_synthesized(pos) || !position_is_synthesized(end)) && pos != end {
            // JsxText is checked explicitly: with jsx "preserve" nothing
            // transforms it, and walking the tree to flag it is expensive.
            let skip_leading_comments = position_is_synthesized(pos)
                || emit_flags & ef::NO_LEADING_COMMENTS != 0
                || kind == K::JsxText;
            let skip_trailing_comments = position_is_synthesized(end)
                || emit_flags & ef::NO_TRAILING_COMMENTS != 0
                || kind == K::JsxText;

            // Emit leading comments if the position is not synthesized and the node
            // has not opted out from emitting leading comments.
            if !skip_leading_comments {
                self.emit_leading_comments(pos, kind == K::NotEmittedStatement);
            }

            if !skip_leading_comments || (pos >= 0 && emit_flags & ef::NO_LEADING_COMMENTS != 0) {
                // Advance the container position if comments get emitted or if they've been disabled explicitly using NoLeadingComments.
                self.container_pos = pos;
            }

            if !skip_trailing_comments || (end >= 0 && emit_flags & ef::NO_TRAILING_COMMENTS != 0) {
                // Advance the container end if comments get emitted or if they've been disabled explicitly using NoTrailingComments.
                self.container_end = end;

                // To avoid invalid comment emit in a down-level binding pattern, we
                // keep track of the last declaration list container's end
                if kind == K::VariableDeclarationList {
                    self.declaration_list_container_end = end;
                }
            }
        }
    }

    // port: tsc/internal/printer/printer.go:Printer.emitTrailingCommentsOfNode
    fn emit_trailing_comments_of_node(
        &mut self,
        kind: NodeKind,
        emit_flags: EmitFlags,
        comment_range: TextRange,
        container_pos: i64,
        container_end: i64,
        declaration_list_container_end: i64,
    ) {
        let pos = comment_range.pos();
        let end = comment_range.end();
        let skip_trailing_comments =
            end < 0 || emit_flags & ef::NO_TRAILING_COMMENTS != 0 || kind == K::JsxText;
        if (!position_is_synthesized(pos) || !position_is_synthesized(end)) && pos != end {
            // Restore previous container state.
            self.container_pos = container_pos;
            self.container_end = container_end;
            self.declaration_list_container_end = declaration_list_container_end;

            // Emit trailing comments if the position is not synthesized and the node
            // has not opted out from emitting leading comments and is an emitted node.
            if !skip_trailing_comments && kind != K::NotEmittedStatement {
                self.emit_trailing_comments(end, CommentSeparator::Before);
            }
        }
    }

    // port: tsc/internal/printer/printer.go:Printer.emitLeadingSyntheticCommentsOfNode
    fn emit_leading_synthetic_comments_of_node(
        &mut self,
        node: Option<NodeId>,
        emit_flags: EmitFlags,
    ) {
        if emit_flags & ef::NO_LEADING_COMMENTS != 0 {
            return;
        }
        let Some(node) = node else {
            return;
        };
        for comment in self.printer.emit_context.synthetic_leading_comments(node) {
            self.emit_leading_synthesized_comment(&comment);
        }
    }

    // port: tsc/internal/printer/printer.go:Printer.emitLeadingSynthesizedComment
    fn emit_leading_synthesized_comment(&mut self, comment: &SynthesizedComment) {
        if comment.has_leading_new_line || comment.kind == K::SingleLineCommentTrivia {
            self.writer.write_line();
        }
        self.write_synthesized_comment(comment);
        if comment.has_trailing_new_line || comment.kind == K::SingleLineCommentTrivia {
            self.writer.write_line();
        } else {
            self.writer.write_space(b" ");
        }
    }

    // port: tsc/internal/printer/printer.go:Printer.emitTrailingSyntheticCommentsOfNode
    fn emit_trailing_synthetic_comments_of_node(
        &mut self,
        node: Option<NodeId>,
        emit_flags: EmitFlags,
    ) {
        if emit_flags & ef::NO_TRAILING_COMMENTS != 0 {
            return;
        }
        let Some(node) = node else {
            return;
        };
        for comment in self.printer.emit_context.synthetic_trailing_comments(node) {
            self.emit_trailing_synthesized_comment(&comment);
        }
    }

    // port: tsc/internal/printer/printer.go:Printer.emitTrailingSynthesizedComment
    fn emit_trailing_synthesized_comment(&mut self, comment: &SynthesizedComment) {
        if !self.writer.is_at_start_of_line() {
            self.writer.write_space(b" ");
        }
        self.write_synthesized_comment(comment);
        if comment.has_trailing_new_line {
            self.writer.write_line();
        }
    }

    // port: tsc/internal/printer/printer.go:Printer.writeSynthesizedComment
    fn write_synthesized_comment(&mut self, comment: &SynthesizedComment) {
        let text = format_synthesized_comment(comment);
        let line_map = if comment.kind == K::MultiLineCommentTrivia {
            tsr_jsstring::line_map::compute_ecma_line_starts(&text)
        } else {
            Vec::new()
        };
        write_comment_range_worker(
            &mut *self.writer,
            &text,
            &line_map,
            comment.kind,
            TextRange::new(0, text.len() as i64),
        );
    }

    /// Returns whether a comment was written.
    // port: tsc/internal/printer/printer.go:Printer.emitLeadingComments
    pub(crate) fn emit_leading_comments(&mut self, mut pos: i64, elided: bool) -> bool {
        // Emit the leading comments only if the container's pos doesn't match because the container should take care of emitting these comments
        if self.comments_disabled
            || self.current_source.is_none()
            || position_is_synthesized(pos)
            || pos == self.container_pos
        {
            return false;
        }

        let mut triple_slash = TripleSlash::Unknown;
        if !elided {
            if pos == 0
                && self
                    .current_source
                    .as_ref()
                    .is_some_and(|(_, source)| source.is_declaration_file)
            {
                triple_slash = TripleSlash::False;
            }
        } else if pos == 0 {
            // A node that is not emitted in JS drops all its comments except
            // a triple-slash comment at the top of the file.
            triple_slash = TripleSlash::True;
        } else {
            return false;
        }

        // skip detached comments
        if let Some(info) = self
            .detached_comments_info
            .pop_if(|info| info.node_pos == pos)
        {
            pos = info.detached_comment_end_pos;
        }

        let text = self.source_text_bytes().expect("checked above");
        let comments: Vec<CommentRange> = tsr_scanner::get_leading_comment_ranges(text, pos)
            .filter(|comment| {
                self.should_write_comment(comment)
                    && self.should_emit_comment_if_triple_slash(comment, triple_slash)
            })
            .collect();

        if !comments.is_empty()
            && self
                .should_emit_new_line_before_leading_comment_of_position(pos, comments[0].loc.pos())
        {
            self.write_line();
        }

        // Leading comments are emitted as /*leading comment1*/space/*leading comment*/space
        self.emit_comments(&comments, CommentSeparator::After)
    }

    // port: tsc/internal/printer/printer.go:Printer.shouldEmitCommentIfTripleSlash
    fn should_emit_comment_if_triple_slash(
        &self,
        comment: &CommentRange,
        triple_slash: TripleSlash,
    ) -> bool {
        match triple_slash {
            TripleSlash::True => self.is_triple_slash_comment(comment),
            TripleSlash::False => !self.is_triple_slash_comment(comment),
            TripleSlash::Unknown => true,
        }
    }

    /// The leading comments start on another line than the position.
    // port: tsc/internal/printer/printer.go:Printer.shouldEmitNewLineBeforeLeadingCommentOfPosition
    fn should_emit_new_line_before_leading_comment_of_position(
        &self,
        pos: i64,
        comment_pos: i64,
    ) -> bool {
        self.current_source.is_some()
            && pos != comment_pos
            && self.line_of(pos) != self.line_of(comment_pos)
    }

    // port: tsc/internal/printer/printer.go:Printer.emitLeadingCommentsOfPosition
    pub(crate) fn emit_leading_comments_of_position(&mut self, pos: i64) {
        if self.comments_disabled || pos == -1 {
            return;
        }
        self.emit_leading_comments(pos, false);
    }

    // port: tsc/internal/printer/printer.go:Printer.emitTrailingComments
    pub(crate) fn emit_trailing_comments(&mut self, pos: i64, separator: CommentSeparator) {
        if self.comments_disabled {
            return;
        }
        // Emit the trailing comments only if the container's end doesn't match because the container should take care of emitting these comments
        if self.comments_disabled
            || self.current_source.is_none()
            || self.container_end != -1
                && (pos == self.container_end || pos == self.declaration_list_container_end)
        {
            return;
        }
        let text = self.source_text_bytes().expect("checked above");
        let comments: Vec<CommentRange> = tsr_scanner::get_trailing_comment_ranges(text, pos)
            .filter(|comment| self.should_write_comment(comment))
            .collect();
        // trailing comments are normally emitted as space/*trailing comment1*/space/*trailing comment2*/
        self.emit_comments(&comments, separator);
    }

    // port: tsc/internal/printer/printer.go:Printer.emitTrailingCommentsOfPosition
    pub(crate) fn emit_trailing_comments_of_position(
        &mut self,
        pos: i64,
        prefix_space: bool,
        force_no_newline: bool,
    ) {
        if self.comments_disabled || self.current_source.is_none() {
            return;
        }
        if self.container_end != -1
            && (pos == self.container_end || pos == self.declaration_list_container_end)
        {
            return;
        }
        let text = self.source_text_bytes().expect("checked above");
        let comments: Vec<CommentRange> =
            tsr_scanner::get_trailing_comment_ranges(text, pos).collect();
        if comments.is_empty() {
            return;
        }
        for comment in &comments {
            if prefix_space {
                if !self.should_write_comment(comment) {
                    continue;
                }
                if !self.writer.is_at_start_of_line() {
                    self.write_space();
                }
                self.emit_comment(comment);
                if comment.has_trailing_new_line {
                    self.write_line();
                }
                continue;
            }
            self.emit_comment(comment);
            if force_no_newline {
                if comment.kind == K::SingleLineCommentTrivia {
                    self.write_line();
                }
            } else if comment.has_trailing_new_line {
                self.write_line();
            } else {
                self.write_space();
            }
        }
    }

    // port: tsc/internal/printer/printer.go:Printer.emitDetachedCommentsAndUpdateCommentsInfo
    fn emit_detached_comments_and_update_comments_info(&mut self, text_range: TextRange) {
        if self.current_source.is_none() {
            return;
        }
        if let Some(info) = self.emit_detached_comments(text_range) {
            self.detached_comments_info.push(info);
        }
    }

    /// Leading comments that look like a copyright header: a run of comments
    /// with no blank line between them, followed by a blank line before the
    /// node.
    // port: tsc/internal/printer/printer.go:Printer.emitDetachedComments
    fn emit_detached_comments(&mut self, text_range: TextRange) -> Option<DetachedCommentsInfo> {
        let text = self.source_text_bytes()?;
        let leading_comments: Vec<CommentRange> = if self.comments_disabled {
            // removeComments is true, only reserve pinned comment at the top of file
            if text_range.pos() == 0 {
                tsr_scanner::get_leading_comment_ranges(text, text_range.pos())
                    .filter(|comment| is_pinned_comment(text, comment))
                    .collect()
            } else {
                Vec::new()
            }
        } else {
            // removeComments is false, just get detached as normal and bypass the process to filter comment
            tsr_scanner::get_leading_comment_ranges(text, text_range.pos()).collect()
        };

        if leading_comments.is_empty() {
            return None;
        }
        let mut detached_comments: Vec<CommentRange> = Vec::new();
        let mut last_comment: Option<CommentRange> = None;
        for (i, comment) in leading_comments.iter().enumerate() {
            if i > 0 {
                let last = last_comment.expect("set on the first iteration");
                let last_comment_line = self.line_of(last.loc.end());
                let comment_line = self.line_of(comment.loc.pos());
                if comment_line >= last_comment_line + 2 {
                    // There was a blank line between the last comment and this comment.  This
                    // comment is not part of the copyright comments.  Return what we have so
                    // far.
                    break;
                }
            }
            detached_comments.push(*comment);
            last_comment = Some(*comment);
        }

        let last = *detached_comments.last()?;
        // All comments look like they could have been part of the copyright header.  Make
        // sure there is at least one blank line between it and the node.  If not, it's not
        // a copyright header.
        let last_comment_line = self.line_of(last.loc.end());
        let node_line = self.line_of(tsr_scanner::skip_trivia(text, text_range.pos()));
        if node_line < last_comment_line + 2 {
            return None;
        }
        // Filter to only comments that should be written (e.g., JSDoc-style in declaration emit)
        let comments_to_emit: Vec<CommentRange> = detached_comments
            .iter()
            .filter(|comment| self.should_write_comment(comment))
            .copied()
            .collect();
        if !comments_to_emit.is_empty() {
            if self.should_emit_new_line_before_leading_comment_of_position(
                text_range.pos(),
                comments_to_emit[0].loc.pos(),
            ) {
                self.write_line();
            }
            self.emit_comments(&comments_to_emit, CommentSeparator::After);
        }
        Some(DetachedCommentsInfo {
            node_pos: text_range.pos(),
            detached_comment_end_pos: last.loc.end(),
        })
    }

    /// Returns whether any comment was given.
    // port: tsc/internal/printer/printer.go:Printer.emitComments
    fn emit_comments(&mut self, comments: &[CommentRange], separator: CommentSeparator) -> bool {
        let mut intervening_separator = false;
        if comments.is_empty() {
            return false;
        }
        if separator == CommentSeparator::Before {
            self.write_space();
        }
        for comment in comments {
            if intervening_separator {
                self.write_space();
                intervening_separator = false;
            }
            self.emit_comment(comment);
            if comment.kind == K::SingleLineCommentTrivia
                || comment.has_trailing_new_line && separator != CommentSeparator::None
            {
                self.write_line();
            } else {
                intervening_separator = separator != CommentSeparator::None;
            }
        }
        if intervening_separator && separator == CommentSeparator::After {
            self.write_space();
        }
        true
    }

    // port: tsc/internal/printer/printer.go:Printer.emitComment
    fn emit_comment(&mut self, comment: &CommentRange) {
        self.emit_pos(comment.loc.pos());
        self.write_comment_range(comment);
        self.emit_pos(comment.loc.end());
    }

    // port: tsc/internal/printer/printer.go:Printer.isTripleSlashComment
    fn is_triple_slash_comment(&self, comment: &CommentRange) -> bool {
        self.source_text_bytes()
            .is_some_and(|text| is_recognized_triple_slash_comment(text, comment))
    }

    // port: tsc/internal/printer/printer.go:Printer.commentWillEmitNewLine
    fn comment_will_emit_new_line(comment: &CommentRange) -> bool {
        comment.kind == K::SingleLineCommentTrivia || comment.has_trailing_new_line
    }

    // port: tsc/internal/printer/printer.go:Printer.syntheticCommentWillEmitNewLine
    fn synthetic_comment_will_emit_new_line(comment: &SynthesizedComment) -> bool {
        comment.kind == K::SingleLineCommentTrivia || comment.has_trailing_new_line
    }

    // port: tsc/internal/printer/printer.go:Printer.willEmitLeadingNewLine
    pub(crate) fn will_emit_leading_new_line(&self, node: NodeId) -> Result<bool, Error> {
        let Some(text) = self.source_text_bytes() else {
            return Ok(false);
        };
        let read = self.node(node)?;
        let mut has_leading_comment_ranges = false;
        let mut has_new_line_comment = false;
        for comment in tsr_scanner::get_leading_comment_ranges(text, i64::from(read.pos())) {
            has_leading_comment_ranges = true;
            if Self::comment_will_emit_new_line(&comment) {
                has_new_line_comment = true;
            }
        }
        if has_leading_comment_ranges {
            if let Some(parse_node) = self.parse_node(node) {
                let parent = self.node(parse_node)?.parent();
                if let Some(parent) = parent {
                    if self.node(parent)?.kind() == K::ParenthesizedExpression {
                        return Ok(true);
                    }
                }
            }
        }
        if has_new_line_comment {
            return Ok(true);
        }
        if self
            .printer
            .emit_context
            .synthetic_leading_comments(node)
            .iter()
            .any(Self::synthetic_comment_will_emit_new_line)
        {
            return Ok(true);
        }
        if read.kind() == K::PartiallyEmittedExpression {
            let expression = read
                .expression()
                .ok_or(Error::MissingNode("partially emitted expression"))?;
            let expression_pos = i64::from(self.node(expression)?.pos());
            if i64::from(read.pos()) != expression_pos {
                for comment in tsr_scanner::get_trailing_comment_ranges(text, expression_pos) {
                    if Self::comment_will_emit_new_line(&comment) {
                        return Ok(true);
                    }
                }
            }
            return self.will_emit_leading_new_line(expression);
        }
        Ok(false)
    }

    //
    // Source maps (boundary)
    //

    /// Boundary: positions reach a source-map generator, which `Write` does not
    /// take yet. With maps disabled this returns before any generator call, as
    /// upstream does.
    fn emit_pos(&mut self, pos: i64) {
        if self.source_maps_disabled || position_is_synthesized(pos) {
            return;
        }
        if let Some(source) = self.source_map_source {
            match source {}
        }
    }

    /// Boundary: `emitSourceMapsBeforeNode` past its guard needs the generator.
    fn emit_source_maps_before_node(
        &mut self,
        node: Option<NodeId>,
    ) -> Result<Option<SourceMapState>, Error> {
        if !self.should_emit_source_maps(node)? {
            return Ok(None);
        }
        Err(Error::Unsupported("source-map emission"))
    }

    /// Boundary: no source-map state exists to finish.
    fn emit_source_maps_after_node(state: Option<SourceMapState>) {
        if let Some(state) = state {
            match state {}
        }
    }

    /// Boundary: `emitSourceMapsBeforeToken` past its guard needs the
    /// generator.
    fn emit_source_maps_before_token(
        &mut self,
        token: K,
        pos: i64,
        context: Option<NodeId>,
        flags: tef::TokenEmitFlags,
    ) -> Result<Option<SourceMapState>, Error> {
        if !self.should_emit_token_source_maps(token, pos, context, flags)? {
            return Ok(None);
        }
        Err(Error::Unsupported("source-map emission"))
    }

    /// Boundary: no source-map state exists to finish.
    fn emit_source_maps_after_token(state: Option<SourceMapState>) {
        if let Some(state) = state {
            match state {}
        }
    }

    //
    // Scoped operations
    //

    // port: tsc/internal/printer/printer.go:Printer.enterNode
    pub(crate) fn enter_node(&mut self, node: NodeId) -> Result<PrinterState, Error> {
        self.writer.on_before_emit_node(node);
        let comment_state = self.emit_comments_before_node(node)?;
        let source_map_state = self.emit_source_maps_before_node(Some(node))?;
        Ok(PrinterState {
            comment_state,
            source_map_state,
        })
    }

    // port: tsc/internal/printer/printer.go:Printer.exitNode
    pub(crate) fn exit_node(&mut self, node: NodeId, state: PrinterState) -> Result<(), Error> {
        Self::emit_source_maps_after_node(state.source_map_state);
        if state.comment_state.is_some() {
            let kind = self.node(node)?.kind();
            let identity = self.comment_identity(node);
            self.emit_comments_after_node(identity, kind, state.comment_state)?;
        }
        self.writer.on_after_emit_node(node);
        Ok(())
    }

    /// `enterNode` for a node the printer creates itself. It has no identity
    /// to report to the emit notifications.
    pub(crate) fn enter_created_node(
        &mut self,
        target: &CommentTarget,
    ) -> Result<PrinterState, Error> {
        let comment_state = if self.should_emit_comments_of_kind(target.kind) {
            Some(self.emit_comments_before_target(target))
        } else {
            None
        };
        let source_map_state = self.emit_source_maps_before_node(None)?;
        Ok(PrinterState {
            comment_state,
            source_map_state,
        })
    }

    /// `exitNode` for a node the printer creates itself.
    pub(crate) fn exit_created_node(
        &mut self,
        target: &CommentTarget,
        state: PrinterState,
    ) -> Result<(), Error> {
        Self::emit_source_maps_after_node(state.source_map_state);
        self.emit_comments_after_node(target.node, target.kind, state.comment_state)
    }

    // port: tsc/internal/printer/printer.go:Printer.enterTokenNode
    pub(crate) fn enter_token_node(
        &mut self,
        node: NodeId,
        flags: tef::TokenEmitFlags,
    ) -> Result<PrinterState, Error> {
        let mut state = PrinterState::default();
        self.writer.on_before_emit_token(node);
        if flags & tef::NO_COMMENTS == 0 {
            state.comment_state = self.emit_comments_before_node(node)?;
        }
        if flags & tef::NO_SOURCE_MAPS == 0 {
            state.source_map_state = self.emit_source_maps_before_node(Some(node))?;
        }
        Ok(state)
    }

    // port: tsc/internal/printer/printer.go:Printer.exitTokenNode
    pub(crate) fn exit_token_node(
        &mut self,
        node: NodeId,
        state: PrinterState,
    ) -> Result<(), Error> {
        Self::emit_source_maps_after_node(state.source_map_state);
        if state.comment_state.is_some() {
            let kind = self.node(node)?.kind();
            let identity = self.comment_identity(node);
            self.emit_comments_after_node(identity, kind, state.comment_state)?;
        }
        self.writer.on_after_emit_token(node);
        Ok(())
    }

    // port: tsc/internal/printer/printer.go:Printer.enterToken
    pub(crate) fn enter_token(
        &mut self,
        token: K,
        pos: i64,
        context: Option<NodeId>,
        flags: tef::TokenEmitFlags,
    ) -> Result<(PrinterState, i64), Error> {
        let mut state = PrinterState::default();
        let (comment_state, pos) = self.emit_comments_before_token(token, pos, context, flags)?;
        state.comment_state = comment_state;
        state.source_map_state = self.emit_source_maps_before_token(token, pos, context, flags)?;
        Ok((state, pos))
    }

    // port: tsc/internal/printer/printer.go:Printer.exitToken
    pub(crate) fn exit_token(
        &mut self,
        token: K,
        pos: i64,
        context: Option<NodeId>,
        state: PrinterState,
    ) -> Result<(), Error> {
        Self::emit_source_maps_after_token(state.source_map_state);
        self.emit_comments_after_token(token, pos, context, state.comment_state)
    }
}
