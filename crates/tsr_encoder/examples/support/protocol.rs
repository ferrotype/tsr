//! Strict framing shared by S06 component and parser adapters.
use serde::de::{self, MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer};
use serde_json::{json, Map, Value};
use std::collections::BTreeSet;
use std::fmt;
use std::io::{self, BufRead, Write};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::{Arc, Mutex};
pub const MAX_REQUEST: usize = 16 * 1024 * 1024;
pub const MAX_RESPONSE: usize = 1024 * 1024;
/// Deserialize without map-key replacement or number coercion, recursively.
pub struct Strict(pub Value);
impl<'de> Deserialize<'de> for Strict {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct StrictVisitor;
        impl<'de> Visitor<'de> for StrictVisitor {
            type Value = Strict;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("strict integer-only JSON")
            }
            fn visit_bool<E: de::Error>(self, value: bool) -> Result<Strict, E> {
                Ok(Strict(value.into()))
            }
            fn visit_i64<E: de::Error>(self, value: i64) -> Result<Strict, E> {
                Ok(Strict(value.into()))
            }
            fn visit_u64<E: de::Error>(self, value: u64) -> Result<Strict, E> {
                i64::try_from(value)
                    .map(|value| Strict(value.into()))
                    .map_err(E::custom)
            }
            fn visit_str<E: de::Error>(self, value: &str) -> Result<Strict, E> {
                Ok(Strict(value.into()))
            }
            fn visit_string<E: de::Error>(self, value: String) -> Result<Strict, E> {
                Ok(Strict(value.into()))
            }
            fn visit_unit<E: de::Error>(self) -> Result<Strict, E> {
                Ok(Strict(Value::Null))
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Strict, A::Error> {
                let mut values = Vec::new();
                while let Some(Strict(value)) = seq.next_element()? {
                    values.push(value);
                }
                Ok(Strict(Value::Array(values)))
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Strict, A::Error> {
                let mut values = Map::new();
                while let Some((key, Strict(value))) = map.next_entry::<String, Strict>()? {
                    if values.insert(key.clone(), value).is_some() {
                        return Err(de::Error::custom(format!("duplicate key {key}")));
                    }
                }
                Ok(Strict(Value::Object(values)))
            }
        }
        deserializer.deserialize_any(StrictVisitor)
    }
}

pub fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut value = String::with_capacity(bytes.len() * 2);
    for &byte in bytes {
        value.push(char::from(DIGITS[usize::from(byte >> 4)]));
        value.push(char::from(DIGITS[usize::from(byte & 15)]));
    }
    value
}
pub fn unhex(value: &Value) -> Result<Vec<u8>, String> {
    let text = value.as_str().ok_or("expected hex string")?;
    if text.len() % 2 != 0 || text.len() > MAX_REQUEST {
        return Err("invalid hex length".into());
    }
    let nibble = |byte: u8| match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        _ => None,
    };
    text.as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            Ok(nibble(pair[0]).ok_or("invalid lowercase hex")? * 16
                + nibble(pair[1]).ok_or("invalid lowercase hex")?)
        })
        .collect()
}
pub fn fields(value: &Value, expected: &str) -> Result<(), String> {
    let object = value.as_object().ok_or("expected object")?;
    let expected: BTreeSet<_> = expected.split_whitespace().collect();
    if object.keys().map(String::as_str).collect::<BTreeSet<_>>() != expected {
        return Err("unknown/missing object field".into());
    }
    Ok(())
}
#[derive(Clone)]
pub struct Session(Arc<Mutex<State>>);
struct State {
    id: String,
    seq: usize,
    stages: usize,
    out: io::BufWriter<io::Stdout>,
    error: Option<String>,
}
impl Session {
    pub fn new(id: &str, op: &str) -> Self {
        let session = Self(Arc::new(Mutex::new(State {
            id: id.into(),
            seq: 0,
            stages: 0,
            out: io::BufWriter::new(io::stdout()),
            error: None,
        })));
        session.frame(json!({"tag":"begin","op":op}));
        session
    }
    fn frame(&self, value: Value) {
        let mut state = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.frame(value);
    }
    pub fn observe(&self, stage: &str, kind: &str, value: Value) {
        let mut state = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let seq = state.seq;
        let mut frame = json!({"tag":"observation","seq":seq,"stage":stage,"kind":kind});
        frame["value"] = value;
        state.frame(frame);
        state.seq += 1;
    }
    pub fn stage(&self, stage: &str, action: impl FnOnce() -> Result<(), String>) -> bool {
        let (outcome, message) = match catch_unwind(AssertUnwindSafe(action)) {
            Ok(Ok(())) => ("ok", String::new()),
            Ok(Err(error)) => ("error", error),
            Err(error) => {
                let message = error.downcast_ref::<&str>().map_or_else(
                    || {
                        error
                            .downcast_ref::<String>()
                            .cloned()
                            .unwrap_or_else(|| "non-string panic payload".into())
                    },
                    |message| (*message).into(),
                );
                ("panic", message)
            }
        };
        let mut state = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.frame(json!({"tag":"stage","stage":stage,"outcome":outcome,"message_hex":hex(message.as_bytes())}));
        state.stages += 1;
        outcome == "ok"
    }
    pub fn failure(&self, error: String) {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .error = Some(error);
    }
    pub fn flush(&self) -> Result<(), String> {
        let mut state = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(error) = state.error.take() {
            return Err(error);
        }
        state.out.flush().map_err(|error| error.to_string())
    }
    pub fn finish(&self) -> Result<(), String> {
        let mut state = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let (seq, stages) = (state.seq, state.stages);
        state.frame(json!({"tag":"end","observations":seq,"stages":stages}));
        if let Some(error) = state.error.take() {
            return Err(error);
        }
        state.out.flush().map_err(|error| error.to_string())
    }
}
impl State {
    fn frame(&mut self, mut value: Value) {
        if self.error.is_some() {
            return;
        }
        value["id"] = self.id.clone().into();
        value["version"] = 1.into();
        let result = (|| {
            let bytes = serde_json::to_vec(&value).map_err(|error| error.to_string())?;
            if bytes.len() > MAX_RESPONSE {
                return Err("oversized response record".into());
            }
            self.out
                .write_all(&bytes)
                .and_then(|()| self.out.write_all(b"\n"))
                .map_err(|error| error.to_string())
        })();
        if let Err(error) = result {
            self.error = Some(error);
        }
    }
}
/// Bounded LF framing rejects an unterminated final record before execution.
pub fn run(
    validate: impl Fn(&Value) -> Result<(), String>,
    execute: impl Fn(&Session, &Value),
) -> Result<(), String> {
    let mut input = io::stdin().lock();
    let mut line = Vec::new();
    let mut seen = BTreeSet::new();
    loop {
        let bytes = input.fill_buf().map_err(|e| e.to_string())?;
        if bytes.is_empty() {
            return if line.is_empty() {
                Ok(())
            } else {
                Err("truncated request record".into())
            };
        }
        let newline = bytes.iter().position(|&byte| byte == b'\n');
        let count = newline.map_or(bytes.len(), |index| index + 1);
        if line.len() + count > MAX_REQUEST + 1 {
            return Err("oversized request record".into());
        }
        line.extend_from_slice(&bytes[..count]);
        input.consume(count);
        if newline.is_none() {
            continue;
        }
        line.pop();
        let Strict(value) = serde_json::from_slice(&line).map_err(|error| error.to_string())?;
        validate(&value)?;
        let id = value["id"].as_str().ok_or("missing id")?;
        if !seen.insert(id.to_owned()) {
            return Err("duplicate request identity".into());
        }
        let session = Session::new(id, value["op"].as_str().ok_or("missing op")?);
        session.flush()?;
        execute(&session, &value);
        session.finish()?;
        line.clear();
    }
}
