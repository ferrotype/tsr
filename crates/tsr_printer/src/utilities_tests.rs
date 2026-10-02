//! `tsc/internal/printer/utilities_test.go`. The escaping helpers live in
//! `tsr_jsstring::escape`; the triple-slash recognizer is this crate's.

use crate::utilities::is_recognized_triple_slash_comment;
use tsr_ast::SyntaxKind as K;
use tsr_core::TextRange;
use tsr_jsstring::escape::{escape_jsx_attribute_string, escape_non_ascii_string, escape_string};
use tsr_jsstring::QuoteChar;
use tsr_scanner::CommentRange;

#[test]
fn test_escape_string() {
    let data: [(&str, QuoteChar, &str); 10] = [
        ("", QuoteChar::Double, ""),
        ("abc", QuoteChar::Double, "abc"),
        ("ab\"c", QuoteChar::Double, r#"ab\"c"#),
        ("ab\tc", QuoteChar::Double, r"ab\tc"),
        ("ab\nc", QuoteChar::Double, r"ab\nc"),
        ("ab'c", QuoteChar::Double, "ab'c"),
        ("ab'c", QuoteChar::Single, r"ab\'c"),
        ("ab\"c", QuoteChar::Single, "ab\"c"),
        ("ab`c", QuoteChar::Backtick, "ab\\`c"),
        ("\u{001f}", QuoteChar::Backtick, "\\u001F"),
    ];
    for (index, (s, quote_char, expected)) in data.into_iter().enumerate() {
        assert_eq!(
            escape_string(s.as_bytes(), quote_char),
            expected.as_bytes(),
            "[{index}] escapeString({s:?}, {quote_char:?})"
        );
    }
}

#[test]
fn test_escape_non_ascii_string() {
    let data: [(&str, QuoteChar, &str); 11] = [
        ("", QuoteChar::Double, ""),
        ("abc", QuoteChar::Double, "abc"),
        ("ab\"c", QuoteChar::Double, r#"ab\"c"#),
        ("ab\tc", QuoteChar::Double, r"ab\tc"),
        ("ab\nc", QuoteChar::Double, r"ab\nc"),
        ("ab'c", QuoteChar::Double, "ab'c"),
        ("ab'c", QuoteChar::Single, r"ab\'c"),
        ("ab\"c", QuoteChar::Single, "ab\"c"),
        ("ab`c", QuoteChar::Backtick, "ab\\`c"),
        ("ab\u{008f}c", QuoteChar::Double, r"ab\u008Fc"),
        ("𝟘𝟙", QuoteChar::Double, r"\uD835\uDFD8\uD835\uDFD9"),
    ];
    for (index, (s, quote_char, expected)) in data.into_iter().enumerate() {
        assert_eq!(
            escape_non_ascii_string(s.as_bytes(), quote_char),
            expected.as_bytes(),
            "[{index}] escapeNonAsciiString({s:?}, {quote_char:?})"
        );
    }
}

#[test]
fn test_escape_jsx_attribute_string() {
    let data: [(&str, QuoteChar, &str); 10] = [
        ("", QuoteChar::Double, ""),
        ("abc", QuoteChar::Double, "abc"),
        ("ab\"c", QuoteChar::Double, "ab&quot;c"),
        ("ab\tc", QuoteChar::Double, "ab&#x9;c"),
        ("ab\nc", QuoteChar::Double, "ab&#xA;c"),
        ("ab'c", QuoteChar::Double, "ab'c"),
        ("ab'c", QuoteChar::Single, "ab&apos;c"),
        ("ab\"c", QuoteChar::Single, "ab\"c"),
        ("ab\u{008f}c", QuoteChar::Double, "ab\u{008F}c"),
        ("𝟘𝟙", QuoteChar::Double, "𝟘𝟙"),
    ];
    for (index, (s, quote_char, expected)) in data.into_iter().enumerate() {
        assert_eq!(
            escape_jsx_attribute_string(s.as_bytes(), quote_char),
            expected.as_bytes(),
            "[{index}] escapeJsxAttributeString({s:?}, {quote_char:?})"
        );
    }
}

#[test]
fn test_is_recognized_triple_slash_comment() {
    // A row without a comment range is a single-line comment over all of `s`,
    // as upstream fills in a zero range of unknown kind.
    let data: [(&str, Option<K>, bool); 39] = [
        ("", Some(K::MultiLineCommentTrivia), false),
        ("", Some(K::SingleLineCommentTrivia), false),
        ("/a", None, false),
        ("//", None, false),
        ("//a", None, false),
        ("///", None, false),
        ("///a", None, false),
        ("///<reference path=\"foo\" />", None, true),
        ("///<reference types=\"foo\" />", None, true),
        ("///<reference lib=\"foo\" />", None, true),
        ("///<reference no-default-lib=\"foo\" />", None, true),
        ("///<amd-dependency path=\"foo\" />", None, true),
        ("///<amd-module />", None, true),
        ("/// <reference path=\"foo\" />", None, true),
        ("/// <reference types=\"foo\" />", None, true),
        ("/// <reference lib=\"foo\" />", None, true),
        ("/// <reference no-default-lib=\"foo\" />", None, true),
        ("/// <amd-dependency path=\"foo\" />", None, true),
        ("/// <amd-module />", None, true),
        ("/// <reference path=\"foo\"/>", None, true),
        ("/// <reference types=\"foo\"/>", None, true),
        ("/// <reference lib=\"foo\"/>", None, true),
        ("/// <reference no-default-lib=\"foo\"/>", None, true),
        ("/// <amd-dependency path=\"foo\"/>", None, true),
        ("/// <amd-module/>", None, true),
        ("/// <reference path='foo' />", None, true),
        ("/// <reference types='foo' />", None, true),
        ("/// <reference lib='foo' />", None, true),
        ("/// <reference no-default-lib='foo' />", None, true),
        ("/// <amd-dependency path='foo' />", None, true),
        ("/// <reference path=\"foo\" />  ", None, true),
        ("/// <reference types=\"foo\" />  ", None, true),
        ("/// <reference lib=\"foo\" />  ", None, true),
        ("/// <reference no-default-lib=\"foo\" />  ", None, true),
        ("/// <amd-dependency path=\"foo\" />  ", None, true),
        ("/// <amd-module />  ", None, true),
        ("/// <foo />", None, false),
        ("/// <reference />", None, false),
        ("/// <amd-dependency />", None, false),
    ];
    for (index, (s, kind, expected)) in data.into_iter().enumerate() {
        let comment_range = match kind {
            Some(kind) => CommentRange {
                loc: TextRange::new(0, 0),
                kind,
                has_trailing_new_line: false,
            },
            None => CommentRange {
                loc: TextRange::new(0, s.len() as i64),
                kind: K::SingleLineCommentTrivia,
                has_trailing_new_line: false,
            },
        };
        assert_eq!(
            is_recognized_triple_slash_comment(s.as_bytes(), &comment_range),
            expected,
            "[{index}] isRecognizedTripleSlashComment({s:?})"
        );
    }
}
