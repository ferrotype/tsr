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
        assert_eq!(
            actual["type_symbol_baselines"][domain], native[domain],
            "{domain}"
        );
    }
}

macro_rules! case {
    ($test:ident, $name:literal) => {
        #[test]
        fn $test() {
            assert_native(
                include_str!(concat!(
                    "fixtures/c2/semantic_gaps/",
                    $name,
                    ".request.json"
                )),
                include_str!(concat!("fixtures/c2/semantic_gaps/", $name, ".native.json")),
            );
        }
    };
}

case!(contextual_cache_respects_flags, "contextualTypeCaching");
case!(
    bigint_property_declarations_report_native_errors,
    "bigintPropertyName"
);
case!(
    branded_index_signature_keys_survive_keyof,
    "indexSignatures1"
);
case!(
    array_elements_use_the_enclosing_inference_context,
    "arrayLiteralInference"
);
case!(
    mapped_display_keeps_the_original_parameter_constraint,
    "declarationEmitMappedTypeDistributivityPreservesConstraints"
);
case!(
    mapped_intersection_keys_preserve_index_signature,
    "specialIntersectionsInMappedTypes"
);
case!(
    import_type_display_rewrites_module_specifier,
    "spuriousCircularityOnTypeImport"
);
case!(
    recursive_index_simplification_preserves_error,
    "recursiveIndexedAccessSimplification"
);
