//! Bounded Content-Length framing and strict JSON decoding for the test host.
//!
//! Valid framing follows pinned `internal/jsonrpc/baseproto.go`. The transport
//! contract deliberately rejects ambiguous or unbounded malformed input earlier.

use std::fmt;
use std::io::{self, BufRead, Write};

use serde::de::{self, DeserializeSeed, MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer};
use serde_json::value::RawValue;

pub const MAX_BODY: usize = 8 * 1024 * 1024;
pub const MAX_HEADER: usize = 8192;
/// Maximum number of nested JSON arrays and objects in a message.
pub const MAX_JSON_DEPTH: usize = 64;

fn invalid(message: impl Into<Box<dyn std::error::Error + Send + Sync>>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

fn header_line<R: BufRead>(reader: &mut R, total: &mut usize) -> io::Result<Option<Vec<u8>>> {
    let mut line = Vec::new();
    loop {
        if *total == MAX_HEADER {
            return Err(invalid("testhost header exceeds byte limit"));
        }
        let mut byte = [0];
        match reader.read_exact(&mut byte) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::UnexpectedEof && *total == 0 => {
                return Ok(None);
            }
            Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "truncated testhost header",
                ));
            }
            Err(error) => return Err(error),
        }
        *total += 1;
        if byte[0] == b'\n' {
            if line.pop() != Some(b'\r') {
                return Err(invalid("testhost header requires CRLF"));
            }
            return Ok(Some(line));
        }
        if line.last() == Some(&b'\r') {
            return Err(invalid("testhost header contains a bare carriage return"));
        }
        line.push(byte[0]);
    }
}

fn content_length(value: &[u8]) -> io::Result<usize> {
    let value = value.trim_ascii();
    if value.is_empty() || !value.iter().all(u8::is_ascii_digit) {
        return Err(invalid(
            "Content-Length must be a positive decimal byte count",
        ));
    }
    let mut length = 0_usize;
    for digit in value {
        length = length
            .checked_mul(10)
            .and_then(|length| length.checked_add(usize::from(digit - b'0')))
            .filter(|length| *length <= MAX_BODY)
            .ok_or_else(|| invalid("testhost body exceeds byte limit"))?;
    }
    if length == 0 {
        return Err(invalid("Content-Length must be positive"));
    }
    Ok(length)
}

/// Read exactly one payload, retaining subsequent frames in `reader`.
///
/// `None` means EOF before any header byte. EOF inside a header or body is an
/// error. After any error the connection must be discarded; framing recovery is
/// not part of this protocol.
pub fn read<R: BufRead>(reader: &mut R) -> io::Result<Option<Vec<u8>>> {
    let mut total = 0;
    let mut length = None;
    loop {
        let Some(line) = header_line(reader, &mut total)? else {
            return Ok(None);
        };
        if line.is_empty() {
            break;
        }
        let colon = line
            .iter()
            .position(|byte| *byte == b':')
            .ok_or_else(|| invalid("testhost header has no colon"))?;
        let (name, value) = (&line[..colon], &line[colon + 1..]);
        if name.is_empty()
            || !name
                .iter()
                .all(|byte| byte.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(byte))
            || !value
                .iter()
                .all(|byte| *byte == b'\t' || (b' '..=b'~').contains(byte))
        {
            return Err(invalid("malformed testhost header"));
        }
        if name == b"Content-Length" {
            if length.is_some() {
                return Err(invalid("duplicate Content-Length header"));
            }
            length = Some(content_length(value)?);
        }
    }
    let length = length.ok_or_else(|| invalid("missing Content-Length header"))?;
    let mut body = vec![0; length];
    reader.read_exact(&mut body).map_err(|error| {
        if error.kind() == io::ErrorKind::UnexpectedEof {
            io::Error::new(io::ErrorKind::UnexpectedEof, "truncated testhost body")
        } else {
            error
        }
    })?;
    Ok(Some(body))
}

/// Write a nonempty bounded payload and flush it as one Content-Length frame.
pub fn write<W: Write>(writer: &mut W, payload: &[u8]) -> io::Result<()> {
    if payload.is_empty() || payload.len() > MAX_BODY {
        return Err(invalid("testhost payload must contain 1..=MAX_BODY bytes"));
    }
    write!(writer, "Content-Length: {}\r\n\r\n", payload.len())?;
    writer.write_all(payload)?;
    writer.flush()
}

/// A complete frame whose JSON keys, strings and nesting have been validated.
/// Opaque numeric tokens remain in the borrowed input instead of becoming f64.
#[derive(Debug)]
pub struct Message<'a> {
    raw: &'a RawValue,
}
impl Message<'_> {
    pub fn as_str(&self) -> &str {
        self.raw.get()
    }
}

struct JsonSeed {
    depth: usize,
}
impl<'de> DeserializeSeed<'de> for JsonSeed {
    type Value = ();

    fn deserialize<D: de::Deserializer<'de>>(self, deserializer: D) -> Result<(), D::Error> {
        let raw = <&RawValue>::deserialize(deserializer)?;
        validate(raw, self.depth).map_err(de::Error::custom)
    }
}
impl<'de> Visitor<'de> for JsonSeed {
    type Value = ();

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("JSON with unique object keys and bounded nesting")
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut sequence: A) -> Result<(), A::Error> {
        while sequence
            .next_element_seed(Self {
                depth: self.depth + 1,
            })?
            .is_some()
        {}
        Ok(())
    }

    fn visit_map<A: MapAccess<'de>>(self, mut object: A) -> Result<(), A::Error> {
        let mut keys = std::collections::BTreeSet::new();
        while let Some(key) = object.next_key::<String>()? {
            if !keys.insert(key) {
                return Err(de::Error::custom("duplicate JSON object key"));
            }
            object.next_value_seed(Self {
                depth: self.depth + 1,
            })?;
        }
        Ok(())
    }
}

fn validate(raw: &RawValue, depth: usize) -> serde_json::Result<()> {
    // RawValue validates JSON syntax without evaluating numbers. Visit containers
    // to reject duplicate keys and strings to reject unpaired Unicode surrogates.
    // Subtree scans borrow the input; repeated scanning is bounded by depth 64.
    match raw.get().as_bytes()[0] {
        b'{' | b'[' if depth == MAX_JSON_DEPTH => {
            Err(de::Error::custom("JSON nesting exceeds depth limit"))
        }
        b'{' => serde_json::Deserializer::from_str(raw.get()).deserialize_map(JsonSeed { depth }),
        b'[' => serde_json::Deserializer::from_str(raw.get()).deserialize_seq(JsonSeed { depth }),
        b'"' => serde_json::from_str::<String>(raw.get()).map(|_| ()),
        _ => Ok(()),
    }
}

/// Validate one complete JSON value, retaining its exact numeric tokens.
///
/// Invalid UTF-8, unpaired surrogates, duplicate keys, excessive nesting,
/// non-JSON numbers (NaN/Infinity) and trailing values are rejected. Finite
/// decimal values such as 1e400 do not need to fit a machine float.
pub fn parse_json(bytes: &[u8]) -> io::Result<Message<'_>> {
    if bytes.len() > MAX_BODY {
        return Err(invalid("testhost body exceeds byte limit"));
    }
    let text = std::str::from_utf8(bytes).map_err(invalid)?;
    let raw = serde_json::from_str::<&RawValue>(text).map_err(invalid)?;
    validate(raw, 0).map_err(invalid)?;
    Ok(Message { raw })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufReader, Cursor};

    #[test]
    fn fragmented_and_coalesced_frames_preserve_utf8_byte_lengths() {
        let payloads = [r#"{"text":"😀"}"#.as_bytes(), b"[null,true,1]"];
        let mut wire = Vec::new();
        for payload in payloads {
            write(&mut wire, payload).unwrap();
        }
        assert!(wire.starts_with(b"Content-Length: 15\r\n\r\n"));
        let mut reader = BufReader::with_capacity(1, Cursor::new(wire));
        for payload in payloads {
            assert_eq!(read(&mut reader).unwrap().unwrap(), payload);
        }
        assert!(read(&mut reader).unwrap().is_none());
    }

    #[test]
    fn eof_is_clean_only_before_the_first_header_byte() {
        assert!(read(&mut Cursor::new(b"")).unwrap().is_none());
        for wire in [
            b"C".as_slice(),
            b"Content-Length: 1\r\n",
            b"Content-Length: 2\r\n\r\nx",
        ] {
            assert_eq!(
                read(&mut Cursor::new(wire)).unwrap_err().kind(),
                io::ErrorKind::UnexpectedEof
            );
        }
    }

    #[test]
    fn malformed_and_ambiguous_headers_are_rejected() {
        for wire in [
            b"Content-Length: 1\n\nx".as_slice(),
            b"Content-Length: 1\rx\r\n\r\nx",
            b"Content-Length: 1\r\nContent-Length: 1\r\n\r\nx",
            b"Content-Length: -1\r\n\r\nx",
            b"Content-Length: +1\r\n\r\nx",
            b"Content-Length: 0\r\n\r\nx",
            b"Content-Length: \r\n\r\nx",
            b"Content-Length: 1.0\r\n\r\nx",
            b"Content-Length: 8388609\r\n\r\nx",
            b"Content-Length: 99999999999999999999999999999\r\n\r\nx",
            b"content-length: 1\r\n\r\nx",
            b"X\r\nContent-Length: 1\r\n\r\nx",
            b"Bad Name: x\r\nContent-Length: 1\r\n\r\nx",
            b"X: \x01\r\nContent-Length: 1\r\n\r\nx",
            b"\r\n",
        ] {
            assert_eq!(
                read(&mut Cursor::new(wire)).unwrap_err().kind(),
                io::ErrorKind::InvalidData,
                "wire: {wire:?}"
            );
        }
    }

    #[test]
    fn header_limit_counts_all_lines_and_the_final_separator() {
        let suffix = b"\r\nContent-Length: \t1 \t\r\n\r\n";
        let mut wire = b"X: ".to_vec();
        wire.resize(MAX_HEADER - suffix.len(), b'a');
        wire.extend_from_slice(suffix);
        wire.push(b'x');
        assert_eq!(read(&mut Cursor::new(&wire)).unwrap().unwrap(), b"x");
        wire.insert(3, b'a');
        assert_eq!(
            read(&mut Cursor::new(wire)).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
    }

    #[derive(Default)]
    struct FlushWriter {
        bytes: Vec<u8>,
        flushes: usize,
    }

    impl Write for FlushWriter {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.bytes.extend_from_slice(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            self.flushes += 1;
            Ok(())
        }
    }

    #[test]
    fn writes_flush_and_reject_invalid_sizes_before_emitting_bytes() {
        let mut writer = FlushWriter::default();
        write(&mut writer, b"{}").unwrap();
        assert_eq!(writer.bytes, b"Content-Length: 2\r\n\r\n{}");
        assert_eq!(writer.flushes, 1);
        assert!(write(&mut writer, b"").is_err());
        assert!(write(&mut writer, &vec![0; MAX_BODY + 1]).is_err());
        assert_eq!(writer.bytes, b"Content-Length: 2\r\n\r\n{}");
        assert_eq!(writer.flushes, 1);
    }

    #[test]
    fn strict_json_rejects_duplicate_keys_at_every_depth() {
        for bytes in [
            br#"{"a":1,"a":2}"#.as_slice(),
            br#"[{"a":{"b":1,"b":2}}]"#,
            br#"{"a":1,"\u0061":2}"#,
        ] {
            assert!(parse_json(bytes)
                .unwrap_err()
                .to_string()
                .contains("duplicate JSON object key"));
        }
        assert_eq!(
            parse_json(br#"[{"a":1},{"a":2}]"#).unwrap().as_str(),
            r#"[{"a":1},{"a":2}]"#
        );
    }

    #[test]
    fn strict_json_rejects_trailing_invalid_and_nonfinite_input() {
        for bytes in [
            b"{} []".as_slice(),
            b"NaN",
            b"Infinity",
            b"{\"x\":\"\xff\"}",
            br#""\ud800""#,
            b"",
        ] {
            assert!(parse_json(bytes).is_err(), "payload: {bytes:?}");
        }
        assert_eq!(parse_json(b"\r\nnull \t").unwrap().as_str(), "null");
    }

    #[test]
    fn raw_validation_preserves_numbers_without_reserved_object_keys() {
        for text in [
            "123456789012345678901234567890",
            "-0",
            "1e400",
            "1e-400",
            "0.123456789012345678901234567890",
            r#"{"$serde_json::private::Number":"123"}"#,
            r#"{"$serde_json::private::RawValue":"[1,2]"}"#,
        ] {
            assert_eq!(parse_json(text.as_bytes()).unwrap().as_str(), text);
        }
        for bytes in [
            br#"{"opaque":["\ud800"]}"#.as_slice(),
            br#"{"opaque":{"\ud800":1}}"#,
            br#"{"opaque":{"$serde_json::private::Number":1,"$serde_json::private::Number":2}}"#,
            b"[01]",
            b"[1e]",
            b"[1.]",
            b"[+1]",
            b"[-Infinity]",
        ] {
            assert!(parse_json(bytes).is_err(), "payload: {bytes:?}");
        }
    }

    #[test]
    fn strict_json_depth_is_explicitly_bounded() {
        let nested = |count: usize| format!("{}0{}", "[".repeat(count), "]".repeat(count));
        assert!(parse_json(nested(MAX_JSON_DEPTH).as_bytes()).is_ok());
        assert!(parse_json(nested(MAX_JSON_DEPTH + 1).as_bytes()).is_err());
        let mixed = format!("{}0{}", "{\"x\":[".repeat(33), "]}".repeat(33));
        assert!(parse_json(mixed.as_bytes()).is_err());
    }
}
