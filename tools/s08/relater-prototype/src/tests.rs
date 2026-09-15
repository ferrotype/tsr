//! The frozen recursive fixtures (`tools/s08/contracts/relations.json`), their
//! Go-observed outcomes (`data/s08/supplemental-observations.json.xz`, cases
//! `recursive-objects` and `recursive-mismatch`), recursion on a reserved stack,
//! and the ownership and failure behaviors the prototype has to make explicit.

use super::*;

/// `interface Left { value: V; next: Left }`, built the way the API intends: the
/// resolver upgrades the self link at resolution time, when the cell exists.
fn self_linked(checker: &Checker, name: &'static str, value: &Rc<TypeCell>) -> Rc<TypeCell> {
    let slot: Rc<RefCell<Weak<TypeCell>>> = Rc::new(RefCell::new(Weak::new()));
    let value_link = checker.graph.link(value);
    let resolver_slot = slot.clone();
    let cell = checker.graph.allocate(
        flags::OBJECT,
        name,
        Some(Box::new(move |graph, _| {
            let next = resolver_slot.borrow().clone();
            if next.upgrade().is_none() {
                return Err(Error::UndeclaredMember(Rc::from("next")));
            }
            let members = vec![
                Member {
                    name: Rc::from("value"),
                    optional: false,
                    readonly: false,
                    class_member: true,
                    r#type: value_link.clone(),
                },
                Member {
                    name: Rc::from("next"),
                    optional: false,
                    readonly: false,
                    class_member: true,
                    r#type: next,
                },
            ];
            for member in &members {
                graph
                    .lazy_records
                    .borrow_mut()
                    .push(Rc::new(member.clone()));
            }
            Ok(Structure {
                members,
                ..Default::default()
            })
        })),
    );
    *slot.borrow_mut() = Rc::downgrade(&cell);
    cell
}

fn sequence(
    checker: &Checker,
    a: &Rc<TypeCell>,
    b: &Rc<TypeCell>,
    mode: Mode,
) -> Vec<(Ternary, bool, usize, Vec<u32>, usize)> {
    let mut rows = Vec::new();
    for (source, target) in [(a, b), (a, b), (b, a), (a, a)] {
        let before = checker.diagnostics().len();
        let (ternary, related) = checker
            .check_type_related_to(source, target, mode, true)
            .unwrap();
        let relation = checker.relation(mode);
        rows.push((
            ternary,
            related,
            relation.entries(),
            relation.result_flags(),
            checker.diagnostics().len() - before,
        ));
    }
    rows
}

#[test]
fn recursive_objects_match_the_go_observations_in_every_mode() {
    for mode in MODES {
        // Fresh checker per mode, as the Go observer does.
        let checker = Checker::new();
        let string = checker.graph.primitive(flags::STRING, "string");
        let a = self_linked(&checker, "Left", &string);
        let b = self_linked(&checker, "Right", &string);
        let created = checker.graph.len();
        let rows = sequence(&checker, &a, &b, mode);
        let succeeded = relation_result::SUCCEEDED;
        let expected = if mode == Mode::Identity {
            vec![
                (MAYBE, true, 1, vec![succeeded], 0),
                (TRUE, true, 1, vec![succeeded], 0),
                (TRUE, true, 1, vec![succeeded], 0),
                (TRUE, true, 1, vec![succeeded], 0),
            ]
        } else {
            vec![
                (MAYBE, true, 1, vec![succeeded], 0),
                (TRUE, true, 1, vec![succeeded], 0),
                (MAYBE, true, 2, vec![succeeded, succeeded], 0),
                (TRUE, true, 2, vec![succeeded, succeeded], 0),
            ]
        };
        assert_eq!(rows, expected, "{mode:?}");
        assert_eq!(
            checker.graph.len(),
            created,
            "relating creates no types, as observed"
        );
        assert_eq!(
            a.resolutions(),
            1,
            "members resolve once, during the first relation"
        );
        assert_eq!(b.resolutions(), 1);
        assert_eq!(
            checker.graph.lazy_records(),
            4,
            "resolution allocated the four member records"
        );
    }
}

#[test]
fn recursive_mismatch_matches_the_go_observations_in_every_mode() {
    for mode in MODES {
        let checker = Checker::new();
        let string = checker.graph.primitive(flags::STRING, "string");
        let number = checker.graph.primitive(flags::NUMBER, "number");
        let a = self_linked(&checker, "Left", &string);
        let b = self_linked(&checker, "Right", &number);
        let rows = sequence(&checker, &a, &b, mode);
        let failed = relation_result::FAILED;
        let expected = if mode == Mode::Identity {
            vec![
                (FALSE, false, 1, vec![failed], 0),
                (FALSE, false, 1, vec![failed], 0),
                (FALSE, false, 1, vec![failed], 0),
                (TRUE, true, 1, vec![failed], 0),
            ]
        } else {
            vec![
                (FALSE, false, 1, vec![failed], 1),
                (FALSE, false, 1, vec![failed], 1),
                (FALSE, false, 2, vec![failed, failed], 1),
                (TRUE, true, 2, vec![failed, failed], 0),
            ]
        };
        assert_eq!(rows, expected, "{mode:?}");
        if mode != Mode::Identity {
            let diagnostics = checker.diagnostics();
            assert!(diagnostics[0].contains("Types of property 'value' are incompatible."));
        }
    }
}

/// Two structurally equal chains of distinct objects, `depth` levels deep.
fn chain(checker: &Checker, depth: usize, leaf: &Rc<TypeCell>) -> Rc<TypeCell> {
    let mut current = leaf.clone();
    for _ in 0..depth {
        current = checker
            .graph
            .object("Node", vec![("next", false, checker.graph.link(&current))]);
    }
    current
}

#[test]
fn deep_chains_recurse_on_a_reserved_stack_and_stop_at_the_depth_limit() {
    std::thread::Builder::new()
        .stack_size(256 << 20)
        .spawn(|| {
            let checker = Checker::new();
            let string = checker.graph.primitive(flags::STRING, "string");
            let a = chain(&checker, 1_000, &string);
            let b = chain(&checker, 1_000, &string);
            let (ternary, related) = checker
                .check_type_related_to(&a, &b, Mode::Assignable, true)
                .unwrap();
            // At 100 nested comparisons upstream assumes relatedness.
            assert_eq!((ternary, related), (MAYBE, true));
            assert_eq!(checker.relation(Mode::Assignable).entries(), 100);
            let shallow_a = chain(&checker, 50, &string);
            let shallow_b = chain(&checker, 50, &string);
            let (ternary, related) = checker
                .check_type_related_to(&shallow_a, &shallow_b, Mode::Assignable, true)
                .unwrap();
            assert_eq!((ternary, related), (TRUE, true));
        })
        .unwrap()
        .join()
        .unwrap();
}

#[test]
fn releasing_the_graph_frees_every_cell_and_fails_dangling_edges_explicitly() {
    let checker = Checker::new();
    let string = checker.graph.primitive(flags::STRING, "string");
    let left = self_linked(&checker, "Left", &string);
    let right = self_linked(&checker, "Right", &string);
    checker
        .check_type_related_to(&left, &right, Mode::Assignable, false)
        .unwrap();
    let weak_string = Rc::downgrade(&string);
    let escaped = left.clone();
    drop(string);
    drop(left);
    drop(right);
    assert!(
        weak_string.upgrade().is_some(),
        "the graph still owns the cells"
    );
    drop(checker);
    assert!(
        weak_string.upgrade().is_none(),
        "no strong cycle survives the graph"
    );
    // The escaped handle keeps its own cell but not its dependencies.
    assert_eq!(escaped.resolutions(), 1);
    let members = &escaped
        .structure
        .get()
        .expect("resolved before release")
        .members;
    assert_eq!(members[0].r#type().err(), Some(Error::Released));
}

#[test]
fn a_panicking_resolver_leaves_the_graph_consistent() {
    let checker = Checker::new();
    let string = checker.graph.primitive(flags::STRING, "string");
    let poisoned = checker.graph.poisoned_object("Poisoned");
    let healthy = checker.graph.object(
        "Healthy",
        vec![("value", false, checker.graph.link(&string))],
    );
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        checker.check_type_related_to(&healthy, &poisoned, Mode::Assignable, true)
    }));
    assert!(result.is_err(), "the injected failure unwinds");
    assert!(
        poisoned.structure.get().is_none(),
        "no partial members were published"
    );
    assert_eq!(poisoned.resolutions(), 0);
    // The graph is intact for the rest of the checker's life; the production
    // design retires the generation instead of continuing (ADR 0012).
    let (ternary, related) = checker
        .check_type_related_to(&healthy, &healthy, Mode::Assignable, true)
        .unwrap();
    assert_eq!((ternary, related), (TRUE, true));
    assert_eq!(
        checker.relation(Mode::Assignable).entries(),
        0,
        "the failed relation cached nothing"
    );
    // A second attempt on the poisoned type fails explicitly, not silently.
    let again = poisoned.members(&checker.graph);
    assert_eq!(again.err(), Some(Error::ResolutionFailed));
    assert!(poisoned.structure.get().is_none());
    assert_eq!(
        checker.check_type_related_to(&healthy, &poisoned, Mode::Assignable, true),
        Err(Error::ResolutionFailed)
    );
}

#[test]
fn an_undeclared_member_type_is_an_error_not_a_default() {
    let checker = Checker::new();
    let dangling = checker
        .graph
        .object("Dangling", vec![("x", false, Weak::new())]);
    let other = checker.graph.object("Other", vec![]);
    assert_eq!(
        checker.check_type_related_to(&dangling, &other, Mode::Identity, false),
        Err(Error::UndeclaredMember(Rc::from("x")))
    );
    assert_eq!(
        checker.check_type_related_to(&dangling, &other, Mode::Identity, false),
        Err(Error::ResolutionFailed)
    );
    assert!(dangling.structure.get().is_none());
    assert_eq!(checker.relation(Mode::Identity).entries(), 0);
}

/// `type A = 'a' | 'b' | 0 | 1 | true; type B = string | number | boolean;`
/// built from a description, as the P7 measurement child builds every fixture:
/// unions link their constituents after construction, literals relate through
/// the primitive-union shortcut, and the observed Go cache transitions of the
/// `literal-union` fixture are reproduced (`data/s08/supplemental-observations.json.xz`).
#[test]
fn described_literal_union_matches_the_go_observations() {
    fn literal(name: &str, flags: u32, value: LiteralValue) -> TypeDesc {
        TypeDesc {
            name: name.into(),
            flags,
            object_flags: 0,
            symbol: None,
            alias: None,
            kind: KindDesc::Literal {
                value,
                fresh: false,
                alternate: None,
            },
        }
    }
    fn intrinsic(name: &str, flags: u32) -> TypeDesc {
        TypeDesc {
            name: name.into(),
            flags,
            object_flags: 0,
            symbol: None,
            alias: None,
            kind: KindDesc::Intrinsic,
        }
    }
    // Production representation: `boolean` is the union `false | true`, and
    // `string | number | boolean` flattens to four constituents.
    let description = Description {
        types: vec![
            // 0..3: string, number, bigint, false, true
            intrinsic("string", flags::STRING),
            intrinsic("number", flags::NUMBER),
            intrinsic("bigint", flags::BIG_INT),
            literal(
                "false",
                flags::BOOLEAN_LITERAL,
                LiteralValue::Boolean(false),
            ),
            literal("true", flags::BOOLEAN_LITERAL, LiteralValue::Boolean(true)),
            // 5: boolean
            TypeDesc {
                name: "boolean".into(),
                flags: flags::BOOLEAN | flags::UNION,
                object_flags: object_flags::PRIMITIVE_UNION,
                symbol: None,
                alias: None,
                kind: KindDesc::Union(vec![3, 4]),
            },
            // 6..9: 'a', 'b', 0, 1
            literal(
                "\"a\"",
                flags::STRING_LITERAL,
                LiteralValue::String(b"a".to_vec()),
            ),
            literal(
                "\"b\"",
                flags::STRING_LITERAL,
                LiteralValue::String(b"b".to_vec()),
            ),
            literal("0", flags::NUMBER_LITERAL, LiteralValue::Number(0)),
            literal(
                "1",
                flags::NUMBER_LITERAL,
                LiteralValue::Number(1.0f64.to_bits()),
            ),
            // 10: A, 11: B
            TypeDesc {
                name: "A".into(),
                flags: flags::UNION,
                object_flags: object_flags::PRIMITIVE_UNION,
                symbol: None,
                alias: Some(1),
                kind: KindDesc::Union(vec![6, 7, 8, 9, 4]),
            },
            TypeDesc {
                name: "B".into(),
                flags: flags::UNION,
                object_flags: object_flags::PRIMITIVE_UNION,
                symbol: None,
                alias: Some(2),
                kind: KindDesc::Union(vec![0, 1, 3, 4]),
            },
        ],
    };
    for mode in MODES {
        let checker = Checker::new();
        let constructed = checker.graph.construct(&description).unwrap();
        checker.register_intrinsics(&constructed.cells);
        let (a, b) = (&constructed.cells[10], &constructed.cells[11]);
        let rows = sequence(&checker, a, b, mode);
        let (succeeded, failed) = (relation_result::SUCCEEDED, relation_result::FAILED);
        let expected = match mode {
            Mode::Identity => vec![
                (FALSE, false, 1, vec![failed], 0),
                (FALSE, false, 1, vec![failed], 0),
                (FALSE, false, 1, vec![failed], 0),
                (TRUE, true, 1, vec![failed], 0),
            ],
            Mode::Comparable => vec![
                (TRUE, true, 2, vec![succeeded; 2], 0),
                (TRUE, true, 2, vec![succeeded; 2], 0),
                (TRUE, true, 4, vec![succeeded; 4], 0),
                (TRUE, true, 4, vec![succeeded; 4], 0),
            ],
            Mode::Assignable | Mode::Subtype | Mode::StrictSubtype => vec![
                (TRUE, true, 6, vec![succeeded; 6], 0),
                (TRUE, true, 6, vec![succeeded; 6], 0),
                (
                    FALSE,
                    false,
                    8,
                    vec![
                        succeeded, succeeded, succeeded, succeeded, succeeded, succeeded, failed,
                        failed,
                    ],
                    1,
                ),
                (
                    TRUE,
                    true,
                    8,
                    vec![
                        succeeded, succeeded, succeeded, succeeded, succeeded, succeeded, failed,
                        failed,
                    ],
                    0,
                ),
            ],
        };
        assert_eq!(rows, expected, "{mode:?}");
    }
}
