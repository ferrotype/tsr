use std::panic::{catch_unwind, AssertUnwindSafe};
use ts_arena::Counters;
use ts_ast::{AstBuilder, JsString, SourceFileParseOptions};
use ts_jsstring::SourceText;

#[test]
fn invalid_source_file_names_keep_go_quoted_panic_payloads() {
    let cases: &[(&[u8], &str)] = &[
        (
            b"relative.ts",
            "fileName should be normalized and absolute: \"relative.ts\"",
        ),
        (
            b"a\0\xff\xed\xa0\x80",
            "fileName should be normalized and absolute: \"a\\x00\\xff\\xed\\xa0\\x80\"",
        ),
        (
            "é\u{a0}\u{200b}\u{1e6c0}".as_bytes(),
            "fileName should be normalized and absolute: \"é\\u00a0\\u200b\u{1e6c0}\"",
        ),
    ];
    for &(name, expected) in cases {
        let mut factory = AstBuilder::new(SourceText::default(), &Counters::new());
        let result = catch_unwind(AssertUnwindSafe(|| {
            factory.new_source_file(
                SourceFileParseOptions {
                    file_name: JsString::from_bytes(name),
                    ..SourceFileParseOptions::default()
                },
                SourceText::default(),
                None,
                None,
            );
        }));
        let payload = result.expect_err("relative filenames are rejected by pinned NewSourceFile");
        assert_eq!(payload.downcast_ref::<String>().unwrap(), expected);
    }
}
