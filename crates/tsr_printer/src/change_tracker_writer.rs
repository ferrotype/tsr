//! The writer that remembers where each node and list was printed
//! (`tsc/internal/printer/changetrackerwriter.go`), and positioned printing
//! built on it (`tsc/internal/printer/syntheticfile.go`).
//!
//! Upstream assigns the recorded positions to a clone of the tree, so that the
//! caller's node can be printed again. The one caller here prints a tree it
//! decoded for that request and drops it afterwards, so the positions are
//! assigned in place and no second tree is built.

use crate::emit_text_writer::decode_last_rune;
use crate::{EmitContext, EmitTextWriter, Error, Printer, PrinterOptions, TextWriter};
use std::collections::HashMap;
use std::ops::ControlFlow;
use tsr_ast::{
    AstBuilder, ChildVisitor, FactoryMethods, NodeData, NodeId, NodeListId, NodeSlice,
    SourceFileParseOptions, SymbolId, SyntaxKind as K,
};
use tsr_core::{NewLineKind, TextRange};
use tsr_jsstring::SourceText;
use tsr_scanner::is_white_space_like;

pub struct ChangeTrackerWriter {
    inner: TextWriter,
    last_non_trivia_position: i64,
    pos: HashMap<NodeId, i64>,
    end: HashMap<NodeId, i64>,
    list_pos: HashMap<NodeListId, i64>,
    list_end: HashMap<NodeListId, i64>,
}

/// A child of a node: a node, or a list with its nodes.
enum Child {
    Node(NodeId),
    List(NodeListId),
    Slice(NodeSlice),
}

struct Children(Vec<Child>);

impl ChildVisitor for Children {
    fn visit_node(&mut self, node: NodeId) -> ControlFlow<()> {
        self.0.push(Child::Node(node));
        ControlFlow::Continue(())
    }
    fn visit_list(&mut self, nodes: NodeListId) -> ControlFlow<()> {
        self.0.push(Child::List(nodes));
        ControlFlow::Continue(())
    }
    fn visit_node_slice(&mut self, nodes: NodeSlice) -> ControlFlow<()> {
        self.0.push(Child::Slice(nodes));
        ControlFlow::Continue(())
    }
}

impl ChangeTrackerWriter {
    /// A negative indent size selects the default; zero is kept.
    // port: tsc/internal/printer/changetrackerwriter.go:NewChangeTrackerWriter
    pub fn new(new_line: &[u8], indent_size: isize) -> Self {
        Self {
            inner: TextWriter::with_indent_size(new_line, indent_size),
            last_non_trivia_position: 0,
            pos: HashMap::new(),
            end: HashMap::new(),
            list_pos: HashMap::new(),
            list_end: HashMap::new(),
        }
    }

    // port: tsc/internal/printer/changetrackerwriter.go:ChangeTrackerWriter.setLastNonTriviaPosition
    fn set_last_non_trivia_position(&mut self, s: &[u8], force: bool) {
        if force || tsr_scanner::skip_trivia(s, 0) != s.len() as i64 {
            self.last_non_trivia_position = self.inner.get_text_pos() as i64;
            // Trim trailing whitespace.
            let mut pos = s.len();
            while pos > 0 {
                match decode_last_rune(&s[..pos]) {
                    Some(rune) if is_white_space_like(rune as i32) => pos -= rune.len_utf8(),
                    _ => break,
                }
            }
            self.last_non_trivia_position -= (s.len() - pos) as i64;
        }
    }

    /// A node or list the printer never entered has position zero, as a
    /// missing key of upstream's maps has.
    fn range_of_node(&self, node: NodeId) -> TextRange {
        TextRange::new(
            self.pos.get(&node).copied().unwrap_or(0),
            self.end.get(&node).copied().unwrap_or(0),
        )
    }

    fn range_of_list(&self, list: NodeListId) -> TextRange {
        TextRange::new(
            self.list_pos.get(&list).copied().unwrap_or(0),
            self.list_end.get(&list).copied().unwrap_or(0),
        )
    }

    /// Gives every node and list under `node` the position it was printed at.
    ///
    /// Upstream's visitor lifts an absent embedded statement into an empty,
    /// synthesized block, so an `if` without an `else` comes out with one. The
    /// formatter then refuses the synthesized node. That is reproduced.
    // port: tsc/internal/printer/changetrackerwriter.go:ChangeTrackerWriter.AssignPositionsToNode
    // port: tsc/internal/printer/changetrackerwriter.go:ChangeTrackerWriter.assignPositionsToNodeWorker
    // port: tsc/internal/printer/changetrackerwriter.go:ChangeTrackerWriter.assignPositionsToNodeArray
    pub fn assign_positions_to_node(
        &self,
        builder: &mut AstBuilder,
        node: NodeId,
    ) -> Result<(), Error> {
        let mut pending = vec![node];
        while let Some(current) = pending.pop() {
            let mut children = Children(Vec::new());
            let lifts_else = {
                let read = builder.view().node(current)?;
                let _ = read.for_each_child(&mut children);
                read.data_source()
                    .as_if_statement()
                    .is_some_and(|statement| statement.else_statement().is_none())
            };
            for child in children.0 {
                match child {
                    Child::Node(child) => pending.push(child),
                    Child::List(list) => {
                        let nodes = builder.view().list(list)?.nodes();
                        pending.extend(builder.view().node_slice(nodes)?.iter().flatten());
                        builder.set_list_location(list, self.range_of_list(list))?;
                    }
                    Child::Slice(nodes) => {
                        pending.extend(builder.view().node_slice(nodes)?.iter().flatten());
                    }
                }
            }
            if lifts_else {
                let statements = builder.new_list(TextRange::new(-1, -1), NodeSlice::empty())?;
                let block = builder.new_block(Some(statements), true);
                builder.node_mut(block)?.set_parent(Some(current));
                match builder.node_mut(current)?.data_mut() {
                    NodeData::IfStatement(data) => data.else_statement = Some(block),
                    _ => return Err(Error::MissingNode("if statement payload")),
                }
            }
            builder
                .node_mut(current)?
                .set_range(self.range_of_node(current));
        }
        Ok(())
    }
}

impl EmitTextWriter for ChangeTrackerWriter {
    fn write(&mut self, s: &[u8]) {
        self.inner.write(s);
        self.set_last_non_trivia_position(s, false);
    }
    fn write_trailing_semicolon(&mut self, text: &[u8]) {
        self.inner.write_trailing_semicolon(text);
        self.set_last_non_trivia_position(text, false);
    }
    fn write_comment(&mut self, text: &[u8]) {
        self.inner.write_comment(text);
    }
    fn write_keyword(&mut self, text: &[u8]) {
        self.inner.write_keyword(text);
        self.set_last_non_trivia_position(text, false);
    }
    fn write_operator(&mut self, text: &[u8]) {
        self.inner.write_operator(text);
        self.set_last_non_trivia_position(text, false);
    }
    fn write_punctuation(&mut self, text: &[u8]) {
        self.inner.write_punctuation(text);
        self.set_last_non_trivia_position(text, false);
    }
    fn write_space(&mut self, text: &[u8]) {
        self.inner.write_space(text);
        self.set_last_non_trivia_position(text, false);
    }
    fn write_string_literal(&mut self, text: &[u8]) {
        self.inner.write_string_literal(text);
        self.set_last_non_trivia_position(text, false);
    }
    fn write_parameter(&mut self, text: &[u8]) {
        self.inner.write_parameter(text);
        self.set_last_non_trivia_position(text, false);
    }
    fn write_property(&mut self, text: &[u8]) {
        self.inner.write_property(text);
        self.set_last_non_trivia_position(text, false);
    }
    fn write_symbol(&mut self, text: &[u8], symbol: Option<SymbolId>) {
        self.inner.write_symbol(text, symbol);
        self.set_last_non_trivia_position(text, false);
    }
    fn write_line(&mut self) {
        self.inner.write_line();
    }
    fn write_line_force(&mut self, force: bool) {
        self.inner.write_line_force(force);
    }
    fn increase_indent(&mut self) {
        self.inner.increase_indent();
    }
    fn decrease_indent(&mut self) {
        self.inner.decrease_indent();
    }
    fn clear(&mut self) {
        self.inner.clear();
        self.last_non_trivia_position = 0;
    }
    fn text(&self) -> &[u8] {
        self.inner.text()
    }
    fn raw_write(&mut self, s: &[u8]) {
        self.inner.raw_write(s);
        self.set_last_non_trivia_position(s, false);
    }
    fn write_literal(&mut self, s: &[u8]) {
        self.inner.write_literal(s);
        self.set_last_non_trivia_position(s, true);
    }
    fn get_text_pos(&self) -> usize {
        self.inner.get_text_pos()
    }
    fn get_line(&self) -> isize {
        self.inner.get_line()
    }
    fn get_column(&self) -> isize {
        self.inner.get_column()
    }
    fn get_indent(&self) -> isize {
        self.inner.get_indent()
    }
    fn is_at_start_of_line(&self) -> bool {
        self.inner.is_at_start_of_line()
    }
    fn has_trailing_comment(&self) -> bool {
        self.inner.has_trailing_comment()
    }
    fn has_trailing_whitespace(&self) -> bool {
        self.inner.has_trailing_whitespace()
    }

    // port: tsc/internal/printer/changetrackerwriter.go:ChangeTrackerWriter.GetPrintHandlers
    fn on_before_emit_node(&mut self, node: NodeId) {
        self.pos.insert(node, self.last_non_trivia_position);
    }
    fn on_after_emit_node(&mut self, node: NodeId) {
        self.end.insert(node, self.last_non_trivia_position);
    }
    fn on_before_emit_node_list(&mut self, nodes: NodeListId) {
        self.list_pos.insert(nodes, self.last_non_trivia_position);
    }
    fn on_after_emit_node_list(&mut self, nodes: NodeListId) {
        self.list_end.insert(nodes, self.last_non_trivia_position);
    }
    fn on_before_emit_token(&mut self, node: NodeId) {
        self.pos.insert(node, self.last_non_trivia_position);
    }
    fn on_after_emit_token(&mut self, node: NodeId) {
        self.end.insert(node, self.last_non_trivia_position);
    }
}

/// Prints a synthesized node with the change tracker's options, trims the
/// trailing new line, and assigns the printed positions to the tree. No source
/// file is passed: the one caller has none.
// port: tsc/internal/printer/syntheticfile.go:PrintAndPositionNode
pub fn print_and_position_node(
    builder: &mut AstBuilder,
    node: NodeId,
    new_line: &[u8],
    indent_size: isize,
    emit_context: &EmitContext,
) -> Result<Vec<u8>, Error> {
    let mut writer = ChangeTrackerWriter::new(new_line, indent_size);
    let printer = Printer::new(
        PrinterOptions {
            // port: tsc/internal/core/compileroptions.go:GetNewLineKind
            new_line: match new_line {
                b"\r\n" => NewLineKind::CRLF,
                b"\n" => NewLineKind::LF,
                _ => NewLineKind::NONE,
            },
            never_ascii_escape: true,
            preserve_source_newlines: true,
            terminate_unterminated_literals: true,
            ..PrinterOptions::default()
        },
        emit_context,
    );
    printer.write(builder.view(), node, None, &mut writer)?;
    let mut text = writer.text().to_vec();
    if !new_line.is_empty() && text.ends_with(new_line) {
        text.truncate(text.len() - new_line.len());
    }
    writer.assign_positions_to_node(builder, node)?;
    Ok(text)
}

/// Wraps a positioned node in a synthetic source file the formatter can work
/// on. The node must already carry the positions it was printed at. The file's
/// one statement is that node, and its text is the printed text.
// port: tsc/internal/printer/syntheticfile.go:CreateSyntheticSourceFile
pub fn create_synthetic_source_file(
    builder: &mut AstBuilder,
    node: NodeId,
    text: &[u8],
    parse_options: SourceFileParseOptions,
) -> Result<NodeId, Error> {
    let length = text.len() as i64;
    let end_of_file = builder.new_token(K::EndOfFile.into());
    builder
        .node_mut(end_of_file)?
        .set_range(TextRange::new(length, length));
    let range = builder.view().node(node)?.range();
    let nodes = builder.node_slice_from_slice(&[Some(node)])?;
    let statements = builder.new_list(range, nodes)?;
    // The decoded tree's storage has no text of its own. Tokens created later
    // for this file are checked against the storage's text, so it adopts the
    // printed text.
    let source = SourceText::from_loaded_bytes(text.to_vec());
    builder.adopt_source(source.clone())?;
    let file = builder.new_source_file(parse_options, source, Some(statements), Some(end_of_file));
    builder.node_mut(file)?.set_range(TextRange::new(0, length));
    tsr_ast::set_parent_in_children(builder, file);
    Ok(file)
}
