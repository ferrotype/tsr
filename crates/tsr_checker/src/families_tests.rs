//! Replays the frozen storage-families trace against the pinned Go printer's
//! observations (`data/s08/families-observations.json`): every root, every
//! named type of `NewChecker`'s prefix and the creation counters must agree.

use crate::storage_families::Prepared;
use serde_json::Value;

fn load(text: &str) -> Value {
    serde_json::from_str(text).expect("valid fixture JSON")
}

#[test]
fn every_frozen_trace_reproduces_the_go_observations() {
    let requests = load(include_str!("../../../data/s08/storage-families.json"));
    let expected = load(include_str!("../../../data/s08/families-observations.json"));
    let traces = requests["traces"].as_array().expect("traces");
    let observed = expected["traces"].as_array().expect("observed traces");
    assert_eq!(
        traces.len(),
        observed.len(),
        "one observation per frozen trace"
    );
    assert!(!traces.is_empty());
    for (request, go) in traces.iter().zip(observed) {
        assert_eq!(request["options"], go["options"]);
        let prepared = Prepared::new(request).expect("frozen trace decodes");
        let live = prepared.execute().expect("frozen trace executes");
        let rust = live.observation().expect("observation");
        assert_eq!(
            rust["named"], go["named"],
            "NewChecker prefix types ({})",
            request["options"]
        );
        assert_eq!(
            rust["counts"], go["counts"],
            "creation counters ({})",
            request["options"]
        );
        assert_eq!(
            rust["prefix_counts"], go["prefix_counts"],
            "NewChecker prefix counters"
        );
        let rust_roots = rust["roots"].as_array().expect("roots");
        let go_roots = go["roots"].as_array().expect("go roots");
        assert_eq!(rust_roots.len(), go_roots.len());
        for (index, (ours, theirs)) in rust_roots.iter().zip(go_roots).enumerate() {
            assert_eq!(
                ours, theirs,
                "root {index} of trace {} ({})",
                request["options"], request["actions"][index]
            );
        }
        // A real NewChecker over an empty program goes on to create the
        // `globalThis` object type and `autoArrayType` in initializeChecker (P2).
        let real = go["real_counts"]["types"]
            .as_u64()
            .expect("real NewChecker type count");
        let prefix = rust["prefix_counts"]["types"]
            .as_u64()
            .expect("prefix type count");
        assert_eq!(
            real,
            prefix + 2,
            "the real constructor adds exactly initializeChecker's two types"
        );
    }
}
