//! The two encoder baselines of tsc/internal/api/encoder/encoder_test.go
//! (`testdata/baselines/reference/api/encodeSourceFile.txt` and
//! `encodeSourceFileWithUnicodeEscapes.txt`), reproduced from the pinned
//! inputs through the Rust encoder and the pin's listing format.
use crate::generated::{
    HEADER_OFFSET_EXTENDED_DATA, HEADER_OFFSET_NODES, HEADER_OFFSET_STRING_DATA,
    HEADER_OFFSET_STRING_OFFSETS, HEADER_OFFSET_STRUCTURED_DATA, NODE_DATA_STRING_INDEX_MASK,
    NODE_DATA_TYPE_MASK, NODE_DATA_TYPE_STRING, NODE_OFFSET_DATA, NODE_OFFSET_END,
    NODE_OFFSET_KIND, NODE_OFFSET_NEXT, NODE_OFFSET_PARENT, NODE_OFFSET_POS, NODE_SIZE,
    SYNTAX_KIND_NODE_LIST,
};
use std::fmt::Write as _;
use tsr_ast::{AstFile, SourceFileParseOptions, SyntaxKind};

fn parse(text: &str) -> AstFile {
    tsr_parser::parse_source_file(
        tsr_jsstring::SourceText::from_loaded_bytes(text.as_bytes().to_vec()),
        tsr_core::ScriptKind::TS,
        SourceFileParseOptions {
            file_name: tsr_ast::JsString::from_bytes(b"/test.ts".as_slice()),
            path: tsr_ast::JsString::from_bytes(b"/test.ts".as_slice()),
            ..Default::default()
        },
    )
    .publish_unbound()
}

fn read_u32(encoded: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(encoded[offset..offset + 4].try_into().unwrap())
}

/// The listing the Go test writes: one line per node with its kind, string,
/// span, one-based index and `next` byte, indented by depth.
/// Follows `formatEncodedSourceFile` of encoder_test.go.
fn format_encoded_source_file(encoded: &[u8]) -> String {
    let node_size = NODE_SIZE as usize;
    let offset_nodes = read_u32(encoded, HEADER_OFFSET_NODES as usize) as usize;
    let offset_string_offsets = read_u32(encoded, HEADER_OFFSET_STRING_OFFSETS as usize) as usize;
    let offset_strings = read_u32(encoded, HEADER_OFFSET_STRING_DATA as usize) as usize;
    fn indent(encoded: &[u8], offset_nodes: usize, node_size: usize, parent: u32) -> String {
        if parent == 0 {
            return String::new();
        }
        let grandparent = read_u32(
            encoded,
            offset_nodes + parent as usize * node_size + NODE_OFFSET_PARENT as usize,
        );
        format!(
            "  {}",
            indent(encoded, offset_nodes, node_size, grandparent)
        )
    }
    let mut result = String::new();
    let mut j = 1;
    let mut i = offset_nodes + node_size;
    while i < encoded.len() {
        let kind = read_u32(encoded, i + NODE_OFFSET_KIND as usize);
        let pos = read_u32(encoded, i + NODE_OFFSET_POS as usize);
        let end = read_u32(encoded, i + NODE_OFFSET_END as usize);
        let parent = read_u32(encoded, i + NODE_OFFSET_PARENT as usize);
        result.push_str(&indent(encoded, offset_nodes, node_size, parent));
        if kind == SYNTAX_KIND_NODE_LIST {
            result.push_str("NodeList");
        } else {
            let name = u16::try_from(kind)
                .ok()
                .and_then(SyntaxKind::from_u16)
                .map_or_else(|| format!("Kind({kind})"), |kind| format!("Kind{kind:?}"));
            result.push_str(&name);
        }
        let data = read_u32(encoded, i + NODE_OFFSET_DATA as usize);
        let data_type = data & NODE_DATA_TYPE_MASK;
        if kind == SyntaxKind::Identifier as u32 || data_type == NODE_DATA_TYPE_STRING {
            let string_index = (data & NODE_DATA_STRING_INDEX_MASK) as usize;
            let start = read_u32(encoded, offset_string_offsets + string_index * 4) as usize;
            let end = read_u32(encoded, offset_string_offsets + string_index * 4 + 4) as usize;
            let text =
                String::from_utf8_lossy(&encoded[offset_strings + start..offset_strings + end]);
            write!(result, " \"{text}\"").unwrap();
        }
        writeln!(
            result,
            " [{pos}, {end}), i={j}, next={}",
            encoded[i + NODE_OFFSET_NEXT as usize]
        )
        .unwrap();
        j += 1;
        i += node_size;
    }
    result
}

fn encoded_listing(text: &str) -> String {
    let file = parse(text);
    let encoded = crate::encode_source_file(
        file.view(),
        file.root().unwrap(),
        &mut tsr_parser::ParserJsDocProvider::default(),
    )
    .unwrap();
    format_encoded_source_file(&encoded.bytes)
}

fn baseline(name: &str) -> String {
    std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("testdata/api/{name}")),
    )
    .unwrap()
}

/// `TestEncodeSourceFile`'s baseline.
#[test]
fn encode_source_file_matches_the_baseline() {
    let listing = encoded_listing(
        "import { bar } from \"bar\";\nexport function foo<T, U>(a: string, b: string): any {}\nfoo();",
    );
    assert_eq!(listing, baseline("encodeSourceFile.txt"));
}

/// `TestEncodeSourceFileWithUnicodeEscapes`'s baseline: lone surrogates in
/// string literals survive the string table.
#[test]
fn encode_source_file_with_unicode_escapes_matches_the_baseline() {
    let listing = encoded_listing(
        r#"let a = "😃"; let b = "\ud83d\ude03"; let c = "\udc00\ud83d\ude03"; let d = "\ud83d\ud83d\ude03""#,
    );
    assert_eq!(listing, baseline("encodeSourceFileWithUnicodeEscapes.txt"));
}

/// Ports `TestBuildNodeIndexTableMatchesEncode`: the table built by walking
/// the tree and the table the encoder records agree node for node.
#[test]
fn the_node_index_table_matches_the_encoding() {
    let file = parse(
        "import { bar } from \"bar\";\nexport function foo<T, U>(a: string, b: string): any {}\nfoo();",
    );
    let view = file.view();
    let source = file.root().unwrap();
    let encoded = crate::encode_source_file(
        view,
        source,
        &mut tsr_parser::ParserJsDocProvider::default(),
    )
    .unwrap();
    let encoded_table = encoded.index.expect("a fresh encoding records its table");
    let built = crate::build_node_index_table(
        view,
        source,
        &mut tsr_parser::ParserJsDocProvider::default(),
    )
    .unwrap();
    assert_eq!(built.nodes(), encoded_table.nodes());
    for (index, node) in encoded_table.nodes().iter().enumerate() {
        let Some(node) = node else { continue };
        let read = view.node(*node).unwrap();
        let from_encoding = encoded_table.get_index(view, Some(&read)).unwrap();
        let from_build = built.get_index(view, Some(&read)).unwrap();
        assert_eq!(
            from_encoding,
            u32::try_from(index).unwrap(),
            "encoded table index of node {index}"
        );
        assert_eq!(
            from_build, from_encoding,
            "built table index of node {index}"
        );
    }
}

/// Ports `TestEncodeContentMapperSourceFileMetadata`: a mapped file's
/// content mapper, virtual file name and diagnostic directives travel in the
/// extended data at the pin's offsets, the directives as a msgpack tuple in
/// UTF-16 positions.
#[test]
fn content_mapper_metadata_is_encoded_in_the_extended_data() {
    let mut parsed = tsr_parser::parse_source_file(
        tsr_jsstring::SourceText::from_loaded_bytes("😀virtual".as_bytes().to_vec()),
        tsr_core::ScriptKind::TS,
        SourceFileParseOptions {
            file_name: tsr_ast::JsString::from_bytes(b"/component.vue".as_slice()),
            path: tsr_ast::JsString::from_bytes(b"/component.vue".as_slice()),
            ..Default::default()
        },
    );
    let directives = parsed
        .builder_mut()
        .source_diagnostic_directives(vec![tsr_ast::MappedDiagnosticDirective {
            original_range: tsr_core::TextRange::new(4, 5),
            virtual_range: tsr_core::TextRange::new(4, 11),
            policy: 1,
            unused_code: 2578,
            unused_message_text: tsr_ast::JsString::from_bytes(
                b"Unused framework directive.".as_slice(),
            ),
            source: tsr_ast::JsString::from_bytes(b"mapper".as_slice()),
        }])
        .unwrap();
    parsed
        .root_source_file_mut()
        .unwrap()
        .set_content_mapper_info(tsr_ast::ContentMapperSourceFileInfo {
            content_mapper: tsr_ast::JsString::from_bytes(b"mapper@1.0.0".as_slice()),
            virtual_file_name: tsr_ast::JsString::from_bytes(b"/component.vue.ts".as_slice()),
            original_text: tsr_jsstring::SourceText::from_loaded_bytes(
                "😀original".as_bytes().to_vec(),
            ),
            diagnostic_directives: directives,
            ..Default::default()
        });
    let file = parsed.publish_unbound();
    let encoded = crate::encode_source_file(
        file.view(),
        file.root().unwrap(),
        &mut tsr_parser::ParserJsDocProvider::default(),
    )
    .unwrap()
    .bytes;
    let nodes_offset = read_u32(&encoded, HEADER_OFFSET_NODES as usize) as usize;
    let root_data = read_u32(
        &encoded,
        nodes_offset + NODE_SIZE as usize + NODE_OFFSET_DATA as usize,
    );
    let extended_offset = (read_u32(&encoded, HEADER_OFFSET_EXTENDED_DATA as usize)
        + (root_data & NODE_DATA_STRING_INDEX_MASK)) as usize;
    assert!(
        extended_offset + 76 <= encoded.len(),
        "extended offset {extended_offset} in {} bytes",
        encoded.len()
    );
    let content_mapper_index = read_u32(&encoded, extended_offset + 64);
    let virtual_file_name_index = read_u32(&encoded, extended_offset + 68);
    let directives_offset = read_u32(&encoded, extended_offset + 72) as usize;
    assert_eq!(
        encoded_string(&encoded, content_mapper_index),
        "mapper@1.0.0"
    );
    assert_eq!(
        encoded_string(&encoded, virtual_file_name_index),
        "/component.vue.ts"
    );
    let structured = read_u32(&encoded, HEADER_OFFSET_STRUCTURED_DATA as usize) as usize;
    let directive = structured + directives_offset;
    assert_eq!(
        &encoded[directive..directive + 10],
        &[
            0x91, // one directive
            0x96, // a six-element tuple
            2, 1, // the original range [2, 3) in UTF-16
            2, 7, // the virtual range [2, 9) in UTF-16
            1, // the expect policy
            0xcd, 10, 18, // the unused diagnostic code 2578
        ]
    );
}

fn encoded_string(encoded: &[u8], index: u32) -> String {
    let string_offsets = read_u32(encoded, HEADER_OFFSET_STRING_OFFSETS as usize) as usize;
    let string_data = read_u32(encoded, HEADER_OFFSET_STRING_DATA as usize) as usize;
    let start = read_u32(encoded, string_offsets + index as usize * 4) as usize;
    let end = read_u32(encoded, string_offsets + index as usize * 4 + 4) as usize;
    String::from_utf8_lossy(&encoded[string_data + start..string_data + end]).into_owned()
}

/// The copied baseline files are the pin's, byte for byte, so a pin bump
/// that changes them is caught here when the submodule is checked out.
#[test]
fn the_copied_baselines_are_the_pinned_files() {
    let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let pinned = manifest.join("../../upstream/tsc/testdata/baselines/reference/api");
    if !pinned.is_dir() {
        return;
    }
    for name in [
        "encodeSourceFile.txt",
        "encodeSourceFileWithUnicodeEscapes.txt",
    ] {
        let expected = std::fs::read(pinned.join(name)).expect("the pinned baseline");
        let copied = std::fs::read(manifest.join("testdata/api").join(name)).expect("the copy");
        assert!(
            expected == copied,
            "{name} differs from the pinned baseline"
        );
    }
}
