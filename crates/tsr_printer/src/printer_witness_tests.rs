//! The printer against native output for generated names, emit helpers and
//! the parentheses `parenthesizeExpressionForNoAsi` creates
//! (`data/phase3/printer/names-helpers.json`, produced by
//! `tools/phase3/printer_witness/a1_witness_test.go` at the pin). Each case is
//! rebuilt here the way the producer builds it: parse, rewrite the marker
//! identifiers with the emit context's node visitor, record helpers and flags,
//! and print with one printer.

use crate::emit_helpers::{
    ADVANCED_ASYNC_SUPER_HELPER, ASYNC_SUPER_HELPER, AWAITER_HELPER, AWAIT_HELPER,
    CREATE_BINDING_HELPER, DECORATE_HELPER, ES_DECORATE_HELPER, EXPORT_STAR_HELPER,
    IMPORT_DEFAULT_HELPER, IMPORT_STAR_HELPER, MAKE_TEMPLATE_OBJECT_HELPER, METADATA_HELPER,
    PARAM_HELPER, PROP_KEY_HELPER, REST_HELPER, RUN_INITIALIZERS_HELPER, SET_FUNCTION_NAME_HELPER,
    SET_MODULE_DEFAULT_HELPER,
};
use crate::generated_identifier_flags as g;
use crate::printer_emit_tests::parse_type_script;
use crate::{
    emit_flags as ef, AutoGenerateOptions, EmitContext, EmitHelper, EmitTextWriter, Printer,
    PrinterOptions, SnippetElement, SnippetKind, TextWriter,
};
use serde_json::Value;
use std::cell::Cell;
use std::collections::HashMap;
use std::ops::ControlFlow;
use tsr_ast::{
    AstBuilder, AstView, ChildVisitor, Factory, FactoryMethods, JsString, NodeId, NodeListId,
    NodeSlice, NodeVisitor, RuntimeFactory, SymbolId, SyntaxKind as K,
};
use tsr_core::NewLineKind;
use tsr_jsstring::SourceText;

const FIXTURE: &str = include_str!("../../../data/phase3/printer/names-helpers.json");

fn helper(name: &str) -> &'static EmitHelper {
    match name {
        "decorate" => &DECORATE_HELPER,
        "metadata" => &METADATA_HELPER,
        "param" => &PARAM_HELPER,
        "awaiter" => &AWAITER_HELPER,
        "await" => &AWAIT_HELPER,
        "rest" => &REST_HELPER,
        "asyncSuper" => &ASYNC_SUPER_HELPER,
        "advancedAsyncSuper" => &ADVANCED_ASYNC_SUPER_HELPER,
        "esDecorate" => &ES_DECORATE_HELPER,
        "runInitializers" => &RUN_INITIALIZERS_HELPER,
        "propKey" => &PROP_KEY_HELPER,
        "setFunctionName" => &SET_FUNCTION_NAME_HELPER,
        "importStar" => &IMPORT_STAR_HELPER,
        "importDefault" => &IMPORT_DEFAULT_HELPER,
        "createBinding" => &CREATE_BINDING_HELPER,
        "setModuleDefault" => &SET_MODULE_DEFAULT_HELPER,
        "exportStar" => &EXPORT_STAR_HELPER,
        "makeTemplateObject" => &MAKE_TEMPLATE_OBJECT_HELPER,
        _ => panic!("unknown helper {name}"),
    }
}

fn helpers(case: &Value, key: &str) -> Vec<&'static EmitHelper> {
    case[key].as_array().map_or_else(Vec::new, |names| {
        names
            .iter()
            .map(|name| helper(name.as_str().expect("helper name")))
            .collect()
    })
}

fn js(text: &str) -> JsString {
    JsString::from_bytes(text.as_bytes())
}

/// The producer's marker rewrite of one identifier, or `None` to keep it.
fn rewrite_identifier(
    ec: &mut EmitContext,
    factory: &mut dyn Factory,
    marks: &Marks,
    node: NodeId,
    text: &str,
) -> Option<NodeId> {
    let unique = |prefix: &str| text.strip_prefix(prefix).map(js);
    if text == "$temp" {
        return Some(ec.new_temp_variable(factory));
    }
    if text == "$loop" {
        return Some(ec.new_loop_variable(factory));
    }
    if text == "$affixed" {
        let options = AutoGenerateOptions {
            prefix: js("pre"),
            suffix: js("suf"),
            ..AutoGenerateOptions::default()
        };
        return Some(ec.new_temp_variable_ex(factory, options));
    }
    if let Some(name) = unique("$unique_") {
        return Some(ec.new_unique_name(factory, name));
    }
    if let Some(name) = unique("$optimistic_") {
        let options = AutoGenerateOptions {
            flags: g::OPTIMISTIC,
            ..AutoGenerateOptions::default()
        };
        return Some(ec.new_unique_name_ex(factory, name, options));
    }
    if let Some(name) = unique("$filelevel_") {
        let options = AutoGenerateOptions {
            flags: g::FILE_LEVEL | g::OPTIMISTIC,
            ..AutoGenerateOptions::default()
        };
        return Some(ec.new_unique_name_ex(factory, name, options));
    }
    if let Some(name) = unique("$helper_") {
        return Some(ec.new_unscoped_helper_name(factory, name.as_bytes()));
    }
    if text.starts_with("$node_") {
        return Some(ec.new_generated_name_for_node(factory, node));
    }
    if text == "$strtemp" {
        let temp = ec.new_temp_variable(factory);
        return Some(ec.new_string_literal_from_node(factory, temp));
    }
    if let Some(name) = unique("$strunique_") {
        let name = ec.new_unique_name(factory, name);
        return Some(ec.new_string_literal_from_node(factory, name));
    }
    if let Some(index) = text.strip_prefix("$decl_") {
        let index: usize = index.parse().expect("statement index");
        return Some(ec.new_generated_name_for_node(factory, marks.statements[index]));
    }
    if let Some(name) = unique("$nlhelper_") {
        let name = ec.new_unscoped_helper_name(factory, name.as_bytes());
        ec.add_emit_flags(name, ef::START_ON_NEW_LINE);
        return Some(name);
    }
    if let Some(text) = unique("$strtext_") {
        let identifier = factory.new_identifier(text);
        return Some(ec.new_string_literal_from_node(factory, identifier));
    }
    if let Some(text) = unique("$strascii_") {
        let identifier = factory.new_identifier(text);
        let literal = ec.new_string_literal_from_node(factory, identifier);
        ec.add_emit_flags(literal, ef::NO_ASCII_ESCAPING);
        return Some(literal);
    }
    if text == "$lastpee" {
        return Some(
            marks
                .last_pee
                .get()
                .expect("a partially emitted expression"),
        );
    }
    if text == "$lastpae" {
        return Some(marks.last_pae.get().expect("a property access"));
    }
    None
}

/// The string-literal markers of a JSX attribute's value.
fn rewrite_string_literal(
    ec: &mut EmitContext,
    factory: &mut dyn Factory,
    text: &[u8],
) -> Option<NodeId> {
    let (rest, ascii) = if let Some(rest) = text.strip_prefix(b"$strjsx_") {
        (rest, false)
    } else {
        (text.strip_prefix(b"$strasciijsx_")?, true)
    };
    let identifier = factory.new_identifier(JsString::from_bytes(rest));
    let literal = ec.new_string_literal_from_node(factory, identifier);
    if ascii {
        ec.add_emit_flags(literal, ef::NO_ASCII_ESCAPING);
    }
    Some(literal)
}

/// What the producer's visitor closes over besides the case: the original
/// statements `$decl_<i>` names, the last partially emitted expression and
/// property access it produced, and the next tab stop.
struct Marks {
    statements: Vec<NodeId>,
    last_pee: Cell<Option<NodeId>>,
    last_pae: Cell<Option<NodeId>>,
    tab_stops: bool,
    tab_stop: Cell<i64>,
}

fn visit(
    ec: &EmitContext,
    pee: &str,
    marks: &Marks,
    visitor: &mut NodeVisitor<'_>,
    node: Option<NodeId>,
) -> Option<NodeId> {
    let id = node?;
    let mut ec = ec.clone();
    let read = visitor.factory().node(id);
    match read.kind().known() {
        Some(K::Identifier) => {
            let text = String::from_utf8(read.as_identifier()?.text().to_vec()).expect("text");
            return rewrite_identifier(&mut ec, visitor, marks, id, &text).or(node);
        }
        Some(K::StringLiteral) => {
            let text = read.data_source().as_string_literal()?.text().to_vec();
            return rewrite_string_literal(&mut ec, visitor, &text).or(node);
        }
        Some(K::JsxAttribute) => {
            let attribute = read.data_source().as_jsx_attribute()?;
            let (name, initializer) = (attribute.name(), attribute.initializer());
            let is_marker = initializer.is_some_and(|initializer| {
                visitor
                    .factory()
                    .node(initializer)
                    .data_source()
                    .as_string_literal()
                    .is_some_and(|literal| literal.text() == b"$strns")
            });
            if is_marker {
                let name_node = name.expect("attribute name");
                let literal = ec.new_string_literal_from_node(visitor, name_node);
                return Some(visitor.update_jsx_attribute(id, name, Some(literal)));
            }
        }
        Some(K::EmptyStatement) => {
            if marks.tab_stops {
                let order = marks.tab_stop.get();
                ec.set_snippet_element(
                    id,
                    SnippetElement {
                        kind: SnippetKind::TabStop,
                        order,
                    },
                );
                marks.tab_stop.set(order + 1);
            }
            return node;
        }
        Some(K::PrivateIdentifier) => {
            let text = read.data_source().as_private_identifier()?.text().to_vec();
            let text = String::from_utf8(text).expect("text");
            if let Some(name) = text.strip_prefix("#unique_") {
                return Some(ec.new_unique_private_name(visitor, js(&format!("#{name}"))));
            }
            return node;
        }
        Some(K::ParenthesizedExpression) if !pee.is_empty() => {
            let expression = read.expression();
            let range = read.range();
            let expression = visitor.visit_node(expression);
            let pee_node = visitor.new_partially_emitted_expression(expression);
            if pee == "original" {
                ec.set_original(pee_node, id);
            } else {
                ec.add_synthetic_leading_comment(
                    pee_node,
                    K::SingleLineCommentTrivia,
                    js(" c"),
                    true,
                );
            }
            visitor.set_node_range(pee_node, range);
            marks.last_pee.set(Some(pee_node));
            return Some(pee_node);
        }
        Some(K::FunctionDeclaration) => {
            let name = read.name();
            let updated = visitor.visit_each_child(node)?;
            if let Some(name) = name {
                if visitor
                    .factory()
                    .node(name)
                    .as_identifier()
                    .is_some_and(|name| name.text().starts_with(b"reuse"))
                {
                    ec.add_emit_flags(updated, ef::REUSE_TEMP_VARIABLE_SCOPE);
                }
            }
            return Some(updated);
        }
        _ => {}
    }
    let visited = visitor.visit_each_child(node)?;
    if visitor.factory().node(visited).kind() == K::PropertyAccessExpression {
        marks.last_pae.set(Some(visited));
    }
    Some(visited)
}

fn run(case: &Value) -> Vec<Vec<u8>> {
    let mut ec = EmitContext::new();
    let mut factory = AstBuilder::with_hooks(
        SourceText::default(),
        &tsr_arena::Counters::new(),
        ec.factory_hooks(),
    );
    let flag = |name: &str| case[name].as_bool() == Some(true);
    let parsed = parse_type_script(
        case["source"].as_str().expect("source").as_bytes(),
        flag("jsx"),
    );
    let original = parsed.root();
    let file = parsed.publish_unbound();
    let bound = flag("bind").then(|| tsr_binder::bind_source_file(&file, original).expect("bind"));
    factory.retain_file(file);
    let statements = {
        let view = factory.view();
        let source = view.node(original).expect("source file");
        let statements = source.statements(view).expect("statements");
        view.node_slice(statements)
            .expect("statements")
            .iter()
            .flatten()
            .collect()
    };
    let marks = Marks {
        statements,
        last_pee: Cell::new(None),
        last_pae: Cell::new(None),
        tab_stops: flag("tab_stops"),
        tab_stop: Cell::new(0),
    };
    let pee = case["pee"].as_str().unwrap_or("").to_owned();
    let hooks = ec.visitor_hooks();
    let visit_context = ec.clone();
    let visitor = move |visitor: &mut NodeVisitor<'_>, node: Option<NodeId>| {
        visit(&visit_context, &pee, &marks, visitor, node)
    };
    let file = hooks
        .new_node_visitor(Some(&visitor), &mut factory)
        .visit_source_file(original);
    if case["external_helpers"].as_bool() == Some(true) {
        ec.add_emit_flags(original, ef::EXTERNAL_HELPERS);
    }
    if case["helpers_module"].as_bool() == Some(true) {
        let name = ec.new_unique_name(&mut factory, js("tslib"));
        ec.set_external_helpers_module_name(&factory, original, Some(name));
    }
    let file_helpers = helpers(case, "helpers");
    if !file_helpers.is_empty() {
        ec.add_emit_helper(file, &file_helpers);
    }
    let body_helpers = helpers(case, "body_helpers");
    if !body_helpers.is_empty() {
        let statements = Factory::node(&factory, file)
            .statement_list()
            .expect("statements");
        let nodes = factory.read_list(statements).nodes();
        let statements: Vec<NodeId> = factory.read_nodes(nodes).iter().flatten().collect();
        for statement in statements {
            let read = Factory::node(&factory, statement);
            if read.kind() == K::FunctionDeclaration {
                ec.add_emit_helper(read.body().expect("body"), &body_helpers);
                break;
            }
        }
    }
    let bound_view = bound.as_ref().map(tsr_ast::BoundFile::view);
    let mut printer = Printer::new(
        PrinterOptions {
            new_line: NewLineKind::LF,
            no_emit_helpers: flag("no_emit_helpers"),
            remove_comments: flag("remove_comments"),
            never_ascii_escape: flag("never_ascii_escape"),
            ..PrinterOptions::default()
        },
        &ec,
    );
    printer.bindings = bound_view
        .as_ref()
        .map(|view| view as &dyn crate::PrinterBindings);
    let writes = case["writes"].as_u64().unwrap_or(1).max(1);
    if flag("listener") {
        // `EmitSourceFile` with the notifications recorded by the writer.
        let mut writer = ListeningWriter {
            inner: TextWriter::new(b"\n", 0),
            before: HashMap::new(),
            after: HashMap::new(),
        };
        let mut output = Vec::new();
        for _ in 0..writes {
            output.push(
                match printer.write(factory.view(), file, Some(file), &mut writer, None) {
                    Ok(()) => writer.text().to_vec(),
                    Err(error) => format!("printer error: {error}").into_bytes(),
                },
            );
        }
        output.push(positions(factory.view(), file, &writer).into_bytes());
        return output;
    }
    (0..writes)
        .map(|_| {
            printer
                .emit_source_file(factory.view(), file)
                .unwrap_or_else(|error| format!("printer error: {error}").into_bytes())
        })
        .collect()
}

/// A text writer that records where each emit notification for a node of
/// the tree came, as the producer's `OnBeforeEmitNode`/`OnAfterEmitNode`
/// handlers do.
struct ListeningWriter {
    inner: TextWriter,
    before: HashMap<NodeId, usize>,
    after: HashMap<NodeId, usize>,
}

/// The children of a node in `ForEachChild` order.
struct Children<'a>(AstView<'a>, Vec<NodeId>);

impl ChildVisitor for Children<'_> {
    fn visit_node(&mut self, node: NodeId) -> ControlFlow<()> {
        self.1.push(node);
        ControlFlow::Continue(())
    }
    fn visit_list(&mut self, nodes: NodeListId) -> ControlFlow<()> {
        let slice = self.0.list(nodes).expect("list").nodes();
        self.visit_node_slice(slice)
    }
    fn visit_node_slice(&mut self, nodes: NodeSlice) -> ControlFlow<()> {
        self.1
            .extend(self.0.node_slice(nodes).expect("nodes").iter().flatten());
        ControlFlow::Continue(())
    }
}

/// The producer's `a1Positions`: each node of the printed tree in pre-order
/// with the writer positions its notifications recorded.
fn positions(view: AstView<'_>, root: NodeId, writer: &ListeningWriter) -> String {
    let mut lines = Vec::new();
    let mut pending = vec![root];
    while let Some(node) = pending.pop() {
        let read = view.node(node).expect("node");
        let at = |map: &HashMap<NodeId, usize>| {
            map.get(&node)
                .map_or_else(|| "-".to_owned(), ToString::to_string)
        };
        lines.push(format!(
            "{:?} {} {}",
            read.kind().known().expect("known kind"),
            at(&writer.before),
            at(&writer.after)
        ));
        let mut children = Children(view, Vec::new());
        let _ = read.for_each_child(&mut children);
        pending.extend(children.1.into_iter().rev());
    }
    lines.join("\n")
}

impl EmitTextWriter for ListeningWriter {
    fn write(&mut self, s: &[u8]) {
        self.inner.write(s);
    }
    fn write_trailing_semicolon(&mut self, text: &[u8]) {
        self.inner.write_trailing_semicolon(text);
    }
    fn write_comment(&mut self, text: &[u8]) {
        self.inner.write_comment(text);
    }
    fn write_keyword(&mut self, text: &[u8]) {
        self.inner.write_keyword(text);
    }
    fn write_operator(&mut self, text: &[u8]) {
        self.inner.write_operator(text);
    }
    fn write_punctuation(&mut self, text: &[u8]) {
        self.inner.write_punctuation(text);
    }
    fn write_space(&mut self, text: &[u8]) {
        self.inner.write_space(text);
    }
    fn write_string_literal(&mut self, text: &[u8]) {
        self.inner.write_string_literal(text);
    }
    fn write_parameter(&mut self, text: &[u8]) {
        self.inner.write_parameter(text);
    }
    fn write_property(&mut self, text: &[u8]) {
        self.inner.write_property(text);
    }
    fn write_symbol(&mut self, text: &[u8], symbol: Option<SymbolId>) {
        self.inner.write_symbol(text, symbol);
    }
    fn write_line(&mut self) {
        self.inner.write_line();
    }
    fn write_line_force(&mut self, force: bool) {
        self.inner.write_line_force(force);
    }
    fn increase_indent(&mut self) {
        self.inner.increase_indent();
    }
    fn decrease_indent(&mut self) {
        self.inner.decrease_indent();
    }
    fn clear(&mut self) {
        self.inner.clear();
    }
    fn text(&self) -> &[u8] {
        self.inner.text()
    }
    fn raw_write(&mut self, s: &[u8]) {
        self.inner.raw_write(s);
    }
    fn write_literal(&mut self, s: &[u8]) {
        self.inner.write_literal(s);
    }
    fn get_text_pos(&self) -> usize {
        self.inner.get_text_pos()
    }
    fn get_line(&self) -> isize {
        self.inner.get_line()
    }
    fn get_column(&self) -> isize {
        self.inner.get_column()
    }
    fn get_indent(&self) -> isize {
        self.inner.get_indent()
    }
    fn is_at_start_of_line(&self) -> bool {
        self.inner.is_at_start_of_line()
    }
    fn has_trailing_comment(&self) -> bool {
        self.inner.has_trailing_comment()
    }
    fn has_trailing_whitespace(&self) -> bool {
        self.inner.has_trailing_whitespace()
    }
    fn on_before_emit_node(&mut self, node: NodeId) {
        self.before.insert(node, self.inner.get_text_pos());
    }
    fn on_after_emit_node(&mut self, node: NodeId) {
        self.after.insert(node, self.inner.get_text_pos());
    }
}

#[test]
fn generated_names_helpers_and_no_asi_parentheses_print_as_pinned() {
    let document: Value = serde_json::from_str(FIXTURE).expect("fixture");
    let cases = document["cases"].as_array().expect("cases");
    assert!(cases.len() >= 137);
    let mut failures = Vec::new();
    for case in cases {
        let id = case["id"].as_str().expect("id");
        let expected: Vec<&str> = case["expected"]
            .as_array()
            .expect("expected")
            .iter()
            .map(|text| text.as_str().expect("text"))
            .collect();
        let actual = run(case);
        let actual: Vec<String> = actual
            .iter()
            .map(|text| String::from_utf8_lossy(text).into_owned())
            .collect();
        if actual != expected {
            failures.push(format!(
                "{id}:\n--- pinned\n{expected:#?}\n--- ported\n{actual:#?}"
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {} cases differ:\n{}",
        failures.len(),
        cases.len(),
        failures.join("\n")
    );
}
