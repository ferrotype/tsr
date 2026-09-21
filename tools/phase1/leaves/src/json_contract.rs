//! The `internal/json` leaf group.
//!
//! Every subject here is a wrapper entry point, not a raw library call: the
//! pinned `tsc/internal/json/json.go` prepends `jsontext.AllowInvalidUTF8(true)`
//! to each of the five marshal functions and to nothing on the read side, and
//! that asymmetry is what the frozen cases witness. A port therefore has to
//! reproduce the wrapper's defaults, not the defaults of whatever JSON library
//! it is built on.
//!
//! There is no `tsr_json` crate. The closest existing Rust code is
//! `crates/tsr_tsoptions/src/config_json.rs::stringify_json`, a compact
//! marshaller for `ConfigValue` written against the
//! `core.StringifyJson` -> `json.MarshalIndent("", "")` boundary; it covers one
//! value type, one option set and the write side only. It is also not reachable
//! from here, because `phase1_leaves` does not depend on `tsr_tsoptions`. So
//! every case in this group is a recorded gap, and the records below name the
//! pinned authority, the signature the port is expected to carry and the file
//! that does not exist yet.
//!
//! The table is keyed by the request's *operation* id, not by its subject.
//! Four cases drive a wrapper entry point in order to witness one of the three
//! option constructors -- `Deterministic`, `AllowDuplicateNames`, `WithIndent`
//! -- and their operation is that constructor. Keying by subject would file
//! those gaps under `Marshal` or `Unmarshal` and hand the reader a signature
//! for a function the case is not about.
//!
//! Preparation records the gap. It never emulates the wrapper to make a
//! comparison run, and it never reads an expected result.

use crate::api::Outcome;
use serde_json::Value;

const MISSING: &[(&str, &str, &str)] = &[
    ("tsc/internal/json/json.go:Marshal",
     "pub fn marshal(value: &JsonInput, options: &MarshalOptions) -> Result<Vec<u8>, MarshalError> defaulting to AllowInvalidUTF8(true) and to nothing else: duplicate member names are rejected, pre-escaped source escapes are not preserved, '<', '&', U+2028 and U+2029 are emitted literally, raw number tokens pass through uncanonicalised, a non-finite f64 is an error rather than a token, and the output is not terminated with a newline",
     "crates/tsr_core/src/json/marshal.rs (absent; crates/tsr_tsoptions/src/config_json.rs covers ConfigValue only)"),
    ("tsc/internal/json/json.go:MarshalIndent",
     "pub fn marshal_indent(value: &JsonInput, prefix: &str, indent: &str) -> Result<Vec<u8>, MarshalError> carrying the wrapper's documented fork: an empty prefix AND an empty indent skip the indent options and produce compact output, while any other pair implies multiline with the prefix repeated on every line after the first and the indent once per depth",
     "crates/tsr_core/src/json/marshal.rs (absent; crates/tsr_tsoptions/src/config_json.rs implements only the compact arm, for ConfigValue)"),
    ("tsc/internal/json/json.go:MarshalWrite",
     "pub fn marshal_write(out: &mut dyn std::io::Write, value: &JsonInput, options: &MarshalOptions) -> Result<(), MarshalError> writing the same bytes as marshal and appending successive values to the writer with no separator and no trailing newline",
     "crates/tsr_core/src/json/marshal.rs (absent)"),
    ("tsc/internal/json/json.go:MarshalIndentWrite",
     "pub fn marshal_indent_write(out: &mut dyn std::io::Write, value: &JsonInput, prefix: &str, indent: &str) -> Result<(), MarshalError> sharing marshal_indent's empty-prefix-and-indent fork",
     "crates/tsr_core/src/json/marshal.rs (absent)"),
    ("tsc/internal/json/json.go:MarshalEncode",
     "pub fn marshal_encode(encoder: &mut JsonEncoder, value: &JsonInput, options: &MarshalOptions) -> Result<(), MarshalError> joining the wrapper's options onto an encoder that already has its own, terminating every completed top-level value with a newline because the caller's encoder does not carry the omit-top-level-newline behavior marshal and marshal_write keep to themselves, and refusing with a 'cannot change UTF-8 checks' error when the join would flip AllowInvalidUTF8 while the encoder is expecting an object name",
     "crates/tsr_core/src/json/encoder.rs (absent)"),
    ("tsc/internal/json/json.go:Unmarshal",
     "pub fn unmarshal<T: FromJson>(input: &[u8], options: &UnmarshalOptions) -> Result<T, UnmarshalError> with the library defaults the wrapper leaves untouched: invalid UTF-8 and duplicate names are both rejected, duplicates compare unescaped names, trailing content after the top-level value is an error while trailing whitespace is not, an unknown member is ignored, a named null zeroes exactly the field it names, and the destination keeps whatever was written before a failing read",
     "crates/tsr_core/src/json/unmarshal.rs (absent)"),
    ("tsc/internal/json/json.go:UnmarshalRead",
     "pub fn unmarshal_read<T: FromJson>(input: &mut dyn std::io::Read, options: &UnmarshalOptions) -> Result<T, UnmarshalError> giving a result independent of where the reader splits its chunks, consuming trailing whitespace and rejecting a second top-level value",
     "crates/tsr_core/src/json/unmarshal.rs (absent)"),
    ("tsc/internal/json/json.go:NewDecoder",
     "pub fn new_decoder(input: &mut dyn std::io::Read) -> JsonDecoder with peek_kind/read_token/read_value/skip_value/input_offset/stack_depth/stack_pointer/unread_buffer, where read_value returns the raw still-escaped member-name bytes that lsproto's jsonObjectRawField compares against",
     "crates/tsr_core/src/json/decoder.rs (absent)"),
    ("tsc/internal/json/json.go:UnmarshalDecode",
     "pub fn unmarshal_decode<T: FromJson>(decoder: &mut JsonDecoder, options: &UnmarshalOptions) -> Result<T, UnmarshalError> consuming exactly one value, leaving the decoder positioned on the next with no separator required, and reporting EOF rather than a syntax error once the input is exhausted",
     "crates/tsr_core/src/json/decoder.rs (absent)"),
    // The three option constructors. A case that drives Marshal or Unmarshal
    // in order to witness one of them is filed here, under the operation it is
    // about.
    ("tsc/internal/json/json.go:Deterministic",
     "pub fn deterministic(enabled: bool) -> MarshalOption sorting map keys by their UTF-8 bytes, which is Go's `slices.Sort` over string keys and not RFC 8785's UTF-16 order, and affecting marshalling only",
     "crates/tsr_core/src/json/options.rs (absent)"),
    ("tsc/internal/json/json.go:AllowDuplicateNames",
     "pub fn allow_duplicate_names(allow: bool) -> JsonOption reaching both sides, appended after the wrapper's own options so a caller can turn the check off, where a duplicate is decided on unescaped names",
     "crates/tsr_core/src/json/options.rs (absent)"),
    ("tsc/internal/json/json.go:WithIndent",
     "pub fn with_indent(indent: &str) -> EncodeOption implying multiline output with no indent prefix, so marshal with it must produce exactly what marshal_indent(\"\", indent) produces",
     "crates/tsr_core/src/json/options.rs (absent)"),
];

pub fn observe(request: &Value) -> Option<Outcome> {
    let operation = request
        .get("operation")
        .and_then(Value::as_str)
        .unwrap_or_default();
    MISSING
        .iter()
        .find(|(id, _, _)| *id == operation)
        .map(|(id, signature, home)| Outcome::missing(*id, id, signature, home))
}
