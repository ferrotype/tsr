//! Pinned classifications plus counterexamples where kind and payload differ.
use super::local_container_flags;
use crate::get_container_flags;
use std::panic::{catch_unwind, AssertUnwindSafe};
use ts_arena::Counters;
use ts_ast::{
    AstBuilder, FactoryMethods, JsString, NodeId, NodeKind, ParsedFile, SourceFileParseOptions,
    SyntaxKind as K,
};
use ts_jsstring::SourceText;

fn finish(mut build: AstBuilder) -> ParsedFile {
    let source = build.new_source_file(
        SourceFileParseOptions {
            file_name: JsString::from_bytes(b"/container-classification.ts".as_slice()),
            ..Default::default()
        },
        SourceText::default(),
        None,
        None,
    );
    build.complete(source).unwrap()
}

fn check_flags(parsed: ParsedFile, cases: &[(NodeId, i32)]) {
    for &(node, flags) in cases {
        assert_eq!(get_container_flags(parsed.view(), node).unwrap().0, flags);
    }
    parsed
        .bind_and_publish(|builder| {
            builder
                .with_local_scope(|local| {
                    for &(node, flags) in cases {
                        let node = local.import_node(node).unwrap();
                        assert_eq!(local_container_flags(&local, node).0, flags);
                    }
                })
                .expect("single-source graph admits local access");
            Ok(())
        })
        .unwrap();
}

#[test]
fn fixed_container_rules_do_not_inspect_payload_or_parent() {
    // Raw values pin the Go flag combinations independently of the Rust selector.
    // Token-shaped constructed nodes deliberately provide no payload or parent.
    let kinds = [
        (K::ClassExpression, 1),
        (K::ClassDeclaration, 1),
        (K::EnumDeclaration, 1),
        (K::ObjectLiteralExpression, 1),
        (K::TypeLiteral, 1),
        (K::JsxAttributes, 1),
        (K::InterfaceDeclaration, 65),
        (K::ModuleDeclaration, 33),
        (K::TypeAliasDeclaration, 33),
        (K::JSTypeAliasDeclaration, 33),
        (K::MappedType, 33),
        (K::IndexSignature, 33),
        (K::SourceFile, 37),
        (K::Constructor, 301),
        (K::FunctionDeclaration, 301),
        (K::ClassStaticBlockDeclaration, 301),
        (K::MethodSignature, 557),
        (K::CallSignature, 557),
        (K::FunctionType, 557),
        (K::ConstructSignature, 557),
        (K::ConstructorType, 557),
        (K::FunctionExpression, 317),
        (K::ArrowFunction, 573),
        (K::ModuleBlock, 4),
        (K::CatchClause, 34),
        (K::ForStatement, 34),
        (K::ForInStatement, 34),
        (K::ForOfStatement, 34),
        (K::CaseBlock, 34),
        (K::JSDocSignature, 0),
        (K::Identifier, 0),
    ];
    let mut build = AstBuilder::new(SourceText::default(), &Counters::new());
    let mut cases: Vec<_> = kinds
        .into_iter()
        .map(|(kind, flags)| (build.new_token(kind.into()), flags))
        .collect();
    for value in [i16::MIN, i16::MAX] {
        cases.push((build.new_token(NodeKind::from_raw(value)), 0));
    }
    check_flags(finish(build), &cases);
}

#[test]
fn method_rules_read_only_the_selected_parent_kind() {
    let mut build = AstBuilder::new(SourceText::default(), &Counters::new());
    let mut cases = Vec::new();
    for method in [K::GetAccessor, K::SetAccessor, K::MethodDeclaration] {
        for (parent_kind, flags) in [
            (K::ObjectLiteralExpression, 429),
            (K::ClassExpression, 429),
            (K::ClassDeclaration, 301),
            (K::SourceFile, 301),
            (K::Unknown, 301),
        ] {
            let parent = build.new_token(parent_kind.into());
            let node = build.new_token(method.into());
            build.node_mut(node).unwrap().set_parent(Some(parent));
            cases.push((node, flags));
        }
    }
    check_flags(finish(build), &cases);
}

#[test]
fn block_rules_include_signature_and_static_block_parents() {
    let mut build = AstBuilder::new(SourceText::default(), &Counters::new());
    let mut cases = Vec::new();
    for (parent_kind, flags) in [
        (K::FunctionDeclaration, 0),
        (K::MethodDeclaration, 0),
        (K::Constructor, 0),
        (K::GetAccessor, 0),
        (K::SetAccessor, 0),
        (K::FunctionExpression, 0),
        (K::ArrowFunction, 0),
        (K::MethodSignature, 0),
        (K::CallSignature, 0),
        (K::JSDocSignature, 0),
        (K::ConstructSignature, 0),
        (K::IndexSignature, 0),
        (K::FunctionType, 0),
        (K::ConstructorType, 0),
        (K::ClassStaticBlockDeclaration, 0),
        (K::SourceFile, 34),
        (K::ObjectLiteralExpression, 34),
        (K::Unknown, 34),
    ] {
        let parent = build.new_token(parent_kind.into());
        let node = build.new_token(K::Block.into());
        build.node_mut(node).unwrap().set_parent(Some(parent));
        cases.push((node, flags));
    }
    check_flags(finish(build), &cases);
}

#[test]
fn property_rules_inspect_initializer_without_requiring_a_parent() {
    let mut build = AstBuilder::new(SourceText::default(), &Counters::new());
    let initializer = build.new_token(K::NullKeyword.into());
    let initialized = build.new_property_declaration(None, None, None, None, Some(initializer));
    let uninitialized = build.new_property_declaration(None, None, None, None, None);
    check_flags(finish(build), &[(initialized, 260), (uninitialized, 0)]);
}

fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    payload
        .downcast_ref::<String>()
        .cloned()
        .unwrap_or_else(|| {
            payload
                .downcast_ref::<&str>()
                .expect("text contract panic")
                .to_string()
        })
}

#[test]
fn local_dynamic_rules_preserve_checked_contract_failures() {
    let mut build = AstBuilder::new(SourceText::default(), &Counters::new());
    let cases: Vec<_> = [
        (K::MethodDeclaration, "nil node in source AST utility"),
        (K::GetAccessor, "nil node in source AST utility"),
        (K::SetAccessor, "nil node in source AST utility"),
        (K::Block, "pinned binder requires a nonnil value"),
        (
            K::PropertyDeclaration,
            "interface conversion: ast.nodeData is *ast.Token, not *ast.PropertyDeclaration",
        ),
    ]
    .into_iter()
    .map(|(kind, expected)| (build.new_token(kind.into()), expected))
    .collect();
    let parsed = finish(build);
    for &(node, expected) in &cases {
        let failure = catch_unwind(AssertUnwindSafe(|| {
            get_container_flags(parsed.view(), node)
        }))
        .unwrap_err();
        assert_eq!(panic_message(&*failure), expected);
    }
    parsed
        .bind_and_publish(|builder| {
            builder
                .with_local_scope(|local| {
                    for &(node, expected) in &cases {
                        let node = local.import_node(node).unwrap();
                        let failure =
                            catch_unwind(AssertUnwindSafe(|| local_container_flags(&local, node)))
                                .unwrap_err();
                        assert_eq!(panic_message(&*failure), expected);
                    }
                })
                .expect("constructed dynamic rules admit local access");
            Ok(())
        })
        .unwrap();
}
