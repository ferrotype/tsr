//! First P1 vertical slice: actual intrinsic/string construction, interning and
//! fresh/regular links on a bare checker state, numbering types from 1 the way
//! its Go counterpart (`&Checker{}`) does. This is not the subset's per-type
//! footprint measurement; the storage-families trace supersedes it for P1.

use crate::{CheckerOptions, CheckerState, Error, LiteralValue, TypeId, TypeKind};
use serde_json::{json, Value};
use std::sync::Arc;
use tsr_arena::{CheckerIdentity, Counters, Generation};
use tsr_ast::JsString;

enum Action {
    Intrinsic(u32, JsString),
    String(JsString),
    Fresh(usize),
}

/// Input and text backing are prepared before the measured constructor phase.
pub struct Prepared {
    actions: Vec<Action>,
}

/// All returned roots and type storage stay alive at the allocation checkpoint.
pub struct Live {
    _counters: Counters,
    _generation: Generation,
    _identity: Arc<CheckerIdentity>,
    state: CheckerState,
    roots: Vec<TypeId>,
}

fn bytes(text: &str) -> Result<Vec<u8>, String> {
    if !text.len().is_multiple_of(2) || !text.is_ascii() {
        return Err("invalid byte hex".into());
    }
    text.as_bytes()
        .chunks_exact(2)
        .map(|chunk| {
            let digit = |byte: u8| char::from(byte).to_digit(16).ok_or("invalid hex digit");
            Ok((digit(chunk[0])? * 16 + digit(chunk[1])?) as u8)
        })
        .collect()
}

pub(crate) fn hex(bytes: &[u8]) -> String {
    crate::storage_families::hex(bytes)
}

impl Prepared {
    pub fn new(requests: &Value) -> Result<Self, String> {
        let rows = requests.as_array().ok_or("expected action array")?;
        if rows.is_empty() {
            return Err("empty constructor trace".into());
        }
        let mut actions = Vec::with_capacity(rows.len());
        for (index, row) in rows.iter().enumerate() {
            let row = row.as_object().ok_or("action must be an object")?;
            let op = row.get("op").and_then(Value::as_str).ok_or("missing op")?;
            let expected: &[&str] = match op {
                "intrinsic" => &["op", "flags", "text_hex"],
                "string" => &["op", "text_hex"],
                "fresh" => &["op", "root"],
                _ => return Err("unknown constructor action".into()),
            };
            if row.len() != expected.len() || expected.iter().any(|key| !row.contains_key(*key)) {
                return Err("missing or unexpected action field".into());
            }
            actions.push(if op == "fresh" {
                let root = row["root"]
                    .as_u64()
                    .and_then(|value| usize::try_from(value).ok())
                    .ok_or("invalid root")?;
                if root >= index {
                    return Err("fresh root is not yet available".into());
                }
                Action::Fresh(root)
            } else {
                let value =
                    JsString::from_bytes(bytes(row["text_hex"].as_str().ok_or("invalid text")?)?);
                if op == "string" {
                    Action::String(value)
                } else {
                    let flags = row["flags"]
                        .as_u64()
                        .and_then(|value| u32::try_from(value).ok())
                        .ok_or("invalid flags")?;
                    Action::Intrinsic(flags, value)
                }
            });
        }
        Ok(Self { actions })
    }

    pub fn execute(&self) -> Result<Live, Error> {
        let counters = Counters::new();
        let generation = Generation::new(&counters);
        let identity = CheckerIdentity::new(generation.clone(), &counters);
        let mut state = CheckerState::bare(&identity, &counters, CheckerOptions::default())?;
        let mut roots = Vec::with_capacity(self.actions.len());
        for action in &self.actions {
            let id = match action {
                Action::Intrinsic(flags, name) => {
                    state.new_intrinsic_type_ex(*flags, name.as_bytes(), 0)?
                }
                Action::String(value) => state.get_string_literal_type(value.clone())?,
                Action::Fresh(root) => {
                    let root = roots[*root];
                    if state.types.flags(root)? != crate::type_flags::STRING_LITERAL {
                        return Err(Error::Unsupported("fresh non-string literal type"));
                    }
                    state.get_fresh_type_of_literal_type(root)?
                }
            };
            roots.push(id);
        }
        Ok(Live {
            _counters: counters,
            _generation: generation,
            _identity: identity,
            state,
            roots,
        })
    }
}

impl Live {
    /// Called after allocator counters are sampled so JSON does not enter the interval.
    pub fn observation(&self) -> Value {
        let tables = self.state.types.tables();
        let roots: Vec<Value> = self
            .roots
            .iter()
            .map(|id| {
                let record = self.state.types.get(*id).expect("trace root");
                match record.kind() {
                    TypeKind::Intrinsic => {
                        let name = &self.state.types.intrinsic(*id).expect("intrinsic").name;
                        json!({"id": id.get(), "flags": record.flags, "kind": "intrinsic",
                            "text_hex": hex(name.as_bytes()), "regular": 0, "fresh": 0})
                    }
                    TypeKind::Literal => {
                        let data = self.state.types.literal(*id).expect("literal");
                        let LiteralValue::String(text) = &data.value else {
                            unreachable!("pilot only constructs string literal payloads")
                        };
                        json!({"id": id.get(), "flags": record.flags, "kind": "string",
                            "text_hex": hex(text.as_bytes()), "regular": data.regular.get(),
                            "fresh": data.fresh.map_or(0, TypeId::get)})
                    }
                    _ => {
                        unreachable!("pilot only constructs intrinsic and string literal payloads")
                    }
                }
            })
            .collect();
        let caches = &self.state.types.caches;
        json!({"roots": roots, "census": {
            "type_records": tables.records.len(), "record_bytes": size_of::<crate::TypeRecord>(),
            "record_capacity_bytes": tables.records.capacity() * size_of::<crate::TypeRecord>(),
            "intrinsic_records": tables.intrinsics.len(), "intrinsic_row_bytes": size_of::<crate::IntrinsicData>(),
            "intrinsic_capacity_bytes": tables.intrinsics.capacity() * size_of::<crate::IntrinsicData>(),
            "string_records": tables.literals.len(), "string_row_bytes": size_of::<crate::LiteralData>(),
            "string_capacity_bytes": tables.literals.capacity() * size_of::<crate::LiteralData>(),
            "string_cache_entries": caches.string_literal_types.len(),
            "string_cache_capacity": caches.string_literal_types.capacity(),
        }})
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn malformed_or_forward_referencing_requests_cannot_produce_observations() {
        for request in [
            json!([]),
            json!([{"op":"string", "text_hex":"f"}]),
            json!([{"op":"string", "text_hex":"gg"}]),
            json!([{"op":"string", "text_hex":"00", "unexpected":true}]),
            json!([{"op":"fresh", "root":0}]),
            json!([{"op":"intrinsic", "text_hex":"", "flags":true}]),
            json!([{"op":"intrinsic", "text_hex":"", "flags":4_294_967_296_u64}]),
        ] {
            assert!(Prepared::new(&request).is_err(), "accepted {request}");
        }
        let non_string = Prepared::new(&json!([
            {"op":"intrinsic", "text_hex":"616e79", "flags":1}, {"op":"fresh", "root":0}
        ]))
        .unwrap();
        assert!(matches!(non_string.execute(), Err(Error::Unsupported(_))));
    }
}
