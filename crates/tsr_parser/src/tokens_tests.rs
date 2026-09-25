//! The pinned isKeywordOrPunctuation table. Every row comes from the pinned
//! parser package (tools/phase1/parser-tokens): each Kind from -1 through
//! KindCount+1. Its pinned callers are assertion guards only, so no parse can
//! observe a wrong answer; this table is the operation's witness.
use super::is_keyword_or_punctuation;
use tsr_ast::SyntaxKind;

#[path = "testdata/keyword_or_punctuation.rs"]
mod table;

#[test]
fn keyword_or_punctuation_matches_the_pinned_kind_table() {
    let count = i64::try_from(SyntaxKind::COUNT).unwrap();
    let kinds: Vec<i64> = table::TABLE.iter().map(|&(kind, _, _)| kind).collect();
    assert_eq!(kinds, (-1..=count + 1).collect::<Vec<_>>());
    for &(kind, name, expected) in table::TABLE {
        match u16::try_from(kind).ok().and_then(SyntaxKind::from_u16) {
            Some(syntax) => {
                // The Go name pins the numbering the Rust enum must share.
                assert_eq!(format!("Kind{}", syntax.as_str()), name, "kind {kind}");
                assert_eq!(is_keyword_or_punctuation(syntax), expected, "{name}");
            }
            // Kind(-1), KindCount and Kind(KindCount+1) have no SyntaxKind, so
            // the Rust signature cannot receive them; the pin answers false.
            None => assert!(!expected && (kind < 0 || kind >= count), "{name}"),
        }
    }
}
