use super::*;
use crate::{Error, ResponseQueue, Snapshot};
use serde_json::Value;
use ts_arena::Counts;
use ts_checker::CheckerOptions;
use ts_project::{CheckerPool, Project};

fn observations() -> Value {
    serde_json::from_str(include_str!(
        "../../../../data/s09/printing-observations.json"
    ))
    .unwrap()
}

fn case(name: &str) -> Value {
    observations()["rows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["name"] == name)
        .unwrap_or_else(|| panic!("missing native print case {name}"))
        .clone()
}

fn bytes(value: &Value) -> Vec<u8> {
    let hex = value.as_str().unwrap();
    assert_eq!(hex.len() % 2, 0);
    hex.as_bytes()
        .chunks_exact(2)
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
        .collect()
}

fn options(row: &Value) -> PrintNodeOptions {
    let options = &row["options"];
    PrintNodeOptions {
        preserve_source_newlines: options["preserve_source_newlines"].as_bool().unwrap(),
        never_ascii_escape: options["never_ascii_escape"].as_bool().unwrap(),
        terminate_unterminated_literals: options["terminate_unterminated_literals"]
            .as_bool()
            .unwrap(),
    }
}

fn snapshot(counters: &Counters) -> Snapshot {
    let pool = CheckerPool::for_types(CheckerOptions::default(), counters, 1);
    Snapshot::new(ts_project::Snapshot::new(Project::new(pool))).unwrap()
}

fn assert_allocated(counters: &Counters, before: Counts) {
    let live = counters.snapshot();
    assert!(live.owners > before.owners, "no live decoded owner");
    assert!(
        live.allocations > before.allocations,
        "no live decoded storage"
    );
}

#[test]
fn native_decode_print_outputs_and_named_boundaries_match() {
    let observations = observations();
    let rows = observations["rows"].as_array().unwrap();
    let requests: Value =
        serde_json::from_str(include_str!("../../../../data/s09/printing-cases.json")).unwrap();
    let cases = requests["cases"].as_array().unwrap();
    assert_eq!(rows.len(), 12);
    assert_eq!(rows.len(), cases.len());
    assert_eq!(observations["pin"], requests["pin"]);
    let counters = Counters::new();
    for (row, request) in rows.iter().zip(cases) {
        assert_eq!(row["name"], request["name"]);
        assert_eq!(row["rust_unsupported"], request["rust_unsupported"]);
        for key in [
            "preserve_source_newlines",
            "never_ascii_escape",
            "terminate_unterminated_literals",
        ] {
            assert_eq!(
                row["options"][key].as_bool().unwrap(),
                request["options"][key].as_bool().unwrap_or(false)
            );
        }
        if let Some(wire) = request["wire_hex"].as_str() {
            assert_eq!(row["encoded_hex"], wire);
        }
        let wire = bytes(&row["encoded_hex"]);
        let result = std::panic::catch_unwind(|| print_node(&wire, options(row), &counters));
        if let Some(reason) = row["rust_unsupported"].as_str() {
            assert!(row["text_hex"].is_string(), "boundary needs native output");
            assert!(
                matches!(result, Ok(Err(PrintError::Print(ts_printer::Error::Unsupported(actual)))) if actual == reason),
                "{}: expected Unsupported({reason})",
                row["name"]
            );
        } else if let Some(expected) = row["panic"].as_str() {
            let payload = result.expect_err("native decode/print panics");
            let message = payload
                .downcast_ref::<&str>()
                .copied()
                .or_else(|| payload.downcast_ref::<String>().map(String::as_str));
            match row["panic_class"].as_str().unwrap() {
                "nil-root" => {
                    assert_eq!(
                        expected,
                        "runtime error: invalid memory address or nil pointer dereference"
                    );
                    assert_eq!(message, Some("nil root passed to API PrintNode"));
                }
                "synthetic-expression" => {
                    assert_eq!(expected, "SyntheticExpression should never be decoded");
                    assert_eq!(message, Some(expected));
                }
                class => panic!("unclassified native panic {class}"),
            }
        } else if let Some(expected) = row["decode_error"].as_str() {
            assert_eq!(
                result.unwrap(),
                Err(PrintError::Decode(DecodeError::Baseline(expected.into()))),
                "{}",
                row["name"]
            );
        } else {
            assert_eq!(
                result.unwrap().unwrap(),
                bytes(&row["text_hex"]),
                "{}",
                row["name"]
            );
        }
        assert_eq!(counters.snapshot(), Counts::default(), "{}", row["name"]);
    }
}

#[test]
fn decoded_scratch_is_live_only_until_emission_returns() {
    let row = case("type-literal");
    let counters = Counters::new();
    let before = counters.snapshot();
    let scratch = decode_nodes(&bytes(&row["encoded_hex"]), &counters).unwrap();
    assert_allocated(&counters, before);
    let text = print_decoded(scratch, options(&row)).unwrap();
    assert_eq!(counters.snapshot(), before);
    assert_eq!(text, bytes(&row["text_hex"]));
}

#[test]
fn printing_does_not_grow_snapshot_roots_and_text_survives_scratch() {
    let row = case("type-literal");
    let wire = bytes(&row["encoded_hex"]);
    let expected = bytes(&row["text_hex"]);
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
    let mut outputs = Vec::new();
    for _ in 0..16 {
        let text = print_node(&wire, options(&row), &counters).unwrap();
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
fn decoder_panic_drops_scratch_and_retires_snapshot_request() {
    let row = case("synthetic-expression-panic");
    let counters = Counters::new();
    let snapshot = snapshot(&counters);
    let before = counters.snapshot();
    let result = snapshot.request(|| {
        Ok(print_node(
            &bytes(&row["encoded_hex"]),
            options(&row),
            &counters,
        ))
    });
    assert_eq!(
        result,
        Err(Error::Panicked(row["panic"].as_str().unwrap().into()))
    );
    assert_eq!(counters.snapshot(), before);
    assert_eq!(
        snapshot.generation().validate(),
        Err(ts_arena::Error::Retired)
    );
    let registry = snapshot.0.registry.lock().unwrap();
    assert!(registry.types.is_empty());
    assert!(registry.latest.is_none());
    drop(registry);
    drop(snapshot);
    assert_eq!(counters.snapshot(), Counts::default());
}

#[test]
fn printer_error_drops_allocated_scratch_without_retiring_snapshot() {
    let row = case("source-newlines-boundary");
    let counters = Counters::new();
    let snapshot = snapshot(&counters);
    let before = counters.snapshot();
    // Exercise the consuming emission path with a demonstrably live arena.
    let scratch = decode_nodes(&bytes(&row["encoded_hex"]), &counters).unwrap();
    assert_allocated(&counters, before);
    let result = snapshot.request(|| Ok(print_decoded(scratch, options(&row))));
    assert_eq!(
        result,
        Ok(Err(PrintError::Print(ts_printer::Error::Unsupported(
            "PreserveSourceNewlines"
        ))))
    );
    assert_eq!(counters.snapshot(), before);
    // Exercise the public wrapper's same failure with its own fresh decode.
    let result = snapshot.request(|| {
        Ok(print_node(
            &bytes(&row["encoded_hex"]),
            options(&row),
            &counters,
        ))
    });
    assert_eq!(
        result,
        Ok(Err(PrintError::Print(ts_printer::Error::Unsupported(
            "PreserveSourceNewlines"
        ))))
    );
    assert_eq!(counters.snapshot(), before);
    assert_eq!(snapshot.generation().validate(), Ok(()));
    assert!(snapshot.latest().unwrap().is_none());
    assert!(snapshot.0.registry.lock().unwrap().types.is_empty());
    drop(snapshot);
    assert_eq!(counters.snapshot(), Counts::default());
}
