use super::*;

fn added_import(preference: tsr_format::SemicolonPreference, require: bool) -> String {
    let text = "mySym\nignoredSym";
    let file = tsr_parser::parse_source_file(
        tsr_jsstring::SourceText::from_loaded_bytes(text.as_bytes().to_vec()),
        tsr_core::ScriptKind::TS,
        tsr_ast::SourceFileParseOptions {
            file_name: tsr_jsstring::JsString::from_bytes(b"/main.ts".as_slice()),
            path: tsr_jsstring::JsString::from_bytes(b"/main.ts".as_slice()),
            ..Default::default()
        },
    )
    .publish_unbound();
    let mut adder = ImportAdder::default();
    adder.add(
        lsp::AutoImportFix {
            kind: lsp::AutoImportFixKind::ADD_NEW,
            import_kind: lsp::ImportKind::NAMED,
            module_specifier: "./foo".into(),
            name: "mySymbol".into(),
            use_require: require,
            ..Default::default()
        },
        false,
    );
    let options = tsr_format::FormatCodeSettings {
        semicolons: preference,
        ..Default::default()
    };
    let changes = adder
        .edits(
            file.view(),
            file.root().unwrap(),
            &Options {
                format: &options,
                locale: &tsr_locale::DEFAULT,
                single_quote: false,
                semicolons: false,
                prefer_type_only: false,
                verbatim: false,
                newline: "\n",
                usage: None,
                specifiers: &crate::edits::SpecifierPreferences::default(),
            },
        )
        .unwrap();
    let mut result = text.to_owned();
    for edit in changes.into_iter().rev() {
        result.replace_range(edit.start as usize..edit.end as usize, &edit.text);
    }
    result
}

// source: tsc/internal/fourslash/tests/autoImportFileExcludePatterns_test.go:TestAutoImportFileExcludePatterns
#[test]
fn auto_detected_no_semicolons_keeps_generated_import_terminator() {
    assert_eq!(
        added_import(tsr_format::SemicolonPreference::Ignore, false),
        "import { mySymbol } from \"./foo\";\n\nmySym\nignoredSym"
    );
    assert_eq!(
        added_import(tsr_format::SemicolonPreference::Ignore, true),
        "const { mySymbol } = require(\"./foo\");\n\nmySym\nignoredSym"
    );
}

#[test]
fn explicit_semicolon_removal_formats_generated_import() {
    assert_eq!(
        added_import(tsr_format::SemicolonPreference::Remove, false),
        "import { mySymbol } from \"./foo\"\n\nmySym\nignoredSym"
    );
}
