use crate::{recursion::take_observations, Parser};
use ts_arena::Counters;
use ts_ast::{AstBuilder, JsString, SourceFileParseOptions, SyntaxKind};
use ts_core::ScriptKind;
use ts_jsstring::SourceText;

fn nested(prefix: &str, inner: &str, suffix: &str, depth: usize) -> Vec<u8> {
    format!("{}{}{}", prefix.repeat(depth), inner, suffix.repeat(depth)).into_bytes()
}

#[test]
fn recursive_grammar_grows_real_segments_from_a_small_initial_stack() {
    let cases = [
        ("parentheses", nested("(", "x", ")", 1200), ScriptKind::TS),
        ("unary", nested("!", "x", "", 2400), ScriptKind::TS),
        (
            "exponentiation",
            nested("x ** ", "x", "", 5000),
            ScriptKind::TS,
        ),
        ("statements", nested("if(x)", ";", "", 1600), ScriptKind::TS),
        (
            "conditional",
            nested("x ? x : ", "x", "", 2000),
            ScriptKind::TS,
        ),
        ("new", nested("new ", "X", "", 2000), ScriptKind::TS),
        (
            "namespaces",
            nested("namespace A {", "const x=0;", "}", 1600),
            ScriptKind::TS,
        ),
        (
            "bindings",
            [b"const ".as_slice(), &nested("[", "x", "]", 1600), b"=[];"].concat(),
            ScriptKind::TS,
        ),
        ("json", nested("[", "0", "]", 1600), ScriptKind::JSON),
        (
            "types",
            [b"type T=".as_slice(), &nested("(", "number", ")", 1200)].concat(),
            ScriptKind::TS,
        ),
        ("jsx", nested("<x>", "", "</x>", 600), ScriptKind::TSX),
        (
            "jsdoc",
            [
                b"/** @type {".as_slice(),
                &nested("Array<", "number", ">", 700),
                b"} */ const x=0;",
            ]
            .concat(),
            ScriptKind::JS,
        ),
    ];
    for (name, text, kind) in cases {
        let observation = std::thread::Builder::new()
            .name(format!("small-stack-{name}"))
            .stack_size(512 * 1024)
            .spawn(move || {
                let source = SourceText::from_loaded_bytes(text);
                let factory = AstBuilder::new(source.clone(), &Counters::default());
                let mut parser = Parser::new(
                    SourceFileParseOptions {
                        file_name: JsString::from_bytes(b"/depth.ts".as_slice()),
                        ..SourceFileParseOptions::default()
                    },
                    &source,
                    kind,
                    factory,
                );
                take_observations();
                parser.next_token();
                let root = if kind == ScriptKind::JSON {
                    parser.parse_json_text()
                } else {
                    parser.parse_source_file_worker()
                };
                assert_eq!(parser.token, SyntaxKind::EndOfFile, "{name}");
                assert!(
                    parser.diagnostics.is_empty(),
                    "{name}: {:?}",
                    parser.diagnostics
                );
                assert!(
                    parser.jsdoc_diagnostics.is_empty(),
                    "{name}: {:?}",
                    parser.jsdoc_diagnostics
                );
                parser.factory.complete(root).unwrap();
                take_observations()
            })
            .unwrap()
            .join()
            .unwrap();
        assert!(observation.entries > 500, "{name}: {observation:?}");
        assert!(observation.growths > 0, "{name}: {observation:?}");
    }
}
