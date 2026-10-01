//! `TestTypeEraser` of `typeeraser_test.go` and `TestPartiallyEmittedExpression`
//! of `printer/printer_test.go`: parse, run the type eraser with the row's
//! compiler options, print the file with `EmitSourceFile` (`emittestutil.CheckEmit`),
//! compare, then reparse the output and require no parse diagnostics.
use super::new_type_eraser_transformer;
use crate::transformer_tests::Unasked;
use crate::{Failure, TransformOptions};
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use tsr_arena::Counters;
use tsr_ast::{AstBuilder, JsString, NodeId, ParsedFile, SourceFileParseOptions};
use tsr_core::{CompilerOptions, ModuleKind, NewLineKind, ScriptKind, Tristate};
use tsr_jsstring::SourceText;
use tsr_printer::{EmitContext, Printer, PrinterOptions};

/// `(title, input, output, jsx, vms)`, in the Go table's order.
#[rustfmt::skip]
const ROWS: &[(&str, &str, &str, bool, bool)] = &[
    ("Modifiers", "class C { public x; private y }", "class C {\n    x;\n    y;\n}", false, false),
    ("InterfaceDeclaration", "interface I { }", "", false, false),
    ("TypeAliasDeclaration", "type T = U;", "", false, false),
    ("NamespaceExportDeclaration", "export as namespace N;", "", false, false),
    ("UninstantiatedNamespace1", "namespace N {}", "", false, false),
    ("UninstantiatedNamespace2", "namespace N { export interface I {} }", "", false, false),
    ("UninstantiatedNamespace3", "namespace N { export type T = U; }", "", false, false),
    ("ExpressionWithTypeArguments", "F<T>", "F;", false, false),
    ("PropertyDeclaration1", "class C { declare x; }", "class C {\n}", false, false),
    ("PropertyDeclaration2", "class C { public x: number; }", "class C {\n    x;\n}", false, false),
    ("PropertyDeclaration3", "class C { public static x: number; }", "class C {\n    static x;\n}", false, false),
    ("ConstructorDeclaration1", "class C { constructor(); }", "class C {\n}", false, false),
    ("ConstructorDeclaration2", "class C { public constructor() {} }", "class C {\n    constructor() { }\n}", false, false),
    ("MethodDeclaration1", "class C { m(); }", "class C {\n}", false, false),
    ("MethodDeclaration2", "class C { public m<T>(): U {} }", "class C {\n    m() { }\n}", false, false),
    ("MethodDeclaration3", "class C { public static m<T>(): U {} }", "class C {\n    static m() { }\n}", false, false),
    ("GetAccessorDeclaration1", "class C { get m(); }", "class C {\n    get m() { }\n}", false, false),
    ("GetAccessorDeclaration2", "class C { public get m<T>(): U {} }", "class C {\n    get m() { }\n}", false, false),
    ("GetAccessorDeclaration3", "class C { public static get m<T>(): U {} }", "class C {\n    static get m() { }\n}", false, false),
    ("SetAccessorDeclaration1", "class C { set m(v); }", "class C {\n    set m(v) { }\n}", false, false),
    ("SetAccessorDeclaration2", "class C { public set m<T>(v): U {} }", "class C {\n    set m(v) { }\n}", false, false),
    ("SetAccessorDeclaration3", "class C { public static set m<T>(v): U {} }", "class C {\n    static set m(v) { }\n}", false, false),
    ("IndexSignature", "class C { [key: string]: number; }", "class C {\n}", false, false),
    ("VariableDeclaration1", "declare var a;", "", false, false),
    ("VariableDeclaration2", "var a: number", "var a;", false, false),
    ("HeritageClause", "class C implements I {}", "class C {\n}", false, false),
    ("ClassDeclaration1", "declare class C {}", "", false, false),
    ("ClassDeclaration2", "class C<T> {}", "class C {\n}", false, false),
    ("ClassExpression", "(class C<T> {})", "(class C {\n});", false, false),
    ("FunctionDeclaration1", "declare function f() {}", "", false, false),
    ("FunctionDeclaration2", "function f();", "", false, false),
    ("FunctionDeclaration3", "function f<T>(): U {}", "function f() { }", false, false),
    ("FunctionExpression", "(function f<T>(): U {})", "(function f() { });", false, false),
    ("ArrowFunction", "(<T>(): U => {})", "(() => { });", false, false),
    ("ParameterDeclaration", "function f(this: x, a: number, b?: boolean) {}", "function f(a, b) { }", false, false),
    ("CallExpression", "f<T>()", "f();", false, false),
    ("NewExpression1", "new f<T>()", "new f();", false, false),
    ("NewExpression2", "new f<T>", "new f;", false, false),
    ("TaggedTemplateExpression", "f<T>``", "f ``;", false, false),
    ("NonNullExpression", "x!", "x;", false, false),
    ("TypeAssertionExpression#1", "<T>x", "x;", false, false),
    ("TypeAssertionExpression#2", "(<T>x).c", "x.c;", false, false),
    ("AsExpression#1", "x as T", "x;", false, false),
    ("AsExpression#2", "(x as T).c", "x.c;", false, false),
    ("SatisfiesExpression#1", "x satisfies T", "x;", false, false),
    ("SatisfiesExpression#2", "(x satisfies T).c", "x.c;", false, false),
    ("JsxSelfClosingElement", "<x<T> />", "<x />;", true, false),
    ("JsxOpeningElement", "<x<T>></x>", "<x></x>;", true, false),
    ("ImportEqualsDeclaration#1", "import x = require(\"m\");", "import x = require(\"m\");", false, false),
    ("ImportEqualsDeclaration#2", "import type x = require(\"m\");", "", false, false),
    ("ImportEqualsDeclaration#3", "import x = y;", "import x = y;", false, false),
    ("ImportEqualsDeclaration#4", "import type x = y;", "", false, false),
    ("ImportDeclaration#1", "import \"m\";", "import \"m\";", false, false),
    ("ImportDeclaration#2", "import * as x from \"m\"; x;", "import * as x from \"m\";\nx;", false, false),
    ("ImportDeclaration#3", "import x from \"m\"; x;", "import x from \"m\";\nx;", false, false),
    ("ImportDeclaration#4", "import { x } from \"m\"; x;", "import { x } from \"m\";\nx;", false, false),
    ("ImportDeclaration#5", "import type * as x from \"m\";", "", false, false),
    ("ImportDeclaration#6", "import type x from \"m\";", "", false, false),
    ("ImportDeclaration#7", "import type { x } from \"m\";", "", false, false),
    ("ImportDeclaration#8", "import { type x } from \"m\";", "", false, false),
    ("ImportDeclaration#9", "import { type x } from \"m\";", "import {} from \"m\";", false, true),
    ("ExportDeclaration#1", "export * from \"m\";", "export * from \"m\";", false, false),
    ("ExportDeclaration#2", "export * as x from \"m\";", "export * as x from \"m\";", false, false),
    ("ExportDeclaration#3", "export { x } from \"m\";", "export { x } from \"m\";", false, false),
    ("ExportDeclaration#4", "export type * from \"m\";", "", false, false),
    ("ExportDeclaration#5", "export type * as x from \"m\";", "", false, false),
    ("ExportDeclaration#6", "export type { x } from \"m\";", "", false, false),
    ("ExportDeclaration#7", "export { type x } from \"m\";", "", false, false),
    ("ExportDeclaration#7", "export { type x } from \"m\";", "export {} from \"m\";", false, true),
];

/// `parsetestutil.ParseTypeScript`.
fn parse_type_script(text: &[u8], jsx: bool) -> ParsedFile {
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

/// The parse diagnostics of a file (`parsetestutil.CheckDiagnostics`), as
/// `code@pos` for a failure message.
fn parse_diagnostics(file: &ParsedFile) -> Vec<String> {
    let view = file.view();
    view.source_file(file.root())
        .expect("a parsed source file")
        .diagnostics
        .iter()
        .map(|diagnostic| format!("TS{}@{}", diagnostic.code, diagnostic.loc.pos()))
        .collect()
}

/// Parses `input`, runs the type eraser over it with `compiler_options` and
/// `context`, and returns the factory holding the result with its root.
fn transform(
    input: &str,
    jsx: bool,
    compiler_options: CompilerOptions,
    context: &EmitContext,
) -> Result<(AstBuilder, NodeId), String> {
    let parsed = parse_type_script(input.as_bytes(), jsx);
    let diagnostics = parse_diagnostics(&parsed);
    if !diagnostics.is_empty() {
        return Err(format!("parse diagnostics {diagnostics:?}"));
    }
    let mut factory = AstBuilder::with_hooks(
        SourceText::default(),
        &Counters::new(),
        context.factory_hooks(),
    );
    let root = parsed.root();
    factory.retain_file(parsed.publish_unbound());
    let opts = TransformOptions {
        context: context.clone(),
        compiler_options: Arc::new(compiler_options),
        resolver: Rc::new(RefCell::new(Unasked)),
        emit_resolver: Rc::new(RefCell::new(Unasked)),
        get_emit_module_format_of_file: Rc::new(|_| Ok(ModuleKind::NONE)),
        failure: Failure::default(),
    };
    let transformer = new_type_eraser_transformer(&opts).expect("a type eraser");
    let file = transformer
        .transform_source_file(&mut factory, root)
        .map_err(|error| format!("transform failed: {error}"))?;
    drop(transformer);
    Ok((factory, file))
}

/// `emittestutil.CheckEmit`, returning a failure description.
fn check_emit(
    emit_context: &EmitContext,
    factory: &AstBuilder,
    file: NodeId,
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
        .emit_source_file(factory.view(), file)
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
fn test_type_eraser() {
    assert_eq!(ROWS.len(), 69);
    let mut failures = Vec::new();
    for &(title, input, output, jsx, vms) in ROWS {
        let mut compiler_options = CompilerOptions::default();
        if vms {
            compiler_options.verbatim_module_syntax = Tristate::TRUE;
        }
        // The transformer gets a new context; `CheckEmit` is given none, so
        // the printer makes its own.
        let result = transform(input, jsx, compiler_options, &EmitContext::new()).and_then(
            |(factory, file)| check_emit(&EmitContext::new(), &factory, file, jsx, output),
        );
        if let Err(error) = result {
            failures.push(format!("{title}: {error}"));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn test_partially_emitted_expression() {
    let compiler_options = CompilerOptions::default();
    let emit_context = EmitContext::new();
    let (factory, file) = transform(
        "return ((container.parent\n    .left as PropertyAccessExpression)\n    .expression as PropertyAccessExpression)\n    .expression;",
        false,
        compiler_options,
        &emit_context,
    )
    .expect("transforms");
    check_emit(
        &emit_context,
        &factory,
        file,
        false,
        "return container.parent\n    .left\n    .expression\n    .expression;",
    )
    .unwrap();
}
