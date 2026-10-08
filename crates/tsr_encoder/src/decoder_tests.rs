//! The decoder tests of tsc/internal/api/encoder/decoder_test.go: a source
//! file parsed, encoded by the Rust encoder and decoded again, then read
//! through the decoded tree as the Go tests read theirs.
use crate::{decode_nodes, decode_source_file, encode_node, encode_source_file, DecodedTree};
use tsr_arena::Counters;
use tsr_ast::{
    node_flags, AstFile, AstView, NodeId, NodeListId, SourceFileParseOptions, SyntaxKind as K,
};

fn parse(text: &str) -> AstFile {
    tsr_parser::parse_source_file(
        tsr_jsstring::SourceText::from_loaded_bytes(text.as_bytes().to_vec()),
        tsr_core::ScriptKind::TS,
        SourceFileParseOptions {
            file_name: tsr_ast::JsString::from_bytes(b"/test.ts".as_slice()),
            path: tsr_ast::JsString::from_bytes(b"/test.ts".as_slice()),
            ..Default::default()
        },
    )
    .publish_unbound()
}

fn decoded(text: &str) -> DecodedTree {
    let file = parse(text);
    let encoded = encode_source_file(
        file.view(),
        file.root().expect("parsed root"),
        &mut tsr_parser::ParserJsDocProvider::default(),
    )
    .expect("encodes");
    decode_source_file(&encoded.bytes, &Counters::new()).expect("decodes")
}

fn root(tree: &DecodedTree) -> NodeId {
    tree.root.expect("decoded root")
}

fn nodes(view: AstView<'_>, list: Option<NodeListId>) -> Vec<NodeId> {
    let list = list.expect("the list is present");
    view.node_slice(view.list(list).expect("list").nodes())
        .expect("list nodes")
        .iter()
        .flatten()
        .collect()
}

fn kind(view: AstView<'_>, node: NodeId) -> K {
    view.node(node)
        .expect("decoded node")
        .kind()
        .known()
        .expect("a known kind")
}

fn identifier_text(view: AstView<'_>, node: NodeId) -> String {
    let read = view.node(node).expect("decoded node");
    let data = read.data_source();
    let identifier = data.as_identifier().expect("an identifier");
    String::from_utf8(identifier.text().to_vec()).expect("UTF-8")
}

fn statements(tree: &DecodedTree) -> Vec<NodeId> {
    let view = tree.builder.view();
    let read = view.node(root(tree)).expect("root");
    let data = read.data_source();
    let file = data.as_source_file().expect("a source file");
    nodes(view, file.statements())
}

/// The first variable declaration of `let x = ...;`-shaped text.
fn first_declaration(tree: &DecodedTree) -> NodeId {
    let view = tree.builder.view();
    let statement = statements(tree)[0];
    let read = view.node(statement).expect("statement");
    let data = read.data_source();
    let list = data
        .as_variable_statement()
        .expect("a variable statement")
        .declaration_list()
        .expect("a declaration list");
    let read = view.node(list).expect("list node");
    let data = read.data_source();
    let declarations = nodes(
        view,
        data.as_variable_declaration_list()
            .expect("a declaration list")
            .declarations(),
    );
    assert_eq!(declarations.len(), 1);
    declarations[0]
}

fn initializer(tree: &DecodedTree) -> NodeId {
    let view = tree.builder.view();
    let declaration = first_declaration(tree);
    let read = view.node(declaration).expect("declaration");
    let data = read.data_source();
    data.as_variable_declaration()
        .expect("a declaration")
        .initializer()
        .expect("an initializer")
}

/// The pin's `TestDecodeSourceFile_Basic`.
#[test]
fn a_decoded_source_file_keeps_its_name_text_statements_and_end_token() {
    let tree = decoded("let x = 1;");
    let view = tree.builder.view();
    assert_eq!(kind(view, root(&tree)), K::SourceFile);
    let source = view.source_file(root(&tree)).expect("source file");
    assert_eq!(source.file_name(), b"/test.ts");
    assert_eq!(source.text().as_bytes(), b"let x = 1;");
    let read = view.node(root(&tree)).expect("root");
    let data = read.data_source();
    let file = data.as_source_file().expect("a source file");
    assert!(file.statements().is_some());
    assert!(file.end_of_file_token().is_some());
}

/// The pin's `TestDecodeSourceFile_Statements`.
#[test]
fn statements_decode_in_order() {
    let tree = decoded("let a = 1;\nlet b = 2;\nlet c = 3;");
    let view = tree.builder.view();
    let statements = statements(&tree);
    assert_eq!(statements.len(), 3);
    for (index, statement) in statements.iter().enumerate() {
        assert_eq!(
            kind(view, *statement),
            K::VariableStatement,
            "statement {index}"
        );
    }
}

/// The pin's `TestDecodeSourceFile_VariableDeclaration`.
#[test]
fn a_variable_declaration_keeps_its_name_and_initializer() {
    let tree = decoded("let x = 1;");
    let view = tree.builder.view();
    let declaration = first_declaration(&tree);
    let read = view.node(declaration).expect("declaration");
    let data = read.data_source();
    let declaration = data.as_variable_declaration().expect("a declaration");
    let name = declaration.name().expect("a name");
    assert_eq!(kind(view, name), K::Identifier);
    assert_eq!(identifier_text(view, name), "x");
    let initializer = declaration.initializer().expect("an initializer");
    assert_eq!(kind(view, initializer), K::NumericLiteral);
    let read = view.node(initializer).expect("initializer");
    let data = read.data_source();
    assert_eq!(
        data.as_numeric_literal().expect("a numeric literal").text(),
        b"1"
    );
}

/// The pin's `TestDecodeSourceFile_VariableDeclarationListFlags`.
#[test]
fn declaration_list_flags_survive_decoding() {
    for (code, expected) in [
        ("const x = 1;", node_flags::CONST),
        ("let x = 1;", node_flags::LET),
        ("var x = 1;", 0),
    ] {
        let tree = decoded(code);
        let view = tree.builder.view();
        let statement = statements(&tree)[0];
        let read = view.node(statement).expect("statement");
        let data = read.data_source();
        let list = data
            .as_variable_statement()
            .expect("a variable statement")
            .declaration_list()
            .expect("a declaration list");
        let flags = view.node(list).expect("list").flags() & (node_flags::LET | node_flags::CONST);
        assert_eq!(flags, expected, "flags for {code:?}");
    }
}

/// The pin's `TestDecodeSourceFile_FunctionDeclaration`.
#[test]
fn a_function_declaration_keeps_its_parts() {
    let tree = decoded("function add(a: number, b: number): number { return a + b; }");
    let view = tree.builder.view();
    let statement = statements(&tree)[0];
    let read = view.node(statement).expect("statement");
    let data = read.data_source();
    let function = data.as_function_declaration().expect("a function");
    assert_eq!(
        identifier_text(view, function.name().expect("a name")),
        "add"
    );
    let parameters = nodes(view, function.parameters());
    assert_eq!(parameters.len(), 2);
    assert!(function.r#type().is_some());
    assert!(function.body().is_some());
    let read = view.node(parameters[0]).expect("parameter");
    let data = read.data_source();
    let parameter = data.as_parameter_declaration().expect("a parameter");
    assert_eq!(
        identifier_text(view, parameter.name().expect("a name")),
        "a"
    );
    assert!(parameter.r#type().is_some());
}

/// The pin's `TestDecodeSourceFile_ImportDeclaration`.
#[test]
fn an_import_declaration_keeps_its_clause_and_specifier() {
    let tree = decoded(r#"import { bar } from "bar";"#);
    let view = tree.builder.view();
    let statement = statements(&tree)[0];
    let read = view.node(statement).expect("statement");
    let data = read.data_source();
    let import = data.as_import_declaration().expect("an import");
    let clause = import.import_clause().expect("an import clause");
    let specifier = import.module_specifier().expect("a module specifier");
    let read = view.node(specifier).expect("specifier");
    let data = read.data_source();
    assert_eq!(
        data.as_string_literal().expect("a string literal").text(),
        b"bar"
    );
    let read = view.node(clause).expect("clause");
    let data = read.data_source();
    let bindings = data
        .as_import_clause()
        .expect("an import clause")
        .named_bindings()
        .expect("named bindings");
    let read = view.node(bindings).expect("bindings");
    let data = read.data_source();
    let elements = nodes(
        view,
        data.as_named_imports().expect("named imports").elements(),
    );
    assert_eq!(elements.len(), 1);
    let read = view.node(elements[0]).expect("element");
    let data = read.data_source();
    let name = data
        .as_import_specifier()
        .expect("an import specifier")
        .name()
        .expect("a name");
    assert_eq!(identifier_text(view, name), "bar");
}

/// The pin's `TestDecodeSourceFile_IfStatement`.
#[test]
fn an_if_statement_keeps_both_branches() {
    let tree = decoded("if (true) { } else { }");
    let view = tree.builder.view();
    let statement = statements(&tree)[0];
    let read = view.node(statement).expect("statement");
    let data = read.data_source();
    let if_statement = data.as_if_statement().expect("an if statement");
    assert!(if_statement.expression().is_some());
    let then = if_statement.then_statement().expect("a then branch");
    let otherwise = if_statement.else_statement().expect("an else branch");
    assert_eq!(kind(view, then), K::Block);
    assert_eq!(kind(view, otherwise), K::Block);
}

/// The pin's `TestDecodeSourceFile_TemplateExpression`.
#[test]
fn a_template_expression_keeps_its_head_and_spans() {
    let tree = decoded("let x = `hello ${name} world`;");
    let view = tree.builder.view();
    let initializer = initializer(&tree);
    let read = view.node(initializer).expect("initializer");
    let data = read.data_source();
    let template = data.as_template_expression().expect("a template");
    let head = template.head().expect("a head");
    let read = view.node(head).expect("head");
    let data = read.data_source();
    assert_eq!(
        data.as_template_head().expect("a template head").text(),
        b"hello "
    );
    let spans = nodes(view, template.template_spans());
    assert_eq!(spans.len(), 1);
    let read = view.node(spans[0]).expect("span");
    let data = read.data_source();
    let span = data.as_template_span().expect("a span");
    assert_eq!(
        kind(view, span.expression().expect("an expression")),
        K::Identifier
    );
    let literal = span.literal().expect("a literal");
    let read = view.node(literal).expect("literal");
    let data = read.data_source();
    assert_eq!(
        data.as_template_tail().expect("a template tail").text(),
        b" world"
    );
}

/// The pin's `TestDecodeSourceFile_ExportModifier`.
#[test]
fn an_export_modifier_decodes_as_a_keyword() {
    let tree = decoded("export function foo() {}");
    let view = tree.builder.view();
    let statement = statements(&tree)[0];
    let read = view.node(statement).expect("statement");
    let data = read.data_source();
    let modifiers = nodes(
        view,
        data.as_function_declaration()
            .expect("a function")
            .modifiers(),
    );
    assert_eq!(modifiers.len(), 1);
    assert_eq!(kind(view, modifiers[0]), K::ExportKeyword);
}

/// The pin's `TestDecodeSourceFile_Positions`.
#[test]
fn the_root_spans_the_whole_text() {
    let code = "let x = 1;";
    let tree = decoded(code);
    let view = tree.builder.view();
    let read = view.node(root(&tree)).expect("root");
    assert_eq!(read.pos(), 0);
    assert_eq!(read.end(), i32::try_from(code.len()).unwrap());
}

/// The pin's `TestDecodeSourceFile_ClassDeclaration`.
#[test]
fn a_class_declaration_keeps_its_name_and_members() {
    let tree = decoded("class Foo { bar(): void {} }");
    let view = tree.builder.view();
    let statement = statements(&tree)[0];
    let read = view.node(statement).expect("statement");
    let data = read.data_source();
    let class = data.as_class_declaration().expect("a class");
    assert_eq!(identifier_text(view, class.name().expect("a name")), "Foo");
    let members = nodes(view, class.members());
    assert_eq!(members.len(), 1);
    assert_eq!(kind(view, members[0]), K::MethodDeclaration);
}

/// The pin's `TestDecodeNodes_SubtreeRoundTrip`.
#[test]
fn a_subtree_round_trips_through_decode_nodes() {
    let file = parse("function greet(name: string) { return `Hello, ${name}!`; }");
    let view = file.view();
    let root = file.root().expect("parsed root");
    let read = view.node(root).expect("root");
    let data = read.data_source();
    let statements = nodes(
        view,
        data.as_source_file().expect("a source file").statements(),
    );
    let function = statements
        .iter()
        .copied()
        .find(|node| kind(view, *node) == K::FunctionDeclaration)
        .expect("the function");
    let encoded = encode_node(
        view,
        function,
        Some(root),
        &mut tsr_parser::ParserJsDocProvider::default(),
    )
    .expect("encodes");
    let tree = decode_nodes(&encoded.bytes, &Counters::new()).expect("decodes");
    let view = tree.builder.view();
    let decoded = tree.root.expect("decoded root");
    assert_eq!(kind(view, decoded), K::FunctionDeclaration);
    let read = view.node(decoded).expect("function");
    let data = read.data_source();
    let function = data.as_function_declaration().expect("a function");
    assert_eq!(
        identifier_text(view, function.name().expect("a name")),
        "greet"
    );
    assert_eq!(nodes(view, function.parameters()).len(), 1);
    assert!(function.body().is_some());
}

/// The pin's `TestDecodeSourceFile_BinaryExpression`.
#[test]
fn a_binary_expression_keeps_both_operands_and_its_operator() {
    let tree = decoded("let x = 1 + 2;");
    let view = tree.builder.view();
    let initializer = initializer(&tree);
    let read = view.node(initializer).expect("initializer");
    let data = read.data_source();
    let binary = data.as_binary_expression().expect("a binary expression");
    let left = binary.left().expect("a left operand");
    let right = binary.right().expect("a right operand");
    assert!(binary.operator_token().is_some());
    assert_eq!(kind(view, left), K::NumericLiteral);
    assert_eq!(kind(view, right), K::NumericLiteral);
}

/// The pin's `TestDecodeSourceFile_KeywordExpressions`.
#[test]
fn this_decodes_as_a_keyword_expression() {
    let tree = decoded("const x = this;");
    let view = tree.builder.view();
    let initializer = initializer(&tree);
    assert_eq!(kind(view, initializer), K::ThisKeyword);
    let read = view.node(initializer).expect("initializer");
    assert!(read.data_source().as_keyword_expression().is_some());
}

/// The pin's `TestDecodeSourceFile_EmptyModuleBlock`.
#[test]
fn an_empty_module_block_keeps_its_empty_statement_list() {
    let tree = decoded("namespace N { }");
    let view = tree.builder.view();
    let statement = statements(&tree)[0];
    let read = view.node(statement).expect("statement");
    let data = read.data_source();
    let body = data
        .as_module_declaration()
        .expect("a module declaration")
        .body()
        .expect("a body");
    let read = view.node(body).expect("body");
    let data = read.data_source();
    let block = data.as_module_block().expect("a module block");
    assert!(block.statements().is_some());
    assert_eq!(nodes(view, block.statements()).len(), 0);
}

/// The pin's `TestDecodeSourceFile_EmptyBlockAndParams`.
#[test]
fn empty_parameter_and_statement_lists_decode_as_present_lists() {
    let tree = decoded("function foo() {}");
    let view = tree.builder.view();
    let statement = statements(&tree)[0];
    let read = view.node(statement).expect("statement");
    let data = read.data_source();
    let function = data.as_function_declaration().expect("a function");
    assert!(function.parameters().is_some());
    assert_eq!(nodes(view, function.parameters()).len(), 0);
    let body = function.body().expect("a body");
    let read = view.node(body).expect("body");
    let data = read.data_source();
    let block = data.as_block().expect("a block");
    assert!(block.statements().is_some());
    assert_eq!(nodes(view, block.statements()).len(), 0);
}

/// The pin's `TestDecodeSourceFile_ArrowFunctionEmptyParams`.
#[test]
fn an_arrow_function_keeps_its_empty_parameter_list() {
    let tree = decoded("const f = () => {};");
    let view = tree.builder.view();
    let initializer = initializer(&tree);
    let read = view.node(initializer).expect("initializer");
    let data = read.data_source();
    let arrow = data.as_arrow_function().expect("an arrow function");
    assert!(arrow.parameters().is_some());
    assert_eq!(nodes(view, arrow.parameters()).len(), 0);
    let body = arrow.body().expect("a body");
    let read = view.node(body).expect("body");
    let data = read.data_source();
    let block = data.as_block().expect("a block");
    assert!(block.statements().is_some());
    assert_eq!(nodes(view, block.statements()).len(), 0);
}

/// The pin's `TestDecodeSourceFile_FunctionExpressionEmptyParams`.
#[test]
fn a_function_expression_keeps_its_empty_parameter_list() {
    let tree = decoded("const f = function() {};");
    let view = tree.builder.view();
    let initializer = initializer(&tree);
    let read = view.node(initializer).expect("initializer");
    let data = read.data_source();
    let function = data
        .as_function_expression()
        .expect("a function expression");
    assert!(function.parameters().is_some());
    assert_eq!(nodes(view, function.parameters()).len(), 0);
}

fn second_statement_expression(tree: &DecodedTree) -> NodeId {
    let view = tree.builder.view();
    let statement = statements(tree)[1];
    let read = view.node(statement).expect("statement");
    let data = read.data_source();
    data.as_expression_statement()
        .expect("an expression statement")
        .expression()
        .expect("an expression")
}

/// The pin's `TestDecodeSourceFile_PostfixUnaryOperator`.
#[test]
fn a_postfix_increment_keeps_its_operator_and_operand() {
    let tree = decoded("let i = 0; i++;");
    let view = tree.builder.view();
    let expression = second_statement_expression(&tree);
    let read = view.node(expression).expect("expression");
    let data = read.data_source();
    let postfix = data
        .as_postfix_unary_expression()
        .expect("a postfix expression");
    assert_eq!(postfix.operator(), K::PlusPlusToken);
    assert_eq!(
        kind(view, postfix.operand().expect("an operand")),
        K::Identifier
    );
}

/// The pin's `TestDecodeSourceFile_PrefixUnaryOperator`.
#[test]
fn a_prefix_negation_keeps_its_operator_and_operand() {
    let tree = decoded("let x = true; !x;");
    let view = tree.builder.view();
    let expression = second_statement_expression(&tree);
    let read = view.node(expression).expect("expression");
    let data = read.data_source();
    let prefix = data
        .as_prefix_unary_expression()
        .expect("a prefix expression");
    assert_eq!(prefix.operator(), K::ExclamationToken);
    assert_eq!(
        kind(view, prefix.operand().expect("an operand")),
        K::Identifier
    );
}

/// The pin's `TestDecodeSourceFile_PostfixDecrement`.
#[test]
fn a_postfix_decrement_keeps_its_operator() {
    let tree = decoded("let n = 5; n--;");
    let view = tree.builder.view();
    let expression = second_statement_expression(&tree);
    let read = view.node(expression).expect("expression");
    let data = read.data_source();
    let postfix = data
        .as_postfix_unary_expression()
        .expect("a postfix expression");
    assert_eq!(postfix.operator(), K::MinusMinusToken);
}
