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
                    "fixtures/c2/inference_gaps/",
                    $name,
                    ".request.json"
                )),
                include_str!(concat!(
                    "fixtures/c2/inference_gaps/",
                    $name,
                    ".native.json"
                )),
            );
        }
    };
}

case!(
    union_of_objects_mapped_context,
    "inferenceUnionOfObjectsMappedContextualType"
);
case!(
    reverse_mapped_widening,
    "reverseMappedTypeInferenceWidening2"
);
case!(
    reverse_mapped_primitive_constraint,
    "reverseMappedTypePrimitiveConstraintProperty"
);
case!(
    binding_pattern_inference_source,
    "bindingPatternCannotBeOnlyInferenceSource"
);

case!(super_type_arguments, "superWithTypeArgument");
case!(
    super_type_arguments_with_parameter,
    "superWithTypeArgument2"
);
case!(super_type_arguments_and_method, "superWithTypeArgument3");
case!(
    extends_instantiation_expression,
    "classExtendsInstantiationExpressionType"
);
case!(contextual_rest_tuples, "restTuplesFromContextualTypes");

case!(
    const_type_parameter_modifiers,
    "typeParameterConstModifiers"
);
case!(
    contravariant_annotated_function,
    "contravariantOnlyInferenceFromAnnotatedFunction"
);
case!(
    contravariant_annotated_function_javascript,
    "contravariantOnlyInferenceFromAnnotatedFunctionJs"
);
case!(
    conditional_contextual_simplification,
    "conditionalTypeContextualTypeSimplificationsSuceeds"
);
