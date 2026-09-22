//! `evaluator` group (F4a plan task 7). The Go package is a callback-driven
//! evaluator shared by the checker, the printer and the language service. The
//! Rust logic exists but is fused into the checker: the entity callback is
//! hard-wired to the checker's enum-member resolution
//! (crates/tsr_checker/src/enum_eval.rs), the skip set to parentheses, and the
//! value domain to numbers and strings. There is no entry point a controlled
//! callback can drive, and manufacturing one here would duplicate checker name
//! resolution, which the plan forbids. So every request names the missing
//! reusable operation instead of emulating it.

use serde_json::Value;

use crate::api::{subject, Outcome};

const SUBJECT: &str = "evaluator";
const HOME: &str = "a reusable evaluator module shared by tsr_checker and tsr_printer; the logic \
                    lives today in crates/tsr_checker/src/enum_eval.rs (CheckerState::evaluate_enum_expression)";

pub fn observe(request: &Value) -> Option<Outcome> {
    if subject(request) != SUBJECT {
        return None;
    }
    let operation = request
        .get("operation")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let (authority, signature) = match operation {
        "tsc/internal/evaluator/evaluator.go:NewEvaluator" => (
            "upstream/tsc/internal/evaluator/evaluator.go:24-122; the only constructor call is \
             checker.go:939, NewEvaluator(c.evaluateEntity, ast.OEKParentheses)",
            "Evaluator::new(evaluate_entity: &mut dyn FnMut(NodeId, NodeId) -> EvaluationResult, \
             outer_expressions_to_skip: OuterExpressionKinds) -> Evaluator, with \
             Evaluator::evaluate(&mut self, expr: NodeId, location: NodeId) -> EvaluationResult",
        ),
        "tsc/internal/evaluator/evaluator.go:evaluateTemplateExpression" => (
            "upstream/tsc/internal/evaluator/evaluator.go:124-140",
            "the template arm of Evaluator::evaluate, sharing the entity callback",
        ),
        "tsc/internal/evaluator/evaluator.go:AnyToString" => (
            "upstream/tsc/internal/evaluator/evaluator.go:142-154",
            "fn any_to_string(value: &EvaluatedValue) -> Result<JsString, Unhandled> over Number, \
             string, bool and PseudoBigInt (today tsr_checker/src/template.rs literal_value_text, private)",
        ),
        "tsc/internal/evaluator/evaluator.go:IsTruthy" => (
            "upstream/tsc/internal/evaluator/evaluator.go:156-168",
            "fn is_truthy(value: &EvaluatedValue) -> Result<bool, Unhandled> (today written inline at \
             tsr_checker/src/truthiness.rs:98-104)",
        ),
        "tsc/internal/evaluator/evaluator.go:NewResult" => (
            "upstream/tsc/internal/evaluator/evaluator.go:18-20",
            "EvaluationResult::new(value, is_syntactically_string, resolved_other_files, \
             has_external_references)",
        ),
        other => {
            return Some(Outcome::Failed(format!(
                "no reviewed evaluator operation {other:?}"
            )))
        }
    };
    Some(Outcome::missing(operation, authority, signature, HOME))
}
