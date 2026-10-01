//! `transformers/utilities.go` over parsed files. Upstream has no tests for
//! this file; every expectation here was derived by reading the pinned Go
//! (`tsc/internal/transformers/utilities.go` and the `ast`, `core` and
//! `printer` functions it calls).
use crate::utilities::{
    convert_binding_pattern_to_assignment_pattern,
    convert_variable_declaration_to_assignment_expression, find_super_statement_index_path,
    get_non_assignment_operator_for_compound_assignment, get_super_call_from_statement,
    is_export_name, is_generated_identifier, is_helper_name, is_identifier_reference,
    is_local_name, is_original_node_single_line, is_simple_copiable_expression,
    is_simple_inlineable_expression, move_range_past_decorators, move_range_past_modifiers,
    single_or_many,
};
use std::ops::ControlFlow;
use tsr_arena::Counters;
use tsr_ast::{
    AstBuilder, ChildVisitor, Factory, FactoryMethods, JsString, NodeId, NodeListId, NodeSlice,
    RuntimeFactory, SourceFileParseOptions, SyntaxKind as K,
};
use tsr_core::{ScriptKind, TextRange};
use tsr_jsstring::SourceText;
use tsr_printer::{emit_flags, EmitContext};

pub(crate) struct Fixture {
    pub(crate) context: EmitContext,
    pub(crate) factory: AstBuilder,
    pub(crate) root: NodeId,
    pub(crate) text: &'static str,
}

pub(crate) fn parse(text: &'static str) -> Fixture {
    parse_as(text, ScriptKind::TS, b"/main.ts")
}

pub(crate) fn parse_tsx(text: &'static str) -> Fixture {
    parse_as(text, ScriptKind::TSX, b"/main.tsx")
}

fn parse_as(text: &'static str, kind: ScriptKind, name: &'static [u8]) -> Fixture {
    let context = EmitContext::new();
    let mut factory = AstBuilder::with_hooks(
        SourceText::default(),
        &Counters::new(),
        context.factory_hooks(),
    );
    let parsed = tsr_parser::parse_source_file(
        SourceText::from_loaded_bytes(text.as_bytes()),
        kind,
        SourceFileParseOptions {
            file_name: JsString::from_bytes(name),
            path: JsString::from_bytes(name),
            ..Default::default()
        },
    );
    let root = parsed.root();
    factory.retain_file(parsed.publish_unbound());
    Fixture {
        context,
        factory,
        root,
        text,
    }
}

/// Pre-order child collection, lists and raw slices included.
struct Children<'a> {
    factory: &'a AstBuilder,
    nodes: Vec<NodeId>,
}
impl ChildVisitor for Children<'_> {
    fn visit_node(&mut self, node: NodeId) -> ControlFlow<()> {
        self.nodes.push(node);
        ControlFlow::Continue(())
    }
    fn visit_list(&mut self, list: NodeListId) -> ControlFlow<()> {
        self.visit_node_slice(self.factory.read_list(list).nodes())
    }
    fn visit_node_slice(&mut self, nodes: NodeSlice) -> ControlFlow<()> {
        self.nodes
            .extend(self.factory.read_nodes(nodes).iter().flatten());
        ControlFlow::Continue(())
    }
}

impl Fixture {
    /// Every node under the root, in pre-order.
    pub(crate) fn all(&self) -> Vec<NodeId> {
        let mut out = Vec::new();
        let mut stack = vec![self.root];
        while let Some(node) = stack.pop() {
            out.push(node);
            let mut children = Children {
                factory: &self.factory,
                nodes: Vec::new(),
            };
            let _ = Factory::node(&self.factory, node).for_each_child(&mut children);
            stack.extend(children.nodes.into_iter().rev());
        }
        out
    }

    /// The `index`th node of `kind` in pre-order.
    pub(crate) fn nth(&self, kind: K, index: usize) -> NodeId {
        self.all()
            .into_iter()
            .filter(|&node| self.kind(node) == kind)
            .nth(index)
            .unwrap_or_else(|| panic!("no {kind:?} #{index} in {:?}", self.text))
    }

    pub(crate) fn first(&self, kind: K) -> NodeId {
        self.nth(kind, 0)
    }

    /// The first identifier with `text`, in pre-order.
    pub(crate) fn identifier(&self, text: &str) -> NodeId {
        self.all()
            .into_iter()
            .find(|&node| self.identifier_text(node).as_deref() == Some(text.as_bytes()))
            .unwrap_or_else(|| panic!("no identifier {text} in {:?}", self.text))
    }

    pub(crate) fn kind(&self, node: NodeId) -> K {
        Factory::node(&self.factory, node)
            .kind()
            .known()
            .expect("known kind")
    }

    pub(crate) fn identifier_text(&self, node: NodeId) -> Option<Vec<u8>> {
        Factory::node(&self.factory, node)
            .as_identifier()
            .map(|data| data.text().to_vec())
    }

    pub(crate) fn text_of(&self, node: NodeId) -> Vec<u8> {
        self.identifier_text(node)
            .unwrap_or_else(|| panic!("{:?} is not an identifier", self.kind(node)))
    }

    pub(crate) fn parent(&self, node: NodeId) -> NodeId {
        Factory::node(&self.factory, node)
            .parent()
            .expect("parsed nodes have parents")
    }

    pub(crate) fn list(&self, list: Option<NodeListId>) -> Vec<Option<NodeId>> {
        let list = list.expect("a list");
        let nodes = self.factory.read_list(list).nodes();
        self.factory.read_nodes(nodes).iter().collect()
    }

    pub(crate) fn range(&self, node: NodeId) -> TextRange {
        Factory::node(&self.factory, node).range()
    }

    /// The statements of the source file.
    pub(crate) fn statements(&self) -> Vec<NodeId> {
        let statements = Factory::node(&self.factory, self.root)
            .as_source_file()
            .expect("source file")
            .statements();
        self.list(statements)
            .into_iter()
            .map(|node| node.expect("statement"))
            .collect()
    }

    /// The byte offset of the `occurrence`th `needle` in the text.
    pub(crate) fn offset(&self, needle: &str, occurrence: usize) -> i64 {
        let mut from = 0;
        for _ in 0..occurrence {
            from += self.text[from..].find(needle).expect("needle") + 1;
        }
        (from + self.text[from..].find(needle).expect("needle")) as i64
    }
}

#[test]
fn emit_name_predicates_read_their_emit_flags_and_generated_names() {
    let mut fixture = parse("a; b; c; d;");
    let [a, b, c, d] = ["a", "b", "c", "d"].map(|name| fixture.identifier(name));
    fixture.context.add_emit_flags(a, emit_flags::HELPER_NAME);
    fixture.context.add_emit_flags(b, emit_flags::LOCAL_NAME);
    fixture.context.add_emit_flags(c, emit_flags::EXPORT_NAME);
    let generated = fixture
        .context
        .new_unique_name(&mut fixture.factory, JsString::from_bytes(&b"_a"[..]));
    let context = &fixture.context;
    let table = [
        (a, [true, false, false, false]),
        (b, [false, true, false, false]),
        (c, [false, false, true, false]),
        (d, [false, false, false, false]),
        (generated, [false, false, false, true]),
    ];
    for (node, expected) in table {
        assert_eq!(
            [
                is_helper_name(context, node),
                is_local_name(context, node),
                is_export_name(context, node),
                is_generated_identifier(context, node),
            ],
            expected
        );
    }
}

/// `(snippet, identifier, expected)` with the identifier's parsed parent.
fn assert_references(cases: &[(&'static str, &str, bool)], tsx: bool) {
    for &(text, name, expected) in cases {
        let fixture = if tsx { parse_tsx(text) } else { parse(text) };
        let node = fixture.identifier(name);
        let parent = fixture.parent(node);
        assert_eq!(
            is_identifier_reference(&fixture.factory, node, parent),
            expected,
            "{name} in {text:?} under {:?}",
            fixture.kind(parent)
        );
    }
}

#[test]
fn every_identifier_child_of_an_expression_parent_is_a_reference() {
    assert_references(
        &[
            ("a + 1;", "a", true),
            ("++a;", "a", true),
            ("a++;", "a", true),
            ("function* g() { yield a; }", "a", true),
            ("a as T;", "a", true),
            ("a satisfies T;", "a", true),
            ("o[a];", "a", true),
            ("o[a];", "o", true),
            ("a!;", "a", true),
            ("[...a];", "a", true),
            ("({ ...a });", "a", true),
            ("(a);", "a", true),
            ("[a];", "a", true),
            ("delete a;", "a", true),
            ("typeof a;", "a", true),
            ("void a;", "a", true),
            ("async function f() { await a; }", "a", true),
            ("<T>a;", "a", true),
            ("class C extends a {}", "a", true),
        ],
        false,
    );
    assert_references(
        &[
            ("<a />;", "a", true),
            ("<div {...a} />;", "a", true),
            ("<div>{a}</div>;", "a", true),
        ],
        true,
    );
    // A partially emitted expression only arises in transforms.
    let mut fixture = parse("a;");
    let a = fixture.identifier("a");
    let partial = fixture.factory.new_partially_emitted_expression(Some(a));
    assert!(is_identifier_reference(&fixture.factory, a, partial));
}

#[test]
fn only_the_expression_child_is_a_reference_under_expression_holders() {
    assert_references(
        &[
            ("({ [a]: 1 });", "a", true),
            ("@a class C {}", "a", true),
            ("if (a) b;", "a", true),
            ("do {} while (a);", "a", true),
            ("while (a) {}", "a", true),
            ("with (a) {}", "a", true),
            ("function f() { return a; }", "a", true),
            ("switch (a) { case b: }", "a", true),
            ("switch (x) { case b: }", "b", true),
            ("throw a;", "a", true),
            ("a;", "a", true),
            ("export default a;", "a", true),
            ("a.b;", "a", true),
            ("a.b;", "b", false),
            ("`${a}`;", "a", true),
        ],
        false,
    );
}

#[test]
fn only_the_initializer_is_a_reference_under_declarations() {
    assert_references(
        &[
            ("let x = a;", "a", true),
            ("let x = a;", "x", false),
            ("function f(p = a) {}", "a", true),
            ("function f(p = a) {}", "p", false),
            ("let { x = a } = o;", "a", true),
            ("let { x = a } = o;", "x", false),
            ("let { y: x } = o;", "y", false),
            ("class C { p = a; }", "a", true),
            ("class C { p = a; }", "p", false),
            ("({ p: a });", "a", true),
            ("({ p: a });", "p", false),
            ("enum E { M = a }", "a", true),
            ("enum E { M = a }", "M", false),
        ],
        false,
    );
    // A JSX attribute's initializer is a string or a JSX expression; its name
    // is never a reference.
    assert_references(&[("<div x={a} />;", "x", false)], true);
    // A property signature's initializer is a grammar error the parser keeps
    // out of the tree; build one.
    let mut fixture = parse("a; p;");
    let (a, p) = (fixture.identifier("a"), fixture.identifier("p"));
    let signature =
        fixture
            .factory
            .new_property_signature_declaration(None, Some(p), None, None, Some(a));
    assert!(is_identifier_reference(&fixture.factory, a, signature));
    assert!(!is_identifier_reference(&fixture.factory, p, signature));
}

#[test]
fn the_remaining_parent_kinds_name_their_reference_children() {
    assert_references(
        &[
            ("({ a = b } = o);", "b", true),
            ("({ a = b } = o);", "a", false),
            ("for (a; b; c) {}", "a", true),
            ("for (a; b; c) {}", "b", true),
            ("for (a; b; c) {}", "c", true),
            ("for (a in b) {}", "a", true),
            ("for (a in b) {}", "b", true),
            ("for (a of b) {}", "a", true),
            ("for (a of b) {}", "b", true),
            ("import x = a;", "a", true),
            ("import x = a;", "x", false),
            ("import x = a.b;", "a", false),
            ("() => a;", "a", true),
            ("(p) => 0;", "p", false),
            ("a ? b : c;", "a", true),
            ("a ? b : c;", "b", true),
            ("a ? b : c;", "c", true),
            ("f(a);", "f", true),
            ("f(x, a);", "a", true),
            ("new C(a);", "C", true),
            ("new C(a);", "a", true),
            ("new C;", "C", true),
            ("f<T>(a);", "T", false),
            ("t`x`;", "t", true),
            ("label: a;", "label", false),
            ("class a {}", "a", false),
        ],
        false,
    );
    assert_references(
        &[("<a></a>;", "a", true), ("<x.a></x.a>;", "a", false)],
        true,
    );
    // An import attribute's value; its name is not a reference.
    let mut fixture = parse("a; type;");
    let (a, name) = (fixture.identifier("a"), fixture.identifier("type"));
    let attribute = fixture.factory.new_import_attribute(Some(name), Some(a));
    assert!(is_identifier_reference(&fixture.factory, a, attribute));
    assert!(!is_identifier_reference(&fixture.factory, name, attribute));
}

/// The node's original is `original` and it took `original`'s ranges.
fn assert_converted_from(fixture: &Fixture, node: NodeId, original: NodeId) {
    assert_eq!(fixture.context.original(node), Some(original));
    assert_eq!(
        fixture.context.comment_range(node),
        Some(fixture.range(original))
    );
    assert_eq!(
        fixture.context.source_map_range(&fixture.factory, node),
        fixture.range(original)
    );
}

fn binding_elements(fixture: &Fixture, pattern: NodeId) -> Vec<NodeId> {
    let elements = Factory::node(&fixture.factory, pattern)
        .as_binding_pattern()
        .expect("binding pattern")
        .elements();
    fixture
        .list(elements)
        .into_iter()
        .map(|node| node.expect("element"))
        .collect()
}

fn literal_elements(fixture: &Fixture, literal: NodeId) -> Vec<NodeId> {
    let read = Factory::node(&fixture.factory, literal);
    let list = match fixture.kind(literal) {
        K::ArrayLiteralExpression => read.as_array_literal_expression().unwrap().elements(),
        K::ObjectLiteralExpression => read.as_object_literal_expression().unwrap().properties(),
        kind => panic!("not a literal: {kind:?}"),
    };
    fixture
        .list(list)
        .into_iter()
        .map(|node| node.expect("element"))
        .collect()
}

fn binary(fixture: &Fixture, node: NodeId) -> (NodeId, K, NodeId) {
    let read = Factory::node(&fixture.factory, node);
    let data = read.as_binary_expression().expect("binary expression");
    (
        data.left().unwrap(),
        fixture.kind(data.operator_token().unwrap()),
        data.right().unwrap(),
    )
}

#[test]
fn array_binding_patterns_become_array_literals() {
    let mut fixture = parse("let [a, , ...r] = x; let [b = 1, [c], { d }] = y;");
    let first = fixture.nth(K::ArrayBindingPattern, 0);
    let [a, hole, rest] = binding_elements(&fixture, first)[..] else {
        panic!("three elements")
    };
    let context = fixture.context.clone();
    let literal =
        convert_binding_pattern_to_assignment_pattern(&context, &mut fixture.factory, first);
    assert_eq!(fixture.kind(literal), K::ArrayLiteralExpression);
    assert_converted_from(&fixture, literal, first);
    let read = Factory::node(&fixture.factory, literal);
    let data = read.as_array_literal_expression().unwrap();
    assert!(!data.multi_line());
    let list = data.elements().unwrap();
    drop(read);
    let pattern_list = Factory::node(&fixture.factory, first)
        .as_binding_pattern()
        .unwrap()
        .elements()
        .unwrap();
    assert_eq!(
        fixture.factory.read_list(list).loc(),
        fixture.factory.read_list(pattern_list).loc()
    );
    let [name, elision, spread] = literal_elements(&fixture, literal)[..] else {
        panic!("three elements")
    };
    // A plain name is the binding's own identifier, not a copy.
    assert_eq!(Some(name), Factory::node(&fixture.factory, a).name());
    assert_eq!(fixture.context.original(name), None);
    assert_eq!(fixture.kind(elision), K::OmittedExpression);
    assert_converted_from(&fixture, elision, hole);
    assert_eq!(fixture.kind(spread), K::SpreadElement);
    assert_converted_from(&fixture, spread, rest);
    assert_eq!(
        Factory::node(&fixture.factory, spread).expression(),
        Some(fixture.identifier("r"))
    );

    let second = fixture.nth(K::ArrayBindingPattern, 1);
    let [b, inner_array_element, inner_object_element] = binding_elements(&fixture, second)[..]
    else {
        panic!("three elements")
    };
    let literal =
        convert_binding_pattern_to_assignment_pattern(&context, &mut fixture.factory, second);
    let [assignment, inner_array, inner_object] = literal_elements(&fixture, literal)[..] else {
        panic!("three elements")
    };
    assert_converted_from(&fixture, assignment, b);
    let (left, operator, right) = binary(&fixture, assignment);
    assert_eq!(fixture.text_of(left), b"b");
    assert_eq!(operator, K::EqualsToken);
    assert_eq!(fixture.kind(right), K::NumericLiteral);
    // A nested pattern converts to a nested literal whose original is the
    // nested pattern, not the element holding it.
    let inner_pattern = Factory::node(&fixture.factory, inner_array_element)
        .name()
        .unwrap();
    assert_eq!(fixture.kind(inner_array), K::ArrayLiteralExpression);
    assert_converted_from(&fixture, inner_array, inner_pattern);
    assert_eq!(
        literal_elements(&fixture, inner_array),
        [fixture.identifier("c")]
    );
    let inner_pattern = Factory::node(&fixture.factory, inner_object_element)
        .name()
        .unwrap();
    assert_eq!(fixture.kind(inner_object), K::ObjectLiteralExpression);
    assert_converted_from(&fixture, inner_object, inner_pattern);
    let [shorthand] = literal_elements(&fixture, inner_object)[..] else {
        panic!("one property")
    };
    assert_eq!(fixture.kind(shorthand), K::ShorthandPropertyAssignment);
}

#[test]
fn object_binding_patterns_become_object_literals() {
    let mut fixture = parse(
        "let { a, b: c, d = 1, e: f = 2, [k]: g, ...rest } = x; let { h: { i }, j: [l] } = y;",
    );
    let first = fixture.nth(K::ObjectBindingPattern, 0);
    let elements = binding_elements(&fixture, first);
    let context = fixture.context.clone();
    let literal =
        convert_binding_pattern_to_assignment_pattern(&context, &mut fixture.factory, first);
    assert_eq!(fixture.kind(literal), K::ObjectLiteralExpression);
    assert_converted_from(&fixture, literal, first);
    assert!(!Factory::node(&fixture.factory, literal)
        .as_object_literal_expression()
        .unwrap()
        .multi_line());
    let properties = literal_elements(&fixture, literal);
    assert_eq!(properties.len(), 6);
    for (&property, &element) in properties.iter().zip(&elements) {
        assert_converted_from(&fixture, property, element);
    }
    let read = |node: NodeId| Factory::node(&fixture.factory, node);

    // `a`: a shorthand without an equals token.
    let shorthand = read(properties[0]);
    let data = shorthand.as_shorthand_property_assignment().unwrap();
    assert_eq!(data.name(), Some(fixture.identifier("a")));
    assert_eq!(data.equals_token(), None);
    assert_eq!(data.object_assignment_initializer(), None);
    drop(shorthand);

    // `b: c`: the property name and the target as they are.
    let assignment = read(properties[1]);
    let data = assignment.as_property_assignment().unwrap();
    assert_eq!(data.name(), Some(fixture.identifier("b")));
    assert_eq!(data.initializer(), Some(fixture.identifier("c")));
    drop(assignment);

    // `d = 1`: a shorthand with a new equals token and the default.
    let shorthand = read(properties[2]);
    let data = shorthand.as_shorthand_property_assignment().unwrap();
    assert_eq!(data.name(), Some(fixture.identifier("d")));
    let equals = data.equals_token().expect("equals token");
    let default = data.object_assignment_initializer().expect("default");
    drop(shorthand);
    assert_eq!(fixture.kind(equals), K::EqualsToken);
    assert_eq!(
        Some(default),
        Factory::node(&fixture.factory, elements[2]).initializer()
    );

    // `e: f = 2`: the default becomes an assignment inside the property,
    // which alone carries the original.
    let assignment = read(properties[3]);
    let data = assignment.as_property_assignment().unwrap();
    assert_eq!(data.name(), Some(fixture.identifier("e")));
    let value = data.initializer().unwrap();
    drop(assignment);
    let (left, operator, _) = binary(&fixture, value);
    assert_eq!(
        (fixture.text_of(left), operator),
        (b"f".to_vec(), K::EqualsToken)
    );
    assert_eq!(fixture.context.original(value), None);

    // `[k]: g`: the computed name is kept.
    let assignment = read(properties[4]);
    let name = assignment.as_property_assignment().unwrap().name().unwrap();
    drop(assignment);
    assert_eq!(fixture.kind(name), K::ComputedPropertyName);

    // `...rest`: a spread assignment of the name.
    assert_eq!(fixture.kind(properties[5]), K::SpreadAssignment);
    assert_eq!(
        read(properties[5]).expression(),
        Some(fixture.identifier("rest"))
    );

    let second = fixture.nth(K::ObjectBindingPattern, 1);
    let literal =
        convert_binding_pattern_to_assignment_pattern(&context, &mut fixture.factory, second);
    let [h, j] = literal_elements(&fixture, literal)[..] else {
        panic!("two properties")
    };
    let nested = Factory::node(&fixture.factory, h).initializer().unwrap();
    assert_eq!(fixture.kind(nested), K::ObjectLiteralExpression);
    let nested = Factory::node(&fixture.factory, j).initializer().unwrap();
    assert_eq!(fixture.kind(nested), K::ArrayLiteralExpression);
    assert_eq!(
        literal_elements(&fixture, nested),
        [fixture.identifier("l")]
    );
}

#[test]
#[should_panic(expected = "Unknown binding pattern")]
fn converting_a_non_pattern_panics() {
    let mut fixture = parse("a;");
    let a = fixture.identifier("a");
    let context = fixture.context.clone();
    let _ = convert_binding_pattern_to_assignment_pattern(&context, &mut fixture.factory, a);
}

#[test]
fn variable_declarations_become_assignments_only_with_an_initializer() {
    let mut fixture = parse("let { a } = x, b = 1, c;");
    let declarations: Vec<NodeId> = fixture
        .all()
        .into_iter()
        .filter(|&node| fixture.kind(node) == K::VariableDeclaration)
        .collect();
    let context = fixture.context.clone();
    let destructuring = convert_variable_declaration_to_assignment_expression(
        &context,
        &mut fixture.factory,
        declarations[0],
    )
    .expect("an assignment");
    assert_converted_from(&fixture, destructuring, declarations[0]);
    let (left, operator, right) = binary(&fixture, destructuring);
    assert_eq!(fixture.kind(left), K::ObjectLiteralExpression);
    assert_eq!(operator, K::EqualsToken);
    assert_eq!(right, fixture.identifier("x"));

    let simple = convert_variable_declaration_to_assignment_expression(
        &context,
        &mut fixture.factory,
        declarations[1],
    )
    .expect("an assignment");
    let (left, _, right) = binary(&fixture, simple);
    assert_eq!(left, fixture.identifier("b"));
    assert_eq!(fixture.kind(right), K::NumericLiteral);

    assert_eq!(
        convert_variable_declaration_to_assignment_expression(
            &context,
            &mut fixture.factory,
            declarations[2]
        ),
        None
    );
}

#[test]
fn single_or_many_keeps_nil_one_and_many_apart() {
    let mut fixture = parse("a; b;");
    let (a, b) = (fixture.identifier("a"), fixture.identifier("b"));
    let factory = &mut fixture.factory;
    assert_eq!(single_or_many(factory, None), None);
    assert_eq!(single_or_many(factory, Some(&[a])), Some(a));
    for nodes in [&[][..], &[a, b][..]] {
        let list = single_or_many(factory, Some(nodes)).expect("a syntax list");
        let read = Factory::node(factory, list);
        assert_eq!(read.kind(), K::SyntaxList);
        let children = read.as_syntax_list().unwrap().children();
        drop(read);
        let children: Vec<_> = factory.read_nodes(children).iter().flatten().collect();
        assert_eq!(children, nodes);
    }
}

#[test]
fn simple_copiable_and_inlineable_expressions() {
    let fixture = parse("'s'; `t`; 1; true; null; this; a; undefined; a.b; f(); 1n; `x${a}`; (1);");
    let table = [
        (K::StringLiteral, true, true),
        (K::NoSubstitutionTemplateLiteral, true, true),
        (K::NumericLiteral, true, true),
        (K::TrueKeyword, true, true),
        (K::NullKeyword, true, true),
        (K::ThisKeyword, true, true),
        (K::Identifier, true, false),
        (K::Identifier, true, false),
        (K::PropertyAccessExpression, false, false),
        (K::CallExpression, false, false),
        (K::BigIntLiteral, false, false),
        (K::TemplateExpression, false, false),
        (K::ParenthesizedExpression, false, false),
    ];
    let statements = fixture.statements();
    assert_eq!(statements.len(), table.len());
    for (statement, (kind, copiable, inlineable)) in statements.into_iter().zip(table) {
        let expression = Factory::node(&fixture.factory, statement)
            .expression()
            .unwrap();
        assert_eq!(fixture.kind(expression), kind);
        assert_eq!(
            is_simple_copiable_expression(&fixture.factory, expression),
            copiable,
            "{kind:?}"
        );
        assert_eq!(
            is_simple_inlineable_expression(&fixture.factory, expression),
            inlineable,
            "{kind:?}"
        );
    }
}

#[test]
fn single_line_reads_the_most_original_node_in_its_source_file() {
    let mut fixture = parse("let a = 1;\nlet b = {\n};\n");
    let [single, multi] = fixture.statements()[..] else {
        panic!("two statements")
    };
    let context = fixture.context.clone();
    let factory = &fixture.factory;
    assert!(is_original_node_single_line(&context, factory, Some(single)).unwrap());
    assert!(!is_original_node_single_line(&context, factory, Some(multi)).unwrap());
    assert!(!is_original_node_single_line(&context, factory, None).unwrap());

    // A synthesized node answers for its most original node; without one it
    // has no source file.
    let mut context = fixture.context.clone();
    let alone = fixture.factory.new_omitted_expression();
    let copy_of_single = fixture.factory.new_omitted_expression();
    let copy_of_copy = fixture.factory.new_omitted_expression();
    let copy_of_multi = fixture.factory.new_omitted_expression();
    context.set_original(copy_of_single, single);
    context.set_original(copy_of_copy, copy_of_single);
    context.set_original(copy_of_multi, multi);
    let factory = &fixture.factory;
    assert!(!is_original_node_single_line(&context, factory, Some(alone)).unwrap());
    assert!(is_original_node_single_line(&context, factory, Some(copy_of_copy)).unwrap());
    assert!(!is_original_node_single_line(&context, factory, Some(copy_of_multi)).unwrap());
}

/// The constructor body statements of the only class in `text`.
fn constructor_statements(fixture: &Fixture) -> Vec<NodeId> {
    let constructor = fixture.first(K::Constructor);
    let body = Factory::node(&fixture.factory, constructor).body().unwrap();
    let statements = Factory::node(&fixture.factory, body).statement_list();
    fixture
        .list(statements)
        .into_iter()
        .map(|node| node.expect("statement"))
        .collect()
}

#[test]
fn super_statement_paths_descend_into_try_blocks() {
    let cases: [(&'static str, usize, &[usize]); 6] = [
        (
            "class C extends B { constructor() { a; super(); b; } }",
            0,
            &[1],
        ),
        (
            "class C extends B { constructor() { a; try { b; (super()); } finally {} } }",
            0,
            &[1, 1],
        ),
        (
            "class C extends B { constructor() { try { try { x; super(); } catch {} } catch {} } }",
            0,
            &[0, 0, 1],
        ),
        (
            "class C extends B { constructor() { a; super.m(); try { b; } finally {} } }",
            0,
            &[],
        ),
        (
            "class C extends B { constructor() { super(); a; } }",
            1,
            &[],
        ),
        (
            "class C extends B { constructor() { a; super(); } }",
            7,
            &[],
        ),
    ];
    for (text, start, expected) in cases {
        let fixture = parse(text);
        let statements = constructor_statements(&fixture);
        assert_eq!(
            find_super_statement_index_path(&fixture.factory, &statements, start),
            expected,
            "{text}"
        );
    }
}

#[test]
fn super_calls_are_found_through_parentheses_in_expression_statements() {
    let fixture =
        parse("class C extends B { constructor() { ((super(1))); super.m(); if (a) super(); } }");
    let [parenthesized, method, nested] = constructor_statements(&fixture)[..] else {
        panic!("three statements")
    };
    let call = get_super_call_from_statement(&fixture.factory, parenthesized).expect("a call");
    assert_eq!(fixture.kind(call), K::CallExpression);
    assert_eq!(
        fixture.kind(Factory::node(&fixture.factory, call).expression().unwrap()),
        K::SuperKeyword
    );
    assert_eq!(
        get_super_call_from_statement(&fixture.factory, method),
        None
    );
    assert_eq!(
        get_super_call_from_statement(&fixture.factory, nested),
        None
    );
}

#[test]
fn ranges_move_past_modifiers_and_decorators() {
    let mut fixture = parse(
        "class C { @d static p = 1; @d m() {} }\n@e export class D {}\nexport let y = 1;\nlet x = 1;\n@f @g class E {}\n",
    );
    let end =
        |fixture: &Fixture, node: NodeId| i64::from(Factory::node(&fixture.factory, node).end());
    let property = fixture.first(K::PropertyDeclaration);
    let method = fixture.first(K::MethodDeclaration);
    let [_, exported_class, exported_let, plain, decorated] = fixture.statements()[..] else {
        panic!("five statements")
    };

    // A property or method starts at its name.
    let p = fixture.identifier("p");
    assert_eq!(
        move_range_past_modifiers(&fixture.factory, property),
        TextRange::new(
            i64::from(Factory::node(&fixture.factory, p).pos()),
            end(&fixture, property)
        )
    );
    let m = fixture.identifier("m");
    assert_eq!(
        move_range_past_modifiers(&fixture.factory, method),
        TextRange::new(
            i64::from(Factory::node(&fixture.factory, m).pos()),
            end(&fixture, method)
        )
    );

    // Past the last modifier, which may follow a decorator.
    let after_export = fixture.offset("export", 0) + 6;
    assert_eq!(
        move_range_past_modifiers(&fixture.factory, exported_class),
        TextRange::new(after_export, end(&fixture, exported_class))
    );
    let after_e = fixture.offset("@e", 0) + 2;
    assert_eq!(
        move_range_past_decorators(&fixture.factory, exported_class),
        TextRange::new(after_e, end(&fixture, exported_class))
    );
    let after_second_export = fixture.offset("export", 1) + 6;
    assert_eq!(
        move_range_past_modifiers(&fixture.factory, exported_let),
        TextRange::new(after_second_export, end(&fixture, exported_let))
    );
    // No decorator: the node's own range.
    assert_eq!(
        move_range_past_decorators(&fixture.factory, exported_let),
        fixture.range(exported_let)
    );
    // No modifiers at all: the node's own range.
    assert_eq!(
        move_range_past_modifiers(&fixture.factory, plain),
        fixture.range(plain)
    );
    // Only decorators: past the last one.
    let after_g = fixture.offset("@g", 0) + 2;
    assert_eq!(
        move_range_past_modifiers(&fixture.factory, decorated),
        TextRange::new(after_g, end(&fixture, decorated))
    );
    // An expression statement cannot have modifiers.
    let statement = fixture.factory.new_expression_statement(Some(p));
    assert_eq!(
        move_range_past_modifiers(&fixture.factory, statement),
        TextRange::new(-1, -1)
    );

    // A synthesized last modifier falls back to the decorators, then to the
    // node's range.
    let export = fixture.factory.new_modifier(K::ExportKeyword.into());
    let nodes = fixture.factory.alloc_nodes(vec![Some(export)]);
    let modifiers = fixture.factory.new_modifier_list(nodes);
    let name = fixture.identifier("D");
    let synthesized =
        fixture
            .factory
            .new_class_declaration(Some(modifiers), Some(name), None, None, None);
    fixture
        .factory
        .set_node_range(synthesized, TextRange::new(3, 9));
    assert_eq!(
        move_range_past_modifiers(&fixture.factory, synthesized),
        TextRange::new(3, 9)
    );
    // A synthesized decorator likewise.
    let decorator = fixture.factory.new_decorator(Some(name));
    let nodes = fixture.factory.alloc_nodes(vec![Some(decorator)]);
    let modifiers = fixture.factory.new_modifier_list(nodes);
    let synthesized =
        fixture
            .factory
            .new_class_declaration(Some(modifiers), Some(name), None, None, None);
    fixture
        .factory
        .set_node_range(synthesized, TextRange::new(4, 8));
    assert_eq!(
        move_range_past_decorators(&fixture.factory, synthesized),
        TextRange::new(4, 8)
    );
}

#[test]
fn compound_assignments_map_to_their_operators() {
    let table = [
        (K::PlusEqualsToken, K::PlusToken),
        (K::MinusEqualsToken, K::MinusToken),
        (K::AsteriskEqualsToken, K::AsteriskToken),
        (K::AsteriskAsteriskEqualsToken, K::AsteriskAsteriskToken),
        (K::SlashEqualsToken, K::SlashToken),
        (K::PercentEqualsToken, K::PercentToken),
        (K::LessThanLessThanEqualsToken, K::LessThanLessThanToken),
        (
            K::GreaterThanGreaterThanEqualsToken,
            K::GreaterThanGreaterThanToken,
        ),
        (
            K::GreaterThanGreaterThanGreaterThanEqualsToken,
            K::GreaterThanGreaterThanGreaterThanToken,
        ),
        (K::AmpersandEqualsToken, K::AmpersandToken),
        (K::BarEqualsToken, K::BarToken),
        (K::CaretEqualsToken, K::CaretToken),
        (K::BarBarEqualsToken, K::BarBarToken),
        (K::AmpersandAmpersandEqualsToken, K::AmpersandAmpersandToken),
        (K::QuestionQuestionEqualsToken, K::QuestionQuestionToken),
        // Anything else is returned unchanged.
        (K::EqualsToken, K::EqualsToken),
        (K::PlusToken, K::PlusToken),
        (K::Identifier, K::Identifier),
    ];
    for (compound, operator) in table {
        assert_eq!(
            get_non_assignment_operator_for_compound_assignment(compound.into()),
            operator,
            "{compound:?}"
        );
    }
}
