use crate::emit_text_writer::decode_last_rune;
use crate::{get_default_indent_size, EmitTextWriter, SingleLineStringWriter, TextWriter};

#[test]
fn text_writer_indents_at_line_start_and_tracks_lines() {
    let mut w = TextWriter::new(b"\n", 0);
    assert_eq!(get_default_indent_size(), 4);
    assert!(w.is_at_start_of_line());
    w.increase_indent();
    assert_eq!(w.get_column(), 4, "pending indent counts before any text");
    w.write(b"a");
    assert_eq!(w.text(), b"    a");
    assert_eq!((w.get_line(), w.get_column(), w.get_indent()), (0, 5, 1));
    w.write_line();
    w.write_line();
    assert_eq!(
        w.text(),
        b"    a\n",
        "a second WriteLine at line start writes nothing"
    );
    assert_eq!(w.get_line(), 1);
    w.write_line_force(true);
    assert_eq!(w.text(), b"    a\n\n");
    w.decrease_indent();
    w.write(b"b");
    assert_eq!(w.text(), b"    a\n\nb");
    assert_eq!(w.get_text_pos(), 8);
}

#[test]
fn text_writer_counts_embedded_line_breaks_and_utf16_columns() {
    let mut w = TextWriter::new(b"\r\n", 2);
    w.write("x\r\ny\u{1F600}".as_bytes());
    assert_eq!(w.get_line(), 1);
    assert_eq!(
        w.get_column(),
        3,
        "y plus an astral scalar is three UTF-16 units"
    );
    assert!(!w.is_at_start_of_line());
    w.raw_write(b"z\n");
    assert_eq!(w.get_line(), 2);
    assert!(
        w.is_at_start_of_line(),
        "text ending in a break leaves the writer at line start"
    );
    // Upstream updates line state even for an empty RawWrite.
    w.raw_write(b"");
    assert!(!w.is_at_start_of_line());
    w.write_line();
    assert_eq!(w.text(), "x\r\ny\u{1F600}z\n\r\n".as_bytes());
}

#[test]
fn text_writer_trailing_state_and_clear() {
    let mut w = TextWriter::new(b"\n", 4);
    assert!(!w.has_trailing_whitespace());
    w.write_comment(b"// c");
    assert!(w.has_trailing_comment());
    w.write(b"");
    assert!(
        w.has_trailing_comment(),
        "an empty write does not clear the comment state"
    );
    w.write(b"a ");
    assert!(!w.has_trailing_comment());
    assert!(w.has_trailing_whitespace());
    w.write("\u{00A0}".as_bytes());
    assert!(
        w.has_trailing_whitespace(),
        "no-break space is whitespace-like"
    );
    w.write(&[0xE2, 0x80]);
    assert!(
        !w.has_trailing_whitespace(),
        "a truncated sequence decodes as RuneError"
    );
    w.increase_indent();
    w.clear();
    assert_eq!((w.text(), w.get_indent(), w.get_line()), (&b""[..], 0, 0));
    assert!(w.is_at_start_of_line());
    w.write(b"a");
    assert_eq!(
        w.text(),
        b"a",
        "Clear resets the indent but keeps the newline and indent size"
    );
    w.write_line();
    assert_eq!(w.text(), b"a\n");
}

#[test]
#[should_panic(expected = "nonnegative indent")]
fn text_writer_keeps_upstream_panic_on_negative_indent() {
    let mut w = TextWriter::new(b"\n", 4);
    w.decrease_indent();
    w.write(b"a");
}

#[test]
fn single_line_writer_flattens_lines_and_reports_no_position() {
    let mut w = SingleLineStringWriter::new();
    w.write_keyword(b"type");
    w.write_line();
    w.increase_indent();
    w.write_punctuation(b"{");
    w.write_line_force(false);
    assert_eq!(w.text(), b"type { ");
    assert_eq!((w.get_line(), w.get_column(), w.get_indent()), (0, 0, 0));
    assert!(!w.is_at_start_of_line());
    assert!(!w.has_trailing_comment());
    assert!(w.has_trailing_whitespace());
    w.write(b"x");
    assert!(!w.has_trailing_whitespace());
    assert_eq!(w.get_text_pos(), 8);
    w.clear();
    assert_eq!(w.text(), b"");
    assert!(!w.has_trailing_whitespace());
}

#[test]
fn last_rune_follows_go_standard_decoding() {
    assert_eq!(decode_last_rune(b""), None);
    assert_eq!(decode_last_rune(b"ab"), Some('b'));
    assert_eq!(decode_last_rune("a\u{00A0}".as_bytes()), Some('\u{00A0}'));
    assert_eq!(decode_last_rune("\u{1F600}".as_bytes()), Some('\u{1F600}'));
    assert_eq!(decode_last_rune(&[0xE2, 0x80]), None, "truncated sequence");
    assert_eq!(decode_last_rune(&[0x80]), None, "lone continuation byte");
    assert_eq!(
        decode_last_rune(&[0xED, 0xA0, 0x80]),
        None,
        "surrogate encoding is invalid UTF-8"
    );
    assert_eq!(
        decode_last_rune("\u{FFFD}".as_bytes()),
        None,
        "matches the RuneError comparison"
    );
    assert_eq!(decode_last_rune(&[0x80, 0x80, 0x80, 0x80, 0x80]), None);
}

// Native stack segments, rather than wasm/Miri's execution-stack model.
#[cfg(not(any(miri, target_family = "wasm")))]
#[test]
fn recursive_printer_grows_and_unwinds_without_retaining_session_state() {
    use crate::{EmitContext, Printer, PrinterOptions};
    use tsr_ast::{AstBuilder, FactoryMethods, JsString, SyntaxKind as K};
    struct StackWriter {
        writer: TextWriter,
        greatest_remaining: usize,
        panic_on_keyword: bool,
    }
    impl StackWriter {
        fn observe_stack(&mut self) {
            self.greatest_remaining = self.greatest_remaining.max(
                stacker::remaining_stack()
                    .expect("native growth test needs observable stack bounds"),
            );
        }
    }
    macro_rules! text_methods {
        ($($name:ident),* $(,)?) => {$(
            fn $name(&mut self, text: &[u8]) {
                self.observe_stack();
                self.writer.$name(text);
            }
        )*};
    }
    impl EmitTextWriter for StackWriter {
        text_methods!(
            write,
            write_trailing_semicolon,
            write_comment,
            write_operator,
            write_punctuation,
            write_space,
            write_string_literal,
            write_parameter,
            write_property,
            raw_write,
            write_literal
        );
        fn write_keyword(&mut self, text: &[u8]) {
            self.observe_stack();
            assert!(!self.panic_on_keyword, "writer panic after stack growth");
            self.writer.write_keyword(text);
        }
        fn write_symbol(&mut self, text: &[u8], symbol: Option<tsr_ast::SymbolId>) {
            self.observe_stack();
            self.writer.write_symbol(text, symbol);
        }
        fn write_line(&mut self) {
            self.writer.write_line();
        }
        fn write_line_force(&mut self, force: bool) {
            self.writer.write_line_force(force);
        }
        fn increase_indent(&mut self) {
            self.writer.increase_indent();
        }
        fn decrease_indent(&mut self) {
            self.writer.decrease_indent();
        }
        fn clear(&mut self) {
            self.writer.clear();
        }
        fn text(&self) -> &[u8] {
            self.writer.text()
        }
        fn get_text_pos(&self) -> usize {
            self.writer.get_text_pos()
        }
        fn get_line(&self) -> isize {
            self.writer.get_line()
        }
        fn get_column(&self) -> isize {
            self.writer.get_column()
        }
        fn get_indent(&self) -> isize {
            self.writer.get_indent()
        }
        fn is_at_start_of_line(&self) -> bool {
            self.writer.is_at_start_of_line()
        }
        fn has_trailing_comment(&self) -> bool {
            self.writer.has_trailing_comment()
        }
        fn has_trailing_whitespace(&self) -> bool {
            self.writer.has_trailing_whitespace()
        }
    }
    const STACK: usize = 512 * 1024;
    std::thread::Builder::new()
        .stack_size(STACK)
        .spawn(|| {
            let counters = tsr_arena::Counters::new();
            let before = counters.snapshot();
            {
                let emit = EmitContext::new();
                let mut ast = AstBuilder::new(
                    tsr_jsstring::SourceText::from_bytes(b"".as_slice()),
                    &counters,
                );
                let mut typ = ast.new_keyword_type_node(K::StringKeyword.into());
                let identifier = ast.new_identifier(JsString::from_bytes(b"x".as_slice()));
                let plus = ast.new_token(K::PlusToken.into());
                let mut expression = identifier;
                let mut qualified = identifier;
                let mut binding = identifier;
                let depth = 3000;
                for _ in 0..depth {
                    typ = ast.new_parenthesized_type_node(Some(typ));
                    qualified = ast.new_qualified_name(Some(qualified), Some(identifier));
                    let element = ast.new_binding_element(None, None, Some(binding), None);
                    let nodes = ast.node_slice(vec![Some(element)]).unwrap();
                    let elements = ast
                        .new_list(tsr_core::TextRange::new(-1, -1), nodes)
                        .unwrap();
                    binding =
                        ast.new_binding_pattern(K::ArrayBindingPattern.into(), Some(elements));
                    expression = ast.new_binary_expression(
                        None,
                        Some(expression),
                        None,
                        Some(plus),
                        Some(identifier),
                    );
                }
                let printer = Printer::new(PrinterOptions::default(), &emit);
                let mut writer = StackWriter {
                    writer: TextWriter::new(b"", 0),
                    greatest_remaining: 0,
                    panic_on_keyword: false,
                };
                printer
                    .write(ast.view(), expression, None, &mut writer)
                    .unwrap();
                assert_eq!(writer.text().len(), 1 + depth * 4);
                assert!(
                    writer.greatest_remaining > STACK,
                    "expression printer must visit a grown stack segment"
                );
                let binding =
                    ast.new_parameter_declaration(None, None, Some(binding), None, None, None);
                for (node, expected_length) in
                    [(qualified, 1 + 2 * depth), (binding, 1 + 2 * depth)]
                {
                    writer.greatest_remaining = 0;
                    printer.write(ast.view(), node, None, &mut writer).unwrap();
                    assert_eq!(writer.text().len(), expected_length);
                    assert!(
                        writer.greatest_remaining > STACK,
                        "nested name printing must grow the stack"
                    );
                }
                writer.greatest_remaining = 0;
                writer.panic_on_keyword = true;
                let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    printer.write(ast.view(), typ, None, &mut writer)
                }));
                assert!(panic.is_err());
                assert!(
                    writer.greatest_remaining > STACK,
                    "the writer panic must occur after native growth"
                );
                writer.panic_on_keyword = false;
                printer.write(ast.view(), typ, None, &mut writer).unwrap();
                assert_eq!(
                    writer.text(),
                    format!("{}string{}", "(".repeat(depth), ")".repeat(depth)).as_bytes()
                );
            }
            assert_eq!(
                counters.snapshot(),
                before,
                "printer failure must not retain AST storage"
            );
        })
        .unwrap()
        .join()
        .unwrap();
}
