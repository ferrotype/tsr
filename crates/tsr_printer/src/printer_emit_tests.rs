//! `TestEmit` of `tsc/internal/printer/printer_test.go`, through
//! `emittestutil.CheckEmit`: parse, print the file with `EmitSourceFile`,
//! compare, then reparse the output and require no parse diagnostics.

#[path = "printer_emit_rows.rs"]
mod rows;

use crate::{EmitContext, Printer, PrinterOptions};
use tsr_ast::{JsString, ParsedFile, SourceFileParseOptions};
use tsr_core::{NewLineKind, ScriptKind};
use tsr_jsstring::SourceText;

/// `parsetestutil.ParseTypeScript`.
pub(crate) fn parse_type_script(text: &[u8], jsx: bool) -> ParsedFile {
    let file_name: &[u8] = if jsx { b"/main.tsx" } else { b"/main.ts" };
    tsr_parser::parse_source_file(
        SourceText::from_loaded_bytes(text.to_vec()),
        if jsx { ScriptKind::TSX } else { ScriptKind::TS },
        SourceFileParseOptions {
            file_name: JsString::from_bytes(file_name),
            path: JsString::from_bytes(file_name),
            ..Default::default()
        },
    )
}

/// The parse diagnostics of a file, as `code@pos` for a failure message.
pub(crate) fn parse_diagnostics(file: &ParsedFile) -> Vec<String> {
    let view = file.view();
    view.source_file(file.root())
        .expect("a parsed source file")
        .diagnostics
        .iter()
        .map(|diagnostic| format!("TS{}@{}", diagnostic.code, diagnostic.loc.pos()))
        .collect()
}

/// `emittestutil.CheckEmit`, returning a failure description instead of
/// failing the test. `jsx` is the language variant of the reparse.
pub(crate) fn check_emit(
    emit_context: &EmitContext,
    view: tsr_ast::AstView<'_>,
    file: tsr_ast::NodeId,
    jsx: bool,
    expected: &str,
) -> Result<(), String> {
    let mut printer = Printer::new(
        PrinterOptions {
            new_line: NewLineKind::LF,
            ..Default::default()
        },
        emit_context,
    );
    let text = printer
        .emit_source_file(view, file)
        .map_err(|error| format!("printer error: {error}"))?;
    let actual = text.strip_suffix(b"\n").unwrap_or(&text);
    if actual != expected.as_bytes() {
        return Err(format!(
            "expected {expected:?}\n  actual {:?}",
            String::from_utf8_lossy(actual)
        ));
    }
    let reparsed = parse_type_script(&text, jsx);
    let diagnostics = parse_diagnostics(&reparsed);
    if !diagnostics.is_empty() {
        return Err(format!("error on reparse: {diagnostics:?}"));
    }
    Ok(())
}

#[test]
fn test_emit() {
    let mut failures = Vec::new();
    for (index, &(title, input, output, jsx)) in rows::EMIT_ROWS.iter().enumerate() {
        let file = parse_type_script(input.as_bytes(), jsx);
        let diagnostics = parse_diagnostics(&file);
        if !diagnostics.is_empty() {
            failures.push(format!(
                "[{index}] {title}: parse diagnostics {diagnostics:?}"
            ));
            continue;
        }
        let emit_context = EmitContext::new();
        if let Err(failure) = check_emit(&emit_context, file.view(), file.root(), jsx, output) {
            failures.push(format!("[{index}] {title}: {failure}"));
        }
    }
    assert_eq!(rows::EMIT_ROWS.len(), 553);
    assert!(
        failures.is_empty(),
        "{} of {} rows failed:\n{}",
        failures.len(),
        rows::EMIT_ROWS.len(),
        failures.join("\n")
    );
}

/// A 20,000-term left-nested binary expression and a 5,000-deep
/// parenthesized expression print on a 512 KiB thread, comments on: the
/// emitters that recurse once per nested node grow the stack. The expected
/// text was derived by reading the pinned Go printer: both statements keep
/// their source spelling and the file's multi-line statement list ends each
/// with a line break.
#[test]
fn deep_nesting_prints_on_a_small_stack() {
    const STACK: usize = 512 * 1024;
    std::thread::Builder::new()
        .stack_size(STACK)
        .spawn(|| {
            let binary = vec!["a"; 20_000].join(" + ");
            let parenthesized = format!("{}a{}", "(".repeat(5_000), ")".repeat(5_000));
            let text = format!("{binary};\n{parenthesized};\n");
            let file = parse_type_script(text.as_bytes(), false);
            assert!(parse_diagnostics(&file).is_empty());
            let emit_context = EmitContext::new();
            let mut printer = Printer::new(
                PrinterOptions {
                    new_line: NewLineKind::LF,
                    ..Default::default()
                },
                &emit_context,
            );
            let output = printer
                .emit_source_file(file.view(), file.root())
                .expect("deep nesting prints");
            assert!(
                output == text.as_bytes(),
                "deep nesting reprints its source"
            );
        })
        .expect("spawn")
        .join()
        .expect("no stack overflow");
}

#[path = "printer_comment_rows.rs"]
mod comment_rows;

/// Comment emission (leading, trailing, detached, pinned, JSDoc-only,
/// shebang, triple-slash directives, multi-line re-indentation, comments in
/// empty lists and JSX expressions) and preserved source lines, against the
/// pinned Go printer's output captured over the same sources.
#[test]
fn comment_emission_matches_the_captured_go_output() {
    let mut failures = Vec::new();
    for (index, &(file_name, source, configuration, expected)) in
        comment_rows::COMMENT_ROWS.iter().enumerate()
    {
        let file = tsr_parser::parse_source_file(
            SourceText::from_loaded_bytes(source.to_vec()),
            ScriptKind::from_file_name(file_name.as_bytes()),
            SourceFileParseOptions {
                file_name: JsString::from_bytes(file_name.as_bytes()),
                path: JsString::from_bytes(file_name.as_bytes()),
                ..Default::default()
            },
        );
        let mut options = PrinterOptions {
            new_line: NewLineKind::LF,
            ..Default::default()
        };
        match configuration {
            1 => options.remove_comments = true,
            2 => options.preserve_source_newlines = true,
            3 => options.only_print_js_doc_style = true,
            4 => {
                options.preserve_source_newlines = true;
                options.remove_comments = true;
            }
            _ => {}
        }
        let emit_context = EmitContext::new();
        let mut printer = Printer::new(options, &emit_context);
        match printer.emit_source_file(file.view(), file.root()) {
            Ok(actual) if actual == expected => {}
            Ok(actual) => failures.push(format!(
                "[{index}] {file_name} c{configuration}: expected {:?}\n  actual {:?}",
                String::from_utf8_lossy(expected),
                String::from_utf8_lossy(&actual)
            )),
            Err(error) => failures.push(format!("[{index}] {file_name}: {error}")),
        }
    }
    assert_eq!(comment_rows::COMMENT_ROWS.len(), 45);
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

fn statements_of(file: &ParsedFile) -> Vec<tsr_ast::NodeId> {
    let view = file.view();
    let list = view
        .node(file.root())
        .expect("root")
        .statement_list()
        .expect("statements");
    view.node_slice(view.list(list).expect("list").nodes())
        .expect("slice")
        .iter()
        .map(|node| node.expect("statement"))
        .collect()
}

fn emit_with(
    emit_context: &EmitContext,
    view: tsr_ast::AstView<'_>,
    file: tsr_ast::NodeId,
) -> Vec<u8> {
    let mut printer = Printer::new(
        PrinterOptions {
            new_line: NewLineKind::LF,
            ..Default::default()
        },
        emit_context,
    );
    printer.emit_source_file(view, file).expect("prints")
}

/// Synthetic comments, comment ranges and the comment emit flags, against the
/// pinned Go printer's output captured for the same trees and flags.
#[test]
fn synthetic_comments_and_comment_flags_match_the_captured_go_output() {
    use crate::emit_flags as ef;
    use tsr_ast::{AstBuilder, FactoryMethods, SyntaxKind as K};

    // Synthetic comments on nodes of the emit context's factory.
    let mut emit_context = EmitContext::new();
    let counters = tsr_arena::Counters::new();
    let mut ast = AstBuilder::with_hooks(
        SourceText::from_loaded_bytes(&b""[..]),
        &counters,
        emit_context.factory_hooks(),
    );
    let a = ast.new_identifier(JsString::from_bytes(&b"a"[..]));
    let statement = ast.new_expression_statement(Some(a));
    emit_context.add_synthetic_leading_comment(
        statement,
        K::MultiLineCommentTrivia,
        JsString::from_bytes(&b"c"[..]),
        false,
    );
    emit_context.add_synthetic_trailing_comment(
        statement,
        K::SingleLineCommentTrivia,
        JsString::from_bytes(&b"t"[..]),
        false,
    );
    let b = ast.new_identifier(JsString::from_bytes(&b"b"[..]));
    let statement2 = ast.new_expression_statement(Some(b));
    emit_context.add_synthetic_leading_comment(
        statement2,
        K::SingleLineCommentTrivia,
        JsString::from_bytes(&b"s"[..]),
        true,
    );
    emit_context.add_synthetic_trailing_comment(
        statement2,
        K::MultiLineCommentTrivia,
        JsString::from_bytes(&b" m\n   * x "[..]),
        true,
    );
    let slice = ast
        .node_slice(vec![Some(statement), Some(statement2)])
        .expect("slice");
    let statements = ast
        .new_list(tsr_core::TextRange::new(-1, -1), slice)
        .expect("list");
    let eof = ast.new_token(K::EndOfFile.into());
    let file = ast.new_source_file(
        SourceFileParseOptions {
            file_name: JsString::from_bytes(&b"/file.ts"[..]),
            path: JsString::from_bytes(&b"/file.ts"[..]),
            ..Default::default()
        },
        SourceText::from_loaded_bytes(&b""[..]),
        Some(statements),
        Some(eof),
    );
    assert_eq!(
        emit_with(&emit_context, ast.view(), file),
        b"/*c*/ a; //t\n//s\nb; /* m\n   * x */\n"
    );

    // NoComments on a parsed statement.
    let parsed = parse_type_script(b"// x\na; // y\nb; // z\n", false);
    let mut emit_context = EmitContext::new();
    emit_context.set_emit_flags(statements_of(&parsed)[0], ef::NO_COMMENTS);
    assert_eq!(
        emit_with(&emit_context, parsed.view(), parsed.root()),
        b"a;\nb; // z\n"
    );

    // NoNestedComments on a parsed function.
    let parsed = parse_type_script(
        b"// before\nfunction f() {\n    // inner\n    a; // t\n} // after\nb; // keep\n",
        false,
    );
    let mut emit_context = EmitContext::new();
    emit_context.set_emit_flags(statements_of(&parsed)[0], ef::NO_NESTED_COMMENTS);
    assert_eq!(
        emit_with(&emit_context, parsed.view(), parsed.root()),
        b"// before\nfunction f() {\n    a;\n} // after\nb; // keep\n"
    );

    // NoLeadingComments and NoTrailingComments, one statement each.
    let parsed = parse_type_script(b"/*l1*/ a /*t1*/;\n/*l2*/ b /*t2*/;\n", false);
    let mut emit_context = EmitContext::new();
    let statements = statements_of(&parsed);
    emit_context.set_emit_flags(statements[0], ef::NO_LEADING_COMMENTS);
    emit_context.set_emit_flags(statements[1], ef::NO_TRAILING_COMMENTS);
    assert_eq!(
        emit_with(&emit_context, parsed.view(), parsed.root()),
        b"a /*t1*/;\n/*l2*/ b /*t2*/;\n"
    );

    // A parsed statement's comment range on a synthesized statement.
    let text = b"/*keep*/ a; // tail\n";
    let parsed = parse_type_script(text, false);
    let range = parsed
        .view()
        .node(statements_of(&parsed)[0])
        .expect("statement")
        .range();
    let mut emit_context = EmitContext::new();
    let mut ast = AstBuilder::with_hooks(
        SourceText::from_loaded_bytes(&text[..]),
        &counters,
        emit_context.factory_hooks(),
    );
    let z = ast.new_identifier(JsString::from_bytes(&b"z"[..]));
    let statement = ast.new_expression_statement(Some(z));
    emit_context.set_comment_range(statement, range);
    let slice = ast.node_slice(vec![Some(statement)]).expect("slice");
    let statements = ast
        .new_list(tsr_core::TextRange::new(-1, -1), slice)
        .expect("list");
    let eof = ast.new_token(K::EndOfFile.into());
    let file = ast.new_source_file(
        SourceFileParseOptions {
            file_name: JsString::from_bytes(&b"/main.ts"[..]),
            path: JsString::from_bytes(&b"/main.ts"[..]),
            ..Default::default()
        },
        SourceText::from_loaded_bytes(&text[..]),
        Some(statements),
        Some(eof),
    );
    assert_eq!(
        emit_with(&emit_context, ast.view(), file),
        b"/*keep*/ z; // tail\n"
    );
}
