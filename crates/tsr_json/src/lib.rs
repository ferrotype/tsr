//! Typed JSON encoding for compiler data. This is the first part of the pinned
//! internal/json contract: raw JSON values and streaming decoding are not yet
//! implemented. Strings accept arbitrary bytes and repair invalid UTF-8 exactly
//! one invalid rune at a time; no HTML or JavaScript escaping is selected.

use std::collections::{HashMap, HashSet};
use std::hash::{BuildHasher, Hash};
use tsr_core::collections::OrderedMap;
use tsr_jsstring::wtf8::{decode_utf8, RUNE_ERROR};
use tsr_jsstring::JsString;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    NonFiniteNumber,
    DuplicateName,
    NestingDepth,
    InvalidIndent,
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for Error {}

/// Explicit options are applied after the wrapper defaults. None means compact;
/// Some("") means multiline with zero indentation (unlike MarshalIndent("", "")).
#[derive(Clone, Debug, Default)]
pub struct Options<'a> {
    pub indent: Option<&'a str>,
    pub prefix: &'a str,
    pub allow_duplicate_names: bool,
    pub deterministic: bool,
}

/// Implementations write their actual storage directly, without building a
/// second JSON tree. Object members and array elements recurse through `value`.
pub trait Encode {
    fn encode(&self, out: &mut Encoder<'_>) -> Result<(), Error>;
}

pub struct Encoder<'a> {
    bytes: Vec<u8>,
    options: Options<'a>,
    depth: usize,
}
impl Encoder<'_> {
    pub fn value(&mut self, value: &(impl Encode + ?Sized)) -> Result<(), Error> {
        stacker::maybe_grow(128 * 1024, 2 * 1024 * 1024, || value.encode(self))
    }
    pub fn null(&mut self) {
        self.bytes.extend_from_slice(b"null");
    }
    pub fn boolean(&mut self, value: bool) {
        self.bytes
            .extend_from_slice(if value { b"true" } else { b"false" });
    }
    pub fn string(&mut self, bytes: &[u8]) {
        append_string(&mut self.bytes, bytes);
    }
    pub fn number(&mut self, value: f64) -> Result<(), Error> {
        if !value.is_finite() {
            return Err(Error::NonFiniteNumber);
        }
        if value == 0.0 && value.is_sign_negative() {
            self.bytes.extend_from_slice(b"-0");
        } else {
            self.bytes
                .extend_from_slice(tsr_jsnum::Number::new(value).to_string().as_bytes());
        }
        Ok(())
    }
    fn begin(&mut self, token: u8) -> Result<(), Error> {
        if self.depth >= 10_000 {
            return Err(Error::NestingDepth);
        }
        self.bytes.push(token);
        self.depth += 1;
        Ok(())
    }
    fn newline(&mut self) {
        if let Some(indent) = self.options.indent {
            self.bytes.push(b'\n');
            self.bytes.extend_from_slice(self.options.prefix.as_bytes());
            for _ in 0..self.depth {
                self.bytes.extend_from_slice(indent.as_bytes());
            }
        }
    }
    pub fn array<'a, T: Encode + ?Sized + 'a>(
        &mut self,
        values: impl IntoIterator<Item = &'a T>,
    ) -> Result<(), Error> {
        self.begin(b'[')?;
        let mut empty = true;
        let result = (|| {
            for value in values {
                if !empty {
                    self.bytes.push(b',');
                }
                self.newline();
                empty = false;
                self.value(value)?;
            }
            Ok(())
        })();
        self.depth -= 1;
        result?;
        if !empty {
            self.newline();
        }
        self.bytes.push(b']');
        Ok(())
    }
    pub fn object<'a, T: Encode + ?Sized + 'a>(
        &mut self,
        entries: impl IntoIterator<Item = (&'a [u8], &'a T)>,
    ) -> Result<(), Error> {
        self.begin(b'{')?;
        let mut names = HashSet::new();
        let mut empty = true;
        let result = (|| {
            for (key, value) in entries {
                if !empty {
                    self.bytes.push(b',');
                }
                self.newline();
                empty = false;
                let start = self.bytes.len();
                self.string(key);
                // Invalid source bytes can repair to the same JSON name even
                // though the original byte keys were distinct.
                if !self.options.allow_duplicate_names
                    && !names.insert(self.bytes[start..].to_vec())
                {
                    return Err(Error::DuplicateName);
                }
                self.bytes.push(b':');
                if self.options.indent.is_some() {
                    self.bytes.push(b' ');
                }
                self.value(value)?;
            }
            Ok(())
        })();
        self.depth -= 1;
        result?;
        if !empty {
            self.newline();
        }
        self.bytes.push(b'}');
        Ok(())
    }
}

/// Typed values only: raw JSON token validation and its partial-error payload
/// are still pending. A failed typed encoding returns no completed document.
/// port: tsc/internal/json/json.go:Marshal
pub fn marshal(value: &(impl Encode + ?Sized), options: Options<'_>) -> Result<Vec<u8>, Error> {
    if !options.prefix.bytes().all(|b| matches!(b, b' ' | b'\t'))
        || options
            .indent
            .is_some_and(|s| !s.bytes().all(|b| matches!(b, b' ' | b'\t')))
    {
        return Err(Error::InvalidIndent);
    }
    let mut out = Encoder {
        bytes: Vec::new(),
        options,
        depth: 0,
    };
    out.value(value)?;
    Ok(out.bytes)
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
            prefix,
            ..Options::default()
        },
    )
}

fn append_string(out: &mut Vec<u8>, bytes: &[u8]) {
    out.push(b'"');
    let mut remaining = bytes;
    while !remaining.is_empty() {
        let (rune, width) = decode_utf8(remaining);
        match rune {
            8 => out.extend_from_slice(b"\\b"),
            9 => out.extend_from_slice(b"\\t"),
            10 => out.extend_from_slice(b"\\n"),
            12 => out.extend_from_slice(b"\\f"),
            13 => out.extend_from_slice(b"\\r"),
            34 => out.extend_from_slice(b"\\\""),
            92 => out.extend_from_slice(b"\\\\"),
            0..=31 => {
                const HEX: &[u8] = b"0123456789abcdef";
                out.extend_from_slice(b"\\u00");
                out.push(HEX[(rune / 16) as usize]);
                out.push(HEX[(rune % 16) as usize]);
            }
            RUNE_ERROR if width == 1 => out.extend_from_slice("�".as_bytes()),
            _ => out.extend_from_slice(&remaining[..width]),
        }
        remaining = &remaining[width..];
    }
    out.push(b'"');
}

impl Encode for JsString {
    fn encode(&self, out: &mut Encoder<'_>) -> Result<(), Error> {
        out.string(self.as_bytes());
        Ok(())
    }
}
impl<T: Encode + ?Sized> Encode for &T {
    fn encode(&self, out: &mut Encoder<'_>) -> Result<(), Error> {
        (*self).encode(out)
    }
}
impl Encode for str {
    fn encode(&self, out: &mut Encoder<'_>) -> Result<(), Error> {
        out.string(self.as_bytes());
        Ok(())
    }
}
impl Encode for String {
    fn encode(&self, out: &mut Encoder<'_>) -> Result<(), Error> {
        self.as_str().encode(out)
    }
}
impl Encode for bool {
    fn encode(&self, out: &mut Encoder<'_>) -> Result<(), Error> {
        out.boolean(*self);
        Ok(())
    }
}
impl Encode for f64 {
    fn encode(&self, out: &mut Encoder<'_>) -> Result<(), Error> {
        out.number(*self)
    }
}
impl Encode for i64 {
    fn encode(&self, out: &mut Encoder<'_>) -> Result<(), Error> {
        out.bytes.extend_from_slice(self.to_string().as_bytes());
        Ok(())
    }
}
impl<T: Encode> Encode for Option<T> {
    fn encode(&self, out: &mut Encoder<'_>) -> Result<(), Error> {
        if let Some(value) = self {
            out.value(value)
        } else {
            out.null();
            Ok(())
        }
    }
}
impl<T: Encode> Encode for [T] {
    fn encode(&self, out: &mut Encoder<'_>) -> Result<(), Error> {
        out.array(self)
    }
}
impl<T: Encode> Encode for Vec<T> {
    fn encode(&self, out: &mut Encoder<'_>) -> Result<(), Error> {
        self.as_slice().encode(out)
    }
}
impl<K: Eq + Hash + AsRef<[u8]>, V: Encode, S: BuildHasher> Encode for OrderedMap<K, V, S> {
    /// String keys; integer/text-marshaler key conversion remains pending.
    /// port: tsc/internal/collections/ordered_map.go:OrderedMap.MarshalJSONTo
    fn encode(&self, out: &mut Encoder<'_>) -> Result<(), Error> {
        out.object(self.entries().map(|(key, value)| (key.as_ref(), value)))
    }
}
impl<K: Eq + Hash + AsRef<[u8]>, V: Encode, S: BuildHasher> Encode for HashMap<K, V, S> {
    fn encode(&self, out: &mut Encoder<'_>) -> Result<(), Error> {
        if out.options.deterministic {
            let mut entries: Vec<_> = self.iter().collect();
            entries.sort_unstable_by(|a, b| a.0.as_ref().cmp(b.0.as_ref()));
            out.object(
                entries
                    .into_iter()
                    .map(|(key, value)| (key.as_ref(), value)),
            )
        } else {
            out.object(self.iter().map(|(key, value)| (key.as_ref(), value)))
        }
    }
}
