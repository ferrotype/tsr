use std::collections::BTreeMap;
use ts_ast::{
    utilities_tail as u, AstBuilder, FactoryMethods, JsString, NodeId, NodeKind, NodeListId,
    SyntaxKind as K,
};
use ts_core::TextRange;
use ts_jsstring::SourceText;

fn string(bytes: &[u8]) -> JsString {
    JsString::from_bytes(bytes)
}

fn list(f: &mut AstBuilder, nodes: &[NodeId]) -> NodeListId {
    let nodes = f
        .node_slice(nodes.iter().copied().map(Some).collect())
        .unwrap();
    f.new_list(TextRange::new(-1, -1), nodes).unwrap()
}

#[test]
fn matches_pinned_go_tail_utility_observations() {
    let mut actual = BTreeMap::new();
    let mut f = AstBuilder::new(SourceText::default(), &ts_arena::Counters::new());
    for raw in i16::MIN..=i16::MAX {
        let kind = NodeKind::from_raw(raw);
        let id = f.new_token(kind);
        let node = f.view().node(id).unwrap();
        let values = [
            u::is_import_or_import_equals_declaration(&node),
            u::has_inferred_type(&node),
            u::is_keyword(kind),
            u::is_non_contextual_keyword(kind),
            u::is_expando_property_declaration(Some(&node)),
        ];
        let mask = values
            .into_iter()
            .enumerate()
            .fold(0u8, |mask, (i, v)| mask | (u8::from(v) << i));
        actual.insert(format!("kind/{raw}"), mask.to_string());
    }
    let mut emit = |name: String, value: bool| {
        assert!(actual.insert(name, value.to_string()).is_none());
    };
    let one = f.new_identifier(string(b"x"));
    let other = f.new_identifier(string(b"x"));
    let different = f.new_identifier(string(b"y"));
    for i in 0..3 {
        let items = match i {
            0 => None,
            1 => Some(list(&mut f, &[])),
            _ => Some(list(&mut f, &[one])),
        };
        let object = f.new_object_literal_expression(items, false);
        let array = f.new_array_literal_expression(items, false);
        emit(
            format!("empty/object/{i}"),
            u::is_empty_object_literal(f.view(), &f.view().node(object).unwrap()).unwrap(),
        );
        emit(
            format!("empty/array/{i}"),
            u::is_empty_array_literal(f.view(), &f.view().node(array).unwrap()).unwrap(),
        );
    }
    emit(
        "empty/other".into(),
        u::is_empty_object_literal(f.view(), &f.view().node(one).unwrap()).unwrap()
            || u::is_empty_array_literal(f.view(), &f.view().node(one).unwrap()).unwrap(),
    );
    let rest = f.new_token(K::DotDotDotToken.into());
    let rest_nodes = [
        f.new_parameter_declaration(None, Some(rest), Some(one), None, None, None),
        f.new_binding_element(Some(rest), None, Some(one), None),
        f.new_spread_element(Some(one)),
        f.new_spread_assignment(Some(one)),
        one,
    ];
    for (i, id) in rest_nodes.into_iter().enumerate() {
        let expected = match i {
            2 | 3 => Some(id),
            4 => None,
            _ => Some(rest),
        };
        emit(
            format!("rest/indicator/{i}"),
            u::get_rest_indicator_of_binding_or_assignment_element(f.view(), id).unwrap()
                == expected,
        );
    }
    let doc = f.new_js_doc(None, None);
    f.node_mut(doc).unwrap().set_parent(Some(one));
    let link = f.new_js_doc_name_reference(Some(other));
    f.node_mut(link).unwrap().set_parent(Some(doc));
    f.node_mut(other).unwrap().set_parent(Some(link));
    emit(
        "jsdoc/context/unflagged".into(),
        u::is_js_doc_name_reference_context(f.view(), other).unwrap(),
    );
    f.node_mut(other)
        .unwrap()
        .set_flags(ts_ast::node_flags::JS_DOC);
    emit(
        "jsdoc/context/flagged".into(),
        u::is_js_doc_name_reference_context(f.view(), other).unwrap(),
    );
    emit(
        "jsdoc/root".into(),
        u::get_js_doc_root(f.view(), other).unwrap() == Some(doc),
    );
    emit(
        "jsdoc/host".into(),
        u::get_js_doc_host(f.view(), other).unwrap() == Some(one),
    );
    emit(
        "jsdoc/root/self".into(),
        u::get_js_doc_root(f.view(), doc).unwrap().is_none(),
    );
    emit(
        "jsdoc/host/absent".into(),
        u::get_js_doc_host(f.view(), one).unwrap().is_none(),
    );
    for (i, kind) in [
        K::PropertyAssignment,
        K::ExportAssignment,
        K::PropertyDeclaration,
        K::VariableDeclaration,
        K::SatisfiesExpression,
        K::ReturnStatement,
        K::VariableStatement,
        K::ExpressionStatement,
        K::Identifier,
    ]
    .into_iter()
    .enumerate()
    {
        let parent = f.new_token(kind.into());
        f.node_mut(different).unwrap().set_parent(Some(parent));
        emit(
            format!("jsdoc/next/{i}"),
            u::get_next_js_doc_comment_location(f.view(), different).unwrap() == Some(parent),
        );
    }
    let items = list(&mut f, &[one, different]);
    let declarations = f.new_variable_declaration_list(Some(items), 0);
    f.node_mut(one).unwrap().set_parent(Some(declarations));
    f.node_mut(different)
        .unwrap()
        .set_parent(Some(declarations));
    emit(
        "jsdoc/next/first".into(),
        u::get_next_js_doc_comment_location(f.view(), one).unwrap() == Some(declarations),
    );
    emit(
        "jsdoc/next/second".into(),
        u::get_next_js_doc_comment_location(f.view(), different)
            .unwrap()
            .is_none(),
    );
    let num = f.new_numeric_literal(string(b"1"), 0);
    let big = f.new_big_int_literal(string(b"1n"), 0);
    let truth = f.new_token(K::TrueKeyword.into());
    for (i, operator) in [K::PlusToken, K::MinusToken, K::ExclamationToken]
        .into_iter()
        .enumerate()
    {
        for (j, operand) in [num, big, truth].into_iter().enumerate() {
            let id = f.new_prefix_unary_expression(operator.into(), Some(operand));
            for include in [false, true] {
                emit(
                    format!("primitive/{i}/{j}/{include}"),
                    u::is_primitive_literal_value(f.view(), &f.view().node(id).unwrap(), include)
                        .unwrap(),
                );
            }
        }
    }
    for kind in [
        K::TrueKeyword,
        K::FalseKeyword,
        K::NumericLiteral,
        K::StringLiteral,
        K::NoSubstitutionTemplateLiteral,
        K::BigIntLiteral,
        K::NullKeyword,
    ] {
        let id = f.new_token(kind.into());
        for include in [false, true] {
            emit(
                format!("primitive/kind/{}/{include}", kind as i16),
                u::is_primitive_literal_value(f.view(), &f.view().node(id).unwrap(), include)
                    .unwrap(),
            );
        }
    }
    for (i, name) in [
        b"Infinity".as_slice(),
        b"-Infinity",
        b"NaN",
        b"+Infinity",
        b"nan",
        b"Infinity\0",
        b"\xff",
    ]
    .into_iter()
    .enumerate()
    {
        emit(
            format!("number-name/{i}"),
            u::is_infinity_or_na_n_string(name),
        );
    }
    let empty = list(&mut f, &[]);
    let args = list(&mut f, &[other, different]);
    let types = [
        None,
        Some(f.new_array_type_node(Some(one))),
        Some(f.new_type_reference_node(Some(one), None)),
        Some(f.new_type_reference_node(Some(one), Some(empty))),
        Some(f.new_type_reference_node(Some(one), Some(args))),
        Some(one),
    ];
    for (i, id) in types.into_iter().enumerate() {
        let expected = match i {
            1 => Some(one),
            4 => Some(other),
            _ => None,
        };
        emit(
            format!("rest/type/{i}"),
            u::get_rest_parameter_element_type(f.view(), id).unwrap() == expected,
        );
    }
    let this = f.new_token(K::ThisKeyword.into());
    let pairs = [
        (one, other),
        (one, different),
        (one, truth),
        (this, this),
        (
            f.new_jsx_namespaced_name(Some(one), Some(other)),
            f.new_jsx_namespaced_name(Some(other), Some(one)),
        ),
        (
            f.new_jsx_namespaced_name(Some(one), Some(other)),
            f.new_jsx_namespaced_name(Some(other), Some(different)),
        ),
        (
            f.new_property_access_expression(Some(one), None, Some(other), 0),
            f.new_property_access_expression(Some(other), None, Some(one), 0),
        ),
        (
            f.new_property_access_expression(Some(one), None, Some(other), 0),
            f.new_property_access_expression(Some(different), None, Some(one), 0),
        ),
    ];
    for (i, (left, right)) in pairs.into_iter().enumerate() {
        emit(
            format!("tag/equal/{i}"),
            u::tag_names_are_equivalent(f.view(), left, right).unwrap(),
        );
    }
    let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        u::tag_names_are_equivalent(f.view(), num, num)
    }))
    .unwrap_err();
    emit(
        "tag/unhandled".into(),
        panic.downcast_ref::<&str>() == Some(&"Unhandled case in TagNamesAreEquivalent"),
    );
    let tag = f.new_js_doc_unknown_tag(Some(one), None);
    f.node_mut(one).unwrap().set_parent(Some(tag));
    f.node_mut(other).unwrap().set_parent(Some(tag));
    emit(
        "tag/name/yes".into(),
        u::is_tag_name(f.view(), one).unwrap(),
    );
    emit(
        "tag/name/no".into(),
        u::is_tag_name(f.view(), other).unwrap(),
    );
    emit(
        "tag/name/root".into(),
        u::is_tag_name(f.view(), tag).unwrap(),
    );
    let access = f.new_element_access_expression(Some(truth), None, Some(one), 0);
    f.node_mut(one).unwrap().set_parent(Some(access));
    f.node_mut(other).unwrap().set_parent(Some(access));
    emit(
        "argument/nil".into(),
        u::is_argument_of_element_access_expression(f.view(), None).unwrap(),
    );
    emit(
        "argument/yes".into(),
        u::is_argument_of_element_access_expression(f.view(), Some(one)).unwrap(),
    );
    emit(
        "argument/no".into(),
        u::is_argument_of_element_access_expression(f.view(), Some(other)).unwrap(),
    );
    emit(
        "argument/root".into(),
        u::is_argument_of_element_access_expression(f.view(), Some(access)).unwrap(),
    );
    emit(
        "expando/nil".into(),
        u::is_expando_property_declaration(None::<&ts_ast::NodeRead<'_>>),
    );
    let super_node = f.new_token(K::SuperKeyword.into());
    let nodes = [
        f.new_property_access_expression(Some(super_node), None, Some(one), 0),
        f.new_element_access_expression(Some(super_node), None, Some(one), 0),
        f.new_property_access_expression(Some(truth), None, Some(one), 0),
        one,
    ];
    for (i, id) in nodes.into_iter().enumerate() {
        emit(
            format!("super/{i}"),
            u::is_super_property(f.view(), &f.view().node(id).unwrap()).unwrap(),
        );
    }
    let proto = f.new_identifier(string(b"__proto__"));
    let proto_string = f.new_string_literal(string(b"__proto__"), 0);
    let names = [
        proto,
        proto_string,
        f.new_numeric_literal(string(b"__proto__"), 0),
        f.new_no_substitution_template_literal(string(b"__proto__"), 0),
        f.new_computed_property_name(Some(proto)),
        one,
    ];
    for (i, id) in names.into_iter().enumerate() {
        emit(
            format!("proto/{i}"),
            u::is_proto_setter(&f.view().node(id).unwrap()),
        );
    }
    let template = f.new_no_substitution_template_literal(string(b"x"), 0);
    let types = [
        f.new_literal_type_node(Some(proto_string)),
        f.new_literal_type_node(Some(template)),
        f.new_literal_type_node(Some(num)),
        one,
    ];
    for (i, id) in types.into_iter().enumerate() {
        emit(
            format!("literal-type/{i}"),
            u::is_string_literal_like_type(f.view(), &f.view().node(id).unwrap()).unwrap(),
        );
    }
    let equals = f.new_token(K::EqualsToken.into());
    let plus = f.new_token(K::PlusToken.into());
    let nodes = [
        f.new_property_assignment(None, Some(proto), None, None, Some(one)),
        f.new_property_assignment(None, Some(one), None, None, None),
        f.new_shorthand_property_assignment(None, Some(one), None, None, None, None),
        f.new_shorthand_property_assignment(None, Some(one), None, None, None, Some(truth)),
        f.new_variable_declaration(Some(one), None, None, Some(truth)),
        f.new_variable_declaration(Some(proto_string), None, None, Some(truth)),
        f.new_parameter_declaration(None, None, Some(one), None, None, Some(truth)),
        f.new_parameter_declaration(None, Some(rest), Some(one), None, None, Some(truth)),
        f.new_binding_element(None, None, Some(one), Some(truth)),
        f.new_binding_element(Some(rest), None, Some(one), Some(truth)),
        f.new_property_declaration(None, Some(one), None, None, Some(truth)),
        f.new_property_declaration(None, Some(one), None, None, None),
        f.new_binary_expression(None, Some(one), None, Some(equals), Some(truth)),
        f.new_binary_expression(None, Some(one), None, Some(plus), Some(truth)),
        f.new_token(K::ExportAssignment.into()),
        one,
    ];
    for (i, id) in nodes.into_iter().enumerate() {
        emit(
            format!("named/{i}"),
            u::is_named_evaluation_source(f.view(), &f.view().node(id).unwrap()).unwrap(),
        );
    }
    let expected: BTreeMap<_, _> =
        include_str!("../../../data/s06/utilities-tail-observations.tsv")
            .lines()
            .filter(|line| !line.starts_with('#'))
            .map(|line| {
                let (name, value) = line.split_once('\t').unwrap();
                (name.to_owned(), value.to_owned())
            })
            .collect();
    assert_eq!(actual.len(), expected.len());
    for (name, value) in expected {
        assert_eq!(actual.get(&name), Some(&value), "{name}");
    }
}
