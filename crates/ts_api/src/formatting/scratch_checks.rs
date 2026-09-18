use super::*;
use crate::{Error, ResponseQueue, Snapshot};
use serde_json::Value;
use ts_arena::Counts;
use ts_ast::{AstFile, NodeId, SourceFileParseOptions};
use ts_checker::CheckerOptions;
use ts_project::{CheckerPool, Project};

fn insertion_rows() -> Value {
    serde_json::from_str(include_str!(
        "../../../../data/s09/insertion-observations.json"
    ))
    .unwrap()
}

fn printing_case(name: &str) -> Vec<u8> {
    let observations: Value = serde_json::from_str(include_str!(
        "../../../../data/s09/printing-observations.json"
    ))
    .unwrap();
    let row = observations["rows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["name"] == name)
        .unwrap_or_else(|| panic!("missing native print case {name}"))
        .clone();
    unhex(row["encoded_hex"].as_str().unwrap())
}

fn unhex(hex: &str) -> Vec<u8> {
    assert_eq!(hex.len() % 2, 0);
    hex.as_bytes()
        .chunks_exact(2)
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
        .collect()
}

fn parse(name: &str, source: &[u8]) -> AstFile {
    let path = format!("/{name}.ts");
    ts_parser::parse_source_file(
        ts_jsstring::SourceText::from_loaded_bytes(source.to_vec()),
        ts_core::ScriptKind::TS,
        SourceFileParseOptions {
            file_name: ts_ast::JsString::from_bytes(path.as_bytes()),
            path: ts_ast::JsString::from_bytes(path.as_bytes()),
            ..Default::default()
        },
    )
    .publish_unbound()
}

fn statement(file: &AstFile, index: usize) -> NodeId {
    let view = file.view();
    let list = view
        .node(file.root().unwrap())
        .unwrap()
        .statement_list()
        .unwrap();
    view.node_slice(view.list(list).unwrap().nodes())
        .unwrap()
        .iter()
        .flatten()
        .nth(index)
        .unwrap()
}

/// What an API request carries for one statement of a parsed file.
fn wire(file: &AstFile, index: usize) -> Vec<u8> {
    let mut provider = ts_parser::ParserJsDocProvider::default();
    ts_encoder::encode_node(
        file.view(),
        statement(file, index),
        file.root(),
        &mut provider,
    )
    .unwrap()
    .bytes
}

fn insert(
    file: &AstFile,
    encoded: &[u8],
    position: i64,
    settings: &FormatCodeSettings,
    counters: &Counters,
) -> Result<Vec<u8>, FormatError> {
    let mut provider = ts_parser::ParserJsDocProvider::default();
    let mut target = FormatFile {
        view: file.view(),
        source: file.root().unwrap(),
        jsdoc: &mut provider,
    };
    let scratch = decode_nodes(encoded, counters).map_err(FormatError::Decode)?;
    format_decoded_for_insertion(scratch, &mut target, position, settings)
}

fn snapshot(counters: &Counters) -> Snapshot {
    let pool = CheckerPool::for_types(CheckerOptions::default(), counters, 1);
    Snapshot::new(ts_project::Snapshot::new(Project::new(pool))).unwrap()
}

#[test]
fn native_insertion_outputs_and_failures_match() {
    let observations = insertion_rows();
    let rows = observations["rows"].as_array().unwrap();
    assert_eq!(rows.len(), 4);
    let counters = Counters::new();
    let mut answers = 0;
    for row in rows {
        let name = row["name"].as_str().unwrap();
        let file = parse(name, &unhex(row["source_hex"].as_str().unwrap()));
        for variant in ["default", "tabs"] {
            let settings = ts_format::probe::variant(variant).unwrap();
            let detail = row["insert"][variant].as_str().unwrap();
            for line in detail.lines().filter(|line| line.starts_with("R|")) {
                let fields: Vec<&str> = line.splitn(4, '|').collect();
                let (index, position, expected) = (
                    fields[1].parse::<usize>().unwrap(),
                    fields[2].parse::<i64>().unwrap(),
                    fields[3],
                );
                let answer = insert(&file, &wire(&file, index), position, &settings, &counters);
                match expected.strip_prefix('!') {
                    // Upstream panics; the text of the panic is the error's.
                    Some(message) => assert_eq!(
                        answer.unwrap_err().to_string(),
                        message,
                        "{name} {variant} {line}"
                    ),
                    None => assert_eq!(answer.unwrap(), unhex(expected), "{name} {variant} {line}"),
                }
                // Every request, passing or failing, has released its scratch.
                assert_eq!(counters.snapshot(), Counts::default(), "{name} {line}");
                answers += 1;
            }
        }
    }
    assert_eq!(answers, 64);
}

#[test]
fn insertion_scratch_is_live_only_until_the_text_returns() {
    let file = parse(
        "live",
        b"class C {\n    m() {\n        return 1;\n    }\n}\n",
    );
    let encoded = wire(&file, 0);
    let counters = Counters::new();
    let before = counters.snapshot();
    let scratch = decode_nodes(&encoded, &counters).unwrap();
    let live = counters.snapshot();
    assert!(live.owners > before.owners, "no live decoded owner");
    assert!(
        live.allocations > before.allocations,
        "no live decoded storage"
    );
    let mut provider = ts_parser::ParserJsDocProvider::default();
    let mut target = FormatFile {
        view: file.view(),
        source: file.root().unwrap(),
        jsdoc: &mut provider,
    };
    let settings = FormatCodeSettings::default();
    let text = format_decoded_for_insertion(scratch, &mut target, 0, &settings).unwrap();
    // The decoded tree, the synthetic file published over the printed text and
    // the tokens the formatter created in it are gone; the text is owned.
    assert_eq!(counters.snapshot(), before);
    assert_eq!(text, b"class C {\n    m() {\n        return 1;\n    }\n}");
}

#[test]
fn insertion_does_not_grow_snapshot_roots_and_text_survives_scratch() {
    let file = parse(
        "repeat",
        b"namespace N {\n    export const v = [1, 2,\n        3];\n}\n",
    );
    let encoded = wire(&file, 0);
    let expected = b"    namespace N {\n        export const v = [1, 2, 3];\n    }".to_vec();
    let counters = Counters::new();
    let snapshot = snapshot(&counters);
    let queue = ResponseQueue::new(1);
    let operation = snapshot.checker().operation().unwrap();
    let ty = operation.builtin_type("stringType").unwrap();
    let response = snapshot
        .prepare(&operation, &[ty], b"existing type".to_vec())
        .unwrap();
    drop(operation);
    snapshot.commit(response, &queue).unwrap();
    drop(queue.pop().unwrap());
    let before = counters.snapshot();
    let settings = FormatCodeSettings::default();
    let mut outputs = Vec::new();
    for _ in 0..16 {
        // Inside the namespace, at the start of its second line.
        let text = insert(&file, &encoded, 14, &settings, &counters).unwrap();
        assert_eq!(counters.snapshot(), before);
        let operation = snapshot.checker().operation().unwrap();
        let response = snapshot.prepare(&operation, &[], text).unwrap();
        drop(operation);
        snapshot.commit(response, &queue).unwrap();
        let registry = snapshot.0.registry.lock().unwrap();
        assert_eq!(registry.types.len(), 1);
        assert!(registry.types.contains_key(&ty.id()));
        drop(registry);
        outputs.push(queue.pop().unwrap());
        assert_eq!(counters.snapshot(), before);
    }
    drop(snapshot);
    drop(queue);
    // Responses contain owned bytes, with no synthetic nodes or checker roots.
    assert_eq!(counters.snapshot(), Counts::default());
    for output in outputs {
        assert_eq!(output.bytes(), expected);
        assert_eq!(output.type_ids().count(), 0);
    }
}

#[test]
fn formatter_failure_drops_scratch_without_retiring_snapshot() {
    // Position assignment gives an `if` without `else` an empty synthesized
    // block, which the pinned formatter refuses.
    let file = parse("failure", b"if (x) { y(); }\nfoo();\n");
    let encoded = wire(&file, 0);
    let counters = Counters::new();
    let snapshot = snapshot(&counters);
    let before = counters.snapshot();
    let settings = FormatCodeSettings::default();
    let result = snapshot.request(|| Ok(insert(&file, &encoded, 0, &settings, &counters)));
    match result {
        Ok(Err(FormatError::Format(error))) => {
            assert_eq!(error.to_string(), "Debug failure. False expression.");
        }
        other => panic!("expected the formatter's assertion, got {other:?}"),
    }
    assert_eq!(counters.snapshot(), before);
    assert_eq!(snapshot.generation().validate(), Ok(()));
    assert!(snapshot.latest().unwrap().is_none());
    assert!(snapshot.0.registry.lock().unwrap().types.is_empty());
    drop(snapshot);
    assert_eq!(counters.snapshot(), Counts::default());
}

#[test]
fn printer_and_decoder_errors_drop_scratch_without_retiring_snapshot() {
    let file = parse("target", b"foo();\n");
    let counters = Counters::new();
    let snapshot = snapshot(&counters);
    let before = counters.snapshot();
    let settings = FormatCodeSettings::default();
    // A statement kind the pinned printer has no case for.
    let unhandled = printing_case("unhandled-statement");
    let result = snapshot.request(|| Ok(insert(&file, &unhandled, 0, &settings, &counters)));
    assert_eq!(
        result,
        Ok(Err(FormatError::Print(ts_printer::Error::UnexpectedKind {
            context: "unhandled statement",
            kind: ts_ast::SyntaxKind::JSImportDeclaration.into(),
        })))
    );
    assert_eq!(counters.snapshot(), before);
    // Bytes that are not protocol-8 syntax.
    let malformed = printing_case("malformed-short");
    let result = snapshot.request(|| Ok(insert(&file, &malformed, 0, &settings, &counters)));
    assert!(
        matches!(result, Ok(Err(FormatError::Decode(_)))),
        "{result:?}"
    );
    assert_eq!(counters.snapshot(), before);
    assert_eq!(snapshot.generation().validate(), Ok(()));
    drop(snapshot);
    assert_eq!(counters.snapshot(), Counts::default());
}

#[test]
fn decoder_panic_drops_scratch_and_retires_snapshot_request() {
    let file = parse("target", b"foo();\n");
    let counters = Counters::new();
    let snapshot = snapshot(&counters);
    let before = counters.snapshot();
    let settings = FormatCodeSettings::default();
    let wire = printing_case("synthetic-expression-panic");
    let result = snapshot.request(|| Ok(insert(&file, &wire, 0, &settings, &counters)));
    assert_eq!(
        result,
        Err(Error::Panicked(
            "SyntheticExpression should never be decoded".into()
        ))
    );
    assert_eq!(counters.snapshot(), before);
    drop(snapshot);
    assert_eq!(counters.snapshot(), Counts::default());
}
