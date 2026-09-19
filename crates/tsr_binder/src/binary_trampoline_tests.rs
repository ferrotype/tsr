use crate::{backend::Backend, Binder};
use std::panic::{catch_unwind, AssertUnwindSafe};
use ts_arena::Counters;
use ts_ast::{
    flow_flags as F, AstBuilder, BinaryExpressionData, Factory, FactoryMethods, JsString,
    ParsedFile, SourceFileParseOptions, SyntaxKind as K,
};
use ts_core::ScriptKind;
use ts_jsstring::SourceText;

fn options() -> SourceFileParseOptions {
    SourceFileParseOptions {
        file_name: JsString::from_bytes(b"/binary-continuations.ts".as_slice()),
        ..Default::default()
    }
}

fn finish(mut build: AstBuilder) -> ParsedFile {
    let source = build.new_source_file(options(), SourceText::default(), None, None);
    build.complete(source).unwrap()
}

fn on_both_backends(parsed: ParsedFile, mut check: impl FnMut(&mut Binder<'_, '_, '_>)) {
    parsed
        .bind_and_publish(|builder| {
            check(&mut Binder::new(builder));
            builder
                .with_local_scope(|local| {
                    check(&mut Binder::from_backend(Backend::Local(local)));
                })
                .expect("single-source graph admits local access");
            Ok(())
        })
        .unwrap();
}

fn assert_panics(expected: &str, operation: impl FnOnce()) {
    let failure = catch_unwind(AssertUnwindSafe(operation)).unwrap_err();
    let message = failure
        .downcast_ref::<String>()
        .map(String::as_str)
        .or_else(|| failure.downcast_ref::<&str>().copied())
        .expect("text contract panic");
    assert_eq!(message, expected);
}

#[test]
fn binary_operands_preserve_shape_selection_nil_links_and_contract_panics() {
    let mut build = AstBuilder::new(SourceText::default(), &Counters::new());
    let left = build.new_token(K::NumericLiteral.into());
    let annotation = build.new_token(K::NumberKeyword.into());
    let operator = build.new_token(K::PlusToken.into());
    let right = build.new_token(K::NullKeyword.into());
    let data = BinaryExpressionData {
        modifiers: None,
        left: Some(left),
        r#type: Some(annotation),
        operator_token: Some(operator),
        right: Some(right),
    };
    // The trampoline's payload access is shape-dispatched, not kind-dispatched.
    let different_kind = build.new_binary_expression_data(K::Unknown.into(), data);
    let nil_links = build.new_binary_expression(None, None, None, None, None);
    let wrong_shape = build.new_token(K::BinaryExpression.into());
    let element_wrong_shape = build.new_token(K::ElementAccessExpression.into());
    on_both_backends(finish(build), |binder| {
        let expression = binder.binary_operands(binder.binding_node(different_kind));
        assert_eq!(
            [
                expression.left,
                expression.r#type,
                expression.operator_token,
                expression.right
            ]
            .map(|node| node.map(|node| binder.node_id(node))),
            [Some(left), Some(annotation), Some(operator), Some(right)]
        );
        let expression = binder.binary_operands(binder.binding_node(nil_links));
        assert!([
            expression.left,
            expression.r#type,
            expression.operator_token,
            expression.right
        ]
        .into_iter()
        .all(|node| node.is_none()));
        assert_panics("binder syntax payload", || {
            let _ = binder.binary_operands(binder.binding_node(wrong_shape));
        });
        assert_panics(
            "interface conversion: ast.nodeData is *ast.Token, not *ast.ElementAccessExpression",
            || {
                let _ = binder.binary_element_expression(binder.binding_node(element_wrong_shape));
            },
        );
    });
}

#[test]
fn binary_continuation_checks_keep_short_circuit_and_failure_order() {
    let mut build = AstBuilder::new(SourceText::default(), &Counters::new());
    let wrong_kind = build.new_token(K::Unknown.into());
    let wrong_shape = build.new_token(K::BinaryExpression.into());
    let nil_operator = build.new_binary_expression(None, None, None, None, None);
    let plus = build.new_token(K::PlusToken.into());
    let logical = build.new_token(K::AmpersandAmpersandToken.into());
    let assignment = build.new_token(K::EqualsToken.into());
    let ordinary = build.new_binary_expression(None, None, None, Some(plus), None);
    let logical = build.new_binary_expression(None, None, None, Some(logical), None);
    let nil_assignment_left = build.new_binary_expression(None, None, None, Some(assignment), None);
    on_both_backends(finish(build), |binder| {
        // Even an invalid payload is not inspected on an unreachable branch.
        assert!(!binder.can_continue_binary(binder.binding_node(wrong_shape)));
        binder.current_flow = Some(binder.new_flow_node(F::START));
        assert!(!binder.can_continue_binary(binder.binding_node(wrong_kind)));
        assert!(binder.can_continue_binary(binder.binding_node(ordinary)));
        assert!(!binder.can_continue_binary(binder.binding_node(logical)));
        for (node, message) in [
            (
                wrong_shape,
                "interface conversion: ast.nodeData is *ast.Token, not *ast.BinaryExpression",
            ),
            (nil_operator, "nil node in source AST utility"),
            (nil_assignment_left, "nil assignment left operand"),
        ] {
            assert_panics(message, || {
                let _ = binder.can_continue_binary(binder.binding_node(node));
            });
        }
    });
}

#[test]
fn local_binary_continuations_keep_deep_chains_off_the_native_stack() {
    let inventory: serde_json::Value =
        serde_json::from_str(include_str!("../../../data/s07/binder-depth-cases.json")).unwrap();
    for case in inventory["small_stack"].as_array().unwrap() {
        if !case["require_binary"].as_bool().unwrap() {
            continue;
        }
        let name = case["id"].as_str().unwrap();
        let bytes: Vec<_> = case["source_hex"]
            .as_str()
            .unwrap()
            .as_bytes()
            .chunks_exact(2)
            .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
            .collect();
        let parsed = ts_parser::parse_source_file(
            SourceText::from_loaded_bytes(bytes),
            ScriptKind::TS,
            options(),
        );
        let observation = std::thread::Builder::new()
            .name(format!("local-{name}"))
            .stack_size(512 * 1024)
            .spawn(move || {
                crate::recursion::take_observations();
                let completed = parsed
                    .bind_and_publish(|builder| {
                        builder
                            .with_local_scope(|local| {
                                crate::run_binding(Binder::from_backend(Backend::Local(local)));
                            })
                            .expect("parsed chain admits local access");
                        Ok(())
                    })
                    .unwrap();
                assert!(completed.bound_with_local_scope());
                assert_eq!(
                    completed.view().node(completed.source()).unwrap().kind(),
                    K::SourceFile
                );
                crate::recursion::take_observations()
            })
            .unwrap()
            .join()
            .unwrap();
        assert!(
            observation.binary_nodes >= 20_000,
            "{name}: {observation:?}"
        );
        assert!(
            observation.binary_frames > 20_000,
            "{name}: {observation:?}"
        );
    }
}
