use super::*;
use tsr_core::CancellationToken;
use tsr_jsstring::PositionEncoding;

fn ranges(text: &str) -> Vec<(u32, u32, u32, u32)> {
    let program = crate::tests::program(b"/index.ts", text.as_bytes());
    let mut service =
        LanguageService::new(&program, PositionEncoding::Utf16, CancellationToken::new());
    let result = service
        .folding_ranges(
            &lsp::DocumentUri("file:///index.ts".into()),
            FoldingOptions::default(),
        )
        .unwrap();
    result
        .folding_ranges
        .unwrap()
        .iter()
        .flatten()
        .map(|r| {
            (
                r.start_line,
                **r.start_character.as_ref().unwrap(),
                r.end_line,
                **r.end_character.as_ref().unwrap(),
            )
        })
        .collect()
}

#[test]
fn corrupted_try_preserves_recovered_outline_spans() {
    // Exact native fixture with fourslash range markers removed.
    let text = "try {\n  var x = [\n    {% try %}{% except %} \n  ]\n} catch (e) {\n  \n}";
    assert_eq!(
        ranges(text),
        [(0, 3, 2, 13), (2, 10, 2, 10), (2, 13, 2, 25), (4, 11, 6, 1)]
    );
}

#[test]
fn valid_try_catch_finally_preserves_brace_ranges() {
    assert_eq!(
        ranges("try {\n} catch(e) {\n} finally {\n}"),
        [(0, 3, 1, 1), (1, 10, 2, 1), (2, 9, 3, 1)]
    );
}
