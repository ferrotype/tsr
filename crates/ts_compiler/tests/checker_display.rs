#[path = "../../../tools/s08/p5/display.rs"]
mod display;

#[test]
fn context_and_builder_display_match_the_native_requests() {
    let request =
        serde_json::from_str(include_str!("../../../data/s08/p5/display/requests.json")).unwrap();
    let expected: serde_json::Value = serde_json::from_str(include_str!(
        "../../../data/s08/p5/display/observations.json"
    ))
    .unwrap();
    let result = display::observe(&request).unwrap();
    assert_eq!(result["programs"], expected["programs"]);
}

#[test]
fn alias_accessibility_matches_native_across_display_contexts() {
    let request = serde_json::from_str(include_str!(
        "../../../data/s08/p6/alias-accessibility/requests.json"
    ))
    .unwrap();
    let expected: serde_json::Value = serde_json::from_str(include_str!(
        "../../../data/s08/p6/alias-accessibility/observations.json"
    ))
    .unwrap();
    let result = display::observe(&request).unwrap();
    assert_eq!(result["programs"], expected["programs"]);
}

#[test]
fn literal_types_and_original_regenerated_printer_paths_match_native_e4() {
    let request =
        serde_json::from_str(include_str!("../../../data/s08/p5/text/requests.json")).unwrap();
    let expected: serde_json::Value =
        serde_json::from_str(include_str!("../../../data/s08/p5/text/observations.json")).unwrap();
    let result = display::observe(&request).unwrap();
    assert_eq!(result["programs"], expected["programs"]);
}

#[test]
fn deeply_qualified_symbol_display_runs_on_a_small_native_stack() {
    const DEPTH: usize = 512;
    use std::fmt::Write as _;
    let mut source = String::new();
    let mut expected = String::new();
    for index in 0..DEPTH {
        write!(source, "export namespace N{index} {{ ").unwrap();
        write!(expected, "N{index}.").unwrap();
    }
    source.push_str("export const value = 1;");
    source.push_str(&"}".repeat(DEPTH));
    expected.push_str("value");
    let request = serde_json::json!({"version":1,"programs":[{
        "id":"deep-namespaces", "files":{"/main.ts":source}, "roots":["/main.ts"],
        "queries":[{"id":"qualified", "declaration":"value", "context":"source",
                    "operation":"symbol_string", "flags":0, "meaning":ts_ast::symbol_flags::VALUE}]
    }]});
    std::thread::Builder::new()
        .stack_size(512 * 1024)
        .spawn(move || {
            let actual = display::observe(&request).unwrap();
            let mut hex = String::new();
            for byte in expected.bytes() {
                write!(hex, "{byte:02x}").unwrap();
            }
            assert_eq!(actual["programs"][0]["queries"][0]["text_hex"], hex);
        })
        .unwrap()
        .join()
        .unwrap();
}

#[test]
fn enum_member_display_through_import_type_parents_matches_native() {
    // Supplemental native capture: identifier and quoted enum members displayed
    // through an import type parent (b.ts) and a local reference parent (a.ts).
    let request = serde_json::from_str(include_str!(
        "../../../data/s08/p6/enum-member-display/requests.json"
    ))
    .unwrap();
    let expected: serde_json::Value = serde_json::from_str(include_str!(
        "../../../data/s08/p6/enum-member-display/observations.json"
    ))
    .unwrap();
    let result = display::observe(&request).unwrap();
    assert_eq!(result["programs"], expected["programs"]);
}
