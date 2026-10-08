//! The checker-backed handlers over a memory file system, driven through
//! the wire dispatch with JSON payloads as a client would send them.
use super::*;
use crate::proto::DocumentIdentifier;
use serde_json::{json, Value};
use tsr_vfs::{
    iovfs,
    vfstest::{self, InputFile},
};

const CONFIG: &str = "/home/projects/p/tsconfig.json";
const INDEX: &str = "/home/projects/p/src/index.ts";
const OTHER_CONFIG: &str = "/home/projects/q/tsconfig.json";
const OTHER_INDEX: &str = "/home/projects/q/src/main.ts";

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
    let project = update
        .projects
        .iter()
        .flatten()
        .find(|project| project.config_file_name == config)
        .expect("the opened project")
        .id
        .0
        .clone();
    (update.snapshot.0, project)
}

const SOURCE: &str = "export const answer: number = 42;\nexport function greet(name: string): string { return name; }\nexport type Pair<T> = [T, T];\n";

#[test]
fn symbols_and_types_resolve_through_handles() {
    let session = session(&[
        (
            CONFIG,
            r#"{ "compilerOptions": { "strict": true, "noLib": true } }"#,
        ),
        (INDEX, SOURCE),
    ]);
    let (snapshot, project) = open(&session, CONFIG);
    // `answer` starts at column 13 of the first line.
    let symbol = request(
        &session,
        "getSymbolAtPosition",
        &json!({"snapshot": snapshot, "project": project, "file": INDEX, "position": 14}),
    )
    .unwrap();
    assert_eq!(symbol["name"], "answer", "{symbol}");
    assert_eq!(symbol["project"], project);
    let declarations = symbol["declarations"].as_array().expect("declarations");
    assert_eq!(declarations.len(), 1);
    let handle = declarations[0].as_str().unwrap();
    assert!(
        handle.ends_with(&format!(".{INDEX}")),
        "index.kind.path: {handle}"
    );
    let symbol_id = symbol["id"].as_u64().unwrap();
    assert!(symbol_id > 0);

    let ty = request(
        &session,
        "getTypeOfSymbol",
        &json!({"snapshot": snapshot, "project": project, "symbol": symbol_id}),
    )
    .unwrap();
    assert_eq!(ty["intrinsicName"], "number", "{ty}");
    let type_id = ty["id"].as_u64().unwrap();
    let printed = request(
        &session,
        "typeToString",
        &json!({"snapshot": snapshot, "project": project, "type": type_id, "location": "", "flags": 0}),
    )
    .unwrap();
    assert_eq!(printed, "number");

    // The declaration node itself has no symbol (the pin's GetSymbolAtLocation
    // answers names), but its type is the declared one, and the type handle
    // is the same id as the one the symbol's type produced.
    let none = request(
        &session,
        "getSymbolAtLocation",
        &json!({"snapshot": snapshot, "project": project, "location": handle}),
    )
    .unwrap();
    assert_eq!(none, Value::Null);
    let at_location = request(
        &session,
        "getTypeAtLocation",
        &json!({"snapshot": snapshot, "project": project, "location": handle}),
    )
    .unwrap();
    assert_eq!(at_location["id"], type_id, "one handle per type identity");

    let function = request(
        &session,
        "getSymbolAtPosition",
        &json!({"snapshot": snapshot, "project": project, "file": INDEX, "position": 51}),
    )
    .unwrap();
    assert_eq!(function["name"], "greet", "{function}");
    let function_type = request(
        &session,
        "getTypeOfSymbol",
        &json!({"snapshot": snapshot, "project": project, "symbol": function["id"]}),
    )
    .unwrap();
    let signatures = request(
        &session,
        "getSignaturesOfType",
        &json!({"snapshot": snapshot, "project": project, "type": function_type["id"], "kind": 0}),
    )
    .unwrap();
    let signatures = signatures.as_array().unwrap();
    assert_eq!(signatures.len(), 1, "{signatures:?}");
    assert!(
        signatures[0]["id"].as_u64().is_some_and(|id| id > 0),
        "signature id: {:?}",
        signatures[0]
    );
    let parameters = request(
        &session,
        "getParametersOfSignature",
        &json!({"snapshot": snapshot, "project": project, "objectId": signatures[0]["id"]}),
    )
    .unwrap();
    assert_eq!(parameters[0]["name"], "name");
    let return_type = request(
        &session,
        "getReturnTypeOfSignature",
        &json!({"snapshot": snapshot, "project": project, "objectId": signatures[0]["id"]}),
    )
    .unwrap();
    assert_eq!(return_type["intrinsicName"], "string");

    let in_scope = request(
        &session,
        "getSymbolsInScope",
        &json!({"snapshot": snapshot, "project": project, "location": handle, "meaning": 111_551}),
    )
    .unwrap();
    let names: Vec<&str> = in_scope
        .as_array()
        .unwrap()
        .iter()
        .map(|symbol| symbol["name"].as_str().unwrap())
        .collect();
    assert!(
        names.contains(&"answer") && names.contains(&"greet"),
        "{names:?}"
    );
}

#[test]
fn intrinsic_and_well_known_handles_are_stable_within_a_snapshot() {
    let session = session(&[
        (CONFIG, r#"{ "compilerOptions": { "noLib": true } }"#),
        (INDEX, SOURCE),
    ]);
    let (snapshot, project) = open(&session, CONFIG);
    let first = request(
        &session,
        "getStringType",
        &json!({"snapshot": snapshot, "project": project}),
    )
    .unwrap();
    let second = request(
        &session,
        "getStringType",
        &json!({"snapshot": snapshot, "project": project}),
    )
    .unwrap();
    assert_eq!(first["id"], second["id"]);
    assert_eq!(first["intrinsicName"], "string");
    let known = request(
        &session,
        "getWellKnownSymbols",
        &json!({"snapshot": snapshot, "project": project}),
    )
    .unwrap();
    assert!(known["unknown"].as_u64().unwrap() > 0 && known["undefined"] != known["unknown"]);
    let signatures = request(
        &session,
        "getWellKnownSignatures",
        &json!({"snapshot": snapshot, "project": project}),
    )
    .unwrap();
    assert!(signatures["unknown"].as_u64().unwrap() > 0);
}

/// The plan's registry witness: a type handle minted on one project's
/// registry is rejected on another project's with the pin's text.
#[test]
fn a_type_handle_of_one_project_is_rejected_by_another() {
    let session = session(&[
        (CONFIG, r#"{ "compilerOptions": { "noLib": true } }"#),
        (INDEX, SOURCE),
        (OTHER_CONFIG, r#"{ "compilerOptions": { "noLib": true } }"#),
        (OTHER_INDEX, "export const other = 'x';\n"),
    ]);
    let (_, project) = open(&session, CONFIG);
    let (snapshot, other) = open(&session, OTHER_CONFIG);
    let ty = request(
        &session,
        "getStringType",
        &json!({"snapshot": snapshot, "project": project}),
    )
    .unwrap();
    let error = request(
        &session,
        "getTypeArguments",
        &json!({"snapshot": snapshot, "project": other, "type": ty["id"]}),
    )
    .unwrap_err();
    assert!(
        error.contains("not found"),
        "a handle of another project is unknown to this registry: {error}"
    );
    let error = request(
        &session,
        "getTypeOfSymbol",
        &json!({"snapshot": snapshot, "project": other, "symbol": 0}),
    )
    .unwrap_err();
    assert_eq!(error, "api: client error: empty symbol handle");
    let error = request(
        &session,
        "getTypeOfSymbol",
        &json!({"snapshot": snapshot, "project": other, "symbol": 999}),
    )
    .unwrap_err();
    assert_eq!(
        error,
        "api: client error: symbol handle 999 not found in snapshot registry"
    );
    let error = request(
        &session,
        "getSymbolAtLocation",
        &json!({"snapshot": snapshot, "project": other, "location": "nonsense"}),
    )
    .unwrap_err();
    assert_eq!(error, "api: client error: invalid node handle \"nonsense\"");
}
