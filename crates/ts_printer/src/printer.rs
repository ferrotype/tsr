//! The printer (`tsc/internal/printer/printer.go`), for the nodes type display
//! produces. Every function that emits a node kind is a port of the upstream
//! function of the same name and keeps its order of writes; helpers whose whole
//! purpose is comments or source maps reduce to nothing under the supported
//! configuration and are named where they would sit.
//!
//! A `Printer` holds options and the emit context. Each `write` opens a
//! [`Session`] over one AST view and one writer; nothing about a node is cached
//! across sessions.

use crate::emit_flags as ef;
use crate::list_format as lf;
use crate::literal_text::{with_flag, LiteralTextFlags};
use crate::{
    get_type_node_precedence, EmitContext, EmitTextWriter, Error, ListFormat, TextWriter,
    TrailingSemicolonDeferringWriter, TypePrecedence,
};
use std::collections::HashMap;
use ts_arena::SymbolId;
use ts_ast::operator_precedence as op;
use ts_ast::{
    node_flags, token_flags, AstView, NodeId, NodeKind, NodeListId, NodeRead, SourceFileRead,
    SyntaxKind as K,
};
use ts_core::{NewLineKind, ScriptTarget};
use ts_jsstring::LiteralEscapeFlags;
use ts_scanner::token_to_string;

/// `PrinterOptions`. Members upstream keeps for source maps and helpers that
/// this port does not consume are omitted; the supported ones keep their names.
#[derive(Clone, Debug, Default)]
pub struct PrinterOptions {
    pub remove_comments: bool,
    pub new_line: NewLineKind,
    pub omit_trailing_semicolon: bool,
    pub target: ScriptTarget,
    pub never_ascii_escape: bool,
    pub preserve_source_newlines: bool,
    pub terminate_unterminated_literals: bool,
}

/// `WriteKind`: which writer method a piece of text goes through.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WriteKind {
    None,
    Keyword,
    Operator,
    Punctuation,
    StringLiteral,
    Parameter,
    Property,
    Comment,
    Literal,
}

pub struct Printer<'c> {
    pub options: PrinterOptions,
    pub(crate) emit_context: &'c EmitContext,
    /// Identifiers to report through `write_symbol` (`Printer.IdToSymbol`).
    pub id_to_symbol: Option<HashMap<NodeId, SymbolId>>,
    own_writer: Option<TextWriter>,
}

fn new_line_character(kind: NewLineKind) -> &'static [u8] {
    if kind == NewLineKind::CRLF {
        b"\r\n"
    } else {
        b"\n"
    }
}

impl<'c> Printer<'c> {
    /// Handlers (name generation, emit notifications) are not taken: no caller in
    /// the supported set installs any.
    // port: tsc/internal/printer/printer.go:NewPrinter
    pub fn new(options: PrinterOptions, emit_context: &'c EmitContext) -> Self {
        Self {
            options,
            emit_context,
            id_to_symbol: None,
            own_writer: None,
        }
    }

    /// Prints into a reusable writer of the printer's own and returns the bytes.
    // port: tsc/internal/printer/printer.go:Printer.Emit
    pub fn emit(
        &mut self,
        view: AstView<'_>,
        node: NodeId,
        source_file: Option<NodeId>,
    ) -> Result<Vec<u8>, Error> {
        let new_line = new_line_character(self.options.new_line);
        let mut writer = self
            .own_writer
            .take()
            .unwrap_or_else(|| TextWriter::new(new_line, 0));
        let result = self.write(view, node, source_file, &mut writer);
        let text = writer.text().to_vec();
        writer.clear();
        self.own_writer = Some(writer);
        result.map(|()| text)
    }

    /// Prints one node through `writer`. `source_file` is the file whose text
    /// parsed literals and identifiers may be copied from, as upstream's
    /// `currentSourceFile`; comments are never emitted, so it must come with
    /// `remove_comments`. Source maps are not produced.
    // port: tsc/internal/printer/printer.go:Printer.Write
    pub fn write(
        &self,
        view: AstView<'_>,
        node: NodeId,
        source_file: Option<NodeId>,
        writer: &mut dyn EmitTextWriter,
    ) -> Result<(), Error> {
        if source_file.is_some() && !self.options.remove_comments {
            return Err(Error::Unsupported("comment emission"));
        }
        if self.options.preserve_source_newlines {
            return Err(Error::Unsupported("PreserveSourceNewlines"));
        }
        let current_source = match source_file {
            Some(file) => Some((file, view.source_file(file)?)),
            None => None,
        };
        let mut deferring;
        let writer: &mut dyn EmitTextWriter = if self.options.omit_trailing_semicolon {
            deferring = TrailingSemicolonDeferringWriter::new(writer);
            &mut deferring
        } else {
            writer
        };
        writer.clear();
        let mut session = Session {
            printer: self,
            view,
            writer,
            current_source,
            line_starts: std::cell::OnceCell::new(),
            write_kind: WriteKind::None,
            in_extends: false,
            next_list_element_pos: 0,
        };
        session.write_root(node)
    }
}

/// One `Printer.Write` in progress.
pub(crate) struct Session<'a, 'c> {
    pub(crate) printer: &'a Printer<'c>,
    pub(crate) view: AstView<'a>,
    writer: &'a mut dyn EmitTextWriter,
    pub(crate) current_source: Option<(NodeId, SourceFileRead<'a>)>,
    line_starts: std::cell::OnceCell<Vec<i32>>,
    write_kind: WriteKind,
    in_extends: bool,
    next_list_element_pos: i64,
}

/// A node position for line comparisons; synthesized tokens upstream creates
/// on the fly are represented by their range without a node.
#[derive(Clone, Copy)]
struct Span {
    node: Option<NodeId>,
    pos: i64,
    end: i64,
}

impl Span {
    fn of(node: &NodeRead<'_>) -> Self {
        Self {
            node: Some(node.id()),
            pos: i64::from(node.pos()),
            end: i64::from(node.end()),
        }
    }
    fn synthesized(self) -> bool {
        position_is_synthesized(self.pos) || position_is_synthesized(self.end)
    }
}

fn position_is_synthesized(position: i64) -> bool {
    position < 0
}

fn is_punctuation_kind(kind: K) -> bool {
    (K::FirstPunctuation as u16..=K::LastPunctuation as u16).contains(&(kind as u16))
}

fn is_keyword_kind(kind: K) -> bool {
    (K::FirstKeyword as u16..=K::LastKeyword as u16).contains(&(kind as u16))
}

fn is_type_node_kind(kind: K) -> bool {
    (K::FirstTypeNode as u16..=K::LastTypeNode as u16).contains(&(kind as u16))
        || matches!(
            kind,
            K::AnyKeyword
                | K::UnknownKeyword
                | K::NumberKeyword
                | K::BigIntKeyword
                | K::ObjectKeyword
                | K::BooleanKeyword
                | K::StringKeyword
                | K::SymbolKeyword
                | K::VoidKeyword
                | K::UndefinedKeyword
                | K::NeverKeyword
                | K::IntrinsicKeyword
                | K::ExpressionWithTypeArguments
                | K::JSDocAllType
                | K::JSDocNullableType
                | K::JSDocNonNullableType
                | K::JSDocOptionalType
                | K::JSDocVariadicType
        )
}

// port: tsc/internal/printer/utilities.go:greatestEnd
fn greatest_end(end: i64, ends: &[Option<i64>]) -> i64 {
    ends.iter()
        .flatten()
        .fold(end, |acc, value| acc.max(*value))
}

impl<'a> Session<'a, '_> {
    pub(crate) fn node(&self, id: NodeId) -> Result<NodeRead<'a>, Error> {
        Ok(self.view.node(id)?)
    }

    fn known_kind(&self, id: NodeId) -> Result<K, Error> {
        let read = self.node(id)?;
        read.kind().known().ok_or(Error::UnexpectedKind {
            context: "node kind",
            kind: read.kind(),
        })
    }

    fn list_nodes(&self, list: NodeListId) -> Result<Vec<NodeId>, Error> {
        let slice = self.view.list(list)?.nodes();
        let read = self.view.node_slice(slice)?;
        read.iter()
            .map(|entry| entry.ok_or(Error::MissingNode("list entry")))
            .collect()
    }

    fn list_len(&self, list: Option<NodeListId>) -> Result<usize, Error> {
        match list {
            Some(list) => Ok(self.view.list(list)?.nodes().len()),
            None => Ok(0),
        }
    }

    fn emit_flags(&self, node: NodeId) -> u32 {
        self.printer.emit_context.emit_flags(node)
    }

    //
    // Low-level writing
    //

    // port: tsc/internal/printer/printer.go:Printer.writeAs
    fn write_as(&mut self, text: &[u8], write_kind: WriteKind) {
        match write_kind {
            WriteKind::None => self.writer.write(text),
            WriteKind::Parameter => self.writer.write_parameter(text),
            WriteKind::Keyword => self.writer.write_keyword(text),
            WriteKind::Operator => self.writer.write_operator(text),
            WriteKind::Property => self.writer.write_property(text),
            WriteKind::Punctuation => self.writer.write_punctuation(text),
            WriteKind::StringLiteral => self.writer.write_string_literal(text),
            WriteKind::Comment => self.writer.write_comment(text),
            WriteKind::Literal => self.writer.write_literal(text),
        }
    }

    // port: tsc/internal/printer/printer.go:Printer.write
    fn write(&mut self, text: &[u8]) {
        self.write_as(text, self.write_kind);
    }

    // port: tsc/internal/printer/printer.go:Printer.writeSymbol
    fn write_symbol(&mut self, text: &[u8], symbol: Option<SymbolId>) {
        match symbol {
            None => self.write(text),
            Some(symbol) => self.writer.write_symbol(text, Some(symbol)),
        }
    }

    // port: tsc/internal/printer/printer.go:Printer.writePunctuation
    fn write_punctuation(&mut self, text: &[u8]) {
        self.writer.write_punctuation(text);
    }

    // port: tsc/internal/printer/printer.go:Printer.writeOperator
    fn write_operator(&mut self, text: &[u8]) {
        self.writer.write_operator(text);
    }

    // port: tsc/internal/printer/printer.go:Printer.writeKeyword
    fn write_keyword(&mut self, text: &[u8]) {
        self.writer.write_keyword(text);
    }

    // port: tsc/internal/printer/printer.go:Printer.writeSpace
    fn write_space(&mut self) {
        self.writer.write_space(b" ");
    }

    // port: tsc/internal/printer/printer.go:Printer.writeLine
    fn write_line(&mut self) {
        self.writer.write_line();
    }

    // port: tsc/internal/printer/printer.go:Printer.writeLineRepeat
    fn write_line_repeat(&mut self, count: i64) {
        for _ in 0..count {
            self.write_line();
        }
    }

    // port: tsc/internal/printer/printer.go:Printer.writeTrailingSemicolon
    fn write_trailing_semicolon(&mut self) {
        self.writer.write_trailing_semicolon(b";");
    }

    // port: tsc/internal/printer/printer.go:Printer.increaseIndent
    fn increase_indent(&mut self) {
        self.writer.increase_indent();
    }

    // port: tsc/internal/printer/printer.go:Printer.decreaseIndent
    fn decrease_indent(&mut self) {
        self.writer.decrease_indent();
    }

    // port: tsc/internal/printer/printer.go:Printer.increaseIndentIf
    fn increase_indent_if(&mut self, requested: bool) {
        if requested {
            self.increase_indent();
        }
    }

    // port: tsc/internal/printer/printer.go:Printer.decreaseIndentIf
    fn decrease_indent_if(&mut self, requested: bool) {
        if requested {
            self.decrease_indent();
        }
    }

    // port: tsc/internal/printer/printer.go:Printer.shouldEmitIndented
    fn should_emit_indented(&self, node: NodeId) -> bool {
        self.emit_flags(node) & ef::INDENTED != 0
    }

    // port: tsc/internal/printer/printer.go:Printer.shouldElideIndentation
    fn should_elide_indentation(&self, node: NodeId) -> bool {
        self.emit_flags(node) & ef::NO_INDENTATION != 0
    }

    // port: tsc/internal/printer/printer.go:Printer.shouldEmitOnSingleLine
    fn should_emit_on_single_line(&self, node: NodeId) -> bool {
        self.emit_flags(node) & ef::SINGLE_LINE != 0
    }

    // port: tsc/internal/printer/printer.go:Printer.shouldEmitOnMultipleLines
    fn should_emit_on_multiple_lines(&self, node: NodeId) -> bool {
        self.emit_flags(node) & ef::MULTI_LINE != 0
    }

    // port: tsc/internal/printer/printer.go:Printer.shouldEmitOnNewLine
    fn should_emit_on_new_line(&self, node: Option<NodeId>, format: ListFormat) -> bool {
        if node.is_some_and(|node| self.emit_flags(node) & ef::START_ON_NEW_LINE != 0) {
            return true;
        }
        format & lf::PREFER_NEW_LINE != 0
    }

    //
    // Line positions (the source-newline-preserving branches are a boundary)
    //

    pub(crate) fn source_text(&self) -> Option<&[u8]> {
        self.current_source
            .as_ref()
            .map(|(_, source)| source.text().as_bytes())
    }

    fn line_starts(&self) -> Option<&[i32]> {
        let text = self.source_text()?;
        Some(
            self.line_starts
                .get_or_init(|| ts_jsstring::line_map::compute_ecma_line_starts(text)),
        )
    }

    // port: tsc/internal/printer/utilities.go:getStartPositionOfRange
    fn get_start_position_of_range(&self, pos: i64) -> i64 {
        if position_is_synthesized(pos) {
            return -1;
        }
        let text = self
            .source_text()
            .expect("line comparisons need a source file");
        ts_scanner::skip_trivia(text, pos)
    }

    // port: tsc/internal/printer/utilities.go:GetLinesBetweenPositions
    fn get_lines_between_positions(&self, pos1: i64, pos2: i64) -> i64 {
        if pos1 == pos2 {
            return 0;
        }
        let line_starts = self
            .line_starts()
            .expect("line comparisons need a source file");
        let (lower, upper, negative) = if pos1 < pos2 {
            (pos1, pos2, false)
        } else {
            (pos2, pos1, true)
        };
        let lower_line =
            ts_jsstring::scanner_positions::compute_line_of_position(line_starts, lower as isize);
        let upper_line = lower_line
            + ts_jsstring::scanner_positions::compute_line_of_position(
                &line_starts[lower_line as usize..],
                upper as isize,
            );
        let lines = (upper_line - lower_line) as i64;
        if negative {
            -lines
        } else {
            lines
        }
    }

    // port: tsc/internal/printer/utilities.go:PositionsAreOnSameLine
    fn positions_are_on_same_line(&self, pos1: i64, pos2: i64) -> bool {
        self.get_lines_between_positions(pos1, pos2) == 0
    }

    // port: tsc/internal/printer/utilities.go:RangeIsOnSingleLine
    fn range_is_on_single_line(&self, range: Span) -> bool {
        self.range_start_is_on_same_line_as_range_end(range, range)
    }

    // port: tsc/internal/printer/utilities.go:RangeStartPositionsAreOnSameLine
    fn range_start_positions_are_on_same_line(&self, range1: Span, range2: Span) -> bool {
        self.positions_are_on_same_line(
            self.get_start_position_of_range(range1.pos),
            self.get_start_position_of_range(range2.pos),
        )
    }

    // port: tsc/internal/printer/utilities.go:rangeEndPositionsAreOnSameLine
    fn range_end_positions_are_on_same_line(&self, range1: Span, range2: Span) -> bool {
        self.positions_are_on_same_line(range1.end, range2.end)
    }

    // port: tsc/internal/printer/utilities.go:rangeStartIsOnSameLineAsRangeEnd
    fn range_start_is_on_same_line_as_range_end(&self, range1: Span, range2: Span) -> bool {
        self.positions_are_on_same_line(self.get_start_position_of_range(range1.pos), range2.end)
    }

    // port: tsc/internal/printer/utilities.go:rangeEndIsOnSameLineAsRangeStart
    fn range_end_is_on_same_line_as_range_start(&self, range1: Span, range2: Span) -> bool {
        self.positions_are_on_same_line(range1.end, self.get_start_position_of_range(range2.pos))
    }

    /// Upstream compares the most original nodes' parents; there are no
    /// original-node links yet, so the nodes themselves are compared.
    // port: tsc/internal/printer/utilities.go:originalNodesHaveSameParent
    fn original_nodes_have_same_parent(
        &self,
        node_a: NodeId,
        node_b: NodeId,
    ) -> Result<bool, Error> {
        let parent_a = self.node(node_a)?.parent();
        if parent_a.is_some() {
            return Ok(parent_a == self.node(node_b)?.parent());
        }
        Ok(false)
    }

    // port: tsc/internal/printer/utilities.go:skipSynthesizedParentheses
    fn skip_synthesized_parentheses(&self, span: Span) -> Result<Span, Error> {
        let mut span = span;
        while let Some(id) = span.node {
            let read = self.node(id)?;
            if read.kind() != K::ParenthesizedExpression
                || !ts_ast::utilities::node_is_synthesized(&read)
            {
                break;
            }
            let inner = read
                .data_source()
                .as_parenthesized_expression()
                .and_then(|node| node.expression())
                .ok_or(Error::MissingNode("parenthesized expression"))?;
            span = Span::of(&self.node(inner)?);
        }
        Ok(span)
    }

    // port: tsc/internal/printer/printer.go:Printer.getLinesBetweenNodes
    fn get_lines_between_nodes(
        &self,
        parent: NodeId,
        node1: Span,
        node2: Span,
    ) -> Result<i64, Error> {
        if self.should_elide_indentation(parent) {
            return Ok(0);
        }
        let parent = Span::of(&self.node(parent)?);
        let node1 = self.skip_synthesized_parentheses(node1)?;
        let node2 = self.skip_synthesized_parentheses(node2)?;
        // Always use a newline for synthesized code if the synthesizer desires it.
        if self.should_emit_on_new_line(node2.node, lf::NONE) {
            return Ok(1);
        }
        if self.current_source.is_some()
            && !parent.synthesized()
            && !node1.synthesized()
            && !node2.synthesized()
        {
            return Ok(i64::from(
                !self.range_end_is_on_same_line_as_range_start(node1, node2),
            ));
        }
        Ok(0)
    }

    // port: tsc/internal/printer/printer.go:Printer.getLeadingLineTerminatorCount
    fn get_leading_line_terminator_count(
        &self,
        parent: Option<NodeId>,
        first_child: Option<NodeId>,
        format: ListFormat,
    ) -> Result<i64, Error> {
        if format & lf::PRESERVE_LINES != 0 {
            if format & lf::PREFER_NEW_LINE != 0 {
                return Ok(1);
            }
            let Some(first_child) = first_child else {
                return Ok(match parent {
                    None => 0,
                    Some(parent) => {
                        let parent = Span::of(&self.node(parent)?);
                        i64::from(
                            !(self.current_source.is_some()
                                && self.range_is_on_single_line(parent)),
                        )
                    }
                });
            };
            let first = self.node(first_child)?;
            if self.next_list_element_pos > 0
                && i64::from(first.pos()) == self.next_list_element_pos
            {
                // The parent list already wrote this child's leading line terminators.
                return Ok(0);
            }
            if first.kind() == K::JsxText {
                return Ok(0);
            }
            if let Some(parent) = parent {
                let parent_read = self.node(parent)?;
                if self.current_source.is_some()
                    && !position_is_synthesized(i64::from(parent_read.pos()))
                    && !ts_ast::utilities::node_is_synthesized(&first)
                    && first.parent().is_none()
                {
                    return Ok(i64::from(!self.range_start_positions_are_on_same_line(
                        Span::of(&parent_read),
                        Span::of(&first),
                    )));
                }
            }
            if self.should_emit_on_new_line(Some(first_child), format) {
                return Ok(1);
            }
        }
        Ok(i64::from(format & lf::MULTI_LINE != 0))
    }

    // port: tsc/internal/printer/printer.go:Printer.getSeparatingLineTerminatorCount
    fn get_separating_line_terminator_count(
        &self,
        previous: Option<NodeId>,
        next: Option<NodeId>,
        format: ListFormat,
    ) -> Result<i64, Error> {
        if format & lf::PRESERVE_LINES != 0 {
            let (Some(previous), Some(next)) = (previous, next) else {
                return Ok(0);
            };
            let previous_read = self.node(previous)?;
            let next_read = self.node(next)?;
            if next_read.kind() == K::JsxText {
                return Ok(0);
            } else if self.current_source.is_some()
                && !ts_ast::utilities::node_is_synthesized(&previous_read)
                && !ts_ast::utilities::node_is_synthesized(&next_read)
            {
                if self.original_nodes_have_same_parent(previous, next)? {
                    return Ok(i64::from(!self.range_end_is_on_same_line_as_range_start(
                        Span::of(&previous_read),
                        Span::of(&next_read),
                    )));
                }
                return Ok(i64::from(format & lf::PREFER_NEW_LINE != 0));
            } else if self.should_emit_on_new_line(Some(previous), format)
                || self.should_emit_on_new_line(Some(next), format)
            {
                return Ok(1);
            }
        } else if self.should_emit_on_new_line(next, lf::NONE) {
            return Ok(1);
        }
        Ok(i64::from(format & lf::MULTI_LINE != 0))
    }

    // port: tsc/internal/printer/printer.go:Printer.getClosingLineTerminatorCount
    fn get_closing_line_terminator_count(
        &self,
        parent: Option<NodeId>,
        last_child: Option<NodeId>,
        format: ListFormat,
    ) -> Result<i64, Error> {
        if format & lf::PRESERVE_LINES != 0 {
            if format & lf::PREFER_NEW_LINE != 0 {
                return Ok(1);
            }
            let Some(last_child) = last_child else {
                return Ok(match parent {
                    None => 0,
                    Some(parent) => {
                        let parent = Span::of(&self.node(parent)?);
                        i64::from(
                            !(self.current_source.is_some()
                                && self.range_is_on_single_line(parent)),
                        )
                    }
                });
            };
            let last = self.node(last_child)?;
            if let Some(parent) = parent {
                let parent_read = self.node(parent)?;
                if self.current_source.is_some()
                    && !position_is_synthesized(i64::from(parent_read.pos()))
                    && !ts_ast::utilities::node_is_synthesized(&last)
                    && (last.parent().is_none() || last.parent() == Some(parent))
                {
                    return Ok(i64::from(!self.range_end_positions_are_on_same_line(
                        Span::of(&parent_read),
                        Span::of(&last),
                    )));
                }
            }
            if self.should_emit_on_new_line(Some(last_child), format) {
                return Ok(1);
            }
        }
        if format & lf::MULTI_LINE != 0 && format & lf::NO_TRAILING_NEW_LINE == 0 {
            return Ok(1);
        }
        Ok(0)
    }

    //
    // Tokens/Keywords
    //

    // port: tsc/internal/printer/printer.go:Printer.writeTokenText
    fn write_token_text(&mut self, token: K, write_kind: WriteKind, pos: i64) -> i64 {
        let token_string = token_to_string(token).as_bytes();
        self.write_as(token_string, write_kind);
        if position_is_synthesized(pos) {
            pos
        } else {
            pos + token_string.len() as i64
        }
    }

    /// Comments and source maps around tokens are not emitted, so entering and
    /// leaving a token is the write itself.
    // port: tsc/internal/printer/printer.go:Printer.emitToken
    fn emit_token(&mut self, token: K, pos: i64, write_kind: WriteKind, _context: NodeId) -> i64 {
        self.write_token_text(token, write_kind, pos)
    }

    // port: tsc/internal/printer/printer.go:Printer.emitKeywordNode
    fn emit_keyword_node(&mut self, node: Option<NodeId>) -> Result<(), Error> {
        let Some(node) = node else {
            return Ok(());
        };
        let read = self.node(node)?;
        let kind = self.known_kind(node)?;
        self.write_token_text(kind, WriteKind::Keyword, i64::from(read.pos()));
        Ok(())
    }

    // port: tsc/internal/printer/printer.go:Printer.emitPunctuationNode
    fn emit_punctuation_node(&mut self, node: Option<NodeId>) -> Result<(), Error> {
        let Some(node) = node else {
            return Ok(());
        };
        let read = self.node(node)?;
        let kind = self.known_kind(node)?;
        self.write_token_text(kind, WriteKind::Punctuation, i64::from(read.pos()));
        Ok(())
    }

    // port: tsc/internal/printer/printer.go:Printer.emitTokenNode
    fn emit_token_node(&mut self, node: Option<NodeId>) -> Result<(), Error> {
        let Some(node) = node else {
            return Ok(());
        };
        let kind = self.known_kind(node)?;
        if is_keyword_kind(kind) {
            self.emit_keyword_node(Some(node))
        } else if is_punctuation_kind(kind) {
            self.emit_punctuation_node(Some(node))
        } else {
            Err(Error::UnexpectedKind {
                context: "TokenNode",
                kind: kind.into(),
            })
        }
    }

    //
    // Literals
    //

    // port: tsc/internal/printer/printer.go:Printer.emitLiteral
    fn emit_literal(&mut self, node: NodeId, mut flags: LiteralTextFlags) -> Result<(), Error> {
        if self.printer.options.never_ascii_escape {
            flags = with_flag(flags, LiteralEscapeFlags::NEVER_ASCII_ESCAPE);
        }
        if self.printer.options.terminate_unterminated_literals {
            flags = with_flag(flags, LiteralEscapeFlags::TERMINATE_UNTERMINATED_LITERALS);
        }
        let text = self.get_literal_text_of_node(node, flags)?;
        // Quick info expects every literal through WriteStringLiteral.
        self.writer.write_string_literal(&text);
        Ok(())
    }

    // port: tsc/internal/printer/printer.go:Printer.emitNumericLiteral
    fn emit_numeric_literal(&mut self, node: NodeId) -> Result<(), Error> {
        self.emit_literal(node, LiteralEscapeFlags::NONE)
    }

    // port: tsc/internal/printer/printer.go:Printer.emitBigIntLiteral
    fn emit_big_int_literal(&mut self, node: NodeId) -> Result<(), Error> {
        self.emit_literal(node, LiteralEscapeFlags::NONE)
    }

    // port: tsc/internal/printer/printer.go:Printer.emitStringLiteral
    fn emit_string_literal(&mut self, node: NodeId) -> Result<(), Error> {
        self.emit_literal(node, LiteralEscapeFlags::NONE)
    }

    // port: tsc/internal/printer/printer.go:Printer.emitNoSubstitutionTemplateLiteral
    fn emit_no_substitution_template_literal(&mut self, node: NodeId) -> Result<(), Error> {
        self.emit_literal(node, LiteralEscapeFlags::NONE)
    }

    // port: tsc/internal/printer/printer.go:Printer.emitRegularExpressionLiteral
    fn emit_regular_expression_literal(&mut self, node: NodeId) -> Result<(), Error> {
        self.emit_literal(node, LiteralEscapeFlags::NONE)
    }

    // port: tsc/internal/printer/printer.go:Printer.emitTemplateHead
    fn emit_template_head(&mut self, node: NodeId) -> Result<(), Error> {
        self.emit_literal(node, LiteralEscapeFlags::NONE)
    }

    // port: tsc/internal/printer/printer.go:Printer.emitTemplateMiddle
    fn emit_template_middle(&mut self, node: NodeId) -> Result<(), Error> {
        self.emit_literal(node, LiteralEscapeFlags::NONE)
    }

    // port: tsc/internal/printer/printer.go:Printer.emitTemplateTail
    fn emit_template_tail(&mut self, node: NodeId) -> Result<(), Error> {
        self.emit_literal(node, LiteralEscapeFlags::NONE)
    }

    // port: tsc/internal/printer/printer.go:Printer.emitTemplateMiddleTail
    fn emit_template_middle_tail(&mut self, node: NodeId) -> Result<(), Error> {
        match self.known_kind(node)? {
            K::TemplateMiddle => self.emit_template_middle(node),
            K::TemplateTail => self.emit_template_tail(node),
            _ => Ok(()),
        }
    }

    //
    // Names
    //

    /// Identifier and literal text. Auto-generated names and string-literal text
    /// sources live in the emit context upstream; neither exists here yet.
    // port: tsc/internal/printer/printer.go:Printer.getTextOfNode
    fn get_text_of_node(&self, node: NodeId, include_trivia: bool) -> Result<Vec<u8>, Error> {
        let read = self.node(node)?;
        let can_use_source_file = self.current_source.is_some()
            && read.parent().is_some()
            && !ts_ast::utilities::node_is_synthesized(&read);
        match read.kind().known() {
            Some(K::Identifier | K::PrivateIdentifier) => {
                if !can_use_source_file || !self.node_belongs_to_current_source(node)? {
                    return Ok(match read.kind().known() {
                        Some(K::Identifier) => read
                            .data_source()
                            .as_identifier()
                            .ok_or(Error::MissingNode("identifier payload"))?
                            .text()
                            .to_vec(),
                        _ => read
                            .data_source()
                            .as_private_identifier()
                            .ok_or(Error::MissingNode("private identifier payload"))?
                            .text()
                            .to_vec(),
                    });
                }
                let text = self.source_text().expect("checked above");
                Ok(ts_scanner::get_text_of_node_from_source_text(
                    self.view,
                    text,
                    Some(node),
                    include_trivia,
                )?
                .as_bytes()
                .to_vec())
            }
            Some(K::JsxNamespacedName) => Err(Error::Unsupported("JsxNamespacedName text")),
            Some(
                K::StringLiteral
                | K::NumericLiteral
                | K::BigIntLiteral
                | K::NoSubstitutionTemplateLiteral
                | K::TemplateHead
                | K::TemplateMiddle
                | K::TemplateTail,
            ) => self.get_literal_text_of_node(node, LiteralEscapeFlags::NONE),
            _ => Err(Error::UnexpectedKind {
                context: "getTextOfNode",
                kind: read.kind(),
            }),
        }
    }

    /// `GetSourceFileOfNode(node) == MostOriginal(currentSourceFile)`, without
    /// original-node links.
    fn node_belongs_to_current_source(&self, node: NodeId) -> Result<bool, Error> {
        let Some((file, _)) = &self.current_source else {
            return Ok(false);
        };
        Ok(ts_ast::utilities::get_source_file_of_node(self.view, Some(node))? == Some(*file))
    }

    // port: tsc/internal/printer/printer.go:Printer.emitIdentifierText
    fn emit_identifier_text(&mut self, node: NodeId) -> Result<(), Error> {
        if self.printer.emit_context.has_auto_generate_info(node) {
            return Err(Error::Unsupported("NameGenerator.generateName"));
        }
        let text = self.get_text_of_node(node, false)?;
        let symbol = self
            .printer
            .id_to_symbol
            .as_ref()
            .and_then(|map| map.get(&node).copied());
        if symbol.is_some() {
            self.write_symbol(&text, symbol);
            return Ok(());
        }
        self.write(&text);
        Ok(())
    }

    // port: tsc/internal/printer/printer.go:Printer.emitIdentifierName
    fn emit_identifier_name(&mut self, node: NodeId) -> Result<(), Error> {
        self.emit_identifier_text(node)
    }

    /// Helper-name substitution needs a source file with external helpers, which
    /// no type-display caller has; the plain identifier is written.
    // port: tsc/internal/printer/printer.go:Printer.emitIdentifierReference
    fn emit_identifier_reference(&mut self, node: NodeId) -> Result<(), Error> {
        self.emit_identifier_text(node)
    }

    // port: tsc/internal/printer/printer.go:Printer.emitBindingIdentifier
    fn emit_binding_identifier(&mut self, node: NodeId) -> Result<(), Error> {
        self.emit_identifier_text(node)
    }

    // port: tsc/internal/printer/printer.go:Printer.emitPrivateIdentifier
    fn emit_private_identifier(&mut self, node: NodeId) -> Result<(), Error> {
        let text = self.get_text_of_node(node, false)?;
        self.write(&text);
        Ok(())
    }

    // port: tsc/internal/printer/printer.go:Printer.emitQualifiedName
    fn emit_qualified_name(&mut self, node: NodeId) -> Result<(), Error> {
        let read = self.node(node)?;
        let name = read
            .data_source()
            .as_qualified_name()
            .ok_or(Error::MissingNode("qualified name payload"))?;
        let left = name
            .left()
            .ok_or(Error::MissingNode("qualified name left"))?;
        let right = name
            .right()
            .ok_or(Error::MissingNode("qualified name right"))?;
        self.emit_entity_name(left)?;
        self.write_punctuation(b".");
        self.emit_member_name(Some(right))
    }

    // port: tsc/internal/printer/printer.go:Printer.emitComputedPropertyName
    fn emit_computed_property_name(&mut self, node: NodeId) -> Result<(), Error> {
        let expression = self
            .node(node)?
            .data_source()
            .as_computed_property_name()
            .and_then(|name| name.expression())
            .ok_or(Error::MissingNode("computed property name expression"))?;
        self.write_punctuation(b"[");
        self.emit_expression(expression, op::DISALLOW_COMMA)?;
        self.write_punctuation(b"]");
        Ok(())
    }

    // port: tsc/internal/printer/printer.go:Printer.emitEntityName
    fn emit_entity_name(&mut self, node: NodeId) -> Result<(), Error> {
        match self.known_kind(node)? {
            K::Identifier => self.emit_identifier_reference(node),
            K::QualifiedName => self.emit_qualified_name(node),
            // TypeQuery nodes may carry a property access as their name.
            K::PropertyAccessExpression => self.emit_expression(node, op::DISALLOW_COMMA),
            kind => Err(Error::UnexpectedKind {
                context: "EntityName",
                kind: kind.into(),
            }),
        }
    }

    // port: tsc/internal/printer/printer.go:Printer.emitBindingName
    fn emit_binding_name(&mut self, node: Option<NodeId>) -> Result<(), Error> {
        let Some(node) = node else {
            return Ok(());
        };
        match self.known_kind(node)? {
            K::Identifier => self.emit_binding_identifier(node),
            K::ObjectBindingPattern | K::ArrayBindingPattern => self.emit_binding_pattern(node),
            kind => Err(Error::UnexpectedKind {
                context: "BindingName",
                kind: kind.into(),
            }),
        }
    }

    // port: tsc/internal/printer/printer.go:Printer.emitObjectBindingPattern
    // port: tsc/internal/printer/printer.go:Printer.emitArrayBindingPattern
    fn emit_binding_pattern(&mut self, node: NodeId) -> Result<(), Error> {
        let read = self.node(node)?;
        let elements = read.element_list();
        let object = read.kind() == K::ObjectBindingPattern;
        self.write_punctuation(if object { b"{" } else { b"[" });
        self.emit_list(
            Self::emit_binding_element,
            node,
            elements,
            if object {
                lf::OBJECT_BINDING_PATTERN_ELEMENTS
            } else {
                lf::ARRAY_BINDING_PATTERN_ELEMENTS
            },
        )?;
        self.write_punctuation(if object { b"}" } else { b"]" });
        Ok(())
    }

    // port: tsc/internal/printer/printer.go:Printer.emitBindingElement
    fn emit_binding_element(&mut self, node: NodeId) -> Result<(), Error> {
        let read = self.node(node)?;
        let data = read
            .data_source()
            .as_binding_element()
            .ok_or(Error::MissingNode("binding element"))?;
        let (rest, property, name, initializer) = (
            data.dot_dot_dot_token(),
            data.property_name(),
            data.name(),
            data.initializer(),
        );
        self.emit_token_node(rest)?;
        if property.is_some() {
            self.emit_property_name(property)?;
            self.write_punctuation(b":");
            self.write_space();
        }
        if let Some(name) = name {
            self.emit_binding_name(Some(name))?;
            self.emit_initializer(initializer, i64::from(self.node(name)?.end()), node)?;
        }
        Ok(())
    }

    // port: tsc/internal/printer/printer.go:Printer.emitPropertyName
    fn emit_property_name(&mut self, node: Option<NodeId>) -> Result<(), Error> {
        let Some(node) = node else {
            return Ok(());
        };
        let saved = self.write_kind;
        self.write_kind = WriteKind::Property;
        let result = match self.known_kind(node)? {
            K::Identifier => self.emit_identifier_name(node),
            K::PrivateIdentifier => self.emit_private_identifier(node),
            K::StringLiteral => self.emit_string_literal(node),
            K::NoSubstitutionTemplateLiteral => self.emit_no_substitution_template_literal(node),
            K::NumericLiteral => self.emit_numeric_literal(node),
            K::BigIntLiteral => self.emit_big_int_literal(node),
            K::ComputedPropertyName => self.emit_computed_property_name(node),
            kind => Err(Error::UnexpectedKind {
                context: "PropertyName",
                kind: kind.into(),
            }),
        };
        self.write_kind = saved;
        result
    }

    // port: tsc/internal/printer/printer.go:Printer.emitMemberName
    fn emit_member_name(&mut self, node: Option<NodeId>) -> Result<(), Error> {
        let Some(node) = node else {
            return Ok(());
        };
        match self.known_kind(node)? {
            K::Identifier => self.emit_identifier_name(node),
            K::PrivateIdentifier => self.emit_private_identifier(node),
            kind => Err(Error::UnexpectedKind {
                context: "MemberName",
                kind: kind.into(),
            }),
        }
    }

    //
    // Signature elements
    //

    /// Returns the position after the modifiers, as upstream does for the token
    /// emitters that follow. Decorators are not type-display nodes.
    // port: tsc/internal/printer/printer.go:Printer.emitModifierList
    fn emit_modifier_list(
        &mut self,
        parent: NodeId,
        modifiers: Option<NodeListId>,
        _allow_decorators: bool,
    ) -> Result<i64, Error> {
        let parent_pos = i64::from(self.node(parent)?.pos());
        let Some(list) = modifiers else {
            return Ok(parent_pos);
        };
        let nodes = self.list_nodes(list)?;
        if nodes.is_empty() {
            return Ok(parent_pos);
        }
        for &node in &nodes {
            if !ts_ast::utilities::is_modifier(&self.node(node)?) {
                return Err(Error::Unsupported("decorators"));
            }
        }
        self.emit_list(
            Self::emit_keyword_node_item,
            parent,
            Some(list),
            lf::MODIFIERS,
        )?;
        let last_end = self.node(*nodes.last().expect("nonempty"))?.end();
        Ok(greatest_end(parent_pos, &[Some(i64::from(last_end))]))
    }

    fn emit_keyword_node_item(&mut self, node: NodeId) -> Result<(), Error> {
        self.emit_keyword_node(Some(node))
    }

    // port: tsc/internal/printer/printer.go:Printer.emitTypeParameter
    fn emit_type_parameter(&mut self, node: NodeId) -> Result<(), Error> {
        let read = self.node(node)?;
        let declaration = read
            .data_source()
            .as_type_parameter_declaration()
            .ok_or(Error::MissingNode("type parameter payload"))?;
        let (modifiers, name, constraint, default_type) = (
            declaration.modifiers(),
            declaration.name(),
            declaration.constraint(),
            declaration.default_type(),
        );
        self.emit_modifier_list(node, modifiers, false)?;
        self.emit_binding_identifier(name.ok_or(Error::MissingNode("type parameter name"))?)?;
        if let Some(constraint) = constraint {
            self.write_space();
            self.write_keyword(b"extends");
            self.write_space();
            self.emit_type_node_outside_extends(constraint)?;
        }
        if let Some(default_type) = default_type {
            self.write_space();
            self.write_operator(b"=");
            self.write_space();
            self.emit_type_node_outside_extends(default_type)?;
        }
        Ok(())
    }

    /// Quick info stores type arguments in place of type parameters on
    /// instantiated signatures; both print here.
    // port: tsc/internal/printer/printer.go:Printer.emitTypeParameterDeclarationNode
    fn emit_type_parameter_declaration_node(&mut self, node: NodeId) -> Result<(), Error> {
        if self.known_kind(node)? == K::TypeParameter {
            self.emit_type_parameter(node)
        } else {
            self.emit_type_argument(node)
        }
    }

    // port: tsc/internal/printer/printer.go:Printer.emitParameterName
    fn emit_parameter_name(&mut self, node: Option<NodeId>) -> Result<(), Error> {
        let saved = self.write_kind;
        self.write_kind = WriteKind::Parameter;
        let result = self.emit_binding_name(node);
        self.write_kind = saved;
        result
    }

    // port: tsc/internal/printer/printer.go:Printer.emitParameter
    fn emit_parameter(&mut self, node: NodeId) -> Result<(), Error> {
        let read = self.node(node)?;
        let parameter = read
            .data_source()
            .as_parameter_declaration()
            .ok_or(Error::MissingNode("parameter payload"))?;
        let (modifiers, dot_dot_dot, name, question, type_node, initializer) = (
            parameter.modifiers(),
            parameter.dot_dot_dot_token(),
            parameter.name(),
            parameter.question_token(),
            parameter.r#type(),
            parameter.initializer(),
        );
        self.emit_modifier_list(node, modifiers, true)?;
        self.emit_token_node(dot_dot_dot)?;
        self.emit_parameter_name(name)?;
        self.emit_token_node(question)?;
        self.emit_type_annotation(type_node)?;
        let mut ends = Vec::new();
        for child in [type_node, question, name].into_iter().flatten() {
            ends.push(Some(i64::from(self.node(child)?.end())));
        }
        let equals_pos = greatest_end(i64::from(read.pos()), &ends);
        self.emit_initializer(initializer, equals_pos, node)
    }

    // port: tsc/internal/printer/printer.go:Printer.emitParameterDeclarationNode
    fn emit_parameter_declaration_node(&mut self, node: NodeId) -> Result<(), Error> {
        self.emit_parameter(node)
    }

    // port: tsc/internal/printer/printer.go:Printer.emitTypeParameters
    fn emit_type_parameters(
        &mut self,
        parent: NodeId,
        nodes: Option<NodeListId>,
    ) -> Result<(), Error> {
        if nodes.is_none() {
            return Ok(());
        }
        // Trailing commas are preserved only for arrow functions upstream.
        self.emit_list(
            Self::emit_type_parameter_declaration_node,
            parent,
            nodes,
            lf::TYPE_PARAMETERS,
        )
    }

    // port: tsc/internal/printer/printer.go:Printer.emitTypeAnnotation
    fn emit_type_annotation(&mut self, node: Option<NodeId>) -> Result<(), Error> {
        let Some(node) = node else {
            return Ok(());
        };
        self.write_punctuation(b":");
        self.write_space();
        self.emit_type_node_outside_extends(node)
    }

    // port: tsc/internal/printer/printer.go:Printer.emitInitializer
    fn emit_initializer(
        &mut self,
        node: Option<NodeId>,
        equal_token_pos: i64,
        context: NodeId,
    ) -> Result<(), Error> {
        let Some(node) = node else {
            return Ok(());
        };
        self.write_space();
        self.emit_token(
            K::EqualsToken,
            equal_token_pos,
            WriteKind::Operator,
            context,
        );
        self.write_space();
        self.emit_expression(node, op::DISALLOW_COMMA)
    }

    /// Name generation only affects auto-generated identifiers, which do not
    /// exist here; the traversal is otherwise the list emission.
    // port: tsc/internal/printer/printer.go:Printer.emitParameters
    fn emit_parameters(
        &mut self,
        parent: NodeId,
        parameters: Option<NodeListId>,
    ) -> Result<(), Error> {
        self.emit_list(
            Self::emit_parameter_declaration_node,
            parent,
            parameters,
            lf::PARAMETERS,
        )
    }

    // port: tsc/internal/printer/printer.go:Printer.emitParametersForIndexSignature
    fn emit_parameters_for_index_signature(
        &mut self,
        parent: NodeId,
        parameters: Option<NodeListId>,
    ) -> Result<(), Error> {
        self.emit_list(
            Self::emit_parameter_declaration_node,
            parent,
            parameters,
            lf::INDEX_SIGNATURE_PARAMETERS,
        )
    }

    /// Type parameters, parameters and return type of a function-like node.
    // port: tsc/internal/printer/printer.go:Printer.emitSignature
    fn emit_signature(&mut self, node: NodeId) -> Result<(), Error> {
        let read = self.node(node)?;
        let data = read.data_source();
        let (type_parameters, parameters, type_node) = match read.kind().known() {
            Some(K::GetAccessor | K::SetAccessor) => (
                read.type_parameter_list(),
                read.parameter_list(),
                read.type_node(),
            ),
            Some(K::MethodSignature) => {
                let method = data
                    .as_method_signature_declaration()
                    .ok_or(Error::MissingNode("method signature"))?;
                (
                    method.type_parameters(),
                    method.parameters(),
                    method.r#type(),
                )
            }
            Some(K::CallSignature) => {
                let signature = data
                    .as_call_signature_declaration()
                    .ok_or(Error::MissingNode("call signature"))?;
                (
                    signature.type_parameters(),
                    signature.parameters(),
                    signature.r#type(),
                )
            }
            Some(K::ConstructSignature) => {
                let signature = data
                    .as_construct_signature_declaration()
                    .ok_or(Error::MissingNode("construct signature"))?;
                (
                    signature.type_parameters(),
                    signature.parameters(),
                    signature.r#type(),
                )
            }
            Some(K::FunctionType) => {
                let function = data
                    .as_function_type_node()
                    .ok_or(Error::MissingNode("function type"))?;
                (
                    function.type_parameters(),
                    function.parameters(),
                    function.r#type(),
                )
            }
            Some(K::ConstructorType) => {
                let constructor = data
                    .as_constructor_type_node()
                    .ok_or(Error::MissingNode("constructor type"))?;
                (
                    constructor.type_parameters(),
                    constructor.parameters(),
                    constructor.r#type(),
                )
            }
            _ => {
                return Err(Error::Unsupported(
                    "function-like declarations outside type display",
                ))
            }
        };
        self.emit_type_parameters(node, type_parameters)?;
        self.emit_parameters(node, parameters)?;
        self.emit_type_annotation(type_node)
    }

    //
    // Type members
    //

    // port: tsc/internal/printer/printer.go:Printer.emitPropertySignature
    fn emit_property_signature(&mut self, node: NodeId) -> Result<(), Error> {
        let read = self.node(node)?;
        let property = read
            .data_source()
            .as_property_signature_declaration()
            .ok_or(Error::MissingNode("property signature payload"))?;
        let (modifiers, name, postfix, type_node) = (
            property.modifiers(),
            property.name(),
            property.postfix_token(),
            property.r#type(),
        );
        self.emit_modifier_list(node, modifiers, false)?;
        self.emit_property_name(name)?;
        self.emit_token_node(postfix)?;
        self.emit_type_annotation(type_node)?;
        self.write_trailing_semicolon();
        Ok(())
    }

    // port: tsc/internal/printer/printer.go:Printer.emitMethodSignature
    fn emit_method_signature(&mut self, node: NodeId) -> Result<(), Error> {
        let read = self.node(node)?;
        let method = read
            .data_source()
            .as_method_signature_declaration()
            .ok_or(Error::MissingNode("method signature payload"))?;
        let (modifiers, name, postfix) =
            (method.modifiers(), method.name(), method.postfix_token());
        self.emit_modifier_list(node, modifiers, false)?;
        self.emit_property_name(name)?;
        self.emit_token_node(postfix)?;
        let indented = self.should_emit_indented(node);
        self.increase_indent_if(indented);
        self.emit_signature(node)?;
        self.write_trailing_semicolon();
        self.decrease_indent_if(indented);
        Ok(())
    }

    // port: tsc/internal/printer/printer.go:Printer.emitCallSignature
    fn emit_call_signature(&mut self, node: NodeId) -> Result<(), Error> {
        let indented = self.should_emit_indented(node);
        self.increase_indent_if(indented);
        self.emit_signature(node)?;
        self.write_trailing_semicolon();
        self.decrease_indent_if(indented);
        Ok(())
    }

    // port: tsc/internal/printer/printer.go:Printer.emitConstructSignature
    fn emit_construct_signature(&mut self, node: NodeId) -> Result<(), Error> {
        self.write_keyword(b"new");
        self.write_space();
        let indented = self.should_emit_indented(node);
        self.increase_indent_if(indented);
        self.emit_signature(node)?;
        self.write_trailing_semicolon();
        self.decrease_indent_if(indented);
        Ok(())
    }

    // port: tsc/internal/printer/printer.go:Printer.emitIndexSignature
    fn emit_index_signature(&mut self, node: NodeId) -> Result<(), Error> {
        let read = self.node(node)?;
        let index = read
            .data_source()
            .as_index_signature_declaration()
            .ok_or(Error::MissingNode("index signature payload"))?;
        let (modifiers, parameters, type_node) =
            (index.modifiers(), index.parameters(), index.r#type());
        self.emit_modifier_list(node, modifiers, false)?;
        let indented = self.should_emit_indented(node);
        self.increase_indent_if(indented);
        self.emit_parameters_for_index_signature(node, parameters)?;
        self.emit_type_annotation(type_node)?;
        self.write_trailing_semicolon();
        self.decrease_indent_if(indented);
        Ok(())
    }

    // port: tsc/internal/printer/printer.go:Printer.emitAccessorDeclaration
    fn emit_accessor_declaration(&mut self, node: NodeId) -> Result<(), Error> {
        let read = self.node(node)?;
        if read.body().is_some() {
            return Err(Error::Unsupported("accessor implementation body"));
        }
        let (modifiers, name, kind) = (read.modifiers(), read.name(), read.kind());
        self.emit_modifier_list(node, modifiers, true)?;
        self.write_keyword(if kind == K::GetAccessor {
            b"get"
        } else {
            b"set"
        });
        self.write_space();
        self.emit_property_name(name)?;
        let indented = self.should_emit_indented(node);
        self.increase_indent_if(indented);
        self.emit_signature(node)?;
        self.write_trailing_semicolon();
        self.decrease_indent_if(indented);
        Ok(())
    }

    // port: tsc/internal/printer/printer.go:Printer.emitTypeElement
    fn emit_type_element(&mut self, node: NodeId) -> Result<(), Error> {
        match self.known_kind(node)? {
            K::PropertySignature => self.emit_property_signature(node),
            K::MethodSignature => self.emit_method_signature(node),
            K::CallSignature => self.emit_call_signature(node),
            K::ConstructSignature => self.emit_construct_signature(node),
            K::IndexSignature => self.emit_index_signature(node),
            K::GetAccessor | K::SetAccessor => self.emit_accessor_declaration(node),
            K::NotEmittedTypeElement => Err(Error::Unsupported("NotEmittedTypeElement")),
            kind => Err(Error::UnexpectedKind {
                context: "TypeElement",
                kind: kind.into(),
            }),
        }
    }

    //
    // Types
    //

    // port: tsc/internal/printer/printer.go:Printer.emitKeywordTypeNode
    fn emit_keyword_type_node(&mut self, node: NodeId) -> Result<(), Error> {
        self.emit_keyword_node(Some(node))
    }

    // port: tsc/internal/printer/printer.go:Printer.emitTypePredicateParameterName
    fn emit_type_predicate_parameter_name(&mut self, node: NodeId) -> Result<(), Error> {
        match self.known_kind(node)? {
            K::Identifier => self.emit_identifier_reference(node),
            K::ThisType => {
                self.emit_this_type();
                Ok(())
            }
            kind => Err(Error::UnexpectedKind {
                context: "TypePredicateParameterName",
                kind: kind.into(),
            }),
        }
    }

    // port: tsc/internal/printer/printer.go:Printer.emitTypePredicate
    fn emit_type_predicate(&mut self, node: NodeId) -> Result<(), Error> {
        let read = self.node(node)?;
        let predicate = read
            .data_source()
            .as_type_predicate_node()
            .ok_or(Error::MissingNode("type predicate payload"))?;
        let (asserts, parameter_name, type_node) = (
            predicate.asserts_modifier(),
            predicate.parameter_name(),
            predicate.r#type(),
        );
        if asserts.is_some() {
            self.emit_token_node(asserts)?;
            self.write_space();
        }
        self.emit_type_predicate_parameter_name(
            parameter_name.ok_or(Error::MissingNode("predicate parameter name"))?,
        )?;
        if let Some(type_node) = type_node {
            self.write_space();
            self.write_keyword(b"is");
            self.write_space();
            self.emit_type_node_outside_extends(type_node)?;
        }
        Ok(())
    }

    // port: tsc/internal/printer/printer.go:Printer.emitTypeArgument
    fn emit_type_argument(&mut self, node: NodeId) -> Result<(), Error> {
        self.emit_type_node_outside_extends(node)
    }

    // port: tsc/internal/printer/printer.go:Printer.emitTypeArguments
    fn emit_type_arguments(
        &mut self,
        parent: NodeId,
        nodes: Option<NodeListId>,
    ) -> Result<(), Error> {
        if nodes.is_none() {
            return Ok(());
        }
        self.emit_list(
            Self::emit_type_parameter_declaration_node,
            parent,
            nodes,
            lf::TYPE_ARGUMENTS,
        )
    }

    // port: tsc/internal/printer/printer.go:Printer.emitTypeReference
    fn emit_type_reference(&mut self, node: NodeId) -> Result<(), Error> {
        let read = self.node(node)?;
        let reference = read
            .data_source()
            .as_type_reference_node()
            .ok_or(Error::MissingNode("type reference payload"))?;
        let (type_name, type_arguments) = (reference.type_name(), reference.type_arguments());
        self.emit_entity_name(type_name.ok_or(Error::MissingNode("type reference name"))?)?;
        self.emit_type_arguments(node, type_arguments)
    }

    /// The return type of a function or constructor type, including the arrow.
    // port: tsc/internal/printer/printer.go:Printer.emitReturnType
    fn emit_return_type(&mut self, node: Option<NodeId>) -> Result<(), Error> {
        let Some(node) = node else {
            return Ok(());
        };
        self.write_punctuation(b"=>");
        self.write_space();
        let constrained_infer = self.in_extends && self.known_kind(node)? == K::InferType && {
            let parameter = self
                .node(node)?
                .data_source()
                .as_infer_type_node()
                .and_then(|infer| infer.type_parameter())
                .ok_or(Error::MissingNode("infer type parameter"))?;
            self.node(parameter)?
                .data_source()
                .as_type_parameter_declaration()
                .is_some_and(|declaration| declaration.constraint().is_some())
        };
        if constrained_infer {
            // In the `extends` clause of a conditional type, `infer U extends V`
            // in a return position must be parenthesized to avoid an ambiguous parse.
            self.emit_type_node_preserving_extends(node, TypePrecedence::HIGHEST)
        } else {
            self.emit_type_node_preserving_extends(node, TypePrecedence::LOWEST)
        }
    }

    // port: tsc/internal/printer/printer.go:Printer.emitFunctionType
    fn emit_function_type(&mut self, node: NodeId) -> Result<(), Error> {
        let read = self.node(node)?;
        let function = read
            .data_source()
            .as_function_type_node()
            .ok_or(Error::MissingNode("function type payload"))?;
        let (type_parameters, parameters, type_node) = (
            function.type_parameters(),
            function.parameters(),
            function.r#type(),
        );
        let indented = self.should_emit_indented(node);
        self.increase_indent_if(indented);
        self.emit_type_parameters(node, type_parameters)?;
        self.emit_parameters(node, parameters)?;
        self.write_space();
        self.emit_return_type(type_node)?;
        self.decrease_indent_if(indented);
        Ok(())
    }

    // port: tsc/internal/printer/printer.go:Printer.emitConstructorType
    fn emit_constructor_type(&mut self, node: NodeId) -> Result<(), Error> {
        let read = self.node(node)?;
        let constructor = read
            .data_source()
            .as_constructor_type_node()
            .ok_or(Error::MissingNode("constructor type payload"))?;
        let (modifiers, type_parameters, parameters, type_node) = (
            constructor.modifiers(),
            constructor.type_parameters(),
            constructor.parameters(),
            constructor.r#type(),
        );
        self.emit_modifier_list(node, modifiers, false)?;
        self.write_keyword(b"new");
        self.write_space();
        let indented = self.should_emit_indented(node);
        self.increase_indent_if(indented);
        self.emit_type_parameters(node, type_parameters)?;
        self.emit_parameters(node, parameters)?;
        self.write_space();
        self.emit_return_type(type_node)?;
        self.decrease_indent_if(indented);
        Ok(())
    }

    // port: tsc/internal/printer/printer.go:Printer.emitTypeQuery
    fn emit_type_query(&mut self, node: NodeId) -> Result<(), Error> {
        let read = self.node(node)?;
        let query = read
            .data_source()
            .as_type_query_node()
            .ok_or(Error::MissingNode("type query payload"))?;
        let (expr_name, type_arguments) = (query.expr_name(), query.type_arguments());
        self.write_keyword(b"typeof");
        self.write_space();
        self.emit_entity_name(expr_name.ok_or(Error::MissingNode("type query name"))?)?;
        self.emit_type_arguments(node, type_arguments)
    }

    // port: tsc/internal/printer/printer.go:Printer.emitTypeLiteral
    fn emit_type_literal(&mut self, node: NodeId) -> Result<(), Error> {
        let members = self
            .node(node)?
            .data_source()
            .as_type_literal_node()
            .ok_or(Error::MissingNode("type literal payload"))?
            .members();
        self.write_punctuation(b"{");
        let flags = if self.should_emit_on_single_line(node) {
            lf::SINGLE_LINE_TYPE_LITERAL_MEMBERS
        } else {
            lf::MULTI_LINE_TYPE_LITERAL_MEMBERS
        };
        self.emit_list(
            Self::emit_type_element,
            node,
            members,
            flags | lf::NO_SPACE_IF_EMPTY,
        )?;
        self.write_punctuation(b"}");
        Ok(())
    }

    // port: tsc/internal/printer/printer.go:Printer.emitArrayType
    fn emit_array_type(&mut self, node: NodeId) -> Result<(), Error> {
        let element = self
            .node(node)?
            .data_source()
            .as_array_type_node()
            .and_then(|array| array.element_type())
            .ok_or(Error::MissingNode("array element type"))?;
        self.emit_postfix_type_operand(element, node)?;
        self.write_punctuation(b"[");
        self.write_punctuation(b"]");
        Ok(())
    }

    /// A parsed `typeof X` operand keeps its parse-tree form; a synthesized one is
    /// parenthesized like any other type-operator-precedence operand.
    // port: tsc/internal/printer/printer.go:Printer.emitPostfixTypeOperand
    fn emit_postfix_type_operand(&mut self, operand: NodeId, parent: NodeId) -> Result<(), Error> {
        let parent_is_parse_tree = self.node(parent)?.flags() & node_flags::SYNTHESIZED == 0;
        if parent_is_parse_tree && self.known_kind(operand)? == K::TypeQuery {
            return self.emit_type_node(operand, TypePrecedence::TypeOperator);
        }
        self.emit_type_node(operand, TypePrecedence::Postfix)
    }

    // port: tsc/internal/printer/printer.go:Printer.emitTupleElementType
    fn emit_tuple_element_type(&mut self, node: NodeId) -> Result<(), Error> {
        self.emit_type_node_outside_extends(node)
    }

    // port: tsc/internal/printer/printer.go:Printer.emitTupleType
    fn emit_tuple_type(&mut self, node: NodeId) -> Result<(), Error> {
        let read = self.node(node)?;
        let elements = read
            .data_source()
            .as_tuple_type_node()
            .ok_or(Error::MissingNode("tuple type payload"))?
            .elements();
        self.emit_token(
            K::OpenBracketToken,
            i64::from(read.pos()),
            WriteKind::Punctuation,
            node,
        );
        let flags = if self.should_emit_on_single_line(node) {
            lf::SINGLE_LINE_TUPLE_TYPE_ELEMENTS
        } else {
            lf::MULTI_LINE_TUPLE_TYPE_ELEMENTS
        };
        self.emit_list(
            Self::emit_tuple_element_type,
            node,
            elements,
            flags | lf::NO_SPACE_IF_EMPTY,
        )?;
        let elements_end = match elements {
            Some(list) => self.view.list(list)?.loc().end(),
            None => -1,
        };
        self.emit_token(
            K::CloseBracketToken,
            elements_end,
            WriteKind::Punctuation,
            node,
        );
        Ok(())
    }

    // port: tsc/internal/printer/printer.go:Printer.emitRestType
    fn emit_rest_type(&mut self, node: NodeId) -> Result<(), Error> {
        let inner = self
            .node(node)?
            .data_source()
            .as_rest_type_node()
            .and_then(|rest| rest.r#type())
            .ok_or(Error::MissingNode("rest type"))?;
        self.write_punctuation(b"...");
        self.emit_type_node_outside_extends(inner)
    }

    // port: tsc/internal/printer/printer.go:Printer.emitOptionalType
    fn emit_optional_type(&mut self, node: NodeId) -> Result<(), Error> {
        let inner = self
            .node(node)?
            .data_source()
            .as_optional_type_node()
            .and_then(|optional| optional.r#type())
            .ok_or(Error::MissingNode("optional type"))?;
        self.emit_postfix_type_operand(inner, node)?;
        self.write_punctuation(b"?");
        Ok(())
    }

    // port: tsc/internal/printer/printer.go:Printer.emitNamedTupleMember
    fn emit_named_tuple_member(&mut self, node: NodeId) -> Result<(), Error> {
        let read = self.node(node)?;
        let member = read
            .data_source()
            .as_named_tuple_member()
            .ok_or(Error::MissingNode("named tuple member payload"))?;
        let (dot_dot_dot, name, question, type_node) = (
            member.dot_dot_dot_token(),
            member.name(),
            member.question_token(),
            member.r#type(),
        );
        let name = name.ok_or(Error::MissingNode("named tuple member name"))?;
        self.emit_punctuation_node(dot_dot_dot)?;
        self.emit_identifier_name(name)?;
        self.emit_punctuation_node(question)?;
        let name_end = i64::from(self.node(name)?.end());
        let question_end = match question {
            Some(question) => Some(i64::from(self.node(question)?.end())),
            None => None,
        };
        self.emit_token(
            K::ColonToken,
            greatest_end(name_end, &[question_end]),
            WriteKind::Punctuation,
            node,
        );
        self.write_space();
        self.emit_type_node_outside_extends(
            type_node.ok_or(Error::MissingNode("named tuple member type"))?,
        )
    }

    // port: tsc/internal/printer/printer.go:Printer.emitUnionTypeConstituent
    fn emit_union_type_constituent(&mut self, node: NodeId) -> Result<(), Error> {
        self.emit_type_node(node, TypePrecedence::TypeOperator)
    }

    // port: tsc/internal/printer/printer.go:Printer.emitUnionType
    fn emit_union_type(&mut self, node: NodeId) -> Result<(), Error> {
        let types = self
            .node(node)?
            .data_source()
            .as_union_type_node()
            .ok_or(Error::MissingNode("union type payload"))?
            .types();
        self.emit_list(
            Self::emit_union_type_constituent,
            node,
            types,
            lf::UNION_TYPE_CONSTITUENTS,
        )
    }

    // port: tsc/internal/printer/printer.go:Printer.emitIntersectionTypeConstituent
    fn emit_intersection_type_constituent(&mut self, node: NodeId) -> Result<(), Error> {
        self.emit_type_node(node, TypePrecedence::TypeOperator)
    }

    // port: tsc/internal/printer/printer.go:Printer.emitIntersectionType
    fn emit_intersection_type(&mut self, node: NodeId) -> Result<(), Error> {
        let types = self
            .node(node)?
            .data_source()
            .as_intersection_type_node()
            .ok_or(Error::MissingNode("intersection type payload"))?
            .types();
        self.emit_list(
            Self::emit_intersection_type_constituent,
            node,
            types,
            lf::INTERSECTION_TYPE_CONSTITUENTS,
        )
    }

    // port: tsc/internal/printer/printer.go:Printer.emitConditionalType
    fn emit_conditional_type(&mut self, node: NodeId) -> Result<(), Error> {
        let read = self.node(node)?;
        let conditional = read
            .data_source()
            .as_conditional_type_node()
            .ok_or(Error::MissingNode("conditional type payload"))?;
        let (check, extends, true_type, false_type) = (
            conditional
                .check_type()
                .ok_or(Error::MissingNode("check type"))?,
            conditional
                .extends_type()
                .ok_or(Error::MissingNode("extends type"))?,
            conditional
                .true_type()
                .ok_or(Error::MissingNode("true type"))?,
            conditional
                .false_type()
                .ok_or(Error::MissingNode("false type"))?,
        );
        self.emit_type_node(check, TypePrecedence::Union)?;
        self.write_space();
        self.write_keyword(b"extends");
        self.write_space();
        self.emit_type_node_in_extends(extends)?;
        self.write_space();
        self.write_punctuation(b"?");
        self.write_space();
        self.emit_type_node_outside_extends(true_type)?;
        self.write_space();
        self.write_punctuation(b":");
        self.write_space();
        self.emit_type_node_outside_extends(false_type)
    }

    // port: tsc/internal/printer/printer.go:Printer.emitInferTypeParameter
    fn emit_infer_type_parameter(&mut self, node: NodeId) -> Result<(), Error> {
        let read = self.node(node)?;
        let declaration = read
            .data_source()
            .as_type_parameter_declaration()
            .ok_or(Error::MissingNode("infer type parameter payload"))?;
        let (name, constraint) = (declaration.name(), declaration.constraint());
        self.emit_binding_identifier(name.ok_or(Error::MissingNode("infer type parameter name"))?)?;
        if let Some(constraint) = constraint {
            self.write_space();
            self.write_keyword(b"extends");
            self.write_space();
            self.emit_type_node_in_extends(constraint)?;
        }
        Ok(())
    }

    // port: tsc/internal/printer/printer.go:Printer.emitInferType
    fn emit_infer_type(&mut self, node: NodeId) -> Result<(), Error> {
        let parameter = self
            .node(node)?
            .data_source()
            .as_infer_type_node()
            .and_then(|infer| infer.type_parameter())
            .ok_or(Error::MissingNode("infer type parameter"))?;
        self.write_keyword(b"infer");
        self.write_space();
        self.emit_infer_type_parameter(parameter)
    }

    // port: tsc/internal/printer/printer.go:Printer.emitParenthesizedType
    fn emit_parenthesized_type(&mut self, node: NodeId) -> Result<(), Error> {
        let inner = self
            .node(node)?
            .data_source()
            .as_parenthesized_type_node()
            .and_then(|parenthesized| parenthesized.r#type())
            .ok_or(Error::MissingNode("parenthesized type"))?;
        self.write_punctuation(b"(");
        self.emit_type_node_outside_extends(inner)?;
        self.write_punctuation(b")");
        Ok(())
    }

    // port: tsc/internal/printer/printer.go:Printer.emitThisType
    fn emit_this_type(&mut self) {
        self.write_keyword(b"this");
    }

    // port: tsc/internal/printer/printer.go:Printer.emitTypeOperator
    fn emit_type_operator(&mut self, node: NodeId) -> Result<(), Error> {
        let read = self.node(node)?;
        let operator_node = read
            .data_source()
            .as_type_operator_node()
            .ok_or(Error::MissingNode("type operator payload"))?;
        let (operator, inner) = (operator_node.operator(), operator_node.r#type());
        let operator = operator.known().ok_or(Error::UnexpectedKind {
            context: "TypeOperator",
            kind: operator,
        })?;
        self.emit_token(operator, i64::from(read.pos()), WriteKind::Keyword, node);
        self.write_space();
        let precedence = if operator == K::ReadonlyKeyword {
            TypePrecedence::Postfix
        } else {
            TypePrecedence::TypeOperator
        };
        self.emit_type_node(
            inner.ok_or(Error::MissingNode("type operator operand"))?,
            precedence,
        )
    }

    // port: tsc/internal/printer/printer.go:Printer.emitIndexedAccessType
    fn emit_indexed_access_type(&mut self, node: NodeId) -> Result<(), Error> {
        let read = self.node(node)?;
        let access = read
            .data_source()
            .as_indexed_access_type_node()
            .ok_or(Error::MissingNode("indexed access payload"))?;
        let (object, index) = (
            access
                .object_type()
                .ok_or(Error::MissingNode("indexed access object"))?,
            access
                .index_type()
                .ok_or(Error::MissingNode("indexed access index"))?,
        );
        self.emit_postfix_type_operand(object, node)?;
        self.write_punctuation(b"[");
        self.emit_type_node_outside_extends(index)?;
        self.write_punctuation(b"]");
        Ok(())
    }

    // port: tsc/internal/printer/printer.go:Printer.emitMappedTypeParameter
    fn emit_mapped_type_parameter(&mut self, node: NodeId) -> Result<(), Error> {
        let read = self.node(node)?;
        let declaration = read
            .data_source()
            .as_type_parameter_declaration()
            .ok_or(Error::MissingNode("mapped type parameter payload"))?;
        let (name, constraint) = (declaration.name(), declaration.constraint());
        self.emit_binding_identifier(
            name.ok_or(Error::MissingNode("mapped type parameter name"))?,
        )?;
        self.write_space();
        self.write_keyword(b"in");
        self.write_space();
        self.emit_type_node_outside_extends(
            constraint.ok_or(Error::MissingNode("mapped type constraint"))?,
        )
    }

    // port: tsc/internal/printer/printer.go:Printer.emitMappedType
    fn emit_mapped_type(&mut self, node: NodeId) -> Result<(), Error> {
        let read = self.node(node)?;
        let mapped = read
            .data_source()
            .as_mapped_type_node()
            .ok_or(Error::MissingNode("mapped type payload"))?;
        let (readonly, type_parameter, name_type, question, type_node, members) = (
            mapped.readonly_token(),
            mapped
                .type_parameter()
                .ok_or(Error::MissingNode("mapped type parameter"))?,
            mapped.name_type(),
            mapped.question_token(),
            mapped.r#type(),
            mapped.members(),
        );
        let single_line = self.should_emit_on_single_line(node);
        self.write_punctuation(b"{");
        if single_line {
            self.write_space();
        } else {
            self.write_line();
            self.increase_indent();
        }
        if let Some(readonly) = readonly {
            self.emit_token_node(Some(readonly))?;
            if self.known_kind(readonly)? != K::ReadonlyKeyword {
                self.write_keyword(b"readonly");
            }
            self.write_space();
        }
        self.write_punctuation(b"[");
        self.emit_mapped_type_parameter(type_parameter)?;
        if let Some(name_type) = name_type {
            self.write_space();
            self.write_keyword(b"as");
            self.write_space();
            self.emit_type_node_outside_extends(name_type)?;
        }
        self.write_punctuation(b"]");
        if let Some(question) = question {
            self.emit_punctuation_node(Some(question))?;
            if self.known_kind(question)? != K::QuestionToken {
                self.write_punctuation(b"?");
            }
        }
        if let Some(type_node) = type_node {
            self.write_punctuation(b":");
            self.write_space();
            self.emit_type_node_outside_extends(type_node)?;
        }
        self.write_trailing_semicolon();
        if self.list_len(members)? > 0 {
            if single_line {
                self.write_space();
            } else {
                self.write_line();
            }
            self.emit_list(Self::emit_type_element, node, members, lf::PRESERVE_LINES)?;
        }
        if single_line {
            self.write_space();
        } else {
            self.write_line();
            self.decrease_indent();
        }
        self.write_punctuation(b"}");
        Ok(())
    }

    // port: tsc/internal/printer/printer.go:Printer.emitLiteralType
    fn emit_literal_type(&mut self, node: NodeId) -> Result<(), Error> {
        let literal = self
            .node(node)?
            .data_source()
            .as_literal_type_node()
            .and_then(|literal| literal.literal())
            .ok_or(Error::MissingNode("literal type literal"))?;
        self.emit_expression(literal, op::COMMA)
    }

    // port: tsc/internal/printer/printer.go:Printer.emitTemplateTypeSpan
    fn emit_template_type_span(&mut self, node: NodeId) -> Result<(), Error> {
        let read = self.node(node)?;
        let span = read
            .data_source()
            .as_template_literal_type_span()
            .ok_or(Error::MissingNode("template span payload"))?;
        let (type_node, literal) = (
            span.r#type()
                .ok_or(Error::MissingNode("template span type"))?,
            span.literal()
                .ok_or(Error::MissingNode("template span literal"))?,
        );
        self.emit_type_node_outside_extends(type_node)?;
        self.emit_template_middle_tail(literal)
    }

    // port: tsc/internal/printer/printer.go:Printer.emitTemplateTypeSpanNode
    fn emit_template_type_span_node(&mut self, node: NodeId) -> Result<(), Error> {
        self.emit_template_type_span(node)
    }

    // port: tsc/internal/printer/printer.go:Printer.emitTemplateType
    fn emit_template_type(&mut self, node: NodeId) -> Result<(), Error> {
        let read = self.node(node)?;
        let template = read
            .data_source()
            .as_template_literal_type_node()
            .ok_or(Error::MissingNode("template literal type payload"))?;
        let (head, spans) = (
            template.head().ok_or(Error::MissingNode("template head"))?,
            template.template_spans(),
        );
        self.emit_template_head(head)?;
        self.emit_list(
            Self::emit_template_type_span_node,
            node,
            spans,
            lf::TEMPLATE_EXPRESSION_SPANS,
        )
    }

    // port: tsc/internal/printer/printer.go:Printer.emitImportTypeNode
    fn emit_import_type_node(&mut self, node: NodeId) -> Result<(), Error> {
        let read = self.node(node)?;
        let import = read
            .data_source()
            .as_import_type_node()
            .ok_or(Error::MissingNode("import type payload"))?;
        let (is_type_of, argument, attributes, qualifier, type_arguments) = (
            import.is_type_of(),
            import
                .argument()
                .ok_or(Error::MissingNode("import type argument"))?,
            import.attributes(),
            import.qualifier(),
            import.type_arguments(),
        );
        if is_type_of {
            self.write_keyword(b"typeof");
            self.write_space();
        }
        self.write_keyword(b"import");
        self.write_punctuation(b"(");
        self.emit_type_node_outside_extends(argument)?;
        if attributes.is_some() {
            return Err(Error::Unsupported("import type attributes"));
        }
        self.write_punctuation(b")");
        if let Some(qualifier) = qualifier {
            self.write_punctuation(b".");
            self.emit_entity_name(qualifier)?;
        }
        self.emit_type_arguments(node, type_arguments)
    }

    // port: tsc/internal/printer/printer.go:Printer.emitTypeNodeInExtends
    fn emit_type_node_in_extends(&mut self, node: NodeId) -> Result<(), Error> {
        let saved = self.in_extends;
        self.in_extends = true;
        let result = self.emit_type_node_preserving_extends(node, TypePrecedence::LOWEST);
        self.in_extends = saved;
        result
    }

    // port: tsc/internal/printer/printer.go:Printer.emitTypeNodeOutsideExtends
    pub(crate) fn emit_type_node_outside_extends(&mut self, node: NodeId) -> Result<(), Error> {
        let saved = self.in_extends;
        self.in_extends = false;
        let result = self.emit_type_node_preserving_extends(node, TypePrecedence::LOWEST);
        self.in_extends = saved;
        result
    }

    // port: tsc/internal/printer/printer.go:Printer.emitTypeNodePreservingExtends
    fn emit_type_node_preserving_extends(
        &mut self,
        node: NodeId,
        precedence: TypePrecedence,
    ) -> Result<(), Error> {
        self.emit_type_node(node, precedence)
    }

    // port: tsc/internal/printer/printer.go:Printer.emitTypeNode
    fn emit_type_node(&mut self, node: NodeId, precedence: TypePrecedence) -> Result<(), Error> {
        let mut precedence = precedence;
        if self.in_extends && precedence <= TypePrecedence::Conditional {
            // In the `extends` clause of a conditional or infer type a conditional
            // type must be parenthesized.
            precedence = TypePrecedence::Function;
        }
        let saved_in_extends = self.in_extends;
        let kind = self.known_kind(node)?;
        let node_precedence =
            get_type_node_precedence(self.view, node)?.ok_or(Error::UnexpectedKind {
                context: "TypeNode",
                kind: kind.into(),
            })?;
        let parens = node_precedence < precedence;
        if parens {
            self.in_extends = false;
            self.write_punctuation(b"(");
        }
        let result = match kind {
            K::AnyKeyword
            | K::UnknownKeyword
            | K::NumberKeyword
            | K::BigIntKeyword
            | K::ObjectKeyword
            | K::BooleanKeyword
            | K::StringKeyword
            | K::SymbolKeyword
            | K::VoidKeyword
            | K::UndefinedKeyword
            | K::NeverKeyword
            | K::IntrinsicKeyword => self.emit_keyword_type_node(node),
            K::TypePredicate => self.emit_type_predicate(node),
            K::TypeReference => self.emit_type_reference(node),
            K::FunctionType => self.emit_function_type(node),
            K::ConstructorType => self.emit_constructor_type(node),
            K::TypeQuery => self.emit_type_query(node),
            K::TypeLiteral => self.emit_type_literal(node),
            K::ArrayType => self.emit_array_type(node),
            K::TupleType => self.emit_tuple_type(node),
            K::OptionalType => self.emit_optional_type(node),
            K::RestType => self.emit_rest_type(node),
            K::UnionType => self.emit_union_type(node),
            K::IntersectionType => self.emit_intersection_type(node),
            K::ConditionalType => self.emit_conditional_type(node),
            K::InferType => self.emit_infer_type(node),
            K::ParenthesizedType => self.emit_parenthesized_type(node),
            K::ThisType => {
                self.emit_this_type();
                Ok(())
            }
            K::TypeOperator => self.emit_type_operator(node),
            K::IndexedAccessType => self.emit_indexed_access_type(node),
            K::MappedType => self.emit_mapped_type(node),
            K::LiteralType => self.emit_literal_type(node),
            K::NamedTupleMember => self.emit_named_tuple_member(node),
            K::TemplateLiteralType => self.emit_template_type(node),
            K::TemplateLiteralTypeSpan => self.emit_template_type_span(node),
            K::ImportType => self.emit_import_type_node(node),
            // Pseudo-types such as `f<T>.C`, where `f` is a generic function.
            K::PropertyAccessExpression => self.emit_property_access_expression(node),
            K::ExpressionWithTypeArguments => self.emit_expression_with_type_arguments(node),
            K::JSDocAllType => self.emit_jsdoc_all_type(node),
            K::JSDocNonNullableType => self.emit_jsdoc_non_nullable_type(node),
            K::JSDocNullableType => self.emit_jsdoc_nullable_type(node),
            K::JSDocOptionalType => self.emit_jsdoc_optional_type(node),
            K::JSDocVariadicType => self.emit_jsdoc_variadic_type(node),
            kind => Err(Error::UnexpectedKind {
                context: "TypeNode",
                kind: kind.into(),
            }),
        };
        result?;
        if parens {
            self.write_punctuation(b")");
        }
        self.in_extends = saved_in_extends;
        Ok(())
    }

    fn jsdoc_type_operand(&self, node: NodeId, what: &'static str) -> Result<NodeId, Error> {
        let read = self.node(node)?;
        let data = read.data_source();
        let inner = match read.kind().known() {
            Some(K::JSDocNonNullableType) => data
                .as_js_doc_non_nullable_type()
                .and_then(|node| node.r#type()),
            Some(K::JSDocNullableType) => data
                .as_js_doc_nullable_type()
                .and_then(|node| node.r#type()),
            Some(K::JSDocOptionalType) => data
                .as_js_doc_optional_type()
                .and_then(|node| node.r#type()),
            Some(K::JSDocVariadicType) => data
                .as_js_doc_variadic_type()
                .and_then(|node| node.r#type()),
            _ => None,
        };
        inner.ok_or(Error::MissingNode(what))
    }

    // port: tsc/internal/printer/printer.go:Printer.emitJSDocAllType
    fn emit_jsdoc_all_type(&mut self, node: NodeId) -> Result<(), Error> {
        self.emit_keyword_node(Some(node))
    }

    // port: tsc/internal/printer/printer.go:Printer.emitJSDocNonNullableType
    fn emit_jsdoc_non_nullable_type(&mut self, node: NodeId) -> Result<(), Error> {
        let inner = self.jsdoc_type_operand(node, "JSDoc non-nullable type")?;
        self.write_punctuation(b"!");
        self.emit_type_node(inner, TypePrecedence::NonArray)
    }

    // port: tsc/internal/printer/printer.go:Printer.emitJSDocNullableType
    fn emit_jsdoc_nullable_type(&mut self, node: NodeId) -> Result<(), Error> {
        let inner = self.jsdoc_type_operand(node, "JSDoc nullable type")?;
        self.write_punctuation(b"?");
        self.emit_type_node(inner, TypePrecedence::NonArray)
    }

    // port: tsc/internal/printer/printer.go:Printer.emitJSDocOptionalType
    fn emit_jsdoc_optional_type(&mut self, node: NodeId) -> Result<(), Error> {
        let inner = self.jsdoc_type_operand(node, "JSDoc optional type")?;
        self.emit_type_node(inner, TypePrecedence::JsDoc)?;
        self.write_punctuation(b"=");
        Ok(())
    }

    // port: tsc/internal/printer/printer.go:Printer.emitJSDocVariadicType
    fn emit_jsdoc_variadic_type(&mut self, node: NodeId) -> Result<(), Error> {
        let inner = self.jsdoc_type_operand(node, "JSDoc variadic type")?;
        self.write_punctuation(b"...");
        self.emit_type_node(inner, TypePrecedence::JsDoc)
    }

    //
    // Expressions (the subset literal types and entity names can hold)
    //

    /// `GetExpressionPrecedence` for the supported expression kinds; `None` marks
    /// a kind the printer does not emit yet.
    fn expression_precedence(&self, node: NodeId) -> Result<Option<i32>, Error> {
        let read = self.node(node)?;
        Ok(match read.kind().known() {
            Some(
                K::TrueKeyword
                | K::FalseKeyword
                | K::NullKeyword
                | K::ThisKeyword
                | K::SuperKeyword
                | K::ImportKeyword
                | K::NumericLiteral
                | K::BigIntLiteral
                | K::StringLiteral
                | K::RegularExpressionLiteral
                | K::NoSubstitutionTemplateLiteral
                | K::Identifier
                | K::PrivateIdentifier,
            ) => Some(op::PRIMARY),
            Some(K::PropertyAccessExpression) => {
                Some(if ts_ast::utilities::is_optional_chain(&read) {
                    op::OPTIONAL_CHAIN
                } else {
                    op::MEMBER
                })
            }
            // Upstream ranks every prefix unary expression, `++` and `--` included, as unary.
            Some(K::PrefixUnaryExpression) => Some(op::UNARY),
            Some(K::ExpressionWithTypeArguments) => Some(op::MEMBER),
            _ => None,
        })
    }

    // port: tsc/internal/printer/printer.go:Printer.emitExpression
    fn emit_expression(&mut self, node: NodeId, precedence: i32) -> Result<(), Error> {
        let kind = self.known_kind(node)?;
        let node_precedence = self.expression_precedence(node)?.ok_or(Error::Unsupported(
            "expressions outside literal types and entity names",
        ))?;
        let parens = node_precedence < precedence;
        if parens {
            self.write_punctuation(b"(");
        }
        match kind {
            K::TrueKeyword | K::FalseKeyword | K::NullKeyword => {
                self.emit_token_node(Some(node))?;
            }
            K::ThisKeyword | K::SuperKeyword | K::ImportKeyword => {
                self.emit_keyword_expression(node)?;
            }
            K::NumericLiteral => self.emit_numeric_literal(node)?,
            K::BigIntLiteral => self.emit_big_int_literal(node)?,
            K::StringLiteral => self.emit_string_literal(node)?,
            K::RegularExpressionLiteral => self.emit_regular_expression_literal(node)?,
            K::NoSubstitutionTemplateLiteral => self.emit_no_substitution_template_literal(node)?,
            K::Identifier => self.emit_identifier_reference(node)?,
            K::PrivateIdentifier => self.emit_private_identifier(node)?,
            K::PropertyAccessExpression => self.emit_property_access_expression(node)?,
            K::PrefixUnaryExpression => self.emit_prefix_unary_expression(node)?,
            K::ExpressionWithTypeArguments => self.emit_expression_with_type_arguments(node)?,
            kind => {
                return Err(Error::UnexpectedKind {
                    context: "Expression",
                    kind: kind.into(),
                })
            }
        }
        if parens {
            self.write_punctuation(b")");
        }
        Ok(())
    }

    // port: tsc/internal/printer/printer.go:Printer.emitKeywordExpression
    fn emit_keyword_expression(&mut self, node: NodeId) -> Result<(), Error> {
        self.emit_keyword_node(Some(node))
    }

    // port: tsc/internal/printer/printer.go:Printer.emitPrefixUnaryExpression
    fn emit_prefix_unary_expression(&mut self, node: NodeId) -> Result<(), Error> {
        let read = self.node(node)?;
        let unary = read
            .data_source()
            .as_prefix_unary_expression()
            .ok_or(Error::MissingNode("prefix unary payload"))?;
        let operator = unary.operator().known().ok_or(Error::UnexpectedKind {
            context: "PrefixUnaryExpression",
            kind: unary.operator(),
        })?;
        let operand = unary
            .operand()
            .ok_or(Error::MissingNode("prefix unary operand"))?;
        self.emit_token(operator, i64::from(read.pos()), WriteKind::Operator, node);
        // `+ +x` and `- -x` need a space so they do not read as increments.
        let operand_read = self.node(operand)?;
        if operand_read.kind() == K::PrefixUnaryExpression {
            let inner = operand_read
                .data_source()
                .as_prefix_unary_expression()
                .map(|inner| inner.operator())
                .and_then(NodeKind::known);
            if (operator == K::PlusToken && matches!(inner, Some(K::PlusToken | K::PlusPlusToken)))
                || (operator == K::MinusToken
                    && matches!(inner, Some(K::MinusToken | K::MinusMinusToken)))
            {
                self.write_space();
            }
        }
        self.emit_expression(operand, op::UNARY)
    }

    /// A numeric literal written without a dot or exponent needs `..` before a
    /// member name, as in `1..toString`.
    // port: tsc/internal/printer/printer.go:Printer.mayNeedDotDotForPropertyAccess
    fn may_need_dot_dot_for_property_access(&self, expression: NodeId) -> Result<bool, Error> {
        let read = self.node(expression)?;
        if read.kind() != K::NumericLiteral {
            return Ok(false);
        }
        let text =
            self.get_literal_text_of_node(expression, LiteralEscapeFlags::NEVER_ASCII_ESCAPE)?;
        let flags = read
            .data_source()
            .as_numeric_literal()
            .map_or(0, |literal| literal.token_flags());
        let with_specifier = token_flags::BINARY_SPECIFIER
            | token_flags::OCTAL_SPECIFIER
            | token_flags::HEX_SPECIFIER;
        Ok(flags & with_specifier == 0
            && !text.contains(&b'.')
            && !text.contains(&b'E')
            && !text.contains(&b'e'))
    }

    // port: tsc/internal/printer/printer.go:Printer.emitPropertyAccessExpression
    fn emit_property_access_expression(&mut self, node: NodeId) -> Result<(), Error> {
        let read = self.node(node)?;
        let access = read
            .data_source()
            .as_property_access_expression()
            .ok_or(Error::MissingNode("property access payload"))?;
        let (expression, question_dot, name) = (
            access
                .expression()
                .ok_or(Error::MissingNode("property access expression"))?,
            access.question_dot_token(),
            access
                .name()
                .ok_or(Error::MissingNode("property access name"))?,
        );
        let precedence = if ts_ast::utilities::is_optional_chain(&read) {
            op::OPTIONAL_CHAIN
        } else {
            op::MEMBER
        };
        self.emit_expression(expression, precedence)?;
        let expression_read = self.node(expression)?;
        let name_read = self.node(name)?;
        // Upstream synthesizes a dot token spanning the gap when none was parsed.
        let token = match question_dot {
            Some(token) => Span::of(&self.node(token)?),
            None => Span {
                node: None,
                pos: i64::from(expression_read.end()),
                end: i64::from(name_read.pos()),
            },
        };
        let token_kind = match question_dot {
            Some(token) => self.known_kind(token)?,
            None => K::DotToken,
        };
        let lines_before_dot =
            self.get_lines_between_nodes(node, Span::of(&expression_read), token)?;
        self.write_line_repeat(lines_before_dot);
        self.increase_indent_if(lines_before_dot > 0);
        let should_emit_dot_dot = token_kind != K::QuestionDotToken
            && self.may_need_dot_dot_for_property_access(expression)?
            && !self.writer.has_trailing_comment()
            && !self.writer.has_trailing_whitespace();
        if should_emit_dot_dot {
            self.write_punctuation(b".");
        }
        if question_dot.is_some() {
            self.emit_token_node(question_dot)?;
        } else {
            self.emit_token(
                K::DotToken,
                i64::from(expression_read.end()),
                WriteKind::Punctuation,
                node,
            );
        }
        let lines_after_dot = self.get_lines_between_nodes(node, token, Span::of(&name_read))?;
        self.write_line_repeat(lines_after_dot);
        self.increase_indent_if(lines_after_dot > 0);
        self.emit_member_name(Some(name))?;
        self.decrease_indent_if(lines_after_dot > 0);
        self.decrease_indent_if(lines_before_dot > 0);
        Ok(())
    }

    // port: tsc/internal/printer/printer.go:Printer.emitExpressionWithTypeArguments
    fn emit_expression_with_type_arguments(&mut self, node: NodeId) -> Result<(), Error> {
        let read = self.node(node)?;
        let with_arguments = read
            .data_source()
            .as_expression_with_type_arguments()
            .ok_or(Error::MissingNode("expression with type arguments payload"))?;
        let (expression, type_arguments) = (
            with_arguments
                .expression()
                .ok_or(Error::MissingNode("expression"))?,
            with_arguments.type_arguments(),
        );
        self.emit_expression(expression, op::LEFT_HAND_SIDE)?;
        self.emit_type_arguments(node, type_arguments)
    }

    //
    // Lists
    //

    // port: tsc/internal/printer/printer.go:Printer.emitList
    fn emit_list(
        &mut self,
        emit: fn(&mut Self, NodeId) -> Result<(), Error>,
        parent: NodeId,
        children: Option<NodeListId>,
        mut format: ListFormat,
    ) -> Result<(), Error> {
        if self.should_emit_on_multiple_lines(parent) {
            format |= lf::PREFER_NEW_LINE | lf::INDENTED;
        }
        self.emit_list_range(emit, parent, children, format, -1, -1)
    }

    // port: tsc/internal/printer/printer.go:Printer.emitListRange
    fn emit_list_range(
        &mut self,
        emit: fn(&mut Self, NodeId) -> Result<(), Error>,
        parent: NodeId,
        children: Option<NodeListId>,
        format: ListFormat,
        start: i64,
        count: i64,
    ) -> Result<(), Error> {
        let is_nil = children.is_none();
        let nodes = match children {
            Some(list) => self.list_nodes(list)?,
            None => Vec::new(),
        };
        let length = nodes.len() as i64;
        let start = start.max(0);
        let count = if count < 0 { length - start } else { count };
        if is_nil && format & lf::OPTIONAL_IF_NIL != 0 {
            return Ok(());
        }
        let is_empty = is_nil || start >= length || count <= 0;
        if is_empty && format & lf::OPTIONAL_IF_EMPTY != 0 {
            return Ok(());
        }
        if format & lf::BRACKETS_MASK != 0 {
            let bracket = get_opening_bracket(format)?;
            self.write_punctuation(bracket);
            // Comments inside empty lists are not emitted.
        }
        if is_empty {
            // Write a line terminator if the parent node was multi-line.
            if format & lf::MULTI_LINE != 0 {
                self.write_line();
            } else if format & lf::SPACE_BETWEEN_BRACES != 0 && format & lf::NO_SPACE_IF_EMPTY == 0
            {
                self.write_space();
            }
        } else {
            let end = (start + count).min(length) as usize;
            let children_range = match children {
                Some(list) => {
                    let loc = self.view.list(list)?.loc();
                    (loc.pos(), loc.end())
                }
                None => (-1, -1),
            };
            self.emit_list_items(
                emit,
                parent,
                &nodes[start as usize..end],
                format,
                false,
                children_range,
            )?;
        }
        if format & lf::BRACKETS_MASK != 0 {
            let bracket = get_closing_bracket(format)?;
            self.write_punctuation(bracket);
        }
        Ok(())
    }

    // port: tsc/internal/printer/printer.go:Printer.writeDelimiter
    fn write_delimiter(&mut self, format: ListFormat) {
        match format & lf::DELIMITERS_MASK {
            lf::COMMA_DELIMITED => self.write_punctuation(b","),
            lf::BAR_DELIMITED => {
                self.write_space();
                self.write_punctuation(b"|");
            }
            lf::ASTERISK_DELIMITED => {
                self.write_space();
                self.write_punctuation(b"*");
                self.write_space();
            }
            lf::AMPERSAND_DELIMITED => {
                self.write_space();
                self.write_punctuation(b"&");
            }
            _ => {}
        }
    }

    /// Emits a list without brackets. Trailing commas are never requested by the
    /// supported formats; the node lists do not record them yet either.
    // port: tsc/internal/printer/printer.go:Printer.emitListItems
    fn emit_list_items(
        &mut self,
        emit: fn(&mut Self, NodeId) -> Result<(), Error>,
        parent: NodeId,
        children: &[NodeId],
        format: ListFormat,
        has_trailing_comma: bool,
        _children_range: (i64, i64),
    ) -> Result<(), Error> {
        let leading = match children.first() {
            Some(&first) => {
                self.get_leading_line_terminator_count(Some(parent), Some(first), format)?
            }
            None => 0,
        };
        if leading > 0 {
            self.write_line_repeat(leading);
        } else if format & lf::SPACE_BETWEEN_BRACES != 0 {
            self.write_space();
        }
        if format & lf::INDENTED != 0 {
            self.increase_indent();
        }
        let mut previous_sibling: Option<NodeId> = None;
        let mut should_decrease_indent_after_emit = false;
        for &child in children {
            if format & lf::ASTERISK_DELIMITED != 0 {
                // JSDoc is always written as "\n *".
                self.write_line();
                self.write_delimiter(format);
            } else if let Some(previous) = previous_sibling {
                self.write_delimiter(format);
                // Either a line terminator or whitespace separates the elements.
                let separating =
                    self.get_separating_line_terminator_count(Some(previous), Some(child), format)?;
                if separating > 0 {
                    // A synthesized node in a single-line list that starts on a new
                    // line increases the indent.
                    if format & (lf::LINES_MASK | lf::INDENTED) == lf::SINGLE_LINE {
                        self.increase_indent();
                        should_decrease_indent_after_emit = true;
                    }
                    self.write_line_repeat(separating);
                } else if format & lf::SPACE_BETWEEN_SIBLINGS != 0 {
                    self.write_space();
                }
            }
            self.next_list_element_pos = i64::from(self.node(child)?.pos());
            emit(self, child)?;
            if should_decrease_indent_after_emit {
                self.decrease_indent();
                should_decrease_indent_after_emit = false;
            }
            previous_sibling = Some(child);
        }
        let emit_trailing_comma = has_trailing_comma
            && format & lf::ALLOW_TRAILING_COMMA != 0
            && format & lf::COMMA_DELIMITED != 0;
        if emit_trailing_comma {
            self.write_punctuation(b",");
        }
        if format & lf::INDENTED != 0 {
            self.decrease_indent();
        }
        let closing =
            self.get_closing_line_terminator_count(Some(parent), previous_sibling, format)?;
        if closing > 0 {
            self.write_line_repeat(closing);
        } else if format & (lf::SPACE_AFTER_LIST | lf::SPACE_BETWEEN_BRACES) != 0 {
            self.write_space();
        }
        Ok(())
    }

    //
    // General
    //

    /// The `Write` dispatch, for the node kinds type display hands the printer.
    fn write_root(&mut self, node: NodeId) -> Result<(), Error> {
        let kind = self.known_kind(node)?;
        match kind {
            K::TemplateHead => self.emit_template_head(node),
            K::TemplateMiddle => self.emit_template_middle(node),
            K::TemplateTail => self.emit_template_tail(node),
            K::Identifier => self.emit_identifier_name(node),
            K::PrivateIdentifier => self.emit_private_identifier(node),
            K::QualifiedName => self.emit_qualified_name(node),
            K::ComputedPropertyName => self.emit_computed_property_name(node),
            K::TypeParameter => self.emit_type_parameter(node),
            K::Parameter => self.emit_parameter(node),
            K::PropertySignature => self.emit_property_signature(node),
            K::MethodSignature => self.emit_method_signature(node),
            K::CallSignature => self.emit_call_signature(node),
            K::ConstructSignature => self.emit_construct_signature(node),
            K::IndexSignature => self.emit_index_signature(node),
            _ if is_type_node_kind(kind) => self.emit_type_node_outside_extends(node),
            _ if ts_ast::utilities::is_expression_kind(kind.into()) => {
                self.emit_expression(node, op::LOWEST)
            }
            _ if is_keyword_kind(kind) => self.emit_keyword_node(Some(node)),
            _ if is_punctuation_kind(kind) => self.emit_punctuation_node(Some(node)),
            _ => Err(Error::Unsupported(
                "statements, declarations and JSDoc nodes",
            )),
        }
    }
}

// port: tsc/internal/printer/printer.go:getOpeningBracket
fn get_opening_bracket(format: ListFormat) -> Result<&'static [u8], Error> {
    Ok(match format & lf::BRACKETS_MASK {
        lf::BRACES => b"{",
        lf::PARENTHESIS => b"(",
        lf::ANGLE_BRACKETS => b"<",
        lf::SQUARE_BRACKETS => b"[",
        _ => return Err(Error::Unsupported("unexpected bracket format")),
    })
}

// port: tsc/internal/printer/printer.go:getClosingBracket
fn get_closing_bracket(format: ListFormat) -> Result<&'static [u8], Error> {
    Ok(match format & lf::BRACKETS_MASK {
        lf::BRACES => b"}",
        lf::PARENTHESIS => b")",
        lf::ANGLE_BRACKETS => b">",
        lf::SQUARE_BRACKETS => b"]",
        _ => return Err(Error::Unsupported("unexpected bracket format")),
    })
}
