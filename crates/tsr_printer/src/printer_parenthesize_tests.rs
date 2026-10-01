//! The tests of `tsc/internal/printer/printer_test.go` after `TestEmit`: the
//! `TestParenthesize*` family over factory-built trees, and the transform and
//! name-generation tests. Every tree is built as upstream builds it, with a
//! factory without hooks; every node and list keeps the undefined range, which
//! is what `parsetestutil.MarkSyntheticRecursive` sets upstream.
#![allow(
    clippy::unnecessary_wraps,
    reason = "the helpers return the optional children the factory takes"
)]

use crate::printer_emit_tests::{check_emit, parse_type_script};
use crate::{EmitContext, Printer, PrinterOptions};
use tsr_ast::{
    node_flags, AstBuilder, FactoryMethods, JsString, NodeId, NodeListId, NodeVisitor,
    SourceFileParseOptions, SyntaxKind as K,
};
use tsr_core::{NewLineKind, TextRange};
use tsr_jsstring::SourceText;

type N = Option<NodeId>;

struct F {
    ast: AstBuilder,
}

impl F {
    fn new() -> Self {
        Self {
            ast: AstBuilder::new(
                SourceText::from_loaded_bytes(&b""[..]),
                &tsr_arena::Counters::new(),
            ),
        }
    }

    fn id(&mut self, text: &str) -> N {
        Some(
            self.ast
                .new_identifier(JsString::from_bytes(text.as_bytes())),
        )
    }

    fn tok(&mut self, kind: K) -> N {
        Some(self.ast.new_token(kind.into()))
    }

    fn list(&mut self, nodes: Vec<N>) -> Option<NodeListId> {
        let slice = self.ast.node_slice(nodes).expect("list slice");
        Some(
            self.ast
                .new_list(TextRange::new(-1, -1), slice)
                .expect("list"),
        )
    }

    fn type_ref(&mut self, name: &str) -> N {
        let name = self.id(name);
        Some(self.ast.new_type_reference_node(name, None))
    }

    fn binary(&mut self, left: N, operator: K, right: N) -> N {
        let operator = self.tok(operator);
        Some(
            self.ast
                .new_binary_expression(None, left, None, operator, right),
        )
    }

    fn binary_ids(&mut self, left: &str, operator: K, right: &str) -> N {
        let (left, right) = (self.id(left), self.id(right));
        self.binary(left, operator, right)
    }

    fn statement(&mut self, expression: N) -> N {
        Some(self.ast.new_expression_statement(expression))
    }

    fn optional_chain_a_b(&mut self) -> N {
        let (a, question_dot, b) = (self.id("a"), self.tok(K::QuestionDotToken), self.id("b"));
        Some(self.ast.new_property_access_expression(
            a,
            question_dot,
            b,
            node_flags::OPTIONAL_CHAIN,
        ))
    }

    fn new_without_arguments(&mut self, name: &str) -> N {
        let name = self.id(name);
        Some(self.ast.new_new_expression(name, None, None))
    }

    fn empty_arrow_function(&mut self) -> N {
        let parameters = self.list(vec![]);
        let arrow = self.tok(K::EqualsGreaterThanToken);
        let statements = self.list(vec![]);
        let body = Some(self.ast.new_block(statements, false));
        Some(
            self.ast
                .new_arrow_function(None, None, parameters, None, None, arrow, body),
        )
    }

    fn type_alias(&mut self, type_node: N) -> N {
        let name = self.id("_");
        Some(
            self.ast
                .new_type_alias_declaration(None, name, None, type_node),
        )
    }

    fn union(&mut self, types: Vec<N>) -> N {
        let types = self.list(types);
        Some(self.ast.new_union_type_node(types))
    }

    fn infer_extends(&mut self, name: &str, constraint: &str) -> N {
        let (name, constraint) = (self.id(name), self.type_ref(constraint));
        let parameter = Some(
            self.ast
                .new_type_parameter_declaration(None, name, constraint, None, None),
        );
        Some(self.ast.new_infer_type_node(parameter))
    }

    fn function_type(&mut self, return_type: N) -> N {
        let parameters = self.list(vec![]);
        Some(
            self.ast
                .new_function_type_node(None, parameters, return_type),
        )
    }

    /// Builds the file around `statements` and runs `emittestutil.CheckEmit`.
    fn check(mut self, statements: Vec<N>, expected: &str) {
        let statements = self.list(statements);
        let eof = self.tok(K::EndOfFile);
        let file = self.ast.new_source_file(
            SourceFileParseOptions {
                file_name: JsString::from_bytes(&b"/file.ts"[..]),
                path: JsString::from_bytes(&b"/file.ts"[..]),
                ..Default::default()
            },
            SourceText::from_loaded_bytes(&b""[..]),
            statements,
            eof,
        );
        let emit_context = EmitContext::new();
        if let Err(failure) = check_emit(&emit_context, self.ast.view(), file, false, expected) {
            panic!("{failure}");
        }
    }
}

#[test]
fn test_parenthesize_decorator() {
    let mut f = F::new();
    let expression = f.binary_ids("a", K::PlusToken, "b");
    let decorator = Some(f.ast.new_decorator(expression));
    let modifiers = f.list(vec![decorator]);
    let name = f.id("C");
    let members = f.list(vec![]);
    let class = Some(
        f.ast
            .new_class_declaration(modifiers, name, None, None, members),
    );
    f.check(vec![class], "@(a + b)\nclass C {\n}");
}

#[test]
fn test_parenthesize_computed_property_name() {
    let mut f = F::new();
    // will be parenthesized on emit:
    let expression = f.binary_ids("a", K::CommaToken, "b");
    let computed = Some(f.ast.new_computed_property_name(expression));
    let property = Some(
        f.ast
            .new_property_declaration(None, computed, None, None, None),
    );
    let members = f.list(vec![property]);
    let name = f.id("C");
    let class = Some(f.ast.new_class_declaration(None, name, None, None, members));
    f.check(vec![class], "class C {\n    [(a, b)];\n}");
}

#[test]
fn test_parenthesize_array_literal() {
    let mut f = F::new();
    // will be parenthesized on emit:
    let element = f.binary_ids("a", K::CommaToken, "b");
    let elements = f.list(vec![element]);
    let array = Some(f.ast.new_array_literal_expression(elements, false));
    let statement = f.statement(array);
    f.check(vec![statement], "[(a, b)];");
}

#[test]
fn test_parenthesize_property_access1() {
    let mut f = F::new();
    // will be parenthesized on emit:
    let expression = f.binary_ids("a", K::CommaToken, "b");
    let name = f.id("c");
    let access =
        Some(
            f.ast
                .new_property_access_expression(expression, None, name, node_flags::NONE),
        );
    let statement = f.statement(access);
    f.check(vec![statement], "(a, b).c;");
}

#[test]
fn test_parenthesize_property_access2() {
    let mut f = F::new();
    // will be parenthesized on emit:
    let expression = f.optional_chain_a_b();
    let name = f.id("c");
    let access =
        Some(
            f.ast
                .new_property_access_expression(expression, None, name, node_flags::NONE),
        );
    let statement = f.statement(access);
    f.check(vec![statement], "(a?.b).c;");
}

#[test]
fn test_parenthesize_property_access3() {
    let mut f = F::new();
    // will be parenthesized on emit:
    let expression = f.new_without_arguments("a");
    let name = f.id("b");
    let access =
        Some(
            f.ast
                .new_property_access_expression(expression, None, name, node_flags::NONE),
        );
    let statement = f.statement(access);
    f.check(vec![statement], "(new a).b;");
}

#[test]
fn test_parenthesize_element_access1() {
    let mut f = F::new();
    // will be parenthesized on emit:
    let expression = f.binary_ids("a", K::CommaToken, "b");
    let argument = f.id("c");
    let access =
        Some(
            f.ast
                .new_element_access_expression(expression, None, argument, node_flags::NONE),
        );
    let statement = f.statement(access);
    f.check(vec![statement], "(a, b)[c];");
}

#[test]
fn test_parenthesize_element_access2() {
    let mut f = F::new();
    // will be parenthesized on emit:
    let expression = f.optional_chain_a_b();
    let argument = f.id("c");
    let access =
        Some(
            f.ast
                .new_element_access_expression(expression, None, argument, node_flags::NONE),
        );
    let statement = f.statement(access);
    f.check(vec![statement], "(a?.b)[c];");
}

#[test]
fn test_parenthesize_element_access3() {
    let mut f = F::new();
    // will be parenthesized on emit:
    let expression = f.new_without_arguments("a");
    let argument = f.id("b");
    let access =
        Some(
            f.ast
                .new_element_access_expression(expression, None, argument, node_flags::NONE),
        );
    let statement = f.statement(access);
    f.check(vec![statement], "(new a)[b];");
}

fn check_call(f: F, callee: N, expected: &str) {
    let mut f = f;
    let arguments = f.list(vec![]);
    let call = Some(
        f.ast
            .new_call_expression(callee, None, None, arguments, node_flags::NONE),
    );
    let statement = f.statement(call);
    f.check(vec![statement], expected);
}

#[test]
fn test_parenthesize_call1() {
    let mut f = F::new();
    // will be parenthesized on emit:
    let callee = f.binary_ids("a", K::CommaToken, "b");
    check_call(f, callee, "(a, b)();");
}

#[test]
fn test_parenthesize_call2() {
    let mut f = F::new();
    // will be parenthesized on emit:
    let callee = f.optional_chain_a_b();
    check_call(f, callee, "(a?.b)();");
}

#[test]
fn test_parenthesize_call3() {
    let mut f = F::new();
    // will be parenthesized on emit:
    let callee = f.new_without_arguments("C");
    check_call(f, callee, "(new C)();");
}

#[test]
fn test_parenthesize_call4() {
    let mut f = F::new();
    let callee = f.id("a");
    let argument = f.binary_ids("b", K::CommaToken, "c");
    let arguments = f.list(vec![argument]);
    let call = Some(
        f.ast
            .new_call_expression(callee, None, None, arguments, node_flags::NONE),
    );
    let statement = f.statement(call);
    f.check(vec![statement], "a((b, c));");
}

#[test]
fn test_parenthesize_new1() {
    let mut f = F::new();
    // will be parenthesized on emit:
    let expression = f.binary_ids("a", K::CommaToken, "b");
    let arguments = f.list(vec![]);
    let new = Some(f.ast.new_new_expression(expression, None, arguments));
    let statement = f.statement(new);
    f.check(vec![statement], "new (a, b)();");
}

#[test]
fn test_parenthesize_new2() {
    let mut f = F::new();
    // will be parenthesized on emit:
    let callee = f.id("C");
    let arguments = f.list(vec![]);
    let call = Some(
        f.ast
            .new_call_expression(callee, None, None, arguments, node_flags::NONE),
    );
    let new = Some(f.ast.new_new_expression(call, None, None));
    let statement = f.statement(new);
    f.check(vec![statement], "new (C());");
}

#[test]
fn test_parenthesize_new3() {
    let mut f = F::new();
    let expression = f.id("C");
    let argument = f.binary_ids("a", K::CommaToken, "b");
    let arguments = f.list(vec![argument]);
    let new = Some(f.ast.new_new_expression(expression, None, arguments));
    let statement = f.statement(new);
    f.check(vec![statement], "new C((a, b));");
}

fn check_tagged_template(f: F, tag: N, expected: &str) {
    let mut f = f;
    let template = Some(
        f.ast
            .new_no_substitution_template_literal(JsString::default(), 0),
    );
    let tagged =
        Some(
            f.ast
                .new_tagged_template_expression(tag, None, None, template, node_flags::NONE),
        );
    let statement = f.statement(tagged);
    f.check(vec![statement], expected);
}

#[test]
fn test_parenthesize_tagged_template1() {
    let mut f = F::new();
    // will be parenthesized on emit:
    let tag = f.binary_ids("a", K::CommaToken, "b");
    check_tagged_template(f, tag, "(a, b) ``;");
}

#[test]
fn test_parenthesize_tagged_template2() {
    let mut f = F::new();
    // will be parenthesized on emit:
    let tag = f.optional_chain_a_b();
    check_tagged_template(f, tag, "(a?.b) ``;");
}

#[test]
fn test_parenthesize_type_assertion1() {
    let mut f = F::new();
    let type_node = f.type_ref("T");
    // will be parenthesized on emit:
    let expression = f.binary_ids("a", K::PlusToken, "b");
    let assertion = Some(f.ast.new_type_assertion(type_node, expression));
    let statement = f.statement(assertion);
    f.check(vec![statement], "<T>(a + b);");
}

#[test]
fn test_parenthesize_arrow_function1() {
    let mut f = F::new();
    let parameters = f.list(vec![]);
    let arrow = f.tok(K::EqualsGreaterThanToken);
    // will be parenthesized on emit:
    let properties = f.list(vec![]);
    let body = Some(f.ast.new_object_literal_expression(properties, false));
    let function = Some(
        f.ast
            .new_arrow_function(None, None, parameters, None, None, arrow, body),
    );
    let statement = f.statement(function);
    f.check(vec![statement], "() => ({});");
}

#[test]
fn test_parenthesize_arrow_function2() {
    let mut f = F::new();
    let parameters = f.list(vec![]);
    let arrow = f.tok(K::EqualsGreaterThanToken);
    // will be parenthesized on emit:
    let properties = f.list(vec![]);
    let object = Some(f.ast.new_object_literal_expression(properties, false));
    let name = f.id("a");
    let body = Some(
        f.ast
            .new_property_access_expression(object, None, name, node_flags::NONE),
    );
    let function = Some(
        f.ast
            .new_arrow_function(None, None, parameters, None, None, arrow, body),
    );
    let statement = f.statement(function);
    f.check(vec![statement], "() => ({}.a);");
}

#[test]
fn test_parenthesize_delete() {
    let mut f = F::new();
    // will be parenthesized on emit:
    let operand = f.binary_ids("a", K::PlusToken, "b");
    let expression = Some(f.ast.new_delete_expression(operand));
    let statement = f.statement(expression);
    f.check(vec![statement], "delete (a + b);");
}

#[test]
fn test_parenthesize_void() {
    let mut f = F::new();
    // will be parenthesized on emit:
    let operand = f.binary_ids("a", K::PlusToken, "b");
    let expression = Some(f.ast.new_void_expression(operand));
    let statement = f.statement(expression);
    f.check(vec![statement], "void (a + b);");
}

#[test]
fn test_parenthesize_type_of() {
    let mut f = F::new();
    // will be parenthesized on emit:
    let operand = f.binary_ids("a", K::PlusToken, "b");
    let expression = Some(f.ast.new_type_of_expression(operand));
    let statement = f.statement(expression);
    f.check(vec![statement], "typeof (a + b);");
}

#[test]
fn test_parenthesize_await() {
    let mut f = F::new();
    // will be parenthesized on emit:
    let operand = f.binary_ids("a", K::PlusToken, "b");
    let expression = Some(f.ast.new_await_expression(operand));
    let statement = f.statement(expression);
    f.check(vec![statement], "await (a + b);");
}

/// `isBinaryOperator` of printer_test.go.
fn is_binary_operator(token: K) -> bool {
    matches!(
        token,
        K::CommaToken
            | K::LessThanToken
            | K::GreaterThanToken
            | K::LessThanEqualsToken
            | K::GreaterThanEqualsToken
            | K::EqualsEqualsToken
            | K::EqualsEqualsEqualsToken
            | K::ExclamationEqualsToken
            | K::ExclamationEqualsEqualsToken
            | K::PlusToken
            | K::MinusToken
            | K::AsteriskToken
            | K::AsteriskAsteriskToken
            | K::SlashToken
            | K::PercentToken
            | K::LessThanLessThanToken
            | K::GreaterThanGreaterThanToken
            | K::GreaterThanGreaterThanGreaterThanToken
            | K::AmpersandToken
            | K::BarToken
            | K::CaretToken
            | K::AmpersandAmpersandToken
            | K::BarBarToken
            | K::QuestionQuestionToken
            | K::EqualsToken
            | K::PlusEqualsToken
            | K::MinusEqualsToken
            | K::AsteriskEqualsToken
            | K::AsteriskAsteriskEqualsToken
            | K::SlashEqualsToken
            | K::PercentEqualsToken
            | K::LessThanLessThanEqualsToken
            | K::GreaterThanGreaterThanEqualsToken
            | K::GreaterThanGreaterThanGreaterThanEqualsToken
            | K::AmpersandEqualsToken
            | K::BarEqualsToken
            | K::BarBarEqualsToken
            | K::AmpersandAmpersandEqualsToken
            | K::QuestionQuestionEqualsToken
            | K::CaretEqualsToken
            | K::InKeyword
            | K::InstanceOfKeyword
    )
}

/// `makeSide` of printer_test.go.
fn make_side(f: &mut F, label: &str, kind: K) -> N {
    if kind == K::Identifier || kind == K::Unknown {
        f.id(label)
    } else if kind == K::ArrowFunction {
        f.empty_arrow_function()
    } else if is_binary_operator(kind) {
        let (left, right) = (format!("{label}l"), format!("{label}r"));
        f.binary_ids(&left, kind, &right)
    } else {
        panic!("unsupported kind")
    }
}

#[test]
fn test_parenthesize_binary() {
    let data = [
        (K::Unknown, K::CommaToken, K::Unknown, "l, r"),
        (K::PlusToken, K::CommaToken, K::Unknown, "ll + lr, r"),
        (K::PlusToken, K::AsteriskToken, K::Unknown, "(ll + lr) * r"),
        (K::Unknown, K::AsteriskToken, K::PlusToken, "l * (rl + rr)"),
        (K::AsteriskToken, K::PlusToken, K::Unknown, "ll * lr + r"),
        (K::Unknown, K::PlusToken, K::AsteriskToken, "l + rl * rr"),
        (K::AsteriskToken, K::SlashToken, K::Unknown, "ll * lr / r"),
        (
            K::AsteriskAsteriskToken,
            K::SlashToken,
            K::Unknown,
            "ll ** lr / r",
        ),
        (
            K::AsteriskToken,
            K::AsteriskAsteriskToken,
            K::Unknown,
            "(ll * lr) ** r",
        ),
        (
            K::AsteriskAsteriskToken,
            K::AsteriskAsteriskToken,
            K::Unknown,
            "(ll ** lr) ** r",
        ),
        (
            K::Unknown,
            K::AsteriskToken,
            K::AsteriskToken,
            "l * rl * rr",
        ),
        (K::Unknown, K::BarToken, K::BarToken, "l | rl | rr"),
        (
            K::Unknown,
            K::AmpersandToken,
            K::AmpersandToken,
            "l & rl & rr",
        ),
        (K::Unknown, K::CaretToken, K::CaretToken, "l ^ rl ^ rr"),
        (
            K::Unknown,
            K::AmpersandAmpersandToken,
            K::ArrowFunction,
            "l && (() => { })",
        ),
    ];
    for (left, operator, right, output) in data {
        let mut f = F::new();
        let left = make_side(&mut f, "l", left);
        let right = make_side(&mut f, "r", right);
        let binary = f.binary(left, operator, right);
        let statement = f.statement(binary);
        f.check(vec![statement], &format!("{output};"));
    }
}

fn check_conditional(f: F, parts: [N; 3], expected: &str) {
    let mut f = f;
    let (question, colon) = (f.tok(K::QuestionToken), f.tok(K::ColonToken));
    let conditional = Some(
        f.ast
            .new_conditional_expression(parts[0], question, parts[1], colon, parts[2]),
    );
    let statement = f.statement(conditional);
    f.check(vec![statement], expected);
}

#[test]
fn test_parenthesize_conditional1() {
    let mut f = F::new();
    // will be parenthesized on emit:
    let condition = f.binary_ids("a", K::CommaToken, "b");
    let (c, d) = (f.id("c"), f.id("d"));
    check_conditional(f, [condition, c, d], "(a, b) ? c : d;");
}

#[test]
fn test_parenthesize_conditional2() {
    let mut f = F::new();
    // will be parenthesized on emit:
    let condition = f.binary_ids("a", K::EqualsToken, "b");
    let (c, d) = (f.id("c"), f.id("d"));
    check_conditional(f, [condition, c, d], "(a = b) ? c : d;");
}

#[test]
fn test_parenthesize_conditional3() {
    let mut f = F::new();
    // will be parenthesized on emit:
    let condition = f.empty_arrow_function();
    let (a, b) = (f.id("a"), f.id("b"));
    check_conditional(f, [condition, a, b], "(() => { }) ? a : b;");
}

#[test]
fn test_parenthesize_conditional4() {
    let mut f = F::new();
    // will be parenthesized on emit:
    let condition = Some(f.ast.new_yield_expression(None, None));
    let (a, b) = (f.id("a"), f.id("b"));
    check_conditional(f, [condition, a, b], "(yield) ? a : b;");
}

#[test]
fn test_parenthesize_conditional5() {
    let mut f = F::new();
    let a = f.id("a");
    // will be parenthesized on emit:
    let when_true = f.binary_ids("b", K::CommaToken, "c");
    let d = f.id("d");
    check_conditional(f, [a, when_true, d], "a ? (b, c) : d;");
}

#[test]
fn test_parenthesize_conditional6() {
    let mut f = F::new();
    let (a, b) = (f.id("a"), f.id("b"));
    // will be parenthesized on emit:
    let when_false = f.binary_ids("c", K::CommaToken, "d");
    check_conditional(f, [a, b, when_false], "a ? b : (c, d);");
}

#[test]
fn test_parenthesize_yield1() {
    let mut f = F::new();
    // will be parenthesized on emit:
    let expression = f.binary_ids("a", K::CommaToken, "b");
    let yield_expression = Some(f.ast.new_yield_expression(None, expression));
    let statement = f.statement(yield_expression);
    f.check(vec![statement], "yield (a, b);");
}

#[test]
fn test_parenthesize_spread_element1() {
    let mut f = F::new();
    // will be parenthesized on emit:
    let expression = f.binary_ids("a", K::CommaToken, "b");
    let spread = Some(f.ast.new_spread_element(expression));
    let elements = f.list(vec![spread]);
    let array = Some(f.ast.new_array_literal_expression(elements, false));
    let statement = f.statement(array);
    f.check(vec![statement], "[...(a, b)];");
}

#[test]
fn test_parenthesize_spread_element2() {
    let mut f = F::new();
    let callee = f.id("a");
    // will be parenthesized on emit:
    let expression = f.binary_ids("b", K::CommaToken, "c");
    let spread = Some(f.ast.new_spread_element(expression));
    let arguments = f.list(vec![spread]);
    let call = Some(
        f.ast
            .new_call_expression(callee, None, None, arguments, node_flags::NONE),
    );
    let statement = f.statement(call);
    f.check(vec![statement], "a(...(b, c));");
}

#[test]
fn test_parenthesize_spread_element3() {
    let mut f = F::new();
    let callee = f.id("a");
    // will be parenthesized on emit:
    let expression = f.binary_ids("b", K::CommaToken, "c");
    let spread = Some(f.ast.new_spread_element(expression));
    let arguments = f.list(vec![spread]);
    let new = Some(f.ast.new_new_expression(callee, None, arguments));
    let statement = f.statement(new);
    f.check(vec![statement], "new a(...(b, c));");
}

#[test]
fn test_parenthesize_expression_with_type_arguments() {
    let mut f = F::new();
    // will be parenthesized on emit:
    let expression = f.binary_ids("a", K::CommaToken, "b");
    let type_argument = f.type_ref("c");
    let type_arguments = f.list(vec![type_argument]);
    let node = Some(
        f.ast
            .new_expression_with_type_arguments(expression, type_arguments),
    );
    let statement = f.statement(node);
    f.check(vec![statement], "(a, b)<c>;");
}

#[test]
fn test_parenthesize_as_expression() {
    let mut f = F::new();
    // will be parenthesized on emit:
    let expression = f.binary_ids("a", K::CommaToken, "b");
    let type_node = f.type_ref("c");
    let node = Some(f.ast.new_as_expression(expression, type_node));
    let statement = f.statement(node);
    f.check(vec![statement], "(a, b) as c;");
}

#[test]
fn test_parenthesize_satisfies_expression() {
    let mut f = F::new();
    // will be parenthesized on emit:
    let expression = f.binary_ids("a", K::CommaToken, "b");
    let type_node = f.type_ref("c");
    let node = Some(f.ast.new_satisfies_expression(expression, type_node));
    let statement = f.statement(node);
    f.check(vec![statement], "(a, b) satisfies c;");
}

#[test]
fn test_parenthesize_non_null_expression() {
    let mut f = F::new();
    // will be parenthesized on emit:
    let expression = f.binary_ids("a", K::CommaToken, "b");
    let node = Some(f.ast.new_non_null_expression(expression, node_flags::NONE));
    let statement = f.statement(node);
    f.check(vec![statement], "(a, b)!;");
}

#[test]
fn test_parenthesize_expression_statement1() {
    let mut f = F::new();
    let properties = f.list(vec![]);
    let object = Some(f.ast.new_object_literal_expression(properties, false));
    let statement = f.statement(object);
    f.check(vec![statement], "({});");
}

fn empty_function_expression(f: &mut F) -> N {
    let parameters = f.list(vec![]);
    let statements = f.list(vec![]);
    let body = Some(f.ast.new_block(statements, false));
    Some(
        f.ast
            .new_function_expression(None, None, None, None, parameters, None, None, body),
    )
}

fn empty_class_expression(f: &mut F) -> N {
    let members = f.list(vec![]);
    Some(f.ast.new_class_expression(None, None, None, None, members))
}

#[test]
fn test_parenthesize_expression_statement2() {
    let mut f = F::new();
    let function = empty_function_expression(&mut f);
    let statement = f.statement(function);
    f.check(vec![statement], "(function () { });");
}

#[test]
fn test_parenthesize_expression_statement3() {
    let mut f = F::new();
    let class = empty_class_expression(&mut f);
    let statement = f.statement(class);
    f.check(vec![statement], "class {\n};");
}

#[test]
fn test_parenthesize_expression_default1() {
    let mut f = F::new();
    // will be parenthesized on emit:
    let class = empty_class_expression(&mut f);
    let export = Some(f.ast.new_export_assignment(None, false, None, class));
    f.check(vec![export], "export default (class {\n});");
}

#[test]
fn test_parenthesize_expression_default2() {
    let mut f = F::new();
    // will be parenthesized on emit:
    let function = empty_function_expression(&mut f);
    let export = Some(f.ast.new_export_assignment(None, false, None, function));
    f.check(vec![export], "export default (function () { });");
}

#[test]
fn test_parenthesize_expression_default3() {
    let mut f = F::new();
    // will be parenthesized on emit:
    let expression = f.binary_ids("a", K::CommaToken, "b");
    let export = Some(f.ast.new_export_assignment(None, false, None, expression));
    f.check(vec![export], "export default (a, b);");
}

#[test]
fn test_parenthesize_array_type() {
    let mut f = F::new();
    // will be parenthesized on emit:
    let (a, b) = (f.type_ref("a"), f.type_ref("b"));
    let union = f.union(vec![a, b]);
    let array = Some(f.ast.new_array_type_node(union));
    let alias = f.type_alias(array);
    f.check(vec![alias], "type _ = (a | b)[];");
}

#[test]
fn test_parenthesize_optional_type() {
    let mut f = F::new();
    // will be parenthesized on emit:
    let (a, b) = (f.type_ref("a"), f.type_ref("b"));
    let union = f.union(vec![a, b]);
    let optional = Some(f.ast.new_optional_type_node(union));
    let elements = f.list(vec![optional]);
    let tuple = Some(f.ast.new_tuple_type_node(elements));
    let alias = f.type_alias(tuple);
    f.check(vec![alias], "type _ = [\n    (a | b)?\n];");
}

#[test]
fn test_parenthesize_union_type1() {
    let mut f = F::new();
    let a = f.type_ref("a");
    // will be parenthesized on emit:
    let b = f.type_ref("b");
    let function = f.function_type(b);
    let union = f.union(vec![a, function]);
    let alias = f.type_alias(union);
    f.check(vec![alias], "type _ = a | (() => b);");
}

#[test]
fn test_parenthesize_union_type2() {
    let mut f = F::new();
    // will be parenthesized on emit:
    let infer = f.infer_extends("a", "b");
    let c = f.type_ref("c");
    let union = f.union(vec![infer, c]);
    let alias = f.type_alias(union);
    f.check(vec![alias], "type _ = (infer a extends b) | c;");
}

#[test]
fn test_parenthesize_intersection_type() {
    let mut f = F::new();
    let a = f.type_ref("a");
    // will be parenthesized on emit:
    let (b, c) = (f.type_ref("b"), f.type_ref("c"));
    let union = f.union(vec![b, c]);
    let types = f.list(vec![a, union]);
    let intersection = Some(f.ast.new_intersection_type_node(types));
    let alias = f.type_alias(intersection);
    f.check(vec![alias], "type _ = a & (b | c);");
}

#[test]
fn test_parenthesize_readonly_type_operator1() {
    let mut f = F::new();
    // will be parenthesized on emit:
    let (a, b) = (f.type_ref("a"), f.type_ref("b"));
    let union = f.union(vec![a, b]);
    let operator = Some(
        f.ast
            .new_type_operator_node(K::ReadonlyKeyword.into(), union),
    );
    let alias = f.type_alias(operator);
    f.check(vec![alias], "type _ = readonly (a | b);");
}

#[test]
fn test_parenthesize_readonly_type_operator2() {
    let mut f = F::new();
    // will be parenthesized on emit:
    let a = f.type_ref("a");
    let keyof = Some(f.ast.new_type_operator_node(K::KeyOfKeyword.into(), a));
    let operator = Some(
        f.ast
            .new_type_operator_node(K::ReadonlyKeyword.into(), keyof),
    );
    let alias = f.type_alias(operator);
    f.check(vec![alias], "type _ = readonly (keyof a);");
}

#[test]
fn test_parenthesize_keyof_type_operator() {
    let mut f = F::new();
    // will be parenthesized on emit:
    let (a, b) = (f.type_ref("a"), f.type_ref("b"));
    let union = f.union(vec![a, b]);
    let operator = Some(f.ast.new_type_operator_node(K::KeyOfKeyword.into(), union));
    let alias = f.type_alias(operator);
    f.check(vec![alias], "type _ = keyof (a | b);");
}

#[test]
fn test_parenthesize_indexed_access_type() {
    let mut f = F::new();
    // will be parenthesized on emit:
    let (a, b) = (f.type_ref("a"), f.type_ref("b"));
    let union = f.union(vec![a, b]);
    let c = f.type_ref("c");
    let access = Some(f.ast.new_indexed_access_type_node(union, c));
    let alias = f.type_alias(access);
    f.check(vec![alias], "type _ = (a | b)[c];");
}

#[test]
fn test_parenthesize_conditional_type1() {
    let mut f = F::new();
    // will be parenthesized on emit:
    let a = f.type_ref("a");
    let function = f.function_type(a);
    let (b, c, d) = (f.type_ref("b"), f.type_ref("c"), f.type_ref("d"));
    let conditional = Some(f.ast.new_conditional_type_node(function, b, c, d));
    let alias = f.type_alias(conditional);
    f.check(vec![alias], "type _ = (() => a) extends b ? c : d;");
}

#[test]
fn test_parenthesize_conditional_type2() {
    let mut f = F::new();
    let a = f.type_ref("a");
    // will be parenthesized on emit:
    let (b, c, d, e) = (
        f.type_ref("b"),
        f.type_ref("c"),
        f.type_ref("d"),
        f.type_ref("e"),
    );
    let inner = Some(f.ast.new_conditional_type_node(b, c, d, e));
    let (g_f, g) = (f.type_ref("f"), f.type_ref("g"));
    let conditional = Some(f.ast.new_conditional_type_node(a, inner, g_f, g));
    let alias = f.type_alias(conditional);
    f.check(
        vec![alias],
        "type _ = a extends (b extends c ? d : e) ? f : g;",
    );
}

#[test]
fn test_parenthesize_conditional_type3() {
    let mut f = F::new();
    let a = f.type_ref("a");
    // will be parenthesized on emit:
    let infer = f.infer_extends("b", "c");
    let function = f.function_type(infer);
    let (d, e) = (f.type_ref("d"), f.type_ref("e"));
    let conditional = Some(f.ast.new_conditional_type_node(a, function, d, e));
    let alias = f.type_alias(conditional);
    f.check(
        vec![alias],
        "type _ = a extends () => (infer b extends c) ? d : e;",
    );
}

#[test]
fn test_parenthesize_conditional_type4() {
    let mut f = F::new();
    let a = f.type_ref("a");
    // will be parenthesized on emit:
    let infer = f.infer_extends("b", "c");
    let d = f.type_ref("d");
    let union = f.union(vec![infer, d]);
    let function = f.function_type(union);
    let (e, g_f) = (f.type_ref("e"), f.type_ref("f"));
    let conditional = Some(f.ast.new_conditional_type_node(a, function, e, g_f));
    let alias = f.type_alias(conditional);
    f.check(
        vec![alias],
        "type _ = a extends () => (infer b extends c) | d ? e : f;",
    );
}

/// `TestNameGeneration`: two `NewTempVariable` declarations, one at file level
/// and one in a function body, each named `_a` in its own scope.
#[test]
fn test_name_generation() {
    let mut ec = EmitContext::new();
    let mut ast = AstBuilder::with_hooks(
        SourceText::from_loaded_bytes(&b""[..]),
        &tsr_arena::Counters::new(),
        ec.factory_hooks(),
    );
    let temp_declaration = |ast: &mut AstBuilder, ec: &mut EmitContext| {
        let temp = ec.new_temp_variable(ast);
        let declaration = ast.new_variable_declaration(Some(temp), None, None, None);
        let nodes = ast.node_slice(vec![Some(declaration)]).expect("list slice");
        let declarations = ast.new_list(TextRange::new(-1, -1), nodes).expect("list");
        let list = ast.new_variable_declaration_list(Some(declarations), node_flags::NONE);
        ast.new_variable_statement(None, Some(list))
    };
    let outer = temp_declaration(&mut ast, &mut ec);
    let name = ast.new_identifier(JsString::from_bytes(&b"f"[..]));
    let parameters = ast.node_slice(vec![]).expect("list slice");
    let parameters = ast
        .new_list(TextRange::new(-1, -1), parameters)
        .expect("list");
    let inner = temp_declaration(&mut ast, &mut ec);
    let body = ast.node_slice(vec![Some(inner)]).expect("list slice");
    let body = ast.new_list(TextRange::new(-1, -1), body).expect("list");
    let body = ast.new_block(Some(body), true);
    let function = ast.new_function_declaration(
        None,
        None,
        Some(name),
        None,
        Some(parameters),
        None,
        None,
        Some(body),
    );
    let statements = ast
        .node_slice(vec![Some(outer), Some(function)])
        .expect("list slice");
    let statements = ast
        .new_list(TextRange::new(-1, -1), statements)
        .expect("list");
    let eof = ast.new_token(K::EndOfFile.into());
    let file = ast.new_source_file(
        SourceFileParseOptions {
            file_name: JsString::from_bytes(&b"/file.ts"[..]),
            path: JsString::from_bytes(&b"/file.ts"[..]),
            ..Default::default()
        },
        SourceText::from_loaded_bytes(&b""[..]),
        Some(statements),
        Some(eof),
    );
    if let Err(failure) = check_emit(
        &ec,
        ast.view(),
        file,
        false,
        "var _a;\nfunction f() {\n    var _a;\n}",
    ) {
        panic!("{failure}");
    }
}

/// Parses `input`, replaces every `a!` with `a` through the emit context's
/// node visitor, and checks the printed file.
fn check_non_null_removed(input: &str, expected: &str) {
    let emit_context = EmitContext::new();
    let mut factory = AstBuilder::with_hooks(
        SourceText::default(),
        &tsr_arena::Counters::new(),
        emit_context.factory_hooks(),
    );
    let parsed = parse_type_script(input.as_bytes(), false);
    let root = parsed.root();
    factory.retain_file(parsed.publish_unbound());
    let hooks = emit_context.visitor_hooks();
    let visit = |visitor: &mut NodeVisitor<'_>, node: Option<NodeId>| -> Option<NodeId> {
        let id = node?;
        let read = visitor.factory().node(id);
        if read.kind() == K::NonNullExpression {
            read.expression()
        } else {
            visitor.visit_each_child(node)
        }
    };
    let file = hooks
        .new_node_visitor(Some(&visit), &mut factory)
        .visit_source_file(root);
    if let Err(failure) = check_emit(&emit_context, factory.view(), file, false, expected) {
        panic!("{failure}");
    }
}

/// `TestNoTrailingCommaAfterTransform`: a visitor replaces `a!` with `a`.
#[test]
fn test_no_trailing_comma_after_transform() {
    check_non_null_removed("[a!]", "[a];");
}

/// `TestTrailingCommaAfterTransform`: a visitor replaces `a!` with `a`.
#[test]
fn test_trailing_comma_after_transform() {
    check_non_null_removed("[a!,]", "[a,];");
}

#[test]
fn test_parenthesize_binary_expression_mixing_nullish_coalescing() {
    let tests = [
        // inner ?? on left side of || or &&
        (
            "BarBarWithLeftQuestionQuestion",
            K::QuestionQuestionToken,
            K::BarBarToken,
            "left",
            "(a ?? b) || c;",
        ),
        (
            "AmpersandAmpersandWithLeftQuestionQuestion",
            K::QuestionQuestionToken,
            K::AmpersandAmpersandToken,
            "left",
            "(a ?? b) && c;",
        ),
        // inner ?? on right side of || or &&
        (
            "BarBarWithRightQuestionQuestion",
            K::QuestionQuestionToken,
            K::BarBarToken,
            "right",
            "a || (b ?? c);",
        ),
        (
            "AmpersandAmpersandWithRightQuestionQuestion",
            K::QuestionQuestionToken,
            K::AmpersandAmpersandToken,
            "right",
            "a && (b ?? c);",
        ),
        // inner || or && on left side of ??
        (
            "QuestionQuestionWithLeftBarBar",
            K::BarBarToken,
            K::QuestionQuestionToken,
            "left",
            "(a || b) ?? c;",
        ),
        (
            "QuestionQuestionWithLeftAmpersandAmpersand",
            K::AmpersandAmpersandToken,
            K::QuestionQuestionToken,
            "left",
            "(a && b) ?? c;",
        ),
        // inner || or && on right side of ??
        (
            "QuestionQuestionWithRightBarBar",
            K::BarBarToken,
            K::QuestionQuestionToken,
            "right",
            "a ?? (b || c);",
        ),
        (
            "QuestionQuestionWithRightAmpersandAmpersand",
            K::AmpersandAmpersandToken,
            K::QuestionQuestionToken,
            "right",
            "a ?? (b && c);",
        ),
    ];
    for (title, inner_op, outer_op, side, output) in tests {
        let mut f = F::new();
        // Upstream builds the inner expression over `a` and `b` and, for the
        // right side, replaces its operands with `b` and `c` before printing.
        let outer = if side == "left" {
            let inner = f.binary_ids("a", inner_op, "b");
            let c = f.id("c");
            f.binary(inner, outer_op, c)
        } else {
            let a = f.id("a");
            let inner = f.binary_ids("b", inner_op, "c");
            f.binary(a, outer_op, inner)
        };
        let statement = f.statement(outer);
        let file_check = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            f.check(vec![statement], output);
        }));
        assert!(file_check.is_ok(), "{title}");
    }
}

#[test]
fn test_omit_trailing_semicolon() {
    let counters = tsr_arena::Counters::new();
    let mut ast = AstBuilder::new(SourceText::from_loaded_bytes(&b""[..]), &counters);
    let name = Some(ast.new_identifier(JsString::from_bytes(&b"m"[..])));
    let slice = ast.node_slice(vec![]).expect("slice");
    let parameters = Some(ast.new_list(TextRange::new(-1, -1), slice).expect("list"));
    let void = Some(ast.new_keyword_type_node(K::VoidKeyword.into()));
    let method_signature =
        ast.new_method_signature_declaration(None, name, None, None, parameters, void);
    // Upstream passes the file parsed from `interface I {}` as the source
    // file. A view reads one owner here, so the same text is given as a file
    // of the signature's own builder; the signature's nodes are synthesized
    // and parentless, so the file supplies neither text nor comments to them.
    let file = ast.new_source_file(
        SourceFileParseOptions {
            file_name: JsString::from_bytes(&b"/main.ts"[..]),
            path: JsString::from_bytes(&b"/main.ts"[..]),
            ..Default::default()
        },
        SourceText::from_loaded_bytes(&b"interface I {}"[..]),
        None,
        None,
    );

    let context = EmitContext::new();
    let mut default_printer = Printer::new(
        PrinterOptions {
            new_line: NewLineKind::LF,
            ..Default::default()
        },
        &context,
    );
    let got = default_printer
        .emit(ast.view(), method_signature, Some(file))
        .expect("default Emit()");
    assert_eq!(got, b"m(): void;", "default Emit()");

    let mut omit_printer = Printer::new(
        PrinterOptions {
            new_line: NewLineKind::LF,
            omit_trailing_semicolon: true,
            ..Default::default()
        },
        &context,
    );
    let got = omit_printer
        .emit(ast.view(), method_signature, Some(file))
        .expect("omit Emit()");
    assert_eq!(got, b"m(): void", "omit Emit()");

    let for_file = parse_type_script(b"for (;;) {}", false);
    let got = omit_printer
        .emit_source_file(for_file.view(), for_file.root())
        .expect("omit EmitSourceFile(for)");
    let got = got.strip_suffix(b"\n").unwrap_or(&got);
    assert_eq!(got, b"for (;;) { }", "omit EmitSourceFile(for)");
}
