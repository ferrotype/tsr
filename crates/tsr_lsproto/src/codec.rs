//! Strict struct and union codec rules from the pinned lsproto implementation.
use tsr_json::{Decode, Decoder, Encode, Encoder, Error, Kind, Options, Token};

pub(crate) struct Field {
    pub name: &'static str,
    pub required_bit: Option<u8>,
    pub reject_null: bool,
}

/// port: tsc/internal/lsp/lsproto/structcodec.go:unmarshalStruct
pub(crate) fn structure(
    input: &mut Decoder<'_>,
    type_name: &'static str,
    strict: bool,
    fields: &[Field],
    mut field: impl FnMut(usize, &mut Decoder<'_>) -> Result<(), Error>,
) -> Result<(), Error> {
    // Generated ordinary struct codecs handle null by zeroing the destination
    // before entering this helper. Custom strict codecs require an object.
    if input.peek_kind() != Kind::BeginObject {
        return Err(Error::Message(format!(
            "expected object start, but encountered {}",
            input.peek_kind().name()
        )));
    }
    input.read_token()?;
    // Bits belong only to required fields, as in specFor. A struct may have
    // more than 64 optional fields. Presence is local to this decode call.
    let mut missing = 0u64;
    for bit in fields.iter().filter_map(|field| field.required_bit) {
        assert!(bit < 64, "too many required protocol fields in {type_name}");
        missing |= 1 << bit;
    }
    while input.peek_kind() != Kind::EndObject {
        let name = if strict {
            // The pin compares raw JSON name bytes in its custom codec. Do not
            // silently normalize escaped spellings into known property names.
            let raw = input.read_value()?;
            raw.get(1..raw.len().saturating_sub(1))
                .unwrap_or_default()
                .to_vec()
        } else {
            let Token::String(name) = input.read_token()? else {
                return Err(Error::Message("object name must be a string".into()));
            };
            name.into_owned()
        };
        if let Some(index) = fields.iter().position(|f| f.name.as_bytes() == name) {
            if let Some(bit) = fields[index].required_bit {
                missing &= !(1 << bit);
            }
            if fields[index].reject_null && input.peek_kind() == Kind::Null {
                return Err(Error::Message(format!(
                    "null value is not allowed for field {:?}",
                    fields[index].name
                )));
            }
            field(index, input)?;
        } else {
            input.skip_value()?;
        }
    }
    input.read_token()?;
    if missing != 0 {
        let names = fields
            .iter()
            .filter(|f| f.required_bit.is_some_and(|bit| missing & (1 << bit) != 0))
            .map(|f| f.name)
            .collect::<Vec<_>>()
            .join(", ");
        return Err(Error::Message(format!(
            "missing required properties: {names}"
        )));
    }
    Ok(())
}

/// port: tsc/internal/lsp/lsproto/structcodec.go:marshalUnion
pub(crate) fn union(
    out: &mut Encoder<'_>,
    name: &str,
    nullable: bool,
    fields: &[Option<&dyn Encode>],
) -> Result<(), Error> {
    let mut values = fields.iter().flatten();
    let first = values.next();
    assert!(
        values.next().is_none() && (nullable || first.is_some()),
        "{name} must have {} value set",
        if nullable {
            "at most one"
        } else {
            "exactly one"
        }
    );
    match first {
        Some(value) => out.value(*value),
        None => out.write_token(Token::Null),
    }
}

pub(crate) fn try_member<T: Decode + Default>(data: &[u8], slot: &mut Option<Box<T>>) -> bool {
    let mut value = Box::default();
    if tsr_json::unmarshal(data, &mut value, Options::default()).is_err() {
        return false;
    }
    *slot = Some(value);
    true
}

/// port: tsc/internal/lsp/lsproto/lsp.go:jsonObjectRawField
pub(crate) fn raw_field(data: &[u8], key: &str) -> Option<Vec<u8>> {
    let mut input = Decoder::new(data);
    if input.peek_kind() != Kind::BeginObject {
        return None;
    }
    input.read_token().ok()?;
    while input.peek_kind() != Kind::EndObject {
        let name = input.read_value().ok()?;
        if name.get(1..name.len().saturating_sub(1)) == Some(key.as_bytes()) {
            return input.read_value().ok();
        }
        input.skip_value().ok()?;
    }
    None
}

/// port: tsc/internal/lsp/lsproto/lsp.go:jsonObjectHasKey
pub(crate) fn first_key(data: &[u8], keys: &[&str]) -> Option<usize> {
    let mut input = Decoder::new(data);
    if input.peek_kind() != Kind::BeginObject {
        return None;
    }
    input.read_token().ok()?;
    while input.peek_kind() != Kind::EndObject {
        let name = input.read_value().ok()?;
        if let Some(index) = keys
            .iter()
            .position(|k| name.get(1..name.len().saturating_sub(1)) == Some(k.as_bytes()))
        {
            return Some(index);
        }
        input.skip_value().ok()?;
    }
    None
}

pub(crate) fn invalid_kind(name: &str, kind: Kind) -> Error {
    Error::Message(format!("invalid {name}: got {}", kind.name()))
}
pub(crate) fn invalid_value(name: &str, data: &[u8]) -> Error {
    Error::Message(format!("invalid {name}: {}", String::from_utf8_lossy(data)))
}
pub(crate) fn literal(input: &mut Decoder<'_>, name: &str, expected: &[u8]) -> Result<(), Error> {
    let data = input.read_value()?;
    if data != expected {
        return Err(Error::Message(format!(
            "expected {name} value {}, got {}",
            String::from_utf8_lossy(expected),
            String::from_utf8_lossy(&data)
        )));
    }
    Ok(())
}
