//! Compact JSON at the pinned core.StringifyJson / json.Marshal boundary.
use crate::ConfigValue;
use std::collections::BTreeSet;
use ts_jsstring::wtf8::{decode_utf8, RUNE_ERROR};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JsonError {
    NonFiniteNumber,
    DuplicateName,
    NestingDepth,
}
impl std::fmt::Display for JsonError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for JsonError {}

/// No indentation, HTML escaping, or JavaScript escaping is selected. Typed nil
/// slices serialize as `[]` under the pinned json/v2 defaults; nil interfaces
/// serialize as `null`. Malformed UTF-8 is replaced one source rune at a time.
/// port: tsc/internal/core/core.go:StringifyJson
pub fn stringify_json(value: &ConfigValue) -> Result<Vec<u8>, JsonError> {
    let mut result = Vec::new();
    append_value(&mut result, value, 0)?;
    Ok(result)
}
fn append_string(result: &mut Vec<u8>, bytes: &[u8]) {
    result.push(b'"');
    let mut remaining = bytes;
    while !remaining.is_empty() {
        let (rune, width) = decode_utf8(remaining);
        match rune {
            8 => result.extend_from_slice(b"\\b"),
            9 => result.extend_from_slice(b"\\t"),
            10 => result.extend_from_slice(b"\\n"),
            12 => result.extend_from_slice(b"\\f"),
            13 => result.extend_from_slice(b"\\r"),
            34 => result.extend_from_slice(b"\\\""),
            92 => result.extend_from_slice(b"\\\\"),
            0..=31 => {
                const HEX: &[u8] = b"0123456789abcdef";
                result.extend_from_slice(b"\\u00");
                result.push(HEX[(rune / 16) as usize]);
                result.push(HEX[(rune % 16) as usize]);
            }
            RUNE_ERROR if width == 1 => result.extend_from_slice("�".as_bytes()),
            _ => result.extend_from_slice(&remaining[..width]),
        }
        remaining = &remaining[width..];
    }
    result.push(b'"');
}
fn append_value(result: &mut Vec<u8>, value: &ConfigValue, depth: usize) -> Result<(), JsonError> {
    stacker::maybe_grow(128 * 1024, 2 * 1024 * 1024, || {
        append_worker(result, value, depth)
    })
}
fn append_worker(result: &mut Vec<u8>, value: &ConfigValue, depth: usize) -> Result<(), JsonError> {
    match value {
        ConfigValue::Null => result.extend_from_slice(b"null"),
        ConfigValue::EmptyStruct => result.extend_from_slice(b"{}"),
        ConfigValue::Boolean(value) => {
            result.extend_from_slice(if *value { b"true" } else { b"false" });
        }
        ConfigValue::Integer(value) => result.extend_from_slice(value.to_string().as_bytes()),
        ConfigValue::Enum(value) => result.extend_from_slice(value.to_string().as_bytes()),
        ConfigValue::Number(value) => {
            if !value.is_finite() {
                return Err(JsonError::NonFiniteNumber);
            }
            if *value == 0.0 && value.is_sign_negative() {
                result.extend_from_slice(b"-0");
            } else {
                result.extend_from_slice(ts_jsnum::Number::new(*value).to_string().as_bytes());
            }
        }
        ConfigValue::String(value) => append_string(result, value.as_bytes()),
        ConfigValue::Array(values) => {
            if depth >= 10_000 {
                return Err(JsonError::NestingDepth);
            }
            result.push(b'[');
            for (index, value) in values.iter().flatten().enumerate() {
                if index > 0 {
                    result.push(b',');
                }
                append_value(result, value, depth + 1)?;
            }
            result.push(b']');
        }
        ConfigValue::Object(values) => {
            if depth >= 10_000 {
                return Err(JsonError::NestingDepth);
            }
            result.push(b'{');
            let mut names = BTreeSet::new();
            for (index, (name, value)) in values.iter().enumerate() {
                if index > 0 {
                    result.push(b',');
                }
                let start = result.len();
                append_string(result, name.as_bytes());
                if !names.insert(result[start..].to_vec()) {
                    return Err(JsonError::DuplicateName);
                }
                result.push(b':');
                append_value(result, value, depth + 1)?;
            }
            result.push(b'}');
        }
    }
    Ok(())
}
