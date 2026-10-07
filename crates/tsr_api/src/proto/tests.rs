//! The pinned wire observations of `data/s03/schema/api-wire-fixtures.json`
//! (copied beside this module), the proto_test.go ports and the method table.
use super::*;
use tsr_json::{Decoder, Options, RawValue};

const FIXTURES: &str = include_str!("wire-fixtures.json");

#[derive(serde::Deserialize)]
struct Fixture {
    id: String,
    operation: String,
    #[serde(rename = "goType")]
    go_type: String,
    input: serde_json::Value,
    #[serde(default)]
    output: serde_json::Value,
    #[serde(default)]
    error: Option<serde_json::Value>,
}
#[derive(serde::Deserialize)]
struct File {
    fixtures: Vec<Fixture>,
}

fn marshal(value: &impl tsr_json::Encode) -> Result<String, tsr_json::Error> {
    tsr_json::marshal(value, Options::default()).map(|bytes| String::from_utf8(bytes).unwrap())
}

fn decode<T: tsr_json::Decode + Default>(text: &str) -> Result<T, tsr_json::Error> {
    let mut value = T::default();
    tsr_json::unmarshal(text.as_bytes(), &mut value, Options::default())?;
    Ok(value)
}

/// The Rust value each marshal fixture's symbolic input names.
fn marshal_fixture(id: &str) -> Result<String, tsr_json::Error> {
    match id {
        "ordinary-empty-options" => marshal(&UpdateSnapshotParams::default()),
        "ordinary-empty-slices" => marshal(&UpdateSnapshotParams {
            open_projects: Vec::new(),
            ..Default::default()
        }),
        "ordinary-required-zero-id" => marshal(&ReleaseParams::default()),
        "ordinary-max-uint64-id" => marshal(&ReleaseParams {
            snapshot: SnapshotId(u64::MAX),
        }),
        "ordinary-null-interface" => marshal(&ReadConfigFileResponse::default()),
        "batch-empty-responses" => marshal(&BatchRequestsResponse::default()),
        "batch-continuation" => marshal(&BatchRequestsResponse {
            continuation_token: "next".into(),
            ..Default::default()
        }),
        "tristate-unknown" => marshal(&tsr_core::Tristate::UNKNOWN),
        "tristate-true" => marshal(&tsr_core::Tristate::TRUE),
        "tristate-false" => marshal(&tsr_core::Tristate::FALSE),
        "enum-jsx-emit" => marshal(&tsr_core::compiler_options::JsxEmit(2)),
        "enum-module-detection" => marshal(&tsr_core::compiler_options::ModuleDetectionKind(2)),
        "enum-module-kind" => marshal(&tsr_core::compiler_options::ModuleKind(2)),
        "enum-module-resolution" => marshal(&tsr_core::compiler_options::ModuleResolutionKind(2)),
        "enum-newline" => marshal(&tsr_core::compiler_options::NewLineKind(1)),
        "enum-script-target" => marshal(&tsr_core::ScriptTarget(2)),
        "literal-method" => marshal(&Method::Release),
        "literal-import-adder" => marshal(&ImportAdderActionKind(
            ImportAdderActionKind::IMPORT_SYMBOL.into(),
        )),
        "raw-json-object" => marshal(&RawValue(br#"{"z":1,"a":[true,null]}"#.to_vec())),
        "raw-json-invalid" => marshal(&RawValue(br#"{"unterminated":"#.to_vec())),
        "ordered-map-insertion-order" => {
            let mut map = tsr_core::collections::OrderedMap::default();
            map.insert("z".to_string(), vec!["last".to_string()]);
            map.insert("a".to_string(), Vec::new());
            marshal(&StringListMap(map))
        }
        other => panic!("unknown marshal fixture {other}"),
    }
}

#[test]
fn pinned_wire_fixtures_hold() {
    let file: File = serde_json::from_str(FIXTURES).unwrap();
    assert_eq!(file.fixtures.len(), 36);
    let mut seen = 0;
    for fixture in &file.fixtures {
        seen += 1;
        match (fixture.operation.as_str(), fixture.go_type.as_str()) {
            ("marshal", _) => {
                let actual = marshal_fixture(&fixture.id);
                match (&fixture.error, actual) {
                    (None, Ok(actual)) => {
                        assert_eq!(actual, fixture.output.as_str().unwrap(), "{}", fixture.id);
                    }
                    (Some(_), Err(_)) => {}
                    (expected, actual) => panic!(
                        "{}: expected error {:?}, got {actual:?}",
                        fixture.id,
                        expected.is_some()
                    ),
                }
            }
            ("unmarshal", "api.DocumentIdentifier") => {
                let input = fixture.input.as_str().unwrap();
                match (&fixture.error, decode::<DocumentIdentifier>(input)) {
                    (None, Ok(value)) => {
                        assert_eq!(
                            value.file_name, fixture.output["fileName"],
                            "{}",
                            fixture.id
                        );
                        assert_eq!(value.uri, fixture.output["uri"], "{}", fixture.id);
                    }
                    (Some(error), Err(actual)) => {
                        let reason = error["cause"]["reason"].as_str().unwrap_or_default();
                        assert!(
                            actual
                                .to_string()
                                .contains(reason.split(':').next().unwrap()),
                            "{}: {actual} does not carry {reason:?}",
                            fixture.id
                        );
                    }
                    (expected, actual) => panic!(
                        "{}: expected error {:?}, got {actual:?}",
                        fixture.id,
                        expected.is_some()
                    ),
                }
            }
            ("unmarshal", "core.Tristate") => {
                let value: tsr_core::Tristate = decode(fixture.input.as_str().unwrap()).unwrap();
                assert_eq!(
                    i64::from(value.0),
                    fixture.output.as_i64().unwrap(),
                    "{}",
                    fixture.id
                );
            }
            ("unmarshal", "packagejson.JSONValue") => {
                let value: PackageJsonValue = decode(fixture.input.as_str().unwrap()).unwrap();
                assert_eq!(
                    value.kind_name(),
                    fixture.output.as_str().unwrap(),
                    "{}",
                    fixture.id
                );
            }
            other => panic!("{}: unhandled fixture {other:?}", fixture.id),
        }
    }
    assert_eq!(seen, 36);
}

#[test]
fn the_copied_fixture_file_matches_the_schema_export() {
    let repository = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../data/s03/schema/api-wire-fixtures.json"
    );
    if let Ok(source) = std::fs::read_to_string(repository) {
        assert_eq!(
            source, FIXTURES,
            "copy data/s03/schema/api-wire-fixtures.json into the crate"
        );
    }
}

#[test]
fn every_method_round_trips_through_its_wire_name() {
    assert_eq!(Method::ALL.len(), 142);
    for method in Method::ALL {
        assert_eq!(Method::from_wire(method.wire()), Some(method));
        assert_eq!(marshal(&method).unwrap(), format!("{:?}", method.wire()));
        let decoded: Method = decode(&format!("{:?}", method.wire())).unwrap();
        assert_eq!(decoded, method);
    }
    assert_eq!(Method::from_wire("echo"), None);
    assert!(decode::<Method>("\"nope\"").is_err());
}

#[test]
fn parameters_decode_as_the_pin_does() {
    let params = Params::decode(
        Method::GetSymbolAtPosition,
        br#"{"snapshot":3,"project":"/tsconfig.json","file":{"uri":"file:///a.ts"},"position":7,"extra":true}"#,
    )
    .unwrap();
    assert_eq!(params.method(), Method::GetSymbolAtPosition);
    let Params::GetSymbolAtPosition(inner) = params else {
        panic!("wrong variant");
    };
    assert_eq!(inner.snapshot, SnapshotId(3));
    assert_eq!(inner.project, ProjectId("/tsconfig.json".into()));
    assert_eq!(inner.file.uri, "file:///a.ts");
    assert_eq!(inner.position, 7);
    // Methods without parameters ignore the payload entirely.
    assert_eq!(
        Params::decode(Method::Initialize, b"garbage").unwrap(),
        Params::Initialize
    );
    // A null decodes a struct to its zero value; duplicate names are errors.
    assert_eq!(
        Params::decode(Method::Release, b"null").unwrap(),
        Params::Release(Box::default())
    );
    assert!(Params::decode(Method::Release, br#"{"snapshot":1,"snapshot":2}"#).is_err());
}

#[test]
fn omission_rules_follow_json_v2() {
    // omitempty never leaves out a number or a boolean; omitzero does.
    let response = TypeResponse::default();
    let text = marshal(&response).unwrap();
    assert!(text.contains("\"objectFlags\":0"), "{text}");
    assert!(text.contains("\"isTupleType\":false"), "{text}");
    assert!(!text.contains("\"target\""), "{text}");
    assert!(text.contains("\"value\":null"), "{text}");
    // A nil slice encodes as [] when not omitted, and a pointer as null.
    let project = ProjectResponse::default();
    let text = marshal(&project).unwrap();
    assert!(text.contains("\"rootFiles\":[]"), "{text}");
    assert!(text.contains("\"parsedCommandLine\":null"), "{text}");
    // Empty strings and slices are omitted under omitempty.
    let symbol = SymbolResponse::default();
    let text = marshal(&symbol).unwrap();
    assert_eq!(
        text,
        r#"{"id":0,"project":"","name":"","flags":0,"checkFlags":0}"#
    );
}

#[test]
fn compiler_options_decode_by_their_tags() {
    let value: CompilerOptionsValue =
        decode(r#"{"strict":true,"module":1,"lib":["es2022"],"nope":1}"#).unwrap();
    assert_eq!(value.0.strict, tsr_core::Tristate::TRUE);
    assert_eq!(
        value.0.module,
        tsr_core::compiler_options::ModuleKind::COMMON_JS
    );
    assert_eq!(
        marshal(&value).unwrap(),
        r#"{"lib":["es2022"],"module":1,"strict":true}"#
    );
    let reference: ProjectReferenceValue = decode(r#"{"path":"../a"}"#).unwrap();
    assert_eq!(
        marshal(&reference).unwrap(),
        r#"{"path":"../a","originalPath":"","circular":false}"#
    );
    let acquisition: TypeAcquisitionValue = decode(r#"{"enable":true,"include":[]}"#).unwrap();
    assert_eq!(
        marshal(&acquisition).unwrap(),
        r#"{"enable":true,"include":[]}"#
    );
}

#[test]
fn document_identifiers_resolve_against_the_working_directory() {
    let by_name = DocumentIdentifier::file("src/a.ts");
    assert_eq!(
        by_name.to_absolute_file_name(b"/work").as_bytes(),
        b"/work/src/a.ts"
    );
    assert_eq!(by_name.to_uri(b"/work").0, "file:///work/src/a.ts");
    assert_eq!(by_name.to_file_name().as_bytes(), b"src/a.ts");
    let by_uri: DocumentIdentifier = decode(r#"{"uri":"file:///work/b.ts"}"#).unwrap();
    assert_eq!(by_uri.to_file_name().as_bytes(), b"/work/b.ts");
    assert_eq!(by_uri.to_string(), "file:///work/b.ts");
    assert_eq!(marshal(&by_uri).unwrap(), r#"{"uri":"file:///work/b.ts"}"#);
    let _ = Decoder::from_slice(b"");
}
