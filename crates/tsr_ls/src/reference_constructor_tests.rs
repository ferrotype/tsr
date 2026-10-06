use super::*;
use std::sync::Arc;
use tsr_core::CancellationToken;

fn reference_positions(text: &str) -> Vec<u32> {
    let program = Arc::new(crate::tests::program(b"/index.ts", text.as_bytes()));
    let source = program.source_file(b"/index.ts").unwrap().source();
    let counters = tsr_arena::Counters::new();
    let pool = tsr_compiler::CompilerCheckerPool::new(program.clone(), &counters);
    let mut checker = pool.checker_for_file_exclusive(source).unwrap();
    let mut service = LanguageService::new(
        &program,
        tsr_jsstring::PositionEncoding::Utf16,
        CancellationToken::new(),
    );
    service
        .references(
            &mut checker,
            &lsp::ReferenceParams {
                text_document: lsp::TextDocumentIdentifier {
                    uri: lsp::DocumentUri("file:///index.ts".into()),
                },
                position: lsp::Position {
                    line: 0,
                    character: text.find("constructor").unwrap() as u32,
                },
                context: Some(Box::new(lsp::ReferenceContext {
                    include_declaration: true,
                })),
                ..Default::default()
            },
        )
        .unwrap()
        .locations
        .unwrap_or_default()
        .iter()
        .map(|location| location.range.start.character)
        .collect()
}

// source: findAllRefsOfConstructor_test.go and findAllReferencesOfConstructor_test.go
#[test]
fn constructor_references_follow_extends_but_not_implements() {
    let text = "class A { constructor() {} } class B extends A {} class C extends A { constructor() { super(); } } class D implements A {} class E implements A { constructor() { super(); } } new A(); new B(); new D(); new E();";
    let positions = reference_positions(text);
    for token in [
        "constructor() {}",
        "super(); } } class D",
        "A(); new B",
        "B(); new D",
    ] {
        assert!(
            positions.contains(&(text.find(token).unwrap() as u32)),
            "missing {token}: {positions:?}"
        );
    }
    for token in ["super(); } } new A", "D(); new E", "E();"] {
        assert!(
            !positions.contains(&(text.find(token).unwrap() as u32)),
            "unexpected {token}: {positions:?}"
        );
    }
}
