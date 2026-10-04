//! Codec matrix from the pinned lsp_json_test.go plus reuse/dispatch boundaries.
//! `tools/phase5/lsproto/check.py` checks these expectations against Go directly.
use serde_json::{json, Value};
use tsr_json::{Decode, Encode, Options};
use tsr_lsproto::*;

fn run_case<T: Decode + Encode + Default>(case: &Value) -> Vec<Value> {
    let mut value = T::default();
    case["inputs"].as_array().unwrap().iter().map(|input| {
        let result = tsr_json::unmarshal(input.as_str().unwrap().as_bytes(), &mut value, Options::default());
        let contains = case["error_contains"].as_str().unwrap();
        if !contains.is_empty() {
            assert!(result.as_ref().is_err_and(|e| e.to_string().contains(contains)), "{}: expected {contains}, got {result:?}", case["name"]);
        }
        let encoded = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| tsr_json::marshal_partial(&value, Options { deterministic: Some(true), ..Options::default() })));
        match encoded {
            Ok((bytes, marshal)) => json!({"ok":result.is_ok(), "encoded":String::from_utf8(bytes).unwrap(), "marshal_panicked":false, "marshal_failed":marshal.is_err()}),
            Err(_) => json!({"ok":result.is_ok(), "encoded":"", "marshal_panicked":true, "marshal_failed":false}),
        }
    }).collect()
}

#[test]
fn pinned_codec_matrix() {
    let cases: Value = serde_json::from_str(include_str!(
        "../../../tools/phase5/lsproto/codec-cases.json"
    ))
    .unwrap();
    let expected: Value = serde_json::from_str(include_str!(
        "../../../tools/phase5/lsproto/codec-expected.json"
    ))
    .unwrap();
    macro_rules! dispatch { ($case:expr, $($ty:ty),+ $(,)?) => { match $case["type"].as_str().unwrap() { $(stringify!($ty) => run_case::<$ty>($case),)+ other => panic!("unknown codec case {other}") } }; }
    let mut count = 0;
    for case in cases.as_array().unwrap() {
        let observed = dispatch!(
            case,
            BooleanOrHoverOptions,
            CallHierarchyIncomingCall,
            CallHierarchyIncomingCallsParams,
            ClientInfo,
            CompletionItem,
            DidChangeConfigurationParams,
            FoldingRange,
            Hover,
            InitializationOptions,
            InitializeParams,
            InitializeResult,
            InlayHint,
            IntegerOrNull,
            IntegerOrString,
            Location,
            Position,
            Registration,
            SemanticTokens,
            StringLiteralCreate,
            StringOrTuple,
            TextDocumentEdit,
            TextDocumentEditOrCreateFileOrRenameFileOrDeleteFile,
            TextEditOrInsertReplaceEdit,
            WorkDoneProgressBeginOrReportOrEnd,
            WorkDoneProgressOptions,
            WorkspaceEdit
        );
        let name = case["name"].as_str().unwrap();
        assert_eq!(json!(observed), expected[name], "{name}");
        count += observed.len();
    }
    assert_eq!(count, 107);
}

#[test]
fn parameter_error_responses_match_the_pinned_server() {
    let cases: Value = serde_json::from_str(include_str!(
        "../../../tools/phase5/lsproto/params-cases.json"
    ))
    .unwrap();
    let expected: Value = serde_json::from_str(include_str!(
        "../../../tools/phase5/lsproto/params-expected.json"
    ))
    .unwrap();
    fn response<T: Decode + Default + 'static>(raw: Option<&tsr_json::RawValue>) -> Value {
        let message = match unmarshal_params::<T>(raw) {
            Ok(_) => Message {
                id: Some(Id::int(7)),
                result: Some(tsr_json::RawValue(b"null".to_vec())),
                ..Default::default()
            },
            Err(error) => Message {
                id: Some(Id::int(7)),
                error: Some(error),
                ..Default::default()
            },
        };
        serde_json::from_slice(&tsr_json::marshal(&message, Options::default()).unwrap()).unwrap()
    }
    for case in cases.as_array().unwrap() {
        let raw = case["params"]
            .as_str()
            .map(|s| tsr_json::RawValue(s.as_bytes().to_vec()));
        let actual = match case["type"].as_str().unwrap() {
            "NoParams" => response::<NoParams>(raw.as_ref()),
            "InitializedParams" => response::<InitializedParams>(raw.as_ref()),
            "InitializeParams" => response::<InitializeParams>(raw.as_ref()),
            "HoverParams" => response::<HoverParams>(raw.as_ref()),
            other => panic!("unknown parameter type {other}"),
        };
        let name = case["name"].as_str().unwrap();
        assert_eq!(actual, expected[name], "{name}");
    }
}

fn decode<T: Decode + Default>(json: &str) -> T {
    let mut value = T::default();
    tsr_json::unmarshal(json.as_bytes(), &mut value, Options::default()).unwrap();
    value
}

#[test]
fn selected_arms_are_observable_and_invalid_discriminators_do_not_fall_through() {
    let begin: WorkDoneProgressBeginOrReportOrEnd =
        decode(r#"{"kind":"begin","title":"Indexing"}"#);
    assert_eq!(begin.begin.unwrap().title, "Indexing");
    assert!(begin.report.is_none() && begin.end.is_none());
    let value: TextDocumentEditOrCreateFileOrRenameFileOrDeleteFile =
        decode(r#"{"kind":"create","uri":"file:///x.ts"}"#);
    assert_eq!(
        value.create_file.unwrap().uri,
        DocumentUri::from("file:///x.ts")
    );
    assert!(
        value.text_document_edit.is_none()
            && value.rename_file.is_none()
            && value.delete_file.is_none()
    );
    let mut value = WorkDoneProgressBeginOrReportOrEnd::default();
    assert!(tsr_json::unmarshal(br#"{"kind":"begin"}"#, &mut value, Options::default()).is_err());
    // A known discriminator commits the selected arm before its required-field
    // check fails. Trying the looser Report or End arms would hide the error.
    assert!(value.begin.is_some() && value.report.is_none() && value.end.is_none());
}

#[test]
fn presence_dispatch_uses_input_key_order() {
    let range = r#"{"start":{"line":0,"character":0},"end":{"line":0,"character":1}}"#;
    let first: TextEditOrInsertReplaceEdit = decode(&format!(
        r#"{{"range":{range},"insert":{range},"replace":{range},"newText":"x"}}"#
    ));
    let second: TextEditOrInsertReplaceEdit = decode(&format!(
        r#"{{"insert":{range},"range":{range},"replace":{range},"newText":"x"}}"#
    ));
    assert!(first.text_edit.is_some() && first.insert_replace_edit.is_none());
    assert!(second.insert_replace_edit.is_some() && second.text_edit.is_none());
}

#[test]
fn parameters_distinguish_absent_from_null_and_scalars() {
    assert!(unmarshal_params::<NoParams>(None).is_ok());
    for raw in ["null", "{}", "[]", "0"] {
        let value = tsr_json::RawValue(raw.as_bytes().to_vec());
        assert_eq!(
            unmarshal_params::<NoParams>(Some(&value)).unwrap_err().code,
            -32602
        );
    }
    for raw in [None, Some("null"), Some("0"), Some("true")] {
        let value = raw.map(|s| tsr_json::RawValue(s.as_bytes().to_vec()));
        assert_eq!(
            unmarshal_params::<InitializedParams>(value.as_ref())
                .unwrap_err()
                .code,
            -32602
        );
    }
    assert!(
        unmarshal_params::<InitializedParams>(Some(&tsr_json::RawValue(b"{}".to_vec()))).is_ok()
    );
    assert_eq!(methods::INITIALIZE.method, "initialize");
    assert_eq!(
        methods::TEXT_DOCUMENT_DID_OPEN.method,
        "textDocument/didOpen"
    );
}

#[test]
fn union_cardinality_is_an_invariant() {
    let invalid = IntegerOrString {
        integer: Some(Box::new(1)),
        string: Some(Box::new("x".into())),
    };
    assert!(std::panic::catch_unwind(|| tsr_json::marshal(&invalid, Options::default())).is_err());
    assert!(std::panic::catch_unwind(|| tsr_json::marshal(
        &IntegerOrString::default(),
        Options::default()
    ))
    .is_err());
    assert_eq!(
        tsr_json::marshal(&IntegerOrNull::default(), Options::default()).unwrap(),
        b"null"
    );
}

#[test]
fn enum_names_and_unknown_numeric_values_follow_the_pin() {
    assert_eq!(InlayHintKind::TYPE.to_string(), "Type");
    assert_eq!(InlayHintKind::PARAMETER.to_string(), "Parameter");
    assert_eq!(InlayHintKind(999).to_string(), "InlayHintKind(999)");
    assert_eq!(SymbolKind::FUNCTION.to_string(), "Function");
    assert_eq!(WatchKind(0).to_string(), "0");
    assert_eq!(WatchKind(3).to_string(), "Create|Change");
    assert_eq!(WatchKind(8).to_string(), "WatchKind(8)");
    assert_eq!(WatchKind(9).to_string(), "Create");
    assert_eq!(ErrorCode::INVALID_PARAMS.to_string(), "InvalidParams");
}

#[test]
fn lsp_parse_error_response_preserves_a_null_id() {
    let error = tsr_lsproto::ResponseError {
        code: -32600,
        message: "InvalidRequest".into(),
        data: None,
    };
    let response = tsr_lsproto::ResponseMessage {
        id: None,
        result: None,
        error: Some(&error),
    };
    assert_eq!(
        tsr_json::marshal(&response, tsr_json::Options::default()).unwrap(),
        br#"{"jsonrpc":"2.0","id":null,"error":{"code":-32600,"message":"InvalidRequest"}}"#
    );
}
