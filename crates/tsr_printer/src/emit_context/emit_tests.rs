//! Environment, emit-helper and factory-helper tests. There are no Go unit
//! tests for these functions; every expectation below was derived by reading
//! the pinned `emitcontext.go`, `helpers.go` and `factory.go`.

use super::*;
use crate::emit_flags as ef;
use crate::emit_helpers::{self as helpers, compare_emit_helpers, EmitHelper};
use tsr_ast::{
    evaluator::outer_expression_kinds as oek, AstBuilder, NodeListId, NodeVisit, NodeVisitor,
    RuntimeFactory, SyntaxKind as K,
};
use tsr_core::TextRange;

fn builder(emit: &EmitContext) -> AstBuilder {
    AstBuilder::with_hooks(
        tsr_jsstring::SourceText::default(),
        &tsr_arena::Counters::new(),
        emit.factory_hooks(),
    )
}
fn ident(ast: &mut AstBuilder, text: &str) -> NodeId {
    ast.new_identifier(JsString::from_bytes(text.as_bytes()))
}
fn prologue(ast: &mut AstBuilder, text: &str) -> NodeId {
    let literal = ast.new_string_literal(JsString::from_bytes(text.as_bytes()), 0);
    ast.new_expression_statement(Some(literal))
}
fn statement(ast: &mut AstBuilder, text: &str) -> NodeId {
    let name = ident(ast, text);
    ast.new_expression_statement(Some(name))
}
fn custom(emit: &mut EmitContext, node: NodeId) -> NodeId {
    emit.add_emit_flags(node, ef::CUSTOM_PROLOGUE);
    node
}
fn hoisted_function(emit: &mut EmitContext, ast: &mut AstBuilder, text: &str) -> NodeId {
    let name = ident(ast, text);
    let node = ast.new_function_declaration(None, None, Some(name), None, None, None, None, None);
    custom(emit, node)
}
fn hoisted_var(emit: &mut EmitContext, ast: &mut AstBuilder, text: &str) -> NodeId {
    let name = ident(ast, text);
    let declaration = ast.new_variable_declaration(Some(name), None, None, None);
    let declarations = new_list(ast, vec![declaration]);
    let list = ast.new_variable_declaration_list(Some(declarations), 0);
    let node = ast.new_variable_statement(None, Some(list));
    custom(emit, node)
}
fn new_list(ast: &mut AstBuilder, nodes: Vec<NodeId>) -> NodeListId {
    let nodes = ast.alloc_nodes(nodes.into_iter().map(Some).collect());
    ast.alloc_list(TextRange::new(-1, -1), nodes)
}
fn nodes(ast: &AstBuilder, list: NodeListId) -> Vec<NodeId> {
    ast.read_nodes(ast.read_list(list).nodes())
        .iter()
        .map(Option::unwrap)
        .collect()
}
fn kind(ast: &AstBuilder, node: NodeId) -> tsr_ast::NodeKind {
    Factory::node(ast, node).kind()
}
fn identifier_text(ast: &AstBuilder, node: NodeId) -> Vec<u8> {
    Factory::node(ast, node)
        .as_identifier()
        .unwrap()
        .text()
        .to_vec()
}
fn declaration_names(ast: &AstBuilder, statement: NodeId) -> (u32, Vec<Vec<u8>>) {
    let list = Factory::node(ast, statement)
        .as_variable_statement()
        .unwrap()
        .declaration_list()
        .unwrap();
    let declarations = Factory::node(ast, list)
        .as_variable_declaration_list()
        .unwrap()
        .declarations()
        .unwrap();
    (
        Factory::node(ast, list).flags(),
        nodes(ast, declarations)
            .into_iter()
            .map(|declaration| {
                identifier_text(ast, Factory::node(ast, declaration).name().unwrap())
            })
            .collect(),
    )
}

// EndVariableEnvironment: hoisted functions, then one `var` statement for the
// hoisted variables, then initialization statements, then the lexical
// environment's `let` statement. Each generated statement is a custom prologue
// and each hoisted declaration has NoNestedSourceMaps.
#[test]
fn end_variable_environment_orders_functions_variables_initializers_and_lets() {
    let mut emit = EmitContext::new();
    let mut ast = builder(&emit);
    emit.start_variable_environment();
    let a = ident(&mut ast, "a");
    let b = ident(&mut ast, "b");
    let c = ident(&mut ast, "c");
    emit.add_variable_declaration(&mut ast, a);
    emit.add_lexical_declaration(&mut ast, c);
    emit.add_variable_declaration(&mut ast, b);
    let name = ident(&mut ast, "f");
    let function =
        ast.new_function_declaration(None, None, Some(name), None, None, None, None, None);
    emit.add_hoisted_function_declaration(function);
    let init = statement(&mut ast, "init");
    emit.add_initialization_statement(init);

    let statements = emit.end_variable_environment(&mut ast);
    assert_eq!(statements.len(), 4);
    assert_eq!(statements[0], function);
    assert_eq!(statements[2], init);
    for &statement in &statements {
        assert_ne!(emit.emit_flags(statement) & ef::CUSTOM_PROLOGUE, 0);
    }
    assert_eq!(
        declaration_names(&ast, statements[1]),
        (node_flags::NONE, vec![b"a".to_vec(), b"b".to_vec()])
    );
    assert_eq!(
        declaration_names(&ast, statements[3]),
        (node_flags::LET, vec![b"c".to_vec()])
    );
    let list = Factory::node(&ast, statements[1])
        .as_variable_statement()
        .unwrap()
        .declaration_list()
        .unwrap();
    let declarations = Factory::node(&ast, list)
        .as_variable_declaration_list()
        .unwrap()
        .declarations()
        .unwrap();
    for declaration in nodes(&ast, declarations) {
        assert_eq!(emit.emit_flags(declaration), ef::NO_NESTED_SOURCE_MAPS);
    }
}

// An environment with nothing hoisted ends with no statements, and the lexical
// environment it started is closed with it.
#[test]
fn empty_environments_produce_no_statements() {
    let mut emit = EmitContext::new();
    let mut ast = builder(&emit);
    emit.start_variable_environment();
    emit.start_lexical_environment();
    assert!(emit.end_lexical_environment(&mut ast).is_empty());
    assert!(emit.end_variable_environment(&mut ast).is_empty());
}

#[test]
#[should_panic(expected = "stack is empty")]
fn ending_an_unopened_environment_panics() {
    let mut emit = EmitContext::new();
    let mut ast = builder(&emit);
    emit.end_variable_environment(&mut ast);
}

// mergeEnvironment places right-hand standard prologues not already on the
// left first, then right functions before left functions, right variables
// before left variables, and right custom prologues before left ones, ahead of
// the other left statements.
#[test]
fn merge_environment_interleaves_each_prologue_group_right_first() {
    let mut emit = EmitContext::new();
    let mut ast = builder(&emit);
    let use_strict_left = prologue(&mut ast, "use strict");
    let function_left = hoisted_function(&mut emit, &mut ast, "fl");
    let var_left = hoisted_var(&mut emit, &mut ast, "vl");
    let custom_left = statement(&mut ast, "cl");
    custom(&mut emit, custom_left);
    let other = statement(&mut ast, "other");
    let left = vec![use_strict_left, function_left, var_left, custom_left, other];

    let use_strict_right = prologue(&mut ast, "use strict");
    let directive_right = prologue(&mut ast, "x");
    let function_right = hoisted_function(&mut emit, &mut ast, "fr");
    let var_right = hoisted_var(&mut emit, &mut ast, "vr");
    let custom_right = statement(&mut ast, "cr");
    custom(&mut emit, custom_right);
    let right = vec![
        use_strict_right,
        directive_right,
        function_right,
        var_right,
        custom_right,
    ];

    assert_eq!(
        emit.merge_environment(&ast, left, &right),
        vec![
            directive_right,
            use_strict_left,
            function_right,
            function_left,
            var_right,
            var_left,
            custom_right,
            custom_left,
            other,
        ]
    );
}

// With no standard prologue on the left, every right prologue is spliced in at
// the start; a variable statement with an initializer is not a hoisted
// variable and is merged with the other custom prologues.
#[test]
fn merge_environment_without_left_prologues_and_initialized_variables() {
    let mut emit = EmitContext::new();
    let mut ast = builder(&emit);
    let other = statement(&mut ast, "other");
    let p1 = prologue(&mut ast, "a");
    let p2 = prologue(&mut ast, "a");
    let name = ident(&mut ast, "v");
    let one = ast.new_numeric_literal(JsString::from_bytes(&b"1"[..]), 0);
    let declaration = ast.new_variable_declaration(Some(name), None, None, Some(one));
    let declarations = new_list(&mut ast, vec![declaration]);
    let list = ast.new_variable_declaration_list(Some(declarations), 0);
    let initialized = ast.new_variable_statement(None, Some(list));
    custom(&mut emit, initialized);
    assert_eq!(
        emit.merge_environment(&ast, vec![other], &[p1, p2, initialized]),
        vec![p1, p2, initialized, other]
    );
}

#[test]
#[should_panic(expected = "Expected declarations to be valid standard or custom prologues")]
fn merge_environment_rejects_ordinary_declarations() {
    let mut emit = EmitContext::new();
    let mut ast = builder(&emit);
    let ordinary = statement(&mut ast, "x");
    emit.merge_environment(&ast, Vec::new(), &[ordinary]);
}

// The list forms keep the original list (by identity) when nothing merges and
// copy its location onto the new list when something does; a nil list that
// receives declarations is upstream's nil dereference.
#[test]
fn merge_environment_lists_keep_identity_or_location() {
    let mut emit = EmitContext::new();
    let mut ast = builder(&emit);
    let body = statement(&mut ast, "body");
    let list = new_list(&mut ast, vec![body]);
    RuntimeFactory::set_list_location(&mut ast, list, TextRange::new(3, 9));
    assert_eq!(emit.merge_environment_list(&mut ast, list, &[]), list);

    emit.start_lexical_environment();
    assert_eq!(
        emit.end_and_merge_lexical_environment_list(&mut ast, Some(list)),
        Some(list)
    );
    emit.start_lexical_environment();
    assert_eq!(
        emit.end_and_merge_lexical_environment_list(&mut ast, None),
        None
    );

    emit.start_variable_environment();
    let temp = ident(&mut ast, "temp");
    emit.add_variable_declaration(&mut ast, temp);
    let merged = emit
        .end_and_merge_variable_environment_list(&mut ast, Some(list))
        .unwrap();
    assert_ne!(merged, list);
    assert_eq!(ast.read_list(merged).loc(), TextRange::new(3, 9));
    let merged = nodes(&ast, merged);
    assert_eq!(merged.len(), 2);
    assert_eq!(merged[1], body);
    assert_eq!(declaration_names(&ast, merged[0]).1, vec![b"temp".to_vec()]);
}

#[test]
#[should_panic(expected = "nil pointer dereference")]
fn merging_declarations_into_a_nil_list_panics() {
    let mut emit = EmitContext::new();
    let mut ast = builder(&emit);
    emit.start_variable_environment();
    let temp = ident(&mut ast, "temp");
    emit.add_variable_declaration(&mut ast, temp);
    emit.end_and_merge_variable_environment_list(&mut ast, None);
}

// VisitParameters: a variable hoisted while visiting a parameter list moves
// each initializer into the body as `if (a === void 0) { a = 1; }`, an
// initialization statement after the hoisted `var`. A parameter list that
// hoists nothing is returned as the visitor produced it.
#[test]
fn visit_parameters_moves_initializers_when_parameters_hoist_variables() {
    let mut emit = EmitContext::new();
    let mut ast = builder(&emit);
    let a = ident(&mut ast, "a");
    let one = ast.new_numeric_literal(JsString::from_bytes(&b"1"[..]), 0);
    let parameter = ast.new_parameter_declaration(None, None, Some(a), None, None, Some(one));
    ast.set_node_range(parameter, TextRange::new(1, 6));
    let parameters = new_list(&mut ast, vec![parameter]);
    RuntimeFactory::set_list_location(&mut ast, parameters, TextRange::new(0, 7));

    // Without hoisting, the list is the visitor's result.
    let identity = |_: &mut NodeVisitor<'_>, node: Option<NodeId>| node;
    {
        let mut visitor = NodeVisitor::new(
            Some(&identity),
            Some(&mut ast as &mut dyn RuntimeFactory),
            tsr_ast::NodeVisitorHooks::default(),
        );
        assert_eq!(
            emit.visit_parameters(Some(parameters), &mut visitor),
            Some(parameters)
        );
    }
    assert!(emit.end_variable_environment(&mut ast).is_empty());

    let context = emit.clone();
    let hoist = move |visitor: &mut NodeVisitor<'_>, node: Option<NodeId>| {
        let temp = visitor.new_identifier(JsString::from_bytes(&b"_a"[..]));
        context
            .clone()
            .add_variable_declaration(visitor.factory_mut(), temp);
        node
    };
    let visited = {
        let mut visitor = NodeVisitor::new(
            Some(&hoist),
            Some(&mut ast as &mut dyn RuntimeFactory),
            tsr_ast::NodeVisitorHooks::default(),
        );
        emit.visit_parameters(Some(parameters), &mut visitor)
            .unwrap()
    };
    assert_ne!(visited, parameters);
    assert_eq!(ast.read_list(visited).loc(), TextRange::new(0, 7));
    let updated = nodes(&ast, visited)[0];
    assert_ne!(updated, parameter);
    assert_eq!(emit.original(updated), Some(parameter));
    let read = Factory::node(&ast, updated);
    let data = read.as_parameter_declaration().unwrap();
    assert_eq!(data.name(), Some(a));
    assert_eq!(data.initializer(), None);
    drop(read);
    assert_eq!(emit.emit_flags(one), ef::NO_SOURCE_MAP | ef::NO_COMMENTS);

    let statements = emit.end_variable_environment(&mut ast);
    assert_eq!(statements.len(), 2);
    assert_eq!(
        declaration_names(&ast, statements[0]).1,
        vec![b"_a".to_vec()]
    );
    let if_statement = statements[1];
    assert_eq!(kind(&ast, if_statement), K::IfStatement);
    assert_ne!(emit.emit_flags(if_statement) & ef::CUSTOM_PROLOGUE, 0);
    let read = Factory::node(&ast, if_statement);
    let data = read.as_if_statement().unwrap();
    let (condition, then_statement) = (data.expression().unwrap(), data.then_statement().unwrap());
    drop(read);
    // a === void 0
    let read = Factory::node(&ast, condition);
    let binary = read.as_binary_expression().unwrap();
    let (left, operator, right) = (
        binary.left().unwrap(),
        binary.operator_token().unwrap(),
        binary.right().unwrap(),
    );
    drop(read);
    assert_eq!(kind(&ast, operator), K::EqualsEqualsEqualsToken);
    assert_eq!(identifier_text(&ast, left), b"a");
    assert_ne!(left, a);
    assert_eq!(kind(&ast, right), K::VoidExpression);
    // { a = 1; } on one line with the parameter's location
    assert_eq!(
        Factory::node(&ast, then_statement).range(),
        TextRange::new(1, 6)
    );
    assert_eq!(
        emit.emit_flags(then_statement),
        ef::SINGLE_LINE | ef::NO_TRAILING_SOURCE_MAP | ef::NO_TOKEN_SOURCE_MAPS | ef::NO_COMMENTS
    );
    let block_statements = Factory::node(&ast, then_statement)
        .as_block()
        .unwrap()
        .statements()
        .unwrap();
    let assignment_statement = nodes(&ast, block_statements)[0];
    let assignment = Factory::node(&ast, assignment_statement)
        .expression()
        .unwrap();
    assert_eq!(
        Factory::node(&ast, assignment).range(),
        TextRange::new(1, 6)
    );
    assert_eq!(emit.emit_flags(assignment), ef::NO_COMMENTS);
    let target = Factory::node(&ast, assignment)
        .as_binary_expression()
        .unwrap()
        .left()
        .unwrap();
    assert_eq!(emit.emit_flags(target), ef::NO_SOURCE_MAP);
}

// VisitFunctionBody ends the environment VisitParameters started; a concise
// body that receives declarations becomes `{ <declarations>; return body; }`.
#[test]
fn visit_function_body_wraps_a_concise_body_with_its_declarations() {
    let mut emit = EmitContext::new();
    let mut ast = builder(&emit);
    let expression = ident(&mut ast, "value");
    ast.set_node_range(expression, TextRange::new(4, 9));
    emit.start_variable_environment();
    let temp = ident(&mut ast, "_t");
    emit.add_variable_declaration(&mut ast, temp);
    let identity = |_: &mut NodeVisitor<'_>, node: Option<NodeId>| node;
    let body = {
        let mut visitor = NodeVisitor::new(
            Some(&identity),
            Some(&mut ast as &mut dyn RuntimeFactory),
            tsr_ast::NodeVisitorHooks::default(),
        );
        emit.visit_function_body(Some(expression), &mut visitor)
            .unwrap()
    };
    assert_eq!(kind(&ast, body), K::Block);
    assert_eq!(emit.emit_flags(expression), ef::NO_COMMENTS);
    let read = Factory::node(&ast, body);
    let block = read.as_block().unwrap();
    assert!(!block.multi_line());
    let statements = block.statements().unwrap();
    drop(read);
    let statements = nodes(&ast, statements);
    assert_eq!(statements.len(), 2);
    assert_eq!(
        declaration_names(&ast, statements[0]).1,
        vec![b"_t".to_vec()]
    );
    assert_eq!(kind(&ast, statements[1]), K::ReturnStatement);
    assert_eq!(
        Factory::node(&ast, statements[1]).range(),
        TextRange::new(4, 9)
    );
}

// VisitIterationBody puts the iteration's lexical declarations ahead of a
// non-block body in a new multi-line block.
#[test]
fn visit_iteration_body_blocks_lexical_declarations() {
    let mut emit = EmitContext::new();
    let mut ast = builder(&emit);
    let body = statement(&mut ast, "body");
    let context = emit.clone();
    let declare = move |visitor: &mut NodeVisitor<'_>, node: Option<NodeId>| {
        let name = visitor.new_identifier(JsString::from_bytes(&b"_i"[..]));
        context
            .clone()
            .add_lexical_declaration(visitor.factory_mut(), name);
        node
    };
    let result = {
        let mut visitor = NodeVisitor::new(
            Some(&declare),
            Some(&mut ast as &mut dyn RuntimeFactory),
            tsr_ast::NodeVisitorHooks::default(),
        );
        emit.visit_iteration_body(Some(body), &mut visitor).unwrap()
    };
    assert_eq!(kind(&ast, result), K::Block);
    let read = Factory::node(&ast, result);
    let block = read.as_block().unwrap();
    assert!(block.multi_line());
    let statements = block.statements().unwrap();
    drop(read);
    let statements = nodes(&ast, statements);
    assert_eq!(statements.len(), 2);
    assert_eq!(
        declaration_names(&ast, statements[0]),
        (node_flags::LET, vec![b"_i".to_vec()])
    );
    assert_eq!(statements[1], body);
}

// VisitEmbeddedStatement replaces a removed statement with an empty statement
// at the original's location, linked to it as its original.
#[test]
fn visit_embedded_statement_replaces_removed_statements() {
    let mut emit = EmitContext::new();
    let mut ast = builder(&emit);
    let body = statement(&mut ast, "body");
    ast.set_node_range(body, TextRange::new(2, 8));
    emit.set_comment_range(body, TextRange::new(1, 8));
    let remove = |_: &mut NodeVisitor<'_>, _: Option<NodeId>| None;
    let result = {
        let mut visitor = NodeVisitor::new(
            Some(&remove),
            Some(&mut ast as &mut dyn RuntimeFactory),
            tsr_ast::NodeVisitorHooks::default(),
        );
        emit.visit_embedded_statement(Some(body), &mut visitor)
            .unwrap()
    };
    assert_eq!(kind(&ast, result), K::EmptyStatement);
    assert_eq!(Factory::node(&ast, result).range(), TextRange::new(2, 8));
    assert_eq!(emit.original(result), Some(body));
    assert_eq!(emit.comment_range(result), Some(TextRange::new(1, 8)));
}

// The hooks NewNodeVisitor installs open a variable environment for the top
// level statements and merge what the visit hoisted into them.
#[test]
fn node_visitor_hooks_merge_top_level_hoisting() {
    let emit = EmitContext::new();
    let mut ast = builder(&emit);
    let use_strict = prologue(&mut ast, "use strict");
    let body = statement(&mut ast, "body");
    let statements = new_list(&mut ast, vec![use_strict, body]);
    let context = emit.clone();
    let hoist = move |visitor: &mut NodeVisitor<'_>, node: Option<NodeId>| {
        if node.is_some_and(|node| Factory::node(visitor, node).kind() == K::ExpressionStatement) {
            let name = visitor.new_identifier(JsString::from_bytes(&b"_h"[..]));
            context
                .clone()
                .add_variable_declaration(visitor.factory_mut(), name);
        }
        node
    };
    let hooks = emit.visitor_hooks();
    let visit: &NodeVisit<'_> = &hoist;
    let merged = {
        let mut visitor = hooks.new_node_visitor(Some(visit), &mut ast);
        (visitor.hooks.visit_top_level_statements.unwrap())(&mut visitor, Some(statements)).unwrap()
    };
    let merged = nodes(&ast, merged);
    assert_eq!(merged.len(), 3);
    assert_eq!(merged[0], use_strict);
    assert_eq!(
        declaration_names(&ast, merged[1]).1,
        vec![b"_h".to_vec(), b"_h".to_vec()]
    );
    assert_eq!(merged[2], body);
}

// compareEmitHelpers: the same helper or two without priority compare equal; a
// helper without priority sorts after one with; otherwise the priorities'
// difference.
#[test]
fn compare_emit_helpers_orders_by_priority_with_unprioritized_last() {
    assert_eq!(
        compare_emit_helpers(&helpers::AWAIT_HELPER, &helpers::AWAIT_HELPER),
        0
    );
    assert_eq!(
        compare_emit_helpers(&helpers::AWAIT_HELPER, &helpers::PROP_KEY_HELPER),
        0
    );
    assert_eq!(
        compare_emit_helpers(&helpers::AWAIT_HELPER, &helpers::DECORATE_HELPER),
        1
    );
    assert_eq!(
        compare_emit_helpers(&helpers::DECORATE_HELPER, &helpers::AWAIT_HELPER),
        -1
    );
    assert_eq!(
        compare_emit_helpers(&helpers::DECORATE_HELPER, &helpers::AWAITER_HELPER),
        -3
    );
    assert_eq!(
        compare_emit_helpers(&helpers::ES_DECORATE_HELPER, &helpers::DECORATE_HELPER),
        0
    );
    let mut sorted: Vec<&EmitHelper> = vec![
        &helpers::AWAIT_HELPER,
        &helpers::AWAITER_HELPER,
        &helpers::ES_DECORATE_HELPER,
        &helpers::MAKE_TEMPLATE_OBJECT_HELPER,
        &helpers::DECORATE_HELPER,
        &helpers::PROP_KEY_HELPER,
    ];
    sorted.sort_by(|x, y| compare_emit_helpers(x, y).cmp(&0));
    let expected: Vec<&EmitHelper> = vec![
        &helpers::MAKE_TEMPLATE_OBJECT_HELPER,
        &helpers::ES_DECORATE_HELPER,
        &helpers::DECORATE_HELPER,
        &helpers::AWAITER_HELPER,
        &helpers::AWAIT_HELPER,
        &helpers::PROP_KEY_HELPER,
    ];
    assert_eq!(sorted, expected);
}

// RequestEmitHelper adds dependencies first and each helper once, in request
// order; ReadEmitHelpers drains the set.
#[test]
fn requested_helpers_follow_dependencies_once() {
    let mut emit = EmitContext::new();
    emit.request_emit_helper(&helpers::IMPORT_STAR_HELPER);
    emit.request_emit_helper(&helpers::EXPORT_STAR_HELPER);
    emit.request_emit_helper(&helpers::ASYNC_DELEGATOR_HELPER);
    emit.request_emit_helper(&helpers::AWAIT_HELPER);
    let expected: Vec<&EmitHelper> = vec![
        &helpers::CREATE_BINDING_HELPER,
        &helpers::SET_MODULE_DEFAULT_HELPER,
        &helpers::IMPORT_STAR_HELPER,
        &helpers::EXPORT_STAR_HELPER,
        &helpers::AWAIT_HELPER,
        &helpers::ASYNC_DELEGATOR_HELPER,
    ];
    assert_eq!(emit.read_emit_helpers(), expected);
    assert!(emit.read_emit_helpers().is_empty());
}

#[test]
#[should_panic(expected = "Cannot request a scoped emit helper")]
fn requesting_a_scoped_helper_panics() {
    EmitContext::new().request_emit_helper(&helpers::ASYNC_SUPER_HELPER);
}

// AddEmitHelper keeps each helper once per node; MoveEmitHelpers moves the
// helpers the predicate selects, keeping the others' order on the source.
#[test]
fn node_helpers_are_unique_and_move_by_predicate() {
    let mut emit = EmitContext::new();
    let mut ast = builder(&emit);
    let source = ident(&mut ast, "source");
    let target = ident(&mut ast, "target");
    emit.add_emit_helper(
        source,
        &[
            &helpers::AWAIT_HELPER,
            &helpers::REST_HELPER,
            &helpers::AWAIT_HELPER,
            &helpers::PROP_KEY_HELPER,
        ],
    );
    emit.add_emit_helper(target, &[&helpers::REST_HELPER]);
    emit.move_emit_helpers(source, target, |helper| *helper != helpers::AWAIT_HELPER);
    let expected: Vec<&EmitHelper> = vec![&helpers::AWAIT_HELPER];
    assert_eq!(emit.get_emit_helpers(source), expected);
    let expected: Vec<&EmitHelper> = vec![&helpers::REST_HELPER, &helpers::PROP_KEY_HELPER];
    assert_eq!(emit.get_emit_helpers(target), expected);
    let empty = ident(&mut ast, "empty");
    emit.move_emit_helpers(empty, target, |_| true);
    assert!(emit.get_emit_helpers(empty).is_empty());
}

// SetOriginal copies the original's emit node (flags, comment and source-map
// ranges, token ranges, helpers, snippet element) but not its synthetic
// comments or type node; a snippet element is only copied when present.
#[test]
fn set_original_copies_the_emit_node() {
    let mut emit = EmitContext::new();
    let mut ast = builder(&emit);
    let original = ident(&mut ast, "original");
    let type_node = ast.new_keyword_type_node(K::NumberKeyword.into());
    emit.set_source_map_range(original, TextRange::new(2, 4));
    emit.set_token_source_map_range(original, K::OpenParenToken.into(), TextRange::new(5, 6));
    emit.add_emit_helper(original, &[&helpers::REST_HELPER]);
    emit.set_type_node(original, Some(type_node));
    emit.add_synthetic_leading_comment(
        original,
        K::SingleLineCommentTrivia,
        JsString::from_bytes(&b"c"[..]),
        false,
    );
    let node = ident(&mut ast, "node");
    let snippet = SnippetElement {
        kind: SnippetKind::TabStop,
        order: 3,
    };
    emit.set_snippet_element(node, snippet);
    emit.set_emit_flags(node, ef::SINGLE_LINE);
    emit.set_comment_range(node, TextRange::new(7, 8));
    emit.set_original(node, original);
    // The original had no flags or comment range: the copy clears them.
    assert_eq!(emit.emit_flags(node), 0);
    assert_eq!(emit.comment_range(node), None);
    assert_eq!(emit.source_map_range(&ast, node), TextRange::new(2, 4));
    assert_eq!(
        emit.token_source_map_range(node, K::OpenParenToken.into()),
        Some(TextRange::new(5, 6))
    );
    assert_eq!(
        emit.token_source_map_range(node, K::CloseParenToken.into()),
        None
    );
    let expected: Vec<&EmitHelper> = vec![&helpers::REST_HELPER];
    assert_eq!(emit.get_emit_helpers(node), expected);
    assert_eq!(emit.snippet_element(node), Some(snippet));
    assert_eq!(emit.get_type_node(node), None);
    assert!(emit.synthetic_leading_comments(node).is_empty());

    // An original without an emit node copies nothing.
    let bare = ident(&mut ast, "bare");
    let other = ident(&mut ast, "other");
    emit.set_emit_flags(other, ef::NO_COMMENTS);
    emit.set_original(other, bare);
    assert_eq!(emit.emit_flags(other), ef::NO_COMMENTS);

    emit.unset_original(other);
    assert_eq!(emit.original(other), None);
    // A node without a custom source-map range reports its own location.
    ast.set_node_range(bare, TextRange::new(9, 12));
    assert_eq!(emit.source_map_range(&ast, bare), TextRange::new(9, 12));
}

// AssignCommentAndSourceMapRanges copies both effective ranges, falling back to
// the source node's location.
#[test]
fn assign_comment_and_source_map_ranges_uses_effective_ranges() {
    let mut emit = EmitContext::new();
    let mut ast = builder(&emit);
    let from = ident(&mut ast, "from");
    ast.set_node_range(from, TextRange::new(1, 5));
    emit.set_source_map_range(from, TextRange::new(2, 3));
    let to = ident(&mut ast, "to");
    emit.assign_comment_and_source_map_ranges(&ast, to, from);
    assert_eq!(emit.comment_range(to), Some(TextRange::new(1, 5)));
    assert_eq!(emit.source_map_range(&ast, to), TextRange::new(2, 3));
}

// ParseNode is the most original node when it is not synthesized; the external
// helpers name and the recorded-helpers test read it from there.
#[test]
fn parse_node_and_external_helpers_follow_the_original_chain() {
    let mut emit = EmitContext::new();
    let mut parsed = AstBuilder::new(
        tsr_jsstring::SourceText::default(),
        &tsr_arena::Counters::new(),
    );
    let parse_tree = ident(&mut parsed, "parsed");
    assert_eq!(emit.parse_node(&parsed, parse_tree), Some(parse_tree));
    let mut ast = builder(&emit);
    let synthesized = ident(&mut ast, "synthesized");
    assert_eq!(emit.parse_node(&ast, synthesized), None);
    assert!(!emit.has_recorded_external_helpers(&ast, synthesized));

    let wrapper = ident(&mut parsed, "wrapper");
    emit.set_original(wrapper, parse_tree);
    assert!(!emit.has_recorded_external_helpers(&parsed, wrapper));
    let name = ident(&mut parsed, "tslib_1");
    emit.set_external_helpers_module_name(&parsed, wrapper, Some(name));
    assert_eq!(
        emit.get_external_helpers_module_name(&parsed, parse_tree),
        Some(name)
    );
    assert!(emit.has_recorded_external_helpers(&parsed, wrapper));
}

// IsCallToHelper requires a call whose callee is an identifier carrying the
// HelperName flag and the given text.
#[test]
fn calls_to_helpers_need_the_helper_name_flag() {
    let mut emit = EmitContext::new();
    let mut ast = builder(&emit);
    let value = ident(&mut ast, "value");
    let call = emit.new_await_helper(&mut ast, value);
    assert!(emit.is_call_to_helper(&ast, call, b"__await"));
    assert!(!emit.is_call_to_helper(&ast, call, b"__awaiter"));
    let plain = ident(&mut ast, "__await");
    let arguments = new_list(&mut ast, Vec::new());
    let plain_call = ast.new_call_expression(Some(plain), None, None, Some(arguments), 0);
    assert!(!emit.is_call_to_helper(&ast, plain_call, b"__await"));
    assert!(!emit.is_call_to_helper(&ast, value, b"__await"));
    let expected: Vec<&EmitHelper> = vec![&helpers::AWAIT_HELPER];
    assert_eq!(emit.read_emit_helpers(), expected);
}

fn comma_parts(ast: &AstBuilder, node: NodeId) -> Option<(NodeId, NodeId)> {
    let read = Factory::node(ast, node);
    let binary = read.as_binary_expression()?;
    let operator = binary.operator_token().unwrap();
    (Factory::node(ast, operator).kind() == K::CommaToken)
        .then(|| (binary.left().unwrap(), binary.right().unwrap()))
}

// InlineExpressions: nil for none, the expression itself for one, otherwise a
// left-associated comma chain over the expressions with synthesized comma
// expressions flattened (a comma with a source location is kept whole).
#[test]
fn inline_expressions_flattens_synthesized_commas() {
    let emit = EmitContext::new();
    let mut ast = builder(&emit);
    assert_eq!(emit.inline_expressions(&mut ast, &[]), None);
    let a = ident(&mut ast, "a");
    assert_eq!(emit.inline_expressions(&mut ast, &[a]), Some(a));
    let b = ident(&mut ast, "b");
    let c = ident(&mut ast, "c");
    let d = ident(&mut ast, "d");
    let e = ident(&mut ast, "e");
    let synthesized = emit.new_comma_expression(&mut ast, b, c);
    let parsed = emit.new_comma_expression(&mut ast, d, e);
    ast.set_node_range(parsed, TextRange::new(0, 4));
    let result = emit
        .inline_expressions(&mut ast, &[a, synthesized, parsed])
        .unwrap();
    let (left, right) = comma_parts(&ast, result).unwrap();
    assert_eq!(right, parsed);
    let (left, right) = comma_parts(&ast, left).unwrap();
    assert_eq!(right, c);
    assert_eq!(comma_parts(&ast, left).unwrap(), (a, b));
}

// CreateExpressionFromEntityName: `a.b.c` as nested property accesses of
// clones, each with the qualified name's location; a plain identifier is a
// clone at its location.
#[test]
fn entity_names_become_property_accesses_of_clones() {
    let emit = EmitContext::new();
    let mut ast = builder(&emit);
    let a = ident(&mut ast, "a");
    let b = ident(&mut ast, "b");
    let c = ident(&mut ast, "c");
    ast.set_node_range(b, TextRange::new(2, 3));
    let inner = ast.new_qualified_name(Some(a), Some(b));
    ast.set_node_range(inner, TextRange::new(0, 3));
    let outer = ast.new_qualified_name(Some(inner), Some(c));
    ast.set_node_range(outer, TextRange::new(0, 5));
    let result = emit.create_expression_from_entity_name(&mut ast, outer);
    assert_eq!(kind(&ast, result), K::PropertyAccessExpression);
    assert_eq!(Factory::node(&ast, result).range(), TextRange::new(0, 5));
    let read = Factory::node(&ast, result);
    let access = read.as_property_access_expression().unwrap();
    let (object, name) = (access.expression().unwrap(), access.name().unwrap());
    drop(read);
    assert_ne!(name, c);
    assert_eq!(identifier_text(&ast, name), b"c");
    assert_eq!(emit.original(name), Some(c));
    assert_eq!(Factory::node(&ast, object).range(), TextRange::new(0, 3));
    let read = Factory::node(&ast, object);
    let access = read.as_property_access_expression().unwrap();
    let (object, name) = (access.expression().unwrap(), access.name().unwrap());
    drop(read);
    assert_eq!(Factory::node(&ast, name).range(), TextRange::new(2, 3));
    assert_eq!(identifier_text(&ast, object), b"a");
    assert_eq!(emit.original(object), Some(a));
}

// RestoreEnclosingLabel rebuilds each label around the new statement, keeping
// the labels; without a label the statement is returned.
#[test]
fn restore_enclosing_label_rewraps_nested_labels() {
    let emit = EmitContext::new();
    let mut ast = builder(&emit);
    let node = statement(&mut ast, "node");
    assert_eq!(emit.restore_enclosing_label(&mut ast, node, None), node);
    let body = statement(&mut ast, "body");
    let inner_label = ident(&mut ast, "inner");
    let inner = ast.new_labeled_statement(Some(inner_label), Some(body));
    let outer_label = ident(&mut ast, "outer");
    let outer = ast.new_labeled_statement(Some(outer_label), Some(inner));
    let result = emit.restore_enclosing_label(&mut ast, node, Some(outer));
    let read = Factory::node(&ast, result);
    let labeled = read.as_labeled_statement().unwrap();
    let (label, statement) = (labeled.label(), labeled.statement().unwrap());
    drop(read);
    assert_eq!(label, Some(outer_label));
    assert_eq!(emit.original(result), Some(outer));
    let read = Factory::node(&ast, statement);
    let labeled = read.as_labeled_statement().unwrap();
    assert_eq!(labeled.label(), Some(inner_label));
    assert_eq!(labeled.statement(), Some(node));
}

// EnsureUseStrict only examines the first statement.
#[test]
fn ensure_use_strict_checks_only_the_first_statement() {
    let emit = EmitContext::new();
    let mut ast = builder(&emit);
    let use_strict = prologue(&mut ast, "use strict");
    let other = statement(&mut ast, "other");
    let other_prologue = prologue(&mut ast, "other");
    assert_eq!(
        emit.ensure_use_strict(&mut ast, vec![use_strict, other]),
        vec![use_strict, other]
    );
    for statements in [
        Vec::new(),
        vec![other, use_strict],
        vec![other_prologue, use_strict],
    ] {
        let result = emit.ensure_use_strict(&mut ast, statements.clone());
        assert_eq!(result.len(), statements.len() + 1);
        assert_eq!(&result[1..], &statements[..]);
        let expression = Factory::node(&ast, result[0]).expression().unwrap();
        assert_eq!(
            Factory::node(&ast, expression)
                .as_string_literal()
                .unwrap()
                .text(),
            b"use strict"
        );
    }
}

// SplitStandardPrologue splits at the first non-directive. SplitCustomPrologue
// splits at the first directive or non-custom statement; when every statement
// is a custom prologue it returns no prologue and the whole slice as the rest.
#[test]
fn prologue_splitters_follow_upstream_boundaries() {
    let mut emit = EmitContext::new();
    let mut ast = builder(&emit);
    let p1 = prologue(&mut ast, "a");
    let p2 = prologue(&mut ast, "b");
    let s = statement(&mut ast, "s");
    let source = [p1, p2, s, p1];
    assert_eq!(
        emit.split_standard_prologue(&ast, &source),
        (&source[..2], &source[2..])
    );
    assert_eq!(
        emit.split_standard_prologue(&ast, &source[..2]),
        (&source[..2], &[][..])
    );

    let c1 = hoisted_var(&mut emit, &mut ast, "c1");
    let c2 = statement(&mut ast, "c2");
    custom(&mut emit, c2);
    let custom_source = [c1, c2, s];
    assert_eq!(
        emit.split_custom_prologue(&ast, &custom_source),
        (&custom_source[..2], &custom_source[2..])
    );
    let directive_first = [p1, c1];
    assert_eq!(
        emit.split_custom_prologue(&ast, &directive_first),
        (&[][..], &directive_first[..])
    );
    let all_custom = [c1, c2];
    assert_eq!(
        emit.split_custom_prologue(&ast, &all_custom),
        (&[][..], &all_custom[..])
    );
}

// RestoreOuterExpressions rebuilds the outer expressions of the requested
// kinds around the new inner expression, stopping at an ignorable synthesized
// parenthesis (no source location, source-map or comment range).
#[test]
fn restore_outer_expressions_skips_ignorable_parentheses() {
    let mut emit = EmitContext::new();
    let mut ast = builder(&emit);
    let x = ident(&mut ast, "x");
    let type_node = ast.new_keyword_type_node(K::NumberKeyword.into());
    let as_expression = ast.new_as_expression(Some(x), Some(type_node));
    let parenthesized = ast.new_parenthesized_expression(Some(as_expression));
    let inner = ident(&mut ast, "inner");

    // Synthesized parentheses are ignorable: the inner expression comes back.
    assert_eq!(
        emit.restore_outer_expressions(&mut ast, Some(parenthesized), inner, oek::ALL)
            .unwrap(),
        inner
    );
    // Kinds that exclude the outer expression's kind restore nothing.
    ast.set_node_range(parenthesized, TextRange::new(0, 10));
    assert_eq!(
        emit.restore_outer_expressions(&mut ast, Some(parenthesized), inner, oek::ASSERTIONS)
            .unwrap(),
        inner
    );
    // Parentheses with a location are restored, and below them every outer
    // expression kind.
    let restored = emit
        .restore_outer_expressions(&mut ast, Some(parenthesized), inner, oek::PARENTHESES)
        .unwrap();
    assert_eq!(emit.original(restored), Some(parenthesized));
    let restored_as = Factory::node(&ast, restored).expression().unwrap();
    assert_eq!(kind(&ast, restored_as), K::AsExpression);
    assert_eq!(emit.original(restored_as), Some(as_expression));
    let read = Factory::node(&ast, restored_as);
    assert_eq!(read.expression(), Some(inner));
    assert_eq!(read.type_node(), Some(type_node));
    drop(read);

    // A synthesized parenthesis with a custom comment range is not ignorable.
    let synthesized = ast.new_parenthesized_expression(Some(x));
    emit.set_comment_range(synthesized, TextRange::new(1, 2));
    let restored = emit
        .restore_outer_expressions(&mut ast, Some(synthesized), inner, oek::ALL)
        .unwrap();
    assert_eq!(Factory::node(&ast, restored).expression(), Some(inner));
    assert_eq!(
        emit.restore_outer_expressions(&mut ast, None, inner, oek::ALL)
            .unwrap(),
        inner
    );
}

fn binary_parts(ast: &AstBuilder, node: NodeId) -> (NodeId, tsr_ast::NodeKind, NodeId) {
    let read = Factory::node(ast, node);
    let binary = read.as_binary_expression().unwrap();
    let operator = binary.operator_token().unwrap();
    (
        binary.left().unwrap(),
        Factory::node(ast, operator).kind(),
        binary.right().unwrap(),
    )
}

// NewTypeCheck: `x === null`, `x === void 0`, or `typeof x === "tag"`.
#[test]
fn type_checks_by_tag() {
    let emit = EmitContext::new();
    let mut ast = builder(&emit);
    let x = ident(&mut ast, "x");
    let null = emit.new_type_check(&mut ast, x, b"null");
    let (left, operator, right) = binary_parts(&ast, null);
    assert_eq!((left, operator), (x, K::EqualsEqualsEqualsToken.into()));
    assert_eq!(kind(&ast, right), K::NullKeyword);

    let undefined = emit.new_type_check(&mut ast, x, b"undefined");
    let (_, _, right) = binary_parts(&ast, undefined);
    assert_eq!(kind(&ast, right), K::VoidExpression);
    let zero = Factory::node(&ast, right).expression().unwrap();
    assert_eq!(
        Factory::node(&ast, zero)
            .as_numeric_literal()
            .unwrap()
            .text(),
        b"0"
    );

    let symbol = emit.new_type_check(&mut ast, x, b"symbol");
    let (left, _, right) = binary_parts(&ast, symbol);
    assert_eq!(kind(&ast, left), K::TypeOfExpression);
    assert_eq!(Factory::node(&ast, left).expression(), Some(x));
    assert_eq!(
        Factory::node(&ast, right)
            .as_string_literal()
            .unwrap()
            .text(),
        b"symbol"
    );
}

// CreateForOfBindingStatement: a declaration list becomes a `var`-style
// statement whose first declaration takes the bound value, keeping the list's
// flags; any other target becomes an assignment statement at its location.
#[test]
fn for_of_binding_statements() {
    let emit = EmitContext::new();
    let mut ast = builder(&emit);
    let value = ident(&mut ast, "value");
    let name = ident(&mut ast, "item");
    let declaration = ast.new_variable_declaration(Some(name), None, None, None);
    let declarations = new_list(&mut ast, vec![declaration]);
    let list = ast.new_variable_declaration_list(Some(declarations), node_flags::LET);
    ast.set_node_range(list, TextRange::new(5, 13));
    let statement = emit.create_for_of_binding_statement(&mut ast, list, value);
    assert_eq!(kind(&ast, statement), K::VariableStatement);
    assert_eq!(
        Factory::node(&ast, statement).range(),
        TextRange::new(5, 13)
    );
    let (flags, names) = declaration_names(&ast, statement);
    assert_eq!(flags & node_flags::LET, node_flags::LET);
    assert_eq!(names, vec![b"item".to_vec()]);
    let updated_list = Factory::node(&ast, statement)
        .as_variable_statement()
        .unwrap()
        .declaration_list()
        .unwrap();
    assert_eq!(emit.original(updated_list), Some(list));

    let target = ident(&mut ast, "target");
    ast.set_node_range(target, TextRange::new(1, 7));
    let statement = emit.create_for_of_binding_statement(&mut ast, target, value);
    assert_eq!(kind(&ast, statement), K::ExpressionStatement);
    assert_eq!(Factory::node(&ast, statement).range(), TextRange::new(1, 7));
    let assignment = Factory::node(&ast, statement).expression().unwrap();
    assert_eq!(
        Factory::node(&ast, assignment).range(),
        TextRange::new(1, 7)
    );
    assert_eq!(
        binary_parts(&ast, assignment),
        (target, K::EqualsToken.into(), value)
    );
}

// NewMethodCall preserves an optional-chain call object's optionality.
#[test]
fn method_calls_preserve_optional_chains() {
    let emit = EmitContext::new();
    let mut ast = builder(&emit);
    let callee = ident(&mut ast, "f");
    let arguments = new_list(&mut ast, Vec::new());
    let optional = ast.new_call_expression(
        Some(callee),
        None,
        None,
        Some(arguments),
        node_flags::OPTIONAL_CHAIN,
    );
    let method = ident(&mut ast, "m");
    let call = emit.new_method_call(&mut ast, optional, method, Vec::new());
    assert_eq!(
        Factory::node(&ast, call).flags() & node_flags::OPTIONAL_CHAIN,
        node_flags::OPTIONAL_CHAIN
    );
    let plain = emit.new_array_slice_call(&mut ast, callee, 2);
    assert_eq!(
        Factory::node(&ast, plain).flags() & node_flags::OPTIONAL_CHAIN,
        0
    );
    let arguments = Factory::node(&ast, plain)
        .as_call_expression()
        .unwrap()
        .arguments()
        .unwrap();
    let arguments = nodes(&ast, arguments);
    assert_eq!(
        Factory::node(&ast, arguments[0])
            .as_numeric_literal()
            .unwrap()
            .text(),
        b"2"
    );
    let no_start = emit.new_array_slice_call(&mut ast, callee, 0);
    let arguments = Factory::node(&ast, no_start)
        .as_call_expression()
        .unwrap()
        .arguments()
        .unwrap();
    assert!(nodes(&ast, arguments).is_empty());
}

// NewStringLiteralFromNode copies the text of the kinds it lists (empty text
// otherwise) and records the source node.
#[test]
fn string_literals_from_nodes_record_their_source() {
    let mut emit = EmitContext::new();
    let mut ast = builder(&emit);
    let name = ident(&mut ast, "name");
    let literal = emit.new_string_literal_from_node(&mut ast, name);
    assert_eq!(
        Factory::node(&ast, literal)
            .as_string_literal()
            .unwrap()
            .text(),
        b"name"
    );
    assert_eq!(emit.text_source(literal), Some(name));
    let other = statement(&mut ast, "s");
    let literal = emit.new_string_literal_from_node(&mut ast, other);
    assert_eq!(
        Factory::node(&ast, literal)
            .as_string_literal()
            .unwrap()
            .text(),
        b""
    );
}

// GetLocalName clones the declaration's name with LocalName, NoComments and
// NoSourceMap unless comments or source maps are allowed.
#[test]
fn local_names_are_flagged_clones() {
    let mut emit = EmitContext::new();
    let mut ast = builder(&emit);
    let name = ident(&mut ast, "f");
    let function =
        ast.new_function_declaration(None, None, Some(name), None, None, None, None, None);
    let local = emit.get_local_name(&mut ast, Some(function)).unwrap();
    assert_ne!(local, name);
    assert_eq!(identifier_text(&ast, local), b"f");
    assert_eq!(
        emit.emit_flags(local),
        ef::LOCAL_NAME | ef::NO_COMMENTS | ef::NO_SOURCE_MAP
    );
    let export = emit
        .get_export_name_ex(
            &mut ast,
            Some(function),
            AssignedNameOptions {
                allow_comments: true,
                ..AssignedNameOptions::default()
            },
        )
        .unwrap();
    assert_eq!(emit.emit_flags(export), ef::EXPORT_NAME | ef::NO_SOURCE_MAP);
    assert_eq!(
        emit.get_local_name(&mut ast, None),
        Err(crate::Error::Unsupported("NewGeneratedNameForNode(nil)"))
    );
}

// The helper calls request their helpers and call `__name` with the arguments
// in upstream's order; optional trailing arguments are omitted when absent.
#[test]
fn helper_calls_request_helpers_and_pass_arguments() {
    let mut emit = EmitContext::new();
    let mut ast = builder(&emit);
    let receiver = ident(&mut ast, "receiver");
    let state = ident(&mut ast, "state");
    let call = emit.new_class_private_field_get_helper(
        &mut ast,
        receiver,
        state,
        PrivateIdentifierKind::Accessor,
        None,
    );
    let read = Factory::node(&ast, call);
    let data = read.as_call_expression().unwrap();
    let (callee, arguments) = (data.expression().unwrap(), data.arguments().unwrap());
    drop(read);
    assert_eq!(identifier_text(&ast, callee), b"__classPrivateFieldGet");
    assert_eq!(emit.emit_flags(callee), ef::HELPER_NAME);
    let arguments = nodes(&ast, arguments);
    assert_eq!(arguments.len(), 3);
    assert_eq!(&arguments[..2], &[receiver, state]);
    assert_eq!(
        Factory::node(&ast, arguments[2])
            .as_string_literal()
            .unwrap()
            .text(),
        b"a"
    );

    let generator = ident(&mut ast, "generator");
    emit.new_async_generator_helper(&mut ast, generator, false);
    assert_eq!(
        emit.emit_flags(generator),
        ef::ASYNC_FUNCTION_BODY | ef::REUSE_TEMP_VARIABLE_SCOPE
    );
    let expected: Vec<&EmitHelper> = vec![
        &helpers::CLASS_PRIVATE_FIELD_GET_HELPER,
        &helpers::AWAIT_HELPER,
        &helpers::ASYNC_GENERATOR_HELPER,
    ];
    assert_eq!(emit.read_emit_helpers(), expected);
}

// GetEmitContext hands out a reset context; releasing it clears the tables a
// retained clone shares.
#[test]
fn pooled_contexts_are_reset_on_release() {
    let mut ast = AstBuilder::new(
        tsr_jsstring::SourceText::default(),
        &tsr_arena::Counters::new(),
    );
    let node = ident(&mut ast, "node");
    let retained = {
        let mut pooled = get_emit_context();
        pooled.set_emit_flags(node, ef::SINGLE_LINE);
        pooled.request_emit_helper(&helpers::AWAIT_HELPER);
        (*pooled).clone()
    };
    assert_eq!(retained.emit_flags(node), 0);
    assert_eq!(retained.metadata_entries(), 0);
}
