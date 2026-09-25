//! Behaviour pinned by native observations; the corpus-wide comparison is
//! `scripts/s09_format.py compare --ops nav`.

use crate::{Error, Navigator};
use tsr_arena::NodeId;
use tsr_ast::{AstFile, SourceFileParseOptions, SyntaxKind as K};
use tsr_jsstring::SourceText;

#[test]
fn child_hooks_preserve_absent_slots_and_the_original_list_range() {
    use crate::HookVisit::{List, Node};

    let (file, root) = parse(b"/a.ts", b"function f(a, b,) {}", tsr_core::ScriptKind::TS);
    let view = file.view();
    let statements = view
        .node(root)
        .unwrap()
        .data_source()
        .as_source_file()
        .unwrap()
        .statements()
        .unwrap();
    let function = view
        .node_slice(view.list(statements).unwrap().nodes())
        .unwrap()
        .iter()
        .next()
        .unwrap()
        .unwrap();
    let function_read = view.node(function).unwrap();
    let data = function_read
        .data_source()
        .as_function_declaration()
        .unwrap();
    let (name, parameters, body) = (data.name(), data.parameters().unwrap(), data.body());
    drop(function_read);

    let mut provider = tsr_parser::ParserJsDocProvider::default();
    let mut nav = Navigator::new(view, root, &mut provider);
    // VisitEachChild calls the ordinary nil hooks, but the navigation wrapper
    // omits an absent modifiers list. These are the pinned hook arguments in
    // FunctionDeclaration.VisitEachChild, not its ForEachChild enumeration.
    assert_eq!(
        nav.visit_child_slots_and_jsdoc(function).unwrap(),
        [
            Node(None),
            Node(name),
            List(None),
            List(Some(parameters)),
            Node(None),
            Node(None),
            Node(body)
        ]
    );
    let list = view.list(parameters).unwrap();
    assert_eq!((list.loc().pos(), list.loc().end()), (11, 16));
    assert!(view.list_has_trailing_comma(parameters).unwrap());
    assert_eq!(
        nav.visit_child_slots_and_jsdoc(function).unwrap()[3],
        List(Some(parameters))
    );

    let (foreign, _) = parse(b"/b.ts", b"function f(a, b,) {}", tsr_core::ScriptKind::TS);
    assert!(
        foreign.view().list(parameters).is_err(),
        "the hook retains the list's owner identity"
    );
}

fn parse(name: &[u8], text: &[u8], kind: tsr_core::ScriptKind) -> (AstFile, NodeId) {
    let file = tsr_parser::parse_source_file(
        SourceText::from_loaded_bytes(text.to_vec()),
        kind,
        SourceFileParseOptions {
            file_name: tsr_ast::JsString::from_bytes(name),
            path: tsr_ast::JsString::from_bytes(name),
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
    let (file, root) = parse(b"/a.ts", b"f ( a , b ) ;", tsr_core::ScriptKind::TS);
    let mut provider = tsr_parser::ParserJsDocProvider::default();
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
    let (file, root) = parse(b"/a.ts", b"f ( a , b ) ;", tsr_core::ScriptKind::TS);
    let mut provider = tsr_parser::ParserJsDocProvider::default();
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
    let (file, root) = parse(b"/a.js", text, tsr_core::ScriptKind::JS);
    let mut provider = tsr_parser::ParserJsDocProvider::default();
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
    let (file, root) = parse(b"/a.ts", text, tsr_core::ScriptKind::TS);
    let mut provider = tsr_parser::ParserJsDocProvider::default();
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
    let (file, root) = parse(b"/a.ts", parenthesized.as_bytes(), tsr_core::ScriptKind::TS);
    let (unary_file, unary_root) = parse(b"/b.ts", unary.as_bytes(), tsr_core::ScriptKind::TS);
    std::thread::Builder::new()
        .stack_size(512 * 1024)
        .spawn(move || {
            let mut provider = tsr_parser::ParserJsDocProvider::default();
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

#[test]
fn a_shift_is_rescanned_by_both_navigation_scans_only_inside_a_jsx_child() {
    // The request of syntax/astnav/jsx-child-shift-gap-scan. Each `<` of `<<`
    // in the first three lines opens an error-recovery JsxSelfClosingElement
    // whose other children are zero-width, so the `<` is found by scanning,
    // and the scanner reads `<<`. The pinned answers (native capture of that
    // case) are the width-1 `<`: unrescanned, the gap scan returns the element
    // and the rightmost-token scan returns the width-2 `<<`.
    let text = b"const a = <div><<</div>;\nconst b = <>{1}<<</>;\nconst c = <a><<b/></a>;\n\
        type F = A<<T>() => T>;\nlet g = f<<T>(x: T) => T>(y);\n";
    let (file, root) = parse(b"/a.tsx", text, tsr_core::ScriptKind::TSX);
    let mut provider = tsr_parser::ParserJsDocProvider::default();
    let mut navigator = Navigator::new(file.view(), root, &mut provider);
    let parent_kind = |token: NodeId| {
        let parent = file.view().node(token).unwrap().parent().unwrap();
        file.view().node(parent).unwrap().kind().known().unwrap()
    };
    for start in [15, 16, 40, 41, 60] {
        let expected = Some((K::LessThanToken, start, start + 1));
        // getTokenAtPosition's scan between the children of the element.
        let token = navigator.get_token_at_position(i64::from(start)).unwrap();
        assert_eq!(describe(&file, Some(token)), expected, "token at {start}");
        assert_eq!(
            parent_kind(token),
            K::JsxSelfClosingElement,
            "the scanned token's containing node at {start}"
        );
        // findRightmostValidToken's scan after the element's last child.
        let preceding = navigator
            .find_preceding_token(i64::from(start) + 1)
            .unwrap();
        assert_eq!(
            describe(&file, preceding),
            expected,
            "preceding {}",
            start + 1
        );
    }
    // The last two lines scan a `<<` whose containing node is not a JSX child
    // (the `<` of type arguments before a generic function type), where the
    // pinned answers keep the unrescanned `<<`: the gap scan's `<<` overruns
    // the next child, so the containing node is returned, and the
    // rightmost-token scan returns the width-2 `<<`. A predicate that ignored
    // the containing node would answer a width-1 `<` at both.
    for (start, container, container_range) in [
        (81, K::TypeReference, (79, 93)),
        (104, K::CallExpression, (102, 123)),
    ] {
        let token = navigator.get_token_at_position(i64::from(start)).unwrap();
        assert_eq!(
            describe(&file, Some(token)),
            Some((container, container_range.0, container_range.1)),
            "token at {start}"
        );
        let preceding = navigator
            .find_preceding_token(i64::from(start) + 1)
            .unwrap();
        assert_eq!(
            describe(&file, preceding),
            Some((K::LessThanLessThanToken, start, start + 2)),
            "preceding {}",
            start + 1
        );
        assert_eq!(
            parent_kind(preceding.unwrap()),
            container,
            "the scanned token's containing node at {}",
            start + 1
        );
    }
}
