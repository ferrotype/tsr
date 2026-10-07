//! Ports the diagnostic response cases of tsc/internal/api/proto_test.go.
use super::responses::diagnostic_response;
use tsr_ast::{AstFile, Diagnostic, SourceFileParseOptions};
use tsr_core::TextRange;

fn parse(name: &str, text: &str) -> AstFile {
    tsr_parser::parse_source_file(
        tsr_jsstring::SourceText::from_loaded_bytes(text.as_bytes().to_vec()),
        tsr_core::ScriptKind::TS,
        SourceFileParseOptions {
            file_name: tsr_ast::JsString::from_bytes(name.as_bytes()),
            path: tsr_ast::JsString::from_bytes(name.as_bytes()),
            ..Default::default()
        },
    )
    .publish_unbound()
}

/// Start and end as (line, character), then the source lines.
type Positions = ((i64, i64), (i64, i64), Vec<(i64, String)>);

fn positions(response: &crate::proto::DiagnosticResponse) -> Positions {
    let start = response
        .start_position
        .as_deref()
        .expect("a start position");
    let end = response.end_position.as_deref().expect("an end position");
    (
        (start.line, start.character.0),
        (end.line, end.character.0),
        response
            .source_lines
            .iter()
            .flatten()
            .map(|line| (line.line, line.text.clone()))
            .collect(),
    )
}

/// Ports `TestNewDiagnosticResponseIncludesFormattingContext`: positions are
/// UTF-16 offsets, with line, character and the source line.
#[test]
fn diagnostic_response_includes_formatting_context() {
    let text = "const 💩 = 1;";
    let file = parse("/unicode.ts", text);
    let pos = i64::try_from(text.find('=').unwrap()).unwrap();
    let diagnostic = Diagnostic::new(
        file.root(),
        TextRange::new(pos, pos + 1),
        tsr_diagnostics::Expression_expected,
        Vec::new(),
    );
    let response = diagnostic_response(&diagnostic, &|_| Some(file.view()));
    assert_eq!((response.pos, response.end), (9, 10));
    assert_eq!(response.file_name, "/unicode.ts");
    assert_eq!(response.text, "Expression expected.");
    assert_eq!(
        positions(&response),
        ((0, 9), (0, 10), vec![(0, text.to_string())])
    );
}

/// Ports `TestNewDiagnosticResponseTruncatesLongFormattingContext`: a span
/// over more than four lines keeps its first two and last two.
#[test]
fn diagnostic_response_truncates_long_formatting_context() {
    let text = "one\ntwo\nthree\nfour\nfive\nsix\nseven";
    let file = parse("/multiline.ts", text);
    let diagnostic = Diagnostic::new(
        file.root(),
        TextRange::new(0, i64::try_from(text.len()).unwrap()),
        tsr_diagnostics::Expression_expected,
        Vec::new(),
    );
    let response = diagnostic_response(&diagnostic, &|_| Some(file.view()));
    assert_eq!(
        positions(&response),
        (
            (0, 0),
            (6, 5),
            vec![
                (0, "one\n".to_string()),
                (1, "two\n".to_string()),
                (5, "six\n".to_string()),
                (6, "seven".to_string()),
            ]
        )
    );
}

/// `base64_decode` follows Go's `StdEncoding.DecodeString`, which the pin's
/// `printNode` and `formatNodeForInsertion` call: newlines are skipped,
/// padding may only end the input, and the error names the offending byte.
#[test]
fn base64_follows_go_std_encoding() {
    use super::responses::base64_decode;
    assert_eq!(base64_decode("YWJj"), Ok(b"abc".to_vec()));
    assert_eq!(base64_decode("YW\r\nJj"), Ok(b"abc".to_vec()));
    assert_eq!(base64_decode("YQ==\r\n"), Ok(b"a".to_vec()));
    assert_eq!(base64_decode("YWI="), Ok(b"ab".to_vec()));
    assert_eq!(base64_decode(""), Ok(Vec::new()));
    assert_eq!(base64_decode("YQ==Yg=="), Err(4), "data after padding");
    assert_eq!(base64_decode("YQ"), Err(0), "missing padding");
    assert_eq!(base64_decode("Y!Jj"), Err(1), "an illegal byte");
    assert_eq!(base64_decode("YQ=Y"), Err(2), "incomplete padding");
}
