//! The pinned `internal/sourcemap/generator_test.go`, test for test.
use tsr_jsstring::JsString;

use crate::{new_generator, Generator, RawSourceMap};

fn js(text: &str) -> JsString {
    JsString::from_bytes(text.as_bytes())
}

/// `NewGenerator("main.js", "/", "/", tspath.ComparePathsOptions{})`: an
/// empty current directory and case-insensitive names.
fn generator() -> Generator {
    new_generator(js("main.js"), js("/"), js("/"), js(""), false)
}

fn raw(sources: &[&str], mappings: &str, names: &[&str]) -> RawSourceMap {
    RawSourceMap {
        version: 3,
        file: js("main.js"),
        source_root: js("/"),
        sources: sources.iter().map(|s| js(s)).collect(),
        mappings: js(mappings),
        names: names.iter().map(|s| js(s)).collect(),
        sources_content: None,
    }
}

fn assert_error(result: Result<(), crate::Error>, message: &str) {
    assert_eq!(result.expect_err("expected an error").to_string(), message);
}

#[test]
fn source_map_generator_empty() {
    let mut gen = generator();
    let source_map = gen.raw_source_map();
    assert_eq!(source_map, raw(&[], "", &[]));
}

#[test]
fn source_map_generator_empty_serialized() {
    let mut gen = generator();
    let actual = gen.string();
    let expected =
        r#"{"version":3,"file":"main.js","sourceRoot":"/","sources":[],"names":[],"mappings":""}"#;
    assert_eq!(actual.as_bytes(), expected.as_bytes());
}

#[test]
fn source_map_generator_add_source() {
    let mut gen = generator();
    let source_index = gen.add_source(js("/main.ts"));
    let source_map = gen.raw_source_map();
    assert_eq!(source_index, 0);
    assert_eq!(source_map, raw(&["main.ts"], "", &[]));
}

#[test]
fn source_map_generator_set_source_content() {
    let mut gen = generator();
    let source_index = gen.add_source(js("/main.ts"));
    let source_content = js("foo");
    gen.set_source_content(source_index, source_content.clone())
        .unwrap();
    let source_map = gen.raw_source_map();
    assert_eq!(source_index, 0);
    assert_eq!(
        source_map,
        RawSourceMap {
            sources_content: Some(vec![Some(source_content)]),
            ..raw(&["main.ts"], "", &[])
        }
    );
}

#[test]
fn source_map_generator_set_source_content_for_second_source_only() {
    let mut gen = generator();
    gen.add_source(js("/skipped.ts"));
    let source_index = gen.add_source(js("/main.ts"));
    let source_content = js("foo");
    gen.set_source_content(source_index, source_content.clone())
        .unwrap();
    let source_map = gen.raw_source_map();
    assert_eq!(source_index, 1);
    assert_eq!(
        source_map,
        RawSourceMap {
            sources_content: Some(vec![None, Some(source_content)]),
            ..raw(&["skipped.ts", "main.ts"], "", &[])
        }
    );
}

#[test]
fn source_map_generator_set_source_content_source_index_out_of_range() {
    let mut gen = generator();
    assert_error(
        gen.set_source_content(-1, js("")),
        "sourceIndex is out of range",
    );
    assert_error(
        gen.set_source_content(0, js("")),
        "sourceIndex is out of range",
    );
}

#[test]
fn source_map_generator_set_source_content_for_second_source_only_serialized() {
    let mut gen = generator();
    gen.add_source(js("/skipped.ts"));
    let source_index = gen.add_source(js("/main.ts"));
    let source_content = js("foo");
    gen.set_source_content(source_index, source_content)
        .unwrap();
    let actual = gen.string();
    let expected = r#"{"version":3,"file":"main.js","sourceRoot":"/","sources":["skipped.ts","main.ts"],"names":[],"mappings":"","sourcesContent":[null,"foo"]}"#;
    assert_eq!(actual.as_bytes(), expected.as_bytes());
}

#[test]
fn source_map_generator_add_name() {
    let mut gen = generator();
    let name_index = gen.add_name(js("foo"));
    let source_map = gen.raw_source_map();
    assert_eq!(name_index, 0);
    assert_eq!(source_map, raw(&[], "", &["foo"]));
}

#[test]
fn source_map_generator_add_generated_mapping() {
    let mut gen = generator();
    gen.add_generated_mapping(0, 0).unwrap();
    let source_map = gen.raw_source_map();
    assert_eq!(source_map, raw(&[], "A", &[]));
}

#[test]
fn source_map_generator_add_generated_mapping_replaces_pending_source_mapping() {
    let mut gen = generator();
    let source_index = gen.add_source(js("/main.ts"));
    gen.add_source_mapping(0, 0, source_index, 0, 0).unwrap();
    gen.add_generated_mapping(0, 0).unwrap();
    let source_map = gen.raw_source_map();
    assert_eq!(source_map.mappings.as_bytes(), b"A");
}

#[test]
fn source_map_generator_add_generated_mapping_is_not_replaced_by_source_mapping() {
    let mut gen = generator();
    let source_index = gen.add_source(js("/main.ts"));
    gen.add_generated_mapping(0, 0).unwrap();
    gen.add_source_mapping(0, 0, source_index, 0, 0).unwrap();
    let source_map = gen.raw_source_map();
    assert_eq!(source_map.mappings.as_bytes(), b"A");
}

#[test]
fn source_map_generator_add_generated_mapping_on_second_line_only() {
    let mut gen = generator();
    gen.add_generated_mapping(1, 0).unwrap();
    let source_map = gen.raw_source_map();
    assert_eq!(source_map, raw(&[], ";A", &[]));
}

#[test]
fn source_map_generator_add_source_mapping() {
    let mut gen = generator();
    let source_index = gen.add_source(js("/main.ts"));
    gen.add_source_mapping(0, 0, source_index, 0, 0).unwrap();
    let source_map = gen.raw_source_map();
    assert_eq!(source_map, raw(&["main.ts"], "AAAA", &[]));
}

#[test]
fn source_map_generator_add_source_mapping_next_generated_character() {
    let mut gen = generator();
    let source_index = gen.add_source(js("/main.ts"));
    gen.add_source_mapping(0, 0, source_index, 0, 0).unwrap();
    gen.add_source_mapping(0, 1, source_index, 0, 0).unwrap();
    let source_map = gen.raw_source_map();
    assert_eq!(source_map, raw(&["main.ts"], "AAAA,CAAA", &[]));
}

#[test]
fn source_map_generator_add_source_mapping_next_generated_and_source_character() {
    let mut gen = generator();
    let source_index = gen.add_source(js("/main.ts"));
    gen.add_source_mapping(0, 0, source_index, 0, 0).unwrap();
    gen.add_source_mapping(0, 1, source_index, 0, 1).unwrap();
    let source_map = gen.raw_source_map();
    assert_eq!(source_map, raw(&["main.ts"], "AAAA,CAAC", &[]));
}

#[test]
fn source_map_generator_add_source_mapping_next_generated_line() {
    let mut gen = generator();
    let source_index = gen.add_source(js("/main.ts"));
    gen.add_source_mapping(0, 0, source_index, 0, 0).unwrap();
    gen.add_source_mapping(1, 0, source_index, 0, 0).unwrap();
    let source_map = gen.raw_source_map();
    assert_eq!(source_map, raw(&["main.ts"], "AAAA;AAAA", &[]));
}

#[test]
fn source_map_generator_add_source_mapping_previous_source_character() {
    let mut gen = generator();
    let source_index = gen.add_source(js("/main.ts"));
    gen.add_source_mapping(0, 0, source_index, 0, 1).unwrap();
    gen.add_source_mapping(0, 1, source_index, 0, 0).unwrap();
    let source_map = gen.raw_source_map();
    assert_eq!(source_map, raw(&["main.ts"], "AAAC,CAAD", &[]));
}

#[test]
fn source_map_generator_add_named_source_mapping() {
    let mut gen = generator();
    let source_index = gen.add_source(js("/main.ts"));
    let name_index = gen.add_name(js("foo"));
    gen.add_named_source_mapping(0, 0, source_index, 0, 0, name_index)
        .unwrap();
    let source_map = gen.raw_source_map();
    assert_eq!(source_map, raw(&["main.ts"], "AAAAA", &["foo"]));
}

#[test]
fn source_map_generator_add_named_source_mapping_with_previous_name() {
    let mut gen = generator();
    let source_index = gen.add_source(js("/main.ts"));
    let name_index1 = gen.add_name(js("foo"));
    let name_index2 = gen.add_name(js("bar"));
    gen.add_named_source_mapping(0, 0, source_index, 0, 0, name_index2)
        .unwrap();
    gen.add_named_source_mapping(0, 1, source_index, 0, 0, name_index1)
        .unwrap();
    let source_map = gen.raw_source_map();
    assert_eq!(
        source_map,
        raw(&["main.ts"], "AAAAC,CAAAD", &["foo", "bar"])
    );
}

#[test]
fn source_map_generator_add_generated_mapping_generated_line_cannot_backtrack() {
    let mut gen = generator();
    gen.add_generated_mapping(1, 0).unwrap();
    assert_error(
        gen.add_generated_mapping(0, 0),
        "generatedLine cannot backtrack",
    );
}

#[test]
fn source_map_generator_add_generated_mapping_generated_character_cannot_be_negative() {
    let mut gen = generator();
    gen.add_generated_mapping(0, 0).unwrap();
    assert_error(
        gen.add_generated_mapping(0, -1),
        "generatedCharacter cannot be negative",
    );
}

#[test]
fn source_map_generator_add_source_mapping_generated_line_cannot_backtrack() {
    let mut gen = generator();
    let source_index = gen.add_source(js("/main.ts"));
    gen.add_source_mapping(1, 0, source_index, 0, 0).unwrap();
    assert_error(
        gen.add_source_mapping(0, 0, source_index, 0, 0),
        "generatedLine cannot backtrack",
    );
}

#[test]
fn source_map_generator_add_source_mapping_generated_character_cannot_be_negative() {
    let mut gen = generator();
    let source_index = gen.add_source(js("/main.ts"));
    gen.add_source_mapping(0, 0, source_index, 0, 0).unwrap();
    assert_error(
        gen.add_source_mapping(0, -1, source_index, 0, 0),
        "generatedCharacter cannot be negative",
    );
}

#[test]
fn source_map_generator_add_source_mapping_source_index_is_out_of_range() {
    let mut gen = generator();
    assert_error(
        gen.add_source_mapping(0, 0, -1, 0, 0),
        "sourceIndex is out of range",
    );
    assert_error(
        gen.add_source_mapping(0, 0, 0, 0, 0),
        "sourceIndex is out of range",
    );
}

#[test]
fn source_map_generator_add_source_mapping_source_line_cannot_be_negative() {
    let mut gen = generator();
    let source_index = gen.add_source(js("/main.ts"));
    assert_error(
        gen.add_source_mapping(0, 0, source_index, -1, 0),
        "sourceLine cannot be negative",
    );
}

#[test]
fn source_map_generator_add_source_mapping_source_character_cannot_be_negative() {
    let mut gen = generator();
    let source_index = gen.add_source(js("/main.ts"));
    assert_error(
        gen.add_source_mapping(0, 0, source_index, 0, -1),
        "sourceCharacter cannot be negative",
    );
}

#[test]
fn source_map_generator_add_named_source_mapping_generated_line_cannot_backtrack() {
    let mut gen = generator();
    let source_index = gen.add_source(js("/main.ts"));
    let name_index = gen.add_name(js("foo"));
    gen.add_named_source_mapping(1, 0, source_index, 0, 0, name_index)
        .unwrap();
    assert_error(
        gen.add_named_source_mapping(0, 0, source_index, 0, 0, name_index),
        "generatedLine cannot backtrack",
    );
}

#[test]
fn source_map_generator_add_named_source_mapping_generated_character_cannot_be_negative() {
    let mut gen = generator();
    let source_index = gen.add_source(js("/main.ts"));
    let name_index = gen.add_name(js("foo"));
    gen.add_named_source_mapping(0, 0, source_index, 0, 0, name_index)
        .unwrap();
    assert_error(
        gen.add_named_source_mapping(0, -1, source_index, 0, 0, name_index),
        "generatedCharacter cannot be negative",
    );
}

#[test]
fn source_map_generator_add_named_source_mapping_source_index_is_out_of_range() {
    let mut gen = generator();
    let name_index = gen.add_name(js("foo"));
    assert_error(
        gen.add_named_source_mapping(0, 0, -1, 0, 0, name_index),
        "sourceIndex is out of range",
    );
    assert_error(
        gen.add_named_source_mapping(0, 0, 0, 0, 0, name_index),
        "sourceIndex is out of range",
    );
}

#[test]
fn source_map_generator_add_named_source_mapping_source_line_cannot_be_negative() {
    let mut gen = generator();
    let name_index = gen.add_name(js("foo"));
    let source_index = gen.add_source(js("/main.ts"));
    assert_error(
        gen.add_named_source_mapping(0, 0, source_index, -1, 0, name_index),
        "sourceLine cannot be negative",
    );
}

#[test]
fn source_map_generator_add_named_source_mapping_source_character_cannot_be_negative() {
    let mut gen = generator();
    let name_index = gen.add_name(js("foo"));
    let source_index = gen.add_source(js("/main.ts"));
    assert_error(
        gen.add_named_source_mapping(0, 0, source_index, 0, -1, name_index),
        "sourceCharacter cannot be negative",
    );
}

#[test]
fn source_map_generator_add_named_source_mapping_name_index_is_out_of_range() {
    let mut gen = generator();
    let source_index = gen.add_source(js("/main.ts"));
    assert_error(
        gen.add_named_source_mapping(0, 0, source_index, 0, 0, -1),
        "nameIndex is out of range",
    );
    assert_error(
        gen.add_named_source_mapping(0, 0, source_index, 0, 0, 0),
        "nameIndex is out of range",
    );
}

// The tests below have no Go counterpart; their expectations were derived by
// reading the pinned generator.go and Go's encoding/base64.

#[test]
fn base64_data_url_encodes_the_serialized_map() {
    let mut gen = generator();
    // base64 of {"version":3,"file":"main.js","sourceRoot":"/","sources":[],"names":[],"mappings":""}
    let expected = "data:application/json;base64,eyJ2ZXJzaW9uIjozLCJmaWxlIjoibWFpbi5qcyIsInNvdXJjZVJvb3QiOiIvIiwic291cmNlcyI6W10sIm5hbWVzIjpbXSwibWFwcGluZ3MiOiIifQ==";
    assert_eq!(gen.base64_data_url().as_bytes(), expected.as_bytes());
}

#[test]
fn sources_returns_the_raw_file_names_once() {
    let mut gen = generator();
    assert_eq!(gen.add_source(js("/a/main.ts")), 0);
    assert_eq!(gen.add_source(js("/b.ts")), 1);
    // The relative source is the key: a second spelling of the same file
    // returns the first index and keeps the first raw name.
    assert_eq!(gen.add_source(js("/a/./main.ts")), 0);
    assert_eq!(gen.sources(), &[js("/a/main.ts"), js("/b.ts")]);
    assert_eq!(
        gen.raw_source_map().sources,
        vec![js("a/main.ts"), js("b.ts")]
    );
}

#[test]
fn large_and_negative_deltas_use_continuation_digits() {
    let mut gen = generator();
    let source_index = gen.add_source(js("/main.ts"));
    // 16 encodes as "gB" (32 | 0, then 1); -16 then encodes as "hB".
    gen.add_source_mapping(0, 16, source_index, 0, 16).unwrap();
    gen.add_source_mapping(2, 0, source_index, 0, 0).unwrap();
    assert_eq!(gen.raw_source_map().mappings.as_bytes(), b"gBAAgB;;AAAhB");
}

#[test]
fn backtracking_source_position_commits_the_pending_mapping() {
    let mut gen = generator();
    let source_index = gen.add_source(js("/main.ts"));
    // A same-position mapping that moves the source backwards is committed
    // as a separate segment rather than replacing the pending one.
    gen.add_source_mapping(0, 0, source_index, 0, 5).unwrap();
    gen.add_source_mapping(0, 0, source_index, 0, 2).unwrap();
    assert_eq!(gen.raw_source_map().mappings.as_bytes(), b"AAAK,AAAH");
}

#[test]
fn json_strings_are_escaped_and_invalid_utf8_repaired() {
    let mut gen = new_generator(
        JsString::from_bytes(&b"a\"b\xffc.js"[..]),
        js(""),
        js("/"),
        js(""),
        false,
    );
    assert_eq!(
        gen.string().as_bytes(),
        "{\"version\":3,\"file\":\"a\\\"b\u{fffd}c.js\",\"sourceRoot\":\"\",\"sources\":[],\"names\":[],\"mappings\":\"\"}".as_bytes()
    );
}

// json v2 resets a struct to its zero value on `null` (`makeStructArshaler`).
#[test]
fn decoding_null_resets_a_populated_map() {
    let mut map = RawSourceMap {
        version: 3,
        file: js("a.js"),
        mappings: js("AAAA"),
        ..RawSourceMap::default()
    };
    tsr_json::unmarshal(b"null", &mut map, tsr_json::Options::default()).unwrap();
    assert_eq!(map, RawSourceMap::default());
}
