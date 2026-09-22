use tsr_core::CompilerOptions;
use tsr_jsstring::JsString;
use tsr_tsoptions::{ParsedCommandLine, ParsedOptions};
fn text(value: &str) -> JsString {
    JsString::from_bytes(value.as_bytes())
}

#[test]
fn first_use_caches_survive_options_replacement_and_clones_keep_owned_inputs() {
    let mut parsed = ParsedCommandLine::new(
        CompilerOptions {
            locale: text("fr"),
            ..Default::default()
        },
        vec![text("a.ts")],
    );
    assert!(!parsed.locale().is_default());
    assert_eq!(
        parsed.file_names_by_path().values().next(),
        Some(&text("a.ts"))
    );
    let mut cloned = parsed.clone();
    cloned.set_parsed_options(ParsedOptions {
        file_names: vec![text("b.ts")],
        ..Default::default()
    });
    // Native setters replace ParsedConfig, never the once caches. This is a
    // stale-by-contract result, not a second cache with its own invalidation rule.
    assert!(!cloned.locale().is_default());
    assert_eq!(
        cloned.file_names_by_path().values().next(),
        Some(&text("a.ts"))
    );
    assert_eq!(cloned.root_file_names, [text("b.ts")]);
    assert_eq!(parsed.root_file_names, [text("a.ts")]);
    parsed.set_compiler_options(CompilerOptions::default());
    assert!(!parsed.locale().is_default());
}

#[test]
fn output_maps_share_names_without_self_cycles_and_diagnose_root_once() {
    let mut parsed = ParsedCommandLine::new(
        CompilerOptions {
            root_dir: text("/src"),
            out_dir: text("/out"),
            ..Default::default()
        },
        vec![
            text("/outside/a.ts"),
            text("/src/b.d.ts"),
            text("/src/data.json"),
        ],
    );
    parsed.parse_input_output_names();
    assert_eq!(parsed.errors.len(), 1);
    assert_eq!(parsed.errors[0].code, 6059);
    parsed.parse_input_output_names();
    assert_eq!(parsed.common_source_directory(), b"/src/");
    assert_eq!(parsed.errors.len(), 1);
    let (_, source) = parsed
        .source_to_project_reference()
        .find(|(_, item)| item.names.source.as_bytes() == b"/outside/a.ts")
        .unwrap();
    let (_, output) = parsed.output_dts_to_project_reference().next().unwrap();
    assert!(std::ptr::eq(source.names, output.names));
    assert!(std::ptr::eq(source.resolved, &raw const parsed));
    assert_eq!(parsed.source_to_project_reference().count(), 3);
    assert_eq!(parsed.output_dts_to_project_reference().count(), 1);
    // Only declaration/JSON inputs never invoke the lazy common-directory
    // worker, even if an output directory is configured.
    let mut declarations = ParsedCommandLine::new(
        CompilerOptions {
            root_dir: text("/src"),
            out_dir: text("/out"),
            ..Default::default()
        },
        vec![text("/outside/data.json")],
    );
    declarations.parse_input_output_names();
    assert!(declarations.errors.is_empty());
}
