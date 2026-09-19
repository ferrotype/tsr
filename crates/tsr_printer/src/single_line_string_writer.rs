//! The single-line writer behind `SymbolToString` and friends
//! (`tsc/internal/printer/singlelinestringwriter.go`).
//!
//! Upstream hands these out from a `sync.Pool`; that is an allocation-reuse
//! policy, not observable behavior, and is left to the checker's call sites once
//! their allocation traffic is measured. As in `TextWriter`, the last written
//! text is the buffer's tail, so only its length is kept.

use crate::emit_text_writer::{decode_last_rune, EmitTextWriter};
use ts_ast::SymbolId;
use ts_scanner::is_white_space_like;

#[derive(Debug, Default)]
pub struct SingleLineStringWriter {
    builder: Vec<u8>,
    last_written_len: usize,
}

impl SingleLineStringWriter {
    pub fn new() -> Self {
        Self::default()
    }

    fn append(&mut self, s: &[u8]) {
        self.builder.extend_from_slice(s);
        self.last_written_len = s.len();
    }

    fn last_written(&self) -> &[u8] {
        &self.builder[self.builder.len() - self.last_written_len..]
    }
}

impl EmitTextWriter for SingleLineStringWriter {
    // port: tsc/internal/printer/singlelinestringwriter.go:singleLineStringWriter.Write
    fn write(&mut self, s: &[u8]) {
        self.append(s);
    }
    // port: tsc/internal/printer/singlelinestringwriter.go:singleLineStringWriter.WriteTrailingSemicolon
    fn write_trailing_semicolon(&mut self, text: &[u8]) {
        self.append(text);
    }
    // port: tsc/internal/printer/singlelinestringwriter.go:singleLineStringWriter.WriteComment
    fn write_comment(&mut self, text: &[u8]) {
        self.append(text);
    }
    // port: tsc/internal/printer/singlelinestringwriter.go:singleLineStringWriter.WriteKeyword
    fn write_keyword(&mut self, text: &[u8]) {
        self.append(text);
    }
    // port: tsc/internal/printer/singlelinestringwriter.go:singleLineStringWriter.WriteOperator
    fn write_operator(&mut self, text: &[u8]) {
        self.append(text);
    }
    // port: tsc/internal/printer/singlelinestringwriter.go:singleLineStringWriter.WritePunctuation
    fn write_punctuation(&mut self, text: &[u8]) {
        self.append(text);
    }
    // port: tsc/internal/printer/singlelinestringwriter.go:singleLineStringWriter.WriteSpace
    fn write_space(&mut self, text: &[u8]) {
        self.append(text);
    }
    // port: tsc/internal/printer/singlelinestringwriter.go:singleLineStringWriter.WriteStringLiteral
    fn write_string_literal(&mut self, text: &[u8]) {
        self.append(text);
    }
    // port: tsc/internal/printer/singlelinestringwriter.go:singleLineStringWriter.WriteParameter
    fn write_parameter(&mut self, text: &[u8]) {
        self.append(text);
    }
    // port: tsc/internal/printer/singlelinestringwriter.go:singleLineStringWriter.WriteProperty
    fn write_property(&mut self, text: &[u8]) {
        self.append(text);
    }
    // port: tsc/internal/printer/singlelinestringwriter.go:singleLineStringWriter.WriteSymbol
    fn write_symbol(&mut self, text: &[u8], _symbol: Option<SymbolId>) {
        self.append(text);
    }
    /// A line break becomes one space on a single line.
    // port: tsc/internal/printer/singlelinestringwriter.go:singleLineStringWriter.WriteLine
    fn write_line(&mut self) {
        self.append(b" ");
    }
    // port: tsc/internal/printer/singlelinestringwriter.go:singleLineStringWriter.WriteLineForce
    fn write_line_force(&mut self, _force: bool) {
        self.append(b" ");
    }
    // port: tsc/internal/printer/singlelinestringwriter.go:singleLineStringWriter.IncreaseIndent
    fn increase_indent(&mut self) {}
    // port: tsc/internal/printer/singlelinestringwriter.go:singleLineStringWriter.DecreaseIndent
    fn decrease_indent(&mut self) {}
    // port: tsc/internal/printer/singlelinestringwriter.go:singleLineStringWriter.Clear
    fn clear(&mut self) {
        self.builder.clear();
        self.last_written_len = 0;
    }
    // port: tsc/internal/printer/singlelinestringwriter.go:singleLineStringWriter.String
    fn text(&self) -> &[u8] {
        &self.builder
    }
    // port: tsc/internal/printer/singlelinestringwriter.go:singleLineStringWriter.RawWrite
    fn raw_write(&mut self, s: &[u8]) {
        self.append(s);
    }
    // port: tsc/internal/printer/singlelinestringwriter.go:singleLineStringWriter.WriteLiteral
    fn write_literal(&mut self, s: &[u8]) {
        self.append(s);
    }
    // port: tsc/internal/printer/singlelinestringwriter.go:singleLineStringWriter.GetTextPos
    fn get_text_pos(&self) -> usize {
        self.builder.len()
    }
    // port: tsc/internal/printer/singlelinestringwriter.go:singleLineStringWriter.GetLine
    fn get_line(&self) -> isize {
        0
    }
    // port: tsc/internal/printer/singlelinestringwriter.go:singleLineStringWriter.GetColumn
    fn get_column(&self) -> isize {
        0
    }
    // port: tsc/internal/printer/singlelinestringwriter.go:singleLineStringWriter.GetIndent
    fn get_indent(&self) -> isize {
        0
    }
    // port: tsc/internal/printer/singlelinestringwriter.go:singleLineStringWriter.IsAtStartOfLine
    fn is_at_start_of_line(&self) -> bool {
        false
    }
    // port: tsc/internal/printer/singlelinestringwriter.go:singleLineStringWriter.HasTrailingComment
    fn has_trailing_comment(&self) -> bool {
        false
    }
    // port: tsc/internal/printer/singlelinestringwriter.go:singleLineStringWriter.HasTrailingWhitespace
    fn has_trailing_whitespace(&self) -> bool {
        if self.builder.is_empty() {
            return false;
        }
        decode_last_rune(self.last_written()).is_some_and(|ch| is_white_space_like(ch as i32))
    }
}
