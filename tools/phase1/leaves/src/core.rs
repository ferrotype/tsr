//! The `internal/core` leaf group: tri-state option semantics, signed source
//! ranges, script kinds and single-wildcard patterns.
//!
//! Most of this group has a real production home, so it drives `tsr_core`
//! directly and expects matches. The two subjects that do not — the tri-state
//! JSON codec and the `TextRange` predicate family — are recorded as gaps
//! naming the Go authority and the intended signature; nothing here emulates
//! them.
//!
//! Byte payloads travel as hex in both directions, because a Go observation of
//! a pattern's text cannot carry invalid UTF-8 through `encoding/json` and the
//! byte-domain cases exist precisely to pin those bytes.
//!
//! The numeric arguments are named for what they are (`tristate`, `kind`)
//! rather than taking the generic `value` key: every probe is handed the whole
//! family schedule, and a neighbouring group already decodes `value` as a
//! string into a typed action struct, which a number here would reject.

use std::panic::AssertUnwindSafe;

use serde_json::{json, Value};
use tsr_core::pattern::{find_best_pattern_match, Pattern};
use tsr_core::{ScriptKind, TextRange, Tristate};

use crate::api::{action_i64, action_op, action_str, actions, ordered, subject, Outcome};

/// The five generated `String()` methods, each with the Rust home it does not
/// have. The port never exposes any of them: the two renderings that do exist
/// are private helpers of consumer crates, written for one call site, and one
/// of them is deliberately partial. Preparation records that; it renders no
/// name here.
const STRINGERS: &[(&str, &str, &str, &str)] = &[
    ("core.Tristate",
     "tsc/internal/core/tristate_stringer_generated.go:Tristate.String",
     "a Display impl or an as_str on tsr_core::Tristate rendering TSUnknown/TSFalse/TSTrue for \
      0/1/2 and Tristate(n) for every other byte. Tristate is a Go byte, so the generated \
      `i < 0` guard is dead and the fallback is reached only past the index table",
     "crates/tsr_core/src/lib.rs (Tristate implements the five predicates and default_if_unknown; \
      it has no Display, no as_str and no name table, and a repo-wide search for the string \
      TSUnknown across crates/ finds nothing)"),
    ("core.ScriptKind",
     "tsc/internal/core/scriptkind_stringer_generated.go:ScriptKind.String",
     "a Display impl or an as_str on tsr_core::ScriptKind rendering the untrimmed \
      ScriptKindUnknown/JS/JSX/TS/TSX for 0..=4 and ScriptKindJSON for 6, with ScriptKind(n) for \
      the unnamed 5 and for everything outside the domain",
     "crates/tsr_core/src/lib.rs (ScriptKind implements from_file_name, ensure_from_file_name \
      and default_extension only; a repo-wide search for the string ScriptKindJSON across \
      crates/ finds nothing)"),
    ("core.LanguageVariant",
     "tsc/internal/core/languagevariant_stringer_generated.go:LanguageVariant.String",
     "a Display impl or an as_str on tsr_core::LanguageVariant rendering the untrimmed \
      LanguageVariantStandard and LanguageVariantJSX for 0 and 1, and LanguageVariant(n) for \
      every other i32 including negatives",
     "crates/tsr_core/src/lib.rs (LanguageVariant declares the two constants and nothing else; a \
      repo-wide search for the string LanguageVariantStandard across crates/ finds nothing)"),
    ("core.ModuleKind",
     "tsc/internal/core/modulekind_stringer_generated.go:ModuleKind.String",
     "one shared renderer on tsr_core::ModuleKind covering all three named runs -- 0..=7, \
      99..=102 and 199..=200 -- with the trimmed names and ModuleKind(n) for the two numeric \
      gaps between them and for everything outside",
     "crates/tsr_core/src/compiler_options.rs (ModuleKind declares the constants, plus \
      is_non_node_esm and supports_import_attributes at :363-374, and no renderer). The port renders this operation in one place only, and not on the type: \
      crates/tsr_checker/src/emit_checks.rs:514 module_kind_text is pub(crate), names all \
      fourteen values and is marked as this operation's source, while \
      crates/tsr_compiler/src/verify_options.rs:584 module_name is a private helper whose only \
      call site (:564) is guarded by (ModuleKind::NODE16..=ModuleKind::NODE_NEXT).contains(&module), \
      so inside its reachable domain it agrees with the pin, including on the unnamed values within \
      that range. Neither rendering is reachable from outside its crate"),
    ("core.ScriptTarget",
     "tsc/internal/core/scripttarget_stringer_generated.go:ScriptTarget.String",
     "one shared renderer on tsr_core::ScriptTarget covering 0..=12 and 99..=100 with the \
      trimmed names and ScriptTarget(n) for the gap between them and for everything outside; \
      the aliases Latest and LatestStandard are ESNext and ES2025 and render as those",
     "crates/tsr_core/src/lib.rs (ScriptTarget declares the constants and nothing else). The \
      only rendering in the port is the private fn script_target_text at \
      crates/tsr_compiler/src/include_reason.rs:684, written for one diagnostic argument and \
      not reachable from outside that crate"),
];

/// Claims a case whose trace is entirely `String()` renderings and records the
/// gap. A trace that mixed a rendering with an observable action could not be
/// answered at all -- the result is one status per case -- so it is refused
/// rather than half-answered.
fn stringer(subject: &str, trace: &[Value]) -> Option<Outcome> {
    let (_, authority, signature, home) = STRINGERS.iter().find(|(name, ..)| *name == subject)?;
    let renderings = trace
        .iter()
        .filter(|action| action_op(action) == "string")
        .count();
    if renderings == 0 {
        return None;
    }
    if renderings != trace.len() {
        return Some(Outcome::Failed(format!(
            "a {subject} case mixes {renderings} String() action(s) with {} action(s) that have \
             a production entry point; one case carries one result, so it is either wholly the \
             recorded gap or wholly observed",
            trace.len() - renderings,
        )));
    }
    Some(Outcome::missing(authority, signature, home))
}

pub fn observe(request: &Value) -> Option<Outcome> {
    if let Some(outcome) = stringer(subject(request), actions(request)) {
        return Some(outcome);
    }
    let replayed = match subject(request) {
        "core.Tristate" => Ok(tristate(actions(request))),
        "core.TextRange" => Ok(text_range(actions(request))),
        "core.ScriptKind" => script_kind(actions(request)),
        "core.Pattern" => pattern(actions(request)),
        "core.TristateJson" => return Some(missing_tristate_json()),
        "core.TextRangePredicates" => return Some(missing_text_range_predicates()),
        _ => return None,
    };
    Some(match replayed {
        Ok(rows) => Outcome::Observed(ordered(rows)),
        Err(problem) => Outcome::Failed(problem),
    })
}

fn missing_tristate_json() -> Outcome {
    Outcome::missing(
        "tsc/internal/core/tristate.go:Tristate.MarshalJSON and Tristate.UnmarshalJSON",
        "a raw-bytes decoder plus a serde impl, because this group observes decoding at both the \
         levels Go exposes it at. \
         (1) An infallible raw-bytes decoder mirroring UnmarshalJSON(data []byte) error, say \
         tsr_core::Tristate::unmarshal_json(&[u8]) -> Self, switching on the exact bytes: \
         b\"true\" is TRUE, b\"false\" is FALSE and every other byte string is UNKNOWN, \
         including b\" true\", b\"TRUE\", the quoted b\"\\\"true\\\"\" and the empty slice, and \
         it never reports an error. A serde Deserialize impl cannot stand in for this: \
         serde_json fixes the token bytes before the impl ever runs. (2) serde \
         Serialize/Deserialize for tsr_core::Tristate, which is where the encoder and the \
         encoding/json-mediated decode path live: \
         TRUE serializes as true, FALSE as false and every other byte as null, and Deserialize \
         accepts any well-formed JSON value, mapping the booleans true and false to TRUE and \
         FALSE and everything else -- a JSON string, including \"true\", a number, null, an \
         object -- to UNKNOWN without an error, while malformed input fails in the parser before \
         the impl is reached",
        "crates/tsr_core/src/lib.rs (Tristate has no codec; tsr_core has no serde dependency)",
    )
}

fn missing_text_range_predicates() -> Outcome {
    Outcome::missing(
        "tsc/internal/core/text.go:TextRange.IsValid, Contains, ContainsInclusive, \
         ContainsExclusive, ContainedBy, Overlaps, Intersects, WithPos, WithEnd, \
         CompareTextRanges and UndefinedTextRange",
        "tsr_core::TextRange::undefined/is_valid/contains/contains_inclusive/contains_exclusive/\
         contained_by/overlaps/intersects/with_pos/with_end, plus a free \
         compare(TextRange, TextRange) -> i64. The endpoints stay the stored i32 pair, but the \
         position arguments and the compare result are machine ints, as they are in Go: \
         contains/contains_inclusive/contains_exclusive take an i64 position and compare it \
         against endpoints widened to i64, with_pos/with_end truncate an i64 argument to i32 the \
         way new does, and compare subtracts widened endpoints so it never wraps -- unlike len, \
         which subtracts in i32 first. Only contained_by/overlaps/intersects compare the stored \
         i32 endpoints directly",
        "crates/tsr_core/src/lib.rs (TextRange implements only new/pos/end/len/is_empty)",
    )
}

fn tristate(trace: &[Value]) -> Vec<Value> {
    let mut rows = Vec::with_capacity(trace.len());
    for action in trace {
        let op = action_op(action);
        let value = action_i64(action, "tristate");
        rows.push(match op {
            "predicates" => {
                let state = Tristate(value as u8);
                json!({
                    "op": op,
                    "tristate": value,
                    "is_true": state.is_true(),
                    "is_false": state.is_false(),
                    "is_unknown": state.is_unknown(),
                    "is_true_or_unknown": state.is_true_or_unknown(),
                    "is_false_or_unknown": state.is_false_or_unknown(),
                })
            }
            "default_if_unknown" => {
                let other = action_i64(action, "tristate_default");
                let result = Tristate(value as u8).default_if_unknown(Tristate(other as u8));
                json!({
                    "op": op,
                    "tristate": value,
                    "tristate_default": other,
                    "result": i64::from(result.0),
                })
            }
            "bool_to_tristate" => {
                let flag = action_bool(action, "flag");
                json!({ "op": op, "flag": flag, "result": i64::from(Tristate::from(flag).0) })
            }
            _ => unsupported(op),
        });
    }
    rows
}

fn text_range(trace: &[Value]) -> Vec<Value> {
    let mut rows = Vec::with_capacity(trace.len());
    for action in trace {
        let op = action_op(action);
        rows.push(match op {
            "new" => {
                let (pos, end) = (action_i64(action, "pos"), action_i64(action, "end"));
                let range = TextRange::new(pos, end);
                json!({
                    "op": op,
                    "pos_in": pos,
                    "end_in": end,
                    "pos": range.pos(),
                    "end": range.end(),
                    "len": range.len(),
                })
            }
            _ => unsupported(op),
        });
    }
    rows
}

fn script_kind(trace: &[Value]) -> Result<Vec<Value>, String> {
    let mut rows = Vec::with_capacity(trace.len());
    for action in trace {
        let op = action_op(action);
        rows.push(match op {
            "from_file_name" | "ensure_from_file_name" => {
                let name = input(action)?;
                let kind = if op == "from_file_name" {
                    ScriptKind::from_file_name(&name)
                } else {
                    ScriptKind::ensure_from_file_name(&name)
                };
                json!({ "op": op, "name_hex": hex(&name), "kind": i64::from(kind.0) })
            }
            "default_extension" => {
                let kind = action_i64(action, "kind");
                let extension = ScriptKind(kind as i32).default_extension();
                json!({
                    "op": op,
                    "kind": kind,
                    "extension": std::str::from_utf8(extension).unwrap_or_default(),
                })
            }
            _ => unsupported(op),
        });
    }
    Ok(rows)
}

fn pattern(trace: &[Value]) -> Result<Vec<Value>, String> {
    // The zero Pattern before any parse, which is the invalid value itself.
    let mut current = Pattern::default();
    let mut rows = Vec::with_capacity(trace.len());
    for action in trace {
        let op = action_op(action);
        rows.push(match op {
            "parse" => {
                current = Pattern::parse(&input(action)?);
                json!({
                    "op": op,
                    "text_hex": hex(&current.text),
                    "star_index": current.star_index as i64,
                    "is_valid": current.is_valid(),
                })
            }
            "is_valid" => json!({ "op": op, "is_valid": current.is_valid() }),
            "matches" => {
                let candidate = input(action)?;
                let (result, panicked) = guarded(AssertUnwindSafe(|| {
                    Value::Bool(current.matches(&candidate))
                }));
                json!({
                    "op": op,
                    "candidate_hex": hex(&candidate),
                    "result": result,
                    "panic": panicked,
                })
            }
            "matched_text" => {
                let candidate = input(action)?;
                let (result, panicked) = guarded(AssertUnwindSafe(|| {
                    Value::String(hex(current.matched_text(&candidate)))
                }));
                json!({
                    "op": op,
                    "candidate_hex": hex(&candidate),
                    "result": result,
                    "panic": panicked,
                })
            }
            "find_best" => {
                let candidate = input(action)?;
                let names = strings(action, "patterns");
                let values = numbers(action, "values");
                // Values are 1-based indices into `patterns`, so the zero value
                // the ported generic returns when nothing matches stays
                // distinguishable from a selected element. An index outside the
                // list is a malformed request, not a pinned behavior, so it
                // fails the case rather than being answered with a Pattern the
                // driver made up; the Go probe rejects the same trace.
                if let Some(index) = values
                    .iter()
                    .find(|value| **value < 1 || **value > names.len() as i64)
                {
                    return Err(format!(
                        "find_best index {index} is outside the {} pattern(s) of a core leaf \
                         action",
                        names.len()
                    ));
                }
                let choose = |value: &i64| Pattern::parse(&names[*value as usize - 1]);
                let (result, panicked) = guarded(AssertUnwindSafe(|| {
                    Value::from(find_best_pattern_match(&values, choose, &candidate))
                }));
                json!({
                    "op": op,
                    "candidate_hex": hex(&candidate),
                    "values": values,
                    "result": result,
                    "panic": panicked,
                })
            }
            _ => unsupported(op),
        });
    }
    Ok(rows)
}

fn unsupported(op: &str) -> Value {
    json!({ "op": op, "unsupported_action": op })
}

fn action_bool(action: &Value, field: &str) -> bool {
    action
        .get(field)
        .and_then(Value::as_bool)
        .unwrap_or_default()
}

fn strings(action: &Value, field: &str) -> Vec<Vec<u8>> {
    action
        .get(field)
        .and_then(Value::as_array)
        .map_or_else(Vec::new, |items| {
            items
                .iter()
                .map(|item| item.as_str().unwrap_or_default().as_bytes().to_vec())
                .collect()
        })
}

fn numbers(action: &Value, field: &str) -> Vec<i64> {
    action
        .get(field)
        .and_then(Value::as_array)
        .map_or_else(Vec::new, |items| {
            items
                .iter()
                .map(|item| item.as_i64().unwrap_or_default())
                .collect()
        })
}

/// An action's single string argument: hex when the case needs a byte string
/// the JSON request cannot carry, plain text otherwise.
fn input(action: &Value) -> Result<Vec<u8>, String> {
    let encoded = action_str(action, "hex");
    if encoded.is_empty() {
        return Ok(action_str(action, "text").as_bytes().to_vec());
    }
    if !encoded.len().is_multiple_of(2) {
        return Err(format!("odd-length hex {encoded:?} in a core leaf action"));
    }
    encoded
        .as_bytes()
        .chunks(2)
        .map(|pair| {
            std::str::from_utf8(pair)
                .ok()
                .and_then(|text| u8::from_str_radix(text, 16).ok())
                .ok_or_else(|| format!("malformed hex {encoded:?} in a core leaf action"))
        })
        .collect()
}

fn hex(value: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(value.len() * 2);
    for byte in value {
        out.push(char::from(DIGITS[usize::from(byte >> 4)]));
        out.push(char::from(DIGITS[usize::from(byte & 0x0f)]));
    }
    out
}

/// Runs `call` and converts a panic into a recorded class, so a case whose
/// subject is the panic boundary observes it instead of aborting the driver.
/// The default hook is silenced for the call: the panic is the expected result
/// here, and its message would otherwise land in the captured stderr.
///
/// Only the class is comparable. Go and Rust word the same failure differently,
/// so an unrecognized panic keeps its text under an `other:` prefix rather than
/// being folded into a class it does not belong to.
fn guarded(call: impl FnOnce() -> Value + std::panic::UnwindSafe) -> (Value, String) {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let result = std::panic::catch_unwind(call);
    std::panic::set_hook(previous);
    match result {
        Ok(value) => (value, String::new()),
        Err(payload) => (Value::Null, classify(payload.as_ref())),
    }
}

fn classify(payload: &(dyn std::any::Any + Send)) -> String {
    let message = payload
        .downcast_ref::<String>()
        .map(String::as_str)
        .or_else(|| payload.downcast_ref::<&str>().copied())
        .unwrap_or("non-string panic payload");
    if message.contains("out of range") || message.contains("out of bounds") {
        // Rust words a slice range and an index panic differently ("out of
        // range for slice of length" against "index out of bounds"), so both
        // are folded here, mirroring the two Go wordings the probe folds.
        "slice_bounds_out_of_range".to_owned()
    } else if message == "candidate does not match pattern" {
        "candidate_does_not_match_pattern".to_owned()
    } else {
        format!("other:{message}")
    }
}
