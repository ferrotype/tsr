use std::borrow::Cow;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use ts_jsstring::escape::{
    escape_jsx_attribute_string, escape_non_ascii_string, escape_string, escape_string_with_flags,
};
use ts_jsstring::helpers::{lower_first_char, to_lower_js, to_upper_js, truncate_by_runes};
use ts_jsstring::wtf8::{
    code_point_to_surrogate_pair, code_points, combine_surrogate_pairs, decode_rune, decode_utf8,
    encode_rune, surrogate_pair_to_code_point, RUNE_ERROR,
};
use ts_jsstring::{
    JsString, LiteralEscapeFlags as Flags, QuoteChar as Quote, SourceText, Validity,
};

#[test]
fn bom_decoding_preserves_go_odd_byte_and_replacement_behavior() {
    let cases: &[(&[u8], &[u8])] = &[
        (b"", b""),
        (&[0xff], &[0xff]),
        (&[0xef, 0xbb, 0xbf, 0xff], &[0xff]),
        (&[0xef, 0xbb, 0xbf, 0xef, 0xbb, 0xbf], &[0xef, 0xbb, 0xbf]),
        (&[0xff, 0xfe, 0x41, 0, 0xff], b"A"),
        (&[0xfe, 0xff, 0, 0x41, 0xff], b"A"),
        (&[0xff, 0xfe, 0xff], b""),
        (&[0xfe, 0xff, 0xff], b""),
        (&[0xff, 0xfe, 0, 0xd8, 0xff], "�".as_bytes()),
        (&[0xfe, 0xff, 0xdc, 0, 0, 0x41], "�A".as_bytes()),
        (&[0xff, 0xfe, 0x3d, 0xd8, 0, 0xde], "😀".as_bytes()),
        (&[0xfe, 0xff, 0xd8, 0x3d, 0xde, 0], "😀".as_bytes()),
    ];
    for &(input, expected) in cases {
        let source = SourceText::from_bytes(input);
        assert_eq!(source.as_bytes(), expected, "input {input:x?}");
        assert_eq!(source.as_str(), std::str::from_utf8(expected).ok());
    }
}

#[test]
fn byte_slices_share_storage_reclassify_and_outlive_the_parent() {
    let backing: Arc<[u8]> = Arc::from("A😀B".as_bytes());
    let string = JsString::from_bytes(Arc::clone(&backing));
    let cloned = string.clone();
    assert_eq!(Arc::strong_count(&backing), 3);
    assert_eq!(cloned.as_bytes().as_ptr(), string.as_bytes().as_ptr());
    let partial = string.slice(2..4).unwrap();
    assert_eq!(partial.validity(), Validity::Raw);
    assert_eq!(partial.as_str(), None);
    assert_eq!(Arc::strong_count(&backing), 4);
    assert_eq!(string.slice(1..5).unwrap().as_str(), Some("😀"));
    assert_eq!(string.slice(2..2).unwrap().as_str(), Some(""));
    assert!(string.slice(std::ops::Range { start: 5, end: 2 }).is_none());
    assert!(string.slice(0..usize::MAX).is_none());
    drop(string);
    drop(cloned);
    drop(backing);
    assert_eq!(partial.as_bytes(), &[0x9f, 0x98]);
    assert_eq!(partial.slice(1..2).unwrap().as_bytes(), &[0x98]);

    let sentinel = JsString::from_bytes(&[0xed, 0xa0, 0x80][..]);
    assert_eq!(sentinel.validity(), Validity::Wtf8);
    assert_eq!(sentinel.slice(0..1).unwrap().validity(), Validity::Raw);
    assert_eq!(sentinel.slice(0..3).unwrap().validity(), Validity::Wtf8);
}

#[test]
fn equality_order_and_hash_ignore_backing_allocation_and_offsets() {
    let parent = JsString::from_bytes(&[1, 0xff, 0, 9][..]);
    let sliced = parent.slice(1..3).unwrap();
    let standalone = JsString::from_bytes(&[0xff, 0][..]);
    assert_eq!(sliced, standalone);
    let hash = |value: &JsString| {
        let mut hasher = DefaultHasher::new();
        value.hash(&mut hasher);
        hasher.finish()
    };
    assert_eq!(hash(&sliced), hash(&standalone));
    assert!(JsString::from_bytes(&[0xfe][..]) < standalone);
    assert!(JsString::from_bytes(&[0xff][..]) < standalone);
}

#[test]
fn decoders_distinguish_empty_replacement_and_malformed_prefixes() {
    assert_eq!(decode_utf8(b""), (RUNE_ERROR, 0));
    assert_eq!(decode_rune("�".as_bytes()), (RUNE_ERROR, 3));
    for bytes in [
        &[0xff][..],
        &[0x80],
        &[0xc0, 0x80],
        &[0xe0, 0x80, 0x80],
        &[0xe2, 0x82],
        &[0xf0, 0x80, 0x80, 0x80],
        &[0xf4, 0x90, 0x80, 0x80],
    ] {
        assert_eq!(decode_utf8(bytes), (RUNE_ERROR, 1), "{bytes:x?}");
        assert_eq!(decode_rune(bytes), (RUNE_ERROR, 1), "{bytes:x?}");
    }
    for byte in 0_u8..=255 {
        let expected = if byte < 128 {
            i32::from(byte)
        } else {
            RUNE_ERROR
        };
        assert_eq!(decode_utf8(&[byte]), (expected, 1));
    }
    assert_eq!(decode_utf8(&[0xed, 0xa0, 0x80]), (RUNE_ERROR, 1));
    assert_eq!(decode_rune(&[0xed, 0xa0, 0x80]), (0xd800, 3));
}

#[test]
fn every_surrogate_round_trips_without_an_invalid_str_view() {
    for rune in 0xd800..=0xdfff {
        let encoded = encode_rune(rune);
        assert_eq!(decode_rune(&encoded), (rune, 3));
        assert_eq!(decode_utf8(&encoded), (RUNE_ERROR, 1));
        let value = JsString::from_bytes(encoded);
        assert_eq!(value.validity(), Validity::Wtf8);
        assert!(value.as_str().is_none());
    }
    for rune in [-1, 0x110_000, i32::MAX] {
        assert_eq!(encode_rune(rune), "�".as_bytes());
    }
    assert_eq!(
        code_points(&[b'a', 0xed, 0xa0, 0x80, 0xff]).collect::<Vec<_>>(),
        [97, 0xd800, RUNE_ERROR]
    );
}

#[test]
fn surrogate_combination_merges_adjacent_pairs_and_preserves_other_bytes() {
    let high = encode_rune(0xd83d);
    let low = encode_rune(0xde00);
    assert_eq!(
        combine_surrogate_pairs(&[high.as_slice(), low.as_slice()].concat()),
        "😀".as_bytes()
    );
    let separated = [high.as_slice(), &[0xff], low.as_slice()].concat();
    assert_eq!(combine_surrogate_pairs(&separated), separated);
    let doubled = [high.as_slice(), high.as_slice(), low.as_slice()].concat();
    assert_eq!(
        combine_surrogate_pairs(&doubled),
        [high.as_slice(), "😀".as_bytes()].concat()
    );
    assert_eq!(surrogate_pair_to_code_point(0xd83d, 0xde00), 0x1f600);
    assert_eq!(surrogate_pair_to_code_point(0xde00, 0xd83d), RUNE_ERROR);
    assert_eq!(code_point_to_surrogate_pair(0x1f600), (0xd83d, 0xde00));
    assert_eq!(
        code_point_to_surrogate_pair(0xffff),
        (RUNE_ERROR, RUNE_ERROR)
    );
}

#[test]
fn js_casing_uses_full_pinned_mappings_and_final_sigma_context() {
    for (input, expected) in [
        ("İSPANYOL", "i̇spanyol"),
        ("Σ", "σ"),
        ("ΟΣ", "ος"),
        ("ΟΣA", "οσa"),
        ("ʕΣ", "ʕς"),
        ("ʰΣ", "ʰσ"),
        ("ͅΣ", "ͅσ"),
        ("ªΣ", "ªς"),
        ("ⅠΣ", "ⅰς"),
        ("AΣͅA", "aσͅa"),
        ("AΣͅ!", "aςͅ!"),
        ("\u{1c89}Σ", "\u{1c89}σ"),
        ("\u{a7cb}Σ", "\u{a7cb}σ"),
    ] {
        assert_eq!(
            to_lower_js(input.as_bytes()),
            expected.as_bytes(),
            "{input}"
        );
    }
    assert_eq!(to_upper_js("ßﬁω".as_bytes()), "SSFIΩ".as_bytes());
    assert_eq!(
        to_lower_js(&[b'A', 0xed, 0xa0, 0x80, 0xff]).as_ref(),
        &[b'a', 0xed, 0xa0, 0x80, 0xef, 0xbf, 0xbd]
    );
    assert_eq!(
        to_upper_js(&[0xed, 0xbf, 0xbf, b'a', 0xff]).as_ref(),
        &[0xed, 0xbf, 0xbf, b'A', 0xef, 0xbf, 0xbd]
    );
}

#[test]
fn unchanged_fast_paths_borrow_and_text_owners_are_send_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<JsString>();
    assert_send_sync::<SourceText>();
    assert!(matches!(to_lower_js(b"already lower!"), Cow::Borrowed(_)));
    assert!(matches!(to_upper_js(b"ALREADY UPPER!"), Cow::Borrowed(_)));
    assert!(matches!(
        combine_surrogate_pairs(&[0xff, b'A']),
        Cow::Borrowed(_)
    ));
    assert!(matches!(to_lower_js(b"CHANGE"), Cow::Owned(_)));
    assert!(matches!(to_upper_js(b"change"), Cow::Owned(_)));
}

#[test]
fn standard_decoder_helpers_preserve_fragments_and_simple_go_casing() {
    let sentinel = [0xed, 0xa0, 0x80];
    assert_eq!(truncate_by_runes(&sentinel, 1), &[0xed]);
    assert_eq!(truncate_by_runes(&sentinel, 2), &[0xed, 0xa0]);
    assert_eq!(truncate_by_runes("😀x".as_bytes(), 1), "😀".as_bytes());
    assert_eq!(truncate_by_runes(b"a", -1), b"");
    assert_eq!(truncate_by_runes(b"a", isize::MAX), b"a");
    assert_eq!(lower_first_char(&sentinel), &[0xef, 0xbf, 0xbd, 0xa0, 0x80]);
    assert_eq!(lower_first_char("İMORE".as_bytes()), b"iMORE");
    // Go 1.27.1 uses Unicode 17 here; JS casing above intentionally stays at 15.1.
    assert_eq!(
        lower_first_char("\u{1c89}Σ".as_bytes()),
        "\u{1c8a}Σ".as_bytes()
    );
    assert_eq!(
        lower_first_char("\u{10d50}Garay".as_bytes()),
        "\u{10d70}Garay".as_bytes()
    );
    assert_eq!(to_lower_js("\u{10d50}".as_bytes()), "\u{10d50}".as_bytes());
}

#[test]
fn escaping_handles_template_crlf_null_digits_jsx_and_malformed_bytes() {
    assert_eq!(
        escape_string(b"a\r\nb\nc${x}`", Quote::Backtick),
        b"a\\r\\nb\nc\\${x}\\`"
    );
    assert_eq!(
        escape_string(&[0, b'x', 0, b'1'], Quote::Double),
        br"\0x\x001"
    );
    assert_eq!(
        escape_string(&[0xed, 0xa0, 0x80, 0xff], Quote::Double),
        br"\uD800\uFFFD"
    );
    assert_eq!(
        escape_string("é😀".as_bytes(), Quote::Single),
        "é😀".as_bytes()
    );
    assert_eq!(
        escape_non_ascii_string("é😀".as_bytes(), Quote::Single),
        br"\u00E9\uD83D\uDE00"
    );
    assert_eq!(
        escape_jsx_attribute_string(b"\\\"'\n\0", Quote::Double),
        b"\\&quot;'&#xA;&#0;"
    );
    assert_eq!(
        escape_jsx_attribute_string(&[0xed, 0xa0, 0x80, 0xff], Quote::Single),
        b"&#xD800;&#xFFFD;"
    );
    assert_eq!(escape_string(&[0x7f], Quote::Double), &[0x7f]);
    let unknown = Flags::from_bits(16);
    assert_eq!(unknown, None);
    for bits in 0..4 {
        assert_eq!(
            escape_string_with_flags(b"a\n", Quote::Double, Flags::from_bits(bits).unwrap()),
            escape_string_with_flags(
                b"a\n",
                Quote::Double,
                Flags::from_bits(bits | 0b1100).unwrap()
            ),
        );
    }
}

#[test]
fn every_slice_boundary_reclassifies_utf8_and_surrogate_fragments() {
    let input: Arc<[u8]> = Arc::from("\u{feff}A😀éB".as_bytes());
    let source = SourceText::from_bytes(Arc::clone(&input));
    assert_eq!(Arc::strong_count(&input), 2);
    assert_eq!(source.as_bytes().as_ptr(), input[3..].as_ptr());
    for start in 0..=source.len() {
        for end in start..=source.len() {
            let slice = source.slice(start..end).unwrap();
            let bytes = &source.as_bytes()[start..end];
            assert_eq!(slice.as_bytes(), bytes);
            assert_eq!(slice.as_bytes().as_ptr(), bytes.as_ptr());
            assert_eq!(slice.as_str(), std::str::from_utf8(bytes).ok());
            let expected = if std::str::from_utf8(bytes).is_ok() {
                Validity::Utf8
            } else {
                Validity::Raw
            };
            assert_eq!(slice.validity(), expected, "UTF-8 slice {start}..{end}");
        }
    }

    let sentinel = JsString::from_bytes(&b"A\xed\xa0\x80B"[..]);
    for start in 0..=sentinel.len() {
        for end in start..=sentinel.len() {
            let slice = sentinel.slice(start..end).unwrap();
            let complete_boundaries = [0, 1, 4, 5];
            let expected = if start == end || end <= 1 || start >= 4 {
                Validity::Utf8
            } else if complete_boundaries.contains(&start) && complete_boundaries.contains(&end) {
                Validity::Wtf8
            } else {
                Validity::Raw
            };
            assert_eq!(slice.validity(), expected, "sentinel slice {start}..{end}");
            assert_eq!(slice.as_str().is_some(), expected == Validity::Utf8);
        }
    }
}

#[test]
fn nested_byte_views_keep_their_own_validity_and_checked_relative_ranges() {
    for bytes in [
        "Aé中😀Z".as_bytes(),
        b"\xffA\xc3\xa9\xe4\xb8\xad\xf0\x9f\x98\x80Z\xfe",
        b"A\xed\xa0\x80\xc3\xa9Z",
        b"\xf0\x9fA\x80Z",
    ] {
        let root = JsString::from_bytes(bytes);
        for start in 0..=root.len() {
            for end in start..=root.len() {
                let parent = root.slice(start..end).unwrap();
                for inner_start in 0..=parent.len() {
                    for inner_end in inner_start..=parent.len() {
                        let range = inner_start..inner_end;
                        let child = parent.slice(range.clone()).unwrap();
                        let selected = &parent.as_bytes()[range];
                        // A fresh constructor always classifies the exact bytes,
                        // independently of any parent or nested-view proof.
                        let fresh = JsString::from_bytes(selected);
                        assert_eq!(child.validity(), fresh.validity());
                        assert_eq!(child.as_str(), fresh.as_str());
                        assert_eq!(child.as_bytes(), selected);
                        assert_eq!(child.as_bytes().as_ptr(), selected.as_ptr());
                    }
                }
                for invalid in [
                    parent.len() + 1..parent.len() + 1,
                    0..parent.len() + 1,
                    std::ops::Range { start: 1, end: 0 },
                    usize::MAX..usize::MAX,
                    0..usize::MAX,
                ] {
                    assert!(parent.slice(invalid).is_none());
                }
            }
        }
    }
}
