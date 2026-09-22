//! Drives the shared production evaluator with the pinned probe's recording
//! entity callback. This adapter decodes values and records calls; name
//! resolution and expression evaluation belong to the production consumers.

use std::panic::{catch_unwind, AssertUnwindSafe};

use serde_json::{json, Value};
use tsr_arena::NodeId;
use tsr_ast::evaluator::{
    any_to_string, is_truthy, outer_expression_kinds as outer, Error as EvalError, EvaluatedValue,
    EvaluationContext, EvaluationResult, Evaluator, Unhandled,
};
use tsr_ast::{AstView, SourceFileParseOptions, SyntaxKind};
use tsr_jsstring::{JsString, SourceText};

use crate::api::{subject, Outcome};

pub fn observe(request: &Value) -> Option<Outcome> {
    (subject(request) == "evaluator").then(|| match run(request) {
        Ok(rows) => Outcome::Observed(crate::api::ordered(rows)),
        Err(error) => Outcome::Failed(error),
    })
}

fn decode(value: &Value) -> Result<Option<EvaluatedValue>, String> {
    if let Some(number) = value.get("number") {
        return Ok(Some(EvaluatedValue::Number(tsr_jsnum::from_string(
            number.as_str().ok_or("number must be text")?.as_bytes(),
        ))));
    }
    if let Some(text) = value.get("string") {
        return Ok(Some(EvaluatedValue::String(JsString::from_bytes(
            text.as_str().ok_or("string must be text")?.as_bytes(),
        ))));
    }
    if let Some(text) = value.get("string_hex") {
        let text = text.as_str().ok_or("string_hex must be text")?;
        if !text.len().is_multiple_of(2) || !text.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err("string_hex must contain complete hex byte pairs".to_owned());
        }
        let bytes = text
            .as_bytes()
            .chunks_exact(2)
            .map(|pair| {
                let digit = |byte: u8| {
                    if byte.is_ascii_digit() {
                        byte - b'0'
                    } else {
                        byte.to_ascii_lowercase() - b'a' + 10
                    }
                };
                digit(pair[0]) * 16 + digit(pair[1])
            })
            .collect::<Vec<_>>();
        return Ok(Some(EvaluatedValue::String(JsString::from_bytes(bytes))));
    }
    if let Some(value) = value.get("bool") {
        return Ok(Some(EvaluatedValue::Bool(
            value.as_bool().ok_or("bool must be boolean")?,
        )));
    }
    if let Some(text) = value.get("bigint") {
        let text = text.as_str().ok_or("bigint must be text")?;
        let (digits, negative) = text
            .strip_prefix('-')
            .map_or((text, false), |digits| (digits, true));
        return Ok(Some(EvaluatedValue::BigInt(tsr_jsnum::PseudoBigInt::new(
            digits.as_bytes(),
            negative,
        ))));
    }
    if value.get("unsupported").and_then(Value::as_bool) == Some(true) {
        return Ok(Some(EvaluatedValue::Unsupported));
    }
    Ok(None)
}

fn encode(value: Option<&EvaluatedValue>) -> Value {
    match value {
        None => Value::Null,
        Some(EvaluatedValue::Number(value)) => json!([
            "number",
            value.to_string(),
            value.value().is_sign_negative() && !value.is_nan(),
            value.is_nan()
        ]),
        Some(EvaluatedValue::String(value)) => string_hex(value),
        Some(EvaluatedValue::Bool(value)) => json!(["bool", value]),
        Some(EvaluatedValue::BigInt(value)) => json!([
            "bigint",
            value.negative,
            String::from_utf8(value.base10_value.clone()).expect("probe bigint digits are UTF-8")
        ]),
        // The probe's foreign Go type is only a protocol discriminator; the
        // production evaluator carries no harness-specific type identity.
        Some(EvaluatedValue::Unsupported) => json!(["unsupported", "evaluator.phase1Unsupported"]),
    }
}

fn string_hex(value: &JsString) -> Value {
    json!(["string_hex", crate::hex(value.as_bytes())])
}

fn encode_result(result: &EvaluationResult) -> Value {
    json!([
        encode(result.value.as_ref()),
        result.is_syntactically_string,
        result.resolved_other_files,
        result.has_external_references
    ])
}

fn unhandled(error: Unhandled) -> Value {
    json!(["panic", error.to_string()])
}

fn guarded(run: impl FnOnce() -> Result<Value, String>) -> Result<Value, String> {
    match catch_unwind(AssertUnwindSafe(run)) {
        Ok(result) => result,
        Err(payload) => Ok(json!([
            "rust_panic",
            payload
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| payload
                    .downcast_ref::<&str>()
                    .map(|text| (*text).to_owned()))
                .unwrap_or_default()
        ])),
    }
}

fn text(view: AstView<'_>, id: NodeId) -> Result<String, String> {
    let read = view.node(id).map_err(|error| format!("{error:?}"))?;
    let source = view.source().as_bytes();
    let start = usize::try_from(tsr_scanner::skip_trivia(source, i64::from(read.pos())))
        .map_err(|error| error.to_string())?;
    let end = usize::try_from(read.end()).map_err(|error| error.to_string())?;
    let bytes = source
        .get(start..end)
        .ok_or("invalid expression source span")?;
    Ok(std::str::from_utf8(bytes)
        .map_err(|error| error.to_string())?
        .trim()
        .to_owned())
}

struct RecordingContext<'a> {
    view: AstView<'a>,
    entities: &'a Value,
    calls: Vec<Value>,
}
impl EvaluationContext for RecordingContext<'_> {
    type Error = String;
    fn ast(&self, _: NodeId) -> Result<AstView<'_>, String> {
        Ok(self.view)
    }
    fn evaluate_entity(
        &mut self,
        expression: NodeId,
        location: Option<NodeId>,
    ) -> Result<EvaluationResult, String> {
        let name = text(self.view, expression)?;
        let expr = self
            .view
            .node(expression)
            .map_err(|error| format!("{error:?}"))?;
        let location = self
            .view
            .node(location.ok_or("probe callback has no location")?)
            .map_err(|error| format!("{error:?}"))?;
        self.calls.push(json!([
            expr.kind().raw(),
            expr.pos(),
            expr.end(),
            name,
            location.kind().raw(),
            location.pos()
        ]));
        let Some(entity) = self.entities.get(&name) else {
            return Ok(EvaluationResult::default());
        };
        Ok(EvaluationResult::new(
            decode(&entity["value"])?,
            entity["is_syntactically_string"].as_bool().unwrap_or(false),
            entity["resolved_other_files"].as_bool().unwrap_or(false),
            entity["has_external_references"].as_bool().unwrap_or(false),
        ))
    }
}

fn evaluate(request: &Value) -> Result<Vec<Value>, String> {
    let source = request["source"]
        .as_str()
        .ok_or("evaluation request has no source")?;
    let mut skip = 0;
    for name in request["skip"]
        .as_array()
        .ok_or("evaluation request has no skip list")?
    {
        skip |= match name.as_str() {
            Some("parentheses") => outer::PARENTHESES,
            Some("type_assertions") => outer::TYPE_ASSERTIONS,
            Some("non_null_assertions") => outer::NON_NULL_ASSERTIONS,
            Some("satisfies") => outer::SATISFIES,
            Some("expressions_with_type") => outer::EXPRESSIONS_WITH_TYPE_ARGUMENTS,
            Some("all") => outer::ALL,
            _ => return Err(format!("unknown outer expression kind {name}")),
        };
    }
    let file = tsr_parser::parse_source_file(
        SourceText::from_loaded_bytes(source.as_bytes().to_vec()),
        tsr_core::ScriptKind::TS,
        SourceFileParseOptions {
            file_name: JsString::from_bytes(&b"/evaluate.ts"[..]),
            path: JsString::from_bytes(&b"/evaluate.ts"[..]),
            ..Default::default()
        },
    )
    .publish_unbound();
    let root = file.root().ok_or("parsed file has no root")?;
    let view = file.view();
    let source_file = view
        .source_file(root)
        .map_err(|error| format!("{error:?}"))?;
    if !source_file.diagnostics().is_empty() {
        return Err("request source does not parse cleanly".to_owned());
    }
    let statements = view
        .node(root)
        .map_err(|error| format!("{error:?}"))?
        .statement_list()
        .ok_or("source file has no statements")?;
    let statements = view
        .node_slice(
            view.list(statements)
                .map_err(|error| format!("{error:?}"))?
                .nodes(),
        )
        .map_err(|error| format!("{error:?}"))?;
    let mut context = RecordingContext {
        view,
        entities: &request["entities"],
        calls: Vec::new(),
    };
    let mut rows = Vec::new();
    for statement in statements.iter() {
        let statement = statement.ok_or("source file has absent statement")?;
        let statement = view.node(statement).map_err(|error| format!("{error:?}"))?;
        if statement.kind() != SyntaxKind::ExpressionStatement {
            return Err("every statement must be an expression statement".to_owned());
        }
        let expression = statement
            .expression()
            .ok_or("expression statement has no expression")?;
        context.calls.clear();
        let result = guarded(|| {
            match Evaluator::new(&mut context, skip).evaluate(expression, Some(expression)) {
                Ok(result) => Ok(encode_result(&result)),
                Err(EvalError::Unhandled(error)) => Ok(unhandled(error)),
                Err(error) => Err(format!("evaluation failed: {error:?}")),
            }
        })?;
        rows.push(json!([
            text(view, expression)?,
            result,
            std::mem::take(&mut context.calls)
        ]));
    }
    Ok(rows)
}

fn run(request: &Value) -> Result<Vec<Value>, String> {
    let operation = request["operation"]
        .as_str()
        .ok_or("evaluator request has no operation")?;
    let mode = request["mode"]
        .as_str()
        .ok_or("evaluator request has no mode")?;
    let valid = match operation {
        "tsc/internal/evaluator/evaluator.go:NewEvaluator"
        | "tsc/internal/evaluator/evaluator.go:evaluateTemplateExpression" => mode == "evaluate",
        "tsc/internal/evaluator/evaluator.go:AnyToString" => mode == "any_to_string",
        "tsc/internal/evaluator/evaluator.go:IsTruthy" => mode == "is_truthy",
        "tsc/internal/evaluator/evaluator.go:NewResult" => mode == "new_result",
        _ => false,
    };
    if !valid {
        return Err(format!(
            "no reviewed evaluator operation {operation:?} with mode {mode:?}"
        ));
    }
    if mode == "evaluate" {
        return evaluate(request);
    }
    let values = request["values"]
        .as_array()
        .ok_or("direct request has no values")?;
    let mut rows = Vec::new();
    if mode == "new_result" {
        for (index, flags) in request["flags"]
            .as_array()
            .ok_or("new_result request has no flags")?
            .iter()
            .enumerate()
        {
            let flags = flags
                .as_array()
                .filter(|flags| flags.len() == 3)
                .ok_or("result requires three flags")?;
            let value = values.get(index).map(decode).transpose()?.flatten();
            let result = EvaluationResult::new(
                value,
                flags[0].as_bool().ok_or("invalid result flag")?,
                flags[1].as_bool().ok_or("invalid result flag")?,
                flags[2].as_bool().ok_or("invalid result flag")?,
            );
            rows.push(encode_result(&result));
        }
    } else {
        for value in values {
            let decoded = decode(value)?;
            let result = guarded(|| {
                Ok(if mode == "any_to_string" {
                    any_to_string(decoded.as_ref())
                        .map(|text| string_hex(&text))
                        .unwrap_or_else(unhandled)
                } else {
                    is_truthy(decoded.as_ref())
                        .map(Value::Bool)
                        .unwrap_or_else(unhandled)
                })
            })?;
            rows.push(json!([encode(decoded.as_ref()), result]));
        }
    }
    Ok(rows)
}
