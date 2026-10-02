//! The pinned package has no tests for source_mapper.go, lineinfo.go or
//! util.go. Every expectation here was derived by reading the pinned Go.
use std::collections::BTreeMap;
use std::sync::Arc;

use tsr_jsstring::line_map::compute_ecma_line_starts;
use tsr_jsstring::JsString;

use crate::source_mapper::try_parse_base64_url;
use crate::{
    create_ecma_line_info, get_document_position_mapper, new_generator, try_get_source_mapping_url,
    DocumentPosition, EcmaLineInfo, Generator, Host,
};

fn js(text: &str) -> JsString {
    JsString::from_bytes(text.as_bytes())
}

fn line_info(text: &str) -> EcmaLineInfo {
    create_ecma_line_info(js(text), compute_ecma_line_starts(text.as_bytes()))
}

#[derive(Default)]
struct TestHost {
    case_sensitive: bool,
    files: BTreeMap<Vec<u8>, JsString>,
}

impl TestHost {
    fn with(mut self, name: &str, text: &str) -> Self {
        self.files.insert(name.as_bytes().to_vec(), js(text));
        self
    }
}

impl Host for TestHost {
    fn use_case_sensitive_file_names(&self) -> bool {
        self.case_sensitive
    }
    fn get_ecma_line_info(&self, file_name: &[u8]) -> Option<Arc<EcmaLineInfo>> {
        self.files.get(file_name).map(|text| {
            Arc::new(create_ecma_line_info(
                text.clone(),
                compute_ecma_line_starts(text.as_bytes()),
            ))
        })
    }
    fn read_file(&self, file_name: &[u8]) -> Option<JsString> {
        self.files.get(file_name).cloned()
    }
}

const GENERATED: &str = "let x = 1;\n";
const SOURCE: &str = "let x: number = 1;\n";

/// `let x = 1;` mapped onto `let x: number = 1;`: generated columns 0, 4 and
/// 6 map to source columns 0, 4 and 14.
fn generator() -> Generator {
    let mut gen = new_generator(js("main.js"), js(""), js("/out"), js(""), true);
    let source_index = gen.add_source(js("/src/main.ts"));
    gen.add_source_mapping(0, 0, source_index, 0, 0).unwrap();
    gen.add_source_mapping(0, 4, source_index, 0, 4).unwrap();
    gen.add_source_mapping(0, 6, source_index, 0, 14).unwrap();
    gen
}

fn at(file_name: &str, pos: isize) -> DocumentPosition {
    DocumentPosition {
        file_name: js(file_name),
        pos,
    }
}

#[test]
fn line_info_line_text_includes_terminators() {
    let info = line_info("a\r\nb\u{2028}c");
    assert_eq!(info.line_count(), 3);
    assert_eq!(info.line_text(0), b"a\r\n");
    assert_eq!(info.line_text(1), "b\u{2028}".as_bytes());
    assert_eq!(info.line_text(2), b"c");
}

#[test]
#[should_panic(expected = "index out of bounds")]
fn line_info_line_text_panics_out_of_range() {
    let _ = line_info("a").line_text(1);
}

#[test]
fn source_mapping_url_is_found_from_the_end() {
    let cases = [
        // Blank lines and other `//#` comments are skipped.
        (
            "a\n//# sourceMappingURL=first.map\n//# debugId=1\n\n",
            "first.map",
        ),
        // A trailing line separator and spaces are trimmed.
        ("  //@ sourceMappingURL=b.map \u{2028}", "b.map"),
        // Any other last line stops the search.
        ("//# sourceMappingURL=a.map\nconsole.log(1)\n", ""),
        ("//#sourceMappingURL=c.map", ""),
        ("//", ""),
        ("", ""),
    ];
    for (text, expected) in cases {
        let info = line_info(text);
        assert_eq!(
            try_get_source_mapping_url(Some(&info)),
            expected.as_bytes(),
            "{text:?}"
        );
    }
    assert_eq!(try_get_source_mapping_url(None), b"");
}

#[test]
fn base64_url_parsing() {
    let cases: [(&str, &str, bool); 8] = [
        ("x.map", "", false),
        ("data:text/plain;base64,AAAA", "", true),
        ("data:application/json;base64,eyJ9", "eyJ9", true),
        (
            "data:application/json;charset=UTF-8;base64,eyJ9",
            "eyJ9",
            true,
        ),
        ("data:application/json;charset=latin1;base64,eyJ9", "", true),
        ("data:application/json;base64,ey J9", "", true),
        ("data:application/json;base64,", "", true),
        ("data:application/json;utf-8,eyJ9", "", true),
    ];
    for (url, expected, matched) in cases {
        assert_eq!(
            try_parse_base64_url(url.as_bytes()),
            (expected.as_bytes(), matched),
            "{url}"
        );
    }
}

#[test]
#[should_panic(expected = "range end index")]
fn base64_url_with_a_short_charset_panics() {
    // The pinned `url[:len("utf-8;")]` slices past the end.
    let _ = try_parse_base64_url(b"data:application/json;charset=utf");
}

#[test]
fn maps_positions_through_a_map_file() {
    let map = generator().string();
    assert_eq!(
        map.as_bytes(),
        br#"{"version":3,"file":"main.js","sourceRoot":"","sources":["../src/main.ts"],"names":[],"mappings":"AAAA,IAAI,EAAU"}"#
    );
    let host = TestHost {
        case_sensitive: true,
        ..TestHost::default()
    }
    .with(
        "/out/main.js",
        &format!("{GENERATED}//# sourceMappingURL=main.js.map\n"),
    )
    .with("/src/main.ts", SOURCE)
    .with("/out/main.js.map", map.as_str().unwrap());
    let mapper = get_document_position_mapper(&host, b"/out/main.js").expect("mapper");

    // The first mapping at or after the position wins.
    assert_eq!(
        mapper.get_source_position(&at("/out/main.js", 4)),
        Some(at("/src/main.ts", 4))
    );
    assert_eq!(
        mapper.get_source_position(&at("/out/main.js", 5)),
        Some(at("/src/main.ts", 14))
    );
    assert_eq!(mapper.get_source_position(&at("/out/main.js", 7)), None);
    assert_eq!(
        mapper.get_generated_position(&at("/src/main.ts", 10)),
        Some(at("/out/main.js", 6))
    );
    assert_eq!(
        mapper.get_generated_position(&at("/src/main.ts", 0)),
        Some(at("/out/main.js", 0))
    );
    assert_eq!(mapper.get_generated_position(&at("/src/main.ts", 15)), None);
    // Case-sensitive host: a different spelling is another file.
    assert_eq!(mapper.get_generated_position(&at("/SRC/main.ts", 0)), None);
}

#[test]
fn falls_back_to_the_dot_map_file_and_folds_case() {
    let map = generator().string();
    let host = TestHost {
        case_sensitive: false,
        ..TestHost::default()
    }
    .with("/out/main.js", GENERATED)
    .with("/src/main.ts", SOURCE)
    .with("/out/main.js.map", map.as_str().unwrap());
    let mapper = get_document_position_mapper(&host, b"/out/main.js").expect("mapper");
    assert_eq!(
        mapper.get_generated_position(&at("/SRC/Main.ts", 4)),
        Some(at("/out/main.js", 4))
    );
}

#[test]
fn maps_positions_through_an_inline_data_url() {
    let url = generator().base64_data_url();
    let host = TestHost::default()
        .with(
            "/out/main.js",
            &format!(
                "{GENERATED}//# sourceMappingURL={}\n",
                url.as_str().unwrap()
            ),
        )
        .with("/src/main.ts", SOURCE);
    let mapper = get_document_position_mapper(&host, b"/out/main.js").expect("mapper");
    assert_eq!(
        mapper.get_source_position(&at("/out/main.js", 6)),
        Some(at("/src/main.ts", 14))
    );
}

#[test]
fn an_unparseable_data_url_falls_back_to_the_dot_map_file() {
    let map = generator().string();
    let host = TestHost::default()
        .with(
            "/out/main.js",
            &format!("{GENERATED}//# sourceMappingURL=data:text/plain,abc\n"),
        )
        .with("/src/main.ts", SOURCE)
        .with("/out/main.js.map", map.as_str().unwrap());
    assert!(get_document_position_mapper(&host, b"/out/main.js").is_some());
    let host = TestHost::default()
        .with(
            "/out/main.js",
            &format!("{GENERATED}//# sourceMappingURL=data:application/json;base64,e\n"),
        )
        .with("/src/main.ts", SOURCE);
    assert!(get_document_position_mapper(&host, b"/out/main.js").is_none());
}

#[test]
fn rejects_invalid_maps() {
    let host_with = |map: &str| {
        TestHost::default()
            .with("/out/main.js", GENERATED)
            .with("/out/main.js.map", map)
    };
    for map in [
        // Wrong version, no sources, no file, no mappings, inline sources,
        // malformed JSON and a mistyped member.
        r#"{"version":2,"file":"main.js","sources":["a.ts"],"mappings":"AAAA"}"#,
        r#"{"version":3,"file":"main.js","sources":[],"mappings":"AAAA"}"#,
        r#"{"version":3,"file":"","sources":["a.ts"],"mappings":"AAAA"}"#,
        r#"{"version":3,"file":"main.js","sources":["a.ts"],"mappings":""}"#,
        r#"{"version":3,"file":"main.js","sources":["a.ts"],"mappings":"AAAA","sourcesContent":[null,"x"]}"#,
        r#"{"version":3,"file":"main.js","#,
        r#"{"version":"3","file":"main.js","sources":["a.ts"],"mappings":"AAAA"}"#,
    ] {
        assert!(
            get_document_position_mapper(&host_with(map), b"/out/main.js").is_none(),
            "{map}"
        );
    }
    // Null inline sources, unknown members and a differently cased member
    // (an unknown member: names match case-sensitively) are accepted.
    for map in [
        r#"{"version":3,"file":"main.js","sources":["a.ts"],"mappings":"AAAA","sourcesContent":[null]}"#,
        r#"{"version":3,"file":"main.js","sources":["a.ts"],"mappings":"AAAA","x_google_ignoreList":[0],"Version":2}"#,
    ] {
        assert!(
            get_document_position_mapper(&host_with(map), b"/out/main.js").is_some(),
            "{map}"
        );
    }
    assert!(get_document_position_mapper(&TestHost::default(), b"/out/main.js").is_none());
}

#[test]
fn a_mapping_error_discards_every_mapping() {
    let host = TestHost::default()
        .with("/out/main.js", GENERATED)
        .with("/a.ts", SOURCE)
        .with(
            "/out/main.js.map",
            r#"{"version":3,"file":"main.js","sourceRoot":"/","sources":["a.ts"],"mappings":"AAAA,D"}"#,
        );
    let mapper = get_document_position_mapper(&host, b"/out/main.js").expect("mapper");
    assert_eq!(mapper.get_source_position(&at("/out/main.js", 0)), None);
    assert_eq!(mapper.get_generated_position(&at("/a.ts", 0)), None);
}

#[test]
fn source_index_is_checked_against_the_mapped_source_count() {
    // Only the second source has mappings, so the pinned
    // `int(sourceIndex) >= len(d.sourceMappings)` check rejects it.
    let host = TestHost::default()
        .with("/out/main.js", GENERATED)
        .with("/a.ts", SOURCE)
        .with("/b.ts", SOURCE)
        .with(
            "/out/main.js.map",
            r#"{"version":3,"file":"main.js","sourceRoot":"/","sources":["a.ts","b.ts"],"mappings":"ACAA"}"#,
        );
    let mapper = get_document_position_mapper(&host, b"/out/main.js").expect("mapper");
    assert_eq!(
        mapper.get_source_position(&at("/out/main.js", 0)),
        Some(at("/b.ts", 0))
    );
    assert_eq!(mapper.get_generated_position(&at("/b.ts", 0)), None);
}

#[test]
fn unknown_line_info_leaves_positions_missing() {
    // Without the source text every mapping has source position -1, so no
    // mapping is source-mapped.
    let map = generator().string();
    let host = TestHost::default()
        .with("/out/main.js", GENERATED)
        .with("/out/main.js.map", map.as_str().unwrap());
    let mapper = get_document_position_mapper(&host, b"/out/main.js").expect("mapper");
    assert_eq!(mapper.get_source_position(&at("/out/main.js", 0)), None);
    assert_eq!(mapper.get_generated_position(&at("/src/main.ts", 0)), None);
}
