//! B03/B08: actual pinned node-builder output for applied outer arguments and
//! distributive conditionals whose instantiated check is no longer a parameter.
#[path = "../../../tools/s08/p5/display.rs"]
mod display;

use serde_json::Value;
use sha2::{Digest, Sha256};

const REQUESTS: &str = include_str!("fixtures/c2/node_builder_refs/requests.json");
const OBSERVATIONS: &str = include_str!("fixtures/c2/node_builder_refs/observations.json");
const PROVENANCE: &str = include_str!("fixtures/c2/node_builder_refs/provenance.json");

#[test]
fn outer_references_and_conditional_wrappers_match_native() {
    let requests: Value = serde_json::from_str(REQUESTS).unwrap();
    let expected: Value = serde_json::from_str(OBSERVATIONS).unwrap();
    let provenance: Value = serde_json::from_str(PROVENANCE).unwrap();
    let pin: Value = serde_json::from_str(include_str!("../../../data/upstream.json")).unwrap();
    let request_hash = format!("{:x}", Sha256::digest(REQUESTS.as_bytes()));
    assert_eq!(provenance["pin"], pin["pin"]);
    assert_eq!(provenance["request_sha256"], request_hash);
    assert_eq!(expected["request_sha256"], request_hash);
    assert_eq!(
        provenance["output_sha256"],
        format!("{:x}", Sha256::digest(OBSERVATIONS.as_bytes()))
    );
    assert_eq!(
        provenance["source_sha256"],
        format!(
            "{:x}",
            Sha256::digest(include_bytes!(
                "../../../tools/s08/p5/display_oracle_test.go"
            ))
        )
    );
    let actual = display::observe(&requests).unwrap();
    assert_eq!(actual["programs"], expected["programs"]);
}
