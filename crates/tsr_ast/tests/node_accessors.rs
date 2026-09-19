use std::{
    collections::BTreeMap,
    panic::{catch_unwind, AssertUnwindSafe},
};
use ts_ast::*;
use ts_core::TextRange;
use ts_jsstring::SourceText;

fn outcome(action: impl FnOnce() -> String) -> String {
    match catch_unwind(AssertUnwindSafe(action)) {
        Ok(value) => format!("value:{value}"),
        Err(payload) => {
            let message = payload
                .downcast_ref::<String>()
                .map(String::as_str)
                .or_else(|| payload.downcast_ref::<&str>().copied())
                .unwrap_or("non-string Rust panic");
            if message == "runtime error: invalid memory address or nil pointer dereference" {
                "panic:nil".into()
            } else if message.starts_with("interface conversion: ") {
                format!("panic:payload:{message}")
            } else {
                format!("panic:message:{message}")
            }
        }
    }
}
fn check(
    actual: &mut BTreeMap<String, String>,
    name: &str,
    f: &mut AstBuilder,
    n: NodeId,
    children: [NodeId; 4],
    mods: NodeListId,
) {
    let id = |node: Option<NodeId>| match node {
        None => "nil",
        Some(n) if n == children[0] => "a",
        Some(n) if n == children[1] => "b",
        Some(n) if n == children[2] => "q",
        Some(n) if n == children[3] => "e",
        _ => panic!("unregistered fixture child"),
    };
    let nodes = |ns: &NodeSliceRead<'_>| ns.iter().map(id).collect::<Vec<_>>().join(",");
    let list = |ns: Option<NodeListId>| {
        ns.map_or_else(
            || "nil".into(),
            |list| {
                format!(
                    "list:{}",
                    nodes(
                        &f.view()
                            .node_slice(f.view().list(list).unwrap().nodes())
                            .unwrap()
                    )
                )
            },
        )
    };
    let mut emit = |operation: &str, value: String| {
        assert!(actual
            .insert(format!("{name}/{operation}"), value)
            .is_none());
    };

    emit(
        "Name",
        outcome(|| id(f.view().node(n).unwrap().name()).into()),
    );
    emit(
        "Expression",
        outcome(|| id(f.view().node(n).unwrap().expression()).into()),
    );
    emit(
        "Type",
        outcome(|| id(f.view().node(n).unwrap().type_node()).into()),
    );
    emit(
        "Initializer",
        outcome(|| id(f.view().node(n).unwrap().initializer()).into()),
    );
    emit(
        "TagName",
        outcome(|| id(f.view().node(n).unwrap().tag_name()).into()),
    );
    emit(
        "PropertyName",
        outcome(|| id(f.view().node(n).unwrap().property_name()).into()),
    );
    emit(
        "Label",
        outcome(|| id(f.view().node(n).unwrap().label()).into()),
    );
    emit(
        "Attributes",
        outcome(|| id(f.view().node(n).unwrap().attributes()).into()),
    );
    emit(
        "ModuleSpecifier",
        outcome(|| id(f.view().node(n).unwrap().module_specifier()).into()),
    );
    emit(
        "ImportClause",
        outcome(|| id(f.view().node(n).unwrap().import_clause()).into()),
    );
    emit(
        "Statement",
        outcome(|| id(f.view().node(n).unwrap().statement()).into()),
    );
    emit(
        "PostfixToken",
        outcome(|| id(f.view().node(n).unwrap().postfix_token()).into()),
    );
    emit(
        "QuestionDotToken",
        outcome(|| id(f.view().node(n).unwrap().question_dot_token()).into()),
    );
    emit(
        "TypeExpression",
        outcome(|| id(f.view().node(n).unwrap().type_expression()).into()),
    );
    emit(
        "ClassName",
        outcome(|| id(f.view().node(n).unwrap().class_name()).into()),
    );
    emit(
        "Body",
        outcome(|| id(f.view().node(n).unwrap().body()).into()),
    );
    emit(
        "PropertyNameOrName",
        outcome(|| id(f.view().node(n).unwrap().property_name_or_name()).into()),
    );
    emit(
        "ArgumentList",
        outcome(|| list(f.view().node(n).unwrap().argument_list())),
    );
    emit(
        "TypeArgumentList",
        outcome(|| list(f.view().node(n).unwrap().type_argument_list())),
    );
    emit(
        "TypeParameterList",
        outcome(|| list(f.view().node(n).unwrap().type_parameter_list())),
    );
    emit(
        "ParameterList",
        outcome(|| list(f.view().node(n).unwrap().parameter_list())),
    );
    emit(
        "MemberList",
        outcome(|| list(f.view().node(n).unwrap().member_list())),
    );
    emit(
        "StatementList",
        outcome(|| list(f.view().node(n).unwrap().statement_list())),
    );
    emit(
        "CommentList",
        outcome(|| list(f.view().node(n).unwrap().comment_list())),
    );
    emit(
        "Children",
        outcome(|| list(f.view().node(n).unwrap().children_list())),
    );
    emit(
        "PropertyList",
        outcome(|| list(f.view().node(n).unwrap().property_list())),
    );
    emit(
        "ElementList",
        outcome(|| list(f.view().node(n).unwrap().element_list())),
    );
    emit(
        "Modifiers",
        outcome(|| list(f.view().node(n).unwrap().modifiers())),
    );
    emit(
        "Arguments",
        outcome(|| {
            nodes(
                &f.view()
                    .node_slice(f.view().node(n).unwrap().arguments(f.view()).unwrap())
                    .unwrap(),
            )
        }),
    );
    emit(
        "TypeArguments",
        outcome(|| {
            nodes(
                &f.view()
                    .node_slice(f.view().node(n).unwrap().type_arguments(f.view()).unwrap())
                    .unwrap(),
            )
        }),
    );
    emit(
        "TypeParameters",
        outcome(|| {
            nodes(
                &f.view()
                    .node_slice(f.view().node(n).unwrap().type_parameters(f.view()).unwrap())
                    .unwrap(),
            )
        }),
    );
    emit(
        "Parameters",
        outcome(|| {
            nodes(
                &f.view()
                    .node_slice(f.view().node(n).unwrap().parameters(f.view()).unwrap())
                    .unwrap(),
            )
        }),
    );
    emit(
        "Members",
        outcome(|| {
            nodes(
                &f.view()
                    .node_slice(f.view().node(n).unwrap().members(f.view()).unwrap())
                    .unwrap(),
            )
        }),
    );
    emit(
        "Statements",
        outcome(|| {
            nodes(
                &f.view()
                    .node_slice(f.view().node(n).unwrap().statements(f.view()).unwrap())
                    .unwrap(),
            )
        }),
    );
    emit(
        "ModifierNodes",
        outcome(|| {
            nodes(
                &f.view()
                    .node_slice(f.view().node(n).unwrap().modifier_nodes(f.view()).unwrap())
                    .unwrap(),
            )
        }),
    );
    emit(
        "Comments",
        outcome(|| {
            nodes(
                &f.view()
                    .node_slice(f.view().node(n).unwrap().comments(f.view()).unwrap())
                    .unwrap(),
            )
        }),
    );
    emit(
        "Properties",
        outcome(|| {
            nodes(
                &f.view()
                    .node_slice(f.view().node(n).unwrap().properties(f.view()).unwrap())
                    .unwrap(),
            )
        }),
    );
    emit(
        "Elements",
        outcome(|| {
            nodes(
                &f.view()
                    .node_slice(f.view().node(n).unwrap().elements(f.view()).unwrap())
                    .unwrap(),
            )
        }),
    );
    emit(
        "QuestionToken",
        outcome(|| id(f.view().node(n).unwrap().question_token(f.view()).unwrap()).into()),
    );
    emit(
        "ModifierFlags",
        outcome(|| {
            f.view()
                .node(n)
                .unwrap()
                .modifier_flags(f.view())
                .unwrap()
                .to_string()
        }),
    );
    emit(
        "KindString",
        outcome(|| f.view().node(n).unwrap().kind_string()),
    );
    emit(
        "KindValue",
        outcome(|| f.view().node(n).unwrap().kind_value().to_string()),
    );
    emit(
        "CanHaveStatements",
        outcome(|| f.view().node(n).unwrap().can_have_statements().to_string()),
    );
    emit(
        "IsTypeOnly",
        outcome(|| f.view().node(n).unwrap().is_type_only().to_string()),
    );
    emit("Text", outcome(|| hex(&f.view().node_text(n).unwrap())));
    emit(
        "RawText",
        outcome(|| hex(f.view().node(n).unwrap().raw_text())),
    );
    emit(
        "SetExpression",
        outcome(|| {
            f.node_mut(n).unwrap().set_expression(Some(children[1]));
            id(f.view().node(n).unwrap().expression()).into()
        }),
    );
    emit(
        "SetType",
        outcome(|| {
            f.node_mut(n).unwrap().set_type_node(Some(children[1]));
            id(f.view().node(n).unwrap().type_node()).into()
        }),
    );
    emit(
        "SetInitializer",
        outcome(|| {
            f.node_mut(n).unwrap().set_initializer(Some(children[1]));
            id(f.view().node(n).unwrap().initializer()).into()
        }),
    );
    emit(
        "SetModifiers",
        outcome(|| {
            f.node_mut(n).unwrap().set_modifiers(Some(mods));
            f.view()
                .node(n)
                .unwrap()
                .modifier_flags(f.view())
                .unwrap()
                .to_string()
        }),
    );
}
fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut text = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        write!(text, "{byte:02x}").unwrap();
    }
    text
}

#[test]
fn matches_pinned_go_node_accessor_observations() {
    let mut f = AstBuilder::new(SourceText::default(), &ts_arena::Counters::new());
    let a = f.new_identifier(JsString::from_bytes(&b"a"[..]));
    let b = f.new_identifier(JsString::from_bytes(&b"b"[..]));
    let q = f.new_token(SyntaxKind::QuestionToken.into());
    let e = f.new_token(SyntaxKind::ExclamationToken.into());
    let public = f.new_token(SyntaxKind::PublicKeyword.into());
    let mod_nodes = f.node_slice(vec![Some(public)]).unwrap();
    let mods = f.new_modifier_list(mod_nodes);
    let text = f
        .text_slice(vec![
            JsString::from_bytes(&b"x"[..]),
            JsString::from_bytes(&b"y\xff"[..]),
        ])
        .unwrap();
    let mut actual = BTreeMap::new();
    // Each expected panic is compared by exact message or explicit nil/payload
    // class. Suppression only prevents tens of thousands of expected traces.
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let run = catch_unwind(AssertUnwindSafe(|| {
        macro_rules! constructor {
            ($name:literal, $method:ident($($arg:expr),* $(,)?)) => {{
                let node=f.$method($($arg),*);
                check(&mut actual,concat!("constructor/",$name),&mut f,node,[a,b,q,e],mods);
            }};
        }

        for raw in (-1..=i16::try_from(SyntaxKind::ALL.len()).unwrap()).chain([i16::MIN, i16::MAX])
        {
            let node = f.new_token(NodeKind::from_raw(raw));
            check(
                &mut actual,
                &format!("token/{raw}"),
                &mut f,
                node,
                [a, b, q, e],
                mods,
            );
        }

        constructor!("NewToken", new_token(NodeKind::from_raw(0)));
        constructor!(
            "NewIdentifier",
            new_identifier(JsString::from_bytes(&b"x\xff"[..]))
        );
        constructor!(
            "NewPrivateIdentifier",
            new_private_identifier(JsString::from_bytes(&b"x\xff"[..]))
        );
        constructor!("NewQualifiedName", new_qualified_name(None, None));
        constructor!("NewComputedPropertyName", new_computed_property_name(None));
        constructor!("NewDecorator", new_decorator(None));
        constructor!("NewEmptyStatement", new_empty_statement());
        constructor!("NewIfStatement", new_if_statement(None, None, None));
        constructor!("NewDoStatement", new_do_statement(None, None));
        constructor!("NewWhileStatement", new_while_statement(None, None));
        constructor!("NewForStatement", new_for_statement(None, None, None, None));
        constructor!(
            "NewForInOrOfStatement",
            new_for_in_or_of_statement(NodeKind::from_raw(0), None, None, None, None)
        );
        constructor!("NewBreakStatement", new_break_statement(None));
        constructor!("NewContinueStatement", new_continue_statement(None));
        constructor!("NewReturnStatement", new_return_statement(None));
        constructor!("NewWithStatement", new_with_statement(None, None));
        constructor!("NewSwitchStatement", new_switch_statement(None, None));
        constructor!("NewCaseBlock", new_case_block(None));
        constructor!(
            "NewCaseOrDefaultClause",
            new_case_or_default_clause(NodeKind::from_raw(0), None, None)
        );
        constructor!("NewThrowStatement", new_throw_statement(None));
        constructor!("NewTryStatement", new_try_statement(None, None, None));
        constructor!("NewCatchClause", new_catch_clause(None, None));
        constructor!("NewDebuggerStatement", new_debugger_statement());
        constructor!("NewLabeledStatement", new_labeled_statement(None, None));
        constructor!("NewExpressionStatement", new_expression_statement(None));
        constructor!("NewBlock", new_block(None, false));
        constructor!("NewVariableStatement", new_variable_statement(None, None));
        constructor!(
            "NewVariableDeclaration",
            new_variable_declaration(None, None, None, None)
        );
        constructor!(
            "NewVariableDeclarationList",
            new_variable_declaration_list(None, 0)
        );
        constructor!(
            "NewBindingPattern",
            new_binding_pattern(NodeKind::from_raw(0), None)
        );
        constructor!(
            "NewParameterDeclaration",
            new_parameter_declaration(None, None, None, None, None, None)
        );
        constructor!(
            "NewBindingElement",
            new_binding_element(None, None, None, None)
        );
        constructor!("NewMissingDeclaration", new_missing_declaration(None));
        constructor!(
            "NewFunctionDeclaration",
            new_function_declaration(None, None, None, None, None, None, None, None)
        );
        constructor!(
            "NewClassDeclaration",
            new_class_declaration(None, None, None, None, None)
        );
        constructor!(
            "NewClassExpression",
            new_class_expression(None, None, None, None, None)
        );
        constructor!(
            "NewHeritageClause",
            new_heritage_clause(NodeKind::from_raw(0), None)
        );
        constructor!(
            "NewInterfaceDeclaration",
            new_interface_declaration(None, None, None, None, None)
        );
        constructor!(
            "NewTypeAliasDeclaration",
            new_type_alias_declaration(None, None, None, None)
        );
        constructor!(
            "NewJSTypeAliasDeclaration",
            new_js_type_alias_declaration(None, None, None, None)
        );
        constructor!("NewEnumMember", new_enum_member(None, None));
        constructor!("NewEnumDeclaration", new_enum_declaration(None, None, None));
        constructor!("NewModuleBlock", new_module_block(None));
        constructor!("NewNotEmittedStatement", new_not_emitted_statement());
        constructor!("NewNotEmittedTypeElement", new_not_emitted_type_element());
        constructor!(
            "NewImportDeclaration",
            new_import_declaration(None, None, None, None)
        );
        constructor!(
            "NewJSImportDeclaration",
            new_js_import_declaration(None, None, None, None)
        );
        constructor!(
            "NewExternalModuleReference",
            new_external_module_reference(None)
        );
        constructor!("NewNamespaceImport", new_namespace_import(None));
        constructor!("NewNamedImports", new_named_imports(None));
        constructor!(
            "NewExportAssignment",
            new_export_assignment(None, false, None, None)
        );
        constructor!(
            "NewNamespaceExportDeclaration",
            new_namespace_export_declaration(None, None)
        );
        constructor!("NewNamespaceExport", new_namespace_export(None));
        constructor!("NewNamedExports", new_named_exports(None));
        constructor!(
            "NewExportSpecifier",
            new_export_specifier(false, None, None)
        );
        constructor!(
            "NewCallSignatureDeclaration",
            new_call_signature_declaration(None, None, None)
        );
        constructor!(
            "NewConstructSignatureDeclaration",
            new_construct_signature_declaration(None, None, None)
        );
        constructor!(
            "NewConstructorDeclaration",
            new_constructor_declaration(None, None, None, None, None, None)
        );
        constructor!(
            "NewGetAccessorDeclaration",
            new_get_accessor_declaration(None, None, None, None, None, None, None)
        );
        constructor!(
            "NewSetAccessorDeclaration",
            new_set_accessor_declaration(None, None, None, None, None, None, None)
        );
        constructor!(
            "NewIndexSignatureDeclaration",
            new_index_signature_declaration(None, None, None)
        );
        constructor!(
            "NewMethodSignatureDeclaration",
            new_method_signature_declaration(None, None, None, None, None, None)
        );
        constructor!(
            "NewMethodDeclaration",
            new_method_declaration(None, None, None, None, None, None, None, None, None)
        );
        constructor!(
            "NewPropertySignatureDeclaration",
            new_property_signature_declaration(None, None, None, None, None)
        );
        constructor!(
            "NewPropertyDeclaration",
            new_property_declaration(None, None, None, None, None)
        );
        constructor!("NewSemicolonClassElement", new_semicolon_class_element());
        constructor!(
            "NewClassStaticBlockDeclaration",
            new_class_static_block_declaration(None, None)
        );
        constructor!("NewOmittedExpression", new_omitted_expression());
        constructor!(
            "NewKeywordExpression",
            new_keyword_expression(NodeKind::from_raw(0))
        );
        constructor!(
            "NewStringLiteral",
            new_string_literal(JsString::from_bytes(&b"x\xff"[..]), 0)
        );
        constructor!(
            "NewNumericLiteral",
            new_numeric_literal(JsString::from_bytes(&b"x\xff"[..]), 0)
        );
        constructor!(
            "NewBigIntLiteral",
            new_big_int_literal(JsString::from_bytes(&b"x\xff"[..]), 0)
        );
        constructor!(
            "NewRegularExpressionLiteral",
            new_regular_expression_literal(JsString::from_bytes(&b"x\xff"[..]), 0)
        );
        constructor!(
            "NewNoSubstitutionTemplateLiteral",
            new_no_substitution_template_literal(JsString::from_bytes(&b"x\xff"[..]), 0)
        );
        constructor!(
            "NewBinaryExpression",
            new_binary_expression(None, None, None, None, None)
        );
        constructor!(
            "NewPrefixUnaryExpression",
            new_prefix_unary_expression(NodeKind::from_raw(0), None)
        );
        constructor!(
            "NewPostfixUnaryExpression",
            new_postfix_unary_expression(None, NodeKind::from_raw(0))
        );
        constructor!("NewYieldExpression", new_yield_expression(None, None));
        constructor!(
            "NewArrowFunction",
            new_arrow_function(None, None, None, None, None, None, None)
        );
        constructor!(
            "NewFunctionExpression",
            new_function_expression(None, None, None, None, None, None, None, None)
        );
        constructor!("NewAsExpression", new_as_expression(None, None));
        constructor!(
            "NewSatisfiesExpression",
            new_satisfies_expression(None, None)
        );
        constructor!(
            "NewConditionalExpression",
            new_conditional_expression(None, None, None, None, None)
        );
        constructor!(
            "NewPropertyAccessExpression",
            new_property_access_expression(None, None, None, 0)
        );
        constructor!(
            "NewElementAccessExpression",
            new_element_access_expression(None, None, None, 0)
        );
        constructor!(
            "NewCallExpression",
            new_call_expression(None, None, None, None, 0)
        );
        constructor!("NewNewExpression", new_new_expression(None, None, None));
        constructor!(
            "NewMetaProperty",
            new_meta_property(NodeKind::from_raw(0), None)
        );
        constructor!("NewNonNullExpression", new_non_null_expression(None, 0));
        constructor!("NewSpreadElement", new_spread_element(None));
        constructor!("NewTemplateExpression", new_template_expression(None, None));
        constructor!("NewTemplateSpan", new_template_span(None, None));
        constructor!(
            "NewTaggedTemplateExpression",
            new_tagged_template_expression(None, None, None, None, 0)
        );
        constructor!(
            "NewParenthesizedExpression",
            new_parenthesized_expression(None)
        );
        constructor!(
            "NewArrayLiteralExpression",
            new_array_literal_expression(None, false)
        );
        constructor!(
            "NewObjectLiteralExpression",
            new_object_literal_expression(None, false)
        );
        constructor!("NewSpreadAssignment", new_spread_assignment(None));
        constructor!(
            "NewPropertyAssignment",
            new_property_assignment(None, None, None, None, None)
        );
        constructor!(
            "NewShorthandPropertyAssignment",
            new_shorthand_property_assignment(None, None, None, None, None, None)
        );
        constructor!("NewDeleteExpression", new_delete_expression(None));
        constructor!("NewTypeOfExpression", new_type_of_expression(None));
        constructor!("NewVoidExpression", new_void_expression(None));
        constructor!("NewAwaitExpression", new_await_expression(None));
        constructor!("NewTypeAssertion", new_type_assertion(None, None));
        constructor!(
            "NewKeywordTypeNode",
            new_keyword_type_node(NodeKind::from_raw(0))
        );
        constructor!("NewUnionTypeNode", new_union_type_node(None));
        constructor!("NewIntersectionTypeNode", new_intersection_type_node(None));
        constructor!(
            "NewConditionalTypeNode",
            new_conditional_type_node(None, None, None, None)
        );
        constructor!(
            "NewTypeOperatorNode",
            new_type_operator_node(NodeKind::from_raw(0), None)
        );
        constructor!("NewInferTypeNode", new_infer_type_node(None));
        constructor!("NewArrayTypeNode", new_array_type_node(None));
        constructor!(
            "NewIndexedAccessTypeNode",
            new_indexed_access_type_node(None, None)
        );
        constructor!("NewTypeReferenceNode", new_type_reference_node(None, None));
        constructor!(
            "NewExpressionWithTypeArguments",
            new_expression_with_type_arguments(None, None)
        );
        constructor!("NewLiteralTypeNode", new_literal_type_node(None));
        constructor!("NewThisTypeNode", new_this_type_node());
        constructor!(
            "NewTypePredicateNode",
            new_type_predicate_node(None, None, None)
        );
        constructor!("NewImportAttribute", new_import_attribute(None, None));
        constructor!(
            "NewImportAttributes",
            new_import_attributes(NodeKind::from_raw(0), None, false)
        );
        constructor!("NewTypeQueryNode", new_type_query_node(None, None));
        constructor!(
            "NewMappedTypeNode",
            new_mapped_type_node(None, None, None, None, None, None)
        );
        constructor!("NewTypeLiteralNode", new_type_literal_node(None));
        constructor!("NewTupleTypeNode", new_tuple_type_node(None));
        constructor!(
            "NewNamedTupleMember",
            new_named_tuple_member(None, None, None, None)
        );
        constructor!("NewOptionalTypeNode", new_optional_type_node(None));
        constructor!("NewRestTypeNode", new_rest_type_node(None));
        constructor!(
            "NewParenthesizedTypeNode",
            new_parenthesized_type_node(None)
        );
        constructor!(
            "NewFunctionTypeNode",
            new_function_type_node(None, None, None)
        );
        constructor!(
            "NewConstructorTypeNode",
            new_constructor_type_node(None, None, None, None)
        );
        constructor!(
            "NewTemplateHead",
            new_template_head(
                JsString::from_bytes(&b"x\xff"[..]),
                JsString::from_bytes(&b"x\xff"[..]),
                0,
            )
        );
        constructor!(
            "NewTemplateMiddle",
            new_template_middle(
                JsString::from_bytes(&b"x\xff"[..]),
                JsString::from_bytes(&b"x\xff"[..]),
                0,
            )
        );
        constructor!(
            "NewTemplateTail",
            new_template_tail(
                JsString::from_bytes(&b"x\xff"[..]),
                JsString::from_bytes(&b"x\xff"[..]),
                0,
            )
        );
        constructor!(
            "NewTemplateLiteralTypeNode",
            new_template_literal_type_node(None, None)
        );
        constructor!(
            "NewTemplateLiteralTypeSpan",
            new_template_literal_type_span(None, None)
        );
        constructor!(
            "NewPartiallyEmittedExpression",
            new_partially_emitted_expression(None)
        );
        constructor!("NewJsxElement", new_jsx_element(None, None, None));
        constructor!("NewJsxAttributes", new_jsx_attributes(None));
        constructor!("NewJsxNamespacedName", new_jsx_namespaced_name(None, None));
        constructor!(
            "NewJsxOpeningElement",
            new_jsx_opening_element(None, None, None)
        );
        constructor!(
            "NewJsxSelfClosingElement",
            new_jsx_self_closing_element(None, None, None)
        );
        constructor!("NewJsxFragment", new_jsx_fragment(None, None, None));
        constructor!("NewJsxOpeningFragment", new_jsx_opening_fragment());
        constructor!("NewJsxClosingFragment", new_jsx_closing_fragment());
        constructor!("NewJsxAttribute", new_jsx_attribute(None, None));
        constructor!("NewJsxSpreadAttribute", new_jsx_spread_attribute(None));
        constructor!("NewJsxClosingElement", new_jsx_closing_element(None));
        constructor!("NewJsxExpression", new_jsx_expression(None, None));
        constructor!(
            "NewJsxText",
            new_jsx_text(JsString::from_bytes(&b"x\xff"[..]), false)
        );
        constructor!("NewSyntaxList", new_syntax_list(NodeSlice::empty()));
        constructor!("NewJSDoc", new_js_doc(None, None));
        constructor!("NewJSDocTypeExpression", new_js_doc_type_expression(None));
        constructor!(
            "NewJSDocNonNullableType",
            new_js_doc_non_nullable_type(None)
        );
        constructor!("NewJSDocNullableType", new_js_doc_nullable_type(None));
        constructor!("NewJSDocAllType", new_js_doc_all_type());
        constructor!("NewJSDocVariadicType", new_js_doc_variadic_type(None));
        constructor!("NewJSDocOptionalType", new_js_doc_optional_type(None));
        constructor!("NewJSDocTypeTag", new_js_doc_type_tag(None, None, None));
        constructor!("NewJSDocUnknownTag", new_js_doc_unknown_tag(None, None));
        constructor!(
            "NewJSDocTemplateTag",
            new_js_doc_template_tag(None, None, None, None)
        );
        constructor!("NewJSDocReturnTag", new_js_doc_return_tag(None, None, None));
        constructor!("NewJSDocPublicTag", new_js_doc_public_tag(None, None));
        constructor!("NewJSDocPrivateTag", new_js_doc_private_tag(None, None));
        constructor!("NewJSDocProtectedTag", new_js_doc_protected_tag(None, None));
        constructor!("NewJSDocReadonlyTag", new_js_doc_readonly_tag(None, None));
        constructor!("NewJSDocOverrideTag", new_js_doc_override_tag(None, None));
        constructor!(
            "NewJSDocDeprecatedTag",
            new_js_doc_deprecated_tag(None, None)
        );
        constructor!("NewJSDocSeeTag", new_js_doc_see_tag(None, None, None));
        constructor!(
            "NewJSDocImplementsTag",
            new_js_doc_implements_tag(None, None, None)
        );
        constructor!(
            "NewJSDocAugmentsTag",
            new_js_doc_augments_tag(None, None, None)
        );
        constructor!(
            "NewJSDocSatisfiesTag",
            new_js_doc_satisfies_tag(None, None, None)
        );
        constructor!("NewJSDocThrowsTag", new_js_doc_throws_tag(None, None, None));
        constructor!("NewJSDocThisTag", new_js_doc_this_tag(None, None, None));
        constructor!(
            "NewJSDocImportTag",
            new_js_doc_import_tag(None, None, None, None, None)
        );
        constructor!(
            "NewJSDocCallbackTag",
            new_js_doc_callback_tag(None, None, None, None)
        );
        constructor!(
            "NewJSDocOverloadTag",
            new_js_doc_overload_tag(None, None, None)
        );
        constructor!(
            "NewJSDocTypedefTag",
            new_js_doc_typedef_tag(None, None, None, None)
        );
        constructor!("NewJSDocSignature", new_js_doc_signature(None, None, None));
        constructor!("NewJSDocNameReference", new_js_doc_name_reference(None));
        constructor!(
            "NewModuleDeclaration",
            new_module_declaration(None, NodeKind::from_raw(0), None, None, None)
        );
        constructor!(
            "NewImportEqualsDeclaration",
            new_import_equals_declaration(None, false, None, None)
        );
        constructor!(
            "NewExportDeclaration",
            new_export_declaration(None, false, None, None, None)
        );
        constructor!(
            "NewImportTypeNode",
            new_import_type_node(false, None, None, None, None)
        );
        constructor!(
            "NewImportClause",
            new_import_clause(NodeKind::from_raw(0), None, None)
        );
        constructor!(
            "NewImportSpecifier",
            new_import_specifier(false, None, None)
        );
        constructor!("NewJSDocText", new_js_doc_text(text));
        constructor!("NewJSDocLink", new_js_doc_link(None, text));
        constructor!("NewJSDocLinkPlain", new_js_doc_link_plain(None, text));
        constructor!("NewJSDocLinkCode", new_js_doc_link_code(None, text));
        constructor!(
            "NewTypeParameterDeclaration",
            new_type_parameter_declaration(None, None, None, None, None)
        );
        constructor!(
            "NewSyntheticReferenceExpression",
            new_synthetic_reference_expression(None, None)
        );
        constructor!(
            "NewJSDocTypeLiteral",
            new_js_doc_type_literal(NodeSlice::empty(), false)
        );
        constructor!(
            "NewJSDocParameterOrPropertyTag",
            new_js_doc_parameter_or_property_tag(
                NodeKind::from_raw(0),
                None,
                None,
                false,
                None,
                false,
                None,
            )
        );
        let ns = f.node_slice(vec![Some(a), None, Some(b)]).unwrap();
        let list = f.new_list(TextRange::new(-1, -1), ns).unwrap();
        let node = f.new_call_expression(
            Some(a),
            Some(q),
            Some(list),
            Some(list),
            node_flags::OPTIONAL_CHAIN,
        );
        check(
            &mut actual,
            "nonnull/call",
            &mut f,
            node,
            [a, b, q, e],
            mods,
        );
        let node = f.new_function_declaration(
            None,
            None,
            Some(a),
            Some(list),
            Some(list),
            Some(b),
            None,
            Some(a),
        );
        check(
            &mut actual,
            "nonnull/function",
            &mut f,
            node,
            [a, b, q, e],
            mods,
        );
        let node = f.new_property_declaration(None, Some(a), Some(q), Some(b), Some(a));
        check(
            &mut actual,
            "nonnull/property-question",
            &mut f,
            node,
            [a, b, q, e],
            mods,
        );
        let node = f.new_property_declaration(None, Some(a), Some(e), Some(b), Some(a));
        check(
            &mut actual,
            "nonnull/property-exclamation",
            &mut f,
            node,
            [a, b, q, e],
            mods,
        );
        let node = f.new_jsx_namespaced_name(Some(a), Some(b));
        check(
            &mut actual,
            "nonnull/jsx-name",
            &mut f,
            node,
            [a, b, q, e],
            mods,
        );
        let node = f.new_meta_property(SyntaxKind::ImportKeyword.into(), Some(a));
        check(
            &mut actual,
            "nonnull/meta",
            &mut f,
            node,
            [a, b, q, e],
            mods,
        );
        let node = f.new_import_clause(SyntaxKind::TypeKeyword.into(), Some(a), Some(b));
        check(
            &mut actual,
            "nonnull/import-type",
            &mut f,
            node,
            [a, b, q, e],
            mods,
        );
        let node = f.new_node(
            SyntaxKind::Identifier.into(),
            NodeData::FunctionDeclaration(Box::new(FunctionDeclarationData {
                modifiers: None,
                asterisk_token: None,
                name: Some(a),
                type_parameters: Some(list),
                parameters: Some(list),
                r#type: Some(b),
                full_signature: None,
                body: Some(a),
            })),
        );
        check(
            &mut actual,
            "nonnull/retag-function",
            &mut f,
            node,
            [a, b, q, e],
            mods,
        );
    }));
    std::panic::set_hook(hook);
    if let Err(p) = run {
        std::panic::resume_unwind(p)
    }
    let expected: BTreeMap<String, String> =
        include_str!("../../../data/s06/accessor-observations.tsv")
            .lines()
            .filter(|line| !line.starts_with('#') && !line.starts_with("source/"))
            .map(|line| {
                let (k, v) = line.split_once('\t').unwrap();
                (k.into(), v.into())
            })
            .collect();
    assert_eq!(actual.len(), expected.len());
    let differences: Vec<_> = actual
        .iter()
        .filter(|(key, value)| expected.get(*key) != Some(*value))
        .take(30)
        .map(|(key, value)| format!("{key}: Rust={value:?}, Go={:?}", expected.get(key)))
        .collect();
    assert!(differences.is_empty(), "{}", differences.join("\n"));
}

fn source_check(
    actual: &mut BTreeMap<String, String>,
    name: &str,
    f: &AstBuilder,
    id: NodeId,
    canonical: NodeId,
    children: [NodeId; 2],
) {
    let sf = f.view().source_file(id).unwrap();
    let mut emit = |operation: &str, value: String| {
        assert!(actual
            .insert(format!("{name}/{operation}"), format!("value:{value}"))
            .is_none());
    };
    let options = |o: &SourceFileParseOptions| {
        format!(
            "{}:{}:{}:{}",
            hex(o.file_name.as_bytes()),
            hex(o.path.as_bytes()),
            o.external_module_indicator_options.jsx,
            o.external_module_indicator_options.force
        )
    };
    emit("FileName", hex(sf.file_name()));
    emit("Path", hex(sf.path()));
    emit("Text", hex(sf.text().as_bytes()));
    emit("OriginalText", hex(sf.original_text()));
    emit("OriginalFileName", hex(&sf.original_file_name().unwrap()));
    emit("ParseOptions", options(sf.parse_options()));
    emit("ContentMapper", hex(sf.content_mapper()));
    emit(
        "ContentMapperTransformIdentity",
        hex(sf.content_mapper_transform_identity()),
    );
    emit(
        "ContentMapperParseOptions",
        options(&sf.content_mapper_parse_options()),
    );
    emit("VirtualFileName", hex(sf.virtual_file_name()));
    emit(
        "IsContentMapperFailureStub",
        sf.is_content_mapper_failure_stub().to_string(),
    );
    emit(
        "IsContentMapperSupplemental",
        sf.is_content_mapper_supplemental().to_string(),
    );
    emit("SpanMap", sf.span_map().is_none().to_string());
    emit(
        "CanonicalSourceFile",
        (sf.canonical_source_file() == Some(canonical)).to_string(),
    );
    emit(
        "SupplementalSourceFiles",
        sf.supplemental_source_files().unwrap().len().to_string(),
    );
    emit(
        "DiagnosticDirectives",
        sf.diagnostic_directives().unwrap().len().to_string(),
    );
    emit(
        "Imports",
        sf.imports()
            .unwrap()
            .iter()
            .map(|&n| {
                if n == Some(children[0]) {
                    "a"
                } else if n == Some(children[1]) {
                    "b"
                } else {
                    assert!(n.is_none(), "unregistered source child");
                    "nil"
                }
            })
            .collect::<Vec<_>>()
            .join(","),
    );
    emit("Diagnostics", sf.diagnostics().len().to_string());
    emit("JSDiagnostics", sf.js_diagnostics().len().to_string());
    emit("JSDocDiagnostics", sf.jsdoc_diagnostics().len().to_string());
    let map = sf.position_map();
    let mut values = vec![
        std::ptr::eq(map, sf.position_map()).to_string(),
        map.is_ascii_only().to_string(),
    ];
    for offset in 0..=sf.text().as_bytes().len() {
        values.push(
            map.utf8_to_utf16(isize::try_from(offset).unwrap())
                .to_string(),
        );
    }
    emit("GetPositionMap", values.join(","));
}
#[test]
fn matches_pinned_go_source_file_accessor_observations() {
    let string = |bytes: &[u8]| JsString::from_bytes(bytes);
    let mut f = AstBuilder::new(SourceText::default(), &ts_arena::Counters::new());
    let a = f.new_identifier(string(b"a"));
    let b = f.new_identifier(string(b"b"));
    let opts = SourceFileParseOptions {
        file_name: string(b"/canonical.ts"),
        path: string(b"/cache/canonical.ts"),
        external_module_indicator_options: ExternalModuleIndicatorOptions {
            jsx: true,
            force: false,
        },
    };
    let source = f.new_source_file(
        opts.clone(),
        SourceText::from_bytes(b"a\r\n\xf0\x9f\x98\x80z".to_vec()),
        None,
        None,
    );
    let mut actual = BTreeMap::new();
    source_check(&mut actual, "source/plain", &f, source, source, [a, b]);
    for kind in [-1, 0, 1, 2, 3, 4, 5, 6, 7, 8, 100] {
        f.source_file_mut(source).unwrap().script_kind = ts_core::ScriptKind(kind);
        actual.insert(
            format!("source/IsJS/{kind}"),
            format!("value:{}", f.view().source_file(source).unwrap().is_js()),
        );
    }
    let imports = f.source_nodes(vec![Some(a), None, Some(b)]).unwrap();
    let diagnostic = || {
        Diagnostic::external(
            None,
            TextRange::new(0, 0),
            JsString::default(),
            0,
            0,
            JsString::default(),
        )
    };
    let sf = f.source_file_mut(source).unwrap();
    sf.imports = imports;
    sf.set_diagnostics(vec![diagnostic()]);
    sf.set_js_diagnostics(vec![diagnostic(), diagnostic()]);
    sf.set_jsdoc_diagnostics(vec![diagnostic(), diagnostic(), diagnostic()]);
    sf.set_has_lazy_jsdoc(true);
    actual.insert(
        "source/lazy/true".into(),
        format!("value:{}", sf.has_lazy_jsdoc),
    );
    sf.set_has_lazy_jsdoc(false);
    actual.insert(
        "source/lazy/false".into(),
        format!("value:{}", sf.has_lazy_jsdoc),
    );
    sf.set_content_mapper_info(ContentMapperSourceFileInfo {
        original_text: SourceText::from_bytes(b"ignored".to_vec()),
        virtual_file_name: string(b"virtual"),
        ..Default::default()
    });
    source_check(
        &mut actual,
        "source/empty-mapper",
        &f,
        source,
        source,
        [a, b],
    );
    let supplemental = f.new_source_file(
        SourceFileParseOptions {
            file_name: string(b"/supp.ts"),
            ..Default::default()
        },
        SourceText::from_bytes(b"transformed".to_vec()),
        None,
        None,
    );
    let directives = f
        .source_diagnostic_directives(vec![MappedDiagnosticDirective::default()])
        .unwrap();
    let supplementals = f.source_nodes(vec![Some(source)]).unwrap();
    f.source_file_mut(supplemental)
        .unwrap()
        .set_content_mapper_info(ContentMapperSourceFileInfo {
            content_mapper: string(b"mapper"),
            transform_identity: string(b"v1"),
            original_text: SourceText::from_bytes(b"original\xff".to_vec()),
            virtual_file_name: string(b"virtual"),
            canonical_source_file: Some(source),
            parse_options: opts,
            diagnostic_directives: directives,
            supplemental_source_files: supplementals,
            ..Default::default()
        });
    source_check(
        &mut actual,
        "source/supplemental",
        &f,
        supplemental,
        source,
        [a, b],
    );
    actual.insert(
        "source/duplicate-info".into(),
        outcome(|| {
            f.source_file_mut(supplemental)
                .unwrap()
                .set_content_mapper_info(ContentMapperSourceFileInfo::default());
            "unexpected".into()
        }),
    );
    let expected: BTreeMap<String, String> =
        include_str!("../../../data/s06/accessor-observations.tsv")
            .lines()
            .filter(|line| line.starts_with("source/"))
            .map(|line| {
                let (k, v) = line.split_once('\t').unwrap();
                (k.into(), v.into())
            })
            .collect();
    assert_eq!(actual, expected);
}
