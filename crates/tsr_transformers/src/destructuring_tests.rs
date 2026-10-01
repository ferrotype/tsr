//! `transformers/destructuring.go` over parsed files. Upstream has no tests
//! for this file; every expected flattening here was derived by tracing the
//! pinned Go (`tsc/internal/transformers/destructuring.go` and the `ast` and
//! `printer` functions it calls) by hand for the input.
//!
//! The printer cannot print statements in this branch, so results are
//! rendered structurally: kinds, texts and child links, with temporaries named
//! `_a`, `_b`, ... in creation order.
use crate::destructuring::{
    binding_or_assignment_element_assigns_to_name,
    binding_or_assignment_element_contains_non_literal_computed_name,
    flatten_destructuring_assignment, flatten_destructuring_binding,
    get_initializer_of_binding_or_assignment_element, CreateAssignmentCallback, FlattenLevel,
};
use crate::utilities_tests::{parse, Fixture};
use std::cell::RefCell;
use std::ops::ControlFlow;
use tsr_ast::{
    node_flags, ChildVisitor, Factory, FactoryMethods, JsString, NodeId, NodeListId, NodeSlice,
    NodeVisit, NodeVisitor, NodeVisitorHooks, RuntimeFactory, SyntaxKind as K,
};
use tsr_core::TextRange;

fn identity(_: &mut NodeVisitor<'_>, node: Option<NodeId>) -> Option<NodeId> {
    node
}

/// A flattening's result and the temporaries it hoisted.
struct Run {
    result: Option<NodeId>,
    hoisted: Vec<NodeId>,
}

/// Runs `flatten` with an identity visitor inside a variable environment.
fn run(
    fixture: &mut Fixture,
    flatten: impl FnOnce(&mut NodeVisitor<'_>, &tsr_printer::EmitContext) -> Option<NodeId>,
) -> Run {
    let mut context = fixture.context.clone();
    context.start_variable_environment();
    let result = {
        let visit: &NodeVisit<'_> = &identity;
        let mut visitor = NodeVisitor::new(
            Some(visit),
            Some(&mut fixture.factory),
            NodeVisitorHooks::default(),
        );
        flatten(&mut visitor, &context)
    };
    let statements = context.end_variable_environment(&mut fixture.factory);
    let mut hoisted = Vec::new();
    for statement in statements {
        let list = Factory::node(&fixture.factory, statement)
            .as_variable_statement()
            .expect("the hoisted var statement")
            .declaration_list()
            .unwrap();
        let declarations = Factory::node(&fixture.factory, list)
            .as_variable_declaration_list()
            .unwrap()
            .declarations();
        for declaration in fixture.list(declarations) {
            hoisted.push(
                Factory::node(&fixture.factory, declaration.unwrap())
                    .name()
                    .unwrap(),
            );
        }
    }
    Run { result, hoisted }
}

fn assignment(
    fixture: &mut Fixture,
    node: NodeId,
    needs_value: bool,
    level: FlattenLevel,
    callback: Option<&CreateAssignmentCallback<'_>>,
) -> Run {
    run(fixture, |visitor, context| {
        flatten_destructuring_assignment(visitor, context, node, needs_value, level, callback)
            .expect("no storage failure")
    })
}

fn binding(
    fixture: &mut Fixture,
    node: NodeId,
    rval: Option<NodeId>,
    level: FlattenLevel,
    hoist_temp_variables: bool,
    skip_initializer: bool,
) -> Run {
    run(fixture, |visitor, context| {
        flatten_destructuring_binding(
            visitor,
            context,
            node,
            rval,
            level,
            hoist_temp_variables,
            skip_initializer,
        )
        .expect("no storage failure")
    })
}

/// Pre-order child collection over the factory, for factory-built trees.
struct Children<'a> {
    fixture: &'a Fixture,
    nodes: Vec<NodeId>,
}
impl ChildVisitor for Children<'_> {
    fn visit_node(&mut self, node: NodeId) -> ControlFlow<()> {
        self.nodes.push(node);
        ControlFlow::Continue(())
    }
    fn visit_list(&mut self, list: NodeListId) -> ControlFlow<()> {
        self.visit_node_slice(self.fixture.factory.read_list(list).nodes())
    }
    fn visit_node_slice(&mut self, nodes: NodeSlice) -> ControlFlow<()> {
        self.nodes
            .extend(self.fixture.factory.read_nodes(nodes).iter().flatten());
        ControlFlow::Continue(())
    }
}

/// Renders trees as source-like text; temporaries are named by creation
/// order across every root of one renderer.
struct Renderer<'a> {
    fixture: &'a Fixture,
    temps: Vec<u32>,
}

impl<'a> Renderer<'a> {
    fn new(fixture: &'a Fixture, roots: &[NodeId]) -> Self {
        let mut temps = Vec::new();
        let mut stack = roots.to_vec();
        while let Some(node) = stack.pop() {
            if let Some(info) = fixture.context.auto_generate_info(node) {
                temps.push(info.id.get());
            }
            let mut children = Children {
                fixture,
                nodes: Vec::new(),
            };
            let _ = Factory::node(&fixture.factory, node).for_each_child(&mut children);
            stack.extend(children.nodes);
        }
        temps.sort_unstable();
        temps.dedup();
        Self { fixture, temps }
    }

    fn read(&self, node: NodeId) -> tsr_ast::NodeRead<'_> {
        Factory::node(&self.fixture.factory, node)
    }

    fn items(&self, list: Option<NodeListId>, separator: &str) -> String {
        self.fixture
            .list(list)
            .into_iter()
            .map(|node| node.map_or(String::new(), |node| self.render(node, false)))
            .collect::<Vec<_>>()
            .join(separator)
    }

    fn optional(&self, node: Option<NodeId>, prefix: &str) -> String {
        node.map_or(String::new(), |node| {
            format!("{prefix}{}", self.render(node, true))
        })
    }

    fn comma_elements(&self, node: NodeId, out: &mut Vec<String>) {
        let read = self.read(node);
        if let Some(data) = read.as_binary_expression() {
            if self.fixture.kind(data.operator_token().unwrap()) == K::CommaToken {
                let (left, right) = (data.left().unwrap(), data.right().unwrap());
                drop(read);
                self.comma_elements(left, out);
                self.comma_elements(right, out);
                return;
            }
        }
        out.push(self.render(node, false));
    }

    fn render(&self, node: NodeId, nested: bool) -> String {
        let kind = self.fixture.kind(node);
        let read = self.read(node);
        let wrap = |text: String| if nested { format!("({text})") } else { text };
        match kind {
            K::Identifier => match self.fixture.context.auto_generate_info(node) {
                Some(info) => {
                    let rank = self.temps.iter().position(|&id| id == info.id.get());
                    format!("_{}", char::from(b'a' + rank.expect("collected") as u8))
                }
                None => String::from_utf8(self.fixture.text_of(node)).unwrap(),
            },
            K::NumericLiteral => {
                String::from_utf8(read.as_numeric_literal().unwrap().text().to_vec()).unwrap()
            }
            K::StringLiteral => format!(
                "{:?}",
                String::from_utf8(read.as_string_literal().unwrap().text().to_vec()).unwrap()
            ),
            K::TrueKeyword => "true".to_owned(),
            K::OmittedExpression => String::new(),
            K::BinaryExpression => {
                let data = read.as_binary_expression().unwrap();
                let operator = match self.fixture.kind(data.operator_token().unwrap()) {
                    K::CommaToken => {
                        drop(read);
                        let mut elements = Vec::new();
                        self.comma_elements(node, &mut elements);
                        return wrap(elements.join(", "));
                    }
                    K::EqualsToken => "=",
                    K::EqualsEqualsEqualsToken => "===",
                    K::PlusToken => "+",
                    other => panic!("operator {other:?}"),
                };
                wrap(format!(
                    "{} {operator} {}",
                    self.render(data.left().unwrap(), true),
                    self.render(data.right().unwrap(), true)
                ))
            }
            K::ConditionalExpression => {
                let data = read.as_conditional_expression().unwrap();
                wrap(format!(
                    "{} ? {} : {}",
                    self.render(data.condition().unwrap(), true),
                    self.render(data.when_true().unwrap(), true),
                    self.render(data.when_false().unwrap(), true)
                ))
            }
            K::VoidExpression => format!("void {}", self.render(read.expression().unwrap(), true)),
            K::TypeOfExpression => {
                format!("typeof {}", self.render(read.expression().unwrap(), true))
            }
            K::ParenthesizedExpression => {
                format!("({})", self.render(read.expression().unwrap(), false))
            }
            K::PropertyAccessExpression => format!(
                "{}.{}",
                self.render(read.expression().unwrap(), true),
                self.render(read.name().unwrap(), false)
            ),
            K::ElementAccessExpression => format!(
                "{}[{}]",
                self.render(read.expression().unwrap(), true),
                self.render(
                    read.as_element_access_expression()
                        .unwrap()
                        .argument_expression()
                        .unwrap(),
                    false
                )
            ),
            K::CallExpression => format!(
                "{}({})",
                self.render(read.expression().unwrap(), true),
                self.items(read.argument_list(), ", ")
            ),
            K::ArrayLiteralExpression => format!(
                "[{}]",
                self.items(read.as_array_literal_expression().unwrap().elements(), ", ")
            ),
            K::ObjectLiteralExpression => format!(
                "{{{}}}",
                self.items(
                    read.as_object_literal_expression().unwrap().properties(),
                    ", "
                )
            ),
            K::ShorthandPropertyAssignment => {
                let data = read.as_shorthand_property_assignment().unwrap();
                format!(
                    "{}{}",
                    self.render(data.name().unwrap(), false),
                    self.optional(data.object_assignment_initializer(), " = ")
                )
            }
            K::PropertyAssignment => format!(
                "{}: {}",
                self.render(read.name().unwrap(), false),
                self.render(read.initializer().unwrap(), true)
            ),
            K::SpreadAssignment | K::SpreadElement => {
                format!("...{}", self.render(read.expression().unwrap(), true))
            }
            K::ComputedPropertyName => {
                format!("[{}]", self.render(read.expression().unwrap(), false))
            }
            K::VariableDeclaration => format!(
                "{}{}",
                self.render(read.name().unwrap(), false),
                self.optional(read.initializer(), " = ")
            ),
            K::ObjectBindingPattern => format!(
                "{{{}}}",
                self.items(read.as_binding_pattern().unwrap().elements(), ", ")
            ),
            K::ArrayBindingPattern => format!(
                "[{}]",
                self.items(read.as_binding_pattern().unwrap().elements(), ", ")
            ),
            K::BindingElement => {
                let data = read.as_binding_element().unwrap();
                format!(
                    "{}{}{}{}",
                    if data.dot_dot_dot_token().is_some() {
                        "..."
                    } else {
                        ""
                    },
                    data.property_name().map_or(String::new(), |name| format!(
                        "{}: ",
                        self.render(name, false)
                    )),
                    data.name()
                        .map_or(String::new(), |name| self.render(name, false)),
                    self.optional(data.initializer(), " = ")
                )
            }
            K::SyntaxList => {
                let children = read.as_syntax_list().unwrap().children();
                drop(read);
                self.fixture
                    .factory
                    .read_nodes(children)
                    .iter()
                    .map(|child| self.render(child.unwrap(), false))
                    .collect::<Vec<_>>()
                    .join("; ")
            }
            other => format!("<{other:?}>"),
        }
    }
}

/// The result and the hoisted temporaries, rendered together.
fn render(fixture: &Fixture, run: &Run) -> (String, Vec<String>) {
    let mut roots = run.hoisted.clone();
    roots.extend(run.result);
    let renderer = Renderer::new(fixture, &roots);
    (
        run.result
            .map_or("<nil>".to_owned(), |node| renderer.render(node, false)),
        run.hoisted
            .iter()
            .map(|&temp| renderer.render(temp, false))
            .collect(),
    )
}

#[test]
fn an_object_assignment_flattens_to_property_reads() {
    let mut fixture = parse("({ a, b: c } = obj);");
    let node = fixture.first(K::BinaryExpression);
    let out = assignment(&mut fixture, node, false, FlattenLevel::All, None);
    assert_eq!(
        render(&fixture, &out),
        ("a = obj.a, c = obj.b".to_owned(), vec![])
    );
    // Each assignment keeps its element as original and location.
    let shorthand = fixture.first(K::ShorthandPropertyAssignment);
    let read = Factory::node(&fixture.factory, out.result.unwrap());
    let first = read.as_binary_expression().unwrap().left().unwrap();
    drop(read);
    assert_eq!(fixture.context.original(first), Some(shorthand));
    assert_eq!(fixture.range(first), fixture.range(shorthand));

    // With the value needed, the identifier is reused and appended.
    let mut fixture = parse("({ a, b: c } = obj);");
    let node = fixture.first(K::BinaryExpression);
    let out = assignment(&mut fixture, node, true, FlattenLevel::All, None);
    assert_eq!(
        render(&fixture, &out),
        ("a = obj.a, c = obj.b, obj".to_owned(), vec![])
    );

    // A non-identifier value is cached in a hoisted temporary.
    let mut fixture = parse("({ a, b } = f());");
    let node = fixture.first(K::BinaryExpression);
    let out = assignment(&mut fixture, node, false, FlattenLevel::All, None);
    assert_eq!(
        render(&fixture, &out),
        (
            "_a = f(), a = _a.a, b = _a.b".to_owned(),
            vec!["_a".to_owned()]
        )
    );
}

#[test]
fn an_array_assignment_flattens_defaults_nested_patterns_and_rest() {
    let mut fixture = parse("[a = 1, [b], ...c] = arr;");
    let node = fixture.first(K::BinaryExpression);
    let out = assignment(&mut fixture, node, false, FlattenLevel::All, None);
    assert_eq!(
        render(&fixture, &out),
        (
            "_a = arr[0], a = ((_a === void 0) ? 1 : _a), b = arr[1][0], c = arr.slice(2)"
                .to_owned(),
            vec!["_a".to_owned()]
        )
    );

    // Omitted elements are skipped without a read.
    let mut fixture = parse("[, a] = arr;");
    let node = fixture.first(K::BinaryExpression);
    let out = assignment(&mut fixture, node, false, FlattenLevel::All, None);
    assert_eq!(render(&fixture, &out), ("a = arr[1]".to_owned(), vec![]));

    // A default that is not simply copiable, on a nested pattern, is cached.
    let mut fixture = parse("[[a] = g()] = arr;");
    let node = fixture.first(K::BinaryExpression);
    let out = assignment(&mut fixture, node, false, FlattenLevel::All, None);
    assert_eq!(
        render(&fixture, &out),
        (
            "_a = arr[0], _b = ((_a === void 0) ? g() : _a), a = _b[0]".to_owned(),
            vec!["_a".to_owned(), "_b".to_owned()]
        )
    );
}

#[test]
fn computed_names_are_cached_and_passed_to_the_rest_helper() {
    let mut fixture = parse("({ [k]: x, ...rest } = obj);");
    let node = fixture.first(K::BinaryExpression);
    let out = assignment(&mut fixture, node, false, FlattenLevel::All, None);
    assert_eq!(
        render(&fixture, &out),
        (
            "_a = obj, _b = k, x = _a[_b], rest = __rest(_a, [(typeof _b === \"symbol\") ? _b : (_b + \"\")])"
                .to_owned(),
            vec!["_a".to_owned(), "_b".to_owned()]
        )
    );
    let helpers = fixture.context.clone().read_emit_helpers();
    assert_eq!(helpers.len(), 1);

    // Literal names are cloned into element accesses, others become names.
    let mut fixture = parse("({ \"s\": a, 1: b, [\"t\"]: c, d: e } = obj);");
    let node = fixture.first(K::BinaryExpression);
    let out = assignment(&mut fixture, node, false, FlattenLevel::All, None);
    assert_eq!(
        render(&fixture, &out),
        (
            "a = obj[\"s\"], b = obj[1], c = obj[\"t\"], e = obj.d".to_owned(),
            vec![]
        )
    );
}

#[test]
fn a_value_that_a_target_reassigns_is_cached_first() {
    let mut fixture = parse("({ a: obj } = obj);");
    let node = fixture.first(K::BinaryExpression);
    let out = assignment(&mut fixture, node, false, FlattenLevel::All, None);
    // One element: the cached value is used as it is.
    assert_eq!(
        render(&fixture, &out),
        ("_a = obj, obj = _a.a".to_owned(), vec!["_a".to_owned()])
    );
}

#[test]
fn empty_patterns_are_skipped_through_nested_assignments() {
    let mut fixture = parse("({} = [] = x);");
    let node = fixture.first(K::BinaryExpression);
    let out = assignment(&mut fixture, node, false, FlattenLevel::All, None);
    assert_eq!(out.result, Some(fixture.identifier("x")));
    assert!(out.hoisted.is_empty());

    let mut fixture = parse("({} = { a } = obj);");
    let node = fixture.first(K::BinaryExpression);
    let out = assignment(&mut fixture, node, false, FlattenLevel::All, None);
    assert_eq!(render(&fixture, &out), ("a = obj.a".to_owned(), vec![]));
}

#[test]
fn a_synthesized_assignment_takes_its_value_location() {
    let mut fixture = parse("({ a, b } = f());");
    let parsed = fixture.first(K::BinaryExpression);
    let (left, right) = {
        let read = Factory::node(&fixture.factory, parsed);
        let data = read.as_binary_expression().unwrap();
        (data.left(), data.right())
    };
    let token = fixture.factory.new_token(K::EqualsToken.into());
    let synthesized = fixture
        .factory
        .new_binary_expression(None, left, None, Some(token), right);
    let out = assignment(&mut fixture, synthesized, false, FlattenLevel::All, None);
    let cache = {
        let read = Factory::node(&fixture.factory, out.result.unwrap());
        let mut node = read.as_binary_expression().unwrap().left().unwrap();
        drop(read);
        // The leftmost element of the comma list.
        while fixture.kind(node) == K::BinaryExpression {
            let read = Factory::node(&fixture.factory, node);
            let data = read.as_binary_expression().unwrap();
            if fixture.kind(data.operator_token().unwrap()) != K::CommaToken {
                break;
            }
            node = data.left().unwrap();
        }
        node
    };
    assert_eq!(fixture.range(cache), fixture.range(right.unwrap()));

    // A parsed assignment keeps its own location.
    let mut fixture = parse("({ a, b } = f());");
    let parsed = fixture.first(K::BinaryExpression);
    let out = assignment(&mut fixture, parsed, false, FlattenLevel::All, None);
    let read = Factory::node(&fixture.factory, out.result.unwrap());
    let mut node = read.as_binary_expression().unwrap().left().unwrap();
    drop(read);
    while fixture.kind(node) == K::BinaryExpression {
        let read = Factory::node(&fixture.factory, node);
        let data = read.as_binary_expression().unwrap();
        if fixture.kind(data.operator_token().unwrap()) != K::CommaToken {
            break;
        }
        node = data.left().unwrap();
    }
    assert_eq!(fixture.range(node), fixture.range(parsed));
}

#[test]
fn a_callback_creates_the_assignments_to_identifiers() {
    let mut fixture = parse("({ a, b: c.d } = obj);");
    let node = fixture.first(K::BinaryExpression);
    let calls = RefCell::new(Vec::new());
    let context = fixture.context.clone();
    let callback = |visitor: &mut NodeVisitor<'_>,
                    name: NodeId,
                    value: NodeId,
                    location: Option<TextRange>|
     -> NodeId {
        calls.borrow_mut().push((name, value, location));
        let factory = visitor.factory_mut();
        let exports = factory.new_identifier(JsString::from_bytes(&b"exports"[..]));
        let name = tsr_ast::clone_node(factory, name);
        let access = factory.new_property_access_expression(
            Some(exports),
            None,
            Some(name),
            node_flags::NONE,
        );
        context.new_assignment_expression(factory, access, value)
    };
    let out = assignment(
        &mut fixture,
        node,
        false,
        FlattenLevel::All,
        Some(&callback),
    );
    assert_eq!(
        render(&fixture, &out),
        ("exports.a = obj.a, c.d = obj.b".to_owned(), vec![])
    );
    let shorthand = fixture.first(K::ShorthandPropertyAssignment);
    let calls = calls.borrow();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].0, fixture.identifier("a"));
    assert_eq!(calls[0].2, Some(fixture.range(shorthand)));
    // The callback's result takes the element as original.
    let read = Factory::node(&fixture.factory, out.result.unwrap());
    let first = read.as_binary_expression().unwrap().left().unwrap();
    drop(read);
    assert_eq!(fixture.context.original(first), Some(shorthand));
}

#[test]
fn a_variable_declaration_flattens_to_assignments() {
    let mut fixture = parse("let { a, b = 2 } = o;");
    let node = fixture.first(K::VariableDeclaration);
    let out = assignment(&mut fixture, node, false, FlattenLevel::All, None);
    assert_eq!(
        render(&fixture, &out),
        (
            "a = o.a, _a = o.b, b = ((_a === void 0) ? 2 : _a)".to_owned(),
            vec!["_a".to_owned()]
        )
    );
}

#[test]
fn object_rest_assignments_keep_the_leading_properties_as_a_pattern() {
    let mut fixture = parse("({ a, ...r } = o);");
    let node = fixture.first(K::BinaryExpression);
    let out = assignment(&mut fixture, node, false, FlattenLevel::ObjectRest, None);
    assert_eq!(
        render(&fixture, &out),
        ("{a} = o, r = __rest(o, [\"a\"])".to_owned(), vec![])
    );
    // The same at FlattenLevelAll decomposes every property.
    let mut fixture = parse("({ a, ...r } = o);");
    let node = fixture.first(K::BinaryExpression);
    let out = assignment(&mut fixture, node, false, FlattenLevel::All, None);
    assert_eq!(
        render(&fixture, &out),
        ("a = o.a, r = __rest(o, [\"a\"])".to_owned(), vec![])
    );

    // An array of holes alone is cached before the pattern is kept.
    let mut fixture = parse("[, ,] = f();");
    let node = fixture.first(K::BinaryExpression);
    let out = assignment(&mut fixture, node, false, FlattenLevel::ObjectRest, None);
    assert_eq!(
        render(&fixture, &out),
        ("_a = f(), [, ] = _a".to_owned(), vec!["_a".to_owned()])
    );
}

#[test]
fn bindings_flatten_to_declarations_with_unhoisted_temporaries() {
    let mut fixture = parse("let { a, b: { c }, d = f() } = o;");
    let node = fixture.first(K::VariableDeclaration);
    let out = binding(&mut fixture, node, None, FlattenLevel::All, false, false);
    assert_eq!(
        render(&fixture, &out),
        (
            "a = o.a; c = o.b.c; _a = o.d; d = ((_a === void 0) ? f() : _a)".to_owned(),
            vec![]
        )
    );
    // Declarations keep their element as original, temporaries none.
    let read = Factory::node(&fixture.factory, out.result.unwrap());
    let children = read.as_syntax_list().unwrap().children();
    drop(read);
    let declarations: Vec<NodeId> = fixture
        .factory
        .read_nodes(children)
        .iter()
        .flatten()
        .collect();
    let first_element = fixture.first(K::BindingElement);
    assert_eq!(
        fixture.context.original(declarations[0]),
        Some(first_element)
    );
    assert_eq!(fixture.range(declarations[0]), fixture.range(first_element));
    assert_eq!(fixture.context.original(declarations[2]), None);

    // One declaration is returned as it is.
    let mut fixture = parse("let [, a] = arr;");
    let node = fixture.first(K::VariableDeclaration);
    let out = binding(&mut fixture, node, None, FlattenLevel::All, false, false);
    assert_eq!(fixture.kind(out.result.unwrap()), K::VariableDeclaration);
    assert_eq!(render(&fixture, &out), ("a = arr[1]".to_owned(), vec![]));
}

#[test]
fn bindings_with_hoisted_temporaries_inline_pending_expressions() {
    // A non-literal computed name: the initializer and the name are cached
    // in hoisted temporaries and inlined into the next declaration.
    let mut fixture = parse("let { [k]: x } = o;");
    let node = fixture.first(K::VariableDeclaration);
    let out = binding(&mut fixture, node, None, FlattenLevel::All, true, false);
    assert_eq!(
        render(&fixture, &out),
        (
            "x = (_a = o, _b = k, _a[_b])".to_owned(),
            vec!["_a".to_owned(), "_b".to_owned()]
        )
    );

    // Expressions left after the last binding go to a new temporary,
    // declared with an empty location and no original.
    let mut fixture = parse("let {} = f();");
    let node = fixture.first(K::VariableDeclaration);
    let out = binding(&mut fixture, node, None, FlattenLevel::All, true, false);
    assert_eq!(
        render(&fixture, &out),
        ("_b = (_a = f())".to_owned(), vec!["_a".to_owned()])
    );
    let declaration = out.result.unwrap();
    assert_eq!(fixture.range(declaration), TextRange::new(0, 0));
    assert_eq!(fixture.context.original(declaration), None);

    // Without hoisting the temporary is a declaration of its own.
    let mut fixture = parse("let {} = f();");
    let node = fixture.first(K::VariableDeclaration);
    let out = binding(&mut fixture, node, None, FlattenLevel::All, false, false);
    assert_eq!(render(&fixture, &out), ("_a = f()".to_owned(), vec![]));
}

#[test]
fn object_rest_bindings_keep_leading_elements_and_split_nested_rests() {
    let mut fixture = parse("let { a, ...rest } = o;");
    let node = fixture.first(K::VariableDeclaration);
    let out = binding(
        &mut fixture,
        node,
        None,
        FlattenLevel::ObjectRest,
        false,
        false,
    );
    assert_eq!(
        render(&fixture, &out),
        ("{a} = o; rest = __rest(o, [\"a\"])".to_owned(), vec![])
    );
    let read = Factory::node(&fixture.factory, out.result.unwrap());
    let children = read.as_syntax_list().unwrap().children();
    drop(read);
    let first = fixture.factory.read_nodes(children).at(0).unwrap();
    let pattern = fixture.first(K::ObjectBindingPattern);
    assert_eq!(fixture.context.original(first), Some(pattern));

    let mut fixture = parse("let [a, { b, ...r }] = arr;");
    let node = fixture.first(K::VariableDeclaration);
    let out = binding(
        &mut fixture,
        node,
        None,
        FlattenLevel::ObjectRest,
        false,
        false,
    );
    assert_eq!(
        render(&fixture, &out),
        (
            "[a, _a] = arr; {b} = _a; r = __rest(_a, [\"b\"])".to_owned(),
            vec![]
        )
    );

    // Once an element is transformed, later non-simple elements are too.
    let mut fixture = parse("let [{ ...a }, b = f()] = arr;");
    let node = fixture.first(K::VariableDeclaration);
    let out = binding(
        &mut fixture,
        node,
        None,
        FlattenLevel::ObjectRest,
        true,
        false,
    );
    assert_eq!(
        render(&fixture, &out),
        (
            "[_a, _b] = arr; a = __rest(_a, []); b = ((_b === void 0) ? f() : _b)".to_owned(),
            vec!["_a".to_owned(), "_b".to_owned()]
        )
    );
    let mut fixture = parse("let [{ ...a }, b = 1] = arr;");
    let node = fixture.first(K::VariableDeclaration);
    let out = binding(
        &mut fixture,
        node,
        None,
        FlattenLevel::ObjectRest,
        false,
        false,
    );
    assert_eq!(
        render(&fixture, &out),
        ("[_a, b = 1] = arr; a = __rest(_a, [])".to_owned(), vec![])
    );
}

#[test]
fn parameters_flatten_from_a_value_with_or_without_their_initializer() {
    let mut fixture = parse("function f({ a, ...r }) {} _p;");
    let parameter = fixture.first(K::Parameter);
    let rval = fixture.identifier("_p");
    let out = binding(
        &mut fixture,
        parameter,
        Some(rval),
        FlattenLevel::ObjectRest,
        false,
        true,
    );
    assert_eq!(
        render(&fixture, &out),
        ("{a} = _p; r = __rest(_p, [\"a\"])".to_owned(), vec![])
    );

    let mut fixture = parse("function f({ a } = d) {} _p;");
    let parameter = fixture.first(K::Parameter);
    let rval = fixture.identifier("_p");
    let out = binding(
        &mut fixture,
        parameter,
        Some(rval),
        FlattenLevel::All,
        false,
        true,
    );
    assert_eq!(render(&fixture, &out), ("a = _p.a".to_owned(), vec![]));

    let mut fixture = parse("function f({ a } = d) {} _p;");
    let parameter = fixture.first(K::Parameter);
    let rval = fixture.identifier("_p");
    let out = binding(
        &mut fixture,
        parameter,
        Some(rval),
        FlattenLevel::All,
        false,
        false,
    );
    assert_eq!(
        render(&fixture, &out),
        ("a = ((_p === void 0) ? d : _p).a".to_owned(), vec![])
    );

    let mut fixture = parse("function f({ a } = g()) {} _p;");
    let parameter = fixture.first(K::Parameter);
    let rval = fixture.identifier("_p");
    let out = binding(
        &mut fixture,
        parameter,
        Some(rval),
        FlattenLevel::All,
        false,
        false,
    );
    assert_eq!(
        render(&fixture, &out),
        (
            "_a = ((_p === void 0) ? g() : _p); a = _a.a".to_owned(),
            vec![]
        )
    );
}

#[test]
fn assigned_names_and_computed_names_are_found_through_nested_patterns() {
    let fixture = parse("let { a, b: [c, { d }] } = x; ({ e: y.z } = x);");
    let view = fixture.factory.view();
    let declaration = fixture.first(K::VariableDeclaration);
    for (name, expected) in [
        ("a", true),
        ("c", true),
        ("d", true),
        ("b", false),
        ("x", false),
    ] {
        assert_eq!(
            binding_or_assignment_element_assigns_to_name(view, declaration, name.as_bytes())
                .unwrap(),
            expected,
            "{name}"
        );
    }
    // A property access target assigns to no name.
    let assignment = fixture.first(K::BinaryExpression);
    for name in ["y", "z", "e"] {
        assert!(
            !binding_or_assignment_element_assigns_to_name(view, assignment, name.as_bytes())
                .unwrap()
        );
    }

    let cases = [
        ("let { [k]: a } = x;", true),
        ("let { [\"k\"]: a } = x;", false),
        ("let { [1]: a } = x;", false),
        ("let { a: { [k]: b } } = x;", true),
        ("let [{ [k]: b }] = x;", true),
        ("let { a } = x;", false),
    ];
    for (text, expected) in cases {
        let fixture = parse(text);
        let declaration = fixture.first(K::VariableDeclaration);
        assert_eq!(
            binding_or_assignment_element_contains_non_literal_computed_name(
                fixture.factory.view(),
                declaration
            )
            .unwrap(),
            expected,
            "{text}"
        );
    }
    let fixture = parse("({ [k]: a } = x);");
    let assignment = fixture.first(K::BinaryExpression);
    assert!(
        binding_or_assignment_element_contains_non_literal_computed_name(
            fixture.factory.view(),
            assignment
        )
        .unwrap()
    );
}

#[test]
fn initializers_of_every_element_shape() {
    let mut fixture = parse(
        "let v = 1; let { e = 2 } = x; ({ p: q = 3 } = x); ({ r: s } = x); ({ t = 4 } = x); [u = 5] = x; [...w] = x; function f(g = 6) {}",
    );
    let initializer = |fixture: &Fixture, node: Option<NodeId>| {
        get_initializer_of_binding_or_assignment_element(fixture.factory.view(), node).unwrap()
    };
    let literal = |fixture: &Fixture, node: Option<NodeId>| {
        node.map(|node| {
            String::from_utf8(
                Factory::node(&fixture.factory, node)
                    .as_numeric_literal()
                    .expect("a numeric literal")
                    .text()
                    .to_vec(),
            )
            .unwrap()
        })
    };
    assert_eq!(initializer(&fixture, None), None);
    let declaration = fixture.first(K::VariableDeclaration);
    assert_eq!(
        literal(&fixture, initializer(&fixture, Some(declaration))),
        Some("1".into())
    );
    let element = fixture.first(K::BindingElement);
    assert_eq!(
        literal(&fixture, initializer(&fixture, Some(element))),
        Some("2".into())
    );
    let with_default = fixture.nth(K::PropertyAssignment, 0);
    assert_eq!(
        literal(&fixture, initializer(&fixture, Some(with_default))),
        Some("3".into())
    );
    let without_default = fixture.nth(K::PropertyAssignment, 1);
    assert_eq!(initializer(&fixture, Some(without_default)), None);
    let shorthand = fixture.first(K::ShorthandPropertyAssignment);
    assert_eq!(
        literal(&fixture, initializer(&fixture, Some(shorthand))),
        Some("4".into())
    );
    let array = fixture.nth(K::ArrayLiteralExpression, 0);
    let first = fixture.list(
        Factory::node(&fixture.factory, array)
            .as_array_literal_expression()
            .unwrap()
            .elements(),
    )[0];
    assert_eq!(
        literal(&fixture, initializer(&fixture, first)),
        Some("5".into())
    );
    let spread = fixture.first(K::SpreadElement);
    assert_eq!(initializer(&fixture, Some(spread)), None);
    let parameter = fixture.first(K::Parameter);
    assert_eq!(
        literal(&fixture, initializer(&fixture, Some(parameter))),
        Some("6".into())
    );

    // A spread of an assignment reads the assignment's default.
    let assignment = Factory::node(&fixture.factory, with_default)
        .initializer()
        .unwrap();
    let spread = fixture.factory.new_spread_element(Some(assignment));
    assert_eq!(
        literal(&fixture, initializer(&fixture, Some(spread))),
        Some("3".into())
    );
}
