use crate::ParserJsDocProvider;
use std::sync::Arc;
use ts_ast::{node_flags, JsDocProvider, JsString, NodeId, ParsedFile, SourceFileParseOptions};
use ts_core::ScriptKind;
use ts_jsstring::SourceText;

fn parse(name: &[u8], text: &[u8], kind: ScriptKind) -> ParsedFile {
    crate::parse_source_file(
        SourceText::from_loaded_bytes(text),
        kind,
        SourceFileParseOptions {
            file_name: JsString::from_bytes(name),
            ..SourceFileParseOptions::default()
        },
    )
}
fn first_statement(file: &ParsedFile) -> NodeId {
    let view = file.view();
    let list = view
        .node(file.root())
        .unwrap()
        .data_source()
        .as_source_file()
        .unwrap()
        .statements()
        .unwrap();
    view.node_slice(view.list(list).unwrap().nodes())
        .unwrap()
        .at(0)
        .unwrap()
}

#[test]
fn empty_jsdoc_comment_has_the_pinned_non_nil_backing() {
    let file = parse(
        b"/comment.js",
        b"/** @constructor */ function Box(x) { this.x = x; }",
        ScriptKind::JS,
    );
    let parent = first_statement(&file);
    let view = file.view();
    let docs = view
        .source_eager_jsdoc(file.root(), parent)
        .unwrap()
        .unwrap();
    let doc = view.node(docs[0]).unwrap();
    let list = view
        .list(doc.data_source().as_js_doc().unwrap().comment().unwrap())
        .unwrap();
    assert_eq!(list.loc(), ts_core::TextRange::new(0, 4));
    assert!(!list.nodes().is_nil());
    assert!(view.node_slice(list.nodes()).unwrap().is_empty());
}

#[test]
fn leading_jsdoc_link_preserves_nil_text_slice() {
    // Pinned conformance/jsdoc/jsdocLinkTag5.ts: the leading JSDocText exists
    // with range 0..4, while stringSliceArena.Clone gives it a nil text slice.
    let file = parse(
        b"/link.ts",
        b"/** {@link UNRESOLVED_LINK} */\nexport interface A {}",
        ScriptKind::TS,
    );
    let parent = first_statement(&file);
    let view = file.view();
    let docs = view
        .source_eager_jsdoc(file.root(), parent)
        .unwrap()
        .unwrap();
    let doc = view.node(docs[0]).unwrap();
    let comment = view
        .list(doc.data_source().as_js_doc().unwrap().comment().unwrap())
        .unwrap();
    let parts = view.node_slice(comment.nodes()).unwrap();
    let leading = view.node(parts.at(0).unwrap()).unwrap();
    assert_eq!(leading.range(), ts_core::TextRange::new(0, 4));
    let text = leading.data_source().as_js_doc_text().unwrap().text();
    assert!(text.is_nil());
    assert!(view.text_slice(text).unwrap().is_empty());
    let link = view.node(parts.at(1).unwrap()).unwrap();
    assert_eq!(link.kind(), ts_ast::SyntaxKind::JSDocLink);
    assert!(link.data_source().as_js_doc_link().unwrap().text().is_nil());
}

#[test]
fn lazy_jsdoc_publishes_once_without_changing_source_parse_counts_or_diagnostics() {
    let file = parse(b"/lazy.ts", b"/** Hello */ const x = 1;", ScriptKind::TS);
    let parent = first_statement(&file);
    let root = file.root();
    let view = file.view();
    assert_ne!(
        view.node(parent).unwrap().flags() & node_flags::HAS_JS_DOC,
        0
    );
    assert!(view.source_eager_jsdoc(root, parent).unwrap().is_none());
    let before = view.source_file(root).unwrap();
    let counts = (
        before.node_count,
        before.text_count,
        before.identifier_count,
    );
    let diagnostic_counts = (before.diagnostics.len(), before.jsdoc_diagnostics.len());
    let mut provider = ParserJsDocProvider::default();
    let roots = provider.jsdoc(view, root, parent).unwrap();
    assert_eq!(roots.len(), 1);
    assert_eq!(view.node(roots[0]).unwrap().parent(), Some(parent));
    assert_eq!(view.node(roots[0]).unwrap().range().pos(), 0);
    assert_eq!(
        provider.jsdoc(view, root, parent).unwrap().as_ref(),
        roots.as_ref()
    );
    assert_eq!(
        view.source_eager_jsdoc(root, parent)
            .unwrap()
            .unwrap()
            .as_ref(),
        roots.as_ref()
    );
    let after = view.source_file(root).unwrap();
    assert_eq!(
        (after.node_count, after.text_count, after.identifier_count),
        counts
    );
    assert_eq!(
        (after.diagnostics.len(), after.jsdoc_diagnostics.len()),
        diagnostic_counts
    );
}

#[test]
fn ordinary_nodes_and_empty_flagged_comments_have_distinct_cache_paths() {
    let mut file = parse(b"/empty.ts", b"const x = 1;", ScriptKind::TS);
    let parent = first_statement(&file);
    let root = file.root();
    let mut provider = ParserJsDocProvider::default();
    assert!(provider
        .jsdoc(file.view(), root, parent)
        .unwrap()
        .is_empty());
    assert!(file
        .view()
        .source_eager_jsdoc(root, parent)
        .unwrap()
        .is_none());
    ts_ast::Factory::add_node_flags(file.builder_mut(), parent, node_flags::HAS_JS_DOC);
    assert!(provider
        .jsdoc(file.view(), root, parent)
        .unwrap()
        .is_empty());
    assert!(file
        .view()
        .source_eager_jsdoc(root, parent)
        .unwrap()
        .unwrap()
        .is_empty());
}

#[test]
fn published_lazy_jsdoc_races_return_the_same_published_ids() {
    let file = parse(
        b"/racing.ts",
        b"/** @param {number} x */ function f(x) {}",
        ScriptKind::TS,
    );
    let parent = first_statement(&file);
    let root = file.root();
    let file = Arc::new(file.publish_unbound());
    let results = std::thread::scope(|scope| {
        let mut workers = Vec::new();
        for _ in 0..4 {
            let file = file.clone();
            workers.push(scope.spawn(move || {
                let mut provider = ParserJsDocProvider::default();
                provider.jsdoc(file.view(), root, parent).unwrap()
            }));
        }
        workers
            .into_iter()
            .map(|worker| worker.join().unwrap())
            .collect::<Vec<_>>()
    });
    assert_eq!(results[0].len(), 1);
    for roots in &results[1..] {
        assert_eq!(roots.as_ref(), results[0].as_ref());
    }
}

#[test]
fn eager_javascript_docs_are_reused_by_the_same_provider() {
    let file = parse(b"/eager.js", b"/** Hello */ const x = 1;", ScriptKind::JS);
    let parent = first_statement(&file);
    let root = file.root();
    let cached = file
        .view()
        .source_eager_jsdoc(root, parent)
        .unwrap()
        .unwrap();
    let mut provider = ParserJsDocProvider::default();
    assert_eq!(
        provider.jsdoc(file.view(), root, parent).unwrap().as_ref(),
        cached.as_ref()
    );
}
