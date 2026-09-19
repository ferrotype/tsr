use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::{Arc, Mutex};

use ts_ast::{token_flags as flags, CommentDirectiveKind, SyntaxKind};
use ts_core::LanguageVariant;
use ts_scanner::Scanner;

#[test]
fn checkpoints_restore_values_and_directives_across_text_replacement() {
    let text = b"// @ts-ignore\nfirst // @ts-expect-error\nsecond";
    let mut scanner = Scanner::new();
    scanner.set_text(text);
    assert_eq!(scanner.scan(), SyntaxKind::Identifier);
    let outer = scanner.mark();
    assert_eq!(scanner.scan(), SyntaxKind::Identifier);
    assert_eq!(scanner.comment_directives().len(), 2);
    let inner = scanner.mark();
    scanner.set_text(b"'\\u0061'");
    scanner.scan();
    assert_eq!(scanner.token_value(), b"a");
    scanner.set_text(text);
    scanner.rewind(inner);
    assert_eq!(scanner.token_value(), b"second");
    assert_eq!(scanner.comment_directives().len(), 2);
    scanner.rewind(outer);
    assert_eq!(scanner.token_value(), b"first");
    assert_eq!(scanner.comment_directives().len(), 1);
    scanner.scan();
    assert_eq!(
        scanner.comment_directives()[1].kind,
        CommentDirectiveKind::ExpectError
    );
}

#[test]
fn reset_keeps_checkpoint_valid_but_does_not_restore_configuration_or_text() {
    let mut scanner = Scanner::new();
    scanner.set_text(b"'\\u0061'");
    scanner.set_skip_trivia(false);
    scanner.set_language_variant(LanguageVariant::JSX);
    scanner.scan();
    let checkpoint = scanner.mark();
    scanner.reset();
    scanner.rewind(checkpoint);
    assert!(scanner.text().is_empty());
    assert_eq!(scanner.token_value(), b"a");
    assert_eq!(scanner.token(), SyntaxKind::StringLiteral);
    assert!(catch_unwind(AssertUnwindSafe(|| scanner.token_text())).is_err());
    scanner.set_text(b" \t</");
    assert_eq!(scanner.scan(), SyntaxKind::LessThanToken);
}

#[test]
fn cooked_values_survive_nested_commit_and_rewind() {
    let mut scanner = Scanner::new();
    scanner.set_text(b"'\\u0061' '\\u0062' '\\u0063'");
    scanner.scan();
    let outer = scanner.mark();
    scanner.scan();
    let inner = scanner.mark();
    scanner.scan();
    scanner.commit(inner);
    assert_eq!(scanner.token_value(), b"c");
    scanner.rewind(outer);
    assert_eq!(scanner.token_value(), b"a");
}

#[test]
fn diagnostics_are_synchronous_and_not_checkpoint_state() {
    let errors = Arc::new(Mutex::new(Vec::new()));
    let target = Arc::clone(&errors);
    let mut scanner = Scanner::new();
    scanner.set_text(b"'unterminated\n");
    scanner.set_on_error(Some(Box::new(move |error| {
        target.lock().unwrap().push(error);
    })));
    let checkpoint = scanner.mark();
    scanner.scan();
    assert_eq!(errors.lock().unwrap().len(), 1);
    scanner.rewind(checkpoint);
    assert_eq!(errors.lock().unwrap().len(), 1);
    scanner.scan();
    assert_eq!(errors.lock().unwrap().len(), 2);
    scanner.reset();
    scanner.set_text(b"'unterminated\n");
    scanner.scan();
    assert_eq!(errors.lock().unwrap().len(), 2);
}

#[test]
fn punctuation_keeps_value_and_negative_jsdoc_counter_still_skips_one_star() {
    let mut scanner = Scanner::new();
    scanner.set_text(b"name +");
    scanner.scan();
    scanner.scan();
    assert_eq!(scanner.token(), SyntaxKind::PlusToken);
    assert_eq!(scanner.token_value(), b"name");
    scanner.scan();
    assert_eq!(scanner.token_value(), b"name");
    scanner.set_text(b"\n*x");
    scanner.set_skip_jsdoc_leading_asterisks(false);
    assert_eq!(scanner.scan(), SyntaxKind::Identifier);
    assert_eq!(scanner.token_value(), b"x");
    assert_ne!(
        scanner.token_flags() & flags::PRECEDING_JSDOC_LEADING_ASTERISKS,
        0
    );
}

#[test]
fn reset_position_beyond_end_succeeds_until_a_bounds_sensitive_accessor() {
    let mut scanner = Scanner::new();
    scanner.set_text(b"a");
    scanner.reset_pos(2);
    assert_eq!(scanner.token_end(), 2);
    assert_eq!(scanner.scan(), SyntaxKind::EndOfFile);
    assert!(catch_unwind(AssertUnwindSafe(|| scanner.token_text())).is_err());
}

#[test]
fn scanner_and_checkpoint_are_send() {
    fn assert_send<T: Send>() {}
    assert_send::<Scanner<'static>>();
    assert_send::<ts_scanner::Checkpoint<'static>>();
}

#[test]
#[should_panic(expected = "checkpoint belongs to another scanner")]
fn foreign_checkpoint_is_rust_api_misuse() {
    let mut first = Scanner::new();
    let mut second = Scanner::new();
    let checkpoint = first.mark();
    second.rewind(checkpoint);
}

#[test]
#[should_panic(expected = "checkpoints must be consumed in LIFO order")]
fn out_of_order_checkpoint_is_rust_api_misuse() {
    let mut scanner = Scanner::new();
    let first = scanner.mark();
    let _second = scanner.mark();
    scanner.rewind(first);
}

#[test]
fn abandoning_a_checkpoint_during_unwind_requires_discarding_the_scanner() {
    let mut scanner = Scanner::new();
    let outer = scanner.mark();
    let interrupted = catch_unwind(AssertUnwindSafe(|| {
        let _inner = scanner.mark();
        panic!("interrupted speculative parse");
    }));
    assert!(interrupted.is_err());
    // Reset cannot invalidate abandoned handles while preserving valid saved ones.
    scanner.reset();
    let result = catch_unwind(AssertUnwindSafe(|| scanner.rewind(outer)));
    let payload = result.unwrap_err();
    let message = payload
        .downcast_ref::<String>()
        .map(String::as_str)
        .or_else(|| payload.downcast_ref::<&str>().copied())
        .unwrap();
    assert!(message.contains("checkpoints must be consumed in LIFO order"));
    drop(scanner);
}

#[test]
fn buffered_delivery_matches_callbacks_and_survives_rewind() {
    let text = b"'unterminated\n 0x '\\u{110000}'";
    let observed = Arc::new(Mutex::new(Vec::new()));
    let destination = Arc::clone(&observed);
    let mut callback = Scanner::new();
    callback.set_text(text);
    callback.set_on_error(Some(Box::new(move |error| {
        destination.lock().unwrap().push(error);
    })));
    let mut buffered = Scanner::new();
    buffered.set_text(text);
    buffered.buffer_diagnostics();
    let saved_callback = callback.mark();
    let saved_buffered = buffered.mark();
    assert_eq!(callback.scan(), buffered.scan());
    callback.rewind(saved_callback);
    buffered.rewind(saved_buffered);
    // Emitted diagnostics remain outside both checkpoint modes.
    assert_eq!(
        buffered.drain_diagnostics().collect::<Vec<_>>(),
        *observed.lock().unwrap()
    );
    observed.lock().unwrap().clear();
    loop {
        let token = callback.scan();
        assert_eq!(token, buffered.scan());
        assert_eq!(
            buffered.drain_diagnostics().collect::<Vec<_>>(),
            *observed.lock().unwrap()
        );
        observed.lock().unwrap().clear();
        if token == SyntaxKind::EndOfFile {
            break;
        }
    }
    buffered.reset();
    buffered.set_text(b"'unterminated");
    buffered.scan();
    assert_eq!(buffered.drain_diagnostics().count(), 0);
}

#[test]
fn retained_values_share_source_and_cooked_bytes_across_advancement() {
    use ts_jsstring::SourceText;
    let source = SourceText::from_loaded_bytes(&b"first '\\u0061' # .5"[..]);
    let mut scanner = Scanner::new();
    scanner.set_text(source.as_bytes());
    scanner.scan();
    let identifier = scanner.retain_token_value();
    let source_pointer = identifier.as_bytes().as_ptr();
    scanner.scan();
    let cooked = scanner.retain_token_value();
    let cooked_pointer = cooked.as_bytes().as_ptr();
    scanner.scan();
    let hash = scanner.retain_token_value();
    scanner.scan();
    let fraction = scanner.retain_token_value();
    scanner.set_text(b"unrelated");
    scanner.scan();
    drop(scanner);
    let identifier = identifier.into_js_string(&source).unwrap();
    let cooked = cooked.into_js_string(&source).unwrap();
    assert_eq!(identifier.as_bytes(), b"first");
    assert_eq!(identifier.as_bytes().as_ptr(), source_pointer);
    assert_eq!(cooked.as_bytes(), b"a");
    assert_eq!(cooked.as_bytes().as_ptr(), cooked_pointer);
    assert_eq!(hash.into_js_string(&source).unwrap().as_bytes(), b"#");
    assert_eq!(fraction.into_js_string(&source).unwrap().as_bytes(), b"0.5");
    drop(source);
    assert_eq!(identifier.as_bytes(), b"first");
    assert_eq!(cooked.as_bytes(), b"a");
}

#[test]
fn a_retained_source_slice_does_not_import_an_equal_foreign_allocation() {
    use ts_jsstring::SourceText;
    let first = SourceText::from_loaded_bytes(&b"name"[..]);
    let second = SourceText::from_loaded_bytes(&b"name"[..]);
    let mut scanner = Scanner::new();
    scanner.set_text(first.as_bytes());
    scanner.scan();
    assert!(scanner
        .retain_token_value()
        .into_js_string(&second)
        .is_none());
}
