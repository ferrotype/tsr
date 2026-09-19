use super::*;
use crate::{AstBuilder, Factory, FactoryMethods, JsString, NodeData, NodeListId};
use std::collections::BTreeMap;
use ts_arena::Counters;
use ts_core::TextRange;
use ts_jsstring::SourceText;

fn id(f: &mut AstBuilder, text: &[u8]) -> NodeId {
    f.new_identifier(JsString::from_bytes(text))
}
fn list(f: &mut AstBuilder, nodes: Vec<Option<NodeId>>) -> NodeListId {
    let nodes = f.node_slice(nodes).unwrap();
    f.new_list(TextRange::new(-1, -1), nodes).unwrap()
}
fn prop(f: &mut AstBuilder, base: NodeId, name: &[u8]) -> NodeId {
    let name = id(f, name);
    f.new_property_access_expression(Some(base), None, Some(name), 0)
}
fn module(f: &mut AstBuilder, body: Option<NodeId>) -> NodeId {
    let name = id(f, b"M");
    f.new_module_declaration(None, K::NamespaceKeyword.into(), Some(name), None, body)
}
fn block(f: &mut AstBuilder, nodes: Vec<Option<NodeId>>) -> NodeId {
    let list = list(f, nodes);
    f.new_module_block(Some(list))
}
fn interface(f: &mut AstBuilder) -> NodeId {
    let name = id(f, b"X");
    f.new_interface_declaration(None, Some(name), None, None, None)
}
fn enumeration(f: &mut AstBuilder) -> NodeId {
    let token = f.new_token(K::ConstKeyword.into());
    let nodes = f.node_slice(vec![Some(token)]).unwrap();
    let modifiers = crate::RuntimeFactory::new_modifier_list(f, nodes);
    let name = id(f, b"X");
    f.new_enum_declaration(Some(modifiers), Some(name), None)
}
fn value(f: &mut AstBuilder) -> NodeId {
    let name = id(f, b"X");
    let decl = f.new_variable_declaration(Some(name), None, None, None);
    let nodes = list(f, vec![Some(decl)]);
    let declarations = f.new_variable_declaration_list(Some(nodes), 0);
    f.new_variable_statement(None, Some(declarations))
}
fn alias(f: &mut AstBuilder) -> NodeId {
    let name = id(f, b"X");
    let specifier = f.new_export_specifier(false, None, Some(name));
    let nodes = list(f, vec![Some(specifier)]);
    let clause = f.new_named_exports(Some(nodes));
    f.new_export_declaration(None, false, Some(clause), None, None)
}
fn panics(callback: impl FnOnce()) -> i64 {
    let Err(payload) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(callback)) else {
        return 0;
    };
    let message = payload
        .downcast_ref::<String>()
        .map(String::as_str)
        .or_else(|| payload.downcast_ref::<&str>().copied())
        .expect("helper panic has a string payload");
    match message {
        "nil node in source AST utility" => 1,
        "ModuleBlock kind requires ModuleBlock payload" => 2,
        _ => panic!("unclassified AST helper panic: {message}"),
    }
}

#[test]
fn binding_syntax_branches_match_frozen_source_observations() {
    let mut expected: BTreeMap<_, _> =
        include_str!("../../../data/s07/ast-helper-observations.tsv")
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
    let mut emit = |label: &str, values: &[i64]| {
        assert_eq!(values, expected.remove(label).unwrap(), "{label}");
    };
    let b = i64::from;
    let mut f = AstBuilder::new(SourceText::default(), &Counters::new());
    for index in 0..10 {
        let base = match index {
            0 | 1 | 3 => id(&mut f, b"module"),
            2 | 8 => id(&mut f, b"exports"),
            4 | 6 => f.new_token(K::ThisKeyword.into()),
            9 => id(&mut f, b"module\xff"),
            _ => id(&mut f, b"obj"),
        };
        let left = match index {
            0 | 1 | 9 => prop(&mut f, base, b"exports"),
            3 => {
                let base = prop(&mut f, base, b"exports");
                prop(&mut f, base, b"x")
            }
            7 => {
                let dynamic = id(&mut f, b"dynamic");
                f.new_element_access_expression(Some(base), None, Some(dynamic), 0)
            }
            _ => prop(&mut f, base, b"x"),
        };
        if !matches!(index, 5..=7) {
            f.node_mut(left)
                .unwrap()
                .set_flags(node_flags::JAVA_SCRIPT_FILE);
        }
        let right = id(&mut f, if index == 1 { b"exports" } else { b"value" });
        let operator = f.new_token(
            if index == 8 {
                K::PlusEqualsToken
            } else {
                K::EqualsToken
            }
            .into(),
        );
        let node = f.new_binary_expression(None, Some(left), None, Some(operator), Some(right));
        emit(
            &format!("assignment/{index}"),
            &[
                get_assignment_declaration_kind(f.view(), node).unwrap() as i64,
                b(is_assignment_expression(f.view(), node, false).unwrap()),
                b(is_assignment_expression(f.view(), node, true).unwrap()),
            ],
        );
    }
    for index in 0..5 {
        let target = match index {
            0 => id(&mut f, b"exports"),
            1 => {
                let base = id(&mut f, b"module");
                prop(&mut f, base, b"exports")
            }
            2 => {
                let base = id(&mut f, b"obj");
                prop(&mut f, base, b"member")
            }
            3 => f.new_token(K::ThisKeyword.into()),
            _ => f.new_numeric_literal(JsString::from_bytes(b"1".as_slice()), 0),
        };
        let base = id(&mut f, b"Object");
        let expression = prop(&mut f, base, b"defineProperty");
        let name = f.new_string_literal(JsString::from_bytes(b"x".as_slice()), 0);
        let descriptor = id(&mut f, b"descriptor");
        let arguments = list(&mut f, vec![Some(target), Some(name), Some(descriptor)]);
        let call = f.new_call_expression(Some(expression), None, None, Some(arguments), 0);
        f.node_mut(call)
            .unwrap()
            .set_flags(node_flags::JAVA_SCRIPT_FILE);
        emit(
            &format!("define/{index}"),
            &[
                b(is_bindable_object_define_property_call(f.view(), call).unwrap()),
                get_assignment_declaration_kind(f.view(), call).unwrap() as i64,
                b(get_non_assigned_name_of_declaration(f.view(), call)
                    .unwrap()
                    .is_some()),
            ],
        );
        f.node_mut(call).unwrap().set_flags(0);
        emit(
            &format!("define-ts/{index}"),
            &[get_assignment_declaration_kind(f.view(), call).unwrap() as i64],
        );
    }
    let call = f.new_call_expression(None, None, None, None, 0);
    emit(
        "define/no-args",
        &[b(
            is_bindable_object_define_property_call(f.view(), call).unwrap()
        )],
    );
    let this = f.new_token(K::ThisKeyword.into());
    let argument = f.new_string_literal(JsString::from_bytes(b"x".as_slice()), 0);
    let element = f.new_element_access_expression(Some(this), None, Some(argument), 0);
    let wrapped = f.new_parenthesized_expression(Some(element));
    let invalid = id(&mut f, b"push\xff");
    emit(
        "names",
        &[
            b(is_entity_name_expression_ex(f.view(), element, false).unwrap()),
            b(is_entity_name_expression_ex(f.view(), element, true).unwrap()),
            b(is_entity_name_expression_ex(f.view(), wrapped, true).unwrap()),
            b(is_dotted_name(f.view(), wrapped).unwrap()),
            b(is_bindable_static_name_expression(f.view(), element, false).unwrap()),
            b(is_bindable_static_name_expression(f.view(), element, true).unwrap()),
            b(is_push_or_unshift_identifier(f.view(), invalid).unwrap()),
        ],
    );
    for index in 0..10 {
        let body = match index {
            0 => None,
            1 => Some(block(&mut f, vec![])),
            2 => {
                let child = interface(&mut f);
                Some(block(&mut f, vec![Some(child)]))
            }
            3 => {
                let child = enumeration(&mut f);
                Some(block(&mut f, vec![Some(child)]))
            }
            4 => {
                let child = value(&mut f);
                Some(block(&mut f, vec![Some(child), None]))
            }
            _ => {
                let export = alias(&mut f);
                let child = match index {
                    5 => Some(interface(&mut f)),
                    6 => Some(enumeration(&mut f)),
                    7 => Some(value(&mut f)),
                    9 => {
                        let name = id(&mut f, b"X");
                        let reference = id(&mut f, b"Y");
                        Some(f.new_import_equals_declaration(
                            None,
                            false,
                            Some(name),
                            Some(reference),
                        ))
                    }
                    _ => None,
                };
                let mut nodes = vec![Some(export)];
                if let Some(child) = child {
                    nodes.push(Some(child));
                }
                Some(block(&mut f, nodes))
            }
        };
        let m = module(&mut f, body);
        emit(
            &format!("module/{index}"),
            &[
                get_module_instance_state(f.view(), m).unwrap() as i64,
                b(is_instantiated_module(f.view(), m, false).unwrap()),
                b(is_instantiated_module(f.view(), m, true).unwrap()),
            ],
        );
    }
    let cycle = module(&mut f, None);
    if let NodeData::ModuleDeclaration(data) = f.node_mut(cycle).unwrap().data_mut() {
        data.body = Some(cycle);
    }
    emit(
        "module/cycle",
        &[get_module_instance_state(f.view(), cycle).unwrap() as i64],
    );
    let nil = block(&mut f, vec![None]);
    let nil_module = module(&mut f, Some(nil));
    emit(
        "module/nil-child",
        &[panics(|| {
            get_module_instance_state(f.view(), nil_module).unwrap();
        })],
    );
    let child = interface(&mut f);
    let malformed = f.new_node(
        K::ModuleBlock.into(),
        crate::ParenthesizedExpressionData {
            expression: Some(child),
        }
        .into(),
    );
    let malformed_module = module(&mut f, Some(malformed));
    emit(
        "module/payload-dispatch",
        &[panics(|| {
            get_module_instance_state(f.view(), malformed_module).unwrap();
        })],
    );
    let name = id(&mut f, b"x");
    let decl = f.new_variable_declaration(Some(name), None, None, None);
    let name = id(&mut f, b"x");
    let typ = f.new_token(K::AnyKeyword.into());
    let typed = f.new_variable_declaration(Some(name), None, Some(typ), None);
    let object = f.new_object_literal_expression(None, false);
    f.node_mut(object)
        .unwrap()
        .set_flags(node_flags::JAVA_SCRIPT_FILE);
    let class = f.new_class_expression(None, None, None, None, None);
    f.node_mut(class)
        .unwrap()
        .set_flags(node_flags::JAVA_SCRIPT_FILE);
    let function = f.new_function_expression(None, None, None, None, None, None, None, None);
    emit(
        "expando",
        &[
            b(is_expando_initializer(f.view(), decl, None).unwrap()),
            b(is_expando_initializer(f.view(), decl, Some(object)).unwrap()),
            b(is_expando_initializer(f.view(), typed, Some(object)).unwrap()),
            b(is_expando_initializer(f.view(), typed, Some(class)).unwrap()),
            b(is_expando_initializer(f.view(), typed, Some(function)).unwrap()),
        ],
    );
    f.node_mut(object).unwrap().set_flags(0);
    emit(
        "expando/ts-object",
        &[b(
            is_expando_initializer(f.view(), decl, Some(object)).unwrap()
        )],
    );
    let nodes = list(&mut f, vec![Some(decl)]);
    let declarations = f.new_variable_declaration_list(Some(nodes), 0);
    f.node_mut(decl).unwrap().set_parent(Some(declarations));
    let statement = f.new_variable_statement(None, Some(declarations));
    f.node_mut(declarations)
        .unwrap()
        .set_parent(Some(statement));
    let nodes = list(&mut f, vec![Some(statement)]);
    let container = f.new_block(Some(nodes), false);
    f.node_mut(statement).unwrap().set_parent(Some(container));
    emit(
        "container",
        &[b(
            get_declaration_container(f.view(), decl).unwrap() == Some(container)
        )],
    );
    let name = id(&mut f, b"x");
    let unparented = f.new_variable_declaration(Some(name), None, None, None);
    emit(
        "container/unparented",
        &[panics(|| {
            get_declaration_container(f.view(), unparented).unwrap();
        })],
    );
    let function = f.new_function_expression(None, None, None, None, None, None, None, None);
    let wrapper = f.new_parenthesized_expression(Some(function));
    f.node_mut(function).unwrap().set_parent(Some(wrapper));
    let invocation = f.new_call_expression(Some(wrapper), None, None, None, 0);
    f.node_mut(wrapper).unwrap().set_parent(Some(invocation));
    emit(
        "iife",
        &[b(get_immediately_invoked_function_expression(
            f.view(),
            function,
        )
        .unwrap()
            == Some(invocation))],
    );
    let name = id(&mut f, b"x");
    emit(
        "missing",
        &[
            b(node_is_missing(None::<&crate::Node>)),
            b(node_is_present(Some(&f.view().node(name).unwrap()))),
        ],
    );

    let type_parameter = f.new_token(K::TypeParameter.into());
    let token_declaration = f.new_token(K::VariableDeclaration.into());
    let binary_declaration = f.new_node(
        K::Unknown.into(),
        crate::BinaryExpressionData {
            modifiers: None,
            left: None,
            r#type: None,
            operator_token: None,
            right: None,
        }
        .into(),
    );
    emit(
        "declaration/payload",
        &[
            b(is_declaration(&f.view().node(type_parameter).unwrap())),
            b(is_declaration(&f.view().node(token_declaration).unwrap())),
            b(is_declaration(&f.view().node(binary_declaration).unwrap())),
        ],
    );
    f.node_mut(type_parameter)
        .unwrap()
        .set_parent(Some(token_declaration));
    emit(
        "declaration/type-parameter-parent",
        &[b(is_declaration(&f.view().node(type_parameter).unwrap()))],
    );
    let no_body = f.new_constructor_declaration(None, None, None, None, None, None);
    let missing_body = f.new_block(None, false);
    f.node_mut(missing_body)
        .unwrap()
        .set_range(TextRange::new(0, 0));
    let missing_constructor =
        f.new_constructor_declaration(None, None, None, None, None, Some(missing_body));
    let present_body = f.new_block(None, false);
    let present_constructor =
        f.new_constructor_declaration(None, None, None, None, None, Some(present_body));
    let members = list(
        &mut f,
        vec![
            Some(no_body),
            Some(missing_constructor),
            Some(present_constructor),
        ],
    );
    let class = f.new_class_declaration(None, None, None, None, Some(members));
    emit(
        "constructor",
        &[b(
            find_constructor_declaration(f.view(), class).unwrap() == Some(present_constructor)
        )],
    );
    let const_x = enumeration(&mut f);
    let name = id(&mut f, b"Y");
    let declaration = f.new_variable_declaration(Some(name), None, None, None);
    let declarations = list(&mut f, vec![Some(declaration)]);
    let declarations = f.new_variable_declaration_list(Some(declarations), 0);
    let value_y = f.new_variable_statement(None, Some(declarations));
    let outer = block(&mut f, vec![Some(const_x), Some(value_y)]);
    for (index, names) in [[b"X", b"Y"], [b"Y", b"X"]].into_iter().enumerate() {
        let mut specifiers = Vec::new();
        for name in names {
            let name = id(&mut f, name);
            specifiers.push(Some(f.new_export_specifier(false, None, Some(name))));
        }
        let specifiers = list(&mut f, specifiers);
        let clause = f.new_named_exports(Some(specifiers));
        let export = f.new_export_declaration(None, false, Some(clause), None, None);
        let m = module(&mut f, Some(export));
        f.node_mut(m).unwrap().set_parent(Some(outer));
        emit(
            &format!("module/alias-order/{index}"),
            &[get_module_instance_state(f.view(), m).unwrap() as i64],
        );
    }
    assert!(
        expected.is_empty(),
        "unconsumed independent Go rows: {expected:?}"
    );
}

#[test]
fn module_state_preserves_lazy_runtime_identity_side_effects() {
    let mut f = AstBuilder::new(SourceText::default(), &Counters::new());
    let child = interface(&mut f);
    let body = block(&mut f, vec![Some(child)]);
    let root = module(&mut f, Some(body));
    for id in [child, body, root] {
        assert_eq!(
            crate::existing_runtime_node_id(&f.view().node(id).unwrap()),
            0
        );
    }
    assert_eq!(
        get_module_instance_state(f.view(), root).unwrap(),
        ModuleInstanceState::NonInstantiated
    );
    assert_eq!(
        crate::existing_runtime_node_id(&f.view().node(root).unwrap()),
        0
    );
    let before =
        [child, body].map(|id| crate::existing_runtime_node_id(&f.view().node(id).unwrap()));
    assert!(before.iter().all(|&id| id != 0));
    get_module_instance_state(f.view(), root).unwrap();
    assert_eq!(
        before,
        [child, body].map(|id| crate::existing_runtime_node_id(&f.view().node(id).unwrap()))
    );
}
