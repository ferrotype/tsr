//! The indenting, line-tracking writer (`tsc/internal/printer/textwriter.go`).
//!
//! Upstream keeps the last written string as a separate field. Every write
//! appends that string to the buffer last, and `Clear` resets both, so the last
//! written text is always the buffer's trailing `last_written_len` bytes. This
//! port stores the length and borrows the tail instead of copying each write.

use crate::emit_text_writer::{decode_last_rune, EmitTextWriter};
use ts_ast::SymbolId;
use ts_jsstring::line_map::{compute_ecma_line_starts_seq, utf16_len};
use ts_scanner::is_white_space_like;

const DEFAULT_INDENT_SIZE: isize = 4;

/// The default indent size (4 spaces) used when no specific indent size is configured.
// port: tsc/internal/printer/textwriter.go:GetDefaultIndentSize
pub fn get_default_indent_size() -> isize {
    DEFAULT_INDENT_SIZE
}

// port: tsc/internal/printer/textwriter.go:getIndentString
fn indent_string(indent: isize, indent_size: isize) -> Vec<u8> {
    if indent == 0 {
        return Vec::new();
    }
    // Upstream's strings.Repeat panics on a negative count; keep that contract.
    let count = usize::try_from(indent * indent_size)
        .expect("strings.Repeat requires a nonnegative indent");
    vec![b' '; count]
}

#[derive(Debug)]
pub struct TextWriter {
    new_line: Vec<u8>,
    indent_size: isize,
    builder: Vec<u8>,
    last_written_len: usize,
    indent: isize,
    line_start: bool,
    line_count: isize,
    line_pos: usize,
    has_trailing_comment_state: bool,
}

impl TextWriter {
    /// A non-positive indent size selects the default of four spaces.
    // port: tsc/internal/printer/textwriter.go:NewTextWriter
    pub fn new(new_line: &[u8], indent_size: isize) -> Self {
        let indent_size = if indent_size <= 0 {
            DEFAULT_INDENT_SIZE
        } else {
            indent_size
        };
        let mut writer = Self {
            new_line: new_line.to_vec(),
            indent_size,
            builder: Vec::new(),
            last_written_len: 0,
            indent: 0,
            line_start: false,
            line_count: 0,
            line_pos: 0,
            has_trailing_comment_state: false,
        };
        writer.clear();
        writer
    }

    /// The change tracker's writer keeps an indent size of zero, which
    /// `new` would replace with the default; only a negative one is replaced.
    pub(crate) fn with_indent_size(new_line: &[u8], indent_size: isize) -> Self {
        let mut writer = Self::new(new_line, indent_size);
        if indent_size == 0 {
            writer.indent_size = 0;
        }
        writer
    }

    // port: tsc/internal/printer/textwriter.go:textWriter.Grow
    pub fn grow(&mut self, additional: usize) {
        self.builder.reserve(additional);
    }

    fn last_written(&self) -> &[u8] {
        &self.builder[self.builder.len() - self.last_written_len..]
    }

    fn append(&mut self, s: &[u8]) {
        self.builder.extend_from_slice(s);
        self.last_written_len = s.len();
    }

    // port: tsc/internal/printer/textwriter.go:textWriter.updateLineCountAndPosFor
    fn update_line_count_and_pos_for(&mut self, s: &[u8]) {
        let mut count: isize = 0;
        let mut last_line_start: i32 = 0;
        for line_start in compute_ecma_line_starts_seq(s) {
            count += 1;
            last_line_start = line_start;
        }
        if count > 1 {
            self.line_count += count - 1;
            let current_len = self.builder.len();
            self.line_pos = current_len - s.len() + last_line_start as usize;
            self.line_start = self.line_pos == current_len;
            return;
        }
        self.line_start = false;
    }

    // port: tsc/internal/printer/textwriter.go:textWriter.writeText
    fn write_text(&mut self, s: &[u8]) {
        if !s.is_empty() {
            if self.line_start {
                let indent = indent_string(self.indent, self.indent_size);
                self.builder.extend_from_slice(&indent);
                self.line_start = false;
            }
            self.append(s);
            self.update_line_count_and_pos_for(s);
        }
    }

    // port: tsc/internal/printer/textwriter.go:textWriter.writeLineRaw
    fn write_line_raw(&mut self) {
        let new_line = std::mem::take(&mut self.new_line);
        self.append(&new_line);
        self.new_line = new_line;
        self.line_count += 1;
        self.line_pos = self.builder.len();
        self.line_start = true;
        self.has_trailing_comment_state = false;
    }
}

impl EmitTextWriter for TextWriter {
    // port: tsc/internal/printer/textwriter.go:textWriter.Write
    fn write(&mut self, s: &[u8]) {
        if !s.is_empty() {
            self.has_trailing_comment_state = false;
        }
        self.write_text(s);
    }
    // port: tsc/internal/printer/textwriter.go:textWriter.WriteTrailingSemicolon
    fn write_trailing_semicolon(&mut self, text: &[u8]) {
        self.write(text);
    }
    // port: tsc/internal/printer/textwriter.go:textWriter.WriteComment
    fn write_comment(&mut self, text: &[u8]) {
        if !text.is_empty() {
            self.has_trailing_comment_state = true;
        }
        self.write_text(text);
    }
    // port: tsc/internal/printer/textwriter.go:textWriter.WriteKeyword
    fn write_keyword(&mut self, text: &[u8]) {
        self.write(text);
    }
    // port: tsc/internal/printer/textwriter.go:textWriter.WriteOperator
    fn write_operator(&mut self, text: &[u8]) {
        self.write(text);
    }
    // port: tsc/internal/printer/textwriter.go:textWriter.WritePunctuation
    fn write_punctuation(&mut self, text: &[u8]) {
        self.write(text);
    }
    // port: tsc/internal/printer/textwriter.go:textWriter.WriteSpace
    fn write_space(&mut self, text: &[u8]) {
        self.write(text);
    }
    // port: tsc/internal/printer/textwriter.go:textWriter.WriteStringLiteral
    fn write_string_literal(&mut self, text: &[u8]) {
        self.write(text);
    }
    // port: tsc/internal/printer/textwriter.go:textWriter.WriteParameter
    fn write_parameter(&mut self, text: &[u8]) {
        self.write(text);
    }
    // port: tsc/internal/printer/textwriter.go:textWriter.WriteProperty
    fn write_property(&mut self, text: &[u8]) {
        self.write(text);
    }
    // port: tsc/internal/printer/textwriter.go:textWriter.WriteSymbol
    fn write_symbol(&mut self, text: &[u8], _symbol: Option<SymbolId>) {
        self.write(text);
    }
    // port: tsc/internal/printer/textwriter.go:textWriter.WriteLine
    fn write_line(&mut self) {
        if !self.line_start {
            self.write_line_raw();
        }
    }
    // port: tsc/internal/printer/textwriter.go:textWriter.WriteLineForce
    fn write_line_force(&mut self, force: bool) {
        if !self.line_start || force {
            self.write_line_raw();
        }
    }
    // port: tsc/internal/printer/textwriter.go:textWriter.IncreaseIndent
    fn increase_indent(&mut self) {
        self.indent += 1;
    }
    // port: tsc/internal/printer/textwriter.go:textWriter.DecreaseIndent
    fn decrease_indent(&mut self) {
        self.indent -= 1;
    }
    /// Keeps the newline and indent size; resets everything else, at line start.
    // port: tsc/internal/printer/textwriter.go:textWriter.Clear
    fn clear(&mut self) {
        self.builder.clear();
        self.last_written_len = 0;
        self.indent = 0;
        self.line_start = true;
        self.line_count = 0;
        self.line_pos = 0;
        self.has_trailing_comment_state = false;
    }
    // port: tsc/internal/printer/textwriter.go:textWriter.String
    fn text(&self) -> &[u8] {
        &self.builder
    }
    /// Writes without indentation. Upstream updates line state even for empty
    /// input, which leaves the writer not at line start; that is preserved.
    // port: tsc/internal/printer/textwriter.go:textWriter.RawWrite
    fn raw_write(&mut self, s: &[u8]) {
        if !s.is_empty() {
            self.append(s);
            self.has_trailing_comment_state = false;
        }
        self.update_line_count_and_pos_for(s);
    }
    // port: tsc/internal/printer/textwriter.go:textWriter.WriteLiteral
    fn write_literal(&mut self, s: &[u8]) {
        self.write(s);
    }
    // port: tsc/internal/printer/textwriter.go:textWriter.GetTextPos
    fn get_text_pos(&self) -> usize {
        self.builder.len()
    }
    // port: tsc/internal/printer/textwriter.go:textWriter.GetLine
    fn get_line(&self) -> isize {
        self.line_count
    }
    /// UTF-16 units since the last line start; a pending indent counts as its
    /// spaces before anything is written on the line.
    // port: tsc/internal/printer/textwriter.go:textWriter.GetColumn
    fn get_column(&self) -> isize {
        if self.line_start {
            return self.indent * self.indent_size;
        }
        utf16_len(&self.builder[self.line_pos..])
    }
    // port: tsc/internal/printer/textwriter.go:textWriter.GetIndent
    fn get_indent(&self) -> isize {
        self.indent
    }
    // port: tsc/internal/printer/textwriter.go:textWriter.IsAtStartOfLine
    fn is_at_start_of_line(&self) -> bool {
        self.line_start
    }
    // port: tsc/internal/printer/textwriter.go:textWriter.HasTrailingComment
    fn has_trailing_comment(&self) -> bool {
        self.has_trailing_comment_state
    }
    // port: tsc/internal/printer/textwriter.go:textWriter.HasTrailingWhitespace
    fn has_trailing_whitespace(&self) -> bool {
        if self.builder.is_empty() {
            return false;
        }
        decode_last_rune(self.last_written()).is_some_and(|ch| is_white_space_like(ch as i32))
    }
}
