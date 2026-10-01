//! A program parses each file into its own arena, so a mapped file names its
//! supplemental files, and a supplemental file its canonical file, by file
//! name. Their encoding must equal that of the same links held as nodes of
//! one arena, which the S06 codec fixtures compare with the pin.
use tsr_ast::{
    AstBuilder, ContentMapperSourceFileInfo, EagerJsDocProvider, FactoryMethods, JsString,
    SourceFileParseOptions, SyntaxKind,
};
use tsr_core::TextRange;
use tsr_jsstring::SourceText;

fn name(text: &str) -> JsString {
    JsString::from_bytes(text.as_bytes())
}

fn options(file: &str) -> SourceFileParseOptions {
    SourceFileParseOptions {
        file_name: name(file),
        path: name(file),
        ..SourceFileParseOptions::default()
    }
}

/// The encoding of a mapped `/app.box` carrying the links `link` makes.
fn encode(link: impl FnOnce(&mut AstBuilder) -> ContentMapperSourceFileInfo) -> Vec<u8> {
    let mut f = AstBuilder::new(SourceText::default(), &tsr_arena::Counters::new());
    let eof = f.new_token(SyntaxKind::EndOfFile.into());
    f.node_mut(eof).unwrap().set_range(TextRange::new(1, 1));
    let statements = f.node_slice(Vec::new()).unwrap();
    let list = f.new_list(TextRange::new(0, 0), statements).unwrap();
    let root = f.new_source_file(
        options("/app.box"),
        SourceText::from_loaded_bytes(b"x".as_slice()),
        Some(list),
        Some(eof),
    );
    f.node_mut(root).unwrap().set_range(TextRange::new(0, 1));
    let info = ContentMapperSourceFileInfo {
        content_mapper: name("box"),
        virtual_file_name: name("/app.box.ts"),
        ..link(&mut f)
    };
    f.source_file_mut(root)
        .unwrap()
        .set_content_mapper_info(info);
    tsr_encoder::encode_source_file(f.view(), root, &mut EagerJsDocProvider::default())
        .unwrap()
        .bytes
}

#[test]
fn files_linked_by_name_encode_as_files_linked_by_node() {
    let unlinked = encode(|_| ContentMapperSourceFileInfo::default());
    let supplemental_nodes = encode(|f| {
        let one = f.new_source_file(options("/app.box.1.ts"), SourceText::default(), None, None);
        let two = f.new_source_file(options("/app.box.2.ts"), SourceText::default(), None, None);
        ContentMapperSourceFileInfo {
            supplemental_source_files: f.source_nodes(vec![Some(one), Some(two)]).unwrap(),
            ..ContentMapperSourceFileInfo::default()
        }
    });
    let supplemental_names = encode(|_| ContentMapperSourceFileInfo {
        supplemental_file_names: vec![name("/app.box.1.ts"), name("/app.box.2.ts")],
        ..ContentMapperSourceFileInfo::default()
    });
    assert_ne!(supplemental_nodes, unlinked);
    assert_eq!(supplemental_names, supplemental_nodes);

    let canonical_node = encode(|f| {
        let canonical =
            f.new_source_file(options("/canonical.box"), SourceText::default(), None, None);
        ContentMapperSourceFileInfo {
            canonical_source_file: Some(canonical),
            ..ContentMapperSourceFileInfo::default()
        }
    });
    let canonical_name = encode(|_| ContentMapperSourceFileInfo {
        canonical_file_name: Some(name("/canonical.box")),
        ..ContentMapperSourceFileInfo::default()
    });
    assert_ne!(canonical_node, unlinked);
    assert_eq!(canonical_name, canonical_node);
}
