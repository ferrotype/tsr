//! Exact native diagnostics for the remaining error-parity witnesses. The
//! corpus screen separately checks type/symbol/display parity and controls.
#[path = "../../../tools/s08/p4/executor.rs"]
mod executor;

#[test]
fn remaining_error_tail_matches_native_diagnostics() {
    let requests: Vec<serde_json::Value> = serde_json::from_str(include_str!(
        "../../../data/s08/p6/error-parity-remaining/requests.json"
    ))
    .unwrap();
    let expected: Vec<serde_json::Value> = serde_json::from_str(include_str!(
        "../../../data/s08/p6/error-parity-remaining/observations.json"
    ))
    .unwrap();
    assert_eq!(requests.len(), expected.len());
    let mut mismatches = Vec::new();
    for (request, expected) in requests.iter().zip(expected) {
        assert_eq!(request["id"], expected["id"]);
        let actual = executor::observe(request, |program, _, _, diagnostics| {
            let sorted = program
                .sort_and_deduplicate_diagnostics(diagnostics.unwrap())
                .unwrap();
            executor::BaselineResults {
                type_symbols: serde_json::json!({"state":"not_requested"}),
                errors: executor::diagnostics::phase(program, &sorted),
            }
        });
        assert_eq!(
            actual["load"]["state"], "executed",
            "{}: load failed",
            request["id"]
        );
        for (phase, result) in actual["phases"].as_object().unwrap() {
            assert!(
                matches!(result["state"].as_str(), Some("executed" | "not_requested")),
                "{}: {phase} failed: {result}",
                request["id"]
            );
        }
        if actual["error_baseline"]["diagnostics"] != expected["diagnostics"] {
            mismatches.push(request["id"].clone());
        }
    }
    assert!(
        mismatches.is_empty(),
        "native diagnostic differences: {mismatches:#?}"
    );
}
