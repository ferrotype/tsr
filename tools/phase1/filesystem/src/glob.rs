//! The LSP glob group: `tsc/internal/glob`, ported as `tsr_glob`.
//!
//! Every action drives the production crate. The driver claims only the
//! subjects it owns, refuses a request tagged with the other dialect, and
//! validates the action vocabulary before replaying it. The two grammars agree
//! on almost nothing, so a request answered by the wrong adapter would compare
//! two different languages and call the result parity; an untagged request is
//! refused for the same reason.
//!
//! Two Go-only shapes have no production form. A nil `*Glob` is an absent
//! receiver here, and an action that needs one reports the native nil-pointer
//! class, as the ordered-collection driver does. An element of a foreign Go type
//! cannot be built at all, because `tsr_glob::Element` is a closed enum: that
//! row reports `unrepresentable_element` and stays a visible difference rather
//! than imitating the pin's defensive panic.

use serde_json::{json, Value};
use tsr_glob::{Element, Glob};

use crate::api::{action_op, actions, ordered, subject, Outcome};

/// The grammar this group owns. `vfsmatch` is the configuration matcher and
/// belongs to a different adapter.
const DIALECT: &str = "lsp";

const SUBJECTS: &[&str] = &[
    "glob.Glob",
    "glob.element",
    "glob.match",
    "glob.parse",
    "glob.parseLiteral",
    "glob.readRangeRune",
    "glob.split",
];

/// The whole action vocabulary, with the payload keys each action requires.
/// A missing key is a failure rather than a default: a defaulted empty pattern
/// is itself a case in this group and the two could not be told apart.
const VOCABULARY: &[(&str, &[&str])] = &[
    ("parse", &["pattern_hex"]),
    ("parse_nested", &["pattern_hex", "nested"]),
    ("parse_literal", &["pattern_hex", "nested"]),
    ("read_range_rune", &["input_hex"]),
    ("split", &["input_hex"]),
    ("match", &["input_hex"]),
    ("match_elems", &["input_hex"]),
    ("build_elems", &["elems"]),
    ("string", &[]),
    ("string_elems", &[]),
    ("use_nil", &[]),
];

pub fn observe(request: &Value) -> Option<Outcome> {
    if !SUBJECTS.contains(&subject(request)) {
        return None;
    }
    if let Err(problem) = check(request) {
        return Some(Outcome::Failed(problem));
    }
    Some(match replay(actions(request)) {
        Ok(rows) => Outcome::Observed(ordered(rows)),
        Err(problem) => Outcome::Failed(problem),
    })
}

/// The glob an action would call. `Nil` is Go's nil receiver after `use_nil`;
/// `Unrepresentable` is a built list holding a kind the closed enum cannot carry.
#[derive(Default)]
enum Receiver {
    #[default]
    Unset,
    Nil,
    Unrepresentable,
    Glob(Glob),
}

#[derive(Default)]
struct State {
    glob: Receiver,
    elements: Option<Vec<Built>>,
}

/// A built element list entry: a production element, or the Go-only foreign kind.
enum Built {
    Element(Element),
    Foreign,
}

fn replay(trace: &[Value]) -> Result<Vec<Value>, String> {
    let mut state = State::default();
    trace.iter().map(|action| row(&mut state, action)).collect()
}

fn row(state: &mut State, action: &Value) -> Result<Value, String> {
    let op = action_op(action);
    Ok(match op {
        "parse" => {
            let parsed = Glob::parse(&payload(action, "pattern_hex")?);
            let row = parse_row(op, parsed.as_ref().map_err(|e| *e), None);
            if let Ok(glob) = parsed {
                state.elements = Some(glob.elements.iter().cloned().map(Built::Element).collect());
                state.glob = Receiver::Glob(glob);
            } else {
                state.glob = Receiver::Unset;
            }
            row
        }
        "parse_nested" => {
            let pattern = payload(action, "pattern_hex")?;
            match tsr_glob::parse(&pattern, flag(action, "nested")?) {
                Ok((glob, residual)) => parse_row(op, Ok(&glob), Some(residual)),
                Err(error) => parse_row(op, Err(error), Some(b"")),
            }
        }
        "parse_literal" => {
            let pattern = payload(action, "pattern_hex")?;
            let mut fresh = Glob::default();
            let residual = fresh
                .parse_literal(&pattern, flag(action, "nested")?)
                .to_vec();
            json!({ "op": op, "appended": fresh.elements.len(), "elems": kinds(&fresh.elements),
                    "residual_hex": hex(&residual) })
        }
        "read_range_rune" => match tsr_glob::read_range_rune(&payload(action, "input_hex")?) {
            Ok((rune, size)) => json!({ "op": op, "rune": rune, "size": size, "error": "" }),
            // The pin returns the decoded value and size alongside the error.
            Err(error) => {
                let input = payload(action, "input_hex")?;
                let (rune, size) = tsr_jsstring::wtf8::decode_utf8(&input);
                json!({ "op": op, "rune": rune, "size": size, "error": error.to_string() })
            }
        },
        "split" => {
            let input = payload(action, "input_hex")?;
            let (first, rest) = tsr_glob::split(&input);
            json!({ "op": op, "first_hex": hex(first), "rest_hex": hex(rest) })
        }
        "build_elems" => {
            let specs = action
                .get("elems")
                .and_then(Value::as_array)
                .ok_or("build_elems needs elems")?;
            let built = specs.iter().map(build).collect::<Result<Vec<_>, _>>()?;
            let rendered = built_kinds(&built);
            state.glob = production(&built).map_or(Receiver::Unrepresentable, |elements| {
                Receiver::Glob(Glob { elements })
            });
            let count = built.len();
            state.elements = Some(built);
            json!({ "op": op, "count": count, "elems": rendered })
        }
        "use_nil" => {
            state.glob = Receiver::Nil;
            state.elements = None;
            json!({ "op": op })
        }
        "string" => match &state.glob {
            Receiver::Glob(glob) => {
                json!({ "op": op, "string_hex": hex(&glob.to_bytes()), "panic": "" })
            }
            Receiver::Nil => json!({ "op": op, "string_hex": null, "panic": NIL }),
            Receiver::Unrepresentable => {
                json!({ "op": op, "string_hex": null, "panic": UNREPRESENTABLE })
            }
            Receiver::Unset => return Err("action needs a glob".into()),
        },
        "string_elems" => {
            let elements = state
                .elements
                .as_ref()
                .ok_or("action needs an element list")?;
            let rendered: Vec<Value> = elements
                .iter()
                .map(|built| match built {
                    Built::Element(element) => Value::String(hex(&element.to_bytes())),
                    Built::Foreign => Value::String(UNREPRESENTABLE.into()),
                })
                .collect();
            json!({ "op": op, "rendered": rendered })
        }
        "match" => {
            let input = payload(action, "input_hex")?;
            match &state.glob {
                Receiver::Glob(glob) => {
                    let glob = glob.clone();
                    let (value, panicked) = guarded(move || glob.matches(&input));
                    json!({ "op": op, "result": value, "panic": panicked })
                }
                Receiver::Nil => json!({ "op": op, "result": null, "panic": NIL }),
                Receiver::Unrepresentable => {
                    json!({ "op": op, "result": null, "panic": UNREPRESENTABLE })
                }
                Receiver::Unset => return Err("action needs a glob".into()),
            }
        }
        "match_elems" => {
            let input = payload(action, "input_hex")?;
            let elements = state
                .elements
                .as_ref()
                .ok_or("action needs an element list")?;
            match production(elements) {
                Some(elements) => {
                    let (value, panicked) =
                        guarded(move || tsr_glob::match_elements(&elements, &input));
                    json!({ "op": op, "result": value, "panic": panicked })
                }
                None => json!({ "op": op, "result": null, "panic": UNREPRESENTABLE }),
            }
        }
        other => return Err(format!("unsupported action {other:?}")),
    })
}

const NIL: &str = "nil_pointer_dereference";
const UNREPRESENTABLE: &str = "unrepresentable_element";

fn parse_row(op: &str, parsed: Result<&Glob, tsr_glob::Error>, residual: Option<&[u8]>) -> Value {
    let mut row = match parsed {
        Ok(glob) => {
            json!({ "op": op, "accepted": true, "error": "", "elems": kinds(&glob.elements) })
        }
        Err(error) => {
            json!({ "op": op, "accepted": false, "error": error.to_string(), "elems": [] })
        }
    };
    if let Some(residual) = residual {
        row["residual_hex"] = Value::String(hex(residual));
    }
    row
}

/// The probe's structural vocabulary for an element list.
fn kinds(elements: &[Element]) -> Value {
    Value::Array(elements.iter().map(kind).collect())
}
fn kind(element: &Element) -> Value {
    match element {
        Element::Slash => json!("slash"),
        Element::Star => json!("star"),
        Element::StarStar => json!("star_star"),
        Element::AnyChar => json!("any_char"),
        Element::Literal(bytes) => json!(["literal", hex(bytes)]),
        Element::CharRange { negate, low, high } => json!(["char_range", negate, low, high]),
        Element::Group(members) => {
            let members: Vec<Value> = members.iter().map(|m| kinds(&m.elements)).collect();
            json!(["group", members])
        }
    }
}
fn built_kinds(built: &[Built]) -> Value {
    Value::Array(
        built
            .iter()
            .map(|entry| match entry {
                Built::Element(element) => kind(element),
                Built::Foreign => json!("foreign"),
            })
            .collect(),
    )
}
/// The production list, or `None` when it holds a kind the enum cannot represent.
fn production(built: &[Built]) -> Option<Vec<Element>> {
    built
        .iter()
        .map(|entry| match entry {
            Built::Element(element) => Some(element.clone()),
            Built::Foreign => None,
        })
        .collect()
}
fn build(spec: &Value) -> Result<Built, String> {
    let kind = spec
        .get("kind")
        .and_then(Value::as_str)
        .ok_or("an element spec must name its kind")?;
    Ok(Built::Element(match kind {
        "slash" => Element::Slash,
        "star" => Element::Star,
        "star_star" => Element::StarStar,
        "any_char" => Element::AnyChar,
        "literal" => Element::Literal(payload(spec, "literal_hex")?),
        "char_range" => Element::CharRange {
            negate: flag(spec, "negate")?,
            low: point(spec, "low")?,
            high: point(spec, "high")?,
        },
        // A nil group and an empty one differ only in memory.
        "group_nil" => Element::Group(Vec::new()),
        "group" => {
            let members = spec
                .get("members")
                .and_then(Value::as_array)
                .ok_or("group needs members")?;
            let mut globs = Vec::new();
            for member in members {
                let specs = member
                    .as_array()
                    .ok_or("a group member is an element list")?;
                let built = specs.iter().map(build).collect::<Result<Vec<_>, _>>()?;
                let elements = production(&built).ok_or("a foreign element inside a group")?;
                globs.push(Glob { elements });
            }
            Element::Group(globs)
        }
        "foreign" => return Ok(Built::Foreign),
        other => return Err(format!("unsupported element kind {other:?}")),
    }))
}
fn point(spec: &Value, key: &str) -> Result<i32, String> {
    spec.get(key)
        .and_then(Value::as_i64)
        .and_then(|value| i32::try_from(value).ok())
        .ok_or_else(|| format!("a char_range element needs {key}"))
}
fn flag(action: &Value, key: &str) -> Result<bool, String> {
    action
        .get(key)
        .and_then(Value::as_bool)
        .ok_or_else(|| format!("missing boolean {key}"))
}
fn payload(action: &Value, key: &str) -> Result<Vec<u8>, String> {
    let text = action
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("missing {key}"))?;
    (0..text.len())
        .step_by(2)
        .map(|i| {
            u8::from_str_radix(text.get(i..i + 2).unwrap_or(""), 16)
                .map_err(|_| format!("{key} is not hex"))
        })
        .collect()
}
fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    bytes.iter().fold(String::new(), |mut out, byte| {
        write!(out, "{byte:02x}").expect("writing to a String is infallible");
        out
    })
}
/// Records the class of a panic the pinned matcher raises by design.
fn guarded(operation: impl FnOnce() -> bool + std::panic::UnwindSafe) -> (Value, String) {
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let outcome = std::panic::catch_unwind(operation);
    std::panic::set_hook(hook);
    match outcome {
        Ok(value) => (Value::Bool(value), String::new()),
        Err(payload) => {
            let message = payload
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| {
                    payload
                        .downcast_ref::<&str>()
                        .map(|text| (*text).to_string())
                })
                .unwrap_or_default();
            let class = if message.contains("out of range") || message.contains("out of bounds") {
                "index_out_of_range".to_string()
            } else {
                format!("other:{message}")
            };
            (Value::Null, class)
        }
    }
}

fn check(request: &Value) -> Result<(), String> {
    match request.get("dialect").and_then(Value::as_str) {
        Some(DIALECT) => {}
        Some(other) => {
            return Err(format!(
                "request is tagged dialect {other:?}; this group owns the {DIALECT} glob grammar \
                 of tsc/internal/glob, and the configuration matcher is a different language \
                 answered by a different adapter"
            ))
        }
        None => {
            return Err(
                "a glob request must declare its dialect; defaulting one would let this grammar \
                 answer for the configuration matcher"
                    .into(),
            )
        }
    }
    let trace = actions(request);
    if trace.is_empty() {
        return Err("a glob case carries an ordered action trace; this request has none".into());
    }
    for action in trace {
        check_action(action)?;
    }
    Ok(())
}

fn check_action(action: &Value) -> Result<(), String> {
    let op = action_op(action);
    let (_, required) = VOCABULARY
        .iter()
        .find(|(name, _)| *name == op)
        .ok_or_else(|| {
            format!(
                "unsupported action {op:?}: an action this group does not define is a harness \
             failure, never an observation"
            )
        })?;
    for field in *required {
        let value = action
            .get(*field)
            .ok_or_else(|| format!("action {op:?} is missing {field}"))?;
        match *field {
            "nested" => {
                value
                    .as_bool()
                    .ok_or_else(|| format!("action {op:?} needs a boolean {field}"))?;
            }
            "elems" => {
                let specs = value
                    .as_array()
                    .ok_or_else(|| format!("action {op:?} needs an array of element specs"))?;
                for spec in specs {
                    check_element(spec)?;
                }
            }
            _ => check_hex(value, field, op)?,
        }
    }
    Ok(())
}

/// Byte payloads travel as hex because a pattern may hold invalid UTF-8.
fn check_hex(value: &Value, field: &str, op: &str) -> Result<(), String> {
    let text = value
        .as_str()
        .ok_or_else(|| format!("action {op:?} needs {field} as a hex string"))?;
    if !text.len().is_multiple_of(2) || !text.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(format!(
            "action {op:?} carries {field}={text:?}, which is not hex"
        ));
    }
    Ok(())
}

fn check_element(spec: &Value) -> Result<(), String> {
    let kind = spec
        .get("kind")
        .and_then(Value::as_str)
        .ok_or_else(|| "an element spec must name its kind".to_owned())?;
    match kind {
        "slash" | "star" | "star_star" | "any_char" | "group_nil" | "foreign" => Ok(()),
        "literal" => {
            let value = spec
                .get("literal_hex")
                .ok_or_else(|| "a literal element needs literal_hex".to_owned())?;
            check_hex(value, "literal_hex", "build_elems")
        }
        "char_range" => {
            spec.get("negate")
                .and_then(Value::as_bool)
                .ok_or_else(|| "a char_range element needs a boolean negate".to_owned())?;
            for bound in ["low", "high"] {
                let point = spec
                    .get(bound)
                    .and_then(Value::as_u64)
                    .ok_or_else(|| format!("a char_range element needs {bound}"))?;
                if point > 0x0010_FFFF {
                    return Err(format!("char_range {bound}={point} is not a code point"));
                }
            }
            Ok(())
        }
        "group" => {
            let members = spec
                .get("members")
                .and_then(Value::as_array)
                .ok_or_else(|| "a group element needs a members array".to_owned())?;
            for member in members {
                let elements = member
                    .as_array()
                    .ok_or_else(|| "a group member is an element list".to_owned())?;
                for element in elements {
                    check_element(element)?;
                }
            }
            Ok(())
        }
        other => Err(format!("unsupported element kind {other:?}")),
    }
}
