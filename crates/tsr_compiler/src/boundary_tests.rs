//! Unsupported loader requests are tested after actual config interpretation.
use crate::{Error, FileCache, Program, ProgramOptions};
use std::sync::Arc;
use ts_arena::{Counters, Counts};
use ts_core::{CompilerOptions, Tristate};
use ts_jsstring::JsString;
use ts_tsoptions::{ConfigValue, ParseConfigHost, ParsedCommandLine};
use ts_vfs::{FileSystem, MemoryBuilder};

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
    ) -> Result<Option<JsString>, ts_vfs::Error> {
        let result = ts_module::resolve_config(name, containing, self.0.clone(), b"/src")
            .expect("fixture config resolution");
        Ok((!result.resolved_file_name.is_empty()).then_some(result.resolved_file_name))
    }
    fn resolve_content_mapper(
        &self,
        containing: &[u8],
        package: &[u8],
    ) -> Result<ts_tsoptions::config_mappers::MapperResolution, ts_vfs::Error> {
        Ok(
            ts_module::resolve_content_mapper_manifest(&self.0, b"/src", containing, package)
                .expect("fixture mapper manifest resolution"),
        )
    }
}
fn parsed(text: &[u8], external_code: bool) -> (ParsedCommandLine, Host) {
    let mut fs = MemoryBuilder::new(b"/src", true);
    fs.insert_loaded(b"/src/tsconfig.json", text);
    fs.insert_loaded(b"/src/main.ts", b"export const value = 1;".as_slice());
    fs.insert_loaded(
        b"/src/node_modules/mapper/package.json",
        br#"{"name":"mapper","version":"1.0.0","typescript":{"contentMapper":{"exec":["must-never-execute"],"compilerOptions":[]}}}"#.as_slice(),
    );
    let host = Host(Arc::new(fs.finish()));
    let result = ts_tsoptions::get_parsed_command_line_of_config_file(
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
fn load(config: ParsedCommandLine, host: Host, counters: &Counters) -> Result<Program, Error> {
    Program::load(
        ProgramOptions {
            config,
            host: host.0,
            current_directory: JsString::from_bytes(b"/src".as_slice()),
            default_library_path: JsString::from_bytes(b"/lib".as_slice()),
            skip_module_resolution: false,
        },
        &mut FileCache::new(),
        counters,
    )
}

#[test]
fn parsed_project_references_are_rejected_before_loading_files() {
    let (config, host) = parsed(
        br#"{"compilerOptions":{"noLib":true},"files":["main.ts"],"references":[{"path":"./referenced"}]}"#,
        false,
    );
    assert!(config.errors.is_empty());
    assert_eq!(config.project_references.as_ref().unwrap().len(), 1);
    let counters = Counters::new();
    assert!(matches!(
        load(config, host, &counters),
        Err(Error::Unsupported("project-reference program loading"))
    ));
    assert_eq!(counters.snapshot(), Counts::default());
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
    assert!(config.errors.iter().any(|diagnostic| diagnostic.code == ts_diagnostics::Content_mappers_require_the_runExternalCode_command_line_flag_to_be_enabled.code));
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
