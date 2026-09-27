//! Phase 2 C4 direct contracts (docs/PHASE2-C4-plan.md, C4.8), over production
//! entry points. Each case's diagnostics were recorded from the pinned `tsgo`
//! by `fixtures/c4/regenerate.py`; the JSX entities and alias state the
//! resolver reads are observed through the checker's links until C5.6 gives
//! the resolver its entry points (the plan's decision 3).
#[path = "support/c4_native.rs"]
mod native;
use native::REACT;
use serde_json::{json, Value};
use std::ops::ControlFlow;
use tsr_arena::NodeId;
use tsr_ast::{AstView, ChildVisitor, SyntaxKind as K};

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

fn link_state(case: &native::Case) -> Value {
    let tag = native::first_jsx_tag(case);
    let mut op = case.owner.operation().unwrap();
    op.jsx_link_state(tag).unwrap()
}

/// Contract 1: one program under each of the five `jsx` modes, without and
/// with `@jsx`/`@jsxFrag` pragmas. Diagnostics equal the native observations;
/// the factory, the `JSX` namespace and the implicit runtime import are those
/// of `getJsxFactoryEntity`, `getJsxNamespaceAt` and
/// `getJsxNamespaceContainerForImplicitImport`: only `react-jsx` finds its
/// runtime (the package has no development runtime, so `react-jsxdev` reports
/// TS2875 and falls back to the classic namespace).
#[test]
fn jsx_mode_matrix_matches_native() {
    const CLASSIC: &str = "/node_modules/react/index.d.ts";
    const RUNTIME: &str = "/node_modules/react/jsx-runtime.d.ts";
    for &mode in MODES {
        let case = native::assert_case(
            &format!("jsx_modes_{mode}"),
            jsx_modes_record(mode),
            &with_react(("jsx_modes.tsx", include_str!("fixtures/c4/jsx_modes.tsx"))),
        );
        let runtime = (mode == "react_jsx").then_some(RUNTIME);
        assert_eq!(
            link_state(&case),
            json!({"factory": "React.createElement", "fragment_factory": null,
                   "namespace": runtime.unwrap_or(CLASSIC), "implicit_import": runtime}),
            "jsx_modes_{mode}"
        );
        let case = native::assert_case(
            &format!("jsx_pragmas_{mode}"),
            jsx_pragmas_record(mode),
            &with_react((
                "jsx_pragmas.tsx",
                include_str!("fixtures/c4/jsx_pragmas.tsx"),
            )),
        );
        assert_eq!(
            link_state(&case),
            json!({"factory": "h", "fragment_factory": "Frag",
                   "namespace": runtime.unwrap_or("/jsx_pragmas.tsx"), "implicit_import": runtime}),
            "jsx_pragmas_{mode}"
        );
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
    assert_eq!(link_state(&case)["namespace"], json!("/jsx_elements.tsx"));
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

fn import_specifier(case: &native::Case, name: &str) -> NodeId {
    struct Walk<'a> {
        view: AstView<'a>,
        name: &'a str,
        found: Option<NodeId>,
    }
    impl ChildVisitor for Walk<'_> {
        fn visit_node(&mut self, node: NodeId) -> ControlFlow<()> {
            let read = self.view.node(node).unwrap();
            if read.kind() == K::ImportSpecifier
                && self
                    .view
                    .node_text(read.name().unwrap())
                    .unwrap()
                    .as_bytes()
                    == self.name.as_bytes()
            {
                self.found = Some(node);
                return ControlFlow::Break(());
            }
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
        name,
        found: None,
    };
    let _ = walk.visit_node(file.source());
    walk.found.expect("import specifier")
}

/// Contract 7: under `emitDecoratorMetadata` the class a decorated
/// constructor parameter names is marked referenced and the interface is not
/// (`markDecoratorAliasReferenced`, `markDecoratorMedataDataTypeNodeAsReferenced`);
/// without the option nothing is marked; under `isolatedModules` the interface
/// imported as a value reports TS1272 (`markEntityNameOrEntityExpressionAsReference`).
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
    let case = native::assert_case(
        "metadata_marking",
        include_str!("fixtures/c4/metadata_marking.native.json"),
        &files,
    );
    let state = |case: &native::Case, name: &str| {
        let specifier = import_specifier(case, name);
        let mut op = case.owner.operation().unwrap();
        op.alias_link_state(specifier).unwrap()["referenced"].clone()
    };
    assert_eq!(state(&case, "Service"), json!(true));
    assert_eq!(state(&case, "Config"), json!(false));
    native::assert_case(
        "metadata_isolated",
        include_str!("fixtures/c4/metadata_isolated.native.json"),
        &files,
    );
    // The same program checked without emitDecoratorMetadata marks nothing:
    // the positions case has decorators and no metadata option.
    let mut record: Value =
        serde_json::from_str(include_str!("fixtures/c4/metadata_marking.native.json")).unwrap();
    let command = record["native_command"].as_array_mut().unwrap();
    command.retain(|flag| flag != "--emitDecoratorMetadata");
    record["diagnostics"] = json!([]);
    let plain = native::load("metadata_plain", &record.to_string(), &files);
    assert_eq!(json!(native::observed(&plain)), json!([]));
    assert_eq!(state(&plain, "Service"), json!(false));
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
    let (_generation, second) = native::checker(&case.program);
    let tag = native::first_jsx_tag(&case);
    let runtime = json!("/node_modules/react/jsx-runtime.d.ts");
    assert_eq!(
        second.operation().unwrap().jsx_link_state(tag).unwrap()["implicit_import"],
        runtime
    );
    assert_eq!(json!(native::observed_with(&case, &second)), expected);
    assert_eq!(link_state(&case)["implicit_import"], runtime);
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
    assert_eq!(link_state(&case)["factory"], json!("React.createElement"));
}
