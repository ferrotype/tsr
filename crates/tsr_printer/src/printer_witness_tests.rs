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
    emit_flags as ef, AutoGenerateOptions, EmitContext, EmitHelper, Printer, PrinterOptions,
};
use serde_json::Value;
use tsr_ast::{
    AstBuilder, Factory, FactoryMethods, JsString, NodeId, NodeVisitor, RuntimeFactory,
    SyntaxKind as K,
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
    None
}

fn visit(
    ec: &EmitContext,
    pee: &str,
    visitor: &mut NodeVisitor<'_>,
    node: Option<NodeId>,
) -> Option<NodeId> {
    let id = node?;
    let mut ec = ec.clone();
    let read = visitor.factory().node(id);
    match read.kind().known() {
        Some(K::Identifier) => {
            let text = String::from_utf8(read.as_identifier()?.text().to_vec()).expect("text");
            return rewrite_identifier(&mut ec, visitor, id, &text).or(node);
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
    visitor.visit_each_child(node)
}

fn run(case: &Value) -> Vec<Vec<u8>> {
    let mut ec = EmitContext::new();
    let mut factory = AstBuilder::with_hooks(
        SourceText::default(),
        &tsr_arena::Counters::new(),
        ec.factory_hooks(),
    );
    let parsed = parse_type_script(case["source"].as_str().expect("source").as_bytes(), false);
    let original = parsed.root();
    factory.retain_file(parsed.publish_unbound());
    let pee = case["pee"].as_str().unwrap_or("").to_owned();
    let hooks = ec.visitor_hooks();
    let visit_context = ec.clone();
    let visitor = move |visitor: &mut NodeVisitor<'_>, node: Option<NodeId>| {
        visit(&visit_context, &pee, visitor, node)
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
    let mut printer = Printer::new(
        PrinterOptions {
            new_line: NewLineKind::LF,
            no_emit_helpers: case["no_emit_helpers"].as_bool() == Some(true),
            remove_comments: case["remove_comments"].as_bool() == Some(true),
            ..PrinterOptions::default()
        },
        &ec,
    );
    let writes = case["writes"].as_u64().unwrap_or(1).max(1);
    (0..writes)
        .map(|_| {
            printer
                .emit_source_file(factory.view(), file)
                .unwrap_or_else(|error| format!("printer error: {error}").into_bytes())
        })
        .collect()
}

#[test]
fn generated_names_helpers_and_no_asi_parentheses_print_as_pinned() {
    let document: Value = serde_json::from_str(FIXTURE).expect("fixture");
    let cases = document["cases"].as_array().expect("cases");
    assert!(cases.len() >= 20);
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
