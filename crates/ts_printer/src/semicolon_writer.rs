//! The writer that drops a trailing semicolon when nothing follows it
//! (`tsc/internal/printer/semicolon_writer.go`), used when `OmitTrailingSemicolon`
//! is set, as it is for `SymbolToString`.

use crate::EmitTextWriter;
use ts_ast::{NodeId, NodeListId, SymbolId};

pub struct TrailingSemicolonDeferringWriter<'a> {
    inner: &'a mut dyn EmitTextWriter,
    has_pending_semicolon: bool,
}

impl<'a> TrailingSemicolonDeferringWriter<'a> {
    // port: tsc/internal/printer/semicolon_writer.go:getTrailingSemicolonDeferringWriter
    pub fn new(inner: &'a mut dyn EmitTextWriter) -> Self {
        Self {
            inner,
            has_pending_semicolon: false,
        }
    }

    // port: tsc/internal/printer/semicolon_writer.go:trailingSemicolonDeferringWriter.commitSemicolon
    fn commit_semicolon(&mut self) {
        if self.has_pending_semicolon {
            self.inner.write_trailing_semicolon(b";");
            self.has_pending_semicolon = false;
        }
    }
}

impl EmitTextWriter for TrailingSemicolonDeferringWriter<'_> {
    // port: tsc/internal/printer/semicolon_writer.go:trailingSemicolonDeferringWriter.Write
    fn write(&mut self, s: &[u8]) {
        self.commit_semicolon();
        self.inner.write(s);
    }
    /// The semicolon is held back until something else is written.
    // port: tsc/internal/printer/semicolon_writer.go:trailingSemicolonDeferringWriter.WriteTrailingSemicolon
    fn write_trailing_semicolon(&mut self, _text: &[u8]) {
        self.has_pending_semicolon = true;
    }
    // port: tsc/internal/printer/semicolon_writer.go:trailingSemicolonDeferringWriter.WriteComment
    fn write_comment(&mut self, text: &[u8]) {
        self.commit_semicolon();
        self.inner.write_comment(text);
    }
    // port: tsc/internal/printer/semicolon_writer.go:trailingSemicolonDeferringWriter.WriteKeyword
    fn write_keyword(&mut self, text: &[u8]) {
        self.commit_semicolon();
        self.inner.write_keyword(text);
    }
    // port: tsc/internal/printer/semicolon_writer.go:trailingSemicolonDeferringWriter.WriteOperator
    fn write_operator(&mut self, text: &[u8]) {
        self.commit_semicolon();
        self.inner.write_operator(text);
    }
    // port: tsc/internal/printer/semicolon_writer.go:trailingSemicolonDeferringWriter.WritePunctuation
    fn write_punctuation(&mut self, text: &[u8]) {
        self.commit_semicolon();
        self.inner.write_punctuation(text);
    }
    // port: tsc/internal/printer/semicolon_writer.go:trailingSemicolonDeferringWriter.WriteSpace
    fn write_space(&mut self, text: &[u8]) {
        self.commit_semicolon();
        self.inner.write_space(text);
    }
    // port: tsc/internal/printer/semicolon_writer.go:trailingSemicolonDeferringWriter.WriteStringLiteral
    fn write_string_literal(&mut self, text: &[u8]) {
        self.commit_semicolon();
        self.inner.write_string_literal(text);
    }
    // port: tsc/internal/printer/semicolon_writer.go:trailingSemicolonDeferringWriter.WriteParameter
    fn write_parameter(&mut self, text: &[u8]) {
        self.commit_semicolon();
        self.inner.write_parameter(text);
    }
    // port: tsc/internal/printer/semicolon_writer.go:trailingSemicolonDeferringWriter.WriteProperty
    fn write_property(&mut self, text: &[u8]) {
        self.commit_semicolon();
        self.inner.write_property(text);
    }
    // port: tsc/internal/printer/semicolon_writer.go:trailingSemicolonDeferringWriter.WriteSymbol
    fn write_symbol(&mut self, text: &[u8], symbol: Option<SymbolId>) {
        self.commit_semicolon();
        self.inner.write_symbol(text, symbol);
    }
    // port: tsc/internal/printer/semicolon_writer.go:trailingSemicolonDeferringWriter.WriteLine
    fn write_line(&mut self) {
        self.commit_semicolon();
        self.inner.write_line();
    }
    // port: tsc/internal/printer/semicolon_writer.go:trailingSemicolonDeferringWriter.WriteLineForce
    fn write_line_force(&mut self, force: bool) {
        self.commit_semicolon();
        self.inner.write_line_force(force);
    }
    // port: tsc/internal/printer/semicolon_writer.go:trailingSemicolonDeferringWriter.IncreaseIndent
    fn increase_indent(&mut self) {
        self.commit_semicolon();
        self.inner.increase_indent();
    }
    // port: tsc/internal/printer/semicolon_writer.go:trailingSemicolonDeferringWriter.DecreaseIndent
    fn decrease_indent(&mut self) {
        self.commit_semicolon();
        self.inner.decrease_indent();
    }
    // port: tsc/internal/printer/semicolon_writer.go:trailingSemicolonDeferringWriter.Clear
    fn clear(&mut self) {
        self.has_pending_semicolon = false;
        self.inner.clear();
    }
    // port: tsc/internal/printer/semicolon_writer.go:trailingSemicolonDeferringWriter.String
    fn text(&self) -> &[u8] {
        self.inner.text()
    }
    // port: tsc/internal/printer/semicolon_writer.go:trailingSemicolonDeferringWriter.RawWrite
    fn raw_write(&mut self, s: &[u8]) {
        self.commit_semicolon();
        self.inner.raw_write(s);
    }
    // port: tsc/internal/printer/semicolon_writer.go:trailingSemicolonDeferringWriter.WriteLiteral
    fn write_literal(&mut self, s: &[u8]) {
        self.commit_semicolon();
        self.inner.write_literal(s);
    }
    // port: tsc/internal/printer/semicolon_writer.go:trailingSemicolonDeferringWriter.GetTextPos
    fn get_text_pos(&self) -> usize {
        self.inner.get_text_pos()
    }
    // port: tsc/internal/printer/semicolon_writer.go:trailingSemicolonDeferringWriter.GetLine
    fn get_line(&self) -> isize {
        self.inner.get_line()
    }
    // port: tsc/internal/printer/semicolon_writer.go:trailingSemicolonDeferringWriter.GetColumn
    fn get_column(&self) -> isize {
        self.inner.get_column()
    }
    // port: tsc/internal/printer/semicolon_writer.go:trailingSemicolonDeferringWriter.GetIndent
    fn get_indent(&self) -> isize {
        self.inner.get_indent()
    }
    // port: tsc/internal/printer/semicolon_writer.go:trailingSemicolonDeferringWriter.IsAtStartOfLine
    fn is_at_start_of_line(&self) -> bool {
        self.inner.is_at_start_of_line()
    }
    // port: tsc/internal/printer/semicolon_writer.go:trailingSemicolonDeferringWriter.HasTrailingComment
    fn has_trailing_comment(&self) -> bool {
        self.inner.has_trailing_comment()
    }
    // port: tsc/internal/printer/semicolon_writer.go:trailingSemicolonDeferringWriter.HasTrailingWhitespace
    fn has_trailing_whitespace(&self) -> bool {
        self.inner.has_trailing_whitespace()
    }

    // The notifications belong to the writer being wrapped.
    fn on_before_emit_node(&mut self, node: NodeId) {
        self.inner.on_before_emit_node(node);
    }
    fn on_after_emit_node(&mut self, node: NodeId) {
        self.inner.on_after_emit_node(node);
    }
    fn on_before_emit_node_list(&mut self, nodes: NodeListId) {
        self.inner.on_before_emit_node_list(nodes);
    }
    fn on_after_emit_node_list(&mut self, nodes: NodeListId) {
        self.inner.on_after_emit_node_list(nodes);
    }
    fn on_before_emit_token(&mut self, node: NodeId) {
        self.inner.on_before_emit_token(node);
    }
    fn on_after_emit_token(&mut self, node: NodeId) {
        self.inner.on_after_emit_token(node);
    }
}
