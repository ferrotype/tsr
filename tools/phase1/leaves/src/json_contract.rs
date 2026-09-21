//! The internal/json leaf group. Typed strings, floats, maps and the declared
//! sample struct now call `tsr_json`. Raw JSON values, native partial-error
//! state, decoder operations and token-stream encoder operations remain gaps.
//! No expected output is read by this driver.

mod typed;
use crate::api::Outcome;
use serde_json::Value;

const MISSING: &[(&str, &str, &str)] = &[
    ("tsc/internal/json/json.go:Marshal",
     "pub fn marshal(value: &JsonInput, options: &MarshalOptions) -> Result<Vec<u8>, MarshalError> defaulting to AllowInvalidUTF8(true) and to nothing else: duplicate member names are rejected, pre-escaped source escapes are not preserved, '<', '&', U+2028 and U+2029 are emitted literally, raw number tokens pass through uncanonicalised, a non-finite f64 is an error rather than a token, and the output is not terminated with a newline",
     "crates/tsr_json/src/lib.rs (typed encoding implemented; raw JSON and partial-error state pending)"),
    ("tsc/internal/json/json.go:MarshalIndent",
     "pub fn marshal_indent(value: &JsonInput, prefix: &str, indent: &str) -> Result<Vec<u8>, MarshalError> carrying the wrapper's documented fork: an empty prefix AND an empty indent skip the indent options and produce compact output, while any other pair implies multiline with the prefix repeated on every line after the first and the indent once per depth",
     "crates/tsr_json/src/lib.rs (typed indentation implemented; raw JSON pending)"),
    ("tsc/internal/json/json.go:MarshalWrite",
     "pub fn marshal_write(out: &mut dyn std::io::Write, value: &JsonInput, options: &MarshalOptions) -> Result<(), MarshalError> writing the same bytes as marshal and appending successive values to the writer with no separator and no trailing newline",
     "crates/tsr_json/src/marshal.rs (absent)"),
    ("tsc/internal/json/json.go:MarshalIndentWrite",
     "pub fn marshal_indent_write(out: &mut dyn std::io::Write, value: &JsonInput, prefix: &str, indent: &str) -> Result<(), MarshalError> sharing marshal_indent's empty-prefix-and-indent fork",
     "crates/tsr_json/src/marshal.rs (absent)"),
    ("tsc/internal/json/json.go:MarshalEncode",
     "pub fn marshal_encode(encoder: &mut JsonEncoder, value: &JsonInput, options: &MarshalOptions) -> Result<(), MarshalError> joining the wrapper's options onto an encoder that already has its own, terminating every completed top-level value with a newline because the caller's encoder does not carry the omit-top-level-newline behavior marshal and marshal_write keep to themselves, and refusing with a 'cannot change UTF-8 checks' error when the join would flip AllowInvalidUTF8 while the encoder is expecting an object name",
     "crates/tsr_json/src/encoder.rs (absent)"),
    ("tsc/internal/json/json.go:Unmarshal",
     "pub fn unmarshal<T: FromJson>(input: &[u8], options: &UnmarshalOptions) -> Result<T, UnmarshalError> with the library defaults the wrapper leaves untouched: invalid UTF-8 and duplicate names are both rejected, duplicates compare unescaped names, trailing content after the top-level value is an error while trailing whitespace is not, an unknown member is ignored, a named null zeroes exactly the field it names, and the destination keeps whatever was written before a failing read",
     "crates/tsr_json/src/unmarshal.rs (absent)"),
    ("tsc/internal/json/json.go:UnmarshalRead",
     "pub fn unmarshal_read<T: FromJson>(input: &mut dyn std::io::Read, options: &UnmarshalOptions) -> Result<T, UnmarshalError> giving a result independent of where the reader splits its chunks, consuming trailing whitespace and rejecting a second top-level value",
     "crates/tsr_json/src/unmarshal.rs (absent)"),
    ("tsc/internal/json/json.go:NewDecoder",
     "pub fn new_decoder(input: &mut dyn std::io::Read) -> JsonDecoder with peek_kind/read_token/read_value/skip_value/input_offset/stack_depth/stack_pointer/unread_buffer, where read_value returns the raw still-escaped member-name bytes that lsproto's jsonObjectRawField compares against",
     "crates/tsr_json/src/decoder.rs (absent)"),
    ("tsc/internal/json/json.go:UnmarshalDecode",
     "pub fn unmarshal_decode<T: FromJson>(decoder: &mut JsonDecoder, options: &UnmarshalOptions) -> Result<T, UnmarshalError> consuming exactly one value, leaving the decoder positioned on the next with no separator required, and reporting EOF rather than a syntax error once the input is exhausted",
     "crates/tsr_json/src/decoder.rs (absent)"),
    // The three option constructors. A case that drives Marshal or Unmarshal
    // in order to witness one of them is filed here, under the operation it is
    // about.
    ("tsc/internal/json/json.go:Deterministic",
     "pub fn deterministic(enabled: bool) -> MarshalOption sorting map keys by their UTF-8 bytes, which is Go's `slices.Sort` over string keys and not RFC 8785's UTF-16 order, and affecting marshalling only",
     "crates/tsr_json/src/lib.rs (typed options implemented; raw/decoder paths pending)"),
    ("tsc/internal/json/json.go:AllowDuplicateNames",
     "pub fn allow_duplicate_names(allow: bool) -> JsonOption reaching both sides, appended after the wrapper's own options so a caller can turn the check off, where a duplicate is decided on unescaped names",
     "crates/tsr_json/src/lib.rs (typed options implemented; raw/decoder paths pending)"),
    ("tsc/internal/json/json.go:WithIndent",
     "pub fn with_indent(indent: &str) -> EncodeOption implying multiline output with no indent prefix, so marshal with it must produce exactly what marshal_indent(\"\", indent) produces",
     "crates/tsr_json/src/lib.rs (typed options implemented; raw/decoder paths pending)"),
];

pub fn observe(request: &Value) -> Option<Outcome> {
    if let Some(result) = typed::observe(request) {
        return Some(result);
    }
    let operation = request
        .get("operation")
        .and_then(Value::as_str)
        .unwrap_or_default();
    MISSING
        .iter()
        .find(|(id, _, _)| *id == operation)
        .map(|(id, signature, home)| Outcome::missing(*id, id, signature, home))
}
