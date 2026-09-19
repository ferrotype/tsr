use super::*;
use crate::{AstBuilder, Factory, FactoryMethods, Node, NodeData, TokenData};
use std::collections::BTreeMap;
use ts_arena::Counters;
use ts_core::TextRange;
use ts_jsstring::SourceText;

fn text(value: &str) -> JsString {
    JsString::from_bytes(value.as_bytes())
}
fn list(factory: &mut AstBuilder, nodes: Vec<Option<NodeId>>) -> NodeListId {
    let nodes = factory.node_slice(nodes).unwrap();
    factory.new_list(TextRange::new(-1, -1), nodes).unwrap()
}

#[test]
fn every_signed_kind_matches_twenty_seven_pinned_go_predicates() {
    let expected = include_bytes!("../../../data/s06/ast-utilities-middle-kinds.bin");
    assert_eq!(expected.len(), 65_536 * 4);
    for (index, raw) in (i16::MIN..=i16::MAX).enumerate() {
        let node = Node::from_factory_parts(NodeKind::from_raw(raw), TokenData {}.into());
        let values = [
            is_js_doc_link_like(&node),
            is_js_doc_tag(&node),
            is_question_token(Some(&node)),
            is_js_doc_node(&node),
            is_property_access_or_qualified_name(&node),
            is_break_or_continue_statement(&node),
            is_parameter_like(&node),
            node_has_kind(Some(&node), K::Identifier.into()),
            is_contextual_keyword(node.kind()),
            is_parameter_property_modifier(node.kind()),
            has_type_arguments(&node),
            is_type_reference_type(&node),
            is_variable_like(&node),
            is_variable_parameter_or_property(&node),
            is_object_type_declaration(&node),
            is_type_keyword_token(&node),
            is_resolution_mode_override_host(Some(&node)),
            is_string_text_containing_node(&node),
            is_template_literal_kind(node.kind()),
            is_template_literal_token(&node),
            is_late_visibility_painted_statement(&node),
            is_jsx_opening_like_element(&node),
            is_call_or_new_expression(&node),
            is_trivia(node.kind()),
            has_comment(node.kind()),
            is_declaration_binding_element(&node),
            is_jsx_call_like(&node),
        ];
        let mask = values
            .into_iter()
            .enumerate()
            .fold(0_u32, |mask, (bit, value)| mask | u32::from(value) << bit);
        assert_eq!(
            mask,
            u32::from_le_bytes(expected[index * 4..index * 4 + 4].try_into().unwrap()),
            "signed kind {raw}"
        );
    }
}

#[test]
fn graph_flags_slice_identity_and_position_search_match_pinned_go() {
    let mut expected: BTreeMap<_, _> =
        include_str!("../../../data/s06/ast-utilities-middle-behaviors.tsv")
            .lines()
            .map(|line| {
                let mut fields = line.split('\t');
                (
                    fields.next().unwrap(),
                    fields
                        .map(|value| value.parse::<i64>().unwrap())
                        .collect::<Vec<_>>(),
                )
            })
            .collect();
    assert_eq!(expected.len(), 26);
    let mut emit = |label: &str, values: &[i64]| {
        assert_eq!(values, expected.remove(label).unwrap(), "{label}");
    };
    let b = i64::from;
    let mut f = AstBuilder::new(SourceText::default(), &Counters::new());
    emit(
        "nullable",
        &[
            b(is_question_token(None::<&crate::Node>)),
            b(node_has_kind(None::<&crate::Node>, K::Unknown.into())),
            b(is_resolution_mode_override_host(None::<&crate::Node>)),
            b(is_plain_js_file(None, Tristate::UNKNOWN)),
        ],
    );
    for (index, flags) in [
        0,
        u32::MAX,
        modifier_flags::PUBLIC
            | modifier_flags::ABSTRACT
            | modifier_flags::READONLY
            | modifier_flags::ASYNC,
    ]
    .into_iter()
    .enumerate()
    {
        let mut order = Vec::new();
        let nodes = create_modifiers_from_modifier_flags(flags, |kind| {
            order.push(i64::from(kind.raw()));
            Some(f.new_token(kind))
        });
        assert_eq!(nodes.as_ref().map_or(0, Vec::len), order.len());
        emit(&format!("modifiers/{index}"), &order);
    }
    let nil_modifiers = create_modifiers_from_modifier_flags(u32::MAX, |_| None).unwrap();
    emit(
        "modifiers/nil",
        &[
            nil_modifiers.len() as i64,
            b(nil_modifiers.iter().all(Option::is_none)),
        ],
    );
    emit(
        "modifiers/empty",
        &[0, modifier_flags::DEPRECATED].map(|flags| {
            b(create_modifiers_from_modifier_flags(flags, |_| {
                panic!("unexpected modifier callback")
            })
            .is_none())
        }),
    );
    for (index, flags) in [0, token_flags::UNTERMINATED, -1].into_iter().enumerate() {
        let numeric = f.new_numeric_literal(text("1"), flags);
        let template = f.new_no_substitution_template_literal(text("x"), flags);
        let head = f.new_template_head(text("x"), text("x"), flags);
        emit(
            &format!("literal/{index}"),
            &[
                b(is_unterminated_literal(&f.node(numeric))),
                b(is_unterminated_literal(&f.node(template))),
                b(is_unterminated_literal(&f.node(head))),
            ],
        );
    }
    let ordinary = f.new_identifier(text("ordinary"));
    let space = f.new_jsx_text(text(" "), true);
    let word = f.new_jsx_text(text("x"), false);
    let empty = f.new_jsx_expression(None, None);
    let filled = f.new_jsx_expression(None, Some(ordinary));
    let children = [
        Some(ordinary),
        Some(space),
        Some(word),
        Some(empty),
        Some(filled),
    ];
    let filtered = get_semantic_jsx_children(f.view(), Some(&children))
        .unwrap()
        .unwrap();
    let retained: Vec<_> = filtered
        .iter()
        .map(|id| children.iter().position(|original| original == id).unwrap() as i64)
        .collect();
    emit("jsx/filter", &retained);
    let kept = [Some(ordinary), Some(word), Some(filled)];
    let same = get_semantic_jsx_children(f.view(), Some(&kept))
        .unwrap()
        .unwrap();
    emit(
        "jsx/identity",
        &[
            b(matches!(same, Cow::Borrowed(_))),
            b(is_whitespace_only_jsx_text(&f.node(space))),
            b(is_whitespace_only_jsx_text(&f.node(word))),
            b(is_non_whitespace_token(&f.node(space))),
            b(is_non_whitespace_token(&f.node(word))),
        ],
    );
    let empty_children = get_semantic_jsx_children(f.view(), Some(&[])).unwrap();
    assert!(matches!(empty_children, Some(Cow::Borrowed(_))));
    let discarded = [Some(space)];
    let discarded = get_semantic_jsx_children(f.view(), Some(&discarded)).unwrap();
    assert!(matches!(discarded, Some(Cow::Owned(_))));
    emit(
        "jsx/empty",
        &[
            b(get_semantic_jsx_children(f.view(), None).unwrap().is_none()),
            b(empty_children.is_none()),
            b(discarded.is_none()),
            discarded.unwrap().len() as i64,
        ],
    );
    let label = f.new_identifier(text("label"));
    let empty = f.new_empty_statement();
    let statement = f.new_labeled_statement(Some(label), Some(empty));
    f.node_mut(label).unwrap().set_parent(Some(statement));
    emit(
        "label/statement",
        &[
            b(is_label_name(f.view(), label).unwrap()),
            b(is_label_of_labeled_statement(f.view(), label).unwrap()),
            b(is_jump_statement_target(f.view(), label).unwrap()),
        ],
    );
    let jump = f.new_break_statement(Some(label));
    f.node_mut(label).unwrap().set_parent(Some(jump));
    emit(
        "label/jump",
        &[
            b(is_label_name(f.view(), label).unwrap()),
            b(is_label_of_labeled_statement(f.view(), label).unwrap()),
            b(is_jump_statement_target(f.view(), label).unwrap()),
        ],
    );
    let base = f.new_identifier(text("base"));
    let name = f.new_identifier(text("name"));
    let access = f.new_property_access_expression(Some(base), None, Some(name), 0);
    f.node_mut(name).unwrap().set_parent(Some(access));
    f.node_mut(base).unwrap().set_parent(Some(access));
    emit(
        "access/property",
        &[
            b(is_right_side_of_property_access(f.view(), name).unwrap()),
            b(is_right_side_of_property_access(f.view(), base).unwrap()),
            b(is_right_side_of_qualified_name_or_property_access(f.view(), name).unwrap()),
            b(climb_past_property_access(f.view(), name).unwrap() == access),
            b(get_first_identifier(f.view(), access).unwrap() == base),
            b(get_leftmost_access_expression(f.view(), access).unwrap() == base),
        ],
    );
    let element = f.new_element_access_expression(Some(base), None, Some(name), 0);
    f.node_mut(name).unwrap().set_parent(Some(element));
    emit(
        "access/element",
        &[
            b(is_argument_expression_of_element_access(f.view(), name).unwrap()),
            b(climb_past_property_or_element_access(f.view(), name).unwrap() == element),
        ],
    );
    let qualified = f.new_qualified_name(Some(base), Some(name));
    f.node_mut(name).unwrap().set_parent(Some(qualified));
    emit(
        "access/qualified",
        &[
            b(is_right_side_of_qualified_name_or_property_access(f.view(), name).unwrap()),
            b(get_first_identifier(f.view(), qualified).unwrap() == base),
        ],
    );
    let require = f.new_identifier(text("require"));
    let literal = f.new_string_literal(text("package"), 0);
    let arguments = list(&mut f, vec![Some(literal)]);
    let call = f.new_call_expression(Some(require), None, None, Some(arguments), 0);
    emit(
        "call/require",
        &[
            b(is_require_call(f.view(), &f.node(call), true).unwrap()),
            b(is_require_call(f.view(), &f.node(call), false).unwrap()),
            b(is_super_call(f.view(), &f.node(call)).unwrap()),
            b(is_call_like_expression(f.view(), &f.node(call)).unwrap()),
            b(is_call_like_or_function_like_expression(f.view(), &f.node(call)).unwrap()),
            b(get_invoked_expression(f.view(), call).unwrap() == Some(require)),
            b(
                select_expression_of_call_or_new_expression_or_decorator(&f.node(call))
                    == Some(require),
            ),
        ],
    );
    let nodes = f.node_slice(vec![Some(ordinary)]).unwrap();
    f.set_list_nodes(arguments, nodes).unwrap();
    emit(
        "call/nonliteral",
        &[
            b(is_require_call(f.view(), &f.node(call), true).unwrap()),
            b(is_require_call(f.view(), &f.node(call), false).unwrap()),
        ],
    );
    let super_token = f.new_token(K::SuperKeyword.into());
    {
        let mut node = f.node_mut(call).unwrap();
        let NodeData::CallExpression(data) = node.data_mut() else {
            panic!("call")
        };
        data.expression = Some(super_token);
    }
    emit(
        "call/super",
        &[
            b(is_super_call(f.view(), &f.node(call)).unwrap()),
            b(is_require_call(f.view(), &f.node(call), false).unwrap()),
        ],
    );
    let const_name = f.new_identifier(text("const"));
    let typ = f.new_type_reference_node(Some(const_name), None);
    let assertion = f.new_as_expression(Some(ordinary), Some(typ));
    emit(
        "const/assertion",
        &[
            b(is_const_type_reference(f.view(), &f.node(typ)).unwrap()),
            b(is_const_assertion(f.view(), &f.node(assertion)).unwrap()),
            b(is_const_assertion(f.view(), &f.node(ordinary)).unwrap()),
        ],
    );
    let args = list(&mut f, vec![Some(ordinary)]);
    {
        let mut node = f.node_mut(typ).unwrap();
        let NodeData::TypeReferenceNode(data) = node.data_mut() else {
            panic!("type")
        };
        data.type_arguments = Some(args);
    }
    emit(
        "const/arguments",
        &[
            b(is_const_type_reference(f.view(), &f.node(typ)).unwrap()),
            b(is_const_assertion(f.view(), &f.node(assertion)).unwrap()),
        ],
    );
    let text_slice = f.text_slice(vec![text("comment")]).unwrap();
    let comment = f.new_js_doc_text(text_slice);
    let comments = list(&mut f, vec![Some(comment)]);
    let doc = f.new_js_doc(Some(comments), None);
    f.node_mut(comment).unwrap().set_parent(Some(doc));
    let alternate = f
        .new_list(
            TextRange::new(-1, -1),
            f.view().list(comments).unwrap().nodes(),
        )
        .unwrap();
    emit(
        "jsdoc/identity",
        &[
            b(is_js_doc_single_comment_node(f.view(), &f.node(doc)).unwrap()),
            b(is_js_doc_single_comment_node_list(f.view(), Some(comments)).unwrap()),
            b(is_js_doc_single_comment_node_list(f.view(), Some(alternate)).unwrap()),
            b(is_js_doc_single_comment_node_comment(f.view(), Some(comment)).unwrap()),
            b(is_js_doc_single_comment_node_list(f.view(), None).unwrap()),
            b(is_js_doc_single_comment_node_comment(f.view(), None).unwrap()),
        ],
    );
    let text_slice = f.text_slice(vec![text("second")]).unwrap();
    let second = f.new_js_doc_text(text_slice);
    let nodes = f.node_slice(vec![Some(comment), Some(second)]).unwrap();
    f.set_list_nodes(comments, nodes).unwrap();
    emit(
        "jsdoc/multiple",
        &[
            b(is_js_doc_single_comment_node(f.view(), &f.node(doc)).unwrap()),
            b(is_js_doc_single_comment_node_comment(f.view(), Some(comment)).unwrap()),
        ],
    );
    let left = f.new_identifier(text("a"));
    let right = f.new_identifier(text("b"));
    f.node_mut(left)
        .unwrap()
        .set_range(TextRange::new(i64::from(i32::MIN), i64::from(i32::MAX)));
    f.node_mut(right)
        .unwrap()
        .set_range(TextRange::new(i64::from(i32::MAX), i64::from(i32::MIN)));
    emit(
        "positions/extreme",
        &[
            compare_node_positions(&f.node(left), &f.node(right)),
            compare_node_positions(&f.node(right), &f.node(left)),
        ],
    );
    f.node_mut(left).unwrap().set_range(TextRange::new(1, 2));
    f.node_mut(right).unwrap().set_range(TextRange::new(1, 4));
    let other = f.new_identifier(text("c"));
    f.node_mut(other).unwrap().set_range(TextRange::new(1, 2));
    emit(
        "positions/search",
        &[
            index_of_node(
                f.view(),
                &[Some(left), Some(other), Some(right)],
                Some(other),
            )
            .unwrap(),
            index_of_node(
                f.view(),
                &[Some(left), Some(other), Some(right)],
                Some(right),
            )
            .unwrap(),
            index_of_node(f.view(), &[], Some(left)).unwrap(),
            index_of_node(f.view(), &[], None).unwrap(),
        ],
    );
    assert!(
        expected.is_empty(),
        "unexecuted Go observations: {expected:?}"
    );
}
