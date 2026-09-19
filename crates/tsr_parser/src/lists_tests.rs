use crate::{Parser, ParserFactory, ParsingContext};
use ts_arena::Counters;
use ts_ast::{AstBuilder, Factory, FactoryMethods, JsString, SourceFileParseOptions, SyntaxKind};
use ts_core::ScriptKind;
use ts_jsstring::SourceText;

#[test]
fn reparsed_elements_precede_their_element_and_count_in_later_callback_indices() {
    for inserted_count in [0, 1, 4] {
        let source = SourceText::from_loaded_bytes(b"a b".as_slice());
        let factory = AstBuilder::new(source.clone(), &Counters::new());
        let mut parser = Parser::new(
            SourceFileParseOptions::default(),
            &source,
            ScriptKind::TS,
            factory,
        );
        let outer = parser.factory.new_identifier(JsString::default());
        let reparsed = parser.factory.new_identifier(JsString::default());
        parser.reparse_list.push(outer);
        parser.parsing_contexts = 1 << ParsingContext::Parameters as u8;
        let saved_contexts = parser.parsing_contexts;
        parser.next_token();
        let mut indices = Vec::new();
        let mut elements = Vec::new();
        let nodes = parser.parse_list_index(ParsingContext::SourceElements, |parser, index| {
            indices.push(index);
            let element = parser.parse_identifier();
            if elements.is_empty() {
                parser
                    .reparse_list
                    .extend(std::iter::repeat_n(reparsed, inserted_count));
            }
            elements.push(element);
            element
        });
        assert_eq!(indices, [0, inserted_count + 1]);
        assert_eq!(parser.parsing_contexts, saved_contexts);
        assert_eq!(parser.reparse_list, [outer]);
        assert_eq!(parser.token, SyntaxKind::EndOfFile);
        let slice = parser.factory.finish_list_buffer(nodes);
        let expected: Vec<_> = std::iter::repeat_n(Some(reparsed), inserted_count)
            .chain(elements.into_iter().map(Some))
            .collect();
        assert_eq!(
            parser
                .factory
                .view()
                .node_slice(slice)
                .unwrap()
                .iter()
                .collect::<Vec<_>>(),
            expected
        );
    }
}

#[test]
fn delimited_abort_restores_context_without_finishing_a_partial_backing() {
    for accepted_count in [1, 5] {
        let source = SourceText::from_loaded_bytes(b"a,b,c,d,e,f".as_slice());
        let factory = AstBuilder::new(source.clone(), &Counters::new());
        let mut parser = Parser::new(
            SourceFileParseOptions::default(),
            &source,
            ScriptKind::TS,
            factory,
        );
        let before = parser.factory.node_slice_from_slice(&[]).unwrap();
        parser.parsing_contexts = 1 << ParsingContext::Parameters as u8;
        let saved_contexts = parser.parsing_contexts;
        parser.next_token();
        let mut parsed = 0;
        let result = parser.parse_delimited_list(ParsingContext::ArgumentExpressions, |parser| {
            if parsed == accepted_count {
                return None;
            }
            parsed += 1;
            Some(parser.parse_identifier())
        });
        assert_eq!(result, None);
        assert_eq!(parsed, accepted_count);
        assert_eq!(parser.parsing_contexts, saved_contexts);
        assert_eq!(parser.token, SyntaxKind::Identifier);
        assert_eq!(parser.identifier_count, i64::from(accepted_count));
        let after = parser.factory.node_slice_from_slice(&[]).unwrap();
        assert_eq!(
            after.backing_id().unwrap().slot(),
            before.backing_id().unwrap().slot() + 1
        );
    }
}

#[test]
fn source_completion_appends_pending_reparse_after_eof_and_keeps_empty_lists_nil() {
    for source_bytes in [b"".as_slice(), b"a;".as_slice()] {
        let source = SourceText::from_loaded_bytes(source_bytes);
        let factory = AstBuilder::new(source.clone(), &Counters::new());
        let mut parser = Parser::new(
            SourceFileParseOptions {
                file_name: JsString::from_bytes(b"/list-test.ts".as_slice()),
                ..SourceFileParseOptions::default()
            },
            &source,
            ScriptKind::TS,
            factory,
        );
        let pending = if source_bytes.is_empty() {
            None
        } else {
            let node = parser.factory.new_identifier(JsString::default());
            parser.reparse_list.push(node);
            Some(node)
        };
        parser.next_token();
        let root = parser.parse_source_file_worker();
        let (statements, eof) = {
            let read = parser.factory.node(root);
            let data = read.as_source_file().unwrap();
            (
                data.statements().unwrap(),
                data.end_of_file_token().unwrap(),
            )
        };
        let list = parser.factory.view().list(statements).unwrap();
        let nodes = parser.factory.view().node_slice(list.nodes()).unwrap();
        if let Some(pending) = pending {
            assert_eq!(nodes.len(), 2);
            assert_eq!(nodes.last(), Some(Some(pending)));
            assert!(nodes.at(0).unwrap().slot() < eof.slot());
            assert!(eof.slot() < root.slot());
        } else {
            assert!(list.nodes().is_nil());
            assert!(nodes.is_empty());
        }
        assert_eq!(parser.factory.node(eof).kind(), SyntaxKind::EndOfFile);
        assert!(parser.reparse_list.is_empty());
    }
}
