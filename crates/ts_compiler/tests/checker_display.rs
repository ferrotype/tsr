#[path = "../../../tools/s08/p5/display.rs"]
mod display;

#[test]
fn context_and_builder_display_match_the_native_requests() {
    let request =
        serde_json::from_str(include_str!("../../../data/s08/p5/display/requests.json")).unwrap();
    let expected: serde_json::Value = serde_json::from_str(include_str!(
        "../../../data/s08/p5/display/observations.json"
    ))
    .unwrap();
    let result = display::observe(&request).unwrap();
    assert_eq!(result["programs"], expected["programs"]);
}
