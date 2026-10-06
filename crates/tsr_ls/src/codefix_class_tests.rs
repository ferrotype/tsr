use super::*;
use std::sync::Arc;
use tsr_core::CancellationToken;

fn fixes(text: &str) -> Vec<Fix> {
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
    let position = text.find("B implements").unwrap() as i64;
    service
        .class_fixes(
            &mut checker,
            source,
            TextRange::new(position, position),
            &CompletionOptions::default(),
            &tsr_locale::DEFAULT,
        )
        .unwrap()
}

// source: codeFixClassImplementInterfacePropertyFromParentConstructorFunction_test.go
#[test]
fn constructor_parameter_property_does_not_offer_implement_interface_fix() {
    assert!(
        fixes("class A { constructor(public x: number) {} } class B implements A {}").is_empty()
    );
    let property = fixes("class A { x: number; } class B implements A {}");
    assert_eq!(property.len(), 1);
    assert!(property[0]
        .edits
        .iter()
        .any(|edit| edit.new_text.contains("x: number")));
}

// source: codeFixClassImplementInterfaceComments_test.go
#[test]
fn implement_interface_retains_reused_signature_trailing_comments() {
    let actions = fixes(
        "interface A { foo<X /** angle */>(a: X /** paren */): string /** semicolon */; } class B implements A {}",
    );
    assert_eq!(actions.len(), 1);
    let text: String = actions[0]
        .edits
        .iter()
        .map(|edit| edit.new_text.as_str())
        .collect();
    assert!(text.contains("X /** angle */>"), "{text}");
    assert!(text.contains("X /** paren */)"), "{text}");
    assert!(text.contains("string /** semicolon */"), "{text}");
}

// source: codeFixClassImplementInterfaceNoTruncationProperties_test.go
#[test]
fn implement_interface_retains_large_type_elision_comments() {
    let props = (b'a'..=b'z')
        .map(|c| format!("\"{}\"", c as char))
        .collect::<Vec<_>>()
        .join(" | ");
    let actions = fixes(&format!(
        "type props = {props}; type manyprops = `${{props}}${{props}}`; interface A<T extends string> {{ foo(a: {{[K in T]: {{[K2 in T]: `${{K}}.${{K2}}`}}}}): void; }} class B implements A<manyprops> {{}}"
    ));
    assert_eq!(actions.len(), 1);
    let text: String = actions[0]
        .edits
        .iter()
        .map(|edit| edit.new_text.as_str())
        .collect();
    assert_eq!(text.matches("/*... 121 more elided ...*/").count(), 1);
    assert_eq!(text.matches("/*... 527 more elided ...*/").count(), 1);
    assert_eq!(text.matches("/*elided*/").count(), 1);
}
