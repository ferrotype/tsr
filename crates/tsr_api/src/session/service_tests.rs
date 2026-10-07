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
    let data = session.snapshot_data(SnapshotId(snapshot)).unwrap();
    let setup = data.setup_checker(&ProjectId(project.to_string())).unwrap();
    let operation = setup.registry.operation().unwrap();
    let node = touching_property_name(setup.program, file.as_bytes(), position)
        .unwrap()
        .expect("a token at the position");
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
    // at its location and yields null, as the Go handler does.
    let declaration = symbol["declarations"][0].as_str().unwrap();
    let none = request(
        &session,
        "getReferencedSymbolsForNode",
        &json!({"snapshot": snapshot, "project": project, "node": declaration, "position": 14}),
    )
    .unwrap();
    assert!(none.is_null(), "{none}");
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
