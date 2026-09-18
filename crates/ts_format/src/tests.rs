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

#[test]
fn the_smart_indenter_answers_as_the_pin_does_at_every_line_start() {
    // A parameter list, a tab-indented `if`, an object literal, an `else`
    // without a block, call arguments, a block comment, a template literal and
    // a multi-line conditional.
    let text = b"function f(a: number,\n    b: string) {\n\tif (a) {\n        return {\n            x: 1,\n        };\n    }\n    else\n        g(a,\n            b);\n    /* note\n       more\n     */\n    const s = `t\n  u`;\n}\nconst c = a\n    ? b\n    : d;\n";
    let lines = |name: &str| -> Vec<(i64, i64)> {
        let settings = probe::variant(name).unwrap();
        rows(b"/u.ts", text, ts_core::ScriptKind::TS, |file, row| {
            probe::indent(file, &settings, &[], row);
        })
        .iter()
        .filter_map(|row| {
            let fields: Vec<&str> = row.split('|').collect();
            (fields[0] == "L" && fields[2] == "0")
                .then(|| (fields[1].parse().unwrap(), fields[3].parse().unwrap()))
        })
        .collect()
    };
    let starts = [
        0, 22, 39, 49, 66, 84, 95, 101, 110, 123, 139, 151, 163, 171, 188, 194, 196, 208, 216, 225,
    ];
    // Inside the comment the answer follows the previous comment line, and
    // inside the template literal it is zero.
    let default = [
        0, 4, 4, 8, 12, 12, 8, 4, 8, 12, 4, 4, 7, 4, 0, 4, 0, 4, 4, 0,
    ];
    let two = [
        0, 2, 2, 4, 10, 10, 4, 2, 4, 10, 2, 4, 7, 2, 0, 2, 0, 2, 2, 0,
    ];
    assert_eq!(
        lines("default"),
        starts.into_iter().zip(default).collect::<Vec<_>>()
    );
    assert_eq!(
        lines("two"),
        starts.into_iter().zip(two).collect::<Vec<_>>()
    );
}

const UNFORMATTED: &[u8] = b"function f( a:number,b :string ){\nif(a){return {x:1 ,y : 2}}\n  else\n g( a,\nb ) ;   \n    /* note\n  more */\n}\nclass C<T>{ @dec()\nm( ) :void{ } }\n";

fn with_file<T>(text: &[u8], run: impl FnOnce(&mut FormatFile<'_, '_>) -> T) -> T {
    let file = ts_parser::parse_source_file(
        SourceText::from_loaded_bytes(text.to_vec()),
        ts_core::ScriptKind::TS,
        SourceFileParseOptions {
            file_name: ts_ast::JsString::from_bytes(&b"/u.ts"[..]),
            path: ts_ast::JsString::from_bytes(&b"/u.ts"[..]),
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
    run(&mut format_file)
}

#[test]
fn a_document_is_formatted_into_the_text_the_pin_produces() {
    let context = crate::FormatContext::new(FormatCodeSettings::default(), b"\n");
    let edits = with_file(UNFORMATTED, |file| crate::format_document(file, &context)).unwrap();
    assert_eq!(edits.len(), 35);
    let text = ts_core::apply_bulk_edits(UNFORMATTED, &edits).unwrap();
    // The member after the decorator keeps its column: upstream does not indent
    // the first token that follows a list of decorators.
    assert_eq!(
        String::from_utf8(text).unwrap(),
        "function f(a: number, b: string) {\n    if (a) { return { x: 1, y: 2 } }\n    else\n        g(a,\n            b);\n    /* note\n  more */\n}\nclass C<T> {\n    @dec()\nm(): void { }\n}\n"
    );
}

#[test]
fn formatting_on_enter_touches_the_previous_line_and_the_current_one() {
    let context = crate::FormatContext::new(FormatCodeSettings::default(), b"\n");
    let answer = |position: i64| -> Vec<(i64, i64, Vec<u8>)> {
        with_file(UNFORMATTED, |file| {
            crate::format_on_enter(file, &context, position)
        })
        .unwrap()
        .into_iter()
        .map(|edit| (edit.range.pos(), edit.range.end(), edit.new_text))
        .collect()
    };
    // Nothing precedes the first line.
    assert!(answer(0).is_empty());
    // Enter after `g( a,`: that line is indented as a statement of the function
    // body, since the span does not reach the `else` above it, and loses its
    // inner space; the new line gets the argument indentation.
    assert_eq!(
        answer(75),
        vec![
            (68, 69, b"    ".to_vec()),
            (71, 72, Vec::new()),
            (75, 75, b"        ".to_vec()),
            (76, 77, Vec::new()),
            (78, 79, Vec::new()),
        ]
    );
}
