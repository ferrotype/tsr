//! Frozen codec constructions; no serialization algorithms live in this adapter.
use crate::protocol::{hex, Session};
use serde_json::{json, Value};
use ts_ast::{
    AstBuilder, ContentMapperSourceFileInfo, EagerJsDocProvider, ExternalModuleIndicatorOptions,
    FactoryMethods, FileReference, JsString, MappedDiagnosticDirective, NodeId, NodeKind,
    SourceFileParseOptions, SourceHash, SpanSegment, SyntaxKind,
};
use ts_core::{LanguageVariant, ScriptKind, TextRange};
use ts_jsstring::SourceText;
pub fn scenario(name: &str) -> bool {
    matches!(
        name,
        "literal/StringLiteral"
            | "literal/NumericLiteral"
            | "literal/BigIntLiteral"
            | "literal/RegularExpressionLiteral"
            | "literal/NoSubstitutionTemplateLiteral"
            | "literal/TemplateHead"
            | "literal/TemplateMiddle"
            | "literal/TemplateTail"
            | "literal/Identifier"
            | "literal/PrivateIdentifier"
            | "literal/JsxText"
            | "literal/JSDocText"
            | "literal/JSDocLink"
            | "literal/JSDocLinkPlain"
            | "literal/JSDocLinkCode"
            | "synthetic-expression"
            | "raw-kind"
            | "source-strings"
            | "source-metadata"
            | "source-empty-span"
            | "msgpack-boundaries"
            | "parsed-js/typedef"
            | "parsed-js/import"
    )
}
fn string(text: &[u8]) -> JsString {
    JsString::from_bytes(text)
}
fn literal(f: &mut AstBuilder, name: &str) -> NodeId {
    let text = b"a\xf0\x9f\x98\x80\xed\xa0\x80\xff";
    let raw = br"raw\n\uD800";
    let flags = -1;
    match name {
        "StringLiteral" => f.new_string_literal(string(text), flags),
        "NumericLiteral" => f.new_numeric_literal(string(text), flags),
        "BigIntLiteral" => f.new_big_int_literal(string(text), flags),
        "RegularExpressionLiteral" => f.new_regular_expression_literal(string(text), flags),
        "NoSubstitutionTemplateLiteral" => {
            f.new_no_substitution_template_literal(string(text), flags)
        }
        "TemplateHead" => f.new_template_head(string(text), string(raw), flags),
        "TemplateMiddle" => f.new_template_middle(string(text), string(raw), flags),
        "TemplateTail" => f.new_template_tail(string(text), string(raw), flags),
        "Identifier" => f.new_identifier(string(text)),
        "PrivateIdentifier" => f.new_private_identifier(string(text)),
        "JsxText" => f.new_jsx_text(string(text), true),
        "JSDocText" => {
            let pieces = f
                .text_slice(vec![string(b"a"), string(b""), string(&text[1..])])
                .unwrap();
            f.new_js_doc_text(pieces)
        }
        "JSDocLink" | "JSDocLinkPlain" | "JSDocLinkCode" => {
            let name_node = f.new_identifier(string(b"name"));
            let pieces = f
                .text_slice(vec![string(b"a"), string(b""), string(&text[1..])])
                .unwrap();
            match name {
                "JSDocLink" => f.new_js_doc_link(Some(name_node), pieces),
                "JSDocLinkPlain" => f.new_js_doc_link_plain(Some(name_node), pieces),
                _ => f.new_js_doc_link_code(Some(name_node), pieces),
            }
        }
        _ => unreachable!("validated literal"),
    }
}
fn options(name: &[u8]) -> SourceFileParseOptions {
    SourceFileParseOptions {
        file_name: string(name),
        ..Default::default()
    }
}
fn source(f: &mut AstBuilder, name: &str) -> NodeId {
    let lit = f.new_string_literal(string("😀".as_bytes()), 1024);
    f.node_mut(lit).unwrap().set_range(TextRange::new(0, 6));
    let a = f.new_identifier(string(b"a"));
    f.node_mut(a).unwrap().set_range(TextRange::new(7, 8));
    let b = f.new_identifier(string(b"a"));
    f.node_mut(b).unwrap().set_range(TextRange::new(9, 10));
    let eof = f.new_token(SyntaxKind::EndOfFile.into());
    f.node_mut(eof).unwrap().set_range(TextRange::new(11, 11));
    let nodes = f.node_slice(vec![Some(lit), Some(a), Some(b)]).unwrap();
    let list = f.new_list(TextRange::new(0, 10), nodes).unwrap();
    let opts = SourceFileParseOptions {
        file_name: string(b"/s06/codec.ts"),
        path: string(b"/s06/codec.ts"),
        external_module_indicator_options: ExternalModuleIndicatorOptions {
            jsx: true,
            force: true,
        },
    };
    let root = f.new_source_file(
        opts,
        SourceText::from_loaded_bytes("'😀' a a\n".as_bytes()),
        Some(list),
        Some(eof),
    );
    f.node_mut(root).unwrap().set_range(TextRange::new(0, 11));
    let nodes = f.node_count();
    let texts = f.text_count();
    {
        let sf = f.source_file_mut(root).unwrap();
        sf.node_count = nodes;
        sf.text_count = texts;
        sf.script_kind = ScriptKind::TS;
        sf.language_variant = LanguageVariant::JSX;
        sf.hash = SourceHash {
            hi: 0x0123_4567_89ab_cdef,
            lo: 0xfedc_ba98_7654_3210,
        };
    }
    if name == "source-metadata" || name == "source-empty-span" {
        let missing_a = f.new_identifier(string(b"missing"));
        let missing_lit = f.new_string_literal(string(b"missing"), 0);
        let one = f.new_source_file(options(b"/s06/one.ts"), SourceText::default(), None, None);
        let two = f.new_source_file(options(b"/s06/two.ts"), SourceText::default(), None, None);
        let canonical = f.new_source_file(
            options(b"/s06/canonical.ts"),
            SourceText::default(),
            None,
            None,
        );
        let module_augmentations = f
            .source_nodes(vec![Some(a), Some(missing_a), None])
            .unwrap();
        let imports = f
            .source_nodes(vec![Some(lit), Some(missing_lit), None])
            .unwrap();
        let ambient_module_names = f
            .source_strings(vec![
                string(b""),
                string(b"repeat"),
                string(b"repeat"),
                string(b"\xff"),
            ])
            .unwrap();
        let referenced_files = f
            .source_references(vec![FileReference {
                loc: TextRange::new(1, 5),
                file_name: string(b"\xff"),
                resolution_mode: -1,
                preserve: true,
            }])
            .unwrap();
        let type_reference_directives = f
            .source_references(vec![FileReference {
                loc: TextRange::new(-1, 128),
                file_name: string(b"types"),
                resolution_mode: 99,
                preserve: false,
            }])
            .unwrap();
        let lib_reference_directives = f
            .source_references(vec![FileReference {
                loc: TextRange::new(0, 11),
                file_name: string(b"lib"),
                resolution_mode: 0,
                preserve: true,
            }])
            .unwrap();
        let segments = if name == "source-empty-span" {
            vec![]
        } else {
            vec![
                SpanSegment {
                    virtual_start: 1,
                    virtual_end: 5,
                    original_start: 1,
                    original_end: 5,
                    kind: 0,
                    features: SpanSegment::FEATURE_ALL,
                },
                SpanSegment {
                    virtual_start: 7,
                    virtual_end: 10,
                    original_start: 5,
                    original_end: 6,
                    kind: 2,
                    features: 0,
                },
            ]
        };
        let supplemental_source_files = f.source_nodes(vec![Some(one), Some(two)]).unwrap();
        let diagnostic_directives = f
            .source_diagnostic_directives(vec![
                MappedDiagnosticDirective {
                    original_range: TextRange::new(1, 5),
                    virtual_range: TextRange::new(1, 5),
                    policy: 1,
                    unused_code: -1,
                    unused_message_text: string(b"not serialized"),
                    source: string(b"not serialized"),
                },
                MappedDiagnosticDirective {
                    original_range: TextRange::new(-1, 128),
                    virtual_range: TextRange::new(-1, 256),
                    policy: 255,
                    unused_code: 65536,
                    unused_message_text: string(b"not serialized"),
                    source: string(b"not serialized"),
                },
            ])
            .unwrap();
        let sf = f.source_file_mut(root).unwrap();
        sf.module_augmentations = module_augmentations;
        sf.imports = imports;
        sf.external_module_indicator = Some(root);
        sf.ambient_module_names = ambient_module_names;
        sf.referenced_files = referenced_files;
        sf.type_reference_directives = type_reference_directives;
        sf.lib_reference_directives = lib_reference_directives;
        sf.set_content_mapper_info(ContentMapperSourceFileInfo {
            content_mapper: string(b"mapper"),
            virtual_file_name: string(b"/s06/codec.virtual.ts"),
            original_text: SourceText::from_loaded_bytes("X😀Y\n".as_bytes()),
            span_map: Some(segments.into()),
            supplemental_source_files,
            canonical_source_file: Some(canonical),
            diagnostic_directives,
            ..Default::default()
        });
    } else if name == "msgpack-boundaries" {
        let mut names = vec![JsString::default(); 65536];
        for (i, n) in [0, 31, 32, 255, 256, 65535, 65536, 1]
            .into_iter()
            .enumerate()
        {
            names[i] = string(&vec![b'x'; n]);
        }
        let positions = [0, 127, 128, 255, 256, 65535, 65536, -1];
        let mut references = Vec::with_capacity(16);
        for i in 0..16 {
            let j = i % 8;
            let p = positions[j];
            references.push(FileReference {
                loc: TextRange::new(p, p),
                file_name: names[j].clone(),
                resolution_mode: p,
                preserve: i % 2 == 0,
            });
        }
        let ambient_module_names = f.source_strings(names).unwrap();
        let referenced_files = f.source_references(references).unwrap();
        let sf = f.source_file_mut(root).unwrap();
        sf.ambient_module_names = ambient_module_names;
        sf.referenced_files = referenced_files;
        sf.type_reference_directives = referenced_files.slice(0..15).unwrap();
    }
    root
}
fn bytes(s: &Session, stage: &str, data: &[u8]) {
    s.observe(stage, "bytes", json!({"length":data.len()}));
    for (i, chunk) in data.chunks(65536).enumerate() {
        s.observe(stage, "chunk", json!({"offset":i*65536,"hex":hex(chunk)}));
    }
}
pub fn execute(s: &Session, r: &Value) {
    let mut encoded = Vec::new();
    if !s.stage("encode", || {
        let mut f = AstBuilder::new(SourceText::default(), &ts_arena::Counters::new());
        let name = r["scenario"].as_str().expect("validated codec");
        if let Some(syntax) = name.strip_prefix("parsed-js/") {
            let text = match syntax {
                "typedef" => b"/** @typedef {number} Foo */ const x=0;".as_slice(),
                "import" => b"/** @import {Foo} from \"bar\" */ const x=0;".as_slice(),
                _ => unreachable!("validated parsed-JS scenario"),
            };
            let parsed = ts_parser::parse_source_file(
                SourceText::from_loaded_bytes(text),
                ScriptKind::JS,
                SourceFileParseOptions {
                    file_name: string(b"/s06/codec.js"),
                    path: string(b"/s06/codec.js"),
                    ..Default::default()
                },
            );
            encoded = ts_encoder::encode_source_file(
                parsed.view(),
                parsed.root(),
                &mut ts_parser::ParserJsDocProvider::default(),
            )
            .map_err(|e| e.to_string())?
            .bytes;
            bytes(s, "encode", &encoded);
            return Ok(());
        }
        let (root, sf) = if let Some(name) = name.strip_prefix("literal/") {
            let root = literal(&mut f, name);
            f.node_mut(root).unwrap().set_range(TextRange::new(-1, 2));
            f.node_mut(root).unwrap().set_flags(0xa5a5_a5a5);
            (root, None)
        } else if name == "synthetic-expression" {
            (f.new_token(SyntaxKind::SyntheticExpression.into()), None)
        } else if name == "raw-kind" {
            let root = f.new_token(NodeKind::from_raw(-1));
            f.node_mut(root)
                .unwrap()
                .set_range(TextRange::new(-1, 2_147_483_647));
            f.node_mut(root).unwrap().set_flags(u32::MAX);
            (root, None)
        } else {
            let root = source(&mut f, name);
            (root, Some(root))
        };
        let mut provider = EagerJsDocProvider::default();
        let result = if let Some(sf) = sf {
            ts_encoder::encode_source_file(f.view(), sf, &mut provider)
        } else {
            ts_encoder::encode_node(f.view(), root, None, &mut provider)
        };
        encoded = result.map_err(|e| e.to_string())?.bytes;
        bytes(s, "encode", &encoded);
        Ok(())
    }) {
        return;
    }
    let mut tree = None;
    if !s.stage("decode", || {
        let decoded = ts_encoder::decode_nodes(&encoded, &ts_arena::Counters::new())
            .map_err(|e| e.to_string())?;
        let root = decoded.root.map(|id| {
            let node = decoded.builder.view().node(id).unwrap();
            json!({"kind":node.kind().raw(),"pos":node.pos(),"end":node.end(),"flags":node.flags()})
        });
        s.observe("decode", "root", json!({"node":root}));
        tree = Some(decoded);
        Ok(())
    }) {
        return;
    }
    let tree = tree.expect("successful decoder");
    if !s.stage("decoded_tree", || {
        super::emit_tree(s, tree.builder.view(), tree.root);
        Ok(())
    }) {
        return;
    }
    s.stage("reencode", || {
        let mut provider = EagerJsDocProvider::default();
        let view = tree.builder.view();
        let result = if tree
            .root
            .is_some_and(|id| view.node(id).unwrap().kind() == SyntaxKind::SourceFile)
        {
            ts_encoder::encode_source_file(view, tree.root.unwrap(), &mut provider)
        } else {
            ts_encoder::encode_optional_node(view, tree.root, None, &mut provider)
        };
        bytes(s, "reencode", &result.map_err(|e| e.to_string())?.bytes);
        Ok(())
    });
}
