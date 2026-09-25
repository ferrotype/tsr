//! Operator choice per return category and scope.

use std::collections::BTreeSet;

use super::{allocating, for_arm, for_function, for_stmt, Choice, Scope, Shape, Site};
use crate::index::{index_file, FnInfo};
use crate::types::TypeIndex;

fn function(source: &str, name: &str) -> FnInfo {
    index_file(source)
        .unwrap()
        .fns
        .into_iter()
        .find(|function| function.name == name)
        .unwrap()
}

fn names(choice: &Choice) -> Vec<&str> {
    choice
        .operators
        .iter()
        .map(|operator| operator.name.as_str())
        .collect()
}

fn choose_with(source: &str, name: &str, scope: Scope, types: &TypeIndex) -> Choice {
    let function = function(source, name);
    for_function(Site {
        scope,
        function: &function,
        types,
        krate: "tsr_x",
    })
}

fn choose(source: &str, name: &str, scope: Scope) -> Choice {
    choose_with(source, name, scope, &TypeIndex::default())
}

const PARSER: Scope = Scope {
    parser: true,
    binder: false,
    chain: false,
    facts: false,
};

fn wrap_value(choice: &Choice, at: usize) -> (&str, Option<&str>) {
    match &choice.operators[at].shape {
        Shape::Wrap { value, capture } => (value.as_str(), capture.as_deref()),
        other => panic!("not a wrapper: {other:?}"),
    }
}

#[test]
fn each_return_category_gets_its_operators() {
    let cases: &[(&str, &str, &[&str])] = &[
        ("fn f() {}", "unit", &["skip_body"]),
        ("fn f() -> () {}", "unit", &["skip_body"]),
        (
            "fn f() -> bool { x() }",
            "bool",
            &["return:true", "return:false"],
        ),
        (
            "fn f() -> u32 { x() }",
            "integer",
            &["return:0", "return:1"],
        ),
        ("fn f() -> Option<Foo> { x() }", "option", &["return:None"]),
        (
            "fn f() -> Result<bool, E> { x() }",
            "result_bool",
            &["return:Ok(true)", "return:Ok(false)"],
        ),
        (
            "fn f() -> crate::Result<Option<NodeId>> { x() }",
            "result_option",
            &["return:Ok(None)"],
        ),
        (
            "fn f() -> Result<(), E> { x() }",
            "result_unit",
            &["return:Ok(())"],
        ),
        (
            "fn f() -> Result<usize, E> { x() }",
            "result_integer",
            &["return:Ok(0)", "return:Ok(1)"],
        ),
        (
            "fn f() -> Result<NodeSlice, E> { x() }",
            "result_empty",
            &["return:Ok(NodeSlice::empty())"],
        ),
        (
            "fn f() -> JsString { x() }",
            "string",
            &["return:Default::default()"],
        ),
        (
            "fn f() -> Vec<NodeId> { x() }",
            "vec",
            &["return:Vec::new()"],
        ),
        ("fn f<'a>() -> &'a [u8] { x() }", "slice", &["return:&[]"]),
        (
            "fn f() -> Cow<'_, [u8]> { x() }",
            "cow",
            &["return:::std::borrow::Cow::Borrowed(&[])"],
        ),
        (
            "fn f() -> Tristate { x() }",
            "tristate",
            &["return:Tristate::TRUE", "return:Tristate::FALSE"],
        ),
        (
            "fn f() -> ControlFlow<()> { x() }",
            "control_flow",
            &[
                "return:::std::ops::ControlFlow::Continue(())",
                "return:::std::ops::ControlFlow::Break(())",
            ],
        ),
        (
            "fn f(a: NodeId, b: NodeListId, c: NodeId) -> NodeId { x() }",
            "node_id",
            &["return:c"],
        ),
        (
            "fn f(a: NodeId) -> Result<crate::NodeId, E> { x() }",
            "result_node_id",
            &["return:Ok(a)"],
        ),
        (
            "fn f(node: Option<NodeId>) -> NodeId { x() }",
            "node_id",
            &["return:node.unwrap()", "param:node:None"],
        ),
        (
            "fn f(list: Option<NodeListId>) -> Option<NodeListId> { x() }",
            "option",
            &["return:None", "return:list"],
        ),
        (
            "fn f(text: &[u8], mut pos: usize) -> usize { x() }",
            "integer",
            &["return:text.len()", "return:0"],
        ),
        (
            "fn f(flag: bool) -> (bool, Option<NodeId>, NodeId) { x() }",
            "tuple",
            &["wrap_tuple:0:!", "wrap_tuple:1:None"],
        ),
        (
            "fn f<'a>(a: &'a [NodeId]) -> Result<(&'a [NodeId], Option<NodeId>), E> { x() }",
            "result_tuple",
            &["wrap_ok_tuple:1:None"],
        ),
        (
            "fn f() -> TextRange { x() }",
            "text_range",
            &["wrap_range:empty"],
        ),
        ("fn f() -> Self { x() }", "self", &[]),
        ("fn f<T>() -> T { x() }", "generic", &[]),
        ("fn f() -> ModuleKind { x() }", "other", &[]),
        ("const fn f() -> bool { true }", "const_fn", &[]),
    ];
    for (source, category, expected) in cases {
        let choice = choose(source, "f", Scope::default());
        assert_eq!(
            (choice.category.as_str(), names(&choice)),
            (*category, expected.to_vec()),
            "{source}"
        );
    }
}

#[test]
fn parser_values_are_result_wrappers() {
    let source = "impl<F> Parser<'_, F> { \
        fn a(&mut self) -> NodeId { x() } \
        fn b(&mut self) -> Option<NodeId> { x() } \
        fn c(&mut self) -> NodeListId { x() } \
        fn d(&mut self, node: NodeId) -> NodeId { x() } \
        fn e(&mut self, kind: K) -> bool { x() } \
        fn g(&mut self) -> NodeListId { self.factory.mark_list_missing(l); l } }";
    assert_eq!(
        names(&choose(source, "a", PARSER)),
        [
            "wrap_result:self.create_missing_identifier()",
            "wrap_flags:THIS_NODE_HAS_ERROR"
        ]
    );
    let optional = choose(source, "b", PARSER);
    assert_eq!(
        names(&optional),
        [
            "wrap_result:None",
            "wrap_present:self.create_missing_identifier()"
        ]
    );
    assert_eq!(
        wrap_value(&optional, 1).0,
        "{R}.map(|_| self.create_missing_identifier())",
        "a failed parse stays failed"
    );
    assert!(optional.operators[1].allocates && !optional.operators[0].allocates);
    assert_eq!(
        names(&choose(source, "c", PARSER)),
        [
            "wrap_result:self.create_missing_list()",
            "wrap_list_missing"
        ]
    );
    assert_eq!(
        names(&choose(source, "g", PARSER)),
        ["wrap_result:self.create_missing_list()"],
        "a list the body already marks missing is not marked again"
    );
    let with_param = choose(source, "d", PARSER);
    assert_eq!(
        names(&with_param),
        [
            "wrap_result:self.create_missing_identifier()",
            "wrap_param:node"
        ],
        "a same-typed parameter is returned after the body, from its entry value"
    );
    assert_eq!(wrap_value(&with_param, 1), ("{P}", Some("node")));
    assert_eq!(
        names(&choose(source, "e", PARSER)),
        ["return:true", "return:false"],
        "an early false from a consuming predicate is what kills it"
    );
    let chained = choose(
        source,
        "d",
        Scope {
            chain: true,
            ..PARSER
        },
    );
    assert_eq!(
        names(&chained),
        ["wrap_param:node", "wrap_flags:THIS_NODE_HAS_ERROR"]
    );
    assert_eq!(chained.notes.len(), 1, "the withheld operator is reported");
    let chain_root = choose(
        source,
        "a",
        Scope {
            chain: true,
            ..PARSER
        },
    );
    assert_eq!(
        names(&chain_root),
        ["wrap_flags:THIS_NODE_HAS_ERROR"],
        "flags never allocate, so they cannot recurse"
    );
    assert!(chain_root.notes[0].contains("create_missing_identifier"));
    assert_eq!(
        names(&choose(source, "a", Scope::default())),
        Vec::<&str>::new(),
        "not a parser scope"
    );
}

#[test]
fn scanners_set_the_token_and_constants_skip_the_equivalent_value() {
    let source = "impl<F> Parser<'_, F> { \
        fn next_token(&mut self) -> SyntaxKind { self.token = self.scan(); self.token } \
        fn rescan(&mut self, tagged: bool) -> SyntaxKind { self.token = self.scan(); self.token } \
        fn peek(&mut self) -> SyntaxKind { self.scan() } }";
    let scan = choose(source, "next_token", PARSER);
    assert_eq!(scan.category, "syntax_kind");
    assert_eq!(
        names(&scan),
        [
            "wrap_token:SyntaxKind::Unknown",
            "wrap_result:SyntaxKind::Unknown"
        ]
    );
    assert_eq!(
        wrap_value(&scan, 0).0,
        "if {R} == SyntaxKind::EndOfFile { {R} } else { self.token = SyntaxKind::Unknown; SyntaxKind::Unknown }"
    );
    assert_eq!(
        names(&choose(source, "rescan", PARSER)),
        ["wrap_token:SyntaxKind::Unknown", "param:tagged:!tagged"],
        "a parameter comes before the weak plain wrapper"
    );
    assert_eq!(
        names(&choose(source, "peek", PARSER)),
        ["wrap_result:SyntaxKind::Unknown"],
        "not a scanner: the token is left alone"
    );
    let facts = Scope {
        facts: true,
        ..Scope::default()
    };
    assert_eq!(
        names(&choose("fn f() -> u32 { NONE }", "f", facts)),
        ["return:!0"]
    );
    assert_eq!(
        names(&choose("fn f() -> u32 { CLASS_FIELDS }", "f", facts)),
        ["return:0", "return:!0"]
    );
    assert_eq!(
        names(&choose("fn f() -> bool { true }", "f", Scope::default())),
        ["return:false"]
    );
    let binder = Scope {
        binder: true,
        ..Scope::default()
    };
    assert_eq!(
        names(&choose(
            "fn f(&self) -> BindingFlow<'s> { x() }",
            "f",
            binder
        )),
        ["return:crate::need(self.unreachable_flow)"]
    );
    assert!(
        choose("fn f(&self) -> BindingFlowList<'s> { x() }", "f", binder)
            .operators
            .is_empty()
    );
}

#[test]
fn workspace_enums_and_newtypes_shift_the_real_result() {
    let root = std::env::temp_dir().join(format!("phase1-ops-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let path = root.join("crates/tsr_x/src/lib.rs");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(
        &path,
        "pub enum State { Unknown, Non, Inst }\npub enum Two { A, B(u8), C }\n\
         pub struct Kind(pub i32);\npub type Mode = Kind;\npub type Flags = u32;\n",
    )
    .unwrap();
    let wanted: BTreeSet<String> = ["State", "Two", "Kind", "Mode", "Flags"]
        .iter()
        .map(|name| (*name).to_owned())
        .collect();
    let types = TypeIndex::build(&root, &wanted).unwrap();
    let _ = std::fs::remove_dir_all(&root);
    let state = choose_with(
        "fn f(v: View) -> Result<a::State, Error> { x() }",
        "f",
        Scope::default(),
        &types,
    );
    assert_eq!(state.category, "result_enum");
    assert_eq!(
        names(&state),
        ["wrap_ok_variant:next", "wrap_ok_variant:previous"]
    );
    assert_eq!(
        wrap_value(&state, 0).0,
        "{R}.map(|__phase1_ok| match __phase1_ok { a::State::Unknown => a::State::Non, \
         a::State::Non => a::State::Inst, a::State::Inst => a::State::Unknown })"
    );
    let two = choose_with("fn f() -> Two { x() }", "f", Scope::default(), &types);
    assert_eq!(
        wrap_value(&two, 0).0,
        "match {R} { Two::A => Two::C, Two::C => Two::A, _ => Two::A }",
        "two unit variants, one data variant: a single shift and a catch-all"
    );
    assert_eq!(names(&two), ["wrap_variant:next"]);
    let mode = choose_with("fn f() -> Mode { x() }", "f", Scope::default(), &types);
    assert_eq!(names(&mode), ["wrap_newtype:^1"]);
    let flags = choose_with(
        "fn f(flags: Flags) -> Result<Flags, E> { x() }",
        "f",
        Scope::default(),
        &types,
    );
    assert_eq!(
        names(&flags),
        ["return:Ok(0)", "return:Ok(1)"],
        "an integer alias is an integer"
    );
    assert_eq!(
        names(&choose_with(
            "fn g(flags: Flags) -> Kind { x() }",
            "g",
            Scope::default(),
            &types
        )),
        ["wrap_newtype:^1", "param:flags:flags^1"]
    );
}

#[test]
fn parameters_fill_the_remaining_slots_in_order() {
    let source = "fn f(_skip: bool, flag: bool, mut kind: Option<K>, n: u32) { x() }";
    assert_eq!(
        names(&choose(source, "f", Scope::default())),
        ["skip_body", "param:flag:!flag"]
    );
    let source = "fn g(flag: bool, mut kind: Option<K>, n: u32) -> ModuleKind { x() }";
    let choice = choose(source, "g", Scope::default());
    assert_eq!(names(&choice), ["param:flag:!flag", "param:kind:None"]);
    assert!(matches!(
        &choice.operators[1].shape,
        Shape::Param { mutable: true, .. }
    ));
}

#[test]
fn arms_and_statements() {
    let source = "fn compute(&mut self) -> u32 { match k { A => NONE, B => CLASS_FIELDS | x, } }\n\
                  fn g() -> u32 { let y = match k { A => 1, _ => 2 }; y }\n\
                  fn h(kind: K) { if matches!(kind, K::A) { return; } let v = if a { 1 } else { 2 }; w(); }";
    let index = index_file(source).unwrap();
    let facts = Scope {
        facts: true,
        ..Scope::default()
    };
    let types = TypeIndex::default();
    let site = Site {
        scope: facts,
        function: &index.fns[0],
        types: &types,
        krate: "tsr_ast",
    };
    let arm = |line: usize, n: usize| {
        index
            .arms
            .iter()
            .filter(|arm| arm.start.0 == line)
            .nth(n)
            .unwrap()
    };
    assert_eq!(names(&for_arm(arm(1, 0), site)), ["arm:!0"]);
    assert_eq!(names(&for_arm(arm(1, 1), site)), ["arm:0", "arm:!0"]);
    let untyped = for_arm(arm(2, 0), site);
    assert!(untyped.operators.is_empty());
    assert_eq!(untyped.category, "unknown");
    let stmts: Vec<_> = index
        .stmts
        .iter()
        .filter(|stmt| stmt.start.0 == 3)
        .map(|stmt| names(&for_stmt(stmt)).join(","))
        .collect();
    // `if`, its `return;` (a skipped jump would fall through), the `let .. =
    // if`, the two branch tails, `w();` (skipped).
    assert_eq!(
        stmts,
        [
            "negate_condition",
            "",
            "negate_condition",
            "",
            "",
            "skip_statement"
        ]
    );
}

#[test]
fn allocation_follows_the_scope_rule() {
    assert!(allocating("self.create_missing_identifier()"));
    assert!(allocating("{R}.map(|_| self.create_missing_list ())"));
    assert!(allocating("f.new_identifier(x)"));
    assert!(!allocating("Vec::new()"));
    assert!(!allocating("TextRange::new(1, 2)"));
    assert!(!allocating("renew_x()"), "a word boundary is required");
    assert!(!allocating("self.create_missing_identifier"), "not a call");
}
