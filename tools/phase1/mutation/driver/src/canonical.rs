//! Canonical JSON with exactly the bytes of the Python evidence scripts'
//! `json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=True)`,
//! optionally through `s07_binder.comparable`.
//!
//! The driver's stage digests are defined over these bytes, so they compare
//! directly with digests the Python side computes from native Go frames.

use serde_json::Value;
use std::fmt;

/// A value the Python encoder would print differently: the protocols are
/// integer-only, so a float in an observation is an observer defect.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NonInteger;

impl fmt::Display for NonInteger {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("non-integer number in a canonical value")
    }
}

/// Appends the canonical encoding of `value`. With `comparable`, an object whose
/// keys are exactly `raw_hex` and `identity` and whose identity is not null is
/// written as `{"identity": ...}` (`scripts/s07_binder.py` `comparable`).
pub fn write(out: &mut Vec<u8>, value: &Value, comparable: bool) -> Result<(), NonInteger> {
    match value {
        Value::Null => out.extend_from_slice(b"null"),
        Value::Bool(true) => out.extend_from_slice(b"true"),
        Value::Bool(false) => out.extend_from_slice(b"false"),
        Value::Number(number) => {
            if let Some(integer) = number.as_i64() {
                out.extend_from_slice(integer.to_string().as_bytes());
            } else if let Some(integer) = number.as_u64() {
                out.extend_from_slice(integer.to_string().as_bytes());
            } else {
                return Err(NonInteger);
            }
        }
        Value::String(text) => string(out, text),
        Value::Array(items) => {
            out.push(b'[');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push(b',');
                }
                write(out, item, comparable)?;
            }
            out.push(b']');
        }
        Value::Object(map) => {
            if comparable && map.len() == 2 && map.contains_key("raw_hex") {
                if let Some(identity) = map.get("identity").filter(|identity| !identity.is_null()) {
                    out.extend_from_slice(b"{\"identity\":");
                    write(out, identity, comparable)?;
                    out.push(b'}');
                    return Ok(());
                }
            }
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort_unstable();
            out.push(b'{');
            for (index, key) in keys.into_iter().enumerate() {
                if index > 0 {
                    out.push(b',');
                }
                string(out, key);
                out.push(b':');
                write(out, &map[key], comparable)?;
            }
            out.push(b'}');
        }
    }
    Ok(())
}

/// `scripts/s07_subset.py` `json_bytes` without its trailing newline: keys
/// sorted, except that the maps under a `paths` or `config_raw` key (and every
/// map below them) keep their document order, because compiler option paths
/// and raw configs are ordered source maps. The caller must parse with
/// serde_json's `preserve_order` for that order to survive (the syntax harness
/// does). The driver itself authenticates its requests with `write`.
#[allow(dead_code)] // used by the phase1_syntax mutation mode
pub fn write_json_bytes(
    out: &mut Vec<u8>,
    value: &Value,
    preserve: bool,
) -> Result<(), NonInteger> {
    match value {
        Value::Array(items) => {
            out.push(b'[');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push(b',');
                }
                write_json_bytes(out, item, preserve)?;
            }
            out.push(b']');
        }
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            if !preserve {
                keys.sort_unstable();
            }
            out.push(b'{');
            for (index, key) in keys.into_iter().enumerate() {
                if index > 0 {
                    out.push(b',');
                }
                string(out, key);
                out.push(b':');
                let ordered = preserve || key == "paths" || key == "config_raw";
                write_json_bytes(out, &map[key], ordered)?;
            }
            out.push(b'}');
        }
        scalar => write(out, scalar, false)?,
    }
    Ok(())
}

/// `{"kind": kind, "value": value}` canonically, without building the object:
/// the per-observation bytes of the E1 digest rule.
pub fn write_observation(out: &mut Vec<u8>, kind: &str, value: &Value) -> Result<(), NonInteger> {
    out.extend_from_slice(b"{\"kind\":");
    string(out, kind);
    out.extend_from_slice(b",\"value\":");
    write(out, value, false)?;
    out.push(b'}');
    Ok(())
}

/// `[kind, comparable(value)]` canonically: the per-record bytes of the binder
/// digest rule (the S07 graph validator's `[kind, value]` record shape).
pub fn write_graph_record(out: &mut Vec<u8>, kind: &str, value: &Value) -> Result<(), NonInteger> {
    out.push(b'[');
    string(out, kind);
    out.push(b',');
    write(out, value, true)?;
    out.push(b']');
    Ok(())
}

/// Python's `ensure_ascii` string escaping: short escapes for the six JSON
/// controls, lowercase `\uXXXX` (surrogate pairs above the BMP) for every other
/// character outside printable ASCII, including DEL.
pub fn string(out: &mut Vec<u8>, text: &str) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let unit = |out: &mut Vec<u8>, code: u16| {
        out.extend_from_slice(b"\\u");
        for shift in [12, 8, 4, 0] {
            out.push(HEX[usize::from((code >> shift) & 15)]);
        }
    };
    out.push(b'"');
    for character in text.chars() {
        match character {
            '"' => out.extend_from_slice(b"\\\""),
            '\\' => out.extend_from_slice(b"\\\\"),
            '\n' => out.extend_from_slice(b"\\n"),
            '\r' => out.extend_from_slice(b"\\r"),
            '\t' => out.extend_from_slice(b"\\t"),
            '\u{8}' => out.extend_from_slice(b"\\b"),
            '\u{c}' => out.extend_from_slice(b"\\f"),
            ' '..='~' => out.push(character as u8),
            _ => {
                let mut units = [0_u16; 2];
                for code in character.encode_utf16(&mut units) {
                    unit(out, *code);
                }
            }
        }
    }
    out.push(b'"');
}

#[cfg(test)]
mod tests {
    use super::{string, write, write_graph_record, write_observation, NonInteger};
    use serde_json::json;

    fn text(value: &serde_json::Value, comparable: bool) -> String {
        let mut out = Vec::new();
        write(&mut out, value, comparable).unwrap();
        String::from_utf8(out).unwrap()
    }

    #[test]
    fn matches_python_sorted_compact_ascii_dumps() {
        // Expected strings are the output of CPython 3 json.dumps(value,
        // sort_keys=True, separators=(",", ":"), ensure_ascii=True).
        let value = json!({"b": [1, -2, true, null], "a": {"z": "q\"\\\n\r\t\u{8}\u{c}\u{1}\u{7f}\u{e9}\u{1f600}", "y": u64::MAX}});
        assert_eq!(
            text(&value, false),
            r#"{"a":{"y":18446744073709551615,"z":"q\"\\\n\r\t\b\f\u0001\u007f\u00e9\ud83d\ude00"},"b":[1,-2,true,null]}"#
        );
        let mut out = Vec::new();
        string(&mut out, "\u{e9}~ ");
        assert_eq!(out, br#""\u00e9~ ""#);
        assert_eq!(
            write(&mut Vec::new(), &json!({"x": 1.5}), false),
            Err(NonInteger)
        );
    }

    #[test]
    fn json_bytes_keeps_the_order_of_option_paths_only() {
        let value: serde_json::Value = serde_json::from_str(
            r#"{"z":1,"options":{"paths":{"b/*":["y","x"],"a/*":[{"d":1,"c":2}]},"b":true,"a":null}}"#,
        )
        .unwrap();
        let mut out = Vec::new();
        super::write_json_bytes(&mut out, &value, false).unwrap();
        let document_order = value["options"]["paths"]
            .as_object()
            .unwrap()
            .keys()
            .next()
            .map(String::as_str)
            == Some("b/*");
        let expected = if document_order {
            // serde_json keeps document order (preserve_order): the paths map
            // and every map below it keep it, like s07_subset.json_bytes.
            r#"{"options":{"a":null,"b":true,"paths":{"b/*":["y","x"],"a/*":[{"d":1,"c":2}]}},"z":1}"#
        } else {
            // Without preserve_order the parsed maps are already sorted.
            r#"{"options":{"a":null,"b":true,"paths":{"a/*":[{"c":2,"d":1}],"b/*":["y","x"]}},"z":1}"#
        };
        assert_eq!(String::from_utf8(out).unwrap(), expected);
    }

    fn named_again() -> serde_json::Value {
        json!({"raw_hex": "fe23", "identity": {"kind": "symbol"}})
    }

    #[test]
    fn comparable_replaces_only_identity_derived_names() {
        let named = json!({"raw_hex": "fe", "identity": {"kind": "node", "ref": 3, "prefix_hex": "", "suffix_hex": ""}});
        let plain = json!({"raw_hex": "fe", "identity": null});
        let value = json!({"names": [named, plain], "raw_hex": "00", "identity": 1, "other": 2});
        assert_eq!(
            text(&value, true),
            r#"{"identity":1,"names":[{"identity":{"kind":"node","prefix_hex":"","ref":3,"suffix_hex":""}},{"identity":null,"raw_hex":"fe"}],"other":2,"raw_hex":"00"}"#
        );
        let mut out = Vec::new();
        write_observation(&mut out, "node", &json!({"b": 1, "a": 2})).unwrap();
        assert_eq!(out, br#"{"kind":"node","value":{"a":2,"b":1}}"#);
        out.clear();
        write_graph_record(&mut out, "symbol", &json!({"name": named_again()})).unwrap();
        assert_eq!(
            out,
            br#"["symbol",{"name":{"identity":{"kind":"symbol"}}}]"#
        );
    }
}
