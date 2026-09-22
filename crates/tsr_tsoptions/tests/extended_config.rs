use std::sync::Arc;
use tsr_core::CompilerOptions;
use tsr_jsstring::JsString;
use tsr_tsoptions::{
    config_mappers::MapperResolution, ConfigValue, ExtendedConfigCache, ParseConfigHost,
};
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
        panic!("fixture uses relative configs")
    }
    fn resolve_content_mapper(&self, _: &[u8], _: &[u8]) -> Result<MapperResolution, Error> {
        panic!("fixture has no mappers")
    }
}
fn host(files: &[(&str, &str)]) -> Host {
    let mut fs = MemoryBuilder::new(b"/", true);
    for (name, content) in files {
        fs.insert_loaded(name.as_bytes(), content.as_bytes());
    }
    Host(fs.finish())
}
#[test]
fn cache_reuses_syntax_but_never_substitutes_the_callers_config_dir_into_it() {
    let host = host(&[
        (
            "/base/base.json",
            r#"{"compilerOptions":{"paths":{"alias":["${configDir}/src"]},"strict":"wrong"}}"#,
        ),
        (
            "/one/tsconfig.json",
            r#"{"extends":"../base/base.json","files":["a.ts"]}"#,
        ),
        (
            "/two/tsconfig.json",
            r#"{"extends":"../base/base.json","files":["a.ts"]}"#,
        ),
    ]);
    let cache = ExtendedConfigCache::new(&host);
    let read = |name: &[u8]| {
        cache
            .read_config_file(name, &CompilerOptions::default(), &ConfigValue::Null)
            .unwrap()
            .command_line
            .unwrap()
    };
    let first = read(b"/one/tsconfig.json");
    let second = read(b"/two/tsconfig.json");
    assert_eq!(
        first
            .options
            .paths
            .as_ref()
            .unwrap()
            .get(b"alias".as_slice())
            .unwrap()
            .as_ref()
            .unwrap()[0]
            .as_bytes(),
        b"/one/src"
    );
    assert_eq!(
        second
            .options
            .paths
            .as_ref()
            .unwrap()
            .get(b"alias".as_slice())
            .unwrap()
            .as_ref()
            .unwrap()[0]
            .as_bytes(),
        b"/two/src"
    );
    let first_error = first.errors.iter().find(|d| d.code == 5024).unwrap();
    let second_error = second.errors.iter().find(|d| d.code == 5024).unwrap();
    assert_eq!(first_error.file, second_error.file);
    let a = cache
        .get_extended_config(
            b"/base/base.json",
            JsString::from_bytes(b"/base/base.json".as_slice()),
            &[],
        )
        .unwrap();
    let b = cache
        .get_extended_config(
            b"/base/base.json",
            JsString::from_bytes(b"/base/base.json".as_slice()),
            &[],
        )
        .unwrap();
    assert!(Arc::ptr_eq(&a, &b));
    drop(cache);
    // A published command line retains the source behind cached diagnostics.
    let owner = second
        .config_dependencies
        .iter()
        .find(|source| Some(source.root) == second_error.file)
        .unwrap();
    assert!(owner.file.view().node(second_error.file.unwrap()).is_ok());
}
#[test]
fn cycles_bypass_cached_initialization_and_diamond_names_are_sorted_and_unique() {
    let host = host(&[
        (
            "/root.json",
            r#"{"extends":["./z.json","./a.json"],"files":["file.ts"]}"#,
        ),
        ("/z.json", r#"{"extends":"./a.json"}"#),
        ("/a.json", r#"{"extends":"./root.json"}"#),
    ]);
    let cache = ExtendedConfigCache::new(&host);
    let parsed = cache
        .read_config_file(
            b"/root.json",
            &CompilerOptions::default(),
            &ConfigValue::Null,
        )
        .unwrap()
        .command_line
        .unwrap();
    assert!(parsed.errors.iter().any(|d| d.code == 18000));
    let names = &parsed.config_file.unwrap().extended_source_files;
    assert_eq!(
        names.iter().map(JsString::as_bytes).collect::<Vec<_>>(),
        [b"/a.json".as_slice(), b"/root.json", b"/z.json"]
    );
}
