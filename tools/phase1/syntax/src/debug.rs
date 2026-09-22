//! Typed argument bridge to the shared production debug helpers.

use std::fmt::{self, Display};
use std::panic::{catch_unwind, AssertUnwindSafe};

use serde_json::{json, Value};
use tsr_core::debug::{self, Argument, Member};

use crate::api::{subject, Outcome};

enum InputArgument<'a> {
    String(&'a str),
    Int(i64),
    KindString(&'a str),
    Stringer(&'a str),
    Both { kind: &'a str, text: &'a str },
    Nil,
}

impl Display for InputArgument<'_> {
    fn fmt(&self, output: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::String(text) | Self::Stringer(text) | Self::Both { text, .. } => {
                output.write_str(text)
            }
            Self::Int(value) => value.fmt(output),
            // The Go fixture is a one-field struct implementing KindString,
            // but no String method. Its ordinary fmt.Sprint view is {kind}.
            Self::KindString(kind) => write!(output, "{{{kind}}}"),
            Self::Nil => output.write_str("<nil>"),
        }
    }
}

impl<'a> InputArgument<'a> {
    fn decode(value: &'a Value) -> Result<Self, String> {
        let object = value
            .as_object()
            .ok_or("debug argument must be an object")?;
        if object.len() != 1 {
            return Err("debug argument must have exactly one typed value".into());
        }
        if let Some(text) = value.get("string").and_then(Value::as_str) {
            Ok(Self::String(text))
        } else if let Some(value) = value.get("int").and_then(Value::as_i64) {
            Ok(Self::Int(value))
        } else if let Some(kind) = value.get("kind_string").and_then(Value::as_str) {
            Ok(Self::KindString(kind))
        } else if let Some(text) = value.get("stringer").and_then(Value::as_str) {
            Ok(Self::Stringer(text))
        } else if let Some(both) = value.get("both").and_then(Value::as_array) {
            match both.as_slice() {
                [kind, text] => Ok(Self::Both {
                    kind: kind.as_str().ok_or("debug KindString must be text")?,
                    text: text.as_str().ok_or("debug Stringer must be text")?,
                }),
                _ => Err("debug both argument must have two strings".into()),
            }
        } else if value.get("nil").and_then(Value::as_bool) == Some(true) {
            Ok(Self::Nil)
        } else {
            Err("invalid typed debug argument".into())
        }
    }

    fn argument(&self) -> Argument<'_> {
        match self {
            Self::String(text) => Argument::String(text),
            Self::Nil => Argument::Nil,
            _ => Argument::Value(self),
        }
    }

    fn kind_string(&self) -> Option<&'a str> {
        match self {
            Self::KindString(kind) | Self::Both { kind, .. } => Some(kind),
            _ => None,
        }
    }
}

fn run(request: &Value) -> Result<Value, String> {
    let call = request
        .get("call")
        .and_then(Value::as_str)
        .ok_or("debug request has no call")?;
    let operation = request
        .get("operation")
        .and_then(Value::as_str)
        .ok_or("debug request has no operation")?;
    let expected_call = match operation {
        "tsc/internal/debug/debug.go:Fail" => "fail",
        "tsc/internal/debug/debug.go:FailBadSyntaxKind" => "fail_bad_syntax_kind",
        "tsc/internal/debug/debug.go:AssertNever" => "assert_never",
        "tsc/internal/debug/debug.go:Assert" => "assert",
        _ => return Err(format!("unknown debug operation {operation:?}")),
    };
    if call != expected_call {
        return Err(format!(
            "debug operation {operation:?} does not support call {call:?}"
        ));
    }
    let message = match request.get("message") {
        None => Vec::new(),
        Some(value) => value
            .as_array()
            .ok_or("debug message must be an array")?
            .iter()
            .map(InputArgument::decode)
            .collect::<Result<Vec<_>, _>>()?,
    };
    let message: Vec<_> = message.iter().map(InputArgument::argument).collect();
    let member = request
        .get("member")
        .map(InputArgument::decode)
        .transpose()?;
    // Validate before catching production panics, so a malformed request can
    // never turn into a successful native-panic observation.
    let reason = request.get("reason").and_then(Value::as_str);
    let condition = request.get("value").and_then(Value::as_bool);
    let kind = member.as_ref().and_then(InputArgument::kind_string);
    match call {
        "fail" if reason.is_none() => return Err("debug fail needs a reason".into()),
        "fail_bad_syntax_kind" if kind.is_none() => {
            return Err("FailBadSyntaxKind needs a KindString member".into())
        }
        "assert_never" if member.is_none() => return Err("AssertNever needs a member".into()),
        "assert" if condition.is_none() => return Err("debug assert needs a condition".into()),
        "fail" | "fail_bad_syntax_kind" | "assert_never" | "assert" => {}
        _ => return Err(format!("unknown debug call {call:?}")),
    }
    let result = catch_unwind(AssertUnwindSafe(|| match call {
        "fail" => debug::fail(reason.expect("validated fail reason")),
        "fail_bad_syntax_kind" => {
            debug::fail_bad_syntax_kind(&kind.expect("validated kind"), &message)
        }
        "assert_never" => {
            let argument = member.as_ref().expect("validated member").argument();
            let member = match &kind {
                Some(kind) => Member::with_kind_string(argument, kind),
                None => Member::new(argument),
            };
            debug::assert_never(member, &message);
        }
        "assert" => debug::assert(condition.expect("validated condition"), &message),
        _ => unreachable!("validated debug call"),
    }));
    let value = match result {
        Ok(()) => json!(["returned"]),
        Err(payload) => {
            let text = payload
                .downcast_ref::<String>()
                .map(String::as_str)
                .or_else(|| payload.downcast_ref::<&str>().copied())
                .ok_or("debug helper produced a non-string panic")?;
            json!(["panic", text])
        }
    };
    Ok(json!({"ordered": [value]}))
}

pub fn observe(request: &Value) -> Option<Outcome> {
    (subject(request) == "debug").then(|| match run(request) {
        Ok(value) => Outcome::Observed(value),
        Err(error) => Outcome::Failed(error),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_or_mismatched_operation_cannot_observe_a_debug_call() {
        for operation in ["unknown", "tsc/internal/debug/debug.go:Fail"] {
            let request = json!({
                "subject": "debug", "operation": operation,
                "call": "assert", "value": true,
            });
            assert!(matches!(observe(&request), Some(Outcome::Failed(_))));
        }
    }
}
