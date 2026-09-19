use super::*;
use crate::bound_input::BoundInputOptions;

fn fixture(text: &[u8]) -> (BoundChecker, NodeId) {
    let file = tsr_binder::bind_parsed_file(tsr_parser::parse_source_file(
        tsr_jsstring::SourceText::from_loaded_bytes(text),
        tsr_core::ScriptKind::TS,
        tsr_ast::SourceFileParseOptions {
            file_name: tsr_ast::JsString::from_bytes(b"/fixture.ts".as_slice()),
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

#[test]
fn conditional_infer_parameters_scope_into_nested_extends_clause_types() {
    // A conditional contributes its `infer` parameters to the outer type
    // parameters of every node below it, so the function type written in the
    // extends clause is instantiable through them. Without that scope the
    // inferred mapper substitutes nothing, the extends check fails and the
    // conditional takes its false branch.
    for text in [
        b"declare function fn0(x: string): number; type A = typeof fn0 extends (...args: any) => infer R ? R : any; type B = number;".as_slice(),
        b"type Returned<T> = T extends (...args: any) => infer R ? R : any; declare function fn0(x: string): number; type A = Returned<typeof fn0>; type B = number;".as_slice(),
    ] {
        let (owner, source) = fixture(text);
        let a = named(&owner, source, b"A");
        let b = named(&owner, source, b"B");
        let checker = owner.checker();
        assert_eq!(checker.type_to_string(&a).unwrap(), "number");
        assert!(checker
            .is_type_related_to(&a, &b, crate::Mode::Identity)
            .unwrap());
    }
}

#[test]
fn union_targets_drop_undefined_for_the_correspondence_fastpath() {
    // `undefined` is frequently added by optionality and would otherwise spoil
    // the index correspondence between two unions, so the target drops it when
    // the source trivially has none.
    let (owner, source) = fixture(
        b"type A = { a: string } | { b: number }; type B = undefined | { a: string } | { b: number }; type C = { a: string } | { b: number };",
    );
    let a = named(&owner, source, b"A");
    let b = named(&owner, source, b"B");
    let c = named(&owner, source, b"C");
    let checker = owner.checker();
    assert!(checker
        .is_type_related_to(&a, &b, crate::Mode::Assignable)
        .unwrap());
    assert!(checker
        .is_type_related_to(&a, &c, crate::Mode::Assignable)
        .unwrap());
}

#[test]
fn generic_member_relation_performs_the_native_construction() {
    // The frozen `deferred-generic-members` fixture, which needs no lib. The
    // pinned checker creates 21 types and 8 signatures and runs 31 counted
    // instantiations for the first identity relation, and leaves five
    // assignable entries (two succeeded, three failed) from the variance
    // measurement. Each number depends on a separate rule: deferred references
    // are normalized, marker types instantiate their type parameters, an
    // instantiated symbol returns to its root with a combined mapper, a type
    // parameter relates to its constraint before any cache entry, and an
    // unconstrained one relates through `unknown`.
    let (owner, source) = fixture(
        b"interface Box<T> { value: T; map<U>(f: (x: T) => U): Box<U> } type A = Box<'x'>; type B = Box<string>;",
    );
    let a = named(&owner, source, b"A");
    let b = named(&owner, source, b"B");
    let checker = owner.checker();
    let (types, signatures) = (checker.graph.len(), owner.signatures_created());
    assert_eq!(owner.instantiations(), 0);
    assert!(!checker
        .is_type_related_to(&a, &b, crate::Mode::Identity)
        .unwrap());
    assert_eq!(checker.graph.len() - types, 21);
    assert_eq!(owner.signatures_created() - signatures, 8);
    assert_eq!(owner.instantiations(), 31);
    let mut assignable = checker.relation(crate::Mode::Assignable).result_flags();
    assignable.sort_unstable();
    assert_eq!(assignable, [1, 1, 2, 2, 2]);
    assert_eq!(checker.relation(crate::Mode::Identity).result_flags(), [2]);
    // A repeat is answered from the caches without any construction.
    assert!(!checker
        .is_type_related_to(&a, &b, crate::Mode::Identity)
        .unwrap());
    assert_eq!(checker.graph.len() - types, 21);
    assert_eq!(owner.instantiations(), 31);
}

#[test]
fn merged_interface_declarations_share_their_type_parameter() {
    // Class and interface type parameters are members of the merged symbol:
    // `T` in the second declaration is the first declaration's `T`, and a
    // member declared there is instantiated with the reference's argument.
    let (owner, source) = fixture(
        b"interface P<T> { tag: 1 } interface P<T> { get(): T } type A = P<string>; type B = P<'x'>;",
    );
    let a = named(&owner, source, b"A");
    let b = named(&owner, source, b"B");
    let checker = owner.checker();
    assert!(checker
        .is_type_related_to(&b, &a, crate::Mode::Assignable)
        .unwrap());
    assert!(!checker
        .is_type_related_to(&a, &b, crate::Mode::Assignable)
        .unwrap());
}

#[test]
fn an_unaliased_empty_type_literal_is_the_shared_empty_type() {
    let (owner, source) = fixture(b"type A = { x: {} }; type B = { y: {} };");
    let a = named(&owner, source, b"A");
    let b = named(&owner, source, b"B");
    let graph = &owner.checker().graph;
    let x = a.member(graph, "x").unwrap().unwrap().r#type().unwrap();
    let y = b.member(graph, "y").unwrap().unwrap().r#type().unwrap();
    assert!(Rc::ptr_eq(&x, &y));
}

#[test]
fn an_optional_mapped_type_makes_its_template_optional_once() {
    // getTemplateTypeFromMappedType applies the `?` modifier to the template,
    // so each property instantiates `X | undefined` instead of gaining
    // `undefined` afterwards, and a property whose type can already be
    // undefined gains nothing.
    let (owner, source) = fixture(
        b"type P<T> = { [K in keyof T]?: T[K] }; type A = P<{ x: string; y: number | undefined }>; type B = { x?: string; y?: number | undefined };",
    );
    let a = named(&owner, source, b"A");
    let b = named(&owner, source, b"B");
    let checker = owner.checker();
    assert!(checker
        .is_type_related_to(&a, &b, crate::Mode::Identity)
        .unwrap());
}

#[test]
fn mapped_modifiers_come_through_a_constrained_key_parameter() {
    // getModifiersTypeFromMappedType: for `K in P` with `P extends keyof T`,
    // the modifiers type is T, so the source property's optionality and
    // readonly-ness carry over; the keys still come from the constraint.
    let (owner, source) = fixture(
        b"type Q<T, P extends keyof T> = { [K in P]: T[K] }; type A = Q<{ x?: string; readonly y: number }, 'x' | 'y'>; type B = { x?: string; readonly y: number };",
    );
    let a = named(&owner, source, b"A");
    let b = named(&owner, source, b"B");
    let checker = owner.checker();
    assert!(checker
        .is_type_related_to(&a, &b, crate::Mode::Identity)
        .unwrap());
}

#[test]
fn keyof_any_and_unknown_follow_the_pinned_results() {
    let (owner, source) = fixture(
        b"type A = keyof any; type B = string | number | symbol; type C = keyof unknown; type D = never;",
    );
    let checker = owner.checker();
    for (left, right) in [(b"A".as_slice(), b"B".as_slice()), (b"C", b"D")] {
        let left = named(&owner, source, left);
        let right = named(&owner, source, right);
        assert!(checker
            .is_type_related_to(&left, &right, crate::Mode::Identity)
            .unwrap());
    }
}

#[test]
fn an_intersection_reduces_over_unions_disjoint_domains_and_supertypes() {
    // getIntersectionType: `{}` drops out against a definitely non-nullable
    // type, disjoint domains empty the intersection, and a union distributes.
    let (owner, source) = fixture(
        b"type A = (string | undefined | null) & {}; type B = string; type C = string & number; type D = never; type E = 'a' & string; type F = 'a';",
    );
    let checker = owner.checker();
    for (left, right) in [
        (b"A".as_slice(), b"B".as_slice()),
        (b"C", b"D"),
        (b"E", b"F"),
    ] {
        let left = named(&owner, source, left);
        let right = named(&owner, source, right);
        assert!(
            checker
                .is_type_related_to(&left, &right, crate::Mode::Identity)
                .unwrap(),
            "{} should be identical to {}",
            left.name(),
            right.name()
        );
    }
}

#[test]
fn a_contravariant_position_infers_its_own_candidate() {
    // A parameter position is contravariant, so `infer P` collects its
    // candidate in the contravariant list and the conditional resolves through
    // `getTypeFromInference`'s intersection of those. A rest position would add
    // the implicit `unknown[]` constraint, which the reference refuses.
    let (owner, source) = fixture(
        b"type Arg<T> = T extends (x: infer P) => unknown ? P : never; type A = Arg<(x: string) => void>; type B = string;",
    );
    let a = named(&owner, source, b"A");
    let b = named(&owner, source, b"B");
    let checker = owner.checker();
    assert!(checker
        .is_type_related_to(&a, &b, crate::Mode::Identity)
        .unwrap());
}

#[test]
fn a_fixed_tuple_index_resolves_instead_of_deferring() {
    // indexTypeLessThan accepts a string literal whose name round-trips through
    // ToNumber and every constituent of a union index, and it counts against the
    // total fixed element count, which includes a fixed suffix after the
    // variadic element. A narrower predicate defers these accesses, and the
    // access then displays as an unresolved indexed access. The instantiated
    // alias resolves either way, so the generic body is what the test reads.
    let (owner, source) = fixture(
        b"interface Array<T> { length: number; [n: number]: T }\
          interface ReadonlyArray<T> { readonly length: number; readonly [n: number]: T }\
          type T0<T extends unknown[]> = [string, ...T, number][0]; type A = T0<[boolean]>;\
          type T1<T extends unknown[]> = [string, number, ...T]['0']; type C = T1<[boolean]>;\
          type T2<T extends unknown[]> = [string, number, ...T][0 | 1]; type D = T2<[boolean]>;\
          type B = string; type E = string | number;",
    );
    let checker = owner.checker();
    for (left, right) in [
        (b"T0".as_slice(), b"B".as_slice()),
        (b"T1".as_slice(), b"B".as_slice()),
        (b"T2".as_slice(), b"E".as_slice()),
    ] {
        let resolved = named(&owner, source, left);
        let expected = named(&owner, source, right);
        assert_eq!(
            resolved.flags & tf::INDEXED_ACCESS,
            0,
            "{} must resolve, not defer",
            String::from_utf8_lossy(left)
        );
        assert!(
            checker
                .is_type_related_to(&resolved, &expected, crate::Mode::Identity)
                .unwrap(),
            "{} resolves to {}",
            String::from_utf8_lossy(left),
            expected.name
        );
    }
}

#[test]
fn conditional_inference_reuses_the_first_signature_for_excess_targets() {
    // inferFromSignatures pairs signatures from the bottom up, so a source with
    // one signature infers from it to every excess target signature. Pairing
    // only the last min(source, target) signatures leaves R uninferred, and the
    // conditional then resolves to the inference default rather than to string.
    let (owner, source) = fixture(
        b"type A = (() => string) extends { (): infer R; (): void } ? R : never; type B = string;",
    );
    let a = named(&owner, source, b"A");
    let b = named(&owner, source, b"B");
    assert!(
        owner
            .checker()
            .is_type_related_to(&a, &b, crate::Mode::Identity)
            .unwrap(),
        "R infers string from the only source signature, not unknown"
    );
}

#[test]
fn an_alias_body_instantiates_through_its_alias_arguments() {
    // couldContainTypeVariables also holds when a type's alias type arguments
    // could contain one, so instantiating a generic alias whose body has no type
    // variable of its own is still real work and still keys a separate cell. The
    // union a distributed indexed access builds is named by the alias, so it
    // carries those arguments even though no source declaration records it.
    let (owner, source) = fixture(
        b"interface Array<T> { length: number; [n: number]: T }\
          interface ReadonlyArray<T> { readonly length: number; readonly [n: number]: T }\
          type W<T extends unknown[]> = [string, number, ...T][0 | 1]; type A = W<[boolean]>;",
    );
    let body = named(&owner, source, b"W");
    let instance = named(&owner, source, b"A");
    assert_eq!(body.flags & tf::UNION, tf::UNION);
    assert!(
        !Rc::ptr_eq(&body, &instance),
        "the alias arguments differ, so the instance is a separate cell"
    );
    assert!(
        owner
            .checker()
            .is_type_related_to(&body, &instance, crate::Mode::Identity)
            .unwrap(),
        "the separate cells stay structurally identical"
    );
}

fn declared(
    checker: &BoundChecker,
    source: NodeId,
    name: &[u8],
) -> Result<Rc<TypeCell>, crate::Error> {
    let node = checker
        .input()
        .declaration_by_name(source, name)
        .unwrap()
        .unwrap();
    checker.declared_type(node)
}

#[test]
fn a_refused_base_constraint_stays_refused_on_a_second_request() {
    // The base-constraint walk behind a declared constraint is remembered so it
    // runs once. Remembering it before it finishes would let a second, identical
    // relation skip the walk and succeed where the first was refused.
    let (owner, source) =
        fixture(b"type G<T, K extends T[keyof T]> = { value: K }; type B = { value: string };");
    let g = named(&owner, source, b"G");
    let b = named(&owner, source, b"B");
    let checker = owner.checker();
    let first = checker
        .is_type_related_to(&g, &b, crate::Mode::Assignable)
        .expect_err("the walk reaches an indexed access");
    let second = checker
        .is_type_related_to(&g, &b, crate::Mode::Assignable)
        .expect_err("a refusal is terminal");
    assert_eq!(first, second);
}

#[test]
fn a_refused_infer_constraint_stays_refused_on_a_second_request() {
    // The type parameter is published in the node cache, so the refusal has to
    // come first; otherwise the second request finds the parameter and succeeds.
    let (owner, source) = fixture(
        b"type P<F> = F extends (...args: infer A) => unknown ? A : never;\
          type A = P<(a: string) => void>;",
    );
    let first = declared(&owner, source, b"A").expect_err("an infer rest parameter is refused");
    let second = declared(&owner, source, b"A").expect_err("a refusal is terminal");
    assert_eq!(first, second);
}
