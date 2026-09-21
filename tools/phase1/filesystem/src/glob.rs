//! The LSP glob group: `tsc/internal/glob`, which has no Rust home at all.
//!
//! Every one of this group's fifteen operations is a recorded gap. The port
//! has one glob file, `crates/tsr_tsoptions/src/glob.rs`, and it is the OTHER
//! dialect: its first line reads "Pinned vfsmatch patterns", every port marker
//! in it names `vfs/vfsmatch/vfsmatch.go`, and its element vocabulary
//! (`Segment::Literal/Star/Question`, `Component::Literal/Wildcard/DoubleAsterisk`
//! at :21-31) has no alternation, no character range and no separator element,
//! so it could not represent `{a,b}`, `[a-z]` or `a//b` even as data. A
//! repository-wide search for `internal/glob` across `crates/` finds nothing.
//!
//! So this module never emulates the grammar to make a comparison run. It does
//! three things: it claims only the subjects it owns, it refuses a request
//! tagged with the other dialect, and it validates the action vocabulary before
//! recording the gap.
//!
//! The dialect refusal is the point of the tag. The two grammars agree on
//! almost nothing -- `*` here spans a whole path segment and then cannot meet
//! the separator that follows it, while vfsmatch's star is per-component -- so
//! a request answered by the wrong adapter would compare two different
//! languages and call the result parity. Neither side may dispatch to the other
//! silently, and an untagged request is refused for the same reason: a default
//! would pick a grammar the request never named.
//!
//! The vocabulary check is the second half. An action this group does not
//! define, or one missing a payload key, is a harness failure on both sides:
//! the Go probe panics on it and this driver fails on it, so a malformed
//! request can never become a row the two sides agree on without executing
//! anything. It never inspects a value's meaning -- there is nothing here to
//! run it against -- only that the request is well formed.

use serde_json::Value;

use crate::api::{action_op, actions, subject, Outcome};

/// The grammar this group owns. `vfsmatch` is the configuration matcher and
/// belongs to a different adapter.
const DIALECT: &str = "lsp";

/// The subjects of this group, each with the Rust home it does not have.
///
/// `tsr_glob` is the crate the ledger reserves for this package (scope.json
/// records `ledger_crate: tsr_glob`, `ledger_status: planned` for all fifteen
/// operations); no such directory exists under `crates/`.
const MISSING: &[(&str, &str, &str, &str)] = &[
    ("glob.Glob",
     "tsc/internal/glob/glob.go:Glob",
     "pub struct Glob holding an ordered element list, with a parse entry point returning \
      Result<Glob, GlobError> over the four pinned errors, a Display rendering the element list \
      back to the pattern text, and a matches(&[u8]) -> bool. Patterns and inputs are bytes, not \
      str: the pin accepts a literal 0xff and matches it byte-wise, and it accepts [<U+FFFD>-a] \
      while rejecting [\\xff-\\xff]",
     "crates/tsr_glob (absent; the crate the ledger reserves for internal/glob). \
      crates/tsr_tsoptions/src/glob.rs is the vfsmatch dialect and has no Display for its \
      Pattern, no parse that returns an error and no element type for a separator run"),
    ("glob.element",
     "tsc/internal/glob/glob.go:element",
     "a closed element enum -- Slash, Literal(Vec<u8>), Star, AnyChar, StarStar, \
      Group(Vec<Glob>), CharRange { negate, low, high } -- each rendering itself: the four \
      constants \"/\", \"*\", \"?\" and \"**\", a literal rendering its bytes unchanged, a group \
      rendering its members joined with ',' inside braces including empty members, and a range \
      rendering \"[low-high]\" WITHOUT the negate flag it stores",
     "crates/tsr_glob (absent). crates/tsr_tsoptions/src/glob.rs:21-31 declares the vfsmatch \
      Segment and Component enums, which have no alternation, no character range and no \
      separator variant, and neither enum implements Display"),
    ("glob.match",
     "tsc/internal/glob/glob.go:match",
     "a free recursive matcher over an element slice and an input, backtracking on Star within \
      one segment and on StarStar across segments, trying each Group alternative with the \
      remaining elements appended, and consuming a whole separator run per Slash element",
     "crates/tsr_glob (absent). No Rust function anywhere takes an element list and an input and \
      recurses over it in this dialect; crates/tsr_tsoptions/src/glob.rs:65 Pattern::matches_parts \
      walks vfsmatch path parts instead"),
    ("glob.parse",
     "tsc/internal/glob/glob.go:parse",
     "the internal parser behind the entry point, taking a grouping flag and returning the glob, \
      the unconsumed residual and an error: under the flag it stops at '}' or ',' and hands both \
      back, which is what makes a group parseable at all",
     "crates/tsr_glob (absent). No Rust function in either glob file takes a nested flag or \
      returns a residual"),
    ("glob.parseLiteral",
     "tsc/internal/glob/glob.go:Glob.parseLiteral",
     "a literal scanner that appends exactly one literal element -- possibly empty -- and returns \
      the unconsumed tail, with a special-character set that includes '}' and ',' only under the \
      grouping flag",
     "crates/tsr_glob (absent). crates/tsr_tsoptions/src/glob.rs:200 parse_component is the \
      nearest shape and is not a counterpart: it splits a whole vfsmatch component into segments \
      at '*' and '?', appends no empty literal and has no residual"),
    ("glob.readRangeRune",
     "tsc/internal/glob/glob.go:readRangeRune",
     "a range-bound decoder returning (code point, byte size, error) where the VALUE gates the \
      error and the size only then selects it (glob.go:140 `if r == utf8.RuneError`, :142-147): a \
      RuneError decode of size 0 is the bad-range error, of size 1 is the invalid-UTF-8 error, \
      and of any larger size is accepted -- that larger size is the properly encoded U+FFFD. A \
      decode that is NOT RuneError never errors whatever its size, so every ASCII bound returns \
      size 1 and no error; a port that switched on the size alone would reject `[a-z]`",
     "crates/tsr_glob (absent). crates/tsr_tsoptions/src/glob.rs does no rune-range decoding; \
      vfsmatch has no [x-y] construct"),
    ("glob.split",
     "tsc/internal/glob/glob.go:split",
     "a total splitter returning the bytes before the first separator and the bytes after the \
      last separator of that run, so a trailing run yields an empty remainder",
     "crates/tsr_glob (absent). crates/tsr_tsoptions/src/glob.rs:228 next_path_part_single and \
      :248 next_path_part_parts are the vfsmatch part walkers: they return an offset into a \
      prefix/suffix pair rather than two substrings, and they are reached by the other dialect's \
      matcher"),
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

// Missing entry points, qualified by the subject that owns them.
const MISSING_OPERATIONS: &[(&str, &str)] = &[
    ("glob.Glob", "tsc/internal/glob/glob.go:Glob.Match"),
    ("glob.Glob", "tsc/internal/glob/glob.go:Glob.String"),
    ("glob.Glob", "tsc/internal/glob/glob.go:Parse"),
    ("glob.element", "tsc/internal/glob/glob.go:charRange.String"),
    ("glob.match", "tsc/internal/glob/glob.go:match"),
    ("glob.parse", "tsc/internal/glob/glob.go:parse"),
    (
        "glob.parseLiteral",
        "tsc/internal/glob/glob.go:Glob.parseLiteral",
    ),
    (
        "glob.readRangeRune",
        "tsc/internal/glob/glob.go:readRangeRune",
    ),
    ("glob.split", "tsc/internal/glob/glob.go:split"),
];

pub fn observe(request: &Value) -> Option<Outcome> {
    let (_, authority, signature, home) = MISSING
        .iter()
        .find(|(name, ..)| *name == subject(request))?;
    if let Err(problem) = check(request) {
        return Some(Outcome::Failed(problem));
    }
    Some(crate::api::missing_for_subject(
        request,
        MISSING_OPERATIONS,
        authority,
        signature,
        home,
    ))
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
