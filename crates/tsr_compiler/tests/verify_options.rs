use serde_json::Value;
use ts_compiler as ts_compiler_error;
use ts_compiler::{FileCache, Program, ProgramOptions};
#[allow(dead_code)] // Shared probe also exposes the full loader observer, exercised by program parity.
#[path = "../../../tools/s07/program/rust_observation.rs"]
mod observation;
#[test]
fn option_diagnostics_locations_paths_and_output_conflicts_match_go() {
    let requests: Vec<Value> = serde_json::from_str(include_str!(
        "../../../data/s07/verify-options-requests.json"
    ))
    .unwrap();
    let expected: Vec<Value> = serde_json::from_str(include_str!(
        "../../../data/s07/verify-options-observations.json"
    ))
    .unwrap();
    assert_eq!(requests.len(), 103);
    assert_eq!(requests.len(), expected.len());
    let mut cache = FileCache::new();
    let counters = ts_arena::Counters::new();
    for (request, expected) in requests.iter().zip(expected) {
        let program = observation::try_load(request, &mut cache, &counters, None).unwrap();
        let actual = observation::verify_options(request["id"].as_str().unwrap(), &program);
        assert_eq!(actual, expected, "request {request}");
    }
}
