use super::*;
use crate::bound_input::BoundInputOptions;

fn fixture(text: &[u8]) -> (BoundChecker, NodeId) {
    let file = ts_binder::bind_parsed_file(ts_parser::parse_source_file(
        ts_jsstring::SourceText::from_loaded_bytes(text),
        ts_core::ScriptKind::TS,
        ts_ast::SourceFileParseOptions {
            file_name: ts_ast::JsString::from_bytes(b"/fixture.ts".as_slice()),
            ..Default::default()
        },
    ))
    .unwrap();
    let source = file.source();
    let input = BoundInput::new(
        vec![file],
        BoundInputOptions {
            strict_null_checks: true,
            ..Default::default()
        },
        Vec::new(),
    )
    .unwrap();
    (BoundChecker::new(input).unwrap(), source)
}

fn named(checker: &BoundChecker, source: NodeId, name: &[u8]) -> Rc<TypeCell> {
    let node = checker
        .input()
        .declaration_by_name(source, name)
        .unwrap()
        .unwrap();
    checker.declared_type(node).unwrap()
}

#[test]
fn member_tables_do_not_pre_resolve_property_type_graphs() {
    let (owner, source) =
        fixture(b"type A = { next: { value: string } }; type B = { next: { value: string } };");
    let a = named(&owner, source, b"A");
    let b = named(&owner, source, b"B");
    let checker = owner.checker();
    let before = checker.graph.len();
    assert_eq!(a.resolutions(), 0);
    assert_eq!(a.members(&checker.graph).unwrap().len(), 1);
    assert_eq!(
        checker.graph.len(),
        before,
        "member discovery must not construct the nested type"
    );
    assert!(checker.is_type_assignable_to(&a, &b).unwrap());
    assert_eq!(
        checker.graph.len() - before,
        2,
        "nested types belong to the relation interval"
    );
    let after = checker.graph.len();
    assert!(checker.is_type_assignable_to(&a, &b).unwrap());
    assert_eq!(
        checker.graph.len(),
        after,
        "repeat relation retains resolved types"
    );
}

#[test]
fn an_escaped_cell_cannot_keep_the_checker_and_bound_program_alive() {
    let (owner, source) = fixture(b"type A = { next: { value: string } };");
    let a = named(&owner, source, b"A");
    let next = a.members(&owner.checker().graph).unwrap()[0].clone();
    let weak = Rc::downgrade(&owner.state);
    drop(owner);
    assert!(weak.upgrade().is_none());
    assert_eq!(next.r#type().unwrap_err(), Error::Released);
}

#[test]
fn unknown_type_syntax_fails_lazily_instead_of_publishing_an_empty_property() {
    let (owner, source) = fixture(b"type A = { value: import('missing').T };");
    let a = named(&owner, source, b"A");
    let member = &a.members(&owner.checker().graph).unwrap()[0];
    assert!(matches!(member.r#type(), Err(Error::Unsupported(_))));
    assert_eq!(member.r#type().unwrap_err(), Error::ResolutionFailed);
}

#[test]
fn mapped_constraint_is_eager_but_property_substitution_is_lazy() {
    let (owner, source) = fixture(b"type Map<T> = { [K in keyof T]: T[K] }; type A = Map<{ x: string; y: number }>; type B = { x: string; y: number };");
    let before = owner.checker().graph.len();
    let a = named(&owner, source, b"A");
    let b = named(&owner, source, b"B");
    assert_eq!(a.object_flags & of::MAPPED, of::MAPPED);
    assert_eq!(owner.checker().graph.len() - before, 8, "native constructors: parameter, mapped target, iteration parameter, constraint, argument, mapped instance, cloned iteration parameter, B");
    assert_eq!(a.resolutions(), 0);
    assert!(owner.checker().is_type_assignable_to(&a, &b).unwrap());
    let after = owner.checker().graph.len();
    assert!(owner.checker().is_type_assignable_to(&a, &b).unwrap());
    assert_eq!(owner.checker().graph.len(), after);
}

#[test]
fn index_signature_relations_construct_property_key_types_at_the_native_point() {
    let (owner, source) = fixture(b"type A = { x: string; y?: number; [key: string]: string | number | undefined }; type B = { x: string; y: number };");
    let a = named(&owner, source, b"A");
    let b = named(&owner, source, b"B");
    let checker = owner.checker();
    assert!(
        !checker
            .check_type_related_to(&a, &b, crate::Mode::Assignable, true)
            .unwrap()
            .1
    );
    let before = checker.graph.len();
    assert!(
        checker
            .check_type_related_to(&b, &a, crate::Mode::Assignable, true)
            .unwrap()
            .1
    );
    assert_eq!(
        checker.graph.len() - before,
        2,
        "the pinned reverse action constructs the x and y literal key types"
    );
    let after = checker.graph.len();
    assert!(
        checker
            .check_type_related_to(&b, &a, crate::Mode::Assignable, true)
            .unwrap()
            .1
    );
    assert_eq!(checker.graph.len(), after);
}

#[test]
fn union_error_elaboration_constructs_real_keyof_types() {
    let (owner, source) = fixture(b"type A = ({ a: string } & { b: number }) | { a: number }; type B = { a: string | number };");
    let a = named(&owner, source, b"A");
    let b = named(&owner, source, b"B");
    let checker = owner.checker();
    assert!(
        checker
            .check_type_related_to(&a, &b, crate::Mode::Assignable, true)
            .unwrap()
            .1
    );
    let before = checker.graph.len();
    assert!(
        !checker
            .check_type_related_to(&b, &a, crate::Mode::Assignable, true)
            .unwrap()
            .1
    );
    assert_eq!(
        checker.graph.len() - before,
        4,
        "the pin constructs the B index origin, two literal keys and their union"
    );
}
