//! F5a diagnostic composition observation. A difference is data, not a passing
//! locale integration claim; the Python comparator uses the pinned Go catalog.
use tsr_core::CompilerOptions;
use tsr_jsstring::{JsString, SourceText};
use tsr_tsoptions::{ConfigValue, ParseConfigHost, TsConfigSourceFile};
use tsr_vfs::{Error, FileSystem, MemoryBuilder, MemorySnapshot};

struct Host(MemorySnapshot);
impl ParseConfigHost for Host {
    fn fs(&self) -> &dyn FileSystem {
        &self.0
    }
    fn current_directory(&self) -> &[u8] {
        b"/"
    }
    fn resolve_config(&self, _: &[u8], _: &[u8]) -> Result<Option<JsString>, Error> {
        panic!("integration fixture has no config inheritance")
    }
    fn resolve_content_mapper(
        &self,
        _: &[u8],
        _: &[u8],
    ) -> Result<tsr_tsoptions::config_mappers::MapperResolution, Error> {
        panic!("integration fixture has no content mapper")
    }
}
fn main() {
    let mut files = MemoryBuilder::new(b"/", true);
    files.insert_loaded(b"/main.ts", b"".as_slice());
    let host = Host(files.finish());
    let name = JsString::from_bytes(b"/tsconfig.json".as_slice());
    let config = tsr_tsoptions::parse_json_source_file_config_file_content(
        TsConfigSourceFile::parse(
            name.clone(),
            name,
            SourceText::from_loaded_bytes(
                br#"{"compilerOptions":{"notAnOption":true},"files":["main.ts"]}"#.as_slice(),
            ),
        ),
        &host,
        b"/",
        &CompilerOptions {
            locale: JsString::from_bytes(b"de-DE".as_slice()),
            ..Default::default()
        },
        &ConfigValue::Null,
        b"/tsconfig.json",
    )
    .unwrap();
    let errors = config.config_file_parsing_diagnostics();
    assert_eq!(errors.len(), 1, "fixture diagnostic schedule changed");
    let diagnostic = &errors[0];
    assert_eq!(diagnostic.code, 5023);
    let args: Vec<_> = diagnostic
        .message_args
        .iter()
        .map(JsString::as_bytes)
        .collect();
    let leaf = tsr_diagnostics::localize(
        config.locale(),
        diagnostic.message,
        diagnostic.message_key.as_bytes(),
        &args,
    );
    let writer = tsr_compiler::diagnostic_writer::localized(diagnostic).unwrap();
    println!(
        "{}",
        serde_json::json!({
            "id": "localized-config-diagnostics",
            "locale": config.locale().tag_string(),
            "code": diagnostic.code,
            "leaf_localized": String::from_utf8(leaf).unwrap(),
            "writer_localized": String::from_utf8(writer).unwrap(),
        })
    );
}
