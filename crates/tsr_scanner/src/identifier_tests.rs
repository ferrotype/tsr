use super::{get_identifier_token, get_viable_keyword_suggestions, keyword, KEYWORDS};
use crate::identifier_to_keyword_kind;
use ts_ast::{IdentifierData, SyntaxKind};
use ts_jsstring::JsString;

fn table_kind(bytes: &[u8]) -> SyntaxKind {
    KEYWORDS
        .iter()
        .find_map(|&(text, kind)| (text.as_bytes() == bytes).then_some(kind))
        .map_or(SyntaxKind::Unknown, |kind| {
            SyntaxKind::from_u16(kind).unwrap()
        })
}

#[test]
fn keyword_match_agrees_with_table_for_keys_and_byte_near_misses() {
    for &(text, _) in KEYWORDS {
        let bytes = text.as_bytes();
        assert_eq!(keyword(bytes), table_kind(bytes));
        for end in 0..bytes.len() {
            assert_eq!(keyword(&bytes[..end]), table_kind(&bytes[..end]));
            assert_eq!(keyword(&bytes[end..]), table_kind(&bytes[end..]));
        }
        for suffix in [0, b'_', b'x', 0xff] {
            let mut changed = bytes.to_vec();
            changed.push(suffix);
            assert_eq!(keyword(&changed), table_kind(&changed));
        }
        for index in 0..bytes.len() {
            for replacement in 0..=u8::MAX {
                let mut changed = bytes.to_vec();
                changed[index] = replacement;
                assert_eq!(keyword(&changed), table_kind(&changed), "{changed:?}");
            }
        }
    }
}

#[test]
fn keyword_utilities_keep_filtered_unknown_and_suggestion_contracts() {
    for bytes in KEYWORDS.iter().map(|&(text, _)| text.as_bytes()).chain([
        b"".as_slice(),
        b"a",
        b"IF",
        b"While",
        b"constructor_with_suffix",
        b"\xffif",
        b"if\0",
    ]) {
        let raw_kind = table_kind(bytes);
        assert_eq!(
            identifier_to_keyword_kind(&IdentifierData {
                text: JsString::from_bytes(bytes),
            }),
            raw_kind
        );
        let filtered = if raw_kind == SyntaxKind::Unknown {
            SyntaxKind::Identifier
        } else {
            raw_kind
        };
        assert_eq!(get_identifier_token(bytes), filtered);
    }
    let expected: Vec<_> = KEYWORDS
        .iter()
        .filter_map(|&(text, _)| (text.len() > 2).then_some(text))
        .collect();
    assert_eq!(get_viable_keyword_suggestions(), expected);
}
