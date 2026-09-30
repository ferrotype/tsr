//! The host over an in-process mapper, as the harness serves its mappers.
use crate::*;
use std::sync::{Arc, Mutex};
use tsr_core::CompilerOptions;
use tsr_ipc::{Context, Handler, HandlerError, HandlerResult};
use tsr_json::RawValue;
use tsr_jsstring::JsString;
use tsr_tsoptions::config_mappers::{ContentMapper, MapperManifest};

/// Answers the protocol from a script: the initialize result, the transform
/// result or error, and a record of the project calls.
struct Scripted {
    initialize: InitializeResult,
    transform: Result<TransformResultMessage, String>,
    calls: Arc<Mutex<Vec<String>>>,
}

impl Handler for Scripted {
    fn handle_request(&self, _: &Context, method: &str, params: &[u8]) -> HandlerResult {
        self.calls.lock().unwrap().push(method.to_owned());
        match method {
            METHOD_INITIALIZE => Ok(Some(Box::new(self.initialize.clone()))),
            METHOD_OPEN_PROJECT => {
                let params = tsr_ipc::unmarshal_params::<OpenProjectParams>(params)?.unwrap();
                self.calls.lock().unwrap().push(format!(
                    "compilerOptions={}",
                    String::from_utf8_lossy(&params.compiler_options.0)
                ));
                Ok(Some(Box::new(OpenProjectResult::default())))
            }
            METHOD_CLOSE_PROJECT => Ok(None),
            METHOD_TRANSFORM => match &self.transform {
                Ok(result) => Ok(Some(Box::new(result.clone()))),
                Err(message) => Err(message.clone().into()),
            },
            _ => Err(format!("unexpected method {method}").into()),
        }
    }
    fn handle_notification(&self, _: &Context, _: &str, _: &[u8]) -> Result<(), HandlerError> {
        Ok(())
    }
}

fn mapper() -> ContentMapper {
    ContentMapper {
        package: JsString::from_bytes(b"mapper".as_slice()),
        extensions: vec![JsString::from_bytes(b".box".as_slice())],
        manifest: MapperManifest {
            name: JsString::from_bytes(b"mapper".as_slice()),
            exec: Some(vec![JsString::from_bytes(b"box-mapper".as_slice())]),
            ..MapperManifest::default()
        },
        ..ContentMapper::default()
    }
}

fn source(name: &str) -> InitializeResult {
    InitializeResult {
        position_encoding: PositionEncoding::utf8(),
        diagnostic_source: name.into(),
    }
}

fn identity_output(text: &str) -> TransformResultMessage {
    let length = text.len();
    TransformResultMessage {
        output: MappedOutput {
            text: text.into(),
            extension: ".ts".into(),
            mappings: Some(RawValue(
                format!("[[0,{length},0,{length},0]]").into_bytes(),
            )),
            diagnostic_directives: None,
        },
        diagnostics: Vec::new(),
        supplemental: Vec::new(),
    }
}

fn host(handler: Scripted) -> HostImpl {
    let handler = Arc::new(handler);
    let spawner = SpawnerFunc(
        move |_: &[JsString], _: &[u8], _: Box<dyn std::io::Write + Send>| {
            let (client, server) = tsr_ipc::pipe();
            let conn = tsr_ipc::AsyncConn::new(server, handler.clone());
            std::thread::spawn(move || {
                let _ = tsr_ipc::Conn::run(&conn, &Context::background());
            });
            Ok(client)
        },
    );
    new_host(
        &Context::background(),
        Arc::new(spawner),
        tsr_locale::Locale::default(),
    )
}

fn project(host: &HostImpl) -> Arc<dyn Project> {
    let options = CompilerOptions {
        target: tsr_core::ScriptTarget::ESNEXT,
        ..CompilerOptions::default()
    };
    host.project(ProjectSpec {
        config_file_name: JsString::from_bytes(b"/tsconfig.json".as_slice()),
        mappers: Arc::from(vec![mapper()]),
        compiler_options: Arc::new(options),
    })
    .unwrap()
}

fn request(content: &str) -> Request {
    Request {
        file_name: JsString::from_bytes(b"/app.box".as_slice()),
        content: content.as_bytes().to_vec(),
    }
}

#[test]
fn a_project_opens_once_transforms_and_closes_its_mapper_project() {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let mut output = identity_output("let x = 1;");
    output.diagnostics.push(Diagnostic {
        message_text: "Unclosed interpolation.".into(),
        start: 4,
        length: 1,
        code: 1000,
    });
    let host = host(Scripted {
        initialize: source("box"),
        transform: Ok(output),
        calls: calls.clone(),
    });
    let project = project(&host);
    let result = project.transform(0, &request("let x = 1;")).unwrap();
    assert_eq!(result.text, "let x = 1;");
    assert_eq!(result.virtual_extension, ".ts");
    assert_eq!(result.mappings.unwrap().segments().len(), 1);
    let diagnostic = &result.diagnostics[0];
    assert_eq!(
        (diagnostic.loc.pos(), diagnostic.loc.end(), diagnostic.code),
        (4, 5, 1000)
    );
    assert_eq!(diagnostic.source.as_bytes(), b"box");
    project.transform(0, &request("let x = 1;")).unwrap();
    assert!(project.identity(0).unwrap().starts_with("mapper:"));
    assert_eq!(project.identity(3).unwrap(), "");
    project.close().unwrap();
    project.close().unwrap();
    assert_eq!(
        *calls.lock().unwrap(),
        [
            "initialize",
            "openProject",
            r#"compilerOptions={"target":99}"#,
            "transform",
            "transform",
            "closeProject"
        ]
    );
    host.close().unwrap();
    assert!(host.project(ProjectSpec::default()).is_none());
}

#[test]
fn failures_carry_their_stage() {
    let failing = host(Scripted {
        initialize: source("box"),
        transform: Err("content mapper failed to transform the file".into()),
        calls: Arc::default(),
    });
    let error = project(&failing).transform(0, &request("x")).unwrap_err();
    assert_eq!(error.transform_kind(), Some(TransformErrorKind::Request));
    assert_eq!(
        error.to_string(),
        "content mapper transform failed: ipc: remote error [-32603]: content mapper failed to transform the file"
    );
    let reserved = host(Scripted {
        initialize: source("TS"),
        transform: Ok(identity_output("x")),
        calls: Arc::default(),
    });
    let error = project(&reserved).transform(0, &request("x")).unwrap_err();
    assert_eq!(error.transform_kind(), Some(TransformErrorKind::Initialize));
    let initialize = error.initialize_error().unwrap();
    assert_eq!(
        initialize.kind,
        InitializeErrorKind::ReservedDiagnosticSource
    );
    assert_eq!(initialize.mapper_name.as_bytes(), b"mapper");
    let mut coffee = identity_output("x");
    coffee.output.extension = ".coffee".into();
    let extension = host(Scripted {
        initialize: source("box"),
        transform: Ok(coffee),
        calls: Arc::default(),
    });
    let error = project(&extension).transform(0, &request("x")).unwrap_err();
    assert_eq!(error.transform_kind(), Some(TransformErrorKind::Response));
    assert!(
        matches!(error.cause(), Error::InvalidVirtualExtension(extension) if extension == ".coffee")
    );
}

#[test]
fn directives_normalize_and_reject_overlap_and_missing_unused_diagnostics() {
    let with_directives = |directives: Vec<MappedDiagnosticDirective>, unused: usize| {
        let mut output = identity_output("let a = 1;\nlet b = 2;\n");
        output.output.diagnostic_directives = Some(DiagnosticDirectives {
            unused_expect_directive_diagnostics: (0..unused)
                .map(|_| UnusedExpectDirectiveDiagnostic {
                    code: 2578,
                    message_text: "Unused.".into(),
                })
                .collect(),
            directives,
        });
        let host = host(Scripted {
            initialize: source("box"),
            transform: Ok(output),
            calls: Arc::default(),
        });
        project(&host).transform(0, &request("let a = 1;\nlet b = 2;\n"))
    };
    let expect = MappedDiagnosticDirective {
        original_start: 0,
        original_length: 10,
        virtual_start: 11,
        virtual_end: 21,
        policy: DIAGNOSTIC_DIRECTIVE_POLICY_EXPECT,
        unused_expect_directive_index: None,
    };
    let result = with_directives(vec![expect.clone()], 1).unwrap();
    let directive = &result.diagnostic_directives[0];
    assert_eq!((directive.policy, directive.unused_code), (1, 2578));
    assert_eq!(
        (directive.virtual_range.pos(), directive.virtual_range.end()),
        (11, 21)
    );
    let missing = with_directives(vec![expect.clone()], 2).unwrap_err();
    assert!(matches!(
        missing.cause(),
        Error::DiagnosticDirective {
            kind: DiagnosticDirectiveErrorKind::ExpectMissingUnusedDiagnostic,
            index: 0,
            ..
        }
    ));
    let overlap = with_directives(
        vec![
            MappedDiagnosticDirective {
                virtual_end: 2,
                ..MappedDiagnosticDirective::default()
            },
            MappedDiagnosticDirective {
                virtual_start: 1,
                virtual_end: 3,
                ..MappedDiagnosticDirective::default()
            },
        ],
        0,
    )
    .unwrap_err();
    assert!(matches!(
        overlap.cause(),
        Error::DiagnosticDirective {
            kind: DiagnosticDirectiveErrorKind::Overlap,
            index: 1,
            ..
        }
    ));
}

#[test]
fn utf16_positions_become_byte_offsets() {
    let text = "a😀b";
    let mut output = identity_output(text);
    output.output.mappings = Some(RawValue(b"[[0,4,0,4,0]]".to_vec()));
    output.diagnostics.push(Diagnostic {
        message_text: "m".into(),
        start: 3,
        length: 1,
        code: 1,
    });
    let host = host(Scripted {
        initialize: InitializeResult {
            position_encoding: PositionEncoding::utf16(),
            diagnostic_source: "box".into(),
        },
        transform: Ok(output),
        calls: Arc::default(),
    });
    let result = project(&host).transform(0, &request(text)).unwrap();
    let segment = result.mappings.unwrap().segments()[0];
    assert_eq!((segment.virtual_end, segment.original_end), (6, 6));
    let diagnostic = &result.diagnostics[0];
    assert_eq!((diagnostic.loc.pos(), diagnostic.loc.end()), (5, 6));
}
