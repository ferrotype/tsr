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
