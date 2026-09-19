//! The externally opaque writer contract (`printer.EmitTextWriter`,
//! `tsc/internal/printer/emittextwriter.go`).

use tsr_ast::{NodeId, NodeListId, SymbolId};

/// One method per upstream interface member. Text is bytes; positions are byte
/// offsets except `get_column`, which counts UTF-16 units (`core.UTF16Offset`, a
/// Go `int`) for source maps. `text` borrows the accumulated output where
/// upstream's `String()` copies it.
pub trait EmitTextWriter {
    fn write(&mut self, s: &[u8]);
    fn write_trailing_semicolon(&mut self, text: &[u8]);
    fn write_comment(&mut self, text: &[u8]);
    fn write_keyword(&mut self, text: &[u8]);
    fn write_operator(&mut self, text: &[u8]);
    fn write_punctuation(&mut self, text: &[u8]);
    fn write_space(&mut self, text: &[u8]);
    fn write_string_literal(&mut self, text: &[u8]);
    fn write_parameter(&mut self, text: &[u8]);
    fn write_property(&mut self, text: &[u8]);
    fn write_symbol(&mut self, text: &[u8], symbol: Option<SymbolId>);
    fn write_line(&mut self);
    fn write_line_force(&mut self, force: bool);
    fn increase_indent(&mut self);
    fn decrease_indent(&mut self);
    fn clear(&mut self);
    fn text(&self) -> &[u8];
    fn raw_write(&mut self, s: &[u8]);
    fn write_literal(&mut self, s: &[u8]);
    fn get_text_pos(&self) -> usize;
    fn get_line(&self) -> isize;
    fn get_column(&self) -> isize;
    fn get_indent(&self) -> isize;
    fn is_at_start_of_line(&self) -> bool;
    fn has_trailing_comment(&self) -> bool;
    fn has_trailing_whitespace(&self) -> bool;

    // Upstream's `PrintHandlers` emit notifications. Its only provider of
    // these six is the change tracker's writer, whose handlers read the
    // writer's own state, so they are part of the writer contract here rather
    // than a second object aliasing the writer. They default to nothing.

    /// `PrintHandlers.OnBeforeEmitNode`.
    fn on_before_emit_node(&mut self, _node: NodeId) {}
    /// `PrintHandlers.OnAfterEmitNode`.
    fn on_after_emit_node(&mut self, _node: NodeId) {}
    /// `PrintHandlers.OnBeforeEmitNodeList`.
    fn on_before_emit_node_list(&mut self, _nodes: NodeListId) {}
    /// `PrintHandlers.OnAfterEmitNodeList`.
    fn on_after_emit_node_list(&mut self, _nodes: NodeListId) {}
    /// `PrintHandlers.OnBeforeEmitToken`.
    fn on_before_emit_token(&mut self, _node: NodeId) {}
    /// `PrintHandlers.OnAfterEmitToken`.
    fn on_after_emit_token(&mut self, _node: NodeId) {}
}

/// Upstream's `utf8.DecodeLastRuneInString`, reduced to the question the writers
/// ask: the last complete, standard-UTF-8 scalar, or `None` where Go would
/// return `RuneError`. This is Go's strict decoder, not the JavaScript sentinel
/// decoder in `tsr_jsstring::wtf8`; the two differ on lone surrogates, and the
/// writers follow upstream's `unicode/utf8` here. A literally encoded U+FFFD is
/// also `None`, as upstream compares the decoded rune against `RuneError`.
pub(crate) fn decode_last_rune(bytes: &[u8]) -> Option<char> {
    let end = bytes.len();
    let last = *bytes.last()?;
    if last < 0x80 {
        return Some(char::from(last));
    }
    // Look back at most four bytes for a start byte, as upstream does.
    let limit = end.saturating_sub(4);
    let mut start = end - 1;
    while start > limit && bytes[start] & 0xC0 == 0x80 {
        start -= 1;
    }
    let mut chars = std::str::from_utf8(&bytes[start..end]).ok()?.chars();
    let rune = chars.next()?;
    if chars.next().is_some() || rune == '\u{FFFD}' {
        return None;
    }
    Some(rune)
}
