//! Exact source requests and native observations retained from the pinned corpus.
#[path = "../../../tools/s08/p5/baseline/mod.rs"]
mod baseline;
#[path = "../../../tools/s08/p5/corpus.rs"]
mod corpus;
#[path = "../../../tools/s08/p5/errors.rs"]
mod errors;
#[path = "../../../tools/s08/p4/executor.rs"]
mod executor;
#[path = "../../../tools/s08/p5/paths.rs"]
mod paths;

use serde_json::Value;
use sha2::{Digest, Sha256};

fn assert_native(request: &str, expected: &str) {
    let native: Value = serde_json::from_str(expected).unwrap();
    let pin: Value = serde_json::from_str(include_str!("../../../data/upstream.json")).unwrap();
    assert_eq!(native["pin"], pin["pin"]);
    assert_eq!(
        native["request_sha256"],
        format!("{:x}", Sha256::digest(request.as_bytes()))
    );
    let request: Value = serde_json::from_str(request).unwrap();
    assert_eq!(request["id"], native["id"]);
    let actual = corpus::observe(&request);
    assert_eq!(actual["error_baseline"]["state"], "executed", "{actual}");
    assert_eq!(
        actual["error_baseline"]["baseline"], native["errors"],
        "diagnostics"
    );
    for domain in ["types", "symbols", "public_type_strings"] {
        if let Some(expected_hash) = native[domain]["canonical_json_sha256"].as_str() {
            // Preserve exact multi-megabyte observations without duplicating
            // their hex payloads in checked-in fixtures.
            let mut canonical = actual["type_symbol_baselines"][domain].clone();
            canonical.sort_all_objects();
            let bytes = serde_json::to_vec(&canonical).unwrap();
            assert_eq!(
                bytes.len() as u64,
                native[domain]["canonical_json_bytes"].as_u64().unwrap(),
                "{domain} length"
            );
            assert_eq!(
                format!("{:x}", Sha256::digest(bytes)),
                expected_hash,
                "{domain}"
            );
        } else {
            assert_eq!(
                actual["type_symbol_baselines"][domain], native[domain],
                "{domain}"
            );
        }
    }
}

macro_rules! case {
    ($test:ident, $name:literal) => {
        #[test]
        fn $test() {
            assert_native(
                include_str!(concat!(
                    "fixtures/c2/declaration_imports/",
                    $name,
                    ".request.json"
                )),
                include_str!(concat!(
                    "fixtures/c2/declaration_imports/",
                    $name,
                    ".native.json"
                )),
            );
        }
    };
}

case!(
    js_export_arrow_reuses_annotation,
    "declarationEmitJsExportAssignedArrowNoCrash"
);
case!(
    unique_symbol_import_uses_exported_module_name,
    "declarationEmitUsingTypeAlias2"
);
