//! Snippet escaping happens after formatting, at the writer's byte ranges.
//! Punctuation and raw `$0` tab stops are not escaped.
use crate::Result;
use tsr_ast::{AstBuilder, NodeId, NodeListId, SymbolId};
use tsr_printer::{ChangeTrackerWriter, EmitContext, EmitTextWriter, Printer, PrinterOptions};

/// Each formatted fragment gets its own source text and mutable positions.
/// Retain the generated graph once, then clone into a private formatting owner
/// per fragment; replacing an owner's already adopted text would invalidate
/// text handles and any lazy tokens created by the previous format operation.
pub(crate) fn print_many(
    ast: AstBuilder,
    roots: &[NodeId],
    emit: &EmitContext,
    source: &tsr_ast::SourceFileRead<'_>,
    settings: &tsr_format::FormatCodeSettings,
) -> Result<Vec<String>> {
    let Some(&first) = roots.first() else {
        return Ok(Vec::new());
    };
    let file = ast.complete(first)?.publish_unbound();
    roots
        .iter()
        .map(|&root| {
            let mut ast = AstBuilder::with_hooks(
                tsr_jsstring::SourceText::from_loaded_bytes(Vec::new()),
                &tsr_arena::Counters::new(),
                emit.factory_hooks(),
            );
            ast.retain_file(file.clone());
            let root = tsr_ast::deep_clone_node(&mut ast, Some(root)).expect("snippet root");
            print(&mut ast, root, emit, source, settings)
        })
        .collect()
}

pub(crate) fn print(
    ast: &mut AstBuilder,
    root: NodeId,
    emit: &EmitContext,
    source: &tsr_ast::SourceFileRead<'_>,
    settings: &tsr_format::FormatCodeSettings,
) -> Result<String> {
    let newline = &settings.editor.new_line_character;
    let mut writer = SnippetWriter {
        inner: ChangeTrackerWriter::new(newline, -1),
        escapes: Vec::new(),
    };
    let printer = Printer::new(
        PrinterOptions {
            remove_comments: true,
            new_line: if newline == b"\r\n" {
                tsr_core::NewLineKind::CRLF
            } else {
                tsr_core::NewLineKind::LF
            },
            ..Default::default()
        },
        emit,
    );
    printer.write(ast.view(), root, None, &mut writer, None)?;
    let text = writer.text().to_vec();
    writer.inner.assign_positions_to_node(ast, root)?;
    let file = tsr_printer::create_synthetic_source_file(
        ast,
        root,
        &text,
        source.parse_options().clone(),
    )?;
    let mut jsdoc = tsr_ast::EagerJsDocProvider::default();
    let mut input = tsr_format::FormatFile {
        view: ast.view(),
        source: file,
        jsdoc: &mut jsdoc,
    };
    let mut edits = tsr_format::format_node_given_indentation(
        &mut input,
        &tsr_format::FormatContext::new(settings.clone(), newline),
        root,
        source.language_variant,
        0,
        0,
    )?;
    edits.extend(writer.escapes);
    edits.sort_by_key(|e| (e.range.pos(), e.range.end()));
    let text = tsr_core::apply_bulk_edits(&text, &edits)?;
    Ok(String::from_utf8_lossy(&text).into_owned())
}
struct SnippetWriter {
    inner: ChangeTrackerWriter,
    escapes: Vec<tsr_core::TextChange>,
}
impl SnippetWriter {
    fn escaping(&mut self, text: &[u8], write: impl FnOnce(&mut ChangeTrackerWriter)) {
        let start = self.inner.get_text_pos();
        write(&mut self.inner);
        if text.contains(&b'$') {
            let mut escaped = Vec::new();
            for &c in text {
                if c == b'$' {
                    escaped.push(b'\\');
                }
                escaped.push(c);
            }
            self.escapes.push(tsr_core::TextChange {
                range: tsr_core::TextRange::new(start as i64, self.inner.get_text_pos() as i64),
                new_text: escaped,
            });
        }
    }
}

impl EmitTextWriter for SnippetWriter {
    fn write(&mut self, s: &[u8]) {
        self.escaping(s, |writer| writer.write(s));
    }
    fn write_trailing_semicolon(&mut self, text: &[u8]) {
        self.inner.write_trailing_semicolon(text);
    }
    fn write_comment(&mut self, text: &[u8]) {
        self.escaping(text, |writer| writer.write_comment(text));
    }
    fn write_keyword(&mut self, text: &[u8]) {
        self.inner.write_keyword(text);
    }
    fn write_operator(&mut self, text: &[u8]) {
        self.inner.write_operator(text);
    }
    fn write_punctuation(&mut self, text: &[u8]) {
        self.inner.write_punctuation(text);
    }
    fn write_space(&mut self, text: &[u8]) {
        self.inner.write_space(text);
    }
    fn write_string_literal(&mut self, text: &[u8]) {
        self.escaping(text, |writer| writer.write_string_literal(text));
    }
    fn write_parameter(&mut self, text: &[u8]) {
        self.escaping(text, |writer| writer.write_parameter(text));
    }
    fn write_property(&mut self, text: &[u8]) {
        self.escaping(text, |writer| writer.write_property(text));
    }
    fn write_symbol(&mut self, text: &[u8], symbol: Option<SymbolId>) {
        self.escaping(text, |writer| writer.write_symbol(text, symbol));
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
    }
    fn text(&self) -> &[u8] {
        self.inner.text()
    }
    fn raw_write(&mut self, s: &[u8]) {
        self.inner.raw_write(s);
    }
    fn write_literal(&mut self, s: &[u8]) {
        self.inner.write_literal(s);
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
