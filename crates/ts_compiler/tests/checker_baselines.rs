#[path = "../../../tools/s08/p5/baseline/mod.rs"]
mod baseline;
#[path = "../../../tools/s08/p5/walker.rs"]
mod walker;
use serde_json::{json, Value};
use std::collections::BTreeMap;

fn local_identities(case: &mut Value) {
    let mut files: BTreeMap<String, BTreeMap<u64, usize>> = BTreeMap::new();
    for query in case["queries"].as_array_mut().unwrap() {
        if let Some(id) = query.get("type_id").and_then(Value::as_u64) {
            let ids = files
                .entry(query["file"].as_str().unwrap().into())
                .or_default();
            let ordinal = ids.len() + 1;
            query["type_id"] = json!(*ids.entry(id).or_insert(ordinal));
        }
    }
}

#[test]
fn native_walker_bytes_and_query_identities_match() {
    let requests =
        serde_json::from_str(include_str!("../../../data/s08/p5/walker/requests.json")).unwrap();
    let mut expected: Value = serde_json::from_str(include_str!(
        "../../../data/s08/p5/walker/observations.json"
    ))
    .unwrap();
    let mut actual = walker::observe(&requests).unwrap();
    for (e, a) in expected["cases"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .zip(actual["cases"].as_array_mut().unwrap())
    {
        assert_eq!(a["state"], "executed", "{}: {a}", e["id"]);
        local_identities(e);
        local_identities(a);
        assert_eq!(a, e, "{}", e["id"]);
    }
    assert_eq!(actual["cases"], expected["cases"]);
}
