//! The generic core helper leaf group: the 25 operations of
//! `tsc/internal/core/core.go` that the rest of the compiler is written in.
//!
//! Twenty-four of them are recorded gaps. The workspace carries exactly one
//! `port:` annotation for any of them -- `crates/tsr_tsoptions/src/config_json.rs:22`
//! for `StringifyJson` -- and the other twenty-four are spelled out at each
//! call site as Rust iterator adapters rather than as named functions, so
//! there is no entry point to drive. Preparation records that; it never
//! emulates the algorithm here to make a comparison run.
//!
//! `StringifyJson` is driven for real, over exactly the domain its port
//! covers: that port is specialised twice, to `ConfigValue` rather than Go's
//! `any` and to the compact arm, because it takes no prefix or indent. A trace
//! that asks for an indent leaves the domain and is recorded as the gap it is.
//!
//! What each gap record has to carry is not the Go signature transliterated.
//! It is the part of the pinned body a Rust port silently drops, which for
//! this family is almost always one of four things: whether the result is nil
//! or a non-nil empty slice (they marshal as `null` and `[]`), whether the
//! result IS an input slice rather than a copy of one, how many times the
//! callback actually ran, or what the panic boundary is. The Go probe's trace
//! pins the answer; the `intended_signature` here is what the port must carry
//! to match it.

use crate::api::{action_op, action_str, actions, ordered, subject, Outcome};
use serde_json::{json, Value};
use tsr_jsstring::JsString;
use tsr_tsoptions::{stringify_json, ConfigValue};

/// The production home shared by the twenty-four operations with no port at
/// all. Verified by grepping `crates/` for `core/core.go:` annotations: the
/// only ones that exist are line_map's ECMA line starts, scanner's spelling
/// suggestion, tsr_core's script kinds and tsoptions' StringifyJson.
const NO_HOME: &str =
    "no Rust entry point a caller could reach for the generic operation: tsr_core has no \
     generic-helper module, and the ports of these helpers are open-coded as iterator adapters at \
     each call site. A `port:` annotation search alone understates this -- annotations are not \
     exhaustive -- so the search behind each record is by name as well";

/// Two of these helpers *are* ported, but onto an arena-backed declaration-list
/// handle rather than a generic slice, so this harness's string traces cannot
/// reach them and the record names the home instead of claiming there is none.
/// Recording a gap that does not exist would send the implementation step to
/// rewrite working code.
const DECLARATION_LIST_HOME: &str =
    "ported, but not as a generic operation and not reachable from here: \
     crates/tsr_ast/src/declaration_lists.rs carries `same` at :57 and `append_if_unique` at :382 \
     (and crates/tsr_ast/src/lists.rs:54 repeats `same` through its macro), each on a copyable \
     list header over an arena rather than on a slice. `same` there is byte for byte the pinned \
     contract -- equal lengths, and either both empty or the same backing and the same start. What \
     is missing is a generic home a caller holding an ordinary Vec could use, which is what these \
     rows record";

/// One record per pinned operation. The id is also the Go authority, because a
/// generic helper's authority is the function itself; the other two fields are
/// the contract a port has to carry and where that port would live.
const MISSING: &[(&str, &str, &str)] = &[
    (
        "tsc/internal/core/core.go:Filter",
        "a filter that returns the INPUT unchanged when every element passes -- Cow-shaped, \
         not a fresh Vec -- and that distinguishes three empty results the way the pinned body \
         does: a nil input comes back nil (the scan never runs), a non-nil input whose first \
         element is rejected comes back as slices.Clone of a zero-length prefix, which is a \
         non-nil EMPTY slice, and a non-nil empty input comes back as itself. The predicate is \
         called once per element up to and including the rejecting one, then once for each \
         element after it",
        NO_HOME,
    ),
    (
        "tsc/internal/core/core.go:Same",
        "a free `same(a, b) -> bool` that is neither slice equality nor pointer equality: equal \
         lengths AND (length zero OR the same first-element address). Two empty slices are same \
         whatever their provenance, a nil and a non-nil empty one included; two distinct slices \
         with equal contents are not. It is the instrument the rest of this group's identity \
         cases are written with, so it has to exist before they can be answered",
        DECLARATION_LIST_HOME,
    ),
    (
        "tsc/internal/core/core.go:Map",
        "a map that PRESERVES the nil/non-nil distinction of its input -- nil in, nil out; \
         non-nil empty in, non-nil empty out -- and that always allocates, so even an identity \
         mapping returns a slice that is not the input. In Rust that is a signature over \
         Option<&[T]> or a Vec-versus-None result, not `iter().map().collect()`, which has no \
         way to say `null`",
        NO_HOME,
    ),
    (
        "tsc/internal/core/core.go:MapIndex",
        "Map's nil-preserving contract with the callback taking (element, index) in that \
         order",
        NO_HOME,
    ),
    (
        "tsc/internal/core/core.go:MapFiltered",
        "a filter-map accumulating into an absent (nil) result, so it returns nil -- NOT an \
         empty slice -- for a nil input, for a non-nil empty input and for an input whose every \
         element is dropped. That is the opposite of Map on the same non-nil empty input, and \
         the two live in one file, so a port cannot normalise both to `Vec::new()`. The \
         callback runs once per element, drop or keep",
        NO_HOME,
    ),
    (
        "tsc/internal/core/core.go:FlatMap",
        "a flat-map accumulating into an absent (nil) result: a non-empty input whose every \
         mapped piece is empty yields nil, not an empty slice, and the callback runs once per \
         element",
        NO_HOME,
    ),
    (
        "tsc/internal/core/core.go:Flatten",
        "a flatten accumulating into an absent (nil) result: an empty outer slice and an outer \
         slice of empty (or absent) inner slices both yield nil. Unlike Concatenate it never \
         returns an operand -- flattening a single non-empty subarray copies it -- so a port \
         must not shortcut the one-element case",
        NO_HOME,
    ),
    (
        "tsc/internal/core/core.go:Concatenate",
        "a concatenation that returns an OPERAND ITSELF whenever the other side is empty, so \
         the result aliases the caller's storage, and whose two empty guards are ORDERED: the \
         second operand is tested first, which makes concatenate(nil, empty) nil and \
         concatenate(empty, nil) a non-nil empty slice. Only the both-non-empty case allocates",
        NO_HOME,
    ),
    (
        "tsc/internal/core/core.go:AppendIfUnique",
        "an append that returns the input unchanged when the element is already present (linear \
         search, first match wins) and otherwise appends. The Go append writes into the input's \
         spare capacity, so appending to a re-sliced prefix overwrites the element past that \
         prefix in the caller's backing array; a Vec-based port cannot reproduce that, so the \
         port owes either the same aliasing or the evidence that the pinned callers -- which \
         assign the result straight back, as binder.go:2548 does -- never observe it",
        DECLARATION_LIST_HOME,
    ),
    (
        "tsc/internal/core/core.go:Deduplicate",
        "a whole-slice deduplication keeping the FIRST occurrence of each value, which returns \
         the input unchanged whenever nothing is removed (including length 0 and 1). Vec::dedup \
         is not this function: it removes only consecutive duplicates",
        NO_HOME,
    ),
    (
        "tsc/internal/core/core.go:Find",
        "find/find_last/find_index as three separate entry points with Go's sentinels, because \
         the call sites are written against them: find and find_last return the ELEMENT TYPE's \
         zero value on a miss, which for a string element makes a matched empty string and a \
         failed search the same answer, and find_index returns -1. A port returning Option<&T> \
         is a different and better contract, and adopting it means re-reading every call site \
         rather than translating the body",
        NO_HOME,
    ),
    (
        "tsc/internal/core/core.go:FindIndex",
        "the first matching index or -1; scans forward and stops at the first match",
        NO_HOME,
    ),
    (
        "tsc/internal/core/core.go:FindLast",
        "the last matching element or the element type's zero value; scans BACKWARD, so the \
         predicate runs a different number of times than Find's for the same input",
        NO_HOME,
    ),
    (
        "tsc/internal/core/core.go:FirstOrNil",
        "first_or_nil/last_or_nil returning the element type's zero value for an empty or nil \
         slice -- no panic and no Option, because callers such as \
         upstream/tsc/internal/vfs/vfsmatch/vfsmatch.go:145 compare the result against a \
         literal instead of testing for absence",
        NO_HOME,
    ),
    (
        "tsc/internal/core/core.go:LastOrNil",
        "the last element or the element type's zero value for an empty or nil slice",
        NO_HOME,
    ),
    (
        "tsc/internal/core/core.go:Some",
        "a short-circuiting any(): the predicate must not run after the first true. The values \
         a port produces are right whichever way it is written, so the contract that matters is \
         the call count",
        NO_HOME,
    ),
    (
        "tsc/internal/core/core.go:SameMap",
        "a map over a comparable element type that returns the INPUT ITSELF when the mapping \
         changes nothing, and otherwise calls f exactly len(slice) times, never len+1: the \
         scan's own result is reused for the element that first changed and the already-equal \
         prefix is copied unmapped. `map().collect()` followed by an equality test gives the \
         right values and the right count and still allocates for the unchanged case",
        NO_HOME,
    ),
    (
        "tsc/internal/core/core.go:SingleElementSlice",
        "a wrap of an optional pointer that yields an ABSENT (nil) slice for an absent element \
         -- not an empty slice, and not a one-element slice of nil -- and otherwise a \
         one-element slice holding the caller's own pointer, so a write through the element is \
         visible to the caller. Over Option<&T> that is Option<Vec<&T>>; returning Vec::new() \
         for None erases the distinction the callers pass straight into a file list",
        NO_HOME,
    ),
    (
        "tsc/internal/core/core.go:Memoize",
        "a memoizer that caches the FIRST result whatever it is, including the zero value, and \
         that clears the producer only AFTER it returns, so a producer that panics is retried \
         by the next call and can then succeed. `if value == zero { recompute }` is the wrong \
         port and the pinned callers -- checker.go:1168 memoizes an *ast.Symbol lookup whose \
         legitimate answer is nil -- are what make it wrong; a once-cell that latches or \
         poisons on a panicking initialiser is wrong the other way",
        NO_HOME,
    ),
    (
        "tsc/internal/core/core.go:Must",
        "an unwrap of a (value, error) pair that panics if and only if the error is present -- \
         an error with an empty message still panics -- and that panics WITH the error value, \
         not with a rendering of it, so a handler can still recover the typed error. In Rust \
         the error payload survives as a panic payload only if it is a value and not a \
         formatted string",
        NO_HOME,
    ),
    (
        "tsc/internal/core/core.go:OrElse",
        "a zero-value fallback over a comparable type: the test is `value != zero` on the value \
         itself, so \"0\" and \"false\" are kept and only \"\" is replaced. It is not \
         Option::unwrap_or (there is no absence to test) and not a truthiness test",
        NO_HOME,
    ),
    (
        "tsc/internal/core/core.go:IfElse",
        "a non-short-circuiting select on the boolean alone, argument order (condition, \
         when_true, when_false). It must not degrade into OrElse: if_else(true, zero, other) is \
         the zero value",
        NO_HOME,
    ),
    (
        "tsc/internal/core/core.go:ConcatenateSeq",
        "a lazy chain over a variadic list of OPTIONAL sequences that SKIPS an absent operand \
         rather than treating it as empty-by-dereference, and that propagates a consumer's \
         early stop out of the whole chain instead of moving to the next operand. Absent \
         operands are real: format/indent.go:160-166 hands it a sequence variable left unset \
         when there is no preceding token",
        NO_HOME,
    ),
    (
        "tsc/internal/core/core.go:comparableValuesEqual",
        "the default equality DiffMaps hands to DiffMapsFunc: Go's `==`, which on a pointer and \
         on a struct carrying one is ADDRESS equality, never the pointee. Its only non-test \
         caller diffs map[tspath.Path]*ast.SourceFile (api/session.go:3845), so a structural \
         PartialEq reports `unchanged` exactly where the pinned session reports a changed file",
        NO_HOME,
    ),
    (
        "tsc/internal/core/core.go:StringifyJson",
        "the PREFIX/INDENT arm, which this record is reached for and which has no Rust home at \
         all: `MarshalIndent` installs the indent options only when prefix or indent is \
         non-empty (upstream/tsc/internal/json/json.go:41-47), so the same value has a compact \
         and a multiline rendering, an empty array stays on one line whatever the indent, and a \
         non-whitespace prefix panics inside the encoder. A port owes an entry point that takes \
         both, over the same value domain as the compact arm. The compact arm itself is not a \
         gap and is driven for real here",
        "crates/tsr_tsoptions/src/config_json.rs:23 `stringify_json(&ConfigValue) -> \
         Result<Vec<u8>, JsonError>` is the workspace's only annotated port of this operation. \
         It covers the compact arm, which this group therefore compares for real, but it takes \
         no prefix or indent and is specialised to ConfigValue rather than Go's `any`, so the \
         indented arm has nowhere to run",
    ),
];

/// The action vocabulary each subject accepts. It is checked here, and not
/// only in the Go probe, because an unknown action has to be a harness failure
/// on BOTH sides: a typo that one side quietly ignores would otherwise become
/// a row the two agree on without either having executed anything.
fn vocabulary(subject: &str) -> Option<&'static [&'static str]> {
    Some(match subject {
        "helpers.Slices" => &[
            "set",
            "alias",
            "slice_from",
            "slice_to",
            "read",
            "mutate",
            "same",
            "filter",
            "deduplicate",
            "append_if_unique",
            "concatenate",
            "map",
            "map_index",
            "map_filtered",
            "flat_map",
            "flatten",
            "flatten_register",
            "same_map",
            "some",
            "find",
            "find_last",
            "find_index",
            "first_or_nil",
            "last_or_nil",
        ],
        "helpers.SingleElementSlice" => &[
            "new_box",
            "clear_box",
            "single",
            "mutate_element",
            "read_box",
        ],
        "helpers.Memoize" => &["memoize", "memoize_panicking", "call"],
        "helpers.Must" => &["must"],
        "helpers.Values" => &["or_else", "if_else"],
        "helpers.ConcatenateSeq" => &["reset_seqs", "push_seq", "push_nil_seq", "collect"],
        "helpers.ComparableValues" => &["new_box", "new_nil_box", "strings", "pointers", "pairs"],
        "helpers.StringifyJson" => &["stringify"],
        _ => return None,
    })
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

/// Decode an action's byte argument: hex when the case needs bytes a JSON
/// string could not carry, plain text otherwise. Malformed hex is a malformed
/// request, not an observation.
fn input_bytes(action: &Value) -> Result<Vec<u8>, String> {
    let encoded = action_str(action, "hex");
    if encoded.is_empty() {
        return Ok(action_str(action, "text").as_bytes().to_vec());
    }
    if !encoded.len().is_multiple_of(2) {
        return Err(format!("action carries malformed hex {encoded:?}"));
    }
    let mut out = Vec::with_capacity(encoded.len() / 2);
    let raw = encoded.as_bytes();
    for pair in raw.chunks_exact(2) {
        let digit = |byte: u8| match byte {
            b'0'..=b'9' => Ok(byte - b'0'),
            b'a'..=b'f' => Ok(byte - b'a' + 10),
            b'A'..=b'F' => Ok(byte - b'A' + 10),
            _ => Err(format!("action carries malformed hex {encoded:?}")),
        };
        out.push(digit(pair[0])? << 4 | digit(pair[1])?);
    }
    Ok(out)
}

fn config_strings(action: &Value) -> Vec<ConfigValue> {
    action
        .get("items")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(|item| ConfigValue::String(JsString::from_bytes(item.as_bytes().to_vec())))
                .collect()
        })
        .unwrap_or_default()
}

/// The value a stringify action marshals. `Array(None)` is the port's model of
/// a Go typed nil slice -- convert_options.rs:190 builds exactly that where the
/// pinned parser has a nil `[]string` -- and `Null` is the nil interface; the
/// two are different documents (`[]` and `null`), which is half of what the
/// case pins.
fn stringify_input(action: &Value) -> Result<ConfigValue, String> {
    Ok(match action_str(action, "shape") {
        "string" => ConfigValue::String(JsString::from_bytes(input_bytes(action)?)),
        "strings" => ConfigValue::Array(Some(config_strings(action))),
        "strings_empty" => ConfigValue::Array(Some(Vec::new())),
        "strings_nil" => ConfigValue::Array(None),
        "nil_any" => ConfigValue::Null,
        "nested" => ConfigValue::Array(Some(
            action
                .get("groups")
                .and_then(Value::as_array)
                .map(|groups| {
                    groups
                        .iter()
                        .map(|group| {
                            group.as_array().map_or(ConfigValue::Array(None), |items| {
                                ConfigValue::Array(Some(
                                    items
                                        .iter()
                                        .filter_map(Value::as_str)
                                        .map(|item| {
                                            ConfigValue::String(JsString::from_bytes(
                                                item.as_bytes().to_vec(),
                                            ))
                                        })
                                        .collect(),
                                ))
                            })
                        })
                        .collect()
                })
                .unwrap_or_default(),
        )),
        other => return Err(format!("stringify action carries unknown shape {other:?}")),
    })
}

/// The one operation in this group with a real Rust home. The port is
/// specialised twice over: to `ConfigValue` rather than Go's `any`, and to the
/// compact arm, because it takes no prefix or indent. A trace that stays inside
/// that domain is answered for real; one that leaves it is recorded as the gap
/// it is, rather than answered by something this harness made up.
fn stringify(trace: &[Value]) -> Option<Result<Vec<Value>, String>> {
    if trace.iter().any(|action| {
        !action_str(action, "prefix").is_empty() || !action_str(action, "indent").is_empty()
    }) {
        return None;
    }
    let mut rows = Vec::with_capacity(trace.len());
    for action in trace {
        let input = match stringify_input(action) {
            Ok(input) => input,
            Err(problem) => return Some(Err(problem)),
        };
        let observed = stringify_json(&input);
        rows.push(json!({
            "op": action_op(action),
            "shape": action_str(action, "shape"),
            "prefix": action_str(action, "prefix"),
            "indent": action_str(action, "indent"),
            "result_hex": hex(observed.as_deref().unwrap_or_default()),
            "error": observed.is_err(),
        }));
    }
    Some(Ok(rows))
}

pub fn observe(request: &Value) -> Option<Outcome> {
    let declared = subject(request);
    let known = vocabulary(declared)?;
    for action in actions(request) {
        let op = action_op(action);
        if !known.contains(&op) {
            return Some(Outcome::Failed(format!(
                "unsupported action {op:?} for subject {declared:?}"
            )));
        }
    }
    if declared == "helpers.StringifyJson" {
        match stringify(actions(request)) {
            Some(Ok(rows)) => return Some(Outcome::Observed(ordered(rows))),
            Some(Err(problem)) => return Some(Outcome::Failed(problem)),
            None => {}
        }
    }
    let operation = request
        .get("operation")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let record = MISSING.iter().find(|(id, _, _)| *id == operation).copied();
    Some(match record {
        Some((authority, signature, home)) => {
            Outcome::missing(authority, authority, signature, home)
        }
        None => Outcome::Failed(format!(
            "case declares operation {operation:?}, which this group has no gap record for"
        )),
    })
}
