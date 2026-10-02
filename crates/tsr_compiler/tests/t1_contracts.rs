//! Phase 3 T1 contracts (docs/PHASE3-plan.md, sections 4 and 5): the
//! printer's recursion and its per-file name generation, over production
//! entry points.
#[allow(dead_code)]
#[path = "support/phase3_contracts.rs"]
mod support;

use support::{files, Mode, SMALL_STACK};
use tsr_ast::{JsString, ParsedFile, SourceFileParseOptions};
use tsr_core::{CompilerOptions, NewLineKind, ScriptKind, ScriptTarget};
use tsr_jsstring::SourceText;
use tsr_printer::{EmitContext, Printer, PrinterOptions};

/// `text` parsed as `file_name`, with no parse diagnostics.
fn parse(file_name: &str, text: &str) -> ParsedFile {
    let file = tsr_parser::parse_source_file(
        SourceText::from_loaded_bytes(text.as_bytes().to_vec()),
        ScriptKind::from_file_name(file_name.as_bytes()),
        SourceFileParseOptions {
            file_name: JsString::from_bytes(file_name.as_bytes()),
            path: JsString::from_bytes(file_name.as_bytes()),
            ..Default::default()
        },
    );
    let diagnostics = &file
        .view()
        .source_file(file.root())
        .expect("a parsed source file")
        .diagnostics;
    assert!(diagnostics.is_empty(), "{file_name}: parse diagnostics");
    file
}

/// `file` printed back by `EmitSourceFile` with comments on, as the reprint
/// witness prints a file.
fn reprint(file: &ParsedFile) -> Vec<u8> {
    let emit_context = EmitContext::new();
    let mut printer = Printer::new(
        PrinterOptions {
            new_line: NewLineKind::LF,
            ..PrinterOptions::default()
        },
        &emit_context,
    );
    printer
        .emit_source_file(file.view(), file.root())
        .expect("the file prints")
}

/// Terms of the T1 binary chain: the plan's 100,000 in release. Printing a
/// chain reads each identifier's text through `GetSourceFileOfNode`, which
/// walks to the root as the pin's does, so the cost is quadratic in the
/// depth; the debug run prints a 20,000-term chain.
const TERMS: usize = if cfg!(debug_assertions) {
    20_000
} else {
    100_000
};

/// Nesting depth of the other T1 stress fixtures.
const NESTING: usize = 5_000;

/// The T1 stress fixtures (ADR 0011; the plan's binary chain, deeply nested
/// parentheses and deeply nested JSX), with the other shapes whose emitters
/// recurse once per nesting level: a right-associative chain, an `else if`
/// chain (`emitIfStatement` emits its `else if` directly) and nested
/// conditional types. Each is parsed on a reserved stack and printed by the
/// printer alone on a 256 KiB thread, so the printer's growth guards, not the
/// stack, carry the depth; each prints back its own source text.
#[test]
fn deep_inputs_reprint_through_the_printers_growth_guards() {
    let cases = [
        (
            "/binary.ts",
            format!("{};\n", support::repeat("a", TERMS, " + ")),
        ),
        (
            "/power.ts",
            format!("{};\n", support::repeat("a", TERMS, " ** ")),
        ),
        (
            "/parentheses.ts",
            format!("{}a{};\n", "(".repeat(TERMS), ")".repeat(TERMS)),
        ),
        (
            "/jsx.tsx",
            format!(
                "const x = {}{};\n",
                "<a>".repeat(NESTING),
                "</a>".repeat(NESTING)
            ),
        ),
        (
            "/else.ts",
            format!("if (a) {{ }}\n{}", "else if (a) { }\n".repeat(NESTING)),
        ),
        (
            "/conditional.ts",
            format!(
                "type T<X> = {}never;\n",
                "X extends 0 ? 0 : ".repeat(NESTING)
            ),
        ),
    ];
    for (file_name, text) in &cases {
        let file = support::on_reserved_stack(|| parse(file_name, text));
        let printed = support::on_stack(SMALL_STACK, || reprint(&file));
        assert!(
            printed == text.as_bytes(),
            "{file_name}: the deep input does not print back as itself"
        );
    }
}

/// The name generator and the emit context are per file
/// (`emitter.emitJSFile` creates both for each file): two identical files
/// that each need two temporary names (`_a`, `_b` for two downleveled
/// optional chains) print the same names in both, in both modes, rather than
/// continuing the first file's names in the second.
#[test]
fn generated_names_restart_in_each_file() {
    let text = "declare const o: any;\nexport const v = o.f()?.p;\nexport const w = o.g()?.q;\n";
    let program = files(&[
        ("/lib.d.ts", support::LIB),
        ("/a.ts", text),
        ("/b.ts", text),
        ("/c.ts", text),
    ]);
    let options = CompilerOptions {
        target: ScriptTarget::ES2019,
        ..CompilerOptions::default()
    };
    for mode in Mode::BOTH {
        let (checked, _) = support::checked(&program, &options, mode);
        let observed = support::emit_all(&checked);
        let first = observed.text("/a.js");
        let names = String::from_utf8_lossy(first);
        assert!(
            names.contains("var _a, _b;") && !names.contains("_c"),
            "{mode:?}: {names}"
        );
        for other in ["/b.js", "/c.js"] {
            assert_eq!(observed.text(other), first, "{mode:?}: {other}");
        }
    }
}
