//! The text/number/semver leaf group.
//!
//! Three subjects, one per native probe: `stringutil` for the comparer,
//! classifier, casing and conversion operations, `semver` for versions and
//! version ranges, `jsnum` for JavaScript numbers and bigint literals.
//!
//! Byte discipline. Every string payload travels as hex in both directions,
//! because serde_json cannot carry a malformed UTF-8 byte or a lone surrogate
//! and would otherwise repair exactly the inputs these cases exist to test.
//! That covers the bigint rows too: `parse_pseudo_big_int` passes its input
//! through verbatim on the default branch, so a malformed byte can reach the
//! result, and reporting it as a JSON string would let `from_utf8_lossy` here
//! and `encoding/json` on the native side repair it to U+FFFD and agree.
//! Every Number travels as its IEEE-754 bit pattern for the same reason, paired
//! with the rendering `Display` produced.
//!
//! An operation with no leaf-crate entry point is recorded in `MISSING`, keyed
//! by the request's operation id. Preparation names the gap; it never emulates
//! the algorithm here to make a comparison run, so a case whose trace would
//! need one missing call is a `MISSING` row in full rather than a trace with a
//! hand-written step in the middle.

use std::cmp::Ordering;

use serde_json::{json, Value};

use crate::api::{self, Outcome};

/// Operations this group covers that have no callable entry point in any crate
/// the leaf driver links. Each row names the pinned Go authority, the signature
/// the port is expected to carry and the file that does not provide it yet.
/// Where a private or out-of-reach equivalent already exists, the home says so
/// rather than claiming the behavior is unwritten.
const MISSING: &[(&str, &str, &str, &str)] = &[
    ("tsc/internal/stringutil/compare.go:CompareStringsCaseInsensitive",
     "tsc/internal/stringutil/compare.go:CompareStringsCaseInsensitive",
     "pub fn compare_case_insensitive(left: &[u8], right: &[u8]) -> Ordering decoding one rune at a time and comparing unicode simple lowercase, with compare_case_sensitive (raw byte order), compare_case_insensitive_then_sensitive, equate_case_sensitive and the two selector functions that return them",
     "crates/tsr_jsstring/src/compare.rs (no such file; tsr_jsstring exposes equal_fold and to_lower_go but no ordering comparer)"),
    ("tsc/internal/stringutil/compare.go:CompareStringsCaseInsensitiveEslintCompatible",
     "tsc/internal/stringutil/compare.go:CompareStringsCaseInsensitiveEslintCompatible",
     "pub fn compare_case_insensitive_eslint(left: &[u8], right: &[u8]) -> Ordering lowercasing both sides with Go strings.ToLower semantics, which repairs every malformed byte to U+FFFD, and then comparing the results as bytes",
     "crates/tsr_jsstring/src/compare.rs (no such file)"),
    ("tsc/internal/stringutil/compare.go:HasPrefix",
     "tsc/internal/stringutil/compare.go:HasPrefix",
     "pub fn has_prefix(text: &[u8], prefix: &[u8], case_sensitive: bool) -> bool with has_suffix and has_prefix_and_suffix_without_overlap, folding a byte window cut to the affix's byte length rather than to a rune boundary",
     "crates/tsr_jsstring/src/compare.rs (no such file)"),
    ("tsc/internal/stringutil/identifier.go:IsUnicodeIdentifierStart",
     "tsc/internal/stringutil/identifier.go:IsUnicodeIdentifierStart",
     "pub fn is_unicode_identifier_start(ch: i32) -> bool and is_unicode_identifier_part over the generated ES ID_Start / ID_Continue range tables",
     "crates/tsr_jsstring/src/identifier.rs (no such file; tsr_scanner::is_identifier_start is the scanner's wider set including $ and _, not these tables, and tsr_scanner is not a leaf dependency)"),
    ("tsc/internal/stringutil/util.go:IsWhiteSpaceLike",
     "tsc/internal/stringutil/util.go:IsWhiteSpaceLike",
     "pub fn is_white_space_like(ch: i32) -> bool with is_white_space_single_line, is_line_break, is_digit, is_octal_digit, is_hex_digit and is_ascii_letter",
     "crates/tsr_jsstring/src/helpers.rs (the file exists; none of these predicates do; equivalents live in crates/tsr_scanner/src/utilities.rs, three public and four pub(crate), and tsr_scanner is not a leaf dependency)"),
    ("tsc/internal/stringutil/util.go:EncodeURI",
     "tsc/internal/stringutil/util.go:EncodeURI",
     "pub fn encode_uri(text: &[u8]) -> Vec<u8> escaping per byte with uppercase hex, plus the should_escape_for_encode_uri predicate that keeps the ECMAScript unreserved set",
     "crates/tsr_jsstring/src/escape.rs (the file exists and carries the string-literal escapers; encode_uri is absent)"),
    ("tsc/internal/stringutil/util.go:RemoveByteOrderMark",
     "tsc/internal/stringutil/util.go:RemoveByteOrderMark",
     "pub fn remove_byte_order_mark(text: &[u8]) -> &[u8] and add_utf8_byte_order_mark, over a byte_order_mark_length probe that recognises the UTF-16 marks as well as the UTF-8 one",
     "crates/tsr_jsstring/src/source_text.rs (the file exists and carries SourceText; neither BOM operation is present)"),
    ("tsc/internal/stringutil/util.go:SplitLines",
     "tsc/internal/stringutil/util.go:SplitLines",
     "pub fn split_lines(text: &[u8]) -> Vec<&[u8]> treating CRLF as one break and emitting no trailing empty line, and guess_indentation(lines: &[&[u8]]) -> usize measuring leading whitespace in bytes",
     "crates/tsr_jsstring/src/line_map.rs (the file exists; neither function does; compute_ecma_line_starts answers a different question and includes U+2028)"),
    ("tsc/internal/stringutil/util.go:StripQuotes",
     "tsc/internal/stringutil/util.go:StripQuotes",
     "pub fn strip_quotes(name: &[u8]) -> &[u8] comparing the first and last runes, and unquote_string applying the `\\\\.` replacement whose dot excludes LF",
     "crates/tsr_jsstring/src/go_quote.rs (the file exists and carries go_quote only; a private byte-level unquote_name exists in crates/tsr_checker/src/node_builder_names.rs and no leaf crate can reach it)"),
    ("tsc/internal/jsnum/jsnum.go:Number.Floor",
     "tsc/internal/jsnum/jsnum.go:Number.Floor",
     "pub fn floor/abs/trunc/is_nan/is_inf on Number, the is_non_finite exponent-mask predicate, the NaN and Inf constructors, and a public shift_count",
     "crates/tsr_jsnum/src/arithmetic.rs (Number carries to_int32, to_uint32 and the operators; shift_count is private and the classifiers and rounding operations are absent)"),
    ("tsc/internal/jsnum/pseudobigint.go:ParseValidBigInt",
     "tsc/internal/jsnum/pseudobigint.go:ParseValidBigInt",
     "pub fn parse_valid_big_int(text: &[u8]) -> PseudoBigInt splitting the sign before the radix parse, and PseudoBigInt::sign(&self) -> i32",
     "crates/tsr_jsnum/src/pseudobigint.rs (PseudoBigInt has new and to_text only)"),
];

pub fn observe(request: &Value) -> Option<Outcome> {
    let subject = api::subject(request);
    if !matches!(subject, "stringutil" | "semver" | "jsnum") {
        return None;
    }
    let operation = request
        .get("operation")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if let Some((_, authority, signature, home)) = MISSING.iter().find(|row| row.0 == operation) {
        return Some(Outcome::missing(*authority, authority, signature, home));
    }
    let replay: fn(&Value) -> Result<Value, String> = match subject {
        "stringutil" => stringutil_row,
        "semver" => semver_row,
        _ => jsnum_row,
    };
    let mut rows = Vec::with_capacity(api::actions(request).len());
    for action in api::actions(request) {
        match replay(action) {
            Ok(row) => rows.push(row),
            Err(error) => return Some(Outcome::Failed(error)),
        }
    }
    Some(Outcome::Observed(api::ordered(rows)))
}

// ---------------------------------------------------------------------------
// payload decoding
// ---------------------------------------------------------------------------

/// Resolve one payload field. The hex form is authoritative; the readable form
/// exists so a reviewer can read the frozen request and is only used when no
/// hex is present. This matches the native probes' rule exactly.
fn payload(action: &Value, plain: &str, hexed: &str) -> Result<Vec<u8>, String> {
    let encoded = api::action_str(action, hexed);
    if encoded.is_empty() {
        return Ok(api::action_str(action, plain).as_bytes().to_vec());
    }
    decode_hex(encoded)
}

fn decode_hex(text: &str) -> Result<Vec<u8>, String> {
    if !text.is_ascii() || !text.len().is_multiple_of(2) {
        return Err(format!("malformed hex payload {text:?}"));
    }
    let mut out = Vec::with_capacity(text.len() / 2);
    let raw = text.as_bytes();
    for pair in raw.chunks_exact(2) {
        let digits = std::str::from_utf8(pair).map_err(|error| error.to_string())?;
        out.push(u8::from_str_radix(digits, 16).map_err(|error| error.to_string())?);
    }
    Ok(out)
}

fn encode_hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

fn rune(action: &Value, field: &str) -> Result<i32, String> {
    let value = api::action_i64(action, field);
    i32::try_from(value).map_err(|_| format!("rune {value} does not fit in an i32"))
}

fn flag(action: &Value, field: &str) -> bool {
    action.get(field).and_then(Value::as_bool).unwrap_or(false)
}

fn text_list<'a>(action: &'a Value, field: &str) -> &'a [Value] {
    action
        .get(field)
        .and_then(Value::as_array)
        .map_or(&[], Vec::as_slice)
}

fn row(op: &str, result: Value) -> Value {
    // Built by hand rather than with `json!`, so the result value is moved into
    // the row instead of being re-serialised from a borrow.
    let mut fields = serde_json::Map::new();
    fields.insert("op".to_owned(), Value::String(op.to_owned()));
    fields.insert("result".to_owned(), result);
    Value::Object(fields)
}

// ---------------------------------------------------------------------------
// stringutil
// ---------------------------------------------------------------------------

fn stringutil_row(action: &Value) -> Result<Value, String> {
    use tsr_jsstring::helpers::{lower_first_char, to_lower_js, to_upper_js, truncate_by_runes};
    use tsr_jsstring::wtf8;

    let op = api::action_op(action);
    let left = payload(action, "left", "left_hex")?;
    let result = match op {
        "equate_ci" => {
            let right = payload(action, "right", "right_hex")?;
            json!(tsr_jsstring::equal_fold(&left, &right))
        }
        "to_lower_js" => json!(encode_hex(&to_lower_js(&left))),
        "to_upper_js" => json!(encode_hex(&to_upper_js(&left))),
        "lower_first_char" => json!(encode_hex(&lower_first_char(&left))),
        "truncate_by_runes" => {
            let count = isize::try_from(api::action_i64(action, "count"))
                .map_err(|error| error.to_string())?;
            json!(encode_hex(truncate_by_runes(&left, count)))
        }
        "decode_js_rune" => {
            let (value, width) = wtf8::decode_rune(&left);
            json!([value, width])
        }
        "encode_js_rune" => json!(encode_hex(&wtf8::encode_rune(rune(action, "rune")?))),
        "is_surrogate" => json!(wtf8::is_surrogate(rune(action, "rune")?)),
        "is_high_surrogate" => json!(wtf8::is_high_surrogate(rune(action, "rune")?)),
        "is_low_surrogate" => json!(wtf8::is_low_surrogate(rune(action, "rune")?)),
        "surrogate_pair_to_code_point" => json!(wtf8::surrogate_pair_to_code_point(
            rune(action, "rune")?,
            rune(action, "rune2")?
        )),
        "code_point_to_surrogate_pair" => {
            let (high, low) = wtf8::code_point_to_surrogate_pair(rune(action, "rune")?);
            json!([high, low])
        }
        "combine_surrogate_pairs" => json!(encode_hex(&wtf8::combine_surrogate_pairs(&left))),
        _ => return Err(format!("the text group has no stringutil action {op:?}")),
    };
    Ok(row(op, result))
}

// ---------------------------------------------------------------------------
// semver
// ---------------------------------------------------------------------------

fn ordering(order: Ordering) -> i32 {
    match order {
        Ordering::Less => -1,
        Ordering::Equal => 0,
        Ordering::Greater => 1,
    }
}

fn parsed_version(text: &[u8]) -> Option<tsr_semver::Version> {
    let (version, error) = tsr_semver::try_parse_version(text);
    if error.is_some() {
        None
    } else {
        Some(version)
    }
}

/// Everything one parse produced, including the partial version retained beside
/// an overflow error. Mirrors the native probe's tuple exactly.
fn version_observation(text: &[u8]) -> Value {
    let (version, error) = tsr_semver::try_parse_version(text);
    json!([
        error.is_none(),
        error.map_or_else(String::new, |error| error.to_string()),
        version.major(),
        version.minor(),
        version.patch(),
        version.prerelease(),
        version.build(),
        version.to_string(),
    ])
}

fn range_observation(text: &[u8], versions: &[Value]) -> Value {
    let (range, ok) = tsr_semver::try_parse_version_range(text);
    let mut tests = Vec::with_capacity(versions.len());
    for candidate in versions {
        let candidate = candidate.as_str().unwrap_or_default().as_bytes();
        match parsed_version(candidate) {
            Some(version) => tests.push(json!(range.test(Some(&version)))),
            None => tests.push(json!("unparsed")),
        }
    }
    json!([ok, range.to_string(), tests])
}

fn semver_row(action: &Value) -> Result<Value, String> {
    let op = api::action_op(action);
    let text = payload(action, "text", "text_hex")?;
    let result = match op {
        "parse" => version_observation(&text),
        "must_parse_round_trip" => {
            // must_parse panics on a bad input, and this driver never panics, so
            // the same text is checked first. Only inputs the trace has already
            // observed as parseable reach this action.
            if parsed_version(&text).is_none() {
                return Err(format!(
                    "must_parse_round_trip was given the unparseable input {:?}",
                    String::from_utf8_lossy(&text)
                ));
            }
            json!(tsr_semver::Version::must_parse(&text).to_string())
        }
        "compare" => {
            let right = payload(action, "right", "right_hex")?;
            match (parsed_version(&text), parsed_version(&right)) {
                (Some(left), Some(right)) => json!([
                    ordering(tsr_semver::Version::compare(Some(&left), Some(&right))),
                    ordering(tsr_semver::Version::compare(Some(&right), Some(&left))),
                ]),
                _ => json!("unparsed"),
            }
        }
        "compare_nil_left" => {
            let right = payload(action, "right", "right_hex")?;
            match parsed_version(&right) {
                Some(right) => json!([
                    ordering(tsr_semver::Version::compare(None, Some(&right))),
                    ordering(tsr_semver::Version::compare(Some(&right), None)),
                ]),
                None => json!("unparsed"),
            }
        }
        "compare_nil_both" => json!(ordering(tsr_semver::Version::compare(None, None))),
        "range" => range_observation(&text, text_list(action, "versions")),
        "range_nil_version" => {
            let (range, ok) = tsr_semver::try_parse_version_range(&text);
            json!([ok, range.to_string(), range.test(None)])
        }
        _ => return Err(format!("the text group has no semver action {op:?}")),
    };
    Ok(row(op, result))
}

// ---------------------------------------------------------------------------
// jsnum
// ---------------------------------------------------------------------------

fn number(action: &Value, field: &str) -> Result<tsr_jsnum::Number, String> {
    let text = api::action_str(action, field);
    if text.len() != 16 {
        return Err(format!(
            "a Number payload needs 16 hex digits, {field} carried {text:?}"
        ));
    }
    let bits = u64::from_str_radix(text, 16).map_err(|error| error.to_string())?;
    Ok(tsr_jsnum::Number::new(f64::from_bits(bits)))
}

/// A Number reported as its bit pattern plus its rendering, so an infinity and
/// the sign of a zero survive and a subnormal is exact.
///
/// A NaN reports the literal `"nan"` instead of its bits, matching the native
/// probe: ECMAScript has one NaN value and nothing here can observe the
/// payload, but Go's `math.NaN()` is `7ff8000000000001` while Rust's `f64::NAN`
/// is `7ff8000000000000`. Freezing the payload would make an unobservable
/// runtime detail a permanent parity failure.
fn number_observation(value: tsr_jsnum::Number) -> Value {
    let raw = value.value();
    if raw.is_nan() {
        return json!(["nan", value.to_string()]);
    }
    json!([format!("{:016x}", raw.to_bits()), value.to_string()])
}

fn big_int_observation(value: &tsr_jsnum::PseudoBigInt) -> Value {
    json!([
        value.negative,
        encode_hex(&value.base10_value),
        encode_hex(&value.to_text()),
    ])
}

fn jsnum_row(action: &Value) -> Result<Value, String> {
    let op = api::action_op(action);
    let result = match op {
        "to_text" => json!(number(action, "bits")?.to_string()),
        "from_text" => {
            let text = payload(action, "text", "text_hex")?;
            number_observation(tsr_jsnum::from_string(&text))
        }
        "to_int32" => json!(number(action, "bits")?.to_int32()),
        "to_uint32" => json!(number(action, "bits")?.to_uint32()),
        "signed_right_shift" => {
            number_observation(number(action, "bits")?.signed_right_shift(number(action, "bits2")?))
        }
        "unsigned_right_shift" => number_observation(
            number(action, "bits")?.unsigned_right_shift(number(action, "bits2")?),
        ),
        "left_shift" => {
            number_observation(number(action, "bits")?.left_shift(number(action, "bits2")?))
        }
        "bitwise_not" => number_observation(number(action, "bits")?.bitwise_not()),
        "bitwise_or" => {
            number_observation(number(action, "bits")?.bitwise_or(number(action, "bits2")?))
        }
        "bitwise_and" => {
            number_observation(number(action, "bits")?.bitwise_and(number(action, "bits2")?))
        }
        "bitwise_xor" => {
            number_observation(number(action, "bits")?.bitwise_xor(number(action, "bits2")?))
        }
        "remainder" => {
            number_observation(number(action, "bits")?.remainder(number(action, "bits2")?))
        }
        "exponentiate" => {
            number_observation(number(action, "bits")?.exponentiate(number(action, "bits2")?))
        }
        "parse_pseudo_big_int" => {
            let text = payload(action, "text", "text_hex")?;
            json!(encode_hex(&tsr_jsnum::parse_pseudo_big_int(&text)))
        }
        "new_pseudo_big_int" => {
            let text = payload(action, "text", "text_hex")?;
            big_int_observation(&tsr_jsnum::PseudoBigInt::new(
                &text,
                flag(action, "enabled"),
            ))
        }
        _ => return Err(format!("the text group has no jsnum action {op:?}")),
    };
    Ok(row(op, result))
}
