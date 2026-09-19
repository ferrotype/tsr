use crate::Parser;
use ts_arena::Counters;
use ts_ast::{
    node_flags, AstBuilder, Factory, FactoryMethods, JsString, SourceFileParseOptions, SyntaxKind,
};
use ts_core::{ScriptKind, TextRange};
use ts_jsstring::SourceText;

#[test]
fn scanner_errors_are_drained_before_node_finish_and_duplicate_errors_still_mark_nodes() {
    let source = SourceText::from_loaded_bytes(b"'\\xQ'".as_slice());
    let counters = Counters::default();
    let factory = AstBuilder::new(source.clone(), &counters);
    let mut parser = Parser::new(
        SourceFileParseOptions::default(),
        &source,
        ScriptKind::TS,
        factory,
    );
    assert_eq!(parser.next_token(), SyntaxKind::StringLiteral);
    assert_eq!(parser.diagnostics.len(), 1);
    let diagnostic_pos = parser.diagnostics[0].loc.pos();
    let literal = parser.parse_literal_expression();
    assert_ne!(
        parser.factory.node(literal).flags() & node_flags::THIS_NODE_HAS_ERROR,
        0
    );
    assert!(!parser.has_parse_error);
    assert_eq!(
        parser.parse_error_at(
            diagnostic_pos,
            diagnostic_pos + 1,
            ts_diagnostics::Identifier_expected,
            vec![]
        ),
        None
    );
    let missing = parser.create_missing_identifier();
    assert_ne!(
        parser.factory.node(missing).flags() & node_flags::THIS_NODE_HAS_ERROR,
        0
    );
    assert_eq!(parser.diagnostics.len(), 1);
}

#[test]
fn speculation_restores_source_checkpoint_fields_but_keeps_counts_allocations_and_source_flags() {
    let source = SourceText::from_loaded_bytes(b"a b".as_slice());
    let counters = Counters::default();
    let factory = AstBuilder::new(source.clone(), &counters);
    let mut parser = Parser::new(
        SourceFileParseOptions::default(),
        &source,
        ScriptKind::TS,
        factory,
    );
    parser.next_token();
    let outer = parser.mark();
    let first = parser.parse_identifier();
    parser.context_flags |= node_flags::AWAIT_CONTEXT;
    parser.source_flags |= node_flags::POSSIBLY_CONTAINS_DYNAMIC_IMPORT;
    parser.statement_has_await_identifier = true;
    parser.parse_error_at(2, 3, ts_diagnostics::Identifier_expected, vec![]);
    let inner = parser.mark();
    parser.parse_identifier();
    parser.commit(inner);
    parser.rewind(outer);
    assert_eq!(parser.token, SyntaxKind::Identifier);
    assert_eq!(parser.scanner.token_value(), b"a");
    assert_eq!(parser.context_flags, 0);
    assert_eq!(
        parser.source_flags,
        node_flags::POSSIBLY_CONTAINS_DYNAMIC_IMPORT
    );
    assert!(!parser.statement_has_await_identifier);
    assert!(!parser.has_parse_error);
    assert!(parser.diagnostics.is_empty());
    assert_eq!(parser.identifier_count, 2);
    assert_eq!(parser.factory.node_count(), 2);
    assert_eq!(parser.factory.node(first).range(), TextRange::new(0, 1));
}

#[test]
fn parent_assignment_observes_generated_children_and_resets_prior_parents() {
    let source = SourceText::default();
    let counters = Counters::default();
    let factory = AstBuilder::new(source.clone(), &counters);
    let mut parser = Parser::new(
        SourceFileParseOptions::default(),
        &source,
        ScriptKind::TS,
        factory,
    );
    let child = parser.factory.new_identifier(JsString::default());
    let first = parser.factory.new_parenthesized_expression(Some(child));
    parser.finish_node(first, 0);
    assert_eq!(parser.factory.node(child).parent(), Some(first));
    let second = parser.factory.new_parenthesized_expression(Some(child));
    parser.finish_node(second, 0);
    assert_eq!(parser.factory.node(child).parent(), Some(second));
}

#[test]
fn finish_failure_and_parent_failure_keep_the_parse_error_reset_boundary() {
    use std::panic::{catch_unwind, AssertUnwindSafe};
    let source = SourceText::default();
    let factory = AstBuilder::new(source.clone(), &Counters::new());
    let mut parser = Parser::new(
        SourceFileParseOptions::default(),
        &source,
        ScriptKind::TS,
        factory,
    );
    let malformed = parser.factory.new_node(
        SyntaxKind::ParenthesizedExpression.into(),
        ts_ast::TokenData {}.into(),
    );
    parser.has_parse_error = true;
    let pos = i64::from(i32::MAX) + 1;
    let failed_parent = catch_unwind(AssertUnwindSafe(|| {
        parser.finish_node_with_end(malformed, pos, -2)
    }));
    assert!(failed_parent.is_err());
    assert!(!parser.has_parse_error);
    let read = parser.factory.node(malformed);
    assert_eq!(read.range(), TextRange::new(i64::from(i32::MIN), -2));
    assert_ne!(read.flags() & node_flags::THIS_NODE_HAS_ERROR, 0);
    let invalid = ts_arena::NodeId::from_parts(malformed.arena(), u32::MAX).unwrap();
    parser.has_parse_error = true;
    assert!(catch_unwind(AssertUnwindSafe(
        || parser.finish_node_with_end(invalid, 1, 2)
    ))
    .is_err());
    assert!(parser.has_parse_error);
}

#[test]
fn source_debug_assertions_keep_contract_messages_instead_of_rust_assert_formatting() {
    use std::panic::{catch_unwind, AssertUnwindSafe};
    fn message(failure: Box<dyn std::any::Any + Send>) -> String {
        match failure.downcast::<String>() {
            Ok(text) => *text,
            Err(failure) => failure.downcast::<&str>().unwrap().to_string(),
        }
    }
    let source = SourceText::default();
    let factory = AstBuilder::new(source.clone(), &Counters::default());
    let mut parser = Parser::new(
        SourceFileParseOptions::default(),
        &source,
        ScriptKind::TSX,
        factory,
    );
    assert_eq!(
        message(
            catch_unwind(AssertUnwindSafe(|| parser.is_in_some_parsing_context())).unwrap_err()
        ),
        "Debug failure. False expression: Missing parsing context"
    );
    assert_eq!(
        message(catch_unwind(AssertUnwindSafe(|| parser.parse_type_assertion())).unwrap_err()),
        "Debug failure. False expression: Type assertions should never be parsed in JSX; they should be parsed as comparisons or JSX elements/fragments."
    );
    assert_eq!(
        message(
            catch_unwind(AssertUnwindSafe(
                || parser.next_is_parenthesized_arrow_function_expression()
            ))
            .unwrap_err()
        ),
        "Debug failure. False expression."
    );
    let identifier = parser.factory.new_identifier(JsString::default());
    assert_eq!(
        message(
            catch_unwind(AssertUnwindSafe(|| parser
                .parse_simple_arrow_function_expression(
                    0, identifier, true, 0, None
                )))
            .unwrap_err()
        ),
        "Debug failure. False expression: parseSimpleArrowFunctionExpression should only have been called if we had a =>"
    );
}

#[test]
fn unspecified_script_kind_keeps_malformed_filename_bytes_in_panic_payload() {
    let failure = std::panic::catch_unwind(|| {
        crate::parse_source_file(
            SourceText::default(),
            ScriptKind::UNKNOWN,
            SourceFileParseOptions {
                file_name: JsString::from_bytes(vec![b'/', 0xff]),
                ..SourceFileParseOptions::default()
            },
        )
    })
    .unwrap_err();
    let message = failure.downcast::<Vec<u8>>().unwrap();
    assert_eq!(
        message.as_slice(),
        b"ScriptKind must be specified when parsing source file: /\xff"
    );
}
