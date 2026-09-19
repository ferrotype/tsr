use super::*;
use std::collections::BTreeMap;
use ts_arena::Counters;
use ts_ast::{AstBuilder, FactoryMethods, NodeData, SourceFileParseOptions};
use ts_jsstring::SourceText;

fn source(f: &mut AstBuilder, text: &[u8]) -> NodeId {
    f.new_source_file(
        SourceFileParseOptions {
            file_name: JsString::from_bytes(b"/file.ts".as_slice()),
            ..SourceFileParseOptions::default()
        },
        SourceText::from_loaded_bytes(text),
        None,
        None,
    )
}
fn node(f: &mut AstBuilder, kind: K, pos: i64, end: i64) -> NodeId {
    let node = f.new_token(kind.into());
    f.node_mut(node)
        .unwrap()
        .set_range(TextRange::new(pos, end));
    node
}
fn range(f: &mut AstBuilder, node: NodeId, pos: i64, end: i64) {
    f.node_mut(node)
        .unwrap()
        .set_range(TextRange::new(pos, end));
}
fn text(value: &[u8]) -> Vec<i64> {
    value.iter().map(|&v| i64::from(v)).collect()
}
fn span(value: TextRange) -> [i64; 2] {
    [value.pos(), value.end()]
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
        "range start index 18446744073709551615 out of range for slice of length 1"
        | "index out of range: TryFromIntError(NegOverflow)" => 3,
        "Debug failure. Unexpected reparser-transformed node kind\nNode KindNumericLiteral was unexpected." => 4,
        _ => panic!("unclassified scanner helper panic: {message}"),
    }
}

#[test]
fn source_text_and_diagnostic_spans_match_pinned_go() {
    let mut expected: BTreeMap<_, _> =
        include_str!("../../../data/s07/scanner-helper-observations.tsv")
            .lines()
            .map(|line| {
                let mut fields = line.split('\t');
                (
                    fields.next().unwrap(),
                    fields
                        .map(|v| v.parse::<i64>().unwrap())
                        .collect::<Vec<_>>(),
                )
            })
            .collect();
    let mut emit = |label: &str, values: &[i64]| {
        assert_eq!(values, expected.remove(label).unwrap(), "{label}");
    };
    let mut f = AstBuilder::new(SourceText::default(), &Counters::new());
    let ordinary = b" /*c*/ name\xff";
    let file = source(&mut f, ordinary);
    let name = f.new_identifier(JsString::from_bytes(b"cooked".as_slice()));
    range(&mut f, name, 0, ordinary.len() as i64);
    f.node_mut(name).unwrap().set_parent(Some(file));
    emit(
        "text/trivia",
        &text(
            get_source_text_of_node_from_source_file(f.view(), file, Some(name), true)
                .unwrap()
                .as_bytes(),
        ),
    );
    emit(
        "text/trim",
        &text(
            get_source_text_of_node_from_source_file(f.view(), file, Some(name), false)
                .unwrap()
                .as_bytes(),
        ),
    );
    emit(
        "name/real",
        &text(
            declaration_name_to_string(f.view(), Some(name))
                .unwrap()
                .as_bytes(),
        ),
    );
    emit(
        "name/nil",
        &text(
            declaration_name_to_string(f.view(), None)
                .unwrap()
                .as_bytes(),
        ),
    );
    let synthetic = f.new_identifier(JsString::from_bytes(b"x".as_slice()));
    emit(
        "name/synthetic",
        &text(
            declaration_name_to_string(f.view(), Some(synthetic))
                .unwrap()
                .as_bytes(),
        ),
    );
    emit(
        "text/nil",
        &text(
            get_text_of_node_from_source_text(f.view(), ordinary, None, false)
                .unwrap()
                .as_bytes(),
        ),
    );
    let missing = node(&mut f, K::Identifier, 2, 2);
    emit(
        "text/missing",
        &text(
            get_text_of_node_from_source_text(f.view(), ordinary, Some(missing), false)
                .unwrap()
                .as_bytes(),
        ),
    );
    let jsdoc = b" * A\r\n * B\xe2\x80\xa8 * C";
    let doc = f.new_js_doc_type_expression(None);
    range(&mut f, doc, 0, jsdoc.len() as i64);
    emit(
        "text/jsdoc",
        &text(
            get_text_of_node_from_source_text(f.view(), jsdoc, Some(doc), true)
                .unwrap()
                .as_bytes(),
        ),
    );
    let typ = node(&mut f, K::NumberKeyword, 0, jsdoc.len() as i64);
    f.node_mut(typ).unwrap().set_flags(node_flags::REPARSED);
    emit(
        "text/reparsed-type",
        &text(
            get_text_of_node_from_source_text(f.view(), jsdoc, Some(typ), true)
                .unwrap()
                .as_bytes(),
        ),
    );
    for (index, flags) in [0, token_flags::SINGLE_QUOTE].into_iter().enumerate() {
        let literal = f.new_string_literal(JsString::from_bytes(b"cooked".as_slice()), flags);
        range(&mut f, literal, 0, 2);
        f.node_mut(literal)
            .unwrap()
            .set_flags(node_flags::REPARSER_TRANSFORMED_LITERAL);
        emit(
            &format!("text/transformed/{index}"),
            &text(
                get_text_of_node_from_source_text(f.view(), b"x\xff", Some(literal), false)
                    .unwrap()
                    .as_bytes(),
            ),
        );
    }
    let transformed = f.new_identifier(JsString::from_bytes(b"cooked\xff".as_slice()));
    range(&mut f, transformed, 0, 2);
    f.node_mut(transformed)
        .unwrap()
        .set_flags(node_flags::REPARSER_TRANSFORMED_LITERAL);
    emit(
        "text/transformed-id",
        &text(
            get_text_of_node_from_source_text(f.view(), b"xx", Some(transformed), false)
                .unwrap()
                .as_bytes(),
        ),
    );
    let bad = node(&mut f, K::NumericLiteral, 0, 1);
    f.node_mut(bad)
        .unwrap()
        .set_flags(node_flags::REPARSER_TRANSFORMED_LITERAL);
    emit(
        "text/transformed-panic",
        &[panics(|| {
            get_text_of_node_from_source_text(f.view(), b"1", Some(bad), false).unwrap();
        })],
    );
    let negative = node(&mut f, K::Identifier, -1, -1);
    emit(
        "text/synthetic-panic",
        &[panics(|| {
            get_text_of_node_from_source_text(f.view(), b"x", Some(negative), false).unwrap();
        })],
    );
    for (index, pos) in [0, 2, ordinary.len() as i64, ordinary.len() as i64 + 3]
        .into_iter()
        .enumerate()
    {
        emit(
            &format!("token/{index}"),
            &span(get_range_of_token_at_position(f.view(), file, pos).unwrap()),
        );
    }
    emit(
        "token/negative",
        &[panics(|| {
            get_range_of_token_at_position(f.view(), file, -1).unwrap();
        })],
    );
    emit(
        "error/source",
        &span(get_error_range_for_node(f.view(), file, file).unwrap()),
    );
    let empty = source(&mut f, b" /*c*/ \r\n");
    emit(
        "error/empty",
        &span(get_error_range_for_node(f.view(), empty, empty).unwrap()),
    );
    let jsx = node(&mut f, K::JsxText, 0, ordinary.len() as i64);
    emit(
        "error/jsx",
        &span(get_error_range_for_node(f.view(), file, jsx).unwrap()),
    );
    emit(
        "error/missing",
        &span(get_error_range_for_node(f.view(), file, missing).unwrap()),
    );
    let declaration = f.new_variable_declaration(Some(name), None, None, None);
    range(&mut f, declaration, 0, ordinary.len() as i64);
    emit(
        "error/declaration",
        &span(get_error_range_for_node(f.view(), file, declaration).unwrap()),
    );
    let class = f.new_class_expression(None, None, None, None, None);
    range(&mut f, class, 0, ordinary.len() as i64);
    emit(
        "error/nameless-class",
        &span(get_error_range_for_node(f.view(), file, class).unwrap()),
    );
    for (index, kind) in [K::ReturnStatement, K::YieldExpression]
        .into_iter()
        .enumerate()
    {
        let text = b" /*c*/ return answer";
        let source = source(&mut f, text);
        let node = node(&mut f, kind, 0, text.len() as i64);
        emit(
            &format!("error/keyword/{index}"),
            &span(get_error_range_for_node(f.view(), source, node).unwrap()),
        );
    }
    let arrow_text = b" /*c*/ () => {\r\n answer\n}";
    let af = source(&mut f, arrow_text);
    let body = f.new_block(None, false);
    range(&mut f, body, 12, arrow_text.len() as i64);
    let arrow = f.new_arrow_function(None, None, None, None, None, None, Some(body));
    range(&mut f, arrow, 0, arrow_text.len() as i64);
    emit(
        "error/arrow",
        &span(get_error_range_for_node(f.view(), af, arrow).unwrap()),
    );
    let ctor_text = b" public /*x*/ constructor() {}";
    let cf = source(&mut f, ctor_text);
    let ctor = f.new_constructor_declaration(None, None, None, None, None, None);
    range(&mut f, ctor, 0, ctor_text.len() as i64);
    emit(
        "error/constructor",
        &span(get_error_range_for_node(f.view(), cf, ctor).unwrap()),
    );
    f.node_mut(ctor).unwrap().set_flags(node_flags::REPARSED);
    emit(
        "error/constructor-reparsed",
        &span(get_error_range_for_node(f.view(), cf, ctor).unwrap()),
    );
    let satisfied = source(&mut f, b"first second x satisfies T");
    let expr = node(&mut f, K::Identifier, 13, 14);
    let target = node(&mut f, K::NumberKeyword, 99, 100);
    f.node_mut(target).unwrap().set_flags(node_flags::REPARSED);
    let sat = f.new_satisfies_expression(Some(expr), Some(target));
    let first_name = f.new_identifier(JsString::from_bytes(b"first".as_slice()));
    range(&mut f, first_name, 0, 5);
    let second_name = f.new_identifier(JsString::from_bytes(b"second".as_slice()));
    range(&mut f, second_name, 6, 12);
    let first_type = node(&mut f, K::NumberKeyword, 90, 91);
    let first_expr = f.new_js_doc_type_expression(Some(first_type));
    let second_expr = f.new_js_doc_type_expression(Some(target));
    let first = f.new_js_doc_satisfies_tag(Some(first_name), Some(first_expr), None);
    let second = f.new_js_doc_satisfies_tag(Some(second_name), Some(second_expr), None);
    let tags = f.node_slice(vec![Some(first), Some(second)]).unwrap();
    let tags = f.new_list(TextRange::new(-1, -1), tags).unwrap();
    let root = f.new_js_doc(None, Some(tags));
    let parent = node(&mut f, K::Unknown, 0, 0);
    f.node_mut(parent)
        .unwrap()
        .set_flags(node_flags::HAS_JS_DOC);
    f.node_mut(sat).unwrap().set_parent(Some(parent));
    f.seed_source_jsdoc(satisfied, parent, vec![root]).unwrap();
    emit(
        "error/satisfies-match",
        &span(get_error_range_for_node(f.view(), satisfied, sat).unwrap()),
    );
    range(&mut f, target, 80, 81);
    let replacement = f.new_js_doc_type_expression(Some(first_type));
    if let NodeData::JSDocSatisfiesTag(data) = f.node_mut(second).unwrap().data_mut() {
        data.type_expression = Some(replacement);
    }
    emit(
        "error/satisfies-first",
        &span(get_error_range_for_node(f.view(), satisfied, sat).unwrap()),
    );
    // A second logical file supplies the same text and an empty eager JSDoc cache.
    let no_eager = source(&mut f, b"first second x satisfies T");
    emit(
        "error/satisfies-no-eager",
        &span(get_error_range_for_node(f.view(), no_eager, sat).unwrap()),
    );
    f.node_mut(target).unwrap().set_flags(0);
    emit(
        "error/satisfies-native",
        &span(get_error_range_for_node(f.view(), no_eager, sat).unwrap()),
    );
    assert!(
        expected.is_empty(),
        "unconsumed source observations: {expected:?}"
    );
}
