//! Byte-oriented typed and token JSON. Go strings remain byte strings until a
//! particular encode/decode operation applies its UTF-8 policy.
mod decode;
mod decoder;
mod encoder;
mod error;
mod state;
mod token;
mod wire;
pub use decode::{unmarshal, unmarshal_decode, unmarshal_read, Decode, DecodeKey};
pub use decoder::Decoder;
pub use encoder::Encoder;
pub use error::{Error, SemanticError, SyntaxError};
pub use token::{Kind, Token};

use std::collections::HashMap;
use std::hash::{BuildHasher, Hash};
use std::io::Write;
use tsr_core::collections::OrderedMap;
use tsr_jsstring::JsString;

#[derive(Clone, Copy, Debug, Default)]
pub struct Options<'a> {
    pub indent: Option<&'a str>,
    pub prefix: Option<&'a str>,
    pub allow_duplicate_names: Option<bool>,
    pub deterministic: Option<bool>,
    /// None leaves the caller's default: strict tokens/decoding, repairing
    /// strings for the pinned Marshal wrappers.
    pub allow_invalid_utf8: Option<bool>,
}

impl Options<'_> {
    fn join(&mut self, other: &Self) {
        if other.indent.is_some() {
            self.indent = other.indent;
        }
        if other.prefix.is_some() {
            self.prefix = other.prefix;
        }
        if other.allow_duplicate_names.is_some() {
            self.allow_duplicate_names = other.allow_duplicate_names;
        }
        if other.deterministic.is_some() {
            self.deterministic = other.deterministic;
        }
        if other.allow_invalid_utf8.is_some() {
            self.allow_invalid_utf8 = other.allow_invalid_utf8;
        }
    }
}

pub trait Encode {
    fn encode(&self, out: &mut Encoder<'_>) -> Result<(), Error>;
    fn type_name(&self) -> &'static str {
        std::any::type_name::<Self>()
    }
    /// Custom recursive codecs are guarded even if they recurse without opening
    /// a JSON container. Built-in scalar implementations bypass this query;
    /// built-in containers guard the traversal instead.
    fn encode_guarded(&self, out: &mut Encoder<'_>) -> Result<(), Error> {
        stacker::maybe_grow(128 * 1024, 2 * 1024 * 1024, || self.encode(out))
    }
}

/// Convenience wrapper for callers that discard incomplete output on error.
pub fn marshal(value: &(impl Encode + ?Sized), options: Options<'_>) -> Result<Vec<u8>, Error> {
    let (bytes, result) = marshal_partial(value, options);
    result.map(|()| bytes)
}
/// Return the committed prefix alongside an encoding failure, as Go Marshal
/// does. Invalid options fail before any bytes are emitted.
/// port: tsc/internal/json/json.go:Marshal
pub fn marshal_partial(
    value: &(impl Encode + ?Sized),
    mut options: Options<'_>,
) -> (Vec<u8>, Result<(), Error>) {
    options.allow_invalid_utf8 = Some(options.allow_invalid_utf8.unwrap_or(true));
    let mut out = match Encoder::new(options) {
        Ok(out) => out,
        Err(error) => return (Vec::new(), Err(error)),
    };
    out.omit_top_level_newline = true;
    let result = out.value(value);
    (out.into_bytes(), result)
}
/// port: tsc/internal/json/json.go:MarshalIndent
pub fn marshal_indent(
    value: &(impl Encode + ?Sized),
    prefix: &str,
    indent: &str,
) -> Result<Vec<u8>, Error> {
    marshal(
        value,
        Options {
            indent: (!(prefix.is_empty() && indent.is_empty())).then_some(indent),
            prefix: Some(prefix),
            ..Options::default()
        },
    )
}
/// port: tsc/internal/json/json.go:MarshalWrite
pub fn marshal_write(
    out: &mut dyn Write,
    value: &(impl Encode + ?Sized),
    mut options: Options<'_>,
) -> Result<(), Error> {
    options.allow_invalid_utf8 = Some(options.allow_invalid_utf8.unwrap_or(true));
    let mut encoder = Encoder::with_writer(out, options)?;
    encoder.omit_top_level_newline = true;
    encoder.value(value)?;
    encoder.flush()
}
/// port: tsc/internal/json/json.go:MarshalIndentWrite
pub fn marshal_indent_write(
    out: &mut dyn Write,
    value: &(impl Encode + ?Sized),
    prefix: &str,
    indent: &str,
) -> Result<(), Error> {
    marshal_write(
        out,
        value,
        Options {
            indent: (!(prefix.is_empty() && indent.is_empty())).then_some(indent),
            prefix: Some(prefix),
            ..Options::default()
        },
    )
}
/// port: tsc/internal/json/json.go:MarshalEncode
pub fn marshal_encode<'a>(
    out: &mut Encoder<'a>,
    value: &(impl Encode + ?Sized),
    options: Options<'a>,
) -> Result<(), Error> {
    out.marshal_encode(value, options)
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RawValue(pub Vec<u8>);
impl RawValue {
    pub fn kind(&self) -> Kind {
        self.0
            .iter()
            .find(|b| !matches!(b, b' ' | b'\t' | b'\r' | b'\n'))
            .map_or(Kind::Invalid, |&b| Kind::from_byte(b))
    }
}
impl Encode for RawValue {
    fn type_name(&self) -> &'static str {
        "jsontext.Value"
    }
    fn encode(&self, out: &mut Encoder<'_>) -> Result<(), Error> {
        out.write_value(&self.0)
            .map_err(|e| out.semantic(self.type_name(), e))
    }
    fn encode_guarded(&self, out: &mut Encoder<'_>) -> Result<(), Error> {
        self.encode(out)
    }
}
macro_rules! scalar {
    ($ty:ty,$name:literal,$method:ident,$convert:expr) => {
        impl Encode for $ty {
            fn type_name(&self) -> &'static str {
                $name
            }
            fn encode(&self, out: &mut Encoder<'_>) -> Result<(), Error> {
                out.$method(($convert)(self))
            }
            fn encode_guarded(&self, out: &mut Encoder<'_>) -> Result<(), Error> {
                self.encode(out)
            }
        }
    };
}
scalar!(bool, "bool", boolean, |v: &bool| *v);
scalar!(f64, "float64", number, |v: &f64| *v);
scalar!(i64, "int64", int, |v: &i64| *v);
scalar!(u64, "uint64", uint, |v: &u64| *v);
scalar!(i32, "int32", int, |v: &i32| i64::from(*v));
scalar!(u32, "uint32", uint, |v: &u32| u64::from(*v));
impl Encode for str {
    fn type_name(&self) -> &'static str {
        "string"
    }
    fn encode(&self, out: &mut Encoder<'_>) -> Result<(), Error> {
        out.string(self.as_bytes())
    }
    fn encode_guarded(&self, out: &mut Encoder<'_>) -> Result<(), Error> {
        self.encode(out)
    }
}
impl Encode for String {
    fn type_name(&self) -> &'static str {
        "string"
    }
    fn encode(&self, out: &mut Encoder<'_>) -> Result<(), Error> {
        self.as_str().encode(out)
    }
    fn encode_guarded(&self, out: &mut Encoder<'_>) -> Result<(), Error> {
        self.encode(out)
    }
}
impl Encode for JsString {
    fn type_name(&self) -> &'static str {
        "string"
    }
    fn encode(&self, out: &mut Encoder<'_>) -> Result<(), Error> {
        out.string(self.as_bytes())
    }
    fn encode_guarded(&self, out: &mut Encoder<'_>) -> Result<(), Error> {
        self.encode(out)
    }
}
impl<T: Encode + ?Sized> Encode for &T {
    fn type_name(&self) -> &'static str {
        (*self).type_name()
    }
    fn encode(&self, out: &mut Encoder<'_>) -> Result<(), Error> {
        (*self).encode(out)
    }
    fn encode_guarded(&self, out: &mut Encoder<'_>) -> Result<(), Error> {
        (*self).encode_guarded(out)
    }
}
impl<T: Encode> Encode for Option<T> {
    fn encode(&self, out: &mut Encoder<'_>) -> Result<(), Error> {
        if let Some(value) = self {
            out.value(value)
        } else {
            out.null()
        }
    }
    fn encode_guarded(&self, out: &mut Encoder<'_>) -> Result<(), Error> {
        self.encode(out)
    }
}
impl<T: Encode> Encode for [T] {
    fn encode(&self, out: &mut Encoder<'_>) -> Result<(), Error> {
        out.array(self)
    }
    fn encode_guarded(&self, out: &mut Encoder<'_>) -> Result<(), Error> {
        self.encode(out)
    }
}
impl<T: Encode> Encode for Vec<T> {
    fn encode(&self, out: &mut Encoder<'_>) -> Result<(), Error> {
        self.as_slice().encode(out)
    }
    fn encode_guarded(&self, out: &mut Encoder<'_>) -> Result<(), Error> {
        self.encode(out)
    }
}
/// Key conversion is separate from value encoding: all names are JSON strings.
pub trait Key {
    fn json_key(&self) -> std::borrow::Cow<'_, [u8]>;
}
impl Key for String {
    fn json_key(&self) -> std::borrow::Cow<'_, [u8]> {
        self.as_bytes().into()
    }
}
impl Key for JsString {
    fn json_key(&self) -> std::borrow::Cow<'_, [u8]> {
        self.as_bytes().into()
    }
}
impl Key for str {
    fn json_key(&self) -> std::borrow::Cow<'_, [u8]> {
        self.as_bytes().into()
    }
}
impl<T: Key + ?Sized> Key for &T {
    fn json_key(&self) -> std::borrow::Cow<'_, [u8]> {
        (*self).json_key()
    }
}
macro_rules! integer_key {($($t:ty),*)=>{$(impl Key for $t {fn json_key(&self)->std::borrow::Cow<'_,[u8]> {self.to_string().into_bytes().into()}})*};}
integer_key!(i8, i16, i32, i64, isize, u8, u16, u32, u64, usize);
impl<K: Eq + Hash + Key, V: Encode, S: BuildHasher> Encode for OrderedMap<K, V, S> {
    /// port: tsc/internal/collections/ordered_map.go:OrderedMap.MarshalJSONTo
    fn encode(&self, out: &mut Encoder<'_>) -> Result<(), Error> {
        out.write_token(Token::BeginObject)?;
        for (key, value) in self.entries() {
            out.string(&key.json_key())?;
            out.value(value)?;
        }
        out.write_token(Token::EndObject)
    }
}
impl<K: Eq + Hash + Key, V: Encode, S: BuildHasher> Encode for HashMap<K, V, S> {
    fn encode(&self, out: &mut Encoder<'_>) -> Result<(), Error> {
        out.write_token(Token::BeginObject)?;
        if out.options.deterministic.unwrap_or(false) {
            let mut entries: Vec<_> = self.iter().map(|(k, v)| (k.json_key(), v)).collect();
            entries.sort_unstable_by(|a, b| a.0.cmp(&b.0));
            for (key, value) in entries {
                out.string(&key)?;
                out.value(value)?;
            }
        } else {
            for (key, value) in self {
                out.string(&key.json_key())?;
                out.value(value)?;
            }
        }
        out.write_token(Token::EndObject)
    }
}

// JSON glue lives above core to keep core independent of the codec crate.
impl Encode for tsr_core::Tristate {
    fn encode(&self, out: &mut Encoder<'_>) -> Result<(), Error> {
        if self.is_true() {
            out.boolean(true)
        } else if self.is_false() {
            out.boolean(false)
        } else {
            out.null()
        }
    }
    fn encode_guarded(&self, out: &mut Encoder<'_>) -> Result<(), Error> {
        self.encode(out)
    }
}
impl Decode for tsr_core::Tristate {
    fn decode(&mut self, input: &mut Decoder<'_>) -> Result<(), Error> {
        *self = Self::unmarshal_json(&input.read_value()?);
        Ok(())
    }
}
