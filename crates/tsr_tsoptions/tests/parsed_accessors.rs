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

fn prime_file_caches(parsed: &mut ParsedCommandLine, directory: &[u8]) {
    assert_eq!(parsed.file_names_by_path().len(), 1);
    assert_eq!(parsed.common_source_directory(), directory);
    parsed.parse_input_output_names();
    assert_eq!(parsed.source_to_project_reference().count(), 1);
    assert_eq!(parsed.output_dts_to_project_reference().count(), 1);
}

fn assert_replaced_file_caches(parsed: &mut ParsedCommandLine) {
    assert_eq!(parsed.root_file_names, [text("/new/b.ts")]);
    assert_eq!(
        parsed.file_names_by_path().values().collect::<Vec<_>>(),
        [&text("/new/b.ts")]
    );
    assert_eq!(parsed.common_source_directory(), b"/new/");
    // Both native constructors leave output maps uninitialized.
    assert_eq!(parsed.source_to_project_reference().count(), 0);
    assert_eq!(parsed.output_dts_to_project_reference().count(), 0);
    parsed.parse_input_output_names();
    let (_, output) = parsed.output_dts_to_project_reference().next().unwrap();
    assert_eq!(output.names.source, text("/new/b.ts"));
    assert_eq!(output.names.output_dts, text("/out/b.d.ts"));
}

#[test]
fn with_file_names_resets_primed_caches_without_changing_the_original() {
    // Pinned parsedcommandline.go:WithFileNames constructs a new command line;
    // only wildcardDirectories and includeGlobs survive from the old caches.
    let mut original = ParsedCommandLine::new(
        CompilerOptions {
            out_dir: text("/out"),
            ..Default::default()
        },
        vec![text("/old/a.ts")],
    );
    prime_file_caches(&mut original, b"/old/");
    assert!(original.locale().is_default());
    original.options.locale = text("fr");
    let mut replacement = original.with_file_names(Some(vec![text("/new/b.ts")]));
    assert_replaced_file_caches(&mut replacement);
    assert!(!replacement.locale().is_default());
    assert!(original.locale().is_default());
    assert_eq!(original.common_source_directory(), b"/old/");
    assert_eq!(
        original.file_names_by_path().values().collect::<Vec<_>>(),
        [&text("/old/a.ts")]
    );
    let mut empty = original.with_file_names(None);
    assert!(empty.file_names_by_path().is_empty());
    empty.parse_input_output_names();
    assert_eq!(empty.source_to_project_reference().count(), 0);
}

#[test]
fn reload_file_names_resets_primed_caches_after_filesystem_changes() {
    let mut original = ParsedCommandLine::new(
        CompilerOptions {
            out_dir: text("/out"),
            ..Default::default()
        },
        vec![text("/old/a.ts")],
    );
    original.set_config_specs(
        Some(tsr_tsoptions::ConfigFileSpecs {
            validated_includes: vec![text("**/*.ts")],
            ..Default::default()
        }),
        text("/"),
        true,
    );
    prime_file_caches(&mut original, b"/old/");
    let mut fs = tsr_vfs::MemoryBuilder::new(b"/", true);
    fs.insert_loaded(b"/new/b.ts", b"".as_slice());
    let mut reloaded = original
        .reload_file_names_of_parsed_command_line(&fs.finish())
        .unwrap();
    assert_replaced_file_caches(&mut reloaded);
    assert_eq!(original.root_file_names, [text("/old/a.ts")]);
}
