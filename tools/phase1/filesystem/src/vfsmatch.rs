//! The `vfs/vfsmatch` group: the configuration matching dialect.
//!
//! Most of this group has a real production home, so it drives
//! `tsr_tsoptions::glob` directly and expects matches. The one subject that
//! does not -- the generated `Usage` stringer -- is recorded as a gap naming
//! the Go authority and the intended signature; nothing here renders a name.
//!
//! Only the public entry points are driven: `read_directory`, `SpecMatcher`
//! with `matches`/`match_index`, and `is_implicit_glob`. The pinned probe is
//! in-package and could reach `compileGlobPattern`, `matchFiles` and
//! `globPattern` directly; it deliberately does not, because the port exposes
//! no counterpart and the comparison would then be with the probe rather than
//! with the port.
//!
//! The host is an in-memory snapshot on both sides -- `vfstest.FromMap` in Go,
//! `tsr_vfs::MemoryBuilder` here, the pairing the F0 pilot already uses -- so
//! every case is host-independent. Both hosts sort directory entries by name
//! and both report symlinks, which is what the walk order depends on.
//!
//! A required action key that is absent is a harness failure, never a default,
//! and an op this module does not know is the same. Neither may become a row
//! the two sides could agree on without executing anything.
//!
//! Byte payloads travel as hex through the `_hex` ops, because a Go
//! observation cannot carry invalid UTF-8 through `encoding/json` and the
//! byte-domain cases exist precisely to pin those bytes. `read_directory`
//! results are rendered as text: both hosts are built from the request's own
//! ASCII paths, so the lossy conversion is exact there.

use std::panic::AssertUnwindSafe;

use serde_json::{json, Value};
use tsr_jsstring::JsString;
use tsr_tsoptions::glob::{self, SpecMatcher, Usage, UNLIMITED_DEPTH};
use tsr_vfs::{Error, MemoryBuilder, MemorySnapshot};

use crate::api::{action_op, actions, ordered, subject, Outcome};

/// The request encoding of `UnlimitedDepth`. `math.MaxInt` does not survive a
/// JSON round trip through every consumer and a missing key must fail rather
/// than default, so the depth key is always present and -1 selects the
/// unlimited walk. Every other negative value is a malformed request; 0 and
/// above are passed to the port verbatim, including 0, whose post-decrement
/// never reaches the `depth == 0` stop.
const UNLIMITED_DEPTH_SENTINEL: i64 = -1;

/// The one subject with no Rust home. The port declares `glob::Usage` with a
/// derived `Debug` whose output happens to spell the three trimmed names, but
/// nothing renders it as a production value, `Debug` is not the operation, and
/// no Rust enum can hold the out-of-range discriminant the pinned stringer
/// formats. Preparation records the gap; it renders no name here.
const USAGE_MISSING: (&str, &str, &str) = (
    "tsc/internal/vfs/vfsmatch/stringer_generated.go:Usage.String",
    "a Display impl or an as_str on tsr_tsoptions::glob::Usage rendering the trimmed \
     Files/Directories/Exclude for 0..=2 and Usage(n) for every other int8, including \
     negatives -- which needs a representation for an out-of-domain Usage that the Rust \
     enum does not have",
    "crates/tsr_tsoptions/src/glob.rs:8-13 (Usage derives Clone, Copy, Debug, PartialEq and \
     Eq and nothing else; a repo-wide search across crates/ for a Display or as_str on it, \
     and for the rendered form Usage(, finds nothing)",
);

pub fn observe(request: &Value) -> Option<Outcome> {
    if request.get("dialect").and_then(Value::as_str) != Some("vfsmatch") {
        return None;
    }
    let subject = subject(request);
    if subject == "vfsmatch.Usage" {
        let (authority, signature, home) = USAGE_MISSING;
        return Some(Outcome::missing(authority, signature, home));
    }
    if !matches!(
        subject,
        "vfsmatch.ReadDirectory" | "vfsmatch.SpecMatcher" | "vfsmatch.IsImplicitGlob"
    ) {
        return None;
    }
    Some(match replay(subject, actions(request)) {
        Ok(rows) => Outcome::Observed(ordered(rows)),
        Err(error) => Outcome::Failed(error),
    })
}

fn replay(subject: &str, actions: &[Value]) -> Result<Vec<Value>, String> {
    actions
        .iter()
        .map(|action| match (subject, action_op(action)) {
            ("vfsmatch.ReadDirectory", "read_directory") => read_directory(action),
            ("vfsmatch.SpecMatcher", "spec_matcher") => spec_matcher(action, false),
            ("vfsmatch.SpecMatcher", "spec_matcher_hex") => spec_matcher(action, true),
            ("vfsmatch.IsImplicitGlob", "is_implicit_glob") => is_implicit_glob(action, false),
            ("vfsmatch.IsImplicitGlob", "is_implicit_glob_hex") => is_implicit_glob(action, true),
            (_, op) => Err(format!("unsupported action: {op}")),
        })
        .collect()
}

fn required_str<'a>(action: &'a Value, key: &str) -> Result<&'a str, String> {
    action
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("action {} requires a string {key}", action_op(action)))
}

fn required_bool(action: &Value, key: &str) -> Result<bool, String> {
    action
        .get(key)
        .and_then(Value::as_bool)
        .ok_or_else(|| format!("action {} requires a boolean {key}", action_op(action)))
}

fn required_i64(action: &Value, key: &str) -> Result<i64, String> {
    action
        .get(key)
        .and_then(Value::as_i64)
        .ok_or_else(|| format!("action {} requires an integer {key}", action_op(action)))
}

fn required_list<'a>(action: &'a Value, key: &str) -> Result<Vec<&'a str>, String> {
    let items = action
        .get(key)
        .and_then(Value::as_array)
        .ok_or_else(|| format!("action {} requires a list {key}", action_op(action)))?;
    items
        .iter()
        .map(|item| {
            item.as_str()
                .ok_or_else(|| format!("action {} has a non-string in {key}", action_op(action)))
        })
        .collect()
}

fn from_hex(text: &str, what: &str) -> Result<Vec<u8>, String> {
    if !text.len().is_multiple_of(2) || !text.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(format!("{what} is not an even-length hex string"));
    }
    (0..text.len() / 2)
        .map(|index| {
            u8::from_str_radix(&text[index * 2..index * 2 + 2], 16)
                .map_err(|_| format!("{what} is not a hex string"))
        })
        .collect()
}

fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().fold(String::new(), |mut text, byte| {
        text.push_str(&format!("{byte:02x}"));
        text
    })
}

fn required_bytes(action: &Value, key: &str) -> Result<Vec<u8>, String> {
    from_hex(required_str(action, key)?, key)
}

fn required_byte_list(action: &Value, key: &str) -> Result<Vec<JsString>, String> {
    required_list(action, key)?
        .into_iter()
        .map(|item| Ok(JsString::from_bytes(from_hex(item, key)?)))
        .collect()
}

fn strings(action: &Value, key: &str) -> Result<Vec<JsString>, String> {
    Ok(required_list(action, key)?
        .into_iter()
        .map(|item| JsString::from_bytes(item.as_bytes().to_vec()))
        .collect())
}

/// See the sentinel's comment: -1 is the unlimited walk and every other
/// negative value is malformed.
fn depth(action: &Value) -> Result<isize, String> {
    let depth = required_i64(action, "depth")?;
    if depth == UNLIMITED_DEPTH_SENTINEL {
        return Ok(UNLIMITED_DEPTH);
    }
    isize::try_from(depth)
        .ok()
        .filter(|depth| *depth >= 0)
        .ok_or_else(|| format!("action {} has an out-of-domain depth", action_op(action)))
}

/// Only the three named discriminants are a legal matcher request. An
/// out-of-range Usage is reachable only through the Usage.String trace, which
/// names it `value` rather than `usage` and which this module never observes.
fn usage(action: &Value) -> Result<Usage, String> {
    match required_i64(action, "usage")? {
        0 => Ok(Usage::Files),
        1 => Ok(Usage::Directories),
        2 => Ok(Usage::Exclude),
        _ => Err(format!(
            "action {} has an out-of-domain usage",
            action_op(action)
        )),
    }
}

/// Runs `call` with the panic hook silenced, so a port panic is recorded as a
/// class instead of killing the capture and landing in the captured stderr.
/// Only the class is comparable: Go and Rust word the same failure
/// differently, so an unrecognized panic keeps its text under `other:`.
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
        // are folded onto the one class the Go probe folds its two wordings
        // onto.
        "index_out_of_range".to_owned()
    } else {
        // A panic the port raises itself keeps its text: next_part's
        // `expect("directory prefix ends in slash")` is the port's own wording
        // where the pin reaches an unchecked slice instead, and collapsing the
        // two would assert an equivalence this module has not established.
        format!("other:{message}")
    }
}

fn error_class(error: &Error) -> &'static str {
    // The payload of Unsupported names a Rust-side message; only the shape is
    // recorded. The pinned ReadDirectory cannot fail at all, so any of these
    // is a genuine shape difference rather than a wording difference.
    match error {
        Error::Unsupported(_) => "unsupported",
        Error::OutsideScope => "outside_scope",
        Error::InvalidPath => "invalid_path",
        Error::SymlinkCycle => "symlink_cycle",
        Error::Io(_) => "io",
    }
}

fn host(action: &Value) -> Result<MemorySnapshot, String> {
    let current = required_str(action, "currentDirectory")?;
    let case_sensitive = required_bool(action, "useCaseSensitiveFileNames")?;
    let mut builder = MemoryBuilder::new(current.as_bytes(), case_sensitive);
    for file in required_list(action, "files")? {
        builder.insert_physical(file.as_bytes(), Vec::new());
    }
    for directory in required_list(action, "directories")? {
        builder.insert_directory(directory.as_bytes());
    }
    let links = action
        .get("symlinks")
        .and_then(Value::as_array)
        .ok_or_else(|| format!("action {} requires a list symlinks", action_op(action)))?;
    for link in links {
        let path = required_str(link, "path")?;
        let target = required_str(link, "target")?;
        builder.insert_symlink(path.as_bytes(), target.as_bytes());
    }
    Ok(builder.finish())
}

fn read_directory(action: &Value) -> Result<Value, String> {
    let snapshot = host(action)?;
    let current = required_str(action, "currentDirectory")?.to_owned();
    let path = required_str(action, "path")?.to_owned();
    let extensions = strings(action, "extensions")?;
    let excludes = strings(action, "excludes")?;
    let includes = strings(action, "includes")?;
    let depth = depth(action)?;

    let mut failure = None;
    let (result, panicked) = guarded(AssertUnwindSafe(|| {
        match glob::read_directory(
            &snapshot,
            current.as_bytes(),
            path.as_bytes(),
            &extensions,
            &excludes,
            &includes,
            depth,
        ) {
            Ok(matched) => Value::Array(
                matched
                    .iter()
                    .map(|name| {
                        Value::String(String::from_utf8_lossy(name.as_bytes()).into_owned())
                    })
                    .collect(),
            ),
            Err(error) => {
                failure = Some(error_class(&error));
                Value::Null
            }
        }
    }));
    let mut row = json!({ "op": "read_directory", "result": result, "panic": panicked });
    if let Some(class) = failure {
        // Go's ReadDirectory has no error arm, so this key is present on one
        // side only. That is the shape difference the matrix records, not a
        // rendering choice.
        row["error"] = Value::String(class.to_owned());
    }
    Ok(row)
}

fn spec_matcher(action: &Value, as_bytes: bool) -> Result<Value, String> {
    let (specs, base, paths) = if as_bytes {
        (
            required_byte_list(action, "specs_hex")?,
            required_bytes(action, "basePath_hex")?,
            required_byte_list(action, "paths_hex")?,
        )
    } else {
        (
            strings(action, "specs")?,
            required_str(action, "basePath")?.as_bytes().to_vec(),
            strings(action, "paths")?,
        )
    };
    let usage = usage(action)?;
    let case_sensitive = required_bool(action, "caseSensitive")?;
    let op = action_op(action).to_owned();

    let (row, panicked) = guarded(AssertUnwindSafe(|| {
        let matcher = SpecMatcher::new(&specs, &base, usage, case_sensitive);
        // A path triple is a list, not an object: canonicalisation sorts keys
        // and the pairing of matches with match_index is what these rows
        // witness. When the port answers None the trace records that and
        // queries nothing, exactly as the Go side declines to dereference a
        // nil *SpecMatcher.
        let queried: Vec<Value> = matcher.as_ref().map_or_else(Vec::new, |matcher| {
            paths
                .iter()
                .map(|path| {
                    let rendered = if as_bytes {
                        to_hex(path.as_bytes())
                    } else {
                        String::from_utf8_lossy(path.as_bytes()).into_owned()
                    };
                    json!([
                        rendered,
                        matcher.matches(path.as_bytes()),
                        matcher
                            .match_index(path.as_bytes())
                            .map_or(-1, |index| i64::try_from(index).unwrap_or(-1))
                    ])
                })
                .collect()
        });
        json!({ "matcher_present": matcher.is_some(), "paths": queried })
    }));
    if !panicked.is_empty() {
        return Ok(json!({ "op": op, "panic": panicked }));
    }
    Ok(json!({
        "op": op,
        "matcher_present": row["matcher_present"].clone(),
        "paths": row["paths"].clone(),
        "panic": "",
    }))
}

fn is_implicit_glob(action: &Value, as_bytes: bool) -> Result<Value, String> {
    let op = action_op(action).to_owned();
    let component = if as_bytes {
        required_bytes(action, "component_hex")?
    } else {
        required_str(action, "component")?.as_bytes().to_vec()
    };
    let (result, panicked) = guarded(AssertUnwindSafe(|| {
        Value::Bool(glob::is_implicit_glob(&component))
    }));
    let mut row = json!({ "op": op, "result": result, "panic": panicked });
    if as_bytes {
        row["component_hex"] = Value::String(to_hex(&component));
    } else {
        row["component"] = Value::String(String::from_utf8_lossy(&component).into_owned());
    }
    Ok(row)
}
