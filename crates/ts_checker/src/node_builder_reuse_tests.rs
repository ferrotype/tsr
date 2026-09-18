use super::*;
use crate::{CheckerOptions, CheckerState};
use ts_arena::{CheckerIdentity, Counters, Generation};
use ts_jsstring::SourceText;

#[test]
fn reused_nonlocal_lists_drop_positions_without_mutating_the_input() {
    for local in [false, true] {
        for replace_child in [false, true] {
            let counters = Counters::new();
            let identity = CheckerIdentity::new(Generation::new(&counters), &counters);
            let mut checker =
                CheckerState::new(&identity, &counters, CheckerOptions::default()).unwrap();
            let mut builder = NodeBuilder::new(&mut checker, 0);
            let element = builder.ast.new_keyword_type_node(K::StringKeyword.into());
            let elements = builder.list(vec![element]).unwrap();
            let range = TextRange::new(10, 16);
            builder.ast.set_list_location(elements, range).unwrap();
            let tuple = builder.ast.new_tuple_type_node(Some(elements));
            let statements = builder.list(vec![tuple]).unwrap();
            let source = builder.ast.new_source_file(
                ts_ast::SourceFileParseOptions {
                    file_name: JsString::from_bytes(b"/input.ts".as_slice()),
                    ..Default::default()
                },
                SourceText::from_bytes(b"".as_slice()),
                Some(statements),
                None,
            );
            builder.ast.set_node_parent(tuple, Some(source));
            builder.ast.set_node_parent(element, Some(tuple));
            let other = builder.ast.new_source_file(
                ts_ast::SourceFileParseOptions {
                    file_name: JsString::from_bytes(b"/other.ts".as_slice()),
                    ..Default::default()
                },
                SourceText::from_bytes(b"".as_slice()),
                None,
                None,
            );
            builder.enclosing = Some(if local { source } else { other });
            let mut replacements = std::collections::HashMap::new();
            if replace_child {
                let replacement = builder.ast.new_keyword_type_node(K::NumberKeyword.into());
                replacements.insert(element, Some(replacement));
            }
            let result = builder
                .reuse_replace_children(tuple, &replacements)
                .unwrap();
            let view = builder.ast.view();
            let output = view
                .node(result)
                .unwrap()
                .data_source()
                .as_tuple_type_node()
                .unwrap()
                .elements()
                .unwrap();
            assert_eq!(
                view.list(output).unwrap().loc(),
                if local { range } else { TextRange::new(-1, -1) }
            );
            assert_eq!(view.list(elements).unwrap().loc(), range);
            if !local {
                assert_ne!(output, elements);
            }
        }
    }
}
