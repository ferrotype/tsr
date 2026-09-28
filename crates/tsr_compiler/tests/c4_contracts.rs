//! Phase 2 C4 direct contracts (docs/PHASE2-C4-plan.md, C4.8), over production
//! entry points. Each case's diagnostics were recorded from the pinned `tsgo`
//! by `fixtures/c4/regenerate.py`. The JSX entities and alias state the
//! resolver reads are compared with the pinned checker's own state, recorded
//! through a Go overlay by `fixtures/c4/state/regenerate.py`: nothing here is
//! a hand-written expectation of checker state.
#[path = "support/c4_native.rs"]
mod native;
use native::REACT;
use serde_json::{json, Value};
use std::ops::ControlFlow;
use tsr_arena::NodeId;
use tsr_ast::{AstView, ChildVisitor};

const MODES: &[&str] = &[
    "preserve",
    "react_native",
    "react",
    "react_jsx",
    "react_jsxdev",
];

fn with_react<'a>(root: (&'a str, &'a str)) -> Vec<(&'a str, &'a str)> {
    let mut files = vec![root];
    files.extend_from_slice(REACT);
    files
}

fn jsx_modes_record(mode: &str) -> &'static str {
    match mode {
        "preserve" => include_str!("fixtures/c4/jsx_modes_preserve.native.json"),
        "react_native" => include_str!("fixtures/c4/jsx_modes_react_native.native.json"),
        "react" => include_str!("fixtures/c4/jsx_modes_react.native.json"),
        "react_jsx" => include_str!("fixtures/c4/jsx_modes_react_jsx.native.json"),
        "react_jsxdev" => include_str!("fixtures/c4/jsx_modes_react_jsxdev.native.json"),
        _ => unreachable!(),
    }
}

fn jsx_pragmas_record(mode: &str) -> &'static str {
    match mode {
        "preserve" => include_str!("fixtures/c4/jsx_pragmas_preserve.native.json"),
        "react_native" => include_str!("fixtures/c4/jsx_pragmas_react_native.native.json"),
        "react" => include_str!("fixtures/c4/jsx_pragmas_react.native.json"),
        "react_jsx" => include_str!("fixtures/c4/jsx_pragmas_react_jsx.native.json"),
        "react_jsxdev" => include_str!("fixtures/c4/jsx_pragmas_react_jsxdev.native.json"),
        _ => unreachable!(),
    }
}

/// Contract 1: one program under each of the five `jsx` modes, without and
/// with `@jsx`/`@jsxFrag` pragmas. Diagnostics equal the native observations;
/// the factory, the `JSX` namespace and the implicit runtime import equal the
/// pinned `getJsxFactoryEntity`, `getJsxFragmentFactoryEntity`,
/// `getJsxNamespaceAt` and `getJsxNamespaceContainerForImplicitImport`, after
/// checking and on a fresh checker that reads the pragmas itself (the
/// resolver may ask before anything is checked). Only `react-jsx` finds the
/// `react` runtime; the package has no development runtime.
#[test]
fn jsx_mode_matrix_matches_native() {
    for &mode in MODES {
        let case = native::assert_case(
            &format!("jsx_modes_{mode}"),
            jsx_modes_record(mode),
            &with_react(("jsx_modes.tsx", include_str!("fixtures/c4/jsx_modes.tsx"))),
        );
        native::assert_state(&case);
        native::assert_unchecked_state(&case);
        let case = native::assert_case(
            &format!("jsx_pragmas_{mode}"),
            jsx_pragmas_record(mode),
            &with_react((
                "jsx_pragmas.tsx",
                include_str!("fixtures/c4/jsx_pragmas.tsx"),
            )),
        );
        native::assert_state(&case);
        native::assert_unchecked_state(&case);
    }
}

/// A `@jsx` pragma of 30,000 components completes with the pin's diagnostics
/// on a bounded stack: the isolated entity name is flattened in a loop, and
/// the factory entity keeps every component (the recorded state binds its
/// length and digest).
#[test]
fn deep_jsx_pragma_completes_on_a_bounded_stack() {
    std::thread::Builder::new()
        .stack_size(512 * 1024)
        .spawn(|| {
            let case = native::assert_named("jsx_deep_pragma");
            native::assert_state(&case);
        })
        .unwrap()
        .join()
        .unwrap();
}

/// The C4.2 exit: a direct case per pragma and option combination. The
/// `jsxFactory` option alone (TS17016 at a fragment) and with
/// `jsxFragmentFactory`; `@jsx`/`@jsxFrag` pragmas over both options;
/// `@jsx` without `@jsxFrag` (TS17017); no `jsx` option (TS17004); `react`
/// without `React` in scope (TS2874); `@jsxRuntime classic` under both
/// automatic modes and `@jsxRuntime automatic` under two classic ones.
#[test]
fn jsx_pragma_and_option_combinations_match_native() {
    for case in [
        "jsx_factory_option",
        "jsx_factory_options",
        "jsx_pragmas_over_options",
        "jsx_pragma_without_frag",
        "jsx_no_flag",
        "jsx_react_missing",
        "jsx_runtime_classic_react_jsx",
        "jsx_runtime_classic_react_jsxdev",
        "jsx_runtime_automatic_react",
        "jsx_runtime_automatic_preserve",
    ] {
        let case = native::assert_named(case);
        native::assert_state(&case);
        native::assert_unchecked_state(&case);
    }
}

/// The C4.5 exit: under each `jsx` mode, an `@jsxImportSource` package that
/// has both runtime modules and one that has neither (TS2875 where the mode
/// needs one), and the `jsxImportSource` option under both automatic modes;
/// the implicit import and the namespace are the pin's.
#[test]
fn jsx_runtime_per_mode_matches_native() {
    for &mode in MODES {
        for case in [
            format!("jsx_import_source_{mode}"),
            format!("jsx_import_source_absent_{mode}"),
        ] {
            let case = native::assert_named(&case);
            native::assert_state(&case);
            native::assert_unchecked_state(&case);
        }
    }
    for case in [
        "jsx_import_source_option_react_jsx",
        "jsx_import_source_option_react_jsxdev",
    ] {
        let case = native::assert_named(case);
        native::assert_state(&case);
        native::assert_unchecked_state(&case);
    }
}

/// The two shared corrections C4 made outside JSX, witnessed without JSX. A
/// distributive conditional instantiated without an alias maps its
/// distribution union (`mapTypeWithAlias`), so an unchanged `keyof Props`
/// keeps its origin (`Pick<Props, keyof Props>`), while an aliased one is
/// rebuilt under its alias; and an excess property of a fresh object literal
/// reports the relation error `isRelatedTo` reports, in declarations, nested
/// literals, union, intersection and array targets, and an argument.
#[test]
fn shared_fixes_hold_outside_jsx() {
    native::assert_named("conditional_distribution");
    native::assert_named("relation_excess");
}

/// Every case of the manifest has a native record and matches it, and every
/// recorded state matches, so no fixture is recorded without a contract.
#[test]
fn every_recorded_case_matches_native() {
    let manifest: Value = serde_json::from_str(native::MANIFEST).unwrap();
    let cases = manifest.as_object().unwrap();
    assert_eq!(cases.len(), native::RECORDS.len(), "one record per case");
    let state: Value = serde_json::from_str(include_str!("fixtures/c4/state/native.json")).unwrap();
    let recorded: Vec<&str> = state["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["id"].as_str().unwrap())
        .collect();
    for name in cases.keys() {
        if name == "jsx_deep_pragma" {
            continue; // on its bounded stack above
        }
        let case = native::assert_named(name);
        if recorded.contains(&name.as_str()) {
            native::assert_state(&case);
            native::assert_unchecked_state(&case);
        }
    }
}

/// Contract 2: intrinsic, function, class, fragment, member and namespaced
/// tags, with `LibraryManagedAttributes` defaulting a class prop, children
/// typing (`getJsxElementChildrenPropertyName`, `checkJsxChildren`) and the
/// JSX excess-property message (`checkJsxOpeningLikeElementOrOpeningFragment`,
/// `createJsxAttributesTypeFromAttributesProperty`).
#[test]
fn element_kinds_match_native() {
    let case = native::assert_case(
        "jsx_elements",
        include_str!("fixtures/c4/jsx_elements.native.json"),
        &[(
            "jsx_elements.tsx",
            include_str!("fixtures/c4/jsx_elements.tsx"),
        )],
    );
    native::assert_state(&case);
}

/// Contract 3: generic components over a bundled-lib-only program: inferred
/// and explicit type arguments, overloads and contextually typed attribute
/// and child functions (`inferJsxTypeArguments`,
/// `checkApplicableSignatureForJsxCallLikeElement`,
/// `getContextualTypeForChildJsxExpression`).
#[test]
fn generic_components_match_native() {
    native::assert_case(
        "jsx_generics",
        include_str!("fixtures/c4/jsx_generics.native.json"),
        &[(
            "jsx_generics.tsx",
            include_str!("fixtures/c4/jsx_generics.tsx"),
        )],
    );
}

/// Contract 4: a `.jsx` file under `checkJs` reports the pin's diagnostics,
/// with a JSDoc-typed component and intrinsic tags without a `JSX` namespace.
#[test]
fn jsx_in_javascript_matches_native() {
    native::assert_case(
        "jsx_in_js",
        include_str!("fixtures/c4/jsx_in_js.native.json"),
        &[("jsx_in_js.jsx", include_str!("fixtures/c4/jsx_in_js.jsx"))],
    );
}

/// Contract 5: class, method, getter, setter, auto-accessor, field,
/// parameter and static-block decorators under ES and legacy decorators, with
/// the pin's signature selection (`getDecoratorCallSignature`,
/// `getDecoratorArgumentCount`) and diagnostics (`checkDecorator`,
/// `checkGrammarDecorator`).
#[test]
fn decorator_positions_match_native() {
    let positions = (
        "decorator_positions.ts",
        include_str!("fixtures/c4/decorator_positions.ts"),
    );
    native::assert_case(
        "decorator_positions_es",
        include_str!("fixtures/c4/decorator_positions_es.native.json"),
        &[positions],
    );
    native::assert_case(
        "decorator_positions_legacy",
        include_str!("fixtures/c4/decorator_positions_legacy.native.json"),
        &[positions],
    );
    let block = (
        "decorator_static_block.ts",
        include_str!("fixtures/c4/decorator_static_block.ts"),
    );
    native::assert_case(
        "decorator_static_block_es",
        include_str!("fixtures/c4/decorator_static_block_es.native.json"),
        &[block],
    );
    native::assert_case(
        "decorator_static_block_legacy",
        include_str!("fixtures/c4/decorator_static_block_legacy.native.json"),
        &[block],
    );
}

/// Contract 6: the ES decorator context types have the pin's identities and
/// members (`newClassMethodDecoratorContextType` and its siblings,
/// `getClassMemberDecoratorContextOverrideType`), shown by the argument each
/// decorator receives, and `Symbol.metadata` resolves through the lib.
#[test]
fn decorator_context_types_match_native() {
    let case = native::assert_case(
        "decorator_contexts",
        include_str!("fixtures/c4/decorator_contexts.native.json"),
        &[(
            "decorator_contexts.ts",
            include_str!("fixtures/c4/decorator_contexts.ts"),
        )],
    );
    let messages: Vec<String> = case.native["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["message"].as_str().unwrap().to_string())
        .collect();
    for context in [
        "ClassMethodDecoratorContext<typeof C, () => void> & { name: \"m\"; private: false; static: true; }",
        "ClassFieldDecoratorContext<C, number> & { name: \"#p\"; private: true; static: false; }",
        "ClassDecoratorContext<typeof D>",
        "DecoratorMetadataObject | null",
    ] {
        assert!(messages.iter().any(|m| m.contains(context)), "{context} in {messages:?}");
    }
}

/// Contract 7: under `emitDecoratorMetadata` the class a decorated
/// constructor parameter names is marked referenced and the interface is not
/// (`markDecoratorAliasReferenced`, `markDecoratorMedataDataTypeNodeAsReferenced`);
/// without the option nothing is marked; under `isolatedModules` the interface
/// imported as a value reports TS1272 (`markEntityNameOrEntityExpressionAsReference`).
/// Each command is its own native case, and the referenced state is the
/// pinned checker's (`aliasSymbolLinks.referenced`).
#[test]
fn metadata_marking_follows_the_pin() {
    let files = [
        (
            "metadata_marking.ts",
            include_str!("fixtures/c4/metadata_marking.ts"),
        ),
        (
            "metadata_types.ts",
            include_str!("fixtures/c4/metadata_types.ts"),
        ),
    ];
    for (case, record) in [
        (
            "metadata_marking",
            include_str!("fixtures/c4/metadata_marking.native.json"),
        ),
        (
            "metadata_isolated",
            include_str!("fixtures/c4/metadata_isolated.native.json"),
        ),
        (
            "metadata_plain",
            include_str!("fixtures/c4/metadata_plain.native.json"),
        ),
    ] {
        let case = native::assert_case(case, record, &files);
        native::assert_state(&case);
    }
}

/// Contract 8: a JSX error and a decorator error leave the checker reusable:
/// a second query repeats the diagnostics, a second checker over the program
/// resolves the runtime import independently, and retiring the first
/// generation leaves the second answering.
#[test]
fn jsx_and_decorator_errors_leave_checkers_reusable() {
    let case = native::assert_case(
        "lifecycle",
        include_str!("fixtures/c4/lifecycle.native.json"),
        &with_react(("lifecycle.tsx", include_str!("fixtures/c4/lifecycle.tsx"))),
    );
    let expected = case.native["diagnostics"].clone();
    assert_eq!(json!(native::observed(&case)), expected);
    let recorded = native::recorded_state(&case);
    let (_generation, second) = native::checker(&case.program);
    assert_eq!(
        native::jsx_state(&case, &second),
        recorded["unchecked"]["jsx"]
    );
    assert_eq!(json!(native::observed_with(&case, &second)), expected);
    native::assert_state(&case);
    case.generation.retire();
    assert!(
        case.owner.operation().is_err(),
        "a retired checker must refuse a new operation"
    );
    assert_eq!(json!(native::observed_with(&case, &second)), expected);
}

fn node_kinds(case: &native::Case) -> Vec<String> {
    struct Walk<'a> {
        view: AstView<'a>,
        kinds: Vec<String>,
    }
    impl ChildVisitor for Walk<'_> {
        fn visit_node(&mut self, node: NodeId) -> ControlFlow<()> {
            let read = self.view.node(node).unwrap();
            self.kinds.push(read.kind_string());
            read.for_each_child(self)
        }
        fn visit_list(&mut self, list: tsr_ast::NodeListId) -> ControlFlow<()> {
            self.visit_node_slice(self.view.list(list).unwrap().nodes())
        }
        fn visit_node_slice(&mut self, slice: tsr_ast::NodeSlice) -> ControlFlow<()> {
            for node in self.view.node_slice(slice).unwrap().iter().flatten() {
                self.visit_node(node)?;
            }
            ControlFlow::Continue(())
        }
    }
    let file = case
        .program
        .file(format!("/{}", case.root).as_bytes())
        .unwrap();
    let mut walk = Walk {
        view: file.bound().view().ast(),
        kinds: Vec::new(),
    };
    let _ = walk.visit_node(file.source());
    walk.kinds
}

/// Contract 9: checking transforms nothing. The source tree is unchanged, the
/// checker's own factory holds no JSX, decorator or factory-call syntax, and
/// the recorded JSX entities are the only output.
#[test]
fn checking_produces_no_transformed_syntax() {
    let case = native::load(
        "jsx_modes_react",
        jsx_modes_record("react"),
        &with_react(("jsx_modes.tsx", include_str!("fixtures/c4/jsx_modes.tsx"))),
    );
    let before = node_kinds(&case);
    assert_eq!(json!(native::observed(&case)), case.native["diagnostics"]);
    assert_eq!(node_kinds(&case), before);
    let decorated = native::assert_case(
        "decorator_positions_es",
        include_str!("fixtures/c4/decorator_positions_es.native.json"),
        &[(
            "decorator_positions.ts",
            include_str!("fixtures/c4/decorator_positions.ts"),
        )],
    );
    for (owner, recorded) in [(&case.owner, true), (&decorated.owner, false)] {
        let kinds = owner.operation().unwrap().synthetic_syntax_kinds().unwrap();
        let kinds: Vec<&str> = kinds
            .iter()
            .map(|kind| kind.strip_prefix("Kind").unwrap_or(kind))
            .collect();
        for kind in &kinds {
            assert!(
                !kind.starts_with("Jsx")
                    && !matches!(
                        *kind,
                        "Decorator"
                            | "CallExpression"
                            | "ObjectLiteralExpression"
                            | "PropertyAccessExpression"
                    ),
                "the checker built {kind} syntax: {kinds:?}"
            );
        }
        // The one JSX entity the checker builds is the recorded factory name.
        assert_eq!(kinds.contains(&"QualifiedName"), recorded, "{kinds:?}");
    }
    native::assert_state(&case);
}
