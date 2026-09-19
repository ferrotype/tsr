//! Behaviour pinned by native observations; the corpus-wide comparison is
//! `scripts/s09_format.py compare --ops nav`.

use crate::{Error, Navigator};
use ts_arena::NodeId;
use ts_ast::{AstFile, SourceFileParseOptions, SyntaxKind as K};
use ts_jsstring::SourceText;

fn parse(name: &[u8], text: &[u8], kind: ts_core::ScriptKind) -> (AstFile, NodeId) {
    let file = ts_parser::parse_source_file(
        SourceText::from_loaded_bytes(text.to_vec()),
        kind,
        SourceFileParseOptions {
            file_name: ts_ast::JsString::from_bytes(name),
            path: ts_ast::JsString::from_bytes(name),
            ..Default::default()
        },
    )
    .publish_unbound();
    let root = file.root().unwrap();
    (file, root)
}

fn describe(file: &AstFile, id: Option<NodeId>) -> Option<(K, i32, i32)> {
    id.map(|id| {
        let node = file.view().node(id).unwrap();
        (node.kind().known().unwrap(), node.pos(), node.end())
    })
}

#[test]
fn tokens_the_tree_does_not_store_come_from_the_cache_and_are_the_same_node_twice() {
    let (file, root) = parse(b"/a.ts", b"f ( a , b ) ;", ts_core::ScriptKind::TS);
    let mut provider = ts_parser::ParserJsDocProvider::default();
    let mut navigator = Navigator::new(file.view(), root, &mut provider);
    let comma = navigator.get_token_at_position(6).unwrap();
    assert_eq!(describe(&file, Some(comma)), Some((K::CommaToken, 5, 7)));
    assert_ne!(
        comma.arena(),
        root.arena(),
        "a node of the file's lazy arena"
    );
    assert_eq!(navigator.get_token_at_position(6).unwrap(), comma);
    // A real node of the tree is returned as itself.
    let name = navigator.get_token_at_position(0).unwrap();
    assert_eq!(describe(&file, Some(name)), Some((K::Identifier, 0, 1)));
    assert_eq!(name.arena(), root.arena());
}

#[test]
fn preceding_and_next_tokens_cross_the_gaps_between_nodes() {
    let (file, root) = parse(b"/a.ts", b"f ( a , b ) ;", ts_core::ScriptKind::TS);
    let mut provider = ts_parser::ParserJsDocProvider::default();
    let mut navigator = Navigator::new(file.view(), root, &mut provider);
    assert_eq!(
        describe(&file, navigator.find_preceding_token(0).unwrap()),
        None
    );
    let open = navigator.find_preceding_token(4).unwrap().unwrap();
    assert_eq!(describe(&file, Some(open)), Some((K::OpenParenToken, 1, 3)));
    let parent = file.view().node(open).unwrap().parent().unwrap();
    let next = navigator.find_next_token(open, parent).unwrap();
    assert_eq!(describe(&file, next), Some((K::Identifier, 3, 5)));
    let close = navigator
        .find_child_of_kind(parent, K::CloseParenToken)
        .unwrap();
    assert_eq!(describe(&file, close), Some((K::CloseParenToken, 9, 11)));
    assert_eq!(
        navigator
            .find_child_of_kind(parent, K::OpenBraceToken)
            .unwrap(),
        None
    );
    let semicolon = navigator.find_preceding_token(13).unwrap();
    assert_eq!(
        describe(&file, semicolon),
        Some((K::SemicolonToken, 11, 13))
    );
}

#[test]
fn a_jsdoc_parameter_tag_is_searched_name_first_as_the_pinned_visitor_does() {
    // Upstream's `VisitEachChild` visits a parameter tag's name before its type
    // whatever order they were written in, and these searches use that visitor.
    // The rightmost valid child is therefore the type, and the token before a
    // position after the comment is the type's closing brace, not the name.
    let text = b"/** @param {<} x */\nfunction f(x) {}\n";
    let (file, root) = parse(b"/a.js", text, ts_core::ScriptKind::JS);
    let mut provider = ts_parser::ParserJsDocProvider::default();
    let mut navigator = Navigator::new(file.view(), root, &mut provider);
    for position in 16..=20 {
        assert_eq!(
            describe(&file, navigator.find_preceding_token(position).unwrap()),
            Some((K::CloseBraceToken, 13, 14)),
            "position {position}"
        );
    }
    // With JSDoc excluded there is nothing before the function.
    assert_eq!(
        navigator.find_preceding_token_ex(19, None, true).unwrap(),
        None
    );
}

#[test]
fn an_identifier_in_a_nodes_trivia_is_the_pinned_assertion() {
    // The pinned navigation panics here (taggedTemplatesWithTypeArguments2.ts).
    // The port reports the same message instead of inventing an answer.
    let text =
        b"class C<T> extends B {\n    constructor() {\n        super<number, string, T> `hello world`;\n    }\n}\n";
    let (file, root) = parse(b"/a.ts", text, ts_core::ScriptKind::TS);
    let mut provider = ts_parser::ParserJsDocProvider::default();
    let mut navigator = Navigator::new(file.view(), root, &mut provider);
    let failures: Vec<String> = (0..text.len() as i64)
        .filter_map(|position| match navigator.get_token_at_position(position) {
            Err(Error::Assertion(message)) => Some(message),
            _ => None,
        })
        .collect();
    assert!(!failures.is_empty(), "the pinned assertion is reachable");
    assert!(failures.iter().all(|message| message
        == "did not expect KindPropertyAccessExpression to have KindIdentifier in its trivia"));
}

#[test]
fn deep_preceding_and_rightmost_searches_fit_a_small_caller_stack() {
    const DEPTH: usize = 12_000;
    let parenthesized = format!("{}x{};", "(".repeat(DEPTH), ")".repeat(DEPTH));
    let unary = format!("{}x", "!".repeat(DEPTH));
    let (file, root) = parse(b"/a.ts", parenthesized.as_bytes(), ts_core::ScriptKind::TS);
    let (unary_file, unary_root) = parse(b"/b.ts", unary.as_bytes(), ts_core::ScriptKind::TS);
    std::thread::Builder::new()
        .stack_size(512 * 1024)
        .spawn(move || {
            let mut provider = ts_parser::ParserJsDocProvider::default();
            let mut nav = Navigator::new(file.view(), root, &mut provider);
            let expected = Some((K::Identifier, DEPTH as i32, DEPTH as i32 + 1));
            assert_eq!(
                describe(&file, nav.find_preceding_token(DEPTH as i64 + 1).unwrap()),
                expected
            );
            let mut nav = Navigator::new(unary_file.view(), unary_root, &mut provider);
            // At EOF this follows the rightmost valid child all the way down
            // the prefix chain, rather than returning a trailing punctuation.
            assert_eq!(
                describe(
                    &unary_file,
                    nav.find_preceding_token(DEPTH as i64 + 1).unwrap()
                ),
                expected
            );
        })
        .unwrap()
        .join()
        .unwrap();
}
