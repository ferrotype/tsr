use crate::{CompletionOptions, LanguageService};
use std::{collections::BTreeMap, sync::Arc};
use tsr_core::{CancellationToken, CompilerOptions, ModuleKind, Tristate};
use tsr_jsstring::{JsString, PositionEncoding};
use tsr_lsproto as lsp;
use tsr_vfs::vfstest::{self, InputFile};
use tsr_vfs::{Entries, Error, FileContent, FileInfo, FileSystem, SnapshotId};

// The fixture is immutable. Keep iovfs path handling for every query: using a
// MemoryBuilder host here would normalize the invalid double-root path.
struct FrozenIo {
    host: Arc<dyn FileSystem>,
    identity: SnapshotId,
}
impl FileSystem for FrozenIo {
    fn snapshot_id(&self) -> Option<SnapshotId> {
        Some(self.identity)
    }
    fn use_case_sensitive_file_names(&self) -> bool {
        true
    }
    fn read_file(&self, path: &[u8]) -> Result<Option<FileContent>, Error> {
        self.host.read_file(path)
    }
    fn stat(&self, path: &[u8]) -> Result<Option<FileInfo>, Error> {
        self.host.stat(path)
    }
    fn entries(&self, path: &[u8]) -> Result<Entries, Error> {
        self.host.entries(path)
    }
    fn realpath(&self, path: &[u8]) -> Result<JsString, Error> {
        self.host.realpath(path)
    }
}

// source: tsc/internal/fourslash/tests/pathCompletionsPackageJsonImportsWildcard3_test.go:TestPathCompletionsPackageJsonImportsWildcard3
#[test]
fn package_import_wildcard_preserves_filesystem_root() {
    let text = b"import {} from \"#component-subfolder/\";";
    let mut inputs = BTreeMap::from([
        (b"/a.ts".to_vec(), InputFile::Text(text.to_vec())),
        (
            b"/package.json".to_vec(),
            InputFile::Text(
                br##"{"imports":{"#component-*":{"types@>=4.3.5":"types/components/*.d.ts"}}}"##
                    .to_vec(),
            ),
        ),
        (
            b"/types/components/index.d.ts".to_vec(),
            InputFile::Text(b"export const index = 0;".to_vec()),
        ),
    ]);
    inputs.insert(
        b"/types/components/subfolder/one.d.ts".to_vec(),
        InputFile::Text(b"export const one = 0;".to_vec()),
    );
    let fs = Arc::new(tsr_vfs::iovfs::from(
        Arc::new(vfstest::from_map(&inputs, true)),
        true,
    ));
    let identity = tsr_vfs::MemoryBuilder::new(b"/", true)
        .finish()
        .snapshot_id()
        .unwrap();
    let fs = Arc::new(FrozenIo { host: fs, identity });
    let counters = tsr_arena::Counters::new();
    let program = Arc::new(
        tsr_compiler::Program::load(
            tsr_compiler::ProgramOptions {
                config: tsr_tsoptions::ParsedCommandLine::new(
                    CompilerOptions {
                        module: ModuleKind::NODE18,
                        no_lib: Tristate::TRUE,
                        ..Default::default()
                    },
                    vec![JsString::from_bytes(b"/a.ts".as_slice())],
                ),
                host: fs.clone(),
                current_directory: JsString::from_bytes(b"/".as_slice()),
                default_library_path: JsString::from_bytes(b"/".as_slice()),
                skip_module_resolution: false,
                single_threaded: Tristate::TRUE,
            },
            &mut tsr_compiler::FileCache::new(),
            &counters,
        )
        .unwrap(),
    );
    let pool = tsr_compiler::CompilerCheckerPool::new(program.clone(), &counters);
    let mut checker = pool
        .checker_for_file_exclusive(program.source_file(b"/a.ts").unwrap().source())
        .unwrap();
    let mut service =
        LanguageService::new(&program, PositionEncoding::Utf16, CancellationToken::new());
    service.set_completion_file_system(fs);
    let result = service
        .completion(
            &mut checker,
            &lsp::CompletionParams {
                text_document: lsp::TextDocumentIdentifier {
                    uri: lsp::DocumentUri("file:///a.ts".into()),
                },
                position: lsp::Position {
                    line: 0,
                    character: (text.len() - 2) as u32,
                },
                ..Default::default()
            },
            &CompletionOptions::default(),
        )
        .unwrap();
    assert!(result
        .list
        .unwrap()
        .items
        .iter()
        .flatten()
        .any(|item| item.label == "one"));
}

// source: tsc/internal/fourslash/tests/pathCompletionsTypesVersionsWildcard3_test.go:TestPathCompletionsTypesVersionsWildcard3
#[test]
fn types_versions_wildcard_reads_files_under_node_modules() {
    use super::{Ending, MappingKind, PathOptions};
    let inputs = BTreeMap::from([
        (
            b"/node_modules/foo/dist/blah.d.ts".to_vec(),
            InputFile::Text(b"export const blah = 0;".to_vec()),
        ),
        (
            b"/node_modules/foo/dist/index.d.ts".to_vec(),
            InputFile::Text(b"export const index = 0;".to_vec()),
        ),
        (
            b"/node_modules/foo/dist/subfolder/one.d.ts".to_vec(),
            InputFile::Text(b"export const one = 0;".to_vec()),
        ),
    ]);
    let fs = Arc::new(tsr_vfs::iovfs::from(
        Arc::new(vfstest::from_map(&inputs, true)),
        true,
    ));
    let program = crate::tests::program(b"/a.ts", b"export {};");
    let mut service =
        LanguageService::new(&program, PositionEncoding::Utf16, CancellationToken::new());
    service.set_completion_file_system(fs);
    let options = PathOptions {
        extensions: vec![b".ts".to_vec(), b".d.ts".to_vec()],
        endings: vec![Ending::Minimal, Ending::Index],
        reference: false,
    };
    let entries = service
        .pattern_modules(
            b"",
            b"/node_modules/foo",
            b"dist/*",
            &options,
            MappingKind::Paths,
        )
        .unwrap();
    let names: Vec<_> = entries
        .0
        .keys()
        .map(|k| String::from_utf8_lossy(k).into_owned())
        .collect();
    assert_eq!(names, ["blah", "index", "subfolder"]);
    let entries = service
        .pattern_modules(
            b"subfolder/",
            b"/node_modules/foo",
            b"dist/*",
            &options,
            MappingKind::Paths,
        )
        .unwrap();
    let names: Vec<_> = entries
        .0
        .keys()
        .map(|k| String::from_utf8_lossy(k).into_owned())
        .collect();
    assert_eq!(names, ["one"]);
}
// source: tsc/internal/fourslash/tests/pathCompletionsTypesVersionsWildcard3_test.go:TestPathCompletionsTypesVersionsWildcard3
#[test]
fn types_versions_wildcard_completion_keeps_single_fragment_separator() {
    let cases = [
        (r#"{"browser/*":["dist/*"]}"#, "browser/", "blah"),
        (r#"{"browser/*":["dist/*"]}"#, "browser/subfolder/", "one"),
        (
            r#"{"*":["dist/*"],"foo/*":["dist/*"],"bar/*":["dist/*"]}"#,
            "",
            "blah",
        ),
        (
            r#"{"*":["dist/*"],"foo/*":["dist/*"],"bar/*":["dist/*"]}"#,
            "foo/",
            "blah",
        ),
        (
            r#"{"bar/*":["dist/*"],"foo/*":["dist/*"],"*":["dist/*"]}"#,
            "",
            "blah",
        ),
        (
            r#"{"bar/*":["dist/*"],"foo/*":["dist/*"],"*":["dist/*"]}"#,
            "foo/",
            "blah",
        ),
    ];
    for (mapping, fragment, expected) in cases {
        let text = format!("import {{}} from \"foo/{fragment}\";");
        let text = text.as_bytes();
        let mut inputs = BTreeMap::from([
            (b"/a.ts".to_vec(), InputFile::Text(text.to_vec())),
            (
                b"/node_modules/foo/package.json".to_vec(),
                InputFile::Text(
                    format!(r#"{{"types":"index.d.ts","typesVersions":{{"*":{mapping}}}}}"#)
                        .into_bytes(),
                ),
            ),
            (
                b"/node_modules/foo/dist/index.d.ts".to_vec(),
                InputFile::Text(b"export const index = 0;".to_vec()),
            ),
        ]);
        inputs.insert(
            b"/node_modules/foo/dist/blah.d.ts".to_vec(),
            InputFile::Text(b"export const blah = 0;".to_vec()),
        );
        inputs.insert(
            b"/node_modules/foo/dist/subfolder/one.d.ts".to_vec(),
            InputFile::Text(b"export const one = 0;".to_vec()),
        );
        let fs = Arc::new(tsr_vfs::iovfs::from(
            Arc::new(vfstest::from_map(&inputs, true)),
            true,
        ));
        let identity = tsr_vfs::MemoryBuilder::new(b"/", true)
            .finish()
            .snapshot_id()
            .unwrap();
        let fs = Arc::new(FrozenIo { host: fs, identity });
        let counters = tsr_arena::Counters::new();
        let program = Arc::new(
            tsr_compiler::Program::load(
                tsr_compiler::ProgramOptions {
                    config: tsr_tsoptions::ParsedCommandLine::new(
                        CompilerOptions {
                            module: ModuleKind::COMMON_JS,
                            no_lib: Tristate::TRUE,
                            ..Default::default()
                        },
                        vec![JsString::from_bytes(b"/a.ts".as_slice())],
                    ),
                    host: fs.clone(),
                    current_directory: JsString::from_bytes(b"/".as_slice()),
                    default_library_path: JsString::from_bytes(b"/".as_slice()),
                    skip_module_resolution: false,
                    single_threaded: Tristate::TRUE,
                },
                &mut tsr_compiler::FileCache::new(),
                &counters,
            )
            .unwrap(),
        );
        let pool = tsr_compiler::CompilerCheckerPool::new(program.clone(), &counters);
        let mut checker = pool
            .checker_for_file_exclusive(program.source_file(b"/a.ts").unwrap().source())
            .unwrap();
        let mut service =
            LanguageService::new(&program, PositionEncoding::Utf16, CancellationToken::new());
        service.set_completion_file_system(fs);
        let result = service
            .completion(
                &mut checker,
                &lsp::CompletionParams {
                    text_document: lsp::TextDocumentIdentifier {
                        uri: lsp::DocumentUri("file:///a.ts".into()),
                    },
                    position: lsp::Position {
                        line: 0,
                        character: (text.len() - 2) as u32,
                    },
                    ..Default::default()
                },
                &CompletionOptions::default(),
            )
            .unwrap();
        assert!(result
            .list
            .unwrap()
            .items
            .iter()
            .flatten()
            .any(|item| item.label == expected));
    }
}
