use crate::{Decoder, Error, Kind, Options, RawValue, SemanticError, Token};
use std::{
    hash::{BuildHasher, Hash},
    io::Read,
};
use tsr_core::collections::OrderedMap;
use tsr_jsstring::JsString;

/// Decode into the caller's existing destination. Successful earlier fields
/// survive a later error; decoding into a temporary and swapping is incorrect.
pub trait Decode {
    fn decode(&mut self, input: &mut Decoder<'_>) -> Result<(), Error>;
    fn type_name() -> &'static str {
        std::any::type_name::<Self>()
    }
}
impl Decoder<'_> {
    pub fn value<T: Decode + ?Sized>(&mut self, value: &mut T) -> Result<(), Error> {
        let before = self.depth_length();
        stacker::maybe_grow(128 * 1024, 2 * 1024 * 1024, || value.decode(self))?;
        if self.depth_length() != (before.0, before.1 + 1) {
            return Err(semantic(
                T::type_name(),
                self.input_offset(),
                self.stack_pointer(),
                Kind::Invalid,
                None,
                Some(Error::Message(
                    "must read or write exactly one value".into(),
                )),
            ));
        }
        Ok(())
    }
    pub fn type_error(&mut self, type_name: &'static str) -> Result<(), Error> {
        let (offset, pointer) = self.next_location()?;
        let kind = self.peek_kind();
        self.skip_value()?;
        Err(semantic(type_name, offset, pointer, kind, None, None))
    }
    pub fn object(
        &mut self,
        mut field: impl FnMut(&[u8], &mut Self) -> Result<(), Error>,
    ) -> Result<(), Error> {
        if self.peek_kind() != Kind::BeginObject {
            return self.type_error("struct");
        }
        self.read_token()?;
        while self.peek_kind() != Kind::EndObject {
            let token = self.read_token()?;
            let Token::String(name) = token else {
                return Err(Error::Message("object name must be a string".into()));
            };
            field(&name, self)?;
        }
        self.read_token()?;
        Ok(())
    }
}
fn semantic(
    type_name: &'static str,
    offset: usize,
    pointer: String,
    kind: Kind,
    value: Option<Vec<u8>>,
    cause: Option<Error>,
) -> Error {
    Error::Semantic(Box::new(SemanticError {
        marshal: false,
        offset,
        pointer,
        kind,
        value,
        type_name,
        cause,
    }))
}

/// port: tsc/internal/json/json.go:Unmarshal
pub fn unmarshal<T: Decode + ?Sized>(
    bytes: &[u8],
    value: &mut T,
    options: Options<'_>,
) -> Result<(), Error> {
    unmarshal_read(std::io::Cursor::new(bytes), value, options)
}
/// port: tsc/internal/json/json.go:UnmarshalRead
pub fn unmarshal_read<T: Decode + ?Sized>(
    reader: impl Read,
    value: &mut T,
    options: Options<'_>,
) -> Result<(), Error> {
    let mut decoder = Decoder::with_options(reader, options);
    let result = decoder.value(value);
    if let Err(Error::Eof) = result {
        let (offset, pointer) = decoder.next_location()?;
        return Err(Error::truncated(offset, pointer));
    }
    result?;
    decoder.finish()
}
/// port: tsc/internal/json/json.go:UnmarshalDecode
pub fn unmarshal_decode<'a, T: Decode + ?Sized>(
    input: &mut Decoder<'a>,
    value: &mut T,
    options: Options<'a>,
) -> Result<(), Error> {
    input.unmarshal_decode(value, options)
}
impl Decode for RawValue {
    fn type_name() -> &'static str {
        "jsontext.Value"
    }
    fn decode(&mut self, input: &mut Decoder<'_>) -> Result<(), Error> {
        self.0 = input.read_value()?;
        Ok(())
    }
}
impl Decode for String {
    fn type_name() -> &'static str {
        "string"
    }
    fn decode(&mut self, input: &mut Decoder<'_>) -> Result<(), Error> {
        match input.peek_kind() {
            Kind::Null => {
                input.read_token()?;
                self.clear();
                Ok(())
            }
            Kind::String => {
                let Token::String(text) = input.read_token()? else {
                    unreachable!()
                };
                *self = String::from_utf8(text.into_owned()).expect("decoded string is UTF-8");
                Ok(())
            }
            _ => input.type_error(Self::type_name()),
        }
    }
}
impl Decode for JsString {
    fn type_name() -> &'static str {
        "string"
    }
    fn decode(&mut self, input: &mut Decoder<'_>) -> Result<(), Error> {
        match input.peek_kind() {
            Kind::Null => {
                input.read_token()?;
                *self = Self::default();
                Ok(())
            }
            Kind::String => {
                let Token::String(text) = input.read_token()? else {
                    unreachable!()
                };
                *self = Self::from_bytes(text.into_owned());
                Ok(())
            }
            _ => input.type_error(Self::type_name()),
        }
    }
}
impl Decode for bool {
    fn type_name() -> &'static str {
        "bool"
    }
    fn decode(&mut self, input: &mut Decoder<'_>) -> Result<(), Error> {
        match input.peek_kind() {
            Kind::Null => {
                input.read_token()?;
                *self = false;
                Ok(())
            }
            Kind::True | Kind::False => {
                *self = input.read_token()? == Token::Boolean(true);
                Ok(())
            }
            _ => input.type_error(Self::type_name()),
        }
    }
}
impl<T: Decode + Default> Decode for Option<T> {
    fn decode(&mut self, input: &mut Decoder<'_>) -> Result<(), Error> {
        if input.peek_kind() == Kind::Null {
            input.read_token()?;
            *self = None;
            Ok(())
        } else {
            input.value(self.get_or_insert_with(T::default))
        }
    }
}
impl<T: Decode + Default> Decode for Vec<T> {
    fn decode(&mut self, input: &mut Decoder<'_>) -> Result<(), Error> {
        if input.peek_kind() == Kind::Null {
            input.read_token()?;
            self.clear();
            return Ok(());
        }
        if input.peek_kind() != Kind::BeginArray {
            return input.type_error("slice");
        }
        input.read_token()?;
        self.clear();
        while input.peek_kind() != Kind::EndArray {
            self.push(T::default());
            input.value(self.last_mut().expect("inserted element"))?;
        }
        input.read_token()?;
        Ok(())
    }
}
impl Decode for f64 {
    fn type_name() -> &'static str {
        "float64"
    }
    fn decode(&mut self, input: &mut Decoder<'_>) -> Result<(), Error> {
        if input.peek_kind() == Kind::Null {
            input.read_token()?;
            *self = 0.0;
            return Ok(());
        }
        if input.peek_kind() != Kind::Number {
            return input.type_error(Self::type_name());
        }
        let (offset, pointer) = input.next_location()?;
        let Token::Number(text) = input.read_token()? else {
            unreachable!()
        };
        let text = text.into_owned();
        *self = std::str::from_utf8(&text)
            .expect("number is ASCII")
            .parse::<f64>()
            .expect("validated JSON number");
        if self.is_infinite() {
            return Err(semantic(
                Self::type_name(),
                offset,
                pointer,
                Kind::Number,
                Some(text),
                Some(Error::Message("value out of range".into())),
            ));
        }
        Ok(())
    }
}
macro_rules! integer {
    ($ty:ty,$name:literal) => {
        impl Decode for $ty {
            fn type_name() -> &'static str {
                $name
            }
            fn decode(&mut self, input: &mut Decoder<'_>) -> Result<(), Error> {
                if input.peek_kind() == Kind::Null {
                    input.read_token()?;
                    *self = 0;
                    return Ok(());
                }
                if input.peek_kind() != Kind::Number {
                    return input.type_error(Self::type_name());
                }
                let (offset, pointer) = input.next_location()?;
                let Token::Number(text) = input.read_token()? else {
                    unreachable!()
                };
                let text = text.into_owned();
                match std::str::from_utf8(&text)
                    .expect("number is ASCII")
                    .parse::<Self>()
                {
                    Ok(value) => {
                        *self = value;
                        Ok(())
                    }
                    Err(error) => {
                        let cause = match error.kind() {
                            std::num::IntErrorKind::PosOverflow
                            | std::num::IntErrorKind::NegOverflow => "value out of range",
                            _ => "invalid syntax",
                        };
                        Err(semantic(
                            Self::type_name(),
                            offset,
                            pointer,
                            Kind::Number,
                            Some(text),
                            Some(Error::Message(cause.into())),
                        ))
                    }
                }
            }
        }
    };
}
integer!(i64, "int64");
integer!(u64, "uint64");
integer!(isize, "int");
integer!(usize, "uint");
integer!(i32, "int32");
integer!(u32, "uint32");

pub trait DecodeKey: Sized {
    fn decode_key(bytes: &[u8]) -> Result<Self, Error>;
}
impl DecodeKey for String {
    fn decode_key(bytes: &[u8]) -> Result<Self, Error> {
        String::from_utf8(bytes.to_vec())
            .map_err(|_| Error::Message("invalid UTF-8 map key".into()))
    }
}
impl DecodeKey for JsString {
    fn decode_key(bytes: &[u8]) -> Result<Self, Error> {
        Ok(Self::from_bytes(bytes))
    }
}
impl DecodeKey for i64 {
    fn decode_key(bytes: &[u8]) -> Result<Self, Error> {
        std::str::from_utf8(bytes)
            .ok()
            .and_then(|s| s.parse().ok())
            .ok_or_else(|| Error::Message("invalid integer map key".into()))
    }
}
impl<K: Eq + Hash + Clone + DecodeKey, V: Decode + Default, S: BuildHasher> Decode
    for OrderedMap<K, V, S>
{
    /// port: tsc/internal/collections/ordered_map.go:OrderedMap.UnmarshalJSONFrom
    fn decode(&mut self, input: &mut Decoder<'_>) -> Result<(), Error> {
        let token = input.read_token()?;
        if token == Token::Null {
            return Ok(());
        }
        if token != Token::BeginObject {
            return Err(Error::Message(
                "cannot unmarshal non-object JSON value into Map".into(),
            ));
        }
        while input.peek_kind() != Kind::EndObject {
            let Token::String(name) = input.read_token()? else {
                return Err(Error::Message("object name must be a string".into()));
            };
            let key = K::decode_key(&name)?;
            let mut value = V::default();
            input.value(&mut value)?;
            self.insert(key, value);
        }
        input.read_token()?;
        Ok(())
    }
}
