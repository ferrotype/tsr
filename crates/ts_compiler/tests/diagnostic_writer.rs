#[path = "../../../tools/s08/p5/error_requests.rs"]
mod error_requests;
#[path = "../../../tools/s08/p5/errors.rs"]
mod errors;
#[path = "../../../tools/s08/p5/paths.rs"]
mod paths;
use serde_json::Value;

#[test]
fn native_plain_pretty_and_error_baseline_bytes_match() {
    let request =
        serde_json::from_str(include_str!("../../../data/s08/p5/errors/requests.json")).unwrap();
    let expected: Value = serde_json::from_str(include_str!(
        "../../../data/s08/p5/errors/observations.json"
    ))
    .unwrap();
    let actual = error_requests::observe(&request).unwrap();
    assert_eq!(
        actual["cases"].as_array().unwrap().len(),
        expected["cases"].as_array().unwrap().len()
    );
    for (e, a) in expected["cases"]
        .as_array()
        .unwrap()
        .iter()
        .zip(actual["cases"].as_array().unwrap())
    {
        assert_eq!(a["state"], "executed", "{}: {a}", e["id"]);
        assert_eq!(a, e, "{}", e["id"]);
    }
}

#[test]
fn deep_message_flattening_uses_an_explicit_stack() {
    std::thread::Builder::new()
        .stack_size(512 * 1024)
        .spawn(|| {
            use std::sync::Arc;
            use ts_ast::Diagnostic;
            use ts_jsstring::JsString;
            fn message() -> Diagnostic {
                Diagnostic::external(
                    None,
                    ts_core::TextRange::new(-1, -1),
                    JsString::default(),
                    1,
                    9999,
                    JsString::from_bytes(b"x".as_slice()),
                )
            }
            let depth = 1000usize;
            let mut node = Arc::new(message());
            for _ in 0..depth {
                let mut parent = message();
                parent.message_chain.push(node);
                node = Arc::new(parent);
            }
            let output = ts_compiler::diagnostic_writer::flattened(&node, b"\n").unwrap();
            assert_eq!(output.len(), depth * (depth + 1) + 2 * depth + 1);
            assert!(output.ends_with(b" x"));
            // Avoid testing Arc's recursively derived drop instead of the writer.
            let mut pending = vec![node];
            while let Some(node) = pending.pop() {
                let mut node = Arc::try_unwrap(node).unwrap();
                pending.append(&mut node.message_chain);
            }
        })
        .unwrap()
        .join()
        .unwrap();
}
