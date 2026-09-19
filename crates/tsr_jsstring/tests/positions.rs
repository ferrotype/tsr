use std::panic::catch_unwind;

use ts_jsstring::line_map::*;
use ts_jsstring::lsp::*;
use ts_jsstring::scanner_positions::*;
use ts_jsstring::PositionMap;

#[test]
fn lsp_scans_compare_signed_line_bounds_before_indexing() {
    let negative_end = LspLineMap {
        line_starts: vec![0, i32::MIN],
        ascii_only: false,
    };
    assert_eq!(
        lsp_line_and_character_to_position(
            b"a",
            &negative_end,
            LspPosition {
                line: 0,
                character: 1
            },
            PositionEncoding::Utf16,
        ),
        0
    );
    let negative_start = LspLineMap {
        line_starts: vec![-1, 1],
        ascii_only: false,
    };
    assert!(catch_unwind(|| lsp_line_and_character_to_position(
        b"a",
        &negative_start,
        LspPosition {
            line: 0,
            character: 0
        },
        PositionEncoding::Utf16,
    ))
    .is_err());
    let wrapped_difference = LspLineMap {
        line_starts: vec![i32::MIN],
        ascii_only: true,
    };
    assert_eq!(
        lsp_position_to_line_and_character(b"", &wrapped_difference, 0, PositionEncoding::Utf8,),
        LspPosition {
            line: 0,
            character: 1_u32 << 31
        }
    );
}

#[test]
fn api_positions_keep_partial_offsets_and_unbounded_arithmetic() {
    let map = PositionMap::new("😀".as_bytes());
    assert!(!map.is_ascii_only());
    for (byte, utf16) in [(-1, -1), (0, 0), (1, 1), (2, 2), (3, 3), (4, 2), (5, 3)] {
        assert_eq!(map.utf8_to_utf16(byte), utf16);
    }
    for (utf16, byte) in [(-1, -1), (0, 0), (1, 1), (2, 4), (3, 5)] {
        assert_eq!(map.utf16_to_utf8(utf16), byte);
    }
    assert_eq!(map.utf16_to_utf8(isize::MAX), isize::MIN + 1);
    let ascii = PositionMap::new(b"abc");
    assert!(ascii.is_ascii_only());
    for offset in [isize::MIN, -1, 0, 100, isize::MAX] {
        assert_eq!(ascii.utf16_to_utf8(offset), offset);
        assert_eq!(ascii.utf8_to_utf16(offset), offset);
    }
    assert!(PositionMap::new(b"").is_ascii_only());
    assert!(!PositionMap::new(b"\xff").is_ascii_only());
}

#[test]
fn api_counts_complete_sentinels_and_raw_bytes_separately() {
    let text = b"a\xed\xa0\x80\xff\xc3\xa9";
    let map = PositionMap::new(text);
    assert_eq!(map.utf8_to_utf16(4), 2);
    assert_eq!(map.utf8_to_utf16(5), 3);
    assert_eq!(map.utf8_to_utf16(7), 4);
    assert_eq!(map.utf16_to_utf8(2), 4);
    assert_eq!(map.utf16_to_utf8(3), 5);
    assert_eq!(map.utf16_to_utf8(4), 7);
    assert_eq!(utf16_len(text), 6);
    let fragment = PositionMap::new(&text[1..3]);
    assert_eq!(fragment.utf16_to_utf8(2), 2);
}

#[test]
fn three_utf16_paths_round_inside_astral_characters_differently() {
    let text = "😀".as_bytes();
    let lsp = LspLineMap::new(text);
    assert_eq!(PositionMap::new(text).utf16_to_utf8(1), 1);
    assert_eq!(
        lsp_line_and_character_to_position(
            text,
            &lsp,
            LspPosition {
                line: 0,
                character: 1
            },
            PositionEncoding::Utf16
        ),
        0
    );
    assert_eq!(
        compute_position_of_line_and_utf16_character(&[0], 0, 1, text, false),
        4
    );
    assert_eq!(
        compute_position_of_line_and_utf16_character(&[0], 0, 2, text, false),
        4
    );
}

#[test]
fn lsp_counts_raw_prefixes_and_preserves_negotiated_byte_offsets() {
    let text = b"\xf0\x9f\x98\x80\xed\xa0\x80\xff";
    let map = LspLineMap::new(text);
    for (position, character) in [
        (0, 0),
        (1, 1),
        (2, 2),
        (3, 3),
        (4, 2),
        (5, 3),
        (6, 4),
        (7, 5),
        (8, 6),
    ] {
        assert_eq!(
            lsp_position_to_line_and_character(text, &map, position, PositionEncoding::Utf16),
            LspPosition { line: 0, character }
        );
        assert_eq!(
            lsp_position_to_line_and_character(text, &map, position, PositionEncoding::Utf8)
                .character,
            position as u32
        );
    }
    for (character, position) in [
        (0, 0),
        (1, 0),
        (2, 4),
        (3, 5),
        (4, 6),
        (5, 7),
        (6, 8),
        (99, 8),
    ] {
        assert_eq!(
            lsp_line_and_character_to_position(
                text,
                &map,
                LspPosition { line: 0, character },
                PositionEncoding::Utf16
            ),
            position
        );
    }
    assert_eq!(
        lsp_line_and_character_to_position(
            text,
            &map,
            LspPosition {
                line: 0,
                character: 1
            },
            PositionEncoding::Utf8
        ),
        1
    );
}

#[test]
fn ecma_and_lsp_line_maps_use_distinct_terminators() {
    let text = "a\r\nb\rc\nd\u{2028}e\u{2029}".as_bytes();
    assert_eq!(compute_ecma_line_starts(text), [0, 3, 5, 7, 11, 15]);
    let map = LspLineMap::new(text);
    assert_eq!(map.line_starts, [0, 3, 5, 7]);
    assert!(!map.ascii_only);
    assert_eq!(LspLineMap::new(b"a\r\nb\rc\n").line_starts, [0, 3, 5, 7]);
    assert!(LspLineMap::new(b"a\r\nb\rc\n").ascii_only);
    assert_eq!(compute_ecma_line_starts(b""), [0]);
    assert_eq!(LspLineMap::new(b"").line_starts, [0]);
    assert!(!LspLineMap::new(b"\xff").ascii_only);
    assert_eq!(compute_ecma_line_starts(b"\xe2\x80\n\xff\r\n"), [0, 3, 6]);
    assert_eq!(
        compute_ecma_line_starts_seq(text)
            .take(2)
            .collect::<Vec<_>>(),
        [0, 3]
    );
}

#[test]
fn lsp_clamps_ranges_and_retains_signed_wire_casts() {
    let text = b"a\r\nb";
    let map = LspLineMap::new(text);
    for encoding in [PositionEncoding::Utf8, PositionEncoding::Utf16] {
        for (line, character, expected) in [
            (0, 99, 3),
            (1, 99, 4),
            (2, 0, 4),
            (0, u32::MAX, 0),
            (1, i32::MAX as u32, 3),
        ] {
            assert_eq!(
                lsp_line_and_character_to_position(
                    text,
                    &map,
                    LspPosition { line, character },
                    encoding
                ),
                expected
            );
        }
        assert_eq!(
            lsp_position_to_line_and_character(text, &map, -1, encoding),
            LspPosition {
                line: 0,
                character: 0
            }
        );
        assert_eq!(
            lsp_position_to_line_and_character(text, &map, 99, encoding),
            LspPosition {
                line: 1,
                character: 1
            }
        );
        assert!(catch_unwind(|| lsp_line_and_character_to_position(
            text,
            &map,
            LspPosition {
                line: u32::MAX,
                character: 0
            },
            encoding
        ))
        .is_err());
    }
    for (position, line) in [(-1, 0), (0, 0), (2, 0), (3, 1), (99, 1)] {
        assert_eq!(map.compute_index_of_line_start(position), line);
    }
}

#[test]
fn scanner_ranges_clamp_only_where_allow_edits_says() {
    let text = b"a\r\nb";
    let starts = compute_ecma_line_starts(text);
    for allow_edits in [false, true] {
        assert_eq!(
            compute_position_of_line_and_utf16_character(&starts, 0, -1, text, allow_edits),
            0
        );
        assert_eq!(
            compute_position_of_line_and_utf16_character(&starts, 0, 3, text, allow_edits),
            3
        );
        assert_eq!(
            compute_position_of_line_and_utf16_character(&starts, 1, 0, text, allow_edits),
            3
        );
    }
    for (line, character) in [(-1, 0), (2, 0), (0, 4), (1, 2)] {
        assert!(
            catch_unwind(|| compute_position_of_line_and_utf16_character(
                &starts, line, character, text, false
            ))
            .is_err()
        );
    }
    for (line, character, expected) in [(-1, 0, 0), (2, 0, 3), (0, 4, 3), (1, 2, 4)] {
        assert_eq!(
            compute_position_of_line_and_utf16_character(&starts, line, character, text, true),
            expected
        );
    }
    assert_eq!(
        compute_position_of_line_and_utf16_character(&[0], 0, 1, b"", true),
        0
    );
    assert!(
        catch_unwind(|| compute_position_of_line_and_utf16_character(&[0], 0, 1, b"", false))
            .is_err()
    );
    assert!(
        catch_unwind(|| compute_position_of_line_and_utf16_character(&[], 0, 0, b"", true))
            .is_err()
    );
    assert_eq!(
        compute_position_of_line_and_utf16_character(&[10], 0, 0, b"", true),
        0
    );
    assert!(
        catch_unwind(|| compute_position_of_line_and_utf16_character(&[10], 0, 0, b"", false))
            .is_err()
    );
}

#[test]
fn line_and_byte_helpers_retain_their_distinct_range_rules() {
    let starts = [0, 3];
    assert_eq!(compute_line_of_position(&starts, -1), -1);
    assert_eq!(compute_line_of_position(&[], 0), -1);
    assert_eq!(compute_line_of_position(&starts, 99), 1);
    assert_eq!(position_to_line_and_byte_offset(-1, &starts), (0, -1));
    assert_eq!(position_to_line_and_byte_offset(99, &starts), (1, 96));
    assert_eq!(
        compute_position_of_line_and_byte_offset(&starts, 1, -10),
        -7
    );
    assert_eq!(
        compute_position_of_line_and_byte_offset(&starts, 1, 100),
        103
    );
    assert!(catch_unwind(|| compute_position_of_line_and_byte_offset(&starts, -1, 0)).is_err());
    assert!(catch_unwind(|| compute_position_of_line_and_byte_offset(&starts, 2, 0)).is_err());
    assert!(catch_unwind(|| position_to_line_and_byte_offset(0, &[])).is_err());
}

#[test]
fn scanner_ecma_wrappers_count_bytes_and_partial_utf8_prefixes() {
    let text = "😀\r\nb".as_bytes();
    for (position, character) in [(0, 0), (1, 1), (2, 2), (3, 3), (4, 2), (5, 3)] {
        assert_eq!(
            get_ecma_line_and_utf16_character_of_position(text, position),
            (0, character)
        );
        assert_eq!(
            get_ecma_line_and_byte_offset_of_position(text, position),
            (0, position)
        );
    }
    assert_eq!(
        get_ecma_line_and_utf16_character_of_position(text, 6),
        (1, 0)
    );
    assert_eq!(
        get_ecma_line_and_byte_offset_of_position(text, 100),
        (1, 94)
    );
    assert!(catch_unwind(|| get_ecma_line_and_utf16_character_of_position(text, 100)).is_err());
    assert!(catch_unwind(|| get_ecma_line_and_utf16_character_of_position(text, -1)).is_err());
    assert!(catch_unwind(|| get_ecma_line_and_byte_offset_of_position(text, -1)).is_err());
    assert_eq!(get_ecma_end_line_position(text, 0), 3);
    assert_eq!(get_ecma_end_line_position(text, 1), 6);
    assert_eq!(get_ecma_end_line_position(b"\n", 0), -1);
    assert_eq!(get_ecma_end_line_position(b"\n", 1), 0);
    assert_eq!(get_ecma_position_of_line_and_utf16_character(text, 0, 1), 4);
    assert_eq!(get_ecma_position_of_line_and_byte_offset(text, 0, 1), 1);
    assert_eq!(get_ecma_line_of_position(text, 6), 1);
}
