//! Pinned source observations at the checker boundary, before declaration emit.
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
    assert_eq!(actual["error_baseline"]["emit"], "not_executed");
    assert_eq!(
        actual["error_baseline"]["diagnostics"], native["diagnostics_before_emit"],
        "checker diagnostics before emit"
    );
    // Native emit can revisit a circular constraint at a new location. Keep
    // both observations, and compare rendered bytes when that schedule agrees.
    if native["diagnostics_before_emit"] == native["diagnostics_after_emit"] {
        assert_eq!(
            actual["error_baseline"]["baseline"], native["errors"],
            "diagnostics"
        );
    }
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
                    "fixtures/c2/constraint_gaps/",
                    $name,
                    ".request.json"
                )),
                include_str!(concat!(
                    "fixtures/c2/constraint_gaps/",
                    $name,
                    ".native.json"
                )),
            );
        }
    };
}

case!(contextual_outer_parameters, "contextualOuterTypeParameters");
case!(
    nullable_argument_inference,
    "inferenceDoesNotAddUndefinedOrNull"
);
case!(generic_null_flow, "unknownControlFlow");
case!(
    circular_mapped_conditional,
    "circularlyConstrainedMappedTypeContainingConditionalNoInfiniteInstantiationDepth"
);
case!(
    deferred_redux_inference,
    "reactReduxLikeDeferredInferenceAllowsAssignment"
);
case!(
    recursive_mapped_constraint_before_emit,
    "incorrectRecursiveMappedTypeConstraint"
);
case!(
    invalid_parameter_constraint_before_emit,
    "typeParameterWithInvalidConstraintType"
);
case!(recursive_mapped_types_before_emit, "recursiveMappedTypes");
