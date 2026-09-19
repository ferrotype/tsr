use std::collections::BTreeMap;
use ts_ast::{utilities as u, AstBuilder, FactoryMethods, NodeKind, SyntaxKind as K};
use ts_core::TextRange;
use ts_jsstring::SourceText;

#[test]
fn matches_pinned_go_front_utility_observations() {
    let mut actual = BTreeMap::<String, String>::new();
    let mut f = AstBuilder::new(SourceText::default(), &ts_arena::Counters::new());
    for raw in i16::MIN..=i16::MAX {
        let kind = NodeKind::from_raw(raw);
        let id = f.new_token(kind);
        let node = f.view().node(id).unwrap();
        let values = [
            u::is_object_binding_or_assignment_element(&node),
            u::is_property_name_literal(&node),
            u::is_property_name(&node),
            u::is_class_element(&node),
            u::is_method_or_accessor(&node),
            u::is_type_element(&node),
            u::is_object_literal_element(&node),
            u::is_jsx_child(&node),
            u::can_have_symbol(&node),
            u::can_have_illegal_modifiers(&node),
            u::can_have_modifiers(&node),
            u::is_unary_expression_kind(kind),
            u::is_expression_kind(kind),
            u::is_declaration_statement_kind(kind),
            u::is_statement_kind_but_not_declaration_kind(kind),
            u::is_binding_pattern(&node),
            u::is_accessor(&node),
            u::is_member_name(&node),
            u::is_entity_name(&node),
            u::is_boolean_literal(&node),
            u::is_string_literal_like(&node),
            u::is_string_or_numeric_literal_like(&node),
            u::is_assertion_expression(&node),
            u::is_access_expression(&node),
            u::is_class_like(&node),
            u::is_class_or_interface_like(&node),
            u::is_jsx_attribute_like(&node),
            u::is_function_expression_or_arrow_function(&node),
            u::is_module_or_enum_declaration(&node),
            u::is_import_or_export_specifier(&node),
            u::node_is_synthesized(&node),
            u::is_modifier(&node),
            u::is_modifier_like(&node),
            u::is_literal_expression(&node),
            u::is_declaration_statement(&node),
            u::is_statement_but_not_declaration(&node),
            u::is_type_node(&node),
            u::is_for_in_or_of_statement(Some(&node)),
            u::is_function_like_declaration(Some(&node)),
            u::is_function_like(Some(&node)),
            u::is_function_like_or_class_static_block_declaration(Some(&node)),
            u::is_function_or_source_file(&node),
            u::is_in_js_file(Some(&node)),
            u::is_in_json_file(&node),
            u::is_compound_assignment(kind),
            u::is_logical_binary_operator(kind),
            u::is_logical_or_coalescing_binary_operator(kind),
            u::is_js_doc_kind(kind),
            u::is_any_import_syntax(&node),
            u::is_import_node(&node),
            u::is_any_import_or_re_export(&node),
            u::is_function_like_declaration_kind(kind),
            u::is_function_like_kind(kind),
            u::is_type_node_kind(kind),
            u::is_optional_chain(&node),
            u::is_optional_chain_root(&node),
        ];
        let mask = values
            .into_iter()
            .enumerate()
            .fold(0u64, |mask, (i, v)| mask | (u64::from(v) << i));
        actual.insert(format!("kind/{raw}"), format!("{mask:x}"));
    }
    let mut emit = |name: String, value: String| {
        assert!(actual.insert(name, value).is_none());
    };
    macro_rules! emit {
        ($name:expr,$value:expr) => {
            emit($name.to_string(), $value.to_string())
        };
    }
    emit!(
        "nil/for",
        u::is_for_in_or_of_statement(None::<&ts_ast::Node>)
    );
    emit!("nil/function", u::is_function_like(None::<&ts_ast::Node>));
    emit!(
        "nil/function-decl",
        u::is_function_like_declaration(None::<&ts_ast::Node>)
    );
    emit!(
        "nil/function-static",
        u::is_function_like_or_class_static_block_declaration(None::<&ts_ast::NodeRead<'_>>)
    );
    emit!("nil/js", u::is_in_js_file(None::<&ts_ast::Node>));
    emit!("nil/block", u::is_function_block(f.view(), None).unwrap());
    emit!(
        "nil/object-method",
        u::is_object_literal_method(f.view(), None).unwrap()
    );
    emit!(
        "nil/descendant",
        u::is_node_descendant_of(f.view(), None, None).unwrap()
    );
    for (i, pos) in [i64::MIN, -1, 0, 1, i64::from(i32::MAX), 1 << 31, i64::MAX]
        .into_iter()
        .enumerate()
    {
        emit!(
            format!("position/{i}"),
            u::position_is_synthesized(pos as isize)
        );
    }
    for (i, (pos, end)) in [
        (-1, -1),
        (0, 0),
        (0, 1),
        (1, 0),
        (-1, 1),
        (1, -1),
        (1 << 31, 1 << 31),
        (1 << 32, 1 << 32),
    ]
    .into_iter()
    .enumerate()
    {
        let id = f.new_token(K::Identifier.into());
        f.node_mut(id).unwrap().set_range(TextRange::new(pos, end));
        let node = f.view().node(id).unwrap();
        emit!(format!("range/{i}"), u::range_is_synthesized(node.range()));
        emit!(format!("node-range/{i}"), u::node_is_synthesized(&node));
    }
    let mut flags = vec![0, u32::MAX];
    flags.extend((0..32).map(|bit| 1 << bit));
    for (i, flag) in flags.into_iter().enumerate() {
        let id = f.new_token(K::Identifier.into());
        f.node_mut(id).unwrap().set_flags(flag);
        let node = f.view().node(id).unwrap();
        emit!(format!("flags/js/{i}"), u::is_in_js_file(Some(&node)));
        emit!(format!("flags/json/{i}"), u::is_in_json_file(&node));
    }
    let a = f.new_token(K::Identifier.into());
    let b = f.new_token(K::ClassExpression.into());
    let c = f.new_source_file(
        ts_ast::SourceFileParseOptions {
            file_name: ts_ast::JsString::from_bytes(b"/s06/front.ts".as_slice()),
            ..Default::default()
        },
        SourceText::default(),
        None,
        None,
    );
    f.node_mut(a).unwrap().set_parent(Some(b));
    f.node_mut(b).unwrap().set_parent(Some(c));
    emit!(
        "ancestor/self",
        u::find_ancestor(f.view(), Some(a), |n| n.kind() == K::Identifier).unwrap() == Some(a)
    );
    emit!(
        "ancestor/class",
        u::get_containing_class(f.view(), a).unwrap() == Some(b)
    );
    emit!(
        "ancestor/class-self",
        u::get_containing_class(f.view(), b).unwrap().is_none()
    );
    emit!(
        "ancestor/source",
        u::get_source_file_of_node(f.view(), Some(a)).unwrap() == Some(c)
    );
    emit!(
        "ancestor/kind",
        u::find_ancestor_kind(f.view(), Some(a), K::ClassExpression.into()).unwrap() == Some(b)
    );
    emit!(
        "ancestor/nil",
        u::find_ancestor(f.view(), None, |_| panic!("callback called"))
            .unwrap()
            .is_none()
    );
    let mut yes1 = |_: &ts_ast::NodeRead<'_>| true;
    let mut yes2 = |_: &ts_ast::NodeRead<'_>| true;
    let mut yes3 = |_: &ts_ast::NodeRead<'_>| true;
    let many =
        u::find_many_ancestors(f.view(), Some(a), &mut [&mut yes1, &mut yes2, &mut yes3]).unwrap();
    emit!("ancestor/many-order", many == [Some(a), Some(b), Some(c)]);
    emit!(
        "ancestor/many-empty",
        u::find_many_ancestors(f.view(), Some(a), &mut [])
            .unwrap()
            .is_empty()
    );
    for value in [-1, 0, 1, 2, 3] {
        let callback = |n: &ts_ast::NodeRead<'_>| {
            if n.kind() == K::Identifier {
                u::FindAncestorResult(value)
            } else {
                u::FindAncestorResult::TRUE
            }
        };
        emit!(
            format!("ancestor/result/{value}"),
            u::find_ancestor_or_quit(f.view(), Some(a), callback).unwrap() == Some(a)
        );
        emit!(
            format!("ancestor/result-parent/{value}"),
            u::find_ancestor_or_quit(f.view(), Some(a), callback).unwrap() == Some(b)
        );
    }
    emit!(
        "ancestor/from-bool/false",
        u::to_find_ancestor_result(false).0
    );
    emit!(
        "ancestor/from-bool/true",
        u::to_find_ancestor_result(true).0
    );
    for (i, (left, right)) in [
        (Some(a), Some(a)),
        (Some(a), Some(b)),
        (Some(a), Some(c)),
        (Some(b), Some(a)),
        (Some(a), None),
        (None, Some(a)),
    ]
    .into_iter()
    .enumerate()
    {
        emit!(
            format!("descendant/{i}"),
            u::is_node_descendant_of(f.view(), left, right).unwrap()
        );
    }
    let paren = f.new_token(K::ParenthesizedExpression.into());
    let paren2 = f.new_token(K::ParenthesizedExpression.into());
    f.node_mut(paren).unwrap().set_parent(Some(paren2));
    f.node_mut(paren2).unwrap().set_parent(Some(a));
    emit!(
        "walk/expression",
        u::walk_up_parenthesized_expressions(f.view(), Some(paren)).unwrap() == Some(a)
    );
    emit!(
        "walk/expression-other",
        u::walk_up_parenthesized_expressions(f.view(), Some(a)).unwrap() == Some(a)
    );
    emit!(
        "walk/expression-nil",
        u::walk_up_parenthesized_expressions(f.view(), None)
            .unwrap()
            .is_none()
    );
    let typ = f.new_token(K::ParenthesizedType.into());
    f.node_mut(typ).unwrap().set_parent(Some(a));
    emit!(
        "walk/type",
        u::walk_up_parenthesized_types(f.view(), Some(typ)).unwrap() == Some(a)
    );
    emit!(
        "walk/type-nil",
        u::walk_up_parenthesized_types(f.view(), None)
            .unwrap()
            .is_none()
    );
    let hidden = f.new_token(K::Identifier.into());
    f.node_mut(hidden)
        .unwrap()
        .set_flags(ts_ast::node_flags::REPARSED);
    emit!(
        "visible/empty",
        u::find_last_visible_node(f.view(), &[]).unwrap().is_none()
    );
    emit!(
        "visible/skip",
        u::find_last_visible_node(f.view(), &[Some(a), Some(hidden), Some(hidden)]).unwrap()
            == Some(a)
    );
    emit!(
        "visible/all-hidden",
        u::find_last_visible_node(f.view(), &[Some(hidden)])
            .unwrap()
            .is_none()
    );
    for (i, kind) in [
        K::FunctionDeclaration,
        K::TryStatement,
        K::CatchClause,
        K::Identifier,
    ]
    .into_iter()
    .enumerate()
    {
        let block = f.new_token(K::Block.into());
        let parent = f.new_token(kind.into());
        f.node_mut(block).unwrap().set_parent(Some(parent));
        emit!(
            format!("block/function/{i}"),
            u::is_function_block(f.view(), Some(block)).unwrap()
        );
        emit!(
            format!("block/statement/{i}"),
            u::is_statement(f.view(), block).unwrap()
        );
        emit!(
            format!("block/module/{i}"),
            u::is_function_or_module_block(f.view(), block).unwrap()
        );
    }
    for flag in 0..8 {
        let variable = f.new_token(K::VariableDeclaration.into());
        let decls = f.new_token(K::VariableDeclarationList.into());
        let stmt = f.new_token(K::VariableStatement.into());
        f.node_mut(variable).unwrap().set_parent(Some(decls));
        f.node_mut(decls).unwrap().set_parent(Some(stmt));
        f.node_mut(variable)
            .unwrap()
            .set_flags(ts_ast::node_flags::JAVA_SCRIPT_FILE);
        f.node_mut(decls).unwrap().set_flags(flag);
        f.node_mut(stmt)
            .unwrap()
            .set_flags(ts_ast::node_flags::AMBIENT);
        let binding = f.new_token(K::BindingElement.into());
        let pattern = f.new_token(K::ObjectBindingPattern.into());
        f.node_mut(binding).unwrap().set_parent(Some(pattern));
        f.node_mut(pattern).unwrap().set_parent(Some(variable));
        emit!(
            format!("combined/flags/{flag}"),
            u::get_combined_node_flags(f.view(), binding).unwrap()
        );
        emit!(
            format!("combined/root/{flag}"),
            u::get_root_declaration(f.view(), binding).unwrap() == variable
        );
        emit!(
            format!("combined/walk/{flag}"),
            u::walk_up_binding_elements_and_patterns(f.view(), binding).unwrap() == Some(variable)
        );
        emit!(
            format!("combined/await/{flag}"),
            u::is_var_await_using(f.view(), binding).unwrap()
        );
        emit!(
            format!("combined/using/{flag}"),
            u::is_var_using(f.view(), binding).unwrap()
        );
        emit!(
            format!("combined/const/{flag}"),
            u::is_var_const(f.view(), binding).unwrap()
        );
        emit!(
            format!("combined/let/{flag}"),
            u::is_var_let(f.view(), binding).unwrap()
        );
        emit!(
            format!("combined/const-like/{flag}"),
            u::is_var_const_like(f.view(), binding).unwrap()
        );
    }
    let parameter = f.new_token(K::Parameter.into());
    let pattern = f.new_token(K::ArrayBindingPattern.into());
    let binding = f.new_token(K::BindingElement.into());
    f.node_mut(binding).unwrap().set_parent(Some(pattern));
    f.node_mut(pattern).unwrap().set_parent(Some(parameter));
    emit!(
        "binding/parameter",
        u::is_part_of_parameter_declaration(f.view(), binding).unwrap()
    );
    let variable = f.new_token(K::VariableDeclaration.into());
    let catch = f.new_token(K::CatchClause.into());
    f.node_mut(variable).unwrap().set_parent(Some(catch));
    emit!(
        "binding/catch",
        u::is_catch_clause_variable_declaration_or_binding_element(f.view(), variable).unwrap()
    );
    emit!(
        "binding/scoped",
        u::is_block_or_catch_scoped(f.view(), variable).unwrap()
    );
    fn panic_matches(action: impl FnOnce(), expected: &str) -> bool {
        let value = std::panic::catch_unwind(std::panic::AssertUnwindSafe(action))
            .expect_err("expected contract panic");
        let text = value
            .downcast_ref::<String>()
            .map(String::as_str)
            .or_else(|| value.downcast_ref::<&str>().copied())
            .expect("string contract panic");
        assert_eq!(text, expected, "wrong contract panic");
        true
    }
    emit!(
        "panic/visible-nil",
        panic_matches(
            || {
                u::find_last_visible_node(f.view(), &[None]).unwrap();
            },
            "nil node in source AST utility"
        )
    );
    let bad_binding = f.new_token(K::BindingElement.into());
    emit!(
        "panic/binding-parent",
        panic_matches(
            || {
                u::get_root_declaration(f.view(), bad_binding).unwrap();
            },
            "nil node in source AST utility"
        )
    );
    let bad_source = f.new_token(K::SourceFile.into());
    emit!(
        "panic/source-payload",
        panic_matches(
            || {
                u::get_source_file_of_node(f.view(), Some(bad_source)).unwrap();
            },
            "SourceFile payload"
        )
    );
    let bad_module = f.new_token(K::ModuleDeclaration.into());
    emit!(
        "panic/module-payload",
        panic_matches(
            || {
                u::is_global_scope_augmentation(&f.view().node(bad_module).unwrap());
            },
            "ModuleDeclaration payload"
        )
    );
    let name = f.new_identifier(ts_ast::JsString::from_bytes(b"x".as_slice()));
    let question = f.new_token(K::QuestionDotToken.into());
    for (i, flag) in [0, ts_ast::node_flags::OPTIONAL_CHAIN, u32::MAX]
        .into_iter()
        .enumerate()
    {
        for (j, q) in [None, Some(question)].into_iter().enumerate() {
            let id = f.new_property_access_expression(Some(a), q, Some(name), 0);
            f.node_mut(id).unwrap().set_flags(flag);
            f.node_mut(id).unwrap().set_parent(Some(b));
            emit!(
                format!("optional/chain/{i}/{j}"),
                u::is_optional_chain(&f.view().node(id).unwrap())
            );
            emit!(
                format!("optional/root/{i}/{j}"),
                u::is_optional_chain_root(&f.view().node(id).unwrap())
            );
            emit!(
                format!("optional/outer/{i}/{j}"),
                u::is_outermost_optional_chain(f.view(), id).unwrap()
            );
            f.node_mut(a).unwrap().set_parent(Some(id));
            emit!(
                format!("optional/expression/{i}/{j}"),
                u::is_expression_of_optional_chain_root(f.view(), a).unwrap()
            );
            f.node_mut(a).unwrap().set_parent(Some(b));
        }
    }
    for (i, kind) in [
        K::BarBarToken,
        K::AmpersandAmpersandToken,
        K::QuestionQuestionToken,
        K::CommaToken,
        K::InstanceOfKeyword,
        K::EqualsToken,
        K::PlusEqualsToken,
        K::BarBarEqualsToken,
        K::AmpersandAmpersandEqualsToken,
        K::QuestionQuestionEqualsToken,
    ]
    .into_iter()
    .enumerate()
    {
        let operator = f.new_token(kind.into());
        let id = f.new_binary_expression(None, Some(a), None, Some(operator), Some(name));
        emit!(
            format!("binary/logical/{i}"),
            u::is_logical_or_coalescing_binary_expression(f.view(), id).unwrap()
        );
        emit!(
            format!("binary/assignment/{i}"),
            u::is_logical_or_coalescing_assignment_expression(f.view(), id).unwrap()
        );
        emit!(
            format!("binary/coalesce/{i}"),
            u::is_nullish_coalesce(f.view(), id).unwrap()
        );
        emit!(
            format!("binary/comma/{i}"),
            u::is_comma_expression(f.view(), id).unwrap()
        );
        emit!(
            format!("binary/sequence/{i}"),
            u::is_comma_sequence(f.view(), id).unwrap()
        );
        emit!(
            format!("binary/instanceof/{i}"),
            u::is_instance_of_expression(f.view(), id).unwrap()
        );
        let negated = f.new_prefix_unary_expression(K::ExclamationToken.into(), Some(id));
        let wrapper = f.new_parenthesized_expression(Some(negated));
        emit!(
            format!("binary/wrapped/{i}"),
            u::is_logical_expression(f.view(), wrapper).unwrap()
        );
    }
    for (i, operator) in [K::PlusToken, K::MinusToken, K::ExclamationToken]
        .into_iter()
        .enumerate()
    {
        let num = f.new_numeric_literal(ts_ast::JsString::from_bytes(b"1".as_slice()), 0);
        let big = f.new_big_int_literal(ts_ast::JsString::from_bytes(b"1n".as_slice()), 0);
        for (j, operand) in [Some(num), Some(big), None].into_iter().enumerate() {
            if j == 2 && i != 2 {
                continue;
            }
            let id = f.new_prefix_unary_expression(operator.into(), operand);
            emit!(
                format!("signed/{i}/{j}"),
                u::is_signed_numeric_literal(f.view(), id).unwrap()
            );
        }
    }
    for (i, flag) in [
        0,
        ts_ast::modifier_flags::STATIC,
        ts_ast::modifier_flags::ACCESSOR,
        ts_ast::modifier_flags::PRIVATE,
        ts_ast::modifier_flags::CONST,
        u32::MAX,
    ]
    .into_iter()
    .enumerate()
    {
        let mods = f
            .new_list(TextRange::new(-1, -1), ts_ast::NodeSlice::empty())
            .unwrap();
        f.list_mut(mods).unwrap().set_modifier_flags(flag);
        let property = f.new_property_declaration(Some(mods), Some(name), None, None, None);
        emit!(
            format!("modifier/static/{i}"),
            u::is_static(f.view(), property).unwrap()
        );
        emit!(
            format!("modifier/accessor/{i}"),
            u::is_auto_accessor_property_declaration(f.view(), property).unwrap()
        );
        emit!(
            format!("modifier/has/{i}"),
            u::has_syntactic_modifier(
                f.view(),
                property,
                ts_ast::modifier_flags::STATIC | ts_ast::modifier_flags::ACCESSOR
            )
            .unwrap()
        );
        let parameter = f.new_parameter_declaration(Some(mods), None, Some(name), None, None, None);
        let constructor = f.new_token(K::Constructor.into());
        emit!(
            format!("modifier/parameter/{i}"),
            u::is_parameter_property_declaration(f.view(), parameter, constructor).unwrap()
        );
        let stmt = f.new_variable_statement(Some(mods), None);
        let decl = f.new_variable_declaration(Some(name), None, None, None);
        f.node_mut(decl).unwrap().set_parent(Some(stmt));
        emit!(
            format!("modifier/combined/{i}"),
            u::get_combined_modifier_flags(f.view(), decl).unwrap()
        );
        emit!(
            format!("modifier/const/{i}"),
            u::is_enum_const(f.view(), stmt).unwrap()
        );
    }
    let private = f.new_private_identifier(ts_ast::JsString::from_bytes(b"#x".as_slice()));
    for (i, name) in [name, private].into_iter().enumerate() {
        let id = f.new_property_declaration(None, Some(name), None, None, None);
        emit!(
            format!("private/{i}"),
            u::is_private_identifier_class_element_declaration(f.view(), id).unwrap()
        );
    }
    let string = f.new_string_literal(ts_ast::JsString::from_bytes(b"use strict".as_slice()), 0);
    let template = f.new_no_substitution_template_literal(
        ts_ast::JsString::from_bytes(b"use strict".as_slice()),
        0,
    );
    for (i, expression) in [string, template, a].into_iter().enumerate() {
        let id = f.new_expression_statement(Some(expression));
        emit!(
            format!("prologue/{i}"),
            u::is_prologue_directive(f.view(), id).unwrap()
        );
    }
    let expected: BTreeMap<_, _> =
        include_str!("../../../data/s06/utilities-front-observations.tsv")
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
