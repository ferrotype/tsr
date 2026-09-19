use ts_arena::Counters;
use ts_ast::{
    AstBuilder, CheckJsDirective, CommentRange, ContentMapperSourceFileInfo, Diagnostic, Factory,
    FactoryMethods, JsString, SourceFileParseOptions, SourceHash, SyntaxKind,
};
use ts_core::{ScriptKind, TextRange};
use ts_jsstring::SourceText;

fn options(name: &[u8]) -> SourceFileParseOptions {
    SourceFileParseOptions {
        file_name: JsString::from_bytes(name),
        path: JsString::from_bytes(name),
        ..SourceFileParseOptions::default()
    }
}

#[test]
fn source_file_clone_copies_only_the_pinned_copy_from_fields() {
    let counters = Counters::new();
    let text = SourceText::from_loaded_bytes(&b"// @ts-check\nexport {}"[..]);
    let mut factory = AstBuilder::new(text.clone(), &counters);
    let eof = factory.new_token(SyntaxKind::EndOfFile.into());
    let original = factory.new_source_file(options(b"/file.ts"), text, None, Some(eof));
    factory
        .node_mut(original)
        .unwrap()
        .set_range(TextRange::new(0, 22));
    factory.set_node_flags(original, 7);
    {
        let file = factory.source_file_mut(original).unwrap();
        file.script_kind = ScriptKind::TS;
        file.identifier_count = 3;
        file.node_count = 7;
        file.text_count = 11;
        file.hash = SourceHash { hi: 13, lo: 17 };
        file.has_lazy_jsdoc = true;
        file.check_js_directive = Some(CheckJsDirective {
            enabled: true,
            range: CommentRange::default(),
        });
        file.external_module_indicator = Some(original);
        file.diagnostics.push(Diagnostic::compiler(
            ts_diagnostics::by_code(1005).unwrap(),
            Vec::new(),
        ));
        file.set_content_mapper_info(ContentMapperSourceFileInfo {
            content_mapper: JsString::from_bytes(&b"mapper"[..]),
            original_text: SourceText::from_loaded_bytes(&b"original\xff"[..]),
            ..ContentMapperSourceFileInfo::default()
        });
    }
    assert_eq!(
        factory.update_source_file(original, None, Some(eof)),
        original
    );
    let clone = factory.clone_source_file(original);
    let view = factory.view();
    let file = view.source_file(clone).unwrap();
    assert_ne!(clone, original);
    assert_eq!(file.script_kind, ScriptKind::TS);
    assert_eq!(file.external_module_indicator, Some(original));
    assert_eq!(file.content_mapper(), b"mapper");
    assert_eq!(file.original_text(), b"original\xff");
    assert!(file.is_content_mapper_failure_stub());
    assert_eq!(
        (file.identifier_count, file.node_count, file.text_count),
        (0, 0, 0)
    );
    assert_eq!(file.hash, SourceHash::default());
    assert!(file.diagnostics.is_empty());
    assert!(file.check_js_directive.is_none());
    assert!(!file.has_lazy_jsdoc);
    assert_eq!(
        view.node(clone).unwrap().range(),
        view.node(original).unwrap().range()
    );
    assert_eq!(view.node(clone).unwrap().flags(), 7);
}

#[test]
fn logical_source_files_keep_their_own_text_and_position_maps() {
    let counters = Counters::new();
    let mut factory = AstBuilder::new(SourceText::default(), &counters);
    let first = factory.new_source_file(
        options(b"/one.ts"),
        SourceText::from_loaded_bytes("😀".as_bytes()),
        None,
        None,
    );
    let second = factory.new_source_file(
        options(b"/two.ts"),
        SourceText::from_loaded_bytes(&b"abcd"[..]),
        None,
        None,
    );
    let file = factory.complete(first).unwrap().publish_unbound();
    let view = file.view();
    assert_eq!(
        view.source_file(first)
            .unwrap()
            .position_map()
            .utf8_to_utf16(4),
        2
    );
    assert_eq!(
        view.source_file(second)
            .unwrap()
            .position_map()
            .utf8_to_utf16(4),
        4
    );
    let retained = file.retain_node(second).unwrap();
    drop(file);
    let retained_file = retained.file();
    assert_eq!(
        retained_file
            .view()
            .source_file(second)
            .unwrap()
            .text()
            .as_bytes(),
        b"abcd"
    );
}

#[test]
fn parser_text_is_not_decoded_during_source_file_construction() {
    let counters = Counters::new();
    let text = SourceText::from_loaded_bytes(&b"\xff\xfea\0"[..]);
    let mut factory = AstBuilder::new(text.clone(), &counters);
    let root = factory.new_source_file(options(b"C:"), text, None, None);
    assert_eq!(
        factory.view().source_file(root).unwrap().text().as_bytes(),
        b"\xff\xfea\0"
    );
    let invalid = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        factory.new_source_file(options(b"/a/../bad.ts"), SourceText::default(), None, None);
    }));
    assert!(invalid.is_err());
}

#[test]
fn node_index_cache_keeps_go_once_panic_and_rust_ownership_error_distinct() {
    let mut factory = AstBuilder::new(SourceText::default(), &Counters::new());
    let source = factory.new_source_file(options(b"/cache.ts"), SourceText::default(), None, None);
    let panic_source =
        factory.new_source_file(options(b"/panic.ts"), SourceText::default(), None, None);
    let reentrant_source =
        factory.new_source_file(options(b"/reentrant.ts"), SourceText::default(), None, None);
    let view = factory.view();
    let file = view.source_file(source).unwrap();
    assert!(file
        .try_node_index_cache(|| Err(ts_arena::Error::WrongOwner))
        .is_err());
    let table = file
        .node_index_cache(|| ts_ast::NodeIndexCache::new(vec![None]))
        .unwrap();
    assert_eq!(table.nodes(), &[None]);
    assert!(std::ptr::eq(
        table,
        file.node_index_cache(|| panic!("cache hit must not invoke initializer"))
            .unwrap()
    ));

    let panicking = view.source_file(panic_source).unwrap();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        panicking.node_index_cache(|| panic!("source initializer failed"));
    }));
    assert!(result.is_err());
    assert!(panicking
        .node_index_cache(|| panic!("Go Once does not retry after panic"))
        .is_none());

    let reentrant = view.source_file(reentrant_source).unwrap();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        reentrant.node_index_cache(|| {
            reentrant.node_index_cache(|| ts_ast::NodeIndexCache::new(vec![]));
            unreachable!("same-thread reentry must be diagnosed");
        });
    }));
    assert!(result.is_err());
    assert!(reentrant.node_index_cache(|| unreachable!()).is_none());
}
