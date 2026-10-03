//! The pinned package's own tests (`buildinfo_contentmapper_test.go` and
//! `external_diagnostic_test.go`).
use crate::build_info::{content_mapper_identities, BuildInfo};
use crate::host::CompilerHost;
use crate::incremental::{read_build_info_program, BuildInfoReader};
use crate::program_to_snapshot::ast_diag_to_build_info_diag;
use std::sync::Arc;
use tsr_contentmapper::{OptionDiagnostic, Project, Request, TransformResult};
use tsr_core::{CompilerOptions, JsxEmit};
use tsr_jsstring::JsString;
use tsr_tsoptions::config_mappers::{ContentMapper, MapperManifest};
use tsr_tsoptions::ParsedCommandLine;

fn js(text: &str) -> JsString {
    JsString::from_bytes(text.as_bytes())
}

// source: tsc/internal/execute/incremental/buildinfo_contentmapper_test.go:configWithMappers
fn config_with_mappers(mappers: Vec<ContentMapper>) -> ParsedCommandLine {
    let mut config = ParsedCommandLine::new(CompilerOptions::default(), Vec::new());
    config.content_mappers = Some(mappers);
    config
}

// source: tsc/internal/execute/incremental/buildinfo_contentmapper_test.go:TestStaticContentMapperTransformIdentity
#[test]
fn static_content_mapper_transform_identity() {
    let named = ContentMapper {
        manifest: MapperManifest {
            name: js("vue"),
            version: js("2.0.0"),
            ..MapperManifest::default()
        },
        ..ContentMapper::default()
    };
    assert_eq!(tsr_contentmapper::identity(&named), js("vue@2.0.0"));
    let anonymous = ContentMapper {
        package: js("anon"),
        ..ContentMapper::default()
    };
    assert_eq!(tsr_contentmapper::identity(&anonymous), js(""));

    let jsx_mapper = ContentMapper {
        package: js("jsx"),
        manifest: MapperManifest {
            name: js("jsx"),
            version: js("1.0.0"),
            compiler_options: Some(vec![js("jsx")]),
            ..MapperManifest::default()
        },
        ..ContentMapper::default()
    };
    let jsx_preserve_identity = tsr_contentmapper::transform_identity(
        &jsx_mapper,
        Some(&CompilerOptions {
            jsx: JsxEmit::PRESERVE,
            ..CompilerOptions::default()
        }),
    );
    let jsx_react_identity = tsr_contentmapper::transform_identity(
        &jsx_mapper,
        Some(&CompilerOptions {
            jsx: JsxEmit::REACT,
            ..CompilerOptions::default()
        }),
    );
    assert_ne!(jsx_preserve_identity, jsx_react_identity);

    let options_a = ContentMapper {
        package: js("vue"),
        options: Some(br#"{"mode":"a"}"#.to_vec()),
        manifest: MapperManifest {
            name: js("vue"),
            version: js("1.0.0"),
            ..MapperManifest::default()
        },
        ..ContentMapper::default()
    };
    let options_b = ContentMapper {
        options: Some(br#"{"mode":"b"}"#.to_vec()),
        ..options_a.clone()
    };
    assert_ne!(
        tsr_contentmapper::transform_identity(&options_a, Some(&CompilerOptions::default())),
        tsr_contentmapper::transform_identity(&options_b, Some(&CompilerOptions::default()))
    );
}

// source: tsc/internal/execute/incremental/buildinfo_contentmapper_test.go:fakeBuildInfoReader
struct FakeBuildInfoReader {
    build_info: BuildInfo,
}

impl BuildInfoReader for FakeBuildInfoReader {
    // source: tsc/internal/execute/incremental/buildinfo_contentmapper_test.go:fakeBuildInfoReader.ReadBuildInfo
    fn read_build_info(&self, _: &ParsedCommandLine) -> Option<BuildInfo> {
        Some(self.build_info.clone())
    }
}

// source: tsc/internal/execute/incremental/buildinfo_contentmapper_test.go:fakeContentMapperProject
struct FakeContentMapperProject {
    identities: Vec<String>,
    err: Option<String>,
}

impl Project for FakeContentMapperProject {
    fn refresh(&self) -> Result<(), tsr_contentmapper::Error> {
        Ok(())
    }
    fn identities(&self) -> Result<Vec<String>, tsr_contentmapper::Error> {
        match &self.err {
            Some(err) => Err(tsr_contentmapper::Error::Message(err.clone())),
            None => Ok(self.identities.clone()),
        }
    }
    fn identity(&self, _: usize) -> Result<String, tsr_contentmapper::Error> {
        Ok(String::new())
    }
    fn watched_files(&self) -> Result<Vec<String>, tsr_contentmapper::Error> {
        Ok(Vec::new())
    }
    fn diagnostics(&self) -> Vec<OptionDiagnostic> {
        Vec::new()
    }
    fn transform(
        &self,
        _: usize,
        _: &Request,
    ) -> Result<TransformResult, tsr_contentmapper::Error> {
        Ok(TransformResult::default())
    }
    fn close(&self) -> Result<(), tsr_contentmapper::Error> {
        Ok(())
    }
}

/// The pin's `compiler.NewCompilerHost("/", vfstest.FromMap(nil, true), "", nil, nil, project)`.
struct TestCompilerHost {
    fs: Arc<dyn tsr_vfs::FileSystem>,
    project: Option<Arc<dyn Project>>,
}

impl TestCompilerHost {
    fn new(project: Arc<dyn Project>) -> Self {
        Self {
            fs: Arc::new(tsr_vfs::MemoryBuilder::new(b"/", true).finish()),
            project: Some(project),
        }
    }
}

impl CompilerHost for TestCompilerHost {
    fn fs(&self) -> &dyn tsr_vfs::FileSystem {
        self.fs.as_ref()
    }
    fn default_library_path(&self) -> &[u8] {
        b""
    }
    fn get_current_directory(&self) -> &[u8] {
        b"/"
    }
    fn content_mapper_project(&self) -> Option<&Arc<dyn Project>> {
        self.project.as_ref()
    }
}

// source: tsc/internal/execute/incremental/buildinfo_contentmapper_test.go:TestDynamicContentMapperIdentities
#[test]
fn dynamic_content_mapper_identities() {
    let config = config_with_mappers(vec![ContentMapper {
        package: js("dynamic"),
        manifest: MapperManifest {
            name: js("dynamic"),
            version: js("1.0.0"),
            dynamic_config: true,
            ..MapperManifest::default()
        },
        ..ContentMapper::default()
    }]);
    let project = Arc::new(FakeContentMapperProject {
        identities: vec!["dynamic@1.0.0:opaque".to_owned()],
        err: None,
    });
    let identities =
        content_mapper_identities(Some(project.as_ref())).expect("the project's identities");
    assert_eq!(identities, Some(vec![js("dynamic@1.0.0:opaque")]));

    let build_info = BuildInfo {
        version: js(tsr_core::version()),
        file_names: Some(vec![js("/src/a.ts")]),
        content_mapper_identities: Some(vec![js("dynamic@1.0.0:old")]),
        ..BuildInfo::default()
    };
    let host = TestCompilerHost::new(project);
    let program = read_build_info_program(&config, &FakeBuildInfoReader { build_info }, &host);
    assert!(
        program.is_none(),
        "expected opaque mapper identity changes to discard the old program"
    );
}

// source: tsc/internal/execute/incremental/buildinfo_contentmapper_test.go:TestContentMapperIdentityError
#[test]
fn content_mapper_identity_error() {
    let project = FakeContentMapperProject {
        identities: Vec::new(),
        err: Some("identity failed".to_owned()),
    };
    let result = content_mapper_identities(Some(&project));
    match result {
        Err(tsr_contentmapper::Error::Message(message)) => assert_eq!(message, "identity failed"),
        other => panic!("expected the project's error, got {other:?}"),
    }
}

// source: tsc/internal/execute/incremental/buildinfo_contentmapper_test.go:TestReadBuildInfoProgramContentMapperIdentityMismatch
#[test]
fn read_build_info_program_content_mapper_identity_mismatch() {
    // An otherwise-valid, incremental build info whose recorded mapper identity differs from the current
    // project cannot be reused: the old program is discarded (None) so the project is rebuilt.
    let build_info = BuildInfo {
        version: js(tsr_core::version()),
        file_names: Some(vec![js("/src/a.ts")]),
        content_mapper_identities: Some(vec![js("vue@1.0.0")]),
        ..BuildInfo::default()
    };
    let config = config_with_mappers(vec![ContentMapper {
        package: js("vue"),
        extensions: vec![js(".vue")],
        manifest: MapperManifest {
            name: js("vue"),
            version: js("2.0.0"),
            ..MapperManifest::default()
        },
        ..ContentMapper::default()
    }]);
    let project = Arc::new(FakeContentMapperProject {
        identities: vec!["vue@2.0.0:current".to_owned()],
        err: None,
    });
    let host = TestCompilerHost::new(project);

    let program = read_build_info_program(&config, &FakeBuildInfoReader { build_info }, &host);
    assert!(
        program.is_none(),
        "expected the old program to be discarded when the mapper identity changed"
    );
}

// source: tsc/internal/execute/incremental/external_diagnostic_test.go:TestExternalDiagnosticBuildInfoRoundTrip
#[test]
fn external_diagnostic_build_info_round_trip() {
    // The pin parses `/app.vue` on its own; a diagnostic's file here is a
    // program file's id, so the file is a program's only root.
    let mut files = tsr_vfs::MemoryBuilder::new(b"/", true);
    files.insert_loaded(b"/app.ts", b"".as_slice());
    let program = Arc::new(
        tsr_compiler::Program::load(
            tsr_compiler::ProgramOptions {
                config: ParsedCommandLine::new(
                    CompilerOptions {
                        no_lib: tsr_core::Tristate::TRUE,
                        ..CompilerOptions::default()
                    },
                    vec![js("/app.ts")],
                ),
                host: Arc::new(files.finish()),
                current_directory: js("/"),
                default_library_path: JsString::from_bytes(tsr_bundled::LIB_PATH),
                skip_module_resolution: false,
                single_threaded: tsr_core::Tristate::TRUE,
            },
            &mut tsr_compiler::FileCache::new(),
            &tsr_arena::Counters::new(),
        )
        .expect("the program loads"),
    );
    let file = program.files()[0].source();
    let diagnostic = tsr_ast::Diagnostic::external(
        Some(file),
        tsr_core::TextRange::new(1, 2),
        js("vue"),
        tsr_diagnostics::Category::Warning as i32,
        1001,
        js("mapper warning"),
    );

    let serialized = ast_diag_to_build_info_diag(&diagnostic);
    assert_eq!(serialized.source, js("vue"));
    assert_eq!(serialized.message_text, js("mapper warning"));

    let restored = serialized
        .to_diagnostic(&program, Some(file))
        .expect("the diagnostic converts");
    assert_eq!(restored.source, js("vue"));
    assert_eq!(
        restored
            .localize(None, &tsr_locale::Locale::default())
            .expect("the diagnostic localizes"),
        b"mapper warning"
    );
}
