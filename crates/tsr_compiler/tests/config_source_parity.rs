#[path = "../../../tools/s07/config/rust_observation.rs"]
mod observation;
#[test]
fn source_config_values_ranges_inheritance_and_globs() {
    let requests: Vec<serde_json::Value> =
        serde_json::from_str(include_str!("../../../tools/s07/config/requests.json")).unwrap();
    let expected: Vec<serde_json::Value> =
        serde_json::from_str(include_str!("../../../data/s07/config-observations.json")).unwrap();
    let actual = observation::observe_all(&requests).unwrap();
    assert_eq!(actual.len(), expected.len());
    assert_eq!(requests.len(), expected.len());
    for ((request, actual), expected) in requests.iter().zip(actual).zip(expected) {
        assert_eq!(actual, expected, "request {}", request["id"]);
    }
}
