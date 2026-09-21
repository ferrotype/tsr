//! The diagnostics leaf group.
//!
//! All formatting and locale actions call the shared production leaf APIs.
//!
//! Byte payloads travel as lowercase hex, matching the Go probe: message text
//! is compared as raw bytes, and a JSON string would hide a difference that
//! only shows up in the encoding.

use serde_json::{json, Map, Value};

use crate::api::{self, Outcome};
use tsr_diagnostics::Message;

mod replay;
pub(super) use replay::panic_text;
pub fn observe(request: &Value) -> Option<Outcome> {
    match api::subject(request) {
        "diagnostics.roster" => Some(roster(request)),
        "diagnostics.identity" => Some(identity(request)),
        subject if subject.starts_with("diagnostics.") => Some(replay::observe(request)),
        _ => None,
    }
}

/// FNV-1a/64 over a record stream, rendered as 16 lowercase hex digits.
///
/// A parity fingerprint, not a cryptographic digest: it exists so comparing
/// 2,211 identities does not have to ship 2,211 rows per side. The Go probe
/// implements the same four lines over the same record stream, so the two agree
/// by construction rather than by a shared library.
fn fingerprint(data: &[u8]) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in data {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100_0000_01b3);
    }
    format!("{hash:016x}")
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: [char; 16] = [
        '0', '1', '2', '3', '4', '5', '6', '7', '8', '9', 'a', 'b', 'c', 'd', 'e', 'f',
    ];
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(DIGITS[usize::from(byte >> 4)]);
        out.push(DIGITS[usize::from(byte & 0x0f)]);
    }
    out
}

fn flag(value: bool) -> char {
    if value {
        '1'
    } else {
        '0'
    }
}

/// One identity as a record in the fingerprinted stream. U+001F separates the
/// fields and U+001E ends the record, so neither can be produced by a key, a
/// decimal number or a flag triple, and a shifted field cannot look like a
/// different identity that happens to agree.
fn identity_record(key: &str, message: Option<&'static Message>) -> String {
    let Some(message) = message else {
        return format!("{key}\u{1f}unresolved\u{1e}");
    };
    format!(
        "{key}\u{1f}{code}\u{1f}{category}\u{1f}{unnecessary}{deprecated}{elided}\u{1f}{text}\u{1e}",
        code = message.code,
        category = message.category as i32,
        unnecessary = flag(message.reports_unnecessary),
        deprecated = flag(message.reports_deprecated),
        elided = flag(message.elided_in_compatibility_pyramid),
        text = message.text,
    )
}

/// Fingerprint every identity the request names. The roster is the request's
/// input -- which keys to look up -- and every value in the record stream is
/// read back out of the production table.
fn roster(request: &Value) -> Outcome {
    let Some(keys) = request.get("roster").and_then(Value::as_array) else {
        return Outcome::Failed("a roster request needs a `roster` array".to_owned());
    };
    let mut records: Vec<String> = Vec::with_capacity(keys.len());
    let mut resolved: usize = 0;
    for key in keys {
        let Some(key) = key.as_str() else {
            return Outcome::Failed("a roster entry is not a string".to_owned());
        };
        let message = tsr_diagnostics::by_key(key);
        if message.is_some() {
            resolved += 1;
        }
        records.push(identity_record(key, message));
    }

    let count = request
        .get("chunks")
        .and_then(Value::as_u64)
        .and_then(|value| usize::try_from(value).ok())
        .unwrap_or(0);
    let mut chunks: Vec<Value> = Vec::new();
    for chunk in 0..count {
        let start = chunk * records.len() / count;
        let end = (chunk + 1) * records.len() / count;
        chunks.push(Value::String(fingerprint(
            records[start..end].concat().as_bytes(),
        )));
    }

    Outcome::Observed(json!({
        "roster_size": keys.len(),
        "resolved": resolved,
        "unresolved": keys.len() - resolved,
        "fingerprint": fingerprint(records.concat().as_bytes()),
        "chunks": chunks,
    }))
}

/// The lookup row both sides emit.
///
/// `text_hex` is the message text, recorded once. The Go probe produces it
/// through `Message.String`, the pinned accessor for the unexported field,
/// which returns it verbatim (`diagnostics.go`: `return m.text`); this side
/// reads the public `text` field, because the crate exposes the bytes as data
/// and has no `Display` or `to_string` to call. That correspondence -- a Go
/// method against a Rust field -- is what the byte-exact-text case witnesses,
/// and it is the same correspondence `code`, `category`, `key_returned` and
/// the three flags already stand on. There is deliberately no second
/// `string_hex` field: it would be `text_hex` again on both sides, equal by
/// construction for every message, so it could not witness a difference.
fn lookup_row(row: &mut Map<String, Value>, message: Option<&'static Message>) {
    row.insert("found".to_owned(), Value::Bool(message.is_some()));
    let Some(message) = message else {
        return;
    };
    row.insert("code".to_owned(), json!(message.code));
    row.insert("category".to_owned(), json!(message.category as i32));
    row.insert(
        "reports_unnecessary".to_owned(),
        Value::Bool(message.reports_unnecessary),
    );
    row.insert(
        "reports_deprecated".to_owned(),
        Value::Bool(message.reports_deprecated),
    );
    row.insert(
        "elided_in_compatibility_pyramid".to_owned(),
        Value::Bool(message.elided_in_compatibility_pyramid),
    );
    row.insert(
        "key_returned".to_owned(),
        Value::String(message.key.to_owned()),
    );
    row.insert(
        "text_hex".to_owned(),
        Value::String(hex(message.text.as_bytes())),
    );
}

fn identity(request: &Value) -> Outcome {
    let mut rows: Vec<Value> = Vec::new();
    for action in api::actions(request) {
        let op = api::action_op(action);
        let mut row = Map::new();
        row.insert("op".to_owned(), Value::String(op.to_owned()));
        if op == "lookup" {
            let key = api::action_str(action, "key");
            row.insert("key".to_owned(), Value::String(key.to_owned()));
            lookup_row(&mut row, tsr_diagnostics::by_key(key));
        } else {
            row.insert(
                "unsupported_action".to_owned(),
                Value::String(op.to_owned()),
            );
        }
        rows.push(Value::Object(row));
    }
    Outcome::Observed(api::ordered(rows))
}

#[cfg(test)]
mod tests {
    use super::{fingerprint, flag, hex, identity_record};

    #[test]
    fn fingerprint_matches_the_probe_definition() {
        // FNV-1a/64 of the ASCII bytes "abc", the value the Go probe's four
        // lines produce for the same input.
        assert_eq!(fingerprint(b"abc"), "e71fa2190541574b");
        assert_eq!(fingerprint(b""), "cbf29ce484222325");
    }

    #[test]
    fn hex_is_lowercase_and_two_digits_per_byte() {
        assert_eq!(hex(&[0x00, 0x0f, 0xff, 0x80]), "000fff80");
        assert_eq!(hex(b""), "");
    }

    #[test]
    fn an_unresolved_key_records_itself_rather_than_a_neighbour() {
        assert_eq!(
            identity_record("no_such_key", None),
            "no_such_key\u{1f}unresolved\u{1e}"
        );
        assert_eq!(flag(true), '1');
        assert_eq!(flag(false), '0');
    }

    #[test]
    fn a_resolved_key_records_code_category_flags_and_text() {
        let message = tsr_diagnostics::by_key("Identifier_expected_1003");
        assert!(message.is_some());
        assert_eq!(
            identity_record("Identifier_expected_1003", message),
            "Identifier_expected_1003\u{1f}1003\u{1f}1\u{1f}000\u{1f}Identifier expected.\u{1e}"
        );
    }
}
