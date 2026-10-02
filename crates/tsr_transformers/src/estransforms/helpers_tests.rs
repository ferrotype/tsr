//! The shared helpers of `estransforms` (`utilities.go`, `namedevaluation.go`,
//! `classthis.go`) against the pin. Upstream has no tests for them; the
//! expectations come from an in-package Go test run as an overlay
//! (`tests/fixtures/estransforms_helpers_test.go`), which runs the same
//! scenarios over the same parsed inputs and prints each result with
//! `printer.NewPrinter`. Each scenario below is that test's twin.
use super::classthis::is_class_this_assignment_block;
use super::namedevaluation::{
    class_has_declared_or_explicitly_assigned_name, class_has_explicitly_assigned_name,
    create_class_named_evaluation_helper_block, finish_transform_named_evaluation,
    get_assigned_name_of_identifier, get_assigned_name_of_property_name,
    inject_class_named_evaluation_helper_block_if_missing, is_anonymous_function_definition,
    is_class_named_evaluation_helper_block, is_named_evaluation, is_named_evaluation_and,
    transform_named_evaluation,
};
use super::utilities::{
    convert_class_declaration_to_class_expression, create_accessor_property_backing_field,
    create_not_null_condition, list_nodes, SuperAccessState,
};
use crate::extract_modifiers;
use std::ops::ControlFlow;
use std::panic::{catch_unwind, AssertUnwindSafe};
use tsr_arena::Counters;
use tsr_ast::{
    modifier_flags, token_flags, AstBuilder, ChildVisitor, Factory, FactoryMethods, JsString,
    NodeId, NodeListId, NodeSlice, RuntimeFactory, SourceFileParseOptions, SyntaxKind as K,
};
use tsr_core::{collections::OrderedSet, NewLineKind, ScriptKind};
use tsr_jsstring::SourceText;
use tsr_printer::{emit_flags, EmitContext, Printer, PrinterOptions};

const FIXTURE: &str = include_str!("../../tests/fixtures/estransforms_helpers.json");

struct Case {
    id: String,
    scenario: String,
    source: String,
    name: String,
    flag: bool,
    this: String,
    mode: String,
    expected: String,
}

fn cases() -> Vec<Case> {
    let document: serde_json::Value = serde_json::from_str(FIXTURE).expect("fixture JSON");
    let text = |case: &serde_json::Value, key: &str| {
        case[key]
            .as_str()
            .unwrap_or_else(|| panic!("{key} is a string"))
            .to_owned()
    };
    document["cases"]
        .as_array()
        .expect("cases")
        .iter()
        .map(|case| Case {
            id: text(case, "id"),
            scenario: text(case, "scenario"),
            source: text(case, "source"),
            name: text(case, "name"),
            flag: case["flag"].as_bool().expect("flag is a bool"),
            this: text(case, "this"),
            mode: text(case, "mode"),
            expected: text(case, "expected"),
        })
        .collect()
}

struct Run {
    ec: EmitContext,
    ast: AstBuilder,
    file: NodeId,
    out: Vec<String>,
}

fn js(text: &str) -> JsString {
    JsString::from_bytes(text.as_bytes())
}

fn yes(value: bool) -> &'static str {
    if value {
        "true"
    } else {
        "false"
    }
}

/// Go's `node.ForEachChild` in order, lists element by element.
fn children(factory: &dyn RuntimeFactory, node: NodeId) -> Vec<NodeId> {
    struct Children<'a> {
        factory: &'a dyn RuntimeFactory,
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
    let mut visitor = Children {
        factory,
        nodes: Vec::new(),
    };
    let _ = factory.node(node).for_each_child(&mut visitor);
    visitor.nodes
}

fn preorder(factory: &dyn RuntimeFactory, node: NodeId, nodes: &mut Vec<NodeId>) {
    nodes.push(node);
    for child in children(factory, node) {
        preorder(factory, child, nodes);
    }
}

fn is_class_like(factory: &dyn RuntimeFactory, node: NodeId) -> bool {
    matches!(
        factory.node(node).kind().known(),
        Some(K::ClassDeclaration | K::ClassExpression)
    )
}

fn identifier_text(factory: &dyn RuntimeFactory, node: NodeId) -> Option<Vec<u8>> {
    Some(factory.node(node).as_identifier()?.text().to_vec())
}

fn is_named_evaluation_kind(factory: &dyn RuntimeFactory, node: NodeId) -> bool {
    matches!(
        factory.node(node).kind().known(),
        Some(
            K::PropertyAssignment
                | K::ShorthandPropertyAssignment
                | K::VariableDeclaration
                | K::Parameter
                | K::BindingElement
                | K::PropertyDeclaration
                | K::BinaryExpression
                | K::ExportAssignment
        )
    )
}

fn is_class_expression(factory: &dyn RuntimeFactory, node: NodeId) -> bool {
    factory.node(node).kind() == K::ClassExpression
}

impl Run {
    fn parse(source: &str) -> Self {
        let ec = EmitContext::new();
        let mut ast =
            AstBuilder::with_hooks(SourceText::default(), &Counters::new(), ec.factory_hooks());
        let parsed = tsr_parser::parse_source_file(
            SourceText::from_loaded_bytes(source.as_bytes()),
            ScriptKind::TS,
            SourceFileParseOptions {
                file_name: js("/main.ts"),
                path: js("/main.ts"),
                ..Default::default()
            },
        );
        let file = parsed.root();
        ast.retain_file(parsed.publish_unbound());
        Self {
            ec,
            ast,
            file,
            out: Vec::new(),
        }
    }

    fn print(&self, node: NodeId) -> String {
        let mut printer = Printer::new(
            PrinterOptions {
                new_line: NewLineKind::LF,
                ..PrinterOptions::default()
            },
            &self.ec,
        );
        let text = printer
            .emit(self.ast.view(), node, Some(self.file))
            .unwrap_or_else(|error| panic!("printer: {error:?}"));
        String::from_utf8(text).expect("UTF-8 output")
    }

    fn line(&mut self, text: String) {
        self.out.push(text);
    }

    fn all(&self) -> Vec<NodeId> {
        let mut nodes = Vec::new();
        preorder(&self.ast, self.file, &mut nodes);
        nodes
    }

    fn statements(&self) -> Vec<NodeId> {
        list_nodes(&self.ast, self.ast.node(self.file).statement_list())
    }

    fn top_level_expressions(&self) -> Vec<NodeId> {
        self.statements()
            .into_iter()
            .filter(|&statement| self.ast.node(statement).kind() == K::ExpressionStatement)
            .map(|statement| self.ast.node(statement).expression().expect("expression"))
            .collect()
    }

    fn string_literal(&mut self, text: &str) -> NodeId {
        self.ast.new_string_literal(js(text), token_flags::NONE)
    }

    /// The Go test's `prepare`: marks the parsed static blocks the way the
    /// class transforms would have produced them.
    fn prepare(&mut self) {
        for node in self.all() {
            let read = self.ast.node(node);
            if read.kind() != K::ClassStaticBlockDeclaration {
                continue;
            }
            let body = read
                .as_class_static_block_declaration()
                .expect("static block")
                .body()
                .expect("body");
            let class = read.parent().expect("parent");
            drop(read);
            let statements = list_nodes(&self.ast, self.ast.node(body).statement_list());
            if statements.len() != 1
                || self.ast.node(statements[0]).kind() != K::ExpressionStatement
            {
                continue;
            }
            let expression = self
                .ast
                .node(statements[0])
                .expression()
                .expect("expression");
            let read = self.ast.node(expression);
            if read.kind() == K::CallExpression {
                let callee = read.expression().expect("callee");
                let arguments = read.argument_list();
                drop(read);
                if let Some(callee_text) = identifier_text(&self.ast, callee) {
                    let args = list_nodes(&self.ast, arguments);
                    match &callee_text[..] {
                        b"__setFunctionName" => {
                            if !(args.len() >= 3
                                && identifier_text(&self.ast, args[2]).as_deref()
                                    == Some(&b"noflag"[..]))
                            {
                                self.ec.add_emit_flags(callee, emit_flags::HELPER_NAME);
                            }
                            let target = if args.len() >= 2 {
                                if identifier_text(&self.ast, args[1]).as_deref()
                                    == Some(&b"wrong"[..])
                                {
                                    args[0]
                                } else {
                                    args[1]
                                }
                            } else {
                                self.string_literal("x")
                            };
                            self.ec.set_assigned_name(node, target);
                            self.ec.set_assigned_name(class, target);
                        }
                        b"__assigned" => {
                            let name = self.string_literal("x");
                            self.ec.set_assigned_name(class, name);
                        }
                        _ => {}
                    }
                    continue;
                }
                continue;
            }
            if read.kind() == K::BinaryExpression {
                let left = read
                    .as_binary_expression()
                    .expect("binary")
                    .left()
                    .expect("left");
                drop(read);
                if let Some(text) = identifier_text(&self.ast, left) {
                    if text.starts_with(b"_classThis") {
                        self.ec.set_class_this(node, left);
                        self.ec.set_class_this(class, left);
                    } else if text.starts_with(b"_other") {
                        let other = self.ast.new_identifier(js("_other"));
                        self.ec.set_class_this(node, other);
                    }
                }
            }
        }
    }

    fn print_hoisted(&mut self) {
        for statement in self.ec.clone().end_variable_environment(&mut self.ast) {
            let text = self.print(statement);
            self.line(format!("hoisted: {text}"));
        }
    }

    fn this_expression(&mut self, case: &Case) -> Option<NodeId> {
        (!case.this.is_empty()).then(|| self.ast.new_identifier(js(&case.this)))
    }

    #[allow(clippy::too_many_lines)]
    fn run(&mut self, case: &Case) {
        self.prepare();
        let ec = self.ec.clone();
        let cb: &dyn Fn(&dyn RuntimeFactory, NodeId) -> bool = &is_class_expression;
        match case.scenario.as_str() {
            "classThisBlock" => {
                for node in self.all() {
                    if is_class_like(&self.ast, node) {
                        let flags: Vec<_> =
                            list_nodes(&self.ast, self.ast.node(node).member_list())
                                .into_iter()
                                .map(|member| {
                                    yes(is_class_this_assignment_block(&ec, &self.ast, member))
                                })
                                .collect();
                        self.line(flags.join(","));
                    }
                }
            }
            "helperBlock" => {
                for node in self.all() {
                    if is_class_like(&self.ast, node) {
                        let flags: Vec<_> =
                            list_nodes(&self.ast, self.ast.node(node).member_list())
                                .into_iter()
                                .map(|member| {
                                    yes(is_class_named_evaluation_helper_block(
                                        &ec, &self.ast, member,
                                    ))
                                })
                                .collect();
                        let explicit = class_has_explicitly_assigned_name(&ec, &self.ast, node);
                        let declared =
                            class_has_declared_or_explicitly_assigned_name(&ec, &self.ast, node);
                        self.line(format!(
                            "members={} explicit={} declared={}",
                            flags.join(","),
                            yes(explicit),
                            yes(declared)
                        ));
                    }
                }
            }
            "anonymous" => {
                for expression in self.top_level_expressions() {
                    let plain =
                        is_anonymous_function_definition(&ec, &self.ast, Some(expression), None);
                    let with_cb = is_anonymous_function_definition(
                        &ec,
                        &self.ast,
                        Some(expression),
                        Some(cb),
                    );
                    self.line(format!("{} {}", yes(plain), yes(with_cb)));
                }
            }
            "namedEvaluation" => {
                for node in self.all() {
                    if is_named_evaluation_kind(&self.ast, node) {
                        let kind = self.ast.node(node).kind();
                        let named = is_named_evaluation(&ec, &self.ast, node);
                        let and = is_named_evaluation_and(&ec, &self.ast, node, Some(cb));
                        self.line(format!("{kind} {} {}", yes(named), yes(and)));
                    }
                }
            }
            "transform" => {
                self.ec.start_variable_environment();
                if case.mode == "unhandled" {
                    let first = self.statements()[0];
                    let result = transform_named_evaluation(
                        &ec,
                        &mut self.ast,
                        first,
                        case.flag,
                        case.name.as_bytes(),
                    );
                    let text = self.print(result);
                    self.line(text);
                }
                for node in self.all() {
                    if is_named_evaluation_kind(&self.ast, node)
                        && is_named_evaluation(&ec, &self.ast, node)
                    {
                        let result = transform_named_evaluation(
                            &ec,
                            &mut self.ast,
                            node,
                            case.flag,
                            case.name.as_bytes(),
                        );
                        let text = self.print(result);
                        self.line(text);
                    }
                }
                self.print_hoisted();
            }
            "finish" => {
                for expression in self.top_level_expressions() {
                    // A partially emitted expression has no source form: the
                    // mode wraps each expression in one, as an earlier
                    // transform would.
                    let expression = if case.mode == "partiallyEmitted" {
                        self.ast.new_partially_emitted_expression(Some(expression))
                    } else {
                        expression
                    };
                    let name = self.string_literal(&case.name);
                    let result = finish_transform_named_evaluation(
                        &ec,
                        &mut self.ast,
                        Some(expression),
                        name,
                        case.flag,
                    )
                    .expect("a result");
                    let text = self.print(result);
                    self.line(text);
                }
            }
            "convert" => {
                for node in self.all() {
                    let kind = self.ast.node(node).kind();
                    if kind == K::ClassDeclaration {
                        let expression =
                            convert_class_declaration_to_class_expression(&ec, &mut self.ast, node);
                        let text = self.print(expression);
                        self.line(text);
                        let x = self.ast.new_identifier(js("x"));
                        let name = get_assigned_name_of_identifier(
                            &ec,
                            &mut self.ast,
                            Some(x),
                            Some(expression),
                        );
                        let text = self.print(name);
                        self.line(format!("name: {text}"));
                        let result = finish_transform_named_evaluation(
                            &ec,
                            &mut self.ast,
                            Some(expression),
                            name,
                            false,
                        )
                        .expect("a result");
                        let text = self.print(result);
                        self.line(text);
                    }
                    if kind == K::FunctionDeclaration {
                        let x = self.ast.new_identifier(js("x"));
                        let name = get_assigned_name_of_identifier(
                            &ec,
                            &mut self.ast,
                            Some(x),
                            Some(node),
                        );
                        let text = self.print(name);
                        self.line(format!("function name: {text}"));
                    }
                }
            }
            "propertyName" => {
                self.ec.start_variable_environment();
                for node in self.all() {
                    if matches!(
                        self.ast.node(node).kind().known(),
                        Some(K::PropertyAssignment | K::PropertyDeclaration)
                    ) {
                        let name = self.ast.node(node).name();
                        let (assigned, updated) = get_assigned_name_of_property_name(
                            &ec,
                            &mut self.ast,
                            name,
                            case.name.as_bytes(),
                        );
                        let updated = updated.expect("a name");
                        let text = format!(
                            "{} {} same={}",
                            self.print(assigned),
                            self.print(updated),
                            yes(Some(updated) == name)
                        );
                        self.line(text);
                    }
                }
                self.print_hoisted();
            }
            "notNull" => {
                let expressions = self.top_level_expressions();
                let result = create_not_null_condition(
                    &ec,
                    &mut self.ast,
                    expressions[0],
                    expressions[1],
                    case.flag,
                );
                let text = self.print(result);
                self.line(text);
            }
            "createBlock" => {
                let this = self.this_expression(case);
                let name = self.string_literal(&case.name);
                let block =
                    create_class_named_evaluation_helper_block(&ec, &mut self.ast, name, this);
                let text = self.print(block);
                self.line(text);
                let helper = is_class_named_evaluation_helper_block(&ec, &self.ast, block);
                self.line(format!("helper={}", yes(helper)));
            }
            "inject" => {
                for node in self.all() {
                    if is_class_like(&self.ast, node) {
                        let this = self.this_expression(case);
                        let name = self.string_literal(&case.name);
                        let result = inject_class_named_evaluation_helper_block_if_missing(
                            &ec,
                            &mut self.ast,
                            node,
                            name,
                            this,
                        );
                        let text = self.print(result);
                        self.line(text);
                        self.line(format!(
                            "same={} classThis={} assigned={}",
                            yes(result == node),
                            yes(ec.class_this(result).is_some()),
                            yes(ec.assigned_name(result) == Some(name))
                        ));
                    }
                }
            }
            "super" => {
                let state = SuperAccessState::new(&ec);
                if case.mode != "nil" && case.mode != "nilStatement" {
                    *state.captured_super_properties.borrow_mut() = Some(OrderedSet::default());
                }
                // Plain identifiers stand in for the transformers' unique
                // names, so the output needs no name generator.
                let binding = self.ast.new_identifier(js("_super"));
                state.super_binding.set(Some(binding));
                let binding = self.ast.new_identifier(js("_superIndex"));
                state.super_index_binding.set(Some(binding));
                let body = self
                    .all()
                    .into_iter()
                    .find(|&node| self.ast.node(node).kind() == K::MethodDeclaration)
                    .and_then(|method| self.ast.node(method).body())
                    .expect("a method body");
                let mut nodes = Vec::new();
                preorder(&self.ast, body, &mut nodes);
                for node in nodes {
                    state.track_super_access(&self.ast, node);
                }
                let captured = state
                    .captured_super_properties
                    .borrow()
                    .as_ref()
                    .map(|names| {
                        names
                            .values()
                            .map(|name| String::from_utf8(name.as_bytes().to_vec()).unwrap())
                            .collect::<Vec<_>>()
                    });
                if let Some(captured) = captured {
                    self.line(format!("captured={}", captured.join(",")));
                }
                self.line(format!(
                    "element={} assignment={}",
                    yes(state.has_super_element_access.get()),
                    yes(state.has_super_property_assignment.get())
                ));
                if case.mode == "nilStatement" {
                    let statement = state.create_super_access_variable_statement(&mut self.ast);
                    let text = self.print(statement);
                    self.line(text);
                }
                let substituted = state
                    .substitute_super_accesses_in_body(&mut self.ast, Some(body))
                    .expect("a body");
                self.line(format!("same={}", yes(substituted == body)));
                for statement in list_nodes(&self.ast, self.ast.node(substituted).statement_list())
                {
                    let text = self.print(statement);
                    self.line(text);
                }
                if state.captured_super_properties.borrow().is_some() {
                    let statement = state.create_super_access_variable_statement(&mut self.ast);
                    let text = self.print(statement);
                    self.line(text);
                }
            }
            "backingField" => {
                for node in self.all() {
                    let read = self.ast.node(node);
                    if read.kind() == K::PropertyDeclaration {
                        let (modifiers, initializer) = (read.modifiers(), read.initializer());
                        drop(read);
                        let modifiers = extract_modifiers(
                            &ec,
                            &mut self.ast,
                            modifiers,
                            !modifier_flags::ACCESSOR,
                        );
                        let field = create_accessor_property_backing_field(
                            &ec,
                            &mut self.ast,
                            node,
                            modifiers,
                            initializer,
                        );
                        let text = self.print(field);
                        self.line(text);
                    }
                }
            }
            "backingFieldParts" => {
                // The backing field without printing its generated name.
                for node in self.all() {
                    let read = self.ast.node(node);
                    if read.kind() == K::PropertyDeclaration {
                        let (modifiers, initializer) = (read.modifiers(), read.initializer());
                        drop(read);
                        let modifiers = extract_modifiers(
                            &ec,
                            &mut self.ast,
                            modifiers,
                            !modifier_flags::ACCESSOR,
                        );
                        let field = create_accessor_property_backing_field(
                            &ec,
                            &mut self.ast,
                            node,
                            modifiers,
                            initializer,
                        );
                        let read = self.ast.node(field);
                        let (name, modifiers, initializer) = (
                            read.name().expect("name"),
                            read.modifiers(),
                            read.initializer(),
                        );
                        drop(read);
                        let text = self
                            .ast
                            .node(name)
                            .as_private_identifier()
                            .expect("a private name")
                            .text()
                            .to_vec();
                        let parts: Vec<_> = list_nodes(&self.ast, modifiers)
                            .into_iter()
                            .map(|modifier| self.print(modifier))
                            .collect();
                        let initializer =
                            initializer.map_or_else(|| "none".to_owned(), |node| self.print(node));
                        self.line(format!(
                            "text={} generated={} modifiers=[{}] initializer={} same={}",
                            String::from_utf8(text).expect("UTF-8 name"),
                            yes(ec.has_auto_generate_info(name)),
                            parts.join(" "),
                            initializer,
                            yes(field == node)
                        ));
                    }
                }
            }
            scenario => panic!("unknown scenario {scenario}"),
        }
    }
}

fn panic_text(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(text) = payload.downcast_ref::<String>() {
        return text.clone();
    }
    if let Some(text) = payload.downcast_ref::<&str>() {
        return (*text).to_owned();
    }
    "<non-string panic>".to_owned()
}

/// Cases whose expected text needs printer support the printer does not have
/// yet. A difference is tolerated as long as no panic other than the
/// printer's own occurs. None remain: the computed-key, backing-field and
/// template-text cases pass since the printer's name generator landed.
const PENDING_PRINTER: &[&str] = &[];

fn execute(case: &Case) -> String {
    let mut run = Run::parse(&case.source);
    if let Err(payload) = catch_unwind(AssertUnwindSafe(|| run.run(case))) {
        run.out.push(format!("panic: {}", panic_text(&*payload)));
    }
    run.out.join("\n")
}

#[test]
fn the_shared_helpers_match_the_pin() {
    let cases = cases();
    assert!(cases.len() >= 40, "at least 40 expectations");
    let mut failures = Vec::new();
    for case in &cases {
        let actual = execute(case);
        if actual != case.expected && PENDING_PRINTER.contains(&case.id.as_str()) {
            let helper_panic = actual
                .lines()
                .any(|line| line.starts_with("panic:") && !line.starts_with("panic: printer:"));
            if !helper_panic {
                continue;
            }
        }
        if actual != case.expected {
            failures.push(format!(
                "{} ({}):\n--- expected\n{}\n--- actual\n{}",
                case.id, case.scenario, case.expected, actual
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {} cases differ:\n{}",
        failures.len(),
        cases.len(),
        failures.join("\n\n")
    );
}

#[test]
fn every_scenario_is_witnessed() {
    let cases = cases();
    for id in PENDING_PRINTER {
        assert!(
            cases.iter().any(|case| case.id == *id),
            "no pending case {id}"
        );
    }
    for scenario in [
        "classThisBlock",
        "helperBlock",
        "anonymous",
        "namedEvaluation",
        "transform",
        "finish",
        "convert",
        "propertyName",
        "notNull",
        "createBlock",
        "inject",
        "super",
        "backingField",
        "backingFieldParts",
    ] {
        assert!(
            cases.iter().any(|case| case.scenario == scenario),
            "no case for {scenario}"
        );
    }
}
