use std::panic::{catch_unwind, AssertUnwindSafe};

use ts_ast::SyntaxKind;
use ts_core::ScriptTarget;

use crate::{Scanner, ScannerDiagnostic};

fn observe(source: &[u8], target: ScriptTarget, report: bool) -> Vec<ScannerDiagnostic> {
    let mut diagnostics = Vec::new();
    {
        let mut scanner = Scanner::new();
        scanner.set_text(source);
        scanner.set_script_target(target);
        scanner.set_on_error(Some(Box::new(|diagnostic| diagnostics.push(diagnostic))));
        assert_eq!(scanner.scan(), SyntaxKind::SlashToken);
        assert_eq!(
            scanner.rescan_slash_token(report),
            SyntaxKind::RegularExpressionLiteral
        );
        assert_eq!(scanner.token_end(), source.len() as i64);
        assert_eq!(scanner.token_value(), source);
    }
    diagnostics
}

#[test]
fn emission_order_and_simple_fold_match_pinned_go() {
    // These tuples were independently observed through the pinned Go scanner.
    let diagnostics = observe(br"/\k<missing>*\9(?<present>a)/z", ScriptTarget::NONE, true);
    assert_eq!(
        diagnostics
            .iter()
            .map(|d| (d.message.code, d.start, d.length))
            .collect::<Vec<_>>(),
        [(1499, 29, 1), (1532, 4, 7), (1533, 14, 1)]
    );
    let dotted_i = observe("/\\k<İ>(?<i>a)/u".as_bytes(), ScriptTarget::NONE, true);
    assert_eq!(
        dotted_i.iter().map(|d| d.message.code).collect::<Vec<_>>(),
        [1532]
    );
    let kelvin = observe(
        "/\\k<\u{212a}>(?<k>a)/u".as_bytes(),
        ScriptTarget::NONE,
        true,
    );
    assert_eq!(
        kelvin.iter().map(|d| d.message.code).collect::<Vec<_>>(),
        [1532, 1369]
    );
    assert_eq!(kelvin[1].args, vec!["k".into()]);
}

#[test]
fn unicode_sets_track_strings_through_each_operator() {
    let cases: &[(&[u8], &[i32])] = &[
        (br"/[\q{a|b}]/v", &[]),
        (br"/[^\q{ab}&&a]/v", &[]),
        (br"/[^\q{ab}--a]/v", &[1518]),
        (br"/[[]/v", &[1005]),
        ("/[😀-a]/".as_bytes(), &[1517]),
        ("/[a-😀]/".as_bytes(), &[]),
        ("/[😀-a]/u".as_bytes(), &[1517]),
    ];
    for &(source, expected) in cases {
        assert_eq!(
            observe(source, ScriptTarget::NONE, true)
                .iter()
                .map(|d| d.message.code)
                .collect::<Vec<_>>(),
            expected,
            "source bytes {source:?}"
        );
    }
}

#[test]
fn false_reporting_skips_grammar_but_keeps_unterminated_diagnostic() {
    assert!(observe(
        br"/\k<missing>*\9(?<present>a)/z",
        ScriptTarget::NONE,
        false
    )
    .is_empty());
    let errors = observe(b"/unterminated", ScriptTarget::NONE, false);
    assert_eq!(errors.len(), 1);
    assert_eq!(
        errors[0].message,
        ts_diagnostics::Unterminated_regular_expression_literal
    );
}

fn panic_message(panic: &(dyn std::any::Any + Send)) -> &str {
    if let Some(value) = panic.downcast_ref::<&str>() {
        value
    } else {
        panic
            .downcast_ref::<String>()
            .expect("string panic payload")
    }
}

#[test]
fn escaped_line_separator_keeps_upstream_assertion_in_release() {
    for source in ["/\\\u{2028}/", "/\\\u{2028}/u", "/\\\u{2029}/v"] {
        let mut scanner = Scanner::new();
        scanner.set_text(source.as_bytes());
        assert_eq!(scanner.scan(), SyntaxKind::SlashToken);
        let panic = catch_unwind(AssertUnwindSafe(|| scanner.rescan_slash_token(true)))
            .expect_err("pinned Go contract panic");
        assert_eq!(
            panic_message(panic.as_ref()),
            "Debug failure. False expression."
        );
        // Go leaves temporary regexp state after this panic; Reset is the reuse boundary.
        scanner.reset();
        scanner.set_text(b"next");
        assert_eq!(scanner.scan(), SyntaxKind::Identifier);
    }
}

#[cfg(not(miri))]
#[test]
fn production_guards_support_deep_groups_and_unicode_sets() {
    std::thread::Builder::new()
        .stack_size(256 * 1024)
        .spawn(|| {
            const DEPTH: usize = 20_000;
            let cases = [
                (format!("/{}a{}/", "(".repeat(DEPTH), ")".repeat(DEPTH)), 0),
                (
                    format!("/{}a{}/", "(?=".repeat(DEPTH), ")".repeat(DEPTH)),
                    0,
                ),
                (format!("/{}a{}/v", "[".repeat(DEPTH), "]".repeat(DEPTH)), 0),
                (
                    format!("/{}a{}]/v", "[".repeat(DEPTH), "]&&a".repeat(DEPTH - 1)),
                    0,
                ),
                (
                    format!("/{}a{}]/v", "[".repeat(DEPTH), "]--a".repeat(DEPTH - 1)),
                    0,
                ),
                (format!("/{}a/", "(".repeat(DEPTH)), DEPTH),
                (format!("/{}a]/v", "[".repeat(DEPTH)), DEPTH - 1),
                (
                    format!(
                        "/{}{}a{}{}/v",
                        "(".repeat(DEPTH / 2),
                        "[".repeat(DEPTH / 2),
                        "]".repeat(DEPTH / 2),
                        ")".repeat(DEPTH / 2)
                    ),
                    0,
                ),
            ];
            for (source, expected_errors) in cases {
                let diagnostics = observe(source.as_bytes(), ScriptTarget::NONE, true);
                assert_eq!(diagnostics.len(), expected_errors);
                assert!(diagnostics.iter().all(|d| d.message.code == 1005));
            }
        })
        .expect("create modest-stack worker")
        .join()
        .expect("guarded regexp scan");
}

#[cfg(not(miri))]
#[test]
fn diagnostic_panic_unwinds_across_grown_segments_on_the_same_thread() {
    std::thread::Builder::new()
        .stack_size(256 * 1024)
        .spawn(|| {
            let source = format!("/{}a/", "(".repeat(20_000));
            let thread = std::thread::current().id();
            let mut scanner = Scanner::new();
            scanner.set_text(source.as_bytes());
            scanner.set_on_error(Some(Box::new(move |_| {
                assert_eq!(std::thread::current().id(), thread);
                panic!("regexp callback unwind witness");
            })));
            scanner.scan();
            let panic = catch_unwind(AssertUnwindSafe(|| scanner.rescan_slash_token(true)))
                .expect_err("callback must unwind");
            assert_eq!(
                panic_message(panic.as_ref()),
                "regexp callback unwind witness"
            );
            scanner.reset();
            scanner.set_text(b"after");
            assert_eq!(scanner.scan(), SyntaxKind::Identifier);
        })
        .expect("create modest-stack worker")
        .join()
        .expect("callback panic handled at test boundary");
}
