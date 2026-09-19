//! JSON adapter for the S04 differential producer; not a compiler-facing API.
use serde::de::{value::MapAccessDeserializer, MapAccess, Visitor};
use serde::{Deserialize, Deserializer};
use serde_json::{json, Value};
use std::io::{self, Read};
use ts_jsstring::{escape, helpers, line_map, lsp, scanner_positions, wtf8};
use ts_jsstring::{
    JsString, LiteralEscapeFlags, LspLineMap, LspPosition, PositionEncoding, PositionMap,
    QuoteChar, SourceText,
};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Operation {
    Source,
    Slice,
    SourceSlice,
    Lower,
    Upper,
    LowerFirst,
    Truncate,
    Combine,
    Encode,
    DecodeJs,
    DecodeUtf8,
    Escape,
    ApiToUtf16,
    ApiToUtf8,
    ApiAscii,
    EcmaLines,
    LspLines,
    LspLineIndex,
    LspToPosition,
    LspFromPosition,
    ScannerToPosition,
    ScannerBytePosition,
    ScannerLine,
    ScannerEndLine,
    ScannerFromPosition,
    ByteFromPosition,
    Utf16Len,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Probe {
    id: String,
    group: String,
    criterion: String,
    op: Operation,
    text: String,
    a: i64,
    b: i64,
    flag: bool,
    #[serde(default)]
    #[allow(dead_code)] // Retained in the frozen wire schema; every payload is now compared.
    panic_message: bool,
}

// Derived struct deserialization also accepts positional arrays. The wire
// contract requires objects; delegation retains serde's duplicate-field checks.
struct ObjectProbe(Probe);

impl<'de> Deserialize<'de> for ObjectProbe {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct ObjectVisitor;
        impl<'de> Visitor<'de> for ObjectVisitor {
            type Value = ObjectProbe;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("a probe object")
            }

            fn visit_map<M: MapAccess<'de>>(self, map: M) -> Result<Self::Value, M::Error> {
                Probe::deserialize(MapAccessDeserializer::new(map)).map(ObjectProbe)
            }
        }
        deserializer.deserialize_map(ObjectVisitor)
    }
}

fn parse_probes(input: &str) -> Result<Vec<(Probe, Vec<u8>)>, String> {
    let probes: Vec<ObjectProbe> =
        serde_json::from_str(input).map_err(|error| error.to_string())?;
    if probes.is_empty() {
        return Err("expected one nonempty probe array".into());
    }
    probes
        .into_iter()
        .map(|ObjectProbe(probe)| {
            if probe.id.is_empty() || probe.group.is_empty() || probe.criterion.is_empty() {
                return Err("probe identity fields must be nonempty".into());
            }
            if matches!(probe.op, Operation::Escape)
                && (!matches!(probe.a, 34 | 39 | 96) || !(0..=3).contains(&probe.b))
            {
                return Err("invalid escape quote or flags".into());
            }
            if !probe.text.len().is_multiple_of(2)
                || !probe.text.bytes().all(|byte| byte.is_ascii_hexdigit())
            {
                return Err("text must be even-length hexadecimal".into());
            }
            let bytes = unhex(&probe.text);
            Ok((probe, bytes))
        })
        .collect()
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut result = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        write!(result, "{byte:02x}").unwrap();
    }
    result
}

fn unhex(text: &str) -> Vec<u8> {
    assert_eq!(text.len() % 2, 0);
    text.as_bytes()
        .chunks_exact(2)
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
        .collect()
}

fn evaluate(case: &Probe, bytes: &[u8]) -> Value {
    let a = case.a;
    let b = case.b;
    let flag = case.flag;
    let encoding = if flag {
        PositionEncoding::Utf8
    } else {
        PositionEncoding::Utf16
    };
    match case.op {
        Operation::Source => {
            let source = SourceText::from_bytes(bytes);
            json!([
                hex(source.as_bytes()),
                source.as_str().map(|text| hex(text.as_bytes()))
            ])
        }
        Operation::Slice => {
            if a < 0 || b < 0 {
                return Value::Null;
            }
            let source = JsString::from_bytes(bytes);
            source
                .slice(a as usize..b as usize)
                .map_or(Value::Null, |slice| {
                    json!([
                        hex(slice.as_bytes()),
                        slice.as_str().map(|text| hex(text.as_bytes()))
                    ])
                })
        }
        Operation::SourceSlice => {
            if a < 0 || b < 0 {
                return Value::Null;
            }
            let source = SourceText::from_bytes(bytes);
            source
                .slice(a as usize..b as usize)
                .map_or(Value::Null, |slice| {
                    json!([
                        hex(slice.as_bytes()),
                        slice.as_str().map(|text| hex(text.as_bytes()))
                    ])
                })
        }
        Operation::Lower => json!(hex(&helpers::to_lower_js(bytes))),
        Operation::Upper => json!(hex(&helpers::to_upper_js(bytes))),
        Operation::LowerFirst => json!(hex(&helpers::lower_first_char(bytes))),
        Operation::Truncate => json!(hex(helpers::truncate_by_runes(bytes, a as isize))),
        Operation::Combine => json!(hex(&wtf8::combine_surrogate_pairs(bytes))),
        Operation::Encode => json!(hex(&wtf8::encode_rune(a as i32))),
        Operation::DecodeJs => json!(wtf8::decode_rune(bytes)),
        Operation::DecodeUtf8 => json!(wtf8::decode_utf8(bytes)),
        Operation::Escape => {
            let quote = match a {
                39 => QuoteChar::Single,
                34 => QuoteChar::Double,
                96 => QuoteChar::Backtick,
                _ => panic!("invalid fixture quote"),
            };
            let flags = match b {
                0 => LiteralEscapeFlags::NONE,
                1 => LiteralEscapeFlags::NEVER_ASCII_ESCAPE,
                2 => LiteralEscapeFlags::JSX_ATTRIBUTE_ESCAPE,
                3 => {
                    LiteralEscapeFlags::NEVER_ASCII_ESCAPE
                        | LiteralEscapeFlags::JSX_ATTRIBUTE_ESCAPE
                }
                _ => panic!("invalid fixture flags"),
            };
            json!(hex(&escape::escape_string_with_flags(bytes, quote, flags)))
        }
        Operation::ApiToUtf16 => json!(PositionMap::new(bytes).utf8_to_utf16(a as isize)),
        Operation::ApiToUtf8 => json!(PositionMap::new(bytes).utf16_to_utf8(a as isize)),
        Operation::ApiAscii => json!(PositionMap::new(bytes).is_ascii_only()),
        Operation::EcmaLines => json!(line_map::compute_ecma_line_starts(bytes)),
        Operation::LspLines => {
            let map = LspLineMap::new(bytes);
            json!([map.line_starts, map.ascii_only])
        }
        Operation::LspLineIndex => {
            json!(LspLineMap::new(bytes).compute_index_of_line_start(a as i32))
        }
        Operation::LspToPosition => json!(lsp::lsp_line_and_character_to_position(
            bytes,
            &LspLineMap::new(bytes),
            LspPosition {
                line: a as u32,
                character: b as u32
            },
            encoding,
        )),
        Operation::LspFromPosition => {
            let position = lsp::lsp_position_to_line_and_character(
                bytes,
                &LspLineMap::new(bytes),
                a as i32,
                encoding,
            );
            json!([position.line, position.character])
        }
        Operation::ScannerToPosition => json!(
            scanner_positions::compute_position_of_line_and_utf16_character(
                &line_map::compute_ecma_line_starts(bytes),
                a as isize,
                b as isize,
                bytes,
                flag,
            )
        ),
        Operation::ScannerBytePosition => {
            json!(scanner_positions::compute_position_of_line_and_byte_offset(
                &line_map::compute_ecma_line_starts(bytes),
                a as isize,
                b as isize,
            ))
        }
        Operation::ScannerLine => json!(scanner_positions::compute_line_of_position(
            &line_map::compute_ecma_line_starts(bytes),
            a as isize
        )),
        Operation::ScannerEndLine => json!(scanner_positions::get_ecma_end_line_position(
            bytes, a as isize
        )),
        Operation::ScannerFromPosition => json!(
            scanner_positions::get_ecma_line_and_utf16_character_of_position(bytes, a as isize)
        ),
        Operation::ByteFromPosition => json!(line_map::position_to_line_and_byte_offset(
            a as isize,
            &line_map::compute_ecma_line_starts(bytes)
        )),
        Operation::Utf16Len => json!(line_map::utf16_len(bytes)),
    }
}

fn main() {
    let mut input = String::new();
    io::stdin().read_to_string(&mut input).unwrap();
    let validated = parse_probes(&input).unwrap_or_else(|error| {
        eprintln!("{error}");
        std::process::exit(1);
    });
    // Every panic payload is retained in JSON for classification and mismatch
    // diagnostics. Suppress only the redundant hook output and backtraces.
    std::panic::set_hook(Box::new(|_| {}));
    let results: Vec<Value> = validated
        .iter()
        .map(
            |(case, bytes)| match std::panic::catch_unwind(|| evaluate(case, bytes)) {
                Ok(value) => json!({"id":case.id, "panic":false, "value":value}),
                Err(payload) => {
                    let message = payload
                        .downcast_ref::<String>()
                        .map(String::as_str)
                        .or_else(|| payload.downcast_ref::<&str>().copied())
                        .unwrap_or("non-string Rust panic payload");
                    let value = json!(message);
                    json!({"id":case.id, "panic":true, "value":value})
                }
            },
        )
        .collect();
    serde_json::to_writer(io::stdout().lock(), &results).unwrap();
}

#[cfg(test)]
mod tests {
    use super::*;

    const VALID_PROBE: &str = r#"{"id":"fixture/1","group":"fixture","criterion":"utf16_positions","op":"scanner_to_position","text":"610a62","a":-1,"b":0,"flag":false,"panic_message":true}"#;

    #[test]
    fn malformed_requests_fail_before_behavioral_evaluation() {
        for (name, probe) in [
            (
                "unknown operation",
                VALID_PROBE.replace("scanner_to_position", "missing_operation"),
            ),
            (
                "unknown field",
                VALID_PROBE.replace(r#""flag":false"#, r#""flag":false,"unexpected":1"#),
            ),
            (
                "case alias",
                VALID_PROBE.replace(r#""flag":false"#, r#""flag":false,"Flag":true"#),
            ),
            ("missing field", VALID_PROBE.replace(r#""flag":false,"#, "")),
            (
                "null field",
                VALID_PROBE.replace(r#""flag":false"#, r#""flag":null"#),
            ),
            (
                "wrong type",
                VALID_PROBE.replace(r#""flag":false"#, r#""flag":0"#),
            ),
            (
                "null optional field",
                VALID_PROBE.replace(r#""panic_message":true"#, r#""panic_message":null"#),
            ),
            ("invalid hex", VALID_PROBE.replace("610a62", "61zz")),
            ("odd hex", VALID_PROBE.replace("610a62", "610")),
            (
                "overflow offset",
                VALID_PROBE.replace(r#""a":-1"#, r#""a":9223372036854775808"#),
            ),
            (
                "float offset",
                VALID_PROBE.replace(r#""a":-1"#, r#""a":1.0"#),
            ),
            (
                "invalid quote",
                VALID_PROBE.replace("scanner_to_position", "escape"),
            ),
            (
                "invalid flags",
                VALID_PROBE
                    .replace("scanner_to_position", "escape")
                    .replace(r#""a":-1"#, r#""a":34"#)
                    .replace(r#""b":0"#, r#""b":4"#),
            ),
        ] {
            assert!(
                parse_probes(&format!("[{probe}]")).is_err(),
                "accepted {name}"
            );
        }
        for (field, original) in [
            ("id", "fixture/1"),
            ("group", "fixture"),
            ("criterion", "utf16_positions"),
        ] {
            let empty = VALID_PROBE.replace(
                &format!(r#""{field}":"{original}""#),
                &format!(r#""{field}":"""#),
            );
            assert!(
                parse_probes(&format!("[{empty}]")).is_err(),
                "accepted empty {field}"
            );
        }
        for invalid in [
            "[]",
            "null",
            "{}",
            "[null]",
            "[[]]",
            r#"[["fixture/1","fixture","utf16_positions","scanner_to_position","610a62",-1,0,false,true]]"#,
        ] {
            assert!(parse_probes(invalid).is_err(), "accepted {invalid}");
        }
        assert!(parse_probes(&format!("[{VALID_PROBE}] []")).is_err());
    }

    #[test]
    fn duplicate_probe_keys_are_rejected_before_values_are_overwritten() {
        for duplicate in [
            r#""id":"other""#,
            r#""group":"other""#,
            r#""criterion":"other""#,
            r#""op":"lower""#,
            r#""text":"""#,
            r#""a":0"#,
            r#""b":1"#,
            r#""flag":true"#,
            r#""panic_message":false"#,
        ] {
            let repeated = format!("[{{{duplicate},{}]", &VALID_PROBE[1..]);
            let error = parse_probes(&repeated).unwrap_err();
            assert!(
                error.contains("duplicate field"),
                "wrong rejection for {duplicate}: {error}"
            );
        }
    }

    #[test]
    fn valid_contract_panics_remain_behavioral_results() {
        let probes = parse_probes(&format!("[{VALID_PROBE}]")).unwrap();
        let (probe, bytes) = &probes[0];
        assert!(probe.panic_message);
        let payload = std::panic::catch_unwind(|| evaluate(probe, bytes)).unwrap_err();
        assert_eq!(
            payload.downcast_ref::<String>().unwrap(),
            "Bad line number. Line: -1, lineStarts.length: 2."
        );
        let ordinary = VALID_PROBE.replace(r#","panic_message":true"#, "");
        assert!(
            !parse_probes(&format!("[{ordinary}]")).unwrap()[0]
                .0
                .panic_message
        );
    }
}
