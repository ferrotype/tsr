//! The pinned package has no decoder tests. These decode the mapping strings
//! the ported generator tests produce, plus each error state; every
//! expectation was derived by reading the pinned decoder.go.
use tsr_jsstring::JsString;

use crate::{decode_mappings, Mapping, MappingsDecoder};

fn decoder(mappings: &str) -> MappingsDecoder {
    decode_mappings(JsString::from_bytes(mappings.as_bytes()))
}

/// A generated-only mapping.
fn generated(line: isize, character: isize) -> Mapping {
    Mapping {
        generated_line: line,
        generated_character: character,
        source_index: -1,
        source_line: -1,
        source_character: -1,
        name_index: -1,
    }
}

fn source(
    line: isize,
    character: isize,
    source_index: isize,
    source_line: isize,
    source_character: isize,
) -> Mapping {
    Mapping {
        generated_line: line,
        generated_character: character,
        source_index,
        source_line,
        source_character,
        name_index: -1,
    }
}

fn named(
    line: isize,
    character: isize,
    source_index: isize,
    source_line: isize,
    source_character: isize,
    name_index: isize,
) -> Mapping {
    Mapping {
        name_index,
        ..source(line, character, source_index, source_line, source_character)
    }
}

fn decode_all(mappings: &str) -> (Vec<Mapping>, Option<String>, isize) {
    let mut decoder = decoder(mappings);
    let values: Vec<Mapping> = decoder.values().collect();
    let error = decoder.error().map(ToString::to_string);
    (values, error, decoder.pos())
}

#[test]
fn decodes_generator_test_mappings() {
    let cases: &[(&str, Vec<Mapping>)] = &[
        ("", vec![]),
        ("A", vec![generated(0, 0)]),
        (";A", vec![generated(1, 0)]),
        ("AAAA", vec![source(0, 0, 0, 0, 0)]),
        (
            "AAAA,CAAA",
            vec![source(0, 0, 0, 0, 0), source(0, 1, 0, 0, 0)],
        ),
        (
            "AAAA,CAAC",
            vec![source(0, 0, 0, 0, 0), source(0, 1, 0, 0, 1)],
        ),
        (
            "AAAA;AAAA",
            vec![source(0, 0, 0, 0, 0), source(1, 0, 0, 0, 0)],
        ),
        (
            "AAAC,CAAD",
            vec![source(0, 0, 0, 0, 1), source(0, 1, 0, 0, 0)],
        ),
        ("AAAAA", vec![named(0, 0, 0, 0, 0, 0)]),
        (
            "AAAAC,CAAAD",
            vec![named(0, 0, 0, 0, 0, 1), named(0, 1, 0, 0, 0, 0)],
        ),
        (
            "gBAAgB;;AAAhB",
            vec![source(0, 16, 0, 0, 16), source(2, 0, 0, 0, 0)],
        ),
        (
            "AAAK,AAAH",
            vec![source(0, 0, 0, 0, 5), source(0, 0, 0, 0, 2)],
        ),
    ];
    for (mappings, expected) in cases {
        let (values, error, pos) = decode_all(mappings);
        assert_eq!(&values, expected, "{mappings}");
        assert_eq!(error, None, "{mappings}");
        assert_eq!(pos, mappings.len() as isize, "{mappings}");
    }
}

#[test]
fn a_name_index_is_only_present_on_its_segment() {
    // The accumulated name index survives a segment without one, but that
    // segment reports MissingName; State reports every accumulated field.
    let mut decoder = decoder("AAAAC,CAAA");
    assert_eq!(decoder.next(), Some(named(0, 0, 0, 0, 0, 1)));
    assert_eq!(decoder.next(), Some(source(0, 1, 0, 0, 0)));
    assert_eq!(decoder.state(), named(0, 1, 0, 0, 0, 1));
    assert_eq!(decoder.next(), None);
    assert!(decoder.error().is_none());
}

#[test]
fn reports_each_error_state() {
    let cases: &[(&str, Vec<Mapping>, &str, isize)] = &[
        // 'D' is -1.
        ("D", vec![], "Invalid generatedCharacter found", 1),
        ("ADAA", vec![], "Invalid sourceIndex found", 2),
        (
            "AA",
            vec![],
            "Unsupported Format: No entries after sourceIndex",
            2,
        ),
        ("AAD", vec![], "Invalid sourceLine found", 3),
        (
            "AAA,",
            vec![],
            "Unsupported Format: No entries after sourceLine",
            3,
        ),
        ("AAAD", vec![], "Invalid sourceCharacter found", 4),
        ("AAAAD", vec![], "Invalid nameIndex found", 5),
        (
            "AAAAAA",
            vec![],
            "Unsupported Error Format: Entries after nameIndex",
            5,
        ),
        ("A,!", vec![generated(0, 0)], "Invalid character in VLQ", 2),
        (
            "A;g",
            vec![generated(0, 0)],
            "Error in decoding base64VLQFormatDecode, past the mapping string",
            3,
        ),
    ];
    for (mappings, expected, message, pos) in cases {
        let (values, error, actual_pos) = decode_all(mappings);
        assert_eq!(&values, expected, "{mappings}");
        assert_eq!(error.as_deref(), Some(*message), "{mappings}");
        assert_eq!(actual_pos, *pos, "{mappings}");
    }
}

#[test]
fn iteration_stays_done_after_an_error() {
    let mut decoder = decoder("D,A");
    assert_eq!(decoder.next(), None);
    assert_eq!(decoder.next(), None);
    assert_eq!(
        decoder.error().map(ToString::to_string).as_deref(),
        Some("Invalid generatedCharacter found")
    );
    assert_eq!(decoder.mappings_string(), b"D,A");
}

#[test]
fn negative_zero_and_overlong_values() {
    // 'B' is -0, which is 0. Twelve continuation digits shift past 64 bits,
    // where Go's shift yields 0 instead of panicking.
    let (values, error, _) = decode_all("B");
    assert_eq!(values, vec![generated(0, 0)]);
    assert_eq!(error, None);
    let (values, error, _) = decode_all("ggggggggggggggA");
    assert_eq!(values, vec![generated(0, 0)]);
    assert_eq!(error, None);
}

#[test]
fn mapping_equals_and_is_source_mapping() {
    let a = source(0, 1, 0, 2, 3);
    let b = source(0, 1, 0, 2, 3);
    assert!(a.equals(&a));
    assert!(a.equals(&b));
    assert!(!a.equals(&named(0, 1, 0, 2, 3, 0)));
    assert!(a.is_source_mapping());
    assert!(!generated(0, 0).is_source_mapping());
}
