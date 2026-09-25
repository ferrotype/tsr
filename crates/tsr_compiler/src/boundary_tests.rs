//! Loader boundaries are tested after actual config interpretation.
use crate::{Error, FileCache, Program, ProgramOptions};
use std::sync::Arc;
use tsr_arena::{Counters, Counts};
use tsr_core::{CompilerOptions, Tristate};
use tsr_jsstring::JsString;
use tsr_tsoptions::{ConfigValue, ParseConfigHost, ParsedCommandLine};
use tsr_vfs::{FileSystem, MemoryBuilder};

struct Host(Arc<dyn FileSystem>);
impl ParseConfigHost for Host {
    fn fs(&self) -> &dyn FileSystem {
        self.0.as_ref()
    }
    fn current_directory(&self) -> &[u8] {
        b"/src"
    }
    fn resolve_config(
        &self,
        name: &[u8],
        containing: &[u8],
    ) -> Result<Option<JsString>, tsr_vfs::Error> {
        let result = tsr_module::resolve_config(name, containing, self.0.clone(), b"/src")
            .expect("fixture config resolution");
        Ok((!result.resolved_file_name.is_empty()).then_some(result.resolved_file_name))
    }
    fn resolve_content_mapper(
        &self,
        containing: &[u8],
        package: &[u8],
    ) -> Result<tsr_tsoptions::config_mappers::MapperResolution, tsr_vfs::Error> {
        Ok(
            tsr_module::resolve_content_mapper_manifest(&self.0, b"/src", containing, package)
                .expect("fixture mapper manifest resolution"),
        )
    }
}
fn parsed(text: &[u8], external_code: bool) -> (ParsedCommandLine, Host) {
    parsed_with(text, external_code, &[])
}
fn parsed_with(
    text: &[u8],
    external_code: bool,
    files: &[(&[u8], &[u8])],
) -> (ParsedCommandLine, Host) {
    let mut fs = MemoryBuilder::new(b"/src", true);
    fs.insert_loaded(b"/src/tsconfig.json", text);
    fs.insert_loaded(b"/src/main.ts", b"export const value = 1;".as_slice());
    for &(name, text) in files {
        fs.insert_loaded(name, text);
    }
    fs.insert_loaded(
        b"/src/node_modules/mapper/package.json",
        br#"{"name":"mapper","version":"1.0.0","typescript":{"contentMapper":{"exec":["must-never-execute"],"compilerOptions":[]}}}"#.as_slice(),
    );
    let host = Host(Arc::new(fs.finish()));
    let result = tsr_tsoptions::get_parsed_command_line_of_config_file(
        b"/src/tsconfig.json",
        &CompilerOptions {
            run_external_code: Tristate::from(external_code),
            ..CompilerOptions::default()
        },
        &ConfigValue::Null,
        &host,
    )
    .unwrap();
    assert!(result.read_errors.is_empty());
    (result.command_line.unwrap(), host)
}
fn options(config: ParsedCommandLine, host: Host) -> ProgramOptions {
    ProgramOptions {
        config,
        host: host.0,
        current_directory: JsString::from_bytes(b"/src".as_slice()),
        default_library_path: JsString::from_bytes(b"/lib".as_slice()),
        skip_module_resolution: false,
    }
}
fn load(config: ParsedCommandLine, host: Host, counters: &Counters) -> Result<Program, Error> {
    Program::load(options(config, host), &mut FileCache::new(), counters)
}

const REFERENCED: &[(&[u8], &[u8])] = &[
    (
        b"/src/referenced/tsconfig.json",
        br#"{"compilerOptions":{"composite":true}}"#,
    ),
    (b"/src/referenced/a.ts", b"export const a = 1;"),
    (b"/src/referenced/a.d.ts", b"export declare const a = 1;"),
];

#[test]
fn parsed_project_references_load_the_referenced_configs() {
    let (config, host) = parsed_with(
        br#"{"compilerOptions":{"noLib":true},"files":["main.ts","referenced/a.ts"],"references":[{"path":"./referenced"},{"path":"./missing"}]}"#,
        false,
        REFERENCED,
    );
    assert!(config.errors.is_empty());
    assert_eq!(config.project_references.as_ref().unwrap().len(), 2);
    let program = load(config, host, &Counters::new()).unwrap();
    // The referenced source is replaced by its output, which explains it.
    assert!(program.file(b"/src/referenced/a.ts").is_none());
    assert!(program.file(b"/src/referenced/a.d.ts").is_some());
    assert_eq!(
        program.source_of_project_reference_if_output_included(
            b"/src/referenced/a.d.ts",
            b"/src/referenced/a.d.ts"
        ),
        b"/src/referenced/a.ts"
    );
    let mut walk = Vec::new();
    assert!(
        program.range_resolved_project_reference(|path, config, parent, index| {
            walk.push((
                String::from_utf8(path.to_vec()).unwrap(),
                config.map(ParsedCommandLine::config_name),
                parent.config_name(),
                index,
            ));
            true
        })
    );
    let root = JsString::from_bytes(b"/src/tsconfig.json".as_slice());
    assert_eq!(
        walk,
        [
            (
                "/src/referenced/tsconfig.json".to_owned(),
                Some(JsString::from_bytes(
                    b"/src/referenced/tsconfig.json".as_slice()
                )),
                root.clone(),
                0
            ),
            ("/src/missing/tsconfig.json".to_owned(), None, root, 1),
        ]
    );
    let diagnostics = &program.option_verification().diagnostics;
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0].code, tsr_diagnostics::File_0_not_found.code);
    // A stopped walk reports false.
    assert!(!program.range_resolved_project_reference(|_, _, _, _| false));
}

#[test]
fn source_of_project_reference_mode_with_outputs_is_an_explicit_boundary() {
    let text = br#"{"compilerOptions":{"noLib":true},"files":["main.ts"],"references":[{"path":"./referenced"}]}"#;
    let (config, host) = parsed_with(text, false, REFERENCED);
    let counters = Counters::new();
    assert!(matches!(
        Program::load_with_source_of_project_reference(
            options(config, host),
            true,
            &mut FileCache::new(),
            &counters,
        ),
        Err(Error::Unsupported(
            "project-reference source redirection (newProjectReferenceDtsFakingHost)"
        ))
    ));
    assert_eq!(counters.snapshot(), Counts::default());

    // Disabling the redirect makes the requested mode load the outputs.
    let text = br#"{"compilerOptions":{"noLib":true,"disableSourceOfProjectReferenceRedirect":true},"files":["main.ts","referenced/a.ts"],"references":[{"path":"./referenced"}]}"#;
    let (config, host) = parsed_with(text, false, REFERENCED);
    let program = Program::load_with_source_of_project_reference(
        options(config, host),
        true,
        &mut FileCache::new(),
        &Counters::new(),
    )
    .unwrap();
    assert!(program.file(b"/src/referenced/a.d.ts").is_some());
    assert!(program.file(b"/src/referenced/a.ts").is_none());

    // A requested mode whose references have no outputs loads the sources as
    // written and records no output-to-source map.
    let text = br#"{"compilerOptions":{"noLib":true},"files":["main.ts","referenced/a.ts"],"references":[{"path":"./missing"}]}"#;
    let (config, host) = parsed_with(text, false, REFERENCED);
    let program = Program::load_with_source_of_project_reference(
        options(config, host),
        true,
        &mut FileCache::new(),
        &Counters::new(),
    )
    .unwrap();
    assert!(program.file(b"/src/referenced/a.ts").is_some());
}

#[test]
fn source_of_project_reference_mode_skips_checking_referenced_sources() {
    // A reference whose only source is a declaration file has no outputs, so
    // the requested source mode loads; its source is then not type checked.
    let files: &[(&[u8], &[u8])] = &[
        (
            b"/src/types/tsconfig.json",
            br#"{"compilerOptions":{"composite":true}}"#,
        ),
        (b"/src/types/globals.d.ts", b"declare const g: number;"),
    ];
    let text = br#"{"compilerOptions":{"noLib":true},"files":["main.ts","types/globals.d.ts"],"references":[{"path":"./types"}]}"#;
    for (use_source, skipped) in [(true, true), (false, false)] {
        let (config, host) = parsed_with(text, false, files);
        let program = Program::load_with_source_of_project_reference(
            options(config, host),
            use_source,
            &mut FileCache::new(),
            &Counters::new(),
        )
        .unwrap();
        let file = program.file(b"/src/types/globals.d.ts").unwrap();
        assert_eq!(program.skip_type_checking(file, false).unwrap(), skipped);
        let main = program.file(b"/src/main.ts").unwrap();
        assert!(!program.skip_type_checking(main, false).unwrap());
    }
}

#[test]
fn parsed_enabled_mappers_are_rejected_and_disabled_diagnostics_are_retained() {
    let text = br#"{"compilerOptions":{"noLib":true},"files":["main.ts"],"contentMappers":[{"package":"mapper","extensions":[".custom"]}]}"#;
    let (config, host) = parsed(text, true);
    assert!(config.errors.is_empty(), "{:?}", config.errors);
    let mapper = &config.content_mappers.as_ref().unwrap()[0];
    assert_eq!(
        mapper.manifest.exec.as_ref().unwrap()[0].as_bytes(),
        b"must-never-execute"
    );
    let counters = Counters::new();
    assert!(matches!(
        load(config, host, &counters),
        Err(Error::Unsupported("content-mapper execution"))
    ));
    assert_eq!(counters.snapshot(), Counts::default());

    let (config, host) = parsed(text, false);
    assert!(config.content_mappers.is_none());
    assert!(config.errors.iter().any(|diagnostic| diagnostic.code == tsr_diagnostics::Content_mappers_require_the_runExternalCode_command_line_flag_to_be_enabled.code));
    let diagnostics = config.errors.clone();
    let program = load(config, host, &counters).unwrap();
    assert_eq!(program.config().errors, diagnostics);
    assert!(program.file(b"/src/main.ts").is_some());
}

#[test]
fn empty_references_and_mappers_preserve_normal_loading() {
    let (mut config, host) = parsed(
        br#"{"compilerOptions":{"noLib":true},"files":["main.ts"],"references":[],"contentMappers":[]}"#,
        true,
    );
    assert!(config.errors.is_empty());
    assert!(config.project_references.as_ref().unwrap().is_empty());
    config.content_mappers = Some(Vec::new());
    let program = load(config, host, &Counters::new()).unwrap();
    assert!(program.file(b"/src/main.ts").is_some());
}
