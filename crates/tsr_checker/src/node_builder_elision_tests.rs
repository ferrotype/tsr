//! Access-only cases exercise the pinned node-builder branches at their limits.
use super::NodeBuilder;
use crate::{CheckerOptions, CheckerState};
use serde_json::{json, Value};
use tsr_arena::{CheckerIdentity, Counters, Generation};
use tsr_ast::FactoryMethods;
use tsr_printer::{EmitTextWriter, Printer, PrinterOptions, TextWriter};

#[test]
fn elision_branches_match_native_text_and_length() {
    let requests: Value = serde_json::from_str(include_str!(
        "../../tsr_compiler/tests/fixtures/c2/elision/branches.requests.json"
    ))
    .unwrap();
    let expected: Value = serde_json::from_str(include_str!(
        "../../tsr_compiler/tests/fixtures/c2/elision/branches.observations.json"
    ))
    .unwrap();
    let counters = Counters::new();
    let identity = CheckerIdentity::new(Generation::new(&counters), &counters);
    let mut checker = CheckerState::new(&identity, &counters, CheckerOptions::default()).unwrap();
    let mut observed = Vec::new();
    for case in requests["cases"].as_array().unwrap() {
        let flags = if case["no_truncation"].as_bool().unwrap() {
            tsr_nodebuilder::flags::NO_TRUNCATION
        } else {
            0
        };
        let length = case["length"].as_u64().unwrap() as usize;
        let number = checker.builtins.number_type;
        let mut builder = NodeBuilder::new(&mut checker, flags);
        builder.approximate_length = length;
        let mut comment_nodes = Vec::new();
        let node = match case["operation"].as_str().unwrap() {
            "placeholder" => builder.elided_type(),
            "conditional" => builder.conditional_type_node(number).unwrap(),
            "list" => {
                let nodes = builder
                    .type_nodes(
                        &vec![number; case["count"].as_u64().unwrap() as usize],
                        case["bare"].as_bool().unwrap(),
                    )
                    .unwrap();
                comment_nodes = nodes.clone();
                let list = builder.list(nodes).unwrap();
                builder.ast.new_tuple_type_node(Some(list))
            }
            _ => panic!("unknown operation"),
        };
        if comment_nodes.is_empty() {
            comment_nodes.push(node);
        }
        let comments = comment_nodes.iter().enumerate().flat_map(|(index, &node)| {
            builder.emit.synthetic_leading_comments(node).into_iter().map(move |comment| {
                json!({"index":index,"text":String::from_utf8(comment.text.as_bytes().to_vec()).unwrap(),"kind":comment.kind as u16,"trailing_newline":comment.has_trailing_new_line})
            })
        }).collect::<Vec<_>>();
        let mut writer = TextWriter::new(b"", 0);
        Printer::new(PrinterOptions::default(), &builder.emit)
            .write(builder.ast.view(), node, None, &mut writer)
            .unwrap();
        observed.push(json!({"id":case["id"],"text":String::from_utf8(writer.text().to_vec()).unwrap(),"added_length":builder.approximate_length-length,"comments":comments}));
    }
    assert_eq!(json!(observed), expected["cases"]);
}

#[test]
fn property_elision_matches_native_members_comments_and_length() {
    use tsr_ast::{symbol_flags, JsString, SyntaxKind as K};
    let request: Value = serde_json::from_str(include_str!(
        "../../tsr_compiler/tests/fixtures/c2/elision/property.requests.json"
    ))
    .unwrap();
    let expected: Value = serde_json::from_str(include_str!(
        "../../tsr_compiler/tests/fixtures/c2/elision/property.observations.json"
    ))
    .unwrap();
    let counters = Counters::new();
    let identity = CheckerIdentity::new(Generation::new(&counters), &counters);
    let mut checker = CheckerState::new(&identity, &counters, CheckerOptions::default()).unwrap();
    let mut observed = Vec::new();
    for q in request["cases"].as_array().unwrap() {
        // These resolved shapes correspond to the source declarations consumed
        // by the native observer; only the builder's entry length is seeded.
        let number = checker.builtins.number_type;
        let mut properties = Vec::new();
        for i in 0..q["properties"].as_u64().unwrap() {
            let symbol = checker
                .new_symbol(
                    symbol_flags::PROPERTY,
                    JsString::from_bytes(vec![b'a' + i as u8]),
                )
                .unwrap();
            checker
                .value_symbol_links
                .get_or_default(symbol)
                .resolved_type = Some(number);
            properties.push(symbol);
        }
        let mut calls = Vec::new();
        let mut constructors = Vec::new();
        for (key, list) in [("calls", &mut calls), ("constructors", &mut constructors)] {
            for _ in 0..q[key].as_u64().unwrap() {
                list.push(
                    checker
                        .signatures
                        .new_signature(0, None, None, None, None, Some(number), None, 0)
                        .unwrap(),
                );
            }
        }
        let mut indexes = Vec::new();
        for _ in 0..q["indexes"].as_u64().unwrap() {
            indexes.push(
                checker
                    .signatures
                    .new_index_info(
                        checker.builtins.string_type,
                        number,
                        q["readonly"] == true,
                        None,
                        None,
                    )
                    .unwrap(),
            );
        }
        let ty = checker
            .new_anonymous_type(None, None, &calls, &constructors, &indexes)
            .unwrap();
        checker.types.structured_mut(ty).unwrap().properties = Some(properties.into());
        let flags = if q["no_truncation"].as_bool().unwrap() {
            tsr_nodebuilder::flags::NO_TRUNCATION
        } else {
            0
        };
        let length = q["length"].as_u64().unwrap() as usize;
        let mut builder = NodeBuilder::new(&mut checker, flags);
        builder.approximate_length = length;
        let node = builder.object_type_members_node(ty).unwrap();
        let mut members = Vec::new();
        let read = builder.ast.view().node(node).unwrap();
        let kind = read.kind().raw();
        if read.kind() == K::TypeLiteral {
            let list = read.member_list().unwrap();
            let view = builder.ast.view();
            for member in view
                .node_slice(view.list(list).unwrap().nodes())
                .unwrap()
                .iter()
            {
                let member = member.unwrap();
                let comments = builder.emit.synthetic_trailing_comments(member).into_iter().map(|c| json!({"kind":c.kind as u16,"text":String::from_utf8(c.text.as_bytes().to_vec()).unwrap(),"pos":c.loc.pos(),"end":c.loc.end(),"leading_newline":c.has_leading_new_line,"trailing_newline":c.has_trailing_new_line})).collect::<Vec<_>>();
                members.push(json!({"kind":builder.ast.view().node(member).unwrap().kind().raw(),"comments":comments}));
            }
        }
        let mut writer = TextWriter::new(b"", 0);
        Printer::new(PrinterOptions::default(), &builder.emit)
            .write(builder.ast.view(), node, None, &mut writer)
            .unwrap();
        observed.push(json!({"id":q["id"],"observation":{"text":String::from_utf8(writer.text().to_vec()).unwrap(),"kind":kind,"members":members,"added_length":builder.approximate_length-length,"restored_flags":builder.flags==flags}}));
    }
    assert_eq!(json!(observed), expected["cases"]);
}
