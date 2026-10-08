//! Batches, diagnostics, emit and language-service queries through the wire
//! dispatch over a memory file system with the bundled libraries. The batch
//! cases port `session_batch_test.go`, the completion case
//! `session_completion_test.go`.
use super::handles::{node_handle, touching_property_name};
use super::*;
use crate::proto::{DocumentIdentifier, ProjectId};
use serde_json::{json, Value};
use tsr_vfs::{
    iovfs,
    vfstest::{self, InputFile},
};

const CONFIG: &str = "/home/projects/p/tsconfig.json";
const INDEX: &str = "/home/projects/p/src/index.ts";
const OTHER: &str = "/home/projects/p/src/other.ts";

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
            default_library_path: JsString::from_bytes(tsr_bundled::LIB_PATH),
            // The standalone server's encoding (the pin's api/server.go).
            position_encoding: tsr_jsstring::PositionEncoding::Utf8,
            ..Default::default()
        },
        Arc::new(tsr_bundled::BundledFs::new(Arc::new(iovfs::from(
            fs, false,
        )))),
    )
}

/// The wire handle of the property name token at `position` of `file`, as
/// a client forms it from a decoded source file.
fn handle_at(
    session: &ApiSession,
    snapshot: u64,
    project: &str,
    file: &str,
    position: u32,
) -> String {
    ancestor_handle_at(session, snapshot, project, file, position, 0)
}

/// The handle of the `levels`-th ancestor of the token at `position`.
fn ancestor_handle_at(
    session: &ApiSession,
    snapshot: u64,
    project: &str,
    file: &str,
    position: u32,
    levels: usize,
) -> String {
    let data = session.snapshot_data(SnapshotId(snapshot)).unwrap();
    let setup = data.setup_checker(&ProjectId(project.to_string())).unwrap();
    let operation = setup.registry.operation().unwrap();
    let mut node = touching_property_name(setup.program, file.as_bytes(), position)
        .unwrap()
        .expect("a token at the position");
    let file = setup.program.file_of_node(node).expect("the token's file");
    let view = file.bound().view().ast();
    for _ in 0..levels {
        node = view.node(node).unwrap().parent().expect("an ancestor");
    }
    node_handle(&operation, node).unwrap().0
}

fn request(session: &ApiSession, method: &str, params: &Value) -> Result<Value, String> {
    let payload = serde_json::to_vec(params).unwrap();
    match session.handle_request(&Context::background(), method, &payload) {
        Ok(Some(Response::Json(value))) => {
            let bytes = tsr_json::marshal(value.as_ref(), tsr_json::Options::default()).unwrap();
            Ok(serde_json::from_slice(&bytes).unwrap())
        }
        Ok(Some(Response::Binary(bytes))) => Ok(Value::String(format!("<{} bytes>", bytes.len()))),
        Ok(None) => Ok(Value::Null),
        Err(error) => Err(error.to_string()),
    }
}

/// A request's decoded response with the length of its encoding, for the
/// page-size assertions (a continuation token is consumed by one request).
fn request_measured(session: &ApiSession, method: &str, params: &Value) -> (Value, usize) {
    let payload = serde_json::to_vec(params).unwrap();
    let Ok(Some(Response::Json(value))) =
        session.handle_request(&Context::background(), method, &payload)
    else {
        panic!("a JSON response")
    };
    let bytes = tsr_json::marshal(value.as_ref(), tsr_json::Options::default()).unwrap();
    (serde_json::from_slice(&bytes).unwrap(), bytes.len())
}

fn open(session: &ApiSession, config: &str) -> (u64, String) {
    let update = session
        .handle_update_snapshot(&UpdateSnapshotParams {
            open_projects: vec![DocumentIdentifier {
                file_name: config.into(),
                uri: String::new(),
            }],
            ..Default::default()
        })
        .unwrap();
    let project = update.projects[0].as_ref().unwrap().id.0.clone();
    (update.snapshot.0, project)
}

/// Ports `TestHandleBatchRequests` and `TestHandleBatchRequestsRejectsNestedBatch`.
#[test]
fn batch_items_answer_individually() {
    let session = session(&[]);
    let response = request(
        &session,
        "batchRequests",
        &json!({"requests": [{"method": "ping", "params": {}}, {"method": "unknown", "params": {}}, {"method": "batchRequests", "params": {"requests": []}}]}),
    )
    .unwrap();
    let responses = response["responses"].as_array().unwrap();
    assert_eq!(responses.len(), 3);
    assert_eq!(responses[0]["method"], "ping");
    assert_eq!(responses[0]["result"], "pong");
    assert!(responses[0].get("error").is_none());
    assert_eq!(responses[1]["method"], "unknown");
    assert_eq!(
        responses[1]["error"],
        "api: invalid request: unknown API method \"unknown\""
    );
    assert_eq!(
        responses[2]["error"],
        "api: invalid request: batchRequests cannot be nested"
    );
    assert!(response.get("continuationToken").is_none());
}

/// Ports `TestHandleBatchRequestsPaginatesResponses`,
/// `TestHandleBatchRequestsAllowsOversizedSingleResponse`,
/// `TestHandleBatchRequestsPageLimitIsRequestScoped` and
/// `TestHandleBatchRequestsRejectsInvalidContinuationToken`.
#[test]
fn batch_pages_respect_the_byte_limit() {
    let session = session(&[]);
    let requests: Vec<Value> = (0..10)
        .map(|_| json!({"method": "ping", "params": {}}))
        .collect();
    let max = 150;
    let mut params = json!({"requests": requests, "maxResponseBytesPerPage": max});
    let mut collected = Vec::new();
    loop {
        let (page, length) = request_measured(&session, "batchRequests", &params);
        assert!(length <= max, "each page fits: {length} > {max}");
        collected.extend(page["responses"].as_array().unwrap().iter().cloned());
        let Some(token) = page.get("continuationToken").and_then(Value::as_str) else {
            break;
        };
        params = json!({"continuationToken": token, "maxResponseBytesPerPage": max});
    }
    assert_eq!(collected.len(), 10);
    assert!(collected
        .iter()
        .all(|response| response["result"] == "pong"));

    let oversized = request(
        &session,
        "batchRequests",
        &json!({"requests": [{"method": "ping", "params": {}}], "maxResponseBytesPerPage": 1}),
    )
    .unwrap();
    assert_eq!(oversized["responses"].as_array().unwrap().len(), 1);
    assert!(
        oversized.get("continuationToken").is_none(),
        "a single oversized response is sent whole"
    );

    let limited = request(
        &session,
        "batchRequests",
        &json!({"requests": [{"method": "ping", "params": {}}, {"method": "ping", "params": {}}], "maxResponseBytesPerPage": 1}),
    )
    .unwrap();
    assert_eq!(limited["responses"].as_array().unwrap().len(), 1);
    assert!(limited["continuationToken"]
        .as_str()
        .unwrap()
        .starts_with(&format!("{}-", session.id)));
    let unlimited = request(
        &session,
        "batchRequests",
        &json!({"requests": [{"method": "ping", "params": {}}, {"method": "ping", "params": {}}]}),
    )
    .unwrap();
    assert_eq!(unlimited["responses"].as_array().unwrap().len(), 2);
    assert!(unlimited.get("continuationToken").is_none());

    let error = request(
        &session,
        "batchRequests",
        &json!({"continuationToken": "invalid"}),
    )
    .unwrap_err();
    assert_eq!(error, "api: client error: invalid batch continuation token");
}

#[test]
fn diagnostics_and_emit_read_the_project_program() {
    let session = session(&[
        (
            CONFIG,
            r#"{ "compilerOptions": { "strict": true, "noLib": true, "target": "es2020" } }"#,
        ),
        (
            INDEX,
            "export const n: number = \"text\";\nexport let unused;\n",
        ),
    ]);
    let (snapshot, project) = open(&session, CONFIG);
    let semantic = request(
        &session,
        "getSemanticDiagnostics",
        &json!({"snapshot": snapshot, "project": project, "files": [INDEX]}),
    )
    .unwrap();
    let semantic = semantic.as_array().unwrap();
    assert!(
        semantic.iter().any(|d| d["code"] == 2322),
        "the assignment error: {semantic:?}"
    );
    assert!(semantic
        .iter()
        .all(|d| d["fileName"] == INDEX && d["startPosition"]["line"].is_number()));
    let syntactic = request(
        &session,
        "getSyntacticDiagnostics",
        &json!({"snapshot": snapshot, "project": project, "files": []}),
    )
    .unwrap();
    assert!(syntactic.as_array().unwrap().is_empty());
    let config = request(
        &session,
        "getConfigFileParsingDiagnostics",
        &json!({"snapshot": snapshot, "project": project}),
    )
    .unwrap();
    assert!(config.as_array().unwrap().is_empty());
    let output = request(
        &session,
        "emitToString",
        &json!({"snapshot": snapshot, "project": project}),
    )
    .unwrap();
    let files = output["outputFiles"].as_array().unwrap();
    assert_eq!(files.len(), 1, "{output}");
    assert_eq!(files[0]["fileName"], "/home/projects/p/src/index.js");
    assert_eq!(files[0]["sourceFileName"], INDEX);
    assert!(files[0]["text"]
        .as_str()
        .unwrap()
        .contains("export const n = \"text\";"));
    let js = request(
        &session,
        "getJavaScriptEmit",
        &json!({"snapshot": snapshot, "project": project, "files": [INDEX]}),
    )
    .unwrap();
    assert_eq!(js["outputFiles"].as_array().unwrap().len(), 1);
    let error = request(
        &session,
        "emit",
        &json!({"snapshot": snapshot, "project": project, "emitOnly": 7}),
    )
    .unwrap_err();
    assert_eq!(error, "api: client error: invalid emitOnly value: 7");
    let error = request(&session, "startCPUProfile", &json!({"dir": "/tmp"})).unwrap_err();
    assert_eq!(error, "method not implemented: startCPUProfile");
}

/// Ports `TestCompletionSymbolTypeIsResolvable` of
/// tsc/internal/api/session_completion_test.go: member completions carry
/// symbols whose types resolve on the same checker. The library is limited
/// to es5, which declares the array members the test reads.
#[test]
fn completions_carry_resolvable_symbols() {
    let content = "declare const people: string[];\npeople.";
    let session = session(&[
        (
            CONFIG,
            r#"{ "compilerOptions": { "strict": true, "lib": ["es5"] } }"#,
        ),
        (INDEX, content),
    ]);
    let (snapshot, project) = open(&session, CONFIG);
    let completions = request(
        &session,
        "getCompletionsAtPosition",
        &json!({"snapshot": snapshot, "project": project, "file": INDEX, "position": content.len(), "includeSymbol": true}),
    )
    .unwrap();
    let entries = completions["entries"]
        .as_array()
        .unwrap_or_else(|| panic!("a completion list: {completions}"));
    let mut saw_symbol = false;
    let mut saw_push = false;
    for entry in entries {
        let Some(symbol) = entry.get("symbol").filter(|symbol| !symbol.is_null()) else {
            continue;
        };
        saw_symbol = true;
        let ty = request(
            &session,
            "getTypeOfSymbol",
            &json!({"snapshot": snapshot, "project": project, "symbol": symbol["id"]}),
        )
        .unwrap();
        assert!(
            ty["id"].as_u64().is_some(),
            "type of completion symbol {} resolves",
            entry["name"]
        );
        if entry["name"] == "push" {
            saw_push = true;
        }
    }
    assert!(saw_symbol, "completion entries include resolvable symbols");
    assert!(saw_push, "array member completions include push");
}

#[test]
fn references_are_reported_as_node_handles() {
    let content = "export const answer = 1;\nexport const twice = answer + answer;\n";
    let session = session(&[
        (CONFIG, r#"{ "compilerOptions": { "noLib": true } }"#),
        (INDEX, content),
    ]);
    let (snapshot, project) = open(&session, CONFIG);
    let symbol = request(
        &session,
        "getSymbolAtPosition",
        &json!({"snapshot": snapshot, "project": project, "file": INDEX, "position": 14}),
    )
    .unwrap();
    assert_eq!(symbol["name"], "answer");
    let in_file = request(
        &session,
        "getReferencesToSymbolInFile",
        &json!({"snapshot": snapshot, "project": project, "file": INDEX, "symbol": symbol["id"]}),
    )
    .unwrap();
    assert_eq!(in_file.as_array().unwrap().len(), 3, "{in_file}");
    // The pin searches from a name token; a declaration node has no symbol
    // at its location and yields no groups, which go out as `[]` (the Go
    // handler's nil slice).
    let declaration = symbol["declarations"][0].as_str().unwrap();
    let none = request(
        &session,
        "getReferencedSymbolsForNode",
        &json!({"snapshot": snapshot, "project": project, "node": declaration, "position": 14}),
    )
    .unwrap();
    assert_eq!(none, json!([]), "{none}");
    let name = handle_at(&session, snapshot, &project, INDEX, 14);
    let groups = request(
        &session,
        "getReferencedSymbolsForNode",
        &json!({"snapshot": snapshot, "project": project, "node": name, "position": 14}),
    )
    .unwrap();
    let groups = groups
        .as_array()
        .unwrap_or_else(|| panic!("reference groups: {groups} (symbol {symbol})"));
    assert_eq!(groups.len(), 1, "{groups:?}");
    assert_eq!(groups[0]["symbol"]["name"], "answer");
    assert!(groups[0]["references"].as_array().unwrap().len() >= 2);
}

/// The pin's nil file list: an omitted `files` means every file, `[]` names
/// none; selected emit refuses the omission and accepts the empty list.
#[test]
fn file_lists_distinguish_omitted_from_empty() {
    let session = session(&[
        (CONFIG, r#"{ "compilerOptions": { "noLib": true } }"#),
        (INDEX, "const a: = 1;\n"),
        (OTHER, "const b: = 1;\n"),
    ]);
    let (snapshot, project) = open(&session, CONFIG);
    let all = request(
        &session,
        "getSyntacticDiagnostics",
        &json!({"snapshot": snapshot, "project": project}),
    )
    .unwrap();
    assert_eq!(all.as_array().unwrap().len(), 2, "{all}");
    let none = request(
        &session,
        "getSyntacticDiagnostics",
        &json!({"snapshot": snapshot, "project": project, "files": []}),
    )
    .unwrap();
    assert_eq!(none, json!([]));
    let one = request(
        &session,
        "getSyntacticDiagnostics",
        &json!({"snapshot": snapshot, "project": project, "files": [OTHER]}),
    )
    .unwrap();
    assert_eq!(one.as_array().unwrap().len(), 1, "{one}");
    assert_eq!(one[0]["fileName"], OTHER);
    assert_eq!(one[0]["sourceLines"][0]["text"], "const b: = 1;\n");
    let error = request(
        &session,
        "getJavaScriptEmit",
        &json!({"snapshot": snapshot, "project": project}),
    )
    .unwrap_err();
    assert_eq!(error, "api: client error: files is required");
    let empty = request(
        &session,
        "getJavaScriptEmit",
        &json!({"snapshot": snapshot, "project": project, "files": []}),
    )
    .unwrap();
    assert_eq!(empty["outputFiles"], json!([]));
}

/// Config-file diagnostics name the config file, which the program holds
/// outside its file list.
#[test]
fn config_diagnostics_name_the_config_file() {
    let session = session(&[
        (
            CONFIG,
            r#"{ "compilerOptions": { "noLib": true, "target": "bogus" } }"#,
        ),
        (INDEX, "export const n = 1;\n"),
    ]);
    let (snapshot, project) = open(&session, CONFIG);
    let config = request(
        &session,
        "getConfigFileParsingDiagnostics",
        &json!({"snapshot": snapshot, "project": project}),
    )
    .unwrap();
    let config = config.as_array().unwrap();
    assert_eq!(config.len(), 1, "{config:?}");
    assert_eq!(config[0]["code"], 6046);
    assert_eq!(config[0]["fileName"], CONFIG);
    assert!(config[0]["pos"].as_u64().unwrap() > 0, "{}", config[0]);
}

/// Ports `TestHandleBatchRequestsRecoversPerRequestPanics`: a failure the pin
/// reaches by panicking is reported in its item alone, under `panic:`.
#[test]
fn batch_items_report_panics_alone() {
    let session = session(&[
        (CONFIG, r#"{ "compilerOptions": { "noLib": true } }"#),
        (INDEX, "export const n = 1;\n"),
    ]);
    let (snapshot, project) = open(&session, CONFIG);
    let string = request(
        &session,
        "getStringType",
        &json!({"snapshot": snapshot, "project": project}),
    )
    .unwrap();
    let response = request(
        &session,
        "batchRequests",
        &json!({"requests": [
            {"method": "ping", "params": {}},
            {"method": "getTypeArguments", "params": {"snapshot": snapshot, "project": project, "type": string["id"]}},
            {"method": "ping", "params": {}},
        ]}),
    )
    .unwrap();
    let responses = response["responses"].as_array().unwrap();
    assert_eq!(responses[0]["result"], "pong");
    let error = responses[1]["error"].as_str().unwrap();
    assert!(error.starts_with("panic: "), "{error}");
    assert_eq!(responses[2]["result"], "pong");
}

/// Ports `TestCompletionOnInferredProject`: a file without a config belongs
/// to an inferred project, whose language service answers completions.
#[test]
fn completions_answer_on_an_inferred_project() {
    let content = "declare const people: string[];\npeople.";
    let session = session(&[(INDEX, content)]);
    let update = session
        .handle_update_snapshot(&UpdateSnapshotParams {
            open_files: vec![DocumentIdentifier {
                file_name: INDEX.into(),
                uri: String::new(),
            }],
            ..Default::default()
        })
        .unwrap();
    let snapshot = update.snapshot.0;
    let project = request(
        &session,
        "getDefaultProjectForFile",
        &json!({"snapshot": snapshot, "file": INDEX}),
    )
    .unwrap();
    let project = project["id"]
        .as_str()
        .expect("an inferred default project")
        .to_string();
    let completions = request(
        &session,
        "getCompletionsAtPosition",
        &json!({"snapshot": snapshot, "project": project, "file": INDEX, "position": content.len()}),
    )
    .unwrap();
    let entries = completions["entries"]
        .as_array()
        .expect("a completion list");
    assert!(
        entries.iter().any(|entry| entry["name"] == "push"),
        "{completions}"
    );
}

/// Ports `TestCompletionRetriesWithAutoImports`: module-export completions
/// come from the project's auto-import registry, which the language service
/// builds on demand where the pin retries on a snapshot cloned with
/// auto-imports.
#[test]
fn completions_include_module_exports() {
    let content = "someV";
    let session = session(&[
        (
            CONFIG,
            r#"{ "compilerOptions": { "module": "esnext", "target": "esnext", "noLib": true } }"#,
        ),
        (
            "/home/projects/p/src/export.ts",
            "export const someValue = 1;",
        ),
        (INDEX, content),
    ]);
    let (snapshot, project) = open(&session, CONFIG);
    let completions = request(
        &session,
        "getCompletionsAtPosition",
        &json!({"snapshot": snapshot, "project": project, "file": INDEX, "position": content.len()}),
    )
    .unwrap();
    let entries = completions["entries"]
        .as_array()
        .expect("a completion list");
    assert!(
        entries.iter().any(|entry| entry["name"] == "someValue"),
        "{completions}"
    );
}

/// A diagnostic the client hands `createProgram` comes back with its text:
/// the pin's ad hoc message, localized to itself.
#[test]
fn create_program_returns_the_client_config_diagnostics() {
    let session = session(&[(INDEX, "export const value = 1;")]);
    let diagnostic = json!({"pos": 0, "end": 0, "code": 9001, "category": 1, "text": "Synthetic config parsing error."});
    let program = request(
        &session,
        "createProgram",
        &json!({"rootFiles": [INDEX], "createProgramOptions": {"compilerOptions": {"noLib": true}, "configFileParsingDiagnostics": [diagnostic]}}),
    )
    .unwrap();
    let diagnostics = request(
        &session,
        "getConfigFileParsingDiagnostics",
        &json!({"snapshot": program["snapshot"], "project": program["project"]["id"]}),
    )
    .unwrap();
    assert_eq!(diagnostics, json!([diagnostic]));
}

/// A malformed node handle index reports `strconv.ParseUint`'s text, as
/// the pin's `resolveNodeHandle` does, and is bounded to 32 bits.
#[test]
fn node_handle_indexes_report_the_pin_parse_errors() {
    let session = session(&[
        (CONFIG, r#"{ "compilerOptions": { "noLib": true } }"#),
        (INDEX, "export const n = 1;\n"),
    ]);
    let (snapshot, project) = open(&session, CONFIG);
    for (handle, reason) in [
        (format!("abc.1.{INDEX}"), "invalid syntax"),
        (format!("99999999999.1.{INDEX}"), "value out of range"),
    ] {
        let error = request(
            &session,
            "getSymbolAtLocation",
            &json!({"snapshot": snapshot, "project": project, "location": handle}),
        )
        .unwrap_err();
        let index = handle.split('.').next().unwrap();
        assert_eq!(
            error,
            format!(
                "api: client error: invalid node handle \"{handle}\": strconv.ParseUint: parsing \"{index}\": {reason}"
            )
        );
    }
}

/// A diagnostic in a config the root extends keeps that file's name and its
/// UTF-16 positions: the response resolves the file through the owner of
/// the diagnostic's node, not the root config's view.
#[test]
fn extended_config_diagnostics_name_their_own_file() {
    const BASE: &str = "/home/projects/p/base.json";
    let base =
        "// 💩 a comment before the error\n{ \"compilerOptions\": { \"target\": \"bogus\" } }";
    let session = session(&[
        (
            CONFIG,
            r#"{ "extends": "./base.json", "compilerOptions": { "noLib": true } }"#,
        ),
        (BASE, base),
        (INDEX, "export const n = 1;\n"),
    ]);
    let (snapshot, project) = open(&session, CONFIG);
    let config = request(
        &session,
        "getConfigFileParsingDiagnostics",
        &json!({"snapshot": snapshot, "project": project}),
    )
    .unwrap();
    let config = config.as_array().unwrap();
    assert_eq!(config.len(), 1, "{config:?}");
    assert_eq!(config[0]["code"], 6046);
    assert_eq!(config[0]["fileName"], BASE, "{}", config[0]);
    let prefix = &base[..base.find("\"bogus\"").unwrap()];
    let pos = prefix.encode_utf16().count();
    assert_eq!(config[0]["pos"], pos, "{}", config[0]);
    assert_eq!(config[0]["end"], pos + "\"bogus\"".len());
    assert_eq!(config[0]["startPosition"]["line"], 1);
    assert_eq!(config[0]["sourceLines"].as_array().unwrap().len(), 1);
}

/// The snapshot reference a derivation takes is released when the request
/// unwinds (the pin's deferred release): the client's own release is then
/// the last one, and the snapshot's handles are rejected with the pin's
/// text.
#[test]
fn a_request_that_unwinds_releases_its_snapshot_reference() {
    let session = session(&[
        (CONFIG, r#"{ "compilerOptions": { "noLib": true } }"#),
        (INDEX, "export const n = 1;\n"),
    ]);
    let (snapshot, project) = open(&session, CONFIG);
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let unwound = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _retained = session.retain_snapshot(SnapshotId(snapshot)).unwrap();
        panic!("a callback panicked during derivation");
    }));
    std::panic::set_hook(previous);
    assert!(unwound.is_err());
    assert_eq!(
        request(&session, "release", &json!({"snapshot": snapshot})).unwrap(),
        json!(true)
    );
    let error = request(
        &session,
        "getSymbolAtPosition",
        &json!({"snapshot": snapshot, "project": project, "file": INDEX, "position": 13}),
    )
    .unwrap_err();
    assert_eq!(
        error,
        format!("api: client error: snapshot {snapshot} not found")
    );
}

/// A client callback that panics during `updateSnapshot` (the pin's read
/// callbacks panic on a client error) unwinds through the session's open-set
/// lock; the next update continues, as the pin's deferred unlock lets it.
#[test]
fn an_update_that_unwinds_in_a_callback_leaves_the_session_usable() {
    use std::sync::atomic::{AtomicBool, Ordering};
    use tsr_vfs::wrapped::{Replacements, WrappedFs};
    use tsr_vfs::FileSystem;
    let files: std::collections::BTreeMap<Vec<u8>, InputFile> = [
        (CONFIG, r#"{ "compilerOptions": { "noLib": true } }"#),
        (INDEX, "export const n = 1;\n"),
    ]
    .into_iter()
    .map(|(name, text)| {
        (
            name.as_bytes().to_vec(),
            InputFile::Text(text.as_bytes().to_vec()),
        )
    })
    .collect();
    let base: Arc<dyn FileSystem> = Arc::new(iovfs::from(
        Arc::new(vfstest::from_map(&files, false)),
        false,
    ));
    let failing = Arc::new(AtomicBool::new(true));
    let mut replacements = Replacements::forwarding(base.clone());
    replacements.read_file_result = Some({
        let failing = failing.clone();
        let base = base.clone();
        Arc::new(move |path: &[u8]| {
            assert!(
                !(path == INDEX.as_bytes() && failing.swap(false, Ordering::SeqCst)),
                "the client's readFile callback failed"
            );
            base.read_file_result(path)
        })
    });
    let session = ApiSession::standalone(
        tsr_project::session::SessionOptions {
            current_directory: JsString::from_bytes(b"/home/projects".as_slice()),
            default_library_path: JsString::from_bytes(tsr_bundled::LIB_PATH),
            ..Default::default()
        },
        Arc::new(tsr_bundled::BundledFs::new(Arc::new(WrappedFs::new(
            base,
            replacements,
        )))),
    );
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let unwound = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| open(&session, CONFIG)));
    std::panic::set_hook(previous);
    assert!(unwound.is_err(), "the first update unwinds in the callback");
    assert!(!failing.load(Ordering::SeqCst));
    let (snapshot, project) = open(&session, CONFIG);
    let symbol = request(
        &session,
        "getSymbolAtPosition",
        &json!({"snapshot": snapshot, "project": project, "file": INDEX, "position": 13}),
    )
    .unwrap();
    assert_eq!(symbol["name"], "n");
    session.close();
}

/// The ownership note's retirement-serialized commitment (section 2.7): a
/// generation retired by a sibling after a query computed its response
/// turns that response into the error form instead of publishing handles
/// of a retired checker, and the retired snapshot answers nothing further.
#[test]
fn a_response_is_not_published_after_its_generation_retires() {
    let session = session(&[
        (CONFIG, r#"{ "compilerOptions": { "noLib": true } }"#),
        (INDEX, "export const answer = 1;\n"),
    ]);
    let (snapshot, project) = open(&session, CONFIG);
    let symbol = request(
        &session,
        "getSymbolAtPosition",
        &json!({"snapshot": snapshot, "project": project, "file": INDEX, "position": 14}),
    )
    .unwrap();
    assert_eq!(symbol["name"], "answer");
    let pool = session
        .snapshot_data(SnapshotId(snapshot))
        .unwrap()
        .project(&ProjectId(project.clone()))
        .unwrap()
        .pool()
        .clone();
    hooks::set_before_commit(Some(Box::new(move || pool.generation().retire())));
    let error = request(
        &session,
        "getTypeOfSymbol",
        &json!({"snapshot": snapshot, "project": project, "symbol": symbol["id"]}),
    )
    .unwrap_err();
    hooks::set_before_commit(None);
    assert!(error.contains("retired"), "{error}");
    let error = request(
        &session,
        "getTypeOfSymbol",
        &json!({"snapshot": snapshot, "project": project, "symbol": symbol["id"]}),
    )
    .unwrap_err();
    assert!(
        error.contains("retired"),
        "the retired generation's handles are rejected: {error}"
    );
}

/// The registry's core: one binder symbol seen from two projects has one
/// id, with the project that first observed it as its canonical project.
#[test]
fn a_symbol_shared_by_two_projects_has_one_id() {
    const SHARED: &str = "/home/projects/p/src/shared.ts";
    const OTHER_CONFIG: &str = "/home/projects/q/tsconfig.json";
    let session = session(&[
        (
            CONFIG,
            r#"{ "compilerOptions": { "noLib": true }, "files": ["src/index.ts", "src/shared.ts"] }"#,
        ),
        (
            OTHER_CONFIG,
            r#"{ "compilerOptions": { "noLib": true }, "files": ["../p/src/shared.ts"] }"#,
        ),
        (INDEX, "export const n = 1;\n"),
        (SHARED, "export const shared = 1;\n"),
    ]);
    let (initial, first) = open(&session, CONFIG);
    let update = session
        .handle_update_snapshot(&UpdateSnapshotParams {
            open_projects: vec![DocumentIdentifier {
                file_name: OTHER_CONFIG.into(),
                uri: String::new(),
            }],
            ..Default::default()
        })
        .unwrap();
    let snapshot = update.snapshot.0;
    assert!(snapshot >= initial);
    let second = update
        .projects
        .iter()
        .flatten()
        .map(|project| project.id.0.clone())
        .find(|id| *id != first)
        .expect("the second project");
    let from_first = request(
        &session,
        "getSymbolAtPosition",
        &json!({"snapshot": snapshot, "project": first, "file": SHARED, "position": 14}),
    )
    .unwrap();
    let from_second = request(
        &session,
        "getSymbolAtPosition",
        &json!({"snapshot": snapshot, "project": second, "file": SHARED, "position": 14}),
    )
    .unwrap();
    assert_eq!(from_first["name"], "shared");
    assert_eq!(
        from_first["id"], from_second["id"],
        "{from_first} {from_second}"
    );
    assert_eq!(from_first["project"], first);
    assert_eq!(
        from_second["project"], first,
        "the first project stays canonical"
    );
}

/// `getRestTypeOfSignature` slices a tuple rest parameter first, as the
/// pin's `tryGetRestTypeOfSignature` does: a tuple with no rest element
/// has no rest type, an array rest parameter answers its element type.
#[test]
fn rest_types_of_signatures_slice_tuples_first() {
    let content = "declare function f(...args: [string, number]): void;\ndeclare function g(...args: string[]): void;\ndeclare function h(...args: [string, ...number[]]): void;\n";
    // Arrays and tuples need the es5 library's `Array`.
    let session = session(&[
        (CONFIG, r#"{ "compilerOptions": { "lib": ["es5"] } }"#),
        (INDEX, content),
    ]);
    let (snapshot, project) = open(&session, CONFIG);
    let rest_type = |name: &str| -> Value {
        let position = content.find(&format!("function {name}")).unwrap() + "function ".len();
        let symbol = request(
            &session,
            "getSymbolAtPosition",
            &json!({"snapshot": snapshot, "project": project, "file": INDEX, "position": position}),
        )
        .unwrap();
        let ty = request(
            &session,
            "getTypeOfSymbol",
            &json!({"snapshot": snapshot, "project": project, "symbol": symbol["id"]}),
        )
        .unwrap();
        let signatures = request(
            &session,
            "getSignaturesOfType",
            &json!({"snapshot": snapshot, "project": project, "type": ty["id"], "kind": 0}),
        )
        .unwrap();
        let rest = request(
            &session,
            "getRestTypeOfSignature",
            &json!({"snapshot": snapshot, "project": project, "signature": signatures[0]["id"]}),
        )
        .unwrap();
        request(
            &session,
            "typeToString",
            &json!({"snapshot": snapshot, "project": project, "type": rest["id"]}),
        )
        .unwrap()
    };
    assert_eq!(
        rest_type("f"),
        json!("any"),
        "a tuple without a rest element"
    );
    assert_eq!(rest_type("g"), json!("string"), "an array rest parameter");
    assert_eq!(
        rest_type("h"),
        json!("any"),
        "a sliced rest element without a numeric index"
    );
}

/// Ports the client's `Checker - getConstantValue` cases: an enum member's
/// numeric or string value, through the member node.
#[test]
fn enum_members_report_their_constant_values() {
    let content = "export enum E { A = 1, B = 2 }\nexport enum Color { Red = \"red\" }\n";
    let session = session(&[
        (CONFIG, r#"{ "compilerOptions": { "noLib": true } }"#),
        (INDEX, content),
    ]);
    let (snapshot, project) = open(&session, CONFIG);
    let value_of = |name: &str, levels: usize| -> Value {
        let position = u32::try_from(content.find(name).unwrap()).unwrap();
        let node = ancestor_handle_at(&session, snapshot, &project, INDEX, position, levels);
        request(
            &session,
            "getConstantValue",
            &json!({"snapshot": snapshot, "project": project, "location": node}),
        )
        .unwrap()
    };
    assert_eq!(value_of("B = 2", 1), json!(2));
    assert_eq!(value_of("Red", 1), json!("red"));
    assert_eq!(
        value_of("B = 2", 0),
        Value::Null,
        "a name token is not a constant"
    );
}

/// Ports the client's `Symbol - getDocumentationComment and getJsDocTags`
/// cases: the comment text without tags, and the tags as name/text pairs.
#[test]
fn documentation_and_tags_come_from_the_declaration_comment() {
    let content = "\n/**\n * Adds two numbers together.\n * @param a the first number\n * @returns the sum\n */\nexport function add(a: number, b: number): number { return a + b; }\n";
    let session = session(&[
        (CONFIG, r#"{ "compilerOptions": { "noLib": true } }"#),
        (INDEX, content),
    ]);
    let (snapshot, project) = open(&session, CONFIG);
    let symbol = request(
        &session,
        "getSymbolAtPosition",
        &json!({"snapshot": snapshot, "project": project, "file": INDEX, "position": content.find("add(a").unwrap()}),
    )
    .unwrap();
    let doc = request(
        &session,
        "getDocumentationComment",
        &json!({"snapshot": snapshot, "project": project, "symbol": symbol["id"]}),
    )
    .unwrap();
    let doc = doc.as_str().unwrap();
    assert!(doc.contains("Adds two numbers together"), "{doc}");
    assert!(!doc.contains("@param"), "{doc}");
    let tags = request(
        &session,
        "getJsDocTags",
        &json!({"snapshot": snapshot, "project": project, "symbol": symbol["id"]}),
    )
    .unwrap();
    assert_eq!(
        tags,
        json!([{"name": "param", "text": "a the first number"}, {"name": "returns", "text": "the sum"}])
    );
}

/// Ports the client's `LanguageService - getSignatureUsage` case: a usage
/// pairs the referencing name with the call it is the callee of.
#[test]
fn signature_usages_pair_names_with_their_calls() {
    let content =
        "function greet(name: string) { return name; }\ngreet(\"world\");\nconst alias = greet;\n";
    let session = session(&[
        (CONFIG, r#"{ "compilerOptions": { "noLib": true } }"#),
        (INDEX, content),
    ]);
    let (snapshot, project) = open(&session, CONFIG);
    let declaration = ancestor_handle_at(&session, snapshot, &project, INDEX, 9, 1);
    let usages = request(
        &session,
        "getSignatureUsages",
        &json!({"snapshot": snapshot, "project": project, "signatureDecl": declaration}),
    )
    .unwrap();
    let usages = usages.as_array().expect("usages");
    assert_eq!(usages.len(), 2, "{usages:?}");
    assert!(
        usages[0]["call"]
            .as_str()
            .is_some_and(|call| !call.is_empty()),
        "{usages:?}"
    );
    assert!(
        usages[1].get("call").is_none(),
        "an uncalled reference has no call: {usages:?}"
    );
}

/// Applies the client's UTF-16 offset edits to ASCII text.
fn apply_text_edits(source: &str, edits: &Value) -> String {
    let mut edits: Vec<(usize, usize, String)> = edits
        .as_array()
        .expect("edits")
        .iter()
        .map(|edit| {
            (
                edit["pos"].as_u64().unwrap() as usize,
                edit["end"].as_u64().unwrap() as usize,
                edit["newText"].as_str().unwrap().to_string(),
            )
        })
        .collect();
    edits.sort_by_key(|edit| std::cmp::Reverse(edit.0));
    let mut text = source.to_string();
    for (pos, end, new_text) in edits {
        text.replace_range(pos..end, &new_text);
    }
    text
}

/// Go's `base64.StdEncoding`, for the insertion-formatting data.
fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let mut word = [0u8; 3];
        word[..chunk.len()].copy_from_slice(chunk);
        let bits = u32::from_be_bytes([0, word[0], word[1], word[2]]);
        for index in 0..4 {
            if index <= chunk.len() {
                out.push(ALPHABET[(bits >> (18 - 6 * index)) as usize & 63] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// The export symbol the client passes to the import adder: the symbol at
/// `position` of `file`, or its export symbol when the server reports one
/// (the declaration's symbol of an exported variable is already the
/// module's export, so the raw field is null for it).
fn export_symbol_at(
    session: &ApiSession,
    snapshot: u64,
    project: &str,
    file: &str,
    position: usize,
) -> Value {
    let symbol = request(
        session,
        "getSymbolAtPosition",
        &json!({"snapshot": snapshot, "project": project, "file": file, "position": position}),
    )
    .unwrap();
    let exported = request(
        session,
        "getExportSymbolOfSymbol",
        &json!({"snapshot": snapshot, "project": project, "objectId": symbol["id"]}),
    )
    .unwrap();
    let id = if exported.is_null() {
        symbol["id"].clone()
    } else {
        exported["id"].clone()
    };
    assert!(id.is_number(), "{symbol} {exported}");
    id
}

/// An import extended past a non-ASCII identifier: the language service
/// converts positions with the standalone session's encoding, UTF-8, so
/// the LSP character the pin's `toAPITextEdits` adds to the line's byte
/// start is a byte count, and the UTF-16 offset it reports lands after
/// `fóo` (12), not inside it (11).
#[test]
fn import_adder_edits_keep_their_coordinates_past_non_ascii_text() {
    const FOO: &str = "/home/projects/p/src/foo.ts";
    let session = session(&[
        (CONFIG, r#"{ "compilerOptions": { "noLib": true } }"#),
        (
            INDEX,
            "import { fóo } from \"./foo\";\nconst value = zoo;\n",
        ),
        (FOO, "export const fóo = 1;\nexport const zoo = 2;\n"),
    ]);
    let (snapshot, project) = open(&session, CONFIG);
    let zoo = export_symbol_at(
        &session,
        snapshot,
        &project,
        FOO,
        "export const fóo = 1;\nexport const "
            .encode_utf16()
            .count(),
    );
    let edits = request(
        &session,
        "getImportAdderEdits",
        &json!({"snapshot": snapshot, "project": project, "file": INDEX, "actions": [{"kind": "importSymbol", "symbol": zoo}]}),
    )
    .unwrap();
    assert_eq!(edits, json!([{"pos": 12, "end": 12, "newText": ", zoo"}]));
}

/// A session hosted by the LSP server formats insertions and import edits
/// with the snapshot's user preferences, as the pin reads them from its
/// snapshot: two-space indentation, CRLF and single quotes here, where a
/// standalone session has the defaults.
#[test]
fn hosted_sessions_read_the_snapshot_preferences() {
    const FOO: &str = "/home/projects/p/src/foo.ts";
    const FILES: &[(&str, &str)] = &[
        (CONFIG, r#"{ "compilerOptions": { "noLib": true } }"#),
        (INDEX, "const value = 1;\n"),
        (FOO, "function f() {\nreturn 1;\n}\nexport const zoo = 2;\n"),
    ];
    let hosted = {
        let fs = Arc::new(vfstest::from_map(
            &FILES
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
        let project = ProjectSession::new(
            tsr_project::session::SessionOptions {
                current_directory: JsString::from_bytes(b"/home/projects".as_slice()),
                default_library_path: JsString::from_bytes(tsr_bundled::LIB_PATH),
                ..Default::default()
            },
            Arc::new(tsr_bundled::BundledFs::new(Arc::new(iovfs::from(
                fs, false,
            )))),
            &tsr_arena::Counters::new(),
        );
        ApiSession::for_lsp(
            project,
            Arc::new(|| {
                let mut preferences = tsr_ls::CompletionOptions::default();
                preferences.format.editor.indent_size = 2;
                preferences.newline = Some("\r\n".into());
                preferences.quote = tsr_ls::QuotePreference::Single;
                preferences
            }),
        )
    };
    let standalone = session(FILES);
    let format = |session: &ApiSession| -> (String, String) {
        let (snapshot, project) = open(session, CONFIG);
        let data = session.snapshot_data(SnapshotId(snapshot)).unwrap();
        let setup = data.setup_checker(&ProjectId(project.clone())).unwrap();
        let name = touching_property_name(setup.program, FOO.as_bytes(), "function ".len() as u32)
            .unwrap()
            .expect("the function name");
        let file = setup
            .program
            .file_of_node(name)
            .expect("the function's file");
        let view = file.bound().view().ast();
        let function = view.node(name).unwrap().parent().expect("the declaration");
        let encoded = tsr_encoder::encode_node(
            view,
            function,
            Some(file.source()),
            &mut tsr_parser::ParserJsDocProvider::default(),
        )
        .unwrap();
        let formatted = request(
            session,
            "formatNodeForInsertion",
            &json!({"snapshot": snapshot, "project": project, "file": INDEX, "position": "const value = 1;\n".len(), "data": base64(&encoded.bytes)}),
        )
        .unwrap();
        let zoo = export_symbol_at(
            session,
            snapshot,
            &project,
            FOO,
            "function f() {\nreturn 1;\n}\nexport const ".len(),
        );
        let edits = request(
            session,
            "getImportAdderEdits",
            &json!({"snapshot": snapshot, "project": project, "file": INDEX, "actions": [{"kind": "importSymbol", "symbol": zoo}]}),
        )
        .unwrap();
        (
            formatted.as_str().unwrap().to_string(),
            apply_text_edits("const value = 1;\n", &edits),
        )
    };
    assert_eq!(
        format(&standalone),
        (
            "function f() {\n    return 1;\n}".to_string(),
            "import { zoo } from \"./foo\";\n\nconst value = 1;\n".to_string()
        )
    );
    assert_eq!(
        format(&hosted),
        (
            "function f() {\r\n  return 1;\r\n}".to_string(),
            "import { zoo } from './foo';\n\nconst value = 1;\n".to_string()
        )
    );
}

/// Ports the client's `LanguageService - imports` cases: a named import is
/// added, two actions coalesce into one import, an existing import is
/// extended, a non-exported symbol yields no edits, and invalid actions
/// are refused with the pin's texts.
#[test]
fn import_adder_edits_add_coalesce_and_extend_imports() {
    const FOO: &str = "/home/projects/p/src/foo.ts";
    let session = session(&[
        (CONFIG, r#"{ "compilerOptions": { "noLib": true } }"#),
        (INDEX, "const value = foo + bar;\n"),
        (
            OTHER,
            "import { foo } from \"./foo\";\nconst value = foo + bar;\n",
        ),
        (
            FOO,
            "export const foo = 1;\nexport const bar = 2;\nconst local = 3;\n",
        ),
    ]);
    let (snapshot, project) = open(&session, CONFIG);
    // The client passes `symbol.getExportSymbol()`, which is the symbol
    // itself when the server answers null: the declaration's symbol of an
    // exported variable is already the module's export, with no export link
    // of its own (the pin's handler reads the raw field).
    let export_symbol = |position: usize| -> Value {
        let symbol = request(
            &session,
            "getSymbolAtPosition",
            &json!({"snapshot": snapshot, "project": project, "file": FOO, "position": position}),
        )
        .unwrap();
        let exported = request(
            &session,
            "getExportSymbolOfSymbol",
            &json!({"snapshot": snapshot, "project": project, "objectId": symbol["id"]}),
        )
        .unwrap();
        if exported.is_null() {
            symbol["id"].clone()
        } else {
            exported["id"].clone()
        }
    };
    let foo = export_symbol("export const ".len());
    let bar = export_symbol("export const foo = 1;\nexport const ".len());
    assert!(foo.is_number() && bar.is_number(), "{foo} {bar}");
    let edits = request(
        &session,
        "getImportAdderEdits",
        &json!({"snapshot": snapshot, "project": project, "file": INDEX, "actions": [{"kind": "importSymbol", "symbol": foo}]}),
    )
    .unwrap();
    assert_eq!(
        apply_text_edits("const value = foo + bar;\n", &edits),
        "import { foo } from \"./foo\";\n\nconst value = foo + bar;\n"
    );
    let edits = request(
        &session,
        "getImportAdderEdits",
        &json!({"snapshot": snapshot, "project": project, "file": INDEX, "actions": [
            {"kind": "importSymbol", "symbol": foo}, {"kind": "importSymbol", "symbol": bar}]}),
    )
    .unwrap();
    assert_eq!(
        apply_text_edits("const value = foo + bar;\n", &edits),
        "import { bar, foo } from \"./foo\";\n\nconst value = foo + bar;\n"
    );
    let edits = request(
        &session,
        "getImportAdderEdits",
        &json!({"snapshot": snapshot, "project": project, "file": OTHER, "actions": [{"kind": "importSymbol", "symbol": bar}]}),
    )
    .unwrap();
    assert_eq!(
        apply_text_edits(
            "import { foo } from \"./foo\";\nconst value = foo + bar;\n",
            &edits
        ),
        "import { bar, foo } from \"./foo\";\nconst value = foo + bar;\n"
    );
    let local = request(
        &session,
        "getSymbolAtPosition",
        &json!({"snapshot": snapshot, "project": project, "file": FOO, "position": "export const foo = 1;\nexport const bar = 2;\nconst ".len()}),
    )
    .unwrap();
    let edits = request(
        &session,
        "getImportAdderEdits",
        &json!({"snapshot": snapshot, "project": project, "file": INDEX, "actions": [{"kind": "importSymbol", "symbol": local["id"]}]}),
    )
    .unwrap();
    assert_eq!(edits, json!([]));
    let error = request(
        &session,
        "getImportAdderEdits",
        &json!({"snapshot": snapshot, "project": project, "file": INDEX, "actions": [{"kind": "unknown", "symbol": foo}]}),
    )
    .unwrap_err();
    assert_eq!(
        error,
        "api: client error: unknown import adder action kind \"unknown\""
    );
    let error = request(
        &session,
        "getImportAdderEdits",
        &json!({"snapshot": snapshot, "project": project, "file": INDEX, "actions": [{"kind": "importSymbol"}]}),
    )
    .unwrap_err();
    assert_eq!(
        error,
        "api: client error: import adder action 0 missing symbol"
    );
    let error = request(
        &session,
        "getImportAdderEdits",
        &json!({"snapshot": snapshot, "project": project, "file": INDEX, "actions": [{"kind": "importSymbol", "symbol": 999_999_999}]}),
    )
    .unwrap_err();
    assert_eq!(
        error,
        "api: client error: symbol handle 999999999 not found in snapshot registry"
    );
}

/// Ports `TestJSONValueToAny` of tsc/internal/api/jsonvalue_test.go: the
/// JSON a client hands `parseJsonConfigFileContent` keeps its key order,
/// its null array elements and its empty arrays on the way to `raw`.
#[test]
fn client_json_keeps_order_nulls_and_empty_arrays() {
    let session = session(&[]);
    let json = json!({"z": 1, "a": {"y": 2, "x": 3}, "m": [{"b": 4, "a": 5}, null], "e": []});
    let response = request(
        &session,
        "parseJsonConfigFileContent",
        &json!({"json": json, "configDirectory": "/home/projects/p"}),
    )
    .unwrap();
    let raw = serde_json::to_string(&response["raw"]).unwrap();
    assert_eq!(
        raw,
        r#"{"z":1,"a":{"y":2,"x":3},"m":[{"b":4,"a":5},null],"e":[]}"#
    );
}

/// Ports `TestCreateProgramWithNoRootFiles` and
/// `TestCreateProgramRemovesAllRootFiles`: a program with no roots has an
/// empty inferred project, also when it replaces an old program's roots.
#[test]
fn create_program_accepts_an_empty_root_set() {
    let session = session(&[(INDEX, "export {};")]);
    let response = request(
        &session,
        "createProgram",
        &json!({"rootFiles": [], "createProgramOptions": {"compilerOptions": {"noLib": true}}}),
    )
    .unwrap();
    assert_eq!(response["project"]["rootFiles"], json!([]));
    let data = session
        .snapshot_data(SnapshotId(response["snapshot"].as_u64().unwrap()))
        .unwrap();
    let project = data
        .project(&ProjectId(
            response["project"]["id"].as_str().unwrap().to_string(),
        ))
        .unwrap();
    assert_eq!(project.program().unwrap().files().len(), 0);

    let old = request(
        &session,
        "createProgram",
        &json!({"rootFiles": [INDEX], "createProgramOptions": {"compilerOptions": {"noLib": true}}}),
    )
    .unwrap();
    assert_eq!(old["project"]["rootFiles"], json!([INDEX]));
    let emptied = request(
        &session,
        "createProgram",
        &json!({"rootFiles": [], "createProgramOptions": {"compilerOptions": {"noLib": true}},
                "oldProgram": {"snapshot": old["snapshot"], "project": old["project"]["id"]},
                "fileChanges": {"changed": [INDEX]}}),
    )
    .unwrap();
    assert_eq!(emptied["project"]["rootFiles"], json!([]));
    let data = session
        .snapshot_data(SnapshotId(emptied["snapshot"].as_u64().unwrap()))
        .unwrap();
    let project = data
        .project(&ProjectId(
            emptied["project"]["id"].as_str().unwrap().to_string(),
        ))
        .unwrap();
    assert_eq!(project.program().unwrap().files().len(), 0);
}

/// Ports `TestCreateProgramPreservesRootFileOrder`: the response lists the
/// roots in the order the client gave them, before and after a reorder on
/// an old program. (The pin's `ProgramUpdateKind` is internal to its
/// program reuse, which the Rust session does not do.)
#[test]
fn create_program_preserves_root_file_order() {
    const A: &str = "/home/projects/p/a.ts";
    const B: &str = "/home/projects/p/b.ts";
    let session = session(&[(A, "export const a = 1;"), (B, "export const b = 1;")]);
    let old = request(
        &session,
        "createProgram",
        &json!({"rootFiles": [B, A], "createProgramOptions": {"compilerOptions": {"noLib": true}}}),
    )
    .unwrap();
    assert_eq!(old["project"]["rootFiles"], json!([B, A]));
    let reordered = request(
        &session,
        "createProgram",
        &json!({"rootFiles": [A, B], "createProgramOptions": {"compilerOptions": {"noLib": true}},
                "oldProgram": {"snapshot": old["snapshot"], "project": old["project"]["id"]}}),
    )
    .unwrap();
    assert_eq!(reordered["project"]["rootFiles"], json!([A, B]));
}

/// Ports the observable half of `TestCreateProgramReusesProgram`: a changed
/// file is re-read for the new program and changed options take effect.
/// The pin's reuse kinds (`Cloned`, `SameFileNames`) have no counterpart:
/// the Rust session loads the program afresh (docs/PHASE6-A2.md).
#[test]
fn create_program_with_an_old_program_sees_changes_and_new_options() {
    let session = session(&[(INDEX, "export const value: string = 1;")]);
    let options = json!({"compilerOptions": {"noLib": true, "strict": true}});
    let old = request(
        &session,
        "createProgram",
        &json!({"rootFiles": [INDEX], "createProgramOptions": options}),
    )
    .unwrap();
    let before = request(
        &session,
        "getSemanticDiagnostics",
        &json!({"snapshot": old["snapshot"], "project": old["project"]["id"], "files": [INDEX]}),
    )
    .unwrap();
    assert_eq!(before.as_array().unwrap().len(), 1, "{before}");
    session
        .file_system()
        .write_file(INDEX.as_bytes(), b"export const value: string = \"valid\";")
        .unwrap();
    let updated = request(
        &session,
        "createProgram",
        &json!({"rootFiles": [INDEX], "createProgramOptions": options,
                "oldProgram": {"snapshot": old["snapshot"], "project": old["project"]["id"]},
                "fileChanges": {"changed": [INDEX]}}),
    )
    .unwrap();
    let after = request(
        &session,
        "getSemanticDiagnostics",
        &json!({"snapshot": updated["snapshot"], "project": updated["project"]["id"], "files": [INDEX]}),
    )
    .unwrap();
    assert_eq!(after, json!([]));
    let relaxed = request(
        &session,
        "createProgram",
        &json!({"rootFiles": [INDEX], "createProgramOptions": {"compilerOptions": {"noLib": true, "strict": false}},
                "oldProgram": {"snapshot": old["snapshot"], "project": old["project"]["id"]}}),
    )
    .unwrap();
    assert_eq!(
        relaxed["project"]["compilerOptions"]["strict"],
        json!(false),
        "{relaxed}"
    );
}

/// Ports `TestCreateProgramProjectReferencesAndReuse`'s observable half: the
/// references a client gives are echoed by the project's command line and
/// resolved by its program; a changed reference replaces the old one.
#[test]
fn create_program_resolves_project_references() {
    const APP: &str = "/home/projects/app/index.ts";
    const LIB_CONFIG: &str = "/home/projects/lib/tsconfig.json";
    const OTHER_CONFIG: &str = "/home/projects/other/tsconfig.json";
    let session = session(&[
        (APP, "export const value: string = 1;"),
        (
            LIB_CONFIG,
            r#"{ "compilerOptions": { "composite": true, "noLib": true }, "files": ["index.ts"] }"#,
        ),
        ("/home/projects/lib/index.ts", "export const lib = 1;"),
        (
            OTHER_CONFIG,
            r#"{ "compilerOptions": { "composite": true, "noLib": true }, "files": ["index.ts"] }"#,
        ),
        ("/home/projects/other/index.ts", "export const other = 1;"),
    ]);
    // The pin's `core.ProjectReference` writes `circular` without omitempty.
    let reference =
        |config: &str| json!({"path": config, "originalPath": config, "circular": false});
    let old = request(
        &session,
        "createProgram",
        &json!({"rootFiles": [APP], "createProgramOptions": {"compilerOptions": {"noLib": true, "strict": true},
                "projectReferences": [reference(LIB_CONFIG)]}}),
    )
    .unwrap();
    assert_eq!(
        old["project"]["parsedCommandLine"]["projectReferences"],
        json!([reference(LIB_CONFIG)]),
        "{old}"
    );
    let data = session
        .snapshot_data(SnapshotId(old["snapshot"].as_u64().unwrap()))
        .unwrap();
    let project = data
        .project(&ProjectId(
            old["project"]["id"].as_str().unwrap().to_string(),
        ))
        .unwrap();
    let resolved: Vec<String> = project
        .program()
        .unwrap()
        .resolved_project_references()
        .flatten()
        .map(|command_line| {
            String::from_utf8_lossy(
                command_line
                    .config_file
                    .as_ref()
                    .unwrap()
                    .file
                    .view()
                    .source_file(command_line.config_file.as_ref().unwrap().root)
                    .unwrap()
                    .file_name(),
            )
            .into_owned()
        })
        .collect();
    assert_eq!(resolved, vec![LIB_CONFIG.to_string()]);
    let changed = request(
        &session,
        "createProgram",
        &json!({"rootFiles": [APP], "createProgramOptions": {"compilerOptions": {"noLib": true, "strict": true},
                "projectReferences": [reference(OTHER_CONFIG)]},
                "oldProgram": {"snapshot": old["snapshot"], "project": old["project"]["id"]}}),
    )
    .unwrap();
    assert_eq!(
        changed["project"]["parsedCommandLine"]["projectReferences"],
        json!([reference(OTHER_CONFIG)]),
        "{changed}"
    );
}

/// Ports `TestCreateProgramFromConfiguredProgramDoesNotRetainOtherProjects`:
/// a program created from a configured project's roots lives in a snapshot
/// with that inferred project alone, and sees the changed file.
#[test]
fn create_program_from_a_configured_project_drops_the_other_projects() {
    const OTHER_CONFIG: &str = "/home/projects/other/tsconfig.json";
    const OTHER_FILE: &str = "/home/projects/other/index.ts";
    let session = session(&[
        (
            CONFIG,
            r#"{ "compilerOptions": { "noLib": true, "strict": true }, "files": ["src/index.ts"] }"#,
        ),
        (INDEX, "export const value: string = 1;"),
        (
            OTHER_CONFIG,
            r#"{ "compilerOptions": { "noLib": true }, "files": ["index.ts"] }"#,
        ),
        (OTHER_FILE, "export const other = 1;"),
    ]);
    let update = session
        .handle_update_snapshot(&UpdateSnapshotParams {
            open_projects: vec![
                DocumentIdentifier {
                    file_name: CONFIG.into(),
                    uri: String::new(),
                },
                DocumentIdentifier {
                    file_name: OTHER_CONFIG.into(),
                    uri: String::new(),
                },
            ],
            ..Default::default()
        })
        .unwrap();
    let base = update
        .projects
        .iter()
        .flatten()
        .find(|project| project.config_file_name == CONFIG)
        .expect("the configured project");
    session
        .file_system()
        .write_file(INDEX.as_bytes(), b"export const value: string = \"valid\";")
        .unwrap();
    let updated = request(
        &session,
        "createProgram",
        &json!({"rootFiles": base.root_files, "createProgramOptions": {"compilerOptions": {"noLib": true, "strict": true}},
                "oldProgram": {"snapshot": update.snapshot.0, "project": base.id.0},
                "fileChanges": {"changed": [INDEX]}}),
    )
    .unwrap();
    let data = session
        .snapshot_data(SnapshotId(updated["snapshot"].as_u64().unwrap()))
        .unwrap();
    let projects: Vec<String> = data
        .snapshot
        .projects_by_path()
        .map(|(path, _)| String::from_utf8_lossy(path.as_bytes()).into_owned())
        .collect();
    assert_eq!(projects.len(), 1, "{projects:?}");
    let diagnostics = request(
        &session,
        "getSemanticDiagnostics",
        &json!({"snapshot": updated["snapshot"], "project": updated["project"]["id"], "files": [INDEX]}),
    )
    .unwrap();
    assert_eq!(diagnostics, json!([]));
    assert_eq!(
        request(
            &session,
            "release",
            &json!({"snapshot": updated["snapshot"]})
        )
        .unwrap(),
        json!(true)
    );
}

/// Ports `TestUpdateTemporarySnapshotAddsUnopenedFile` and
/// `TestUpdateTemporarySnapshotUsesClientSnapshotAsBase`: a temporary file
/// joins the configured project's roots in the temporary snapshot only, and
/// the temporary snapshot derives from the client's base, not from a later
/// snapshot's state.
#[test]
fn temporary_snapshots_add_unopened_files_and_derive_from_the_client_base() {
    const TEMPORARY: &str = "/home/projects/p/src/temporary.ts";
    const LATER: &str = "/home/projects/p/src/later.ts";
    let session = session(&[
        (
            CONFIG,
            r#"{ "compilerOptions": { "noLib": true }, "include": ["src/**/*.ts"] }"#,
        ),
        (INDEX, "export const existing = 1;"),
    ]);
    let base = session
        .handle_update_snapshot(&UpdateSnapshotParams {
            open_files: vec![DocumentIdentifier {
                file_name: INDEX.into(),
                uri: String::new(),
            }],
            ..Default::default()
        })
        .unwrap();
    let base_roots: Vec<String> = base.projects[0].as_ref().unwrap().root_files.clone();
    assert!(
        !base_roots.iter().any(|root| root == TEMPORARY),
        "{base_roots:?}"
    );
    let temporary = request(
        &session,
        "updateTemporarySnapshot",
        &json!({"snapshot": base.snapshot.0, "file": TEMPORARY, "newText": "export const temporary = 1;"}),
    )
    .unwrap();
    let roots = temporary["projects"][0]["rootFiles"].as_array().unwrap();
    assert!(roots.iter().any(|root| root == TEMPORARY), "{temporary}");
    assert!(
        !base_roots.iter().any(|root| root == TEMPORARY),
        "the base stays unchanged"
    );
    assert_eq!(
        request(
            &session,
            "release",
            &json!({"snapshot": temporary["snapshot"]})
        )
        .unwrap(),
        json!(true)
    );

    // A file created and opened after the client's snapshot is not in a
    // temporary snapshot derived from it. (The pin's test opens the file
    // through the LSP session; the standalone session learns of the new
    // file through the update's file changes.)
    session
        .file_system()
        .write_file(LATER.as_bytes(), b"export const later = 1;")
        .unwrap();
    let later = session
        .handle_update_snapshot(&UpdateSnapshotParams {
            open_files: vec![DocumentIdentifier {
                file_name: LATER.into(),
                uri: String::new(),
            }],
            file_changes: Some(Box::new(crate::proto::ApiFileChanges {
                created: vec![DocumentIdentifier {
                    file_name: LATER.into(),
                    uri: String::new(),
                }],
                ..Default::default()
            })),
            ..Default::default()
        })
        .unwrap();
    assert!(
        later.projects[0]
            .as_ref()
            .unwrap()
            .root_files
            .iter()
            .any(|root| root == LATER),
        "{:?}",
        later.projects[0]
    );
    let temporary = request(
        &session,
        "updateTemporarySnapshot",
        &json!({"snapshot": base.snapshot.0, "file": INDEX, "newText": "export const existing = 2;"}),
    )
    .unwrap();
    let roots = temporary["projects"][0]["rootFiles"].as_array().unwrap();
    assert!(!roots.iter().any(|root| root == LATER), "{temporary}");
}

/// Ports `TestUpdateSnapshotResponseSkipsUnloadedAncestorProject`: opening a
/// nested project reports it with its roots and options, and not an
/// ancestor config that is known but not loaded.
#[test]
fn update_snapshot_reports_loaded_projects_only() {
    const NESTED: &str = "/repo/packages/app/tsconfig.json";
    const ANCESTOR: &str = "/repo/packages/tsconfig.json";
    const FILE: &str = "/repo/packages/app/src/index.ts";
    let session = session(&[
        (ANCESTOR, r#"{ "files": [] }"#),
        (
            NESTED,
            r#"{ "compilerOptions": { "composite": true, "noLib": true }, "include": ["**/*"] }"#,
        ),
        (FILE, "let s: string = 1234;"),
    ]);
    let update = session
        .handle_update_snapshot(&UpdateSnapshotParams {
            open_files: vec![DocumentIdentifier {
                file_name: FILE.into(),
                uri: String::new(),
            }],
            open_projects: vec![DocumentIdentifier {
                file_name: NESTED.into(),
                uri: String::new(),
            }],
            ..Default::default()
        })
        .unwrap();
    let names: Vec<&str> = update
        .projects
        .iter()
        .flatten()
        .map(|project| project.config_file_name.as_str())
        .collect();
    assert!(names.contains(&NESTED), "{names:?}");
    assert!(!names.contains(&ANCESTOR), "{names:?}");
    let nested = update
        .projects
        .iter()
        .flatten()
        .find(|project| project.config_file_name == NESTED)
        .unwrap();
    assert!(!nested.root_files.is_empty());
    assert!(nested.compiler_options.is_some());
}
