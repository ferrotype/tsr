//! Direct tests of the API session over a memory file system: the lifecycle
//! and the handlers that need no checker. The pinned
//! `session_apistate_test.go` cases are ported where the Rust session has
//! the same observation; snapshot ids are not compared, because the Rust
//! counter is process-wide.
use super::*;
use crate::proto::{
    CompilerOptionsValue, CreateProgramOldProgramParams, CreateProgramOptions, DocumentIdentifier,
    GetSourceFileNamesParams, GetSourceFileParams, ParseConfigFileParams, ReadConfigFileParams,
    TranspileOptions, TranspileParams,
};
use tsr_vfs::{
    iovfs,
    vfstest::{self, InputFile},
};

const CONFIG: &str = "/home/projects/p/tsconfig.json";
const INDEX: &str = "/home/projects/p/src/index.ts";

fn session(files: &[(&str, &str)]) -> Arc<ApiSession> {
    let fs = Arc::new(vfstest::from_map(
        &files
            .iter()
            .map(|(name, text)| {
                (
                    name.as_bytes().to_vec(),
                    InputFile::Text(text.as_bytes().to_vec()),
                )
            })
            .collect(),
        false,
    ));
    ApiSession::standalone(
        tsr_project::session::SessionOptions {
            current_directory: JsString::from_bytes(b"/home/projects".as_slice()),
            ..Default::default()
        },
        Arc::new(iovfs::from(fs, false)),
    )
}

fn project_files() -> Vec<(&'static str, &'static str)> {
    vec![
        (CONFIG, r#"{ "compilerOptions": { "strict": true } }"#),
        (INDEX, "export const x = 1;"),
    ]
}

fn document(name: &str) -> DocumentIdentifier {
    DocumentIdentifier {
        file_name: name.to_string(),
        uri: String::new(),
    }
}

/// Ports `TestStandaloneSessionUsesSnapshotHostWithoutProjectSession` of
/// tsc/internal/api/session_apistate_test.go up to `createProgram`.
#[test]
fn a_standalone_session_updates_snapshots_over_its_own_project_session() {
    let session = session(&project_files());
    let first = session
        .handle_update_snapshot(&UpdateSnapshotParams {
            open_files: vec![document(INDEX)],
            ..Default::default()
        })
        .unwrap();
    assert_eq!(
        first.projects.len(),
        1,
        "the configured project of the opened file"
    );
    let project = first.projects[0].as_ref().unwrap();
    assert_eq!(project.config_file_name, CONFIG);
    assert!(project.root_files.iter().any(|name| name == INDEX));
    assert!(
        first.changes.is_none(),
        "no previous snapshot to diff against"
    );

    let second = session
        .handle_update_snapshot(&UpdateSnapshotParams {
            open_projects: vec![document(CONFIG)],
            ..Default::default()
        })
        .unwrap();
    assert_ne!(second.snapshot, first.snapshot);
    assert!(
        second.changes.is_some(),
        "the second update diffs against the first"
    );

    let default = session
        .handle_get_default_project_for_file(&GetDefaultProjectForFileParams {
            snapshot: second.snapshot,
            file: document(INDEX),
        })
        .unwrap()
        .expect("the file belongs to the configured project");
    assert_eq!(default.id, project.id);
    let names = session
        .handle_get_source_file_names(&GetSourceFileNamesParams {
            snapshot: second.snapshot,
            project: project.id.clone(),
        })
        .unwrap();
    assert!(names.iter().any(|name| name == INDEX), "{names:?}");

    session
        .handle_release(&ReleaseParams {
            snapshot: first.snapshot,
        })
        .unwrap();
    let error = session
        .handle_release(&ReleaseParams {
            snapshot: first.snapshot,
        })
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        format!("api: client error: snapshot {} not found", first.snapshot.0)
    );
    assert_eq!(
        session
            .handle_release(&ReleaseParams {
                snapshot: SnapshotId(0)
            })
            .unwrap_err()
            .to_string(),
        "api: client error: empty handle"
    );
    session.close();
}

/// Ports the open-tracking cases of `TestSessionTracksAndReleasesAPIRefs`:
/// opens are idempotent and closes release only held refs.
#[test]
fn project_opens_are_idempotent_and_closes_release_only_held_refs() {
    let session = session(&project_files());
    for _ in 0..2 {
        session
            .handle_update_snapshot(&UpdateSnapshotParams {
                open_projects: vec![document(CONFIG)],
                ..Default::default()
            })
            .unwrap();
        assert_eq!(session.open.lock().unwrap().projects.len(), 1);
    }
    for _ in 0..2 {
        session
            .handle_update_snapshot(&UpdateSnapshotParams {
                close_projects: vec![document(CONFIG)],
                ..Default::default()
            })
            .unwrap();
        assert_eq!(session.open.lock().unwrap().projects.len(), 0);
    }
    session
        .handle_update_snapshot(&UpdateSnapshotParams {
            open_files: vec![document(INDEX), document(INDEX)],
            ..Default::default()
        })
        .unwrap();
    assert_eq!(session.open.lock().unwrap().files.len(), 1);
    session.close();
    assert_eq!(session.open.lock().unwrap().files.len(), 0);
}

#[test]
fn source_files_are_encoded_or_null() {
    let session = session(&project_files());
    let update = session
        .handle_update_snapshot(&UpdateSnapshotParams {
            open_projects: vec![document(CONFIG)],
            ..Default::default()
        })
        .unwrap();
    let project = update.projects[0].as_ref().unwrap().id.clone();
    let response = session
        .handle_get_source_file(&GetSourceFileParams {
            snapshot: update.snapshot,
            project: project.clone(),
            file: document(INDEX),
        })
        .unwrap();
    let Response::Json(value) = response else {
        panic!("JSON mode encodes base64")
    };
    let bytes = tsr_json::marshal(value.as_ref(), tsr_json::Options::default()).unwrap();
    let text = String::from_utf8(bytes).unwrap();
    assert!(text.starts_with(r#"{"data":""#), "{text}");
    session.set_binary_responses(true);
    let response = session
        .handle_get_source_file(&GetSourceFileParams {
            snapshot: update.snapshot,
            project: project.clone(),
            file: document("/home/projects/p/src/missing.ts"),
        })
        .unwrap();
    let Response::Binary(bytes) = response else {
        panic!("binary mode")
    };
    assert!(
        bytes.is_empty(),
        "a missing file is an empty binary response"
    );
    let error = session
        .handle_get_source_file_names(&GetSourceFileNamesParams {
            snapshot: update.snapshot,
            project: ProjectId("/nope/tsconfig.json".into()),
        })
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "api: client error: project /nope/tsconfig.json not found"
    );
}

#[test]
fn configuration_handlers_follow_the_pin() {
    let session = session(&project_files());
    let missing = session.handle_read_config_file(&ReadConfigFileParams {
        file: document("/home/projects/p/missing.json"),
    });
    assert_eq!(missing.config.unwrap().0, b"{}");
    let error = missing.error.unwrap();
    assert_eq!(
        error.text,
        "Cannot read file '/home/projects/p/missing.json'."
    );
    assert_eq!(error.code, 5083);
    let read = session.handle_read_config_file(&ReadConfigFileParams {
        file: document(CONFIG),
    });
    assert!(read.error.is_none());
    assert_eq!(
        String::from_utf8(read.config.unwrap().0).unwrap(),
        r#"{"compilerOptions":{"strict":true}}"#
    );
    let parsed = session
        .handle_parse_config_file(&ParseConfigFileParams {
            file: document(CONFIG),
        })
        .unwrap();
    assert_eq!(parsed.file_names, vec![INDEX.to_string()]);
    assert!(parsed.options.unwrap().0.strict.is_true());
    assert_eq!(
        session
            .handle_parse_config_file(&ParseConfigFileParams {
                file: document("/home/projects/p/missing.json"),
            })
            .unwrap_err()
            .to_string(),
        "api: client error: could not read file \"/home/projects/p/missing.json\""
    );
    let transpiled = ApiSession::handle_transpile(
        &TranspileParams {
            input: "export const x: number = 1;".into(),
            options: TranspileOptions {
                compiler_options: None,
                file_name: String::new(),
                report_diagnostics: false,
            },
        },
        false,
    )
    .unwrap();
    assert!(
        transpiled.output_text.contains("export const x = 1;"),
        "{}",
        transpiled.output_text
    );
}

#[test]
fn unknown_methods_and_bad_params_fail_by_class() {
    let session = session(&project_files());
    let ctx = Context::background();
    let error = session
        .handle_request(&ctx, "noSuchMethod", b"{}")
        .err()
        .unwrap();
    assert_eq!(
        error.to_string(),
        "api: invalid request: unknown API method \"noSuchMethod\""
    );
    let error = session
        .handle_request(&ctx, "release", b"[1,")
        .err()
        .unwrap();
    assert!(
        error.to_string().starts_with("api: invalid request: "),
        "{error}"
    );
    let Some(Response::Json(pong)) = session.handle_request(&ctx, "ping", b"").unwrap() else {
        panic!("ping answers")
    };
    assert_eq!(
        tsr_json::marshal(pong.as_ref(), tsr_json::Options::default()).unwrap(),
        b"\"pong\""
    );
}

/// Ports `TestUpdateTemporarySnapshot` of
/// tsc/internal/api/session_temporary_test.go up to its diagnostics: the
/// temporary snapshot is distinct, the latest snapshot is untouched and the
/// base keeps its own view of the file.
#[test]
fn a_temporary_snapshot_overrides_one_file_without_advancing_the_latest() {
    let session = session(&project_files());
    let base = session
        .handle_update_snapshot(&UpdateSnapshotParams {
            open_files: vec![document(INDEX)],
            ..Default::default()
        })
        .unwrap();
    let project = base.projects[0].as_ref().unwrap().id.clone();
    let temporary = session
        .handle_update_temporary_snapshot(&UpdateTemporarySnapshotParams {
            snapshot: base.snapshot,
            file: document(INDEX),
            new_text: "export const x: string = 1;".into(),
        })
        .unwrap();
    assert_ne!(temporary.snapshot, base.snapshot);
    assert_eq!(session.snapshots.lock().unwrap().latest, base.snapshot.0);
    let changes = temporary.changes.expect("diffed against the base");
    assert!(
        changes.changed_projects.0.contains_key(&project),
        "the overridden file changes its project: {:?}",
        changes.changed_projects.0.keys().collect::<Vec<_>>()
    );
    let text = |snapshot: SnapshotId| {
        let data = session.snapshot_data(snapshot).unwrap();
        let program = data.program(&project).unwrap();
        let file = program.source_file(INDEX.as_bytes()).unwrap();
        file.bound()
            .view()
            .source_file()
            .unwrap()
            .text()
            .as_bytes()
            .to_vec()
    };
    assert_eq!(text(temporary.snapshot), b"export const x: string = 1;");
    assert_eq!(text(base.snapshot), b"export const x = 1;");
    session
        .handle_release(&ReleaseParams {
            snapshot: temporary.snapshot,
        })
        .unwrap();
    assert_eq!(text(base.snapshot), b"export const x = 1;");
    let error = session
        .handle_update_temporary_snapshot(&UpdateTemporarySnapshotParams {
            snapshot: base.snapshot,
            file: document("/home/projects/p/src/notes.unknownext"),
            new_text: String::new(),
        })
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "api: client error: failed to update temporary snapshot: unsupported file extension: /home/projects/p/src/notes.unknownext"
    );
}

/// Ports the shape of `TestCreateProgram` of
/// tsc/internal/api/session_createprogram_test.go up to its diagnostics.
#[test]
fn create_program_builds_one_synthetic_project_from_explicit_roots() {
    let session = session(&[(
        "/home/projects/p/index.ts",
        "export const value: string = 1;",
    )]);
    let base = session
        .handle_update_snapshot(&UpdateSnapshotParams::default())
        .unwrap();
    let options = tsr_core::CompilerOptions {
        no_lib: tsr_core::Tristate::TRUE,
        strict: tsr_core::Tristate::TRUE,
        ..Default::default()
    };
    let response = session
        .handle_create_program(&CreateProgramParams {
            root_files: vec![document("/home/projects/p/index.ts")],
            create_program_options: CreateProgramOptions {
                compiler_options: CompilerOptionsValue(options.clone()),
                project_references: Vec::new(),
                config_file_parsing_diagnostics: Vec::new(),
            },
            old_program: None,
            file_changes: None,
        })
        .unwrap();
    assert_ne!(response.snapshot, base.snapshot);
    assert_eq!(session.snapshots.lock().unwrap().latest, base.snapshot.0);
    let project = response.project.unwrap();
    assert_eq!(
        project.root_files,
        vec!["/home/projects/p/index.ts".to_string()]
    );
    assert!(project.compiler_options.unwrap().0.strict.is_true());
    let data = session.snapshot_data(response.snapshot).unwrap();
    assert_eq!(data.snapshot.projects().len(), 1, "one synthetic project");
    let error = session
        .handle_create_program(&CreateProgramParams {
            root_files: vec![document("/home/projects/p/index.ts")],
            create_program_options: CreateProgramOptions {
                compiler_options: CompilerOptionsValue(options),
                project_references: Vec::new(),
                config_file_parsing_diagnostics: Vec::new(),
            },
            old_program: None,
            file_changes: Some(Box::new(crate::proto::ApiFileChanges::default())),
        })
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "api: client error: fileChanges requires an oldProgram"
    );
    let again = session
        .handle_create_program(&CreateProgramParams {
            root_files: vec![document("/home/projects/p/index.ts")],
            create_program_options: CreateProgramOptions::default(),
            old_program: Some(Box::new(CreateProgramOldProgramParams {
                snapshot: response.snapshot,
                project: project.id.clone(),
            })),
            file_changes: None,
        })
        .unwrap();
    assert_ne!(again.snapshot, response.snapshot);
}
