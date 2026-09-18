//! Behaviour pinned by native observations. The corpus-wide comparison is
//! `scripts/s09_format.py compare`; these keep a few of its rows under
//! `cargo test`, where no Go toolchain is needed.

use crate::{probe, FormatCodeSettings, FormatFile};
use ts_ast::SourceFileParseOptions;
use ts_jsstring::SourceText;

fn rows(
    name: &[u8],
    text: &[u8],
    kind: ts_core::ScriptKind,
    run: impl FnOnce(&mut FormatFile<'_, '_>, &mut dyn FnMut(&str)),
) -> Vec<String> {
    let file = ts_parser::parse_source_file(
        SourceText::from_loaded_bytes(text.to_vec()),
        kind,
        SourceFileParseOptions {
            file_name: ts_ast::JsString::from_bytes(name),
            path: ts_ast::JsString::from_bytes(name),
            ..Default::default()
        },
    )
    .publish_unbound();
    let mut provider = ts_parser::ParserJsDocProvider::default();
    let mut format_file = FormatFile {
        view: file.view(),
        source: file.root().unwrap(),
        jsdoc: &mut provider,
    };
    let mut out = Vec::new();
    run(&mut format_file, &mut |row| out.push(row.to_owned()));
    out
}

#[test]
fn the_formatting_scanner_rescans_where_the_container_says_so() {
    // `>=`, a regular expression, template parts, a JSX attribute value, JSX
    // text and `</` are each more than the smallest token the scanner would
    // return on its own.
    let text = b"const a = x >= 1 ? /re/g : `t${b}u`; // c\nlet v = <div id='q'>text</div>;\n";
    let rows = rows(b"/s.tsx", text, ts_core::ScriptKind::TSX, |file, row| {
        probe::scan(file, row);
    });
    let tokens: Vec<&str> = rows
        .iter()
        .filter_map(|row| row.strip_prefix("K|"))
        .map(|row| row.split('|').next().unwrap())
        .collect();
    for expected in [
        "33,12,14", "13,19,24", "15,27,31", "17,32,35", "10,58,61", "11,62,66", "30,66,68",
    ] {
        assert!(tokens.contains(&expected), "{expected} in {tokens:?}");
    }
    // The line comment and the new line after it trail the semicolon.
    assert!(rows.contains(&"K|26,35,36|26|0||5,36,37;2,37,41;4,41,42".to_owned()));
    assert_eq!(rows.last().unwrap(), "E|1,74,74|");
}

#[test]
fn rules_are_selected_in_the_pinned_order_for_adjacent_tokens() {
    let text = b"namespace Default{var x= ( { } ) ;}\n";
    let rows = rows(b"/s.ts", text, ts_core::ScriptKind::TS, |file, row| {
        probe::rules(file, FormatCodeSettings::default(), row);
    });
    assert_eq!(
        &rows[..6],
        [
            "R|145,0,9|79,10,17|268|SpaceAfterCertainTypeScriptKeywords",
            "R|79,10,17|18,17,18|268|SpaceBeforeOpenBraceInTypeScriptDeclWithBlock",
            "R|18,17,18|114,18,21|269|SpaceAfterOpenBrace",
            "R|114,18,21|79,22,23|262|SpaceAfterCertainKeywords",
            "R|79,22,23|63,23,24|261|SpaceBeforeBinaryOperator",
            "R|63,23,24|20,25,26|261|SpaceAfterBinaryOperator",
        ]
    );
}

#[test]
fn an_option_changes_which_rule_answers() {
    let text = b"f(a,b);\n";
    let pick = |settings: FormatCodeSettings| {
        rows(b"/s.ts", text, ts_core::ScriptKind::TS, |file, row| {
            probe::rules(file, settings, row);
        })
        .into_iter()
        .find(|row| row.starts_with("R|27,3,4|"))
        .unwrap()
    };
    assert!(pick(probe::variant("default").unwrap()).ends_with("|SpaceAfterComma"));
    assert!(pick(probe::variant("terse").unwrap()).ends_with("|NoSpaceAfterComma"));
}

#[test]
fn the_rules_map_has_the_pinned_shape() {
    let mut buckets = 0usize;
    let mut longest = 0usize;
    probe::rules_map(&mut |row| {
        buckets += 1;
        longest = longest.max(row.rsplit('|').next().unwrap().split(',').count());
    });
    // The native map has 27,724 non-empty buckets; the digest of their contents
    // is what the differential compares.
    assert_eq!(buckets, 27_724);
    assert!(longest > 1);
}
