//! Handwritten codecs for the export's special mappings
//! (`data/s03/api-special-codecs.json`) and the json v2 struct rule the
//! generated DTOs share.
use std::collections::BTreeMap;
use tsr_json::{Decode, DecodeKey, Decoder, Encode, Encoder, Error, Key, Kind, RawValue, Token};
use tsr_jsstring::JsString;
use tsr_lsproto::DocumentUri;

/// json v2 struct decoding after the caller has mapped a null to the zero
/// value: the value must be an object, known names decode in place, unknown
/// names are skipped, and duplicate names are the decoder's error.
pub(crate) fn structure(
    input: &mut Decoder<'_>,
    type_name: &'static str,
    names: &[&str],
    mut field: impl FnMut(usize, &mut Decoder<'_>) -> Result<(), Error>,
) -> Result<(), Error> {
    if input.peek_kind() != Kind::BeginObject {
        return input.type_error(type_name);
    }
    input.object(
        |name, input| match names.iter().position(|known| known.as_bytes() == name) {
            Some(index) => field(index, input),
            None => input.skip_value(),
        },
    )
}

/// Whether json v2 `omitempty` leaves a raw value out: absent, or encoding as
/// null, `""`, `{}` or `[]`.
pub trait RawEmpty {
    fn raw_is_empty(&self) -> bool;
}
impl RawEmpty for RawValue {
    /// Decided on the encoded value: whitespace between tokens is not part
    /// of it, a string's content is, so `" "` is a value and `{ }` is not.
    fn raw_is_empty(&self) -> bool {
        let mut decoder = Decoder::from_slice(&self.0);
        match decoder.peek_kind() {
            Kind::Null => true,
            Kind::String => {
                matches!(decoder.read_token(), Ok(Token::String(text)) if text.is_empty())
            }
            Kind::BeginObject => {
                decoder.read_token().is_ok() && decoder.peek_kind() == Kind::EndObject
            }
            Kind::BeginArray => {
                decoder.read_token().is_ok() && decoder.peek_kind() == Kind::EndArray
            }
            // An absent value, or no token at all.
            Kind::Invalid => self
                .0
                .iter()
                .all(|byte| matches!(byte, b' ' | b'\t' | b'\r' | b'\n')),
            _ => false,
        }
    }
}
impl<T: RawEmpty> RawEmpty for Option<T> {
    fn raw_is_empty(&self) -> bool {
        self.as_ref().is_none_or(RawEmpty::raw_is_empty)
    }
}
pub fn raw_is_empty(value: &impl RawEmpty) -> bool {
    value.raw_is_empty()
}

/// `api.DocumentIdentifier`: a file name or a `{ uri }` object on the wire.
/// The pin's `DocumentIdentifier` of tsc/internal/api/proto.go.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct DocumentIdentifier {
    pub file_name: String,
    pub uri: String,
}
impl DocumentIdentifier {
    #[must_use]
    pub fn file(name: &str) -> Self {
        Self {
            file_name: name.into(),
            uri: String::new(),
        }
    }
    #[must_use]
    pub fn is_json_empty(&self) -> bool {
        self.file_name.is_empty() && self.uri.is_empty()
    }
    /// port: tsc/internal/api/proto.go:DocumentIdentifier.ToFileName
    #[must_use]
    pub fn to_file_name(&self) -> JsString {
        if !self.uri.is_empty() {
            return DocumentUri(self.uri.clone()).file_name();
        }
        JsString::from_bytes(self.file_name.as_bytes())
    }
    /// port: tsc/internal/api/proto.go:DocumentIdentifier.ToURI
    #[must_use]
    pub fn to_uri(&self, cwd: &[u8]) -> DocumentUri {
        if !self.uri.is_empty() {
            return DocumentUri(self.uri.clone());
        }
        DocumentUri::from_file_name(&tsr_tspath::absolute(self.file_name.as_bytes(), cwd))
    }
    /// port: tsc/internal/api/proto.go:DocumentIdentifier.ToAbsoluteFileName
    #[must_use]
    pub fn to_absolute_file_name(&self, cwd: &[u8]) -> JsString {
        if !self.uri.is_empty() {
            return DocumentUri(self.uri.clone()).file_name();
        }
        JsString::from_bytes(tsr_tspath::absolute(self.file_name.as_bytes(), cwd))
    }
}
impl std::fmt::Display for DocumentIdentifier {
    /// port: tsc/internal/api/proto.go:DocumentIdentifier.String
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.uri.is_empty() {
            f.write_str(&self.file_name)
        } else {
            f.write_str(&self.uri)
        }
    }
}
impl Encode for DocumentIdentifier {
    fn type_name(&self) -> &'static str {
        "api.DocumentIdentifier"
    }
    fn encode(&self, out: &mut Encoder<'_>) -> Result<(), Error> {
        tsr_jsonrpc::object(
            out,
            &[
                tsr_jsonrpc::omitted(
                    b"fileName",
                    (!self.file_name.is_empty()).then_some(&self.file_name as &dyn Encode),
                ),
                tsr_jsonrpc::omitted(
                    b"uri",
                    (!self.uri.is_empty()).then_some(&self.uri as &dyn Encode),
                ),
            ],
        )
    }
}
impl Decode for DocumentIdentifier {
    fn type_name() -> &'static str {
        "api.DocumentIdentifier"
    }
    fn custom_unmarshal() -> bool {
        true
    }
    /// port: tsc/internal/api/proto.go:DocumentIdentifier.UnmarshalJSONFrom
    fn decode(&mut self, input: &mut Decoder<'_>) -> Result<(), Error> {
        match input.peek_kind() {
            Kind::String => {
                let mut name = String::new();
                input.value(&mut name)?;
                *self = Self::file(&name);
                Ok(())
            }
            Kind::BeginObject => {
                *self = Self::default();
                input.object(|key, input| {
                    if key == b"uri" {
                        input.value(&mut self.uri)
                    } else {
                        input.skip_value()
                    }
                })
            }
            kind => Err(Error::Message(format!(
                "DocumentIdentifier: expected string or object, got {}",
                kind.name()
            ))),
        }
    }
}

/// `api.ImportAdderActionKind`, whose only value is `importSymbol`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct ImportAdderActionKind(pub String);
impl ImportAdderActionKind {
    pub const IMPORT_SYMBOL: &'static str = "importSymbol";
    #[must_use]
    pub fn is_json_empty(&self) -> bool {
        self.0.is_empty()
    }
}
impl Encode for ImportAdderActionKind {
    fn type_name(&self) -> &'static str {
        "api.ImportAdderActionKind"
    }
    fn encode(&self, out: &mut Encoder<'_>) -> Result<(), Error> {
        out.string(self.0.as_bytes())
    }
}
impl Decode for ImportAdderActionKind {
    fn type_name() -> &'static str {
        "api.ImportAdderActionKind"
    }
    fn decode(&mut self, input: &mut Decoder<'_>) -> Result<(), Error> {
        input.value(&mut self.0)
    }
}

/// Bytes a response carries verbatim on the msgpack protocol (the pin's
/// `RawBinary`): encoded source files and synthesized nodes. It is never
/// JSON; the JSON-RPC protocol only ever sees the base64 DTOs.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RawBinary(pub Vec<u8>);

/// `core.CompilerOptions` with the pin's struct-tag codec.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CompilerOptionsValue(pub tsr_core::CompilerOptions);
impl CompilerOptionsValue {
    #[must_use]
    pub fn is_json_empty(&self) -> bool {
        self.0 == tsr_core::CompilerOptions::default()
    }
}
impl Encode for CompilerOptionsValue {
    fn type_name(&self) -> &'static str {
        "core.CompilerOptions"
    }
    fn encode(&self, out: &mut Encoder<'_>) -> Result<(), Error> {
        tsr_tsoptions::options_json::CompilerOptionsJson(&self.0).encode(out)
    }
}
impl Decode for CompilerOptionsValue {
    fn type_name() -> &'static str {
        "core.CompilerOptions"
    }
    fn decode(&mut self, input: &mut Decoder<'_>) -> Result<(), Error> {
        tsr_tsoptions::options_json::decode_compiler_options(&mut self.0, input)
    }
}

/// `core.ProjectReference`: `path`, `originalPath` and `circular`, always.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectReferenceValue(pub tsr_tsoptions::ProjectReference);
impl Default for ProjectReferenceValue {
    fn default() -> Self {
        Self(tsr_tsoptions::ProjectReference {
            path: JsString::default(),
            original_path: JsString::default(),
            circular: false,
        })
    }
}
impl ProjectReferenceValue {
    #[must_use]
    pub fn is_json_empty(&self) -> bool {
        false
    }
}
impl Encode for ProjectReferenceValue {
    fn type_name(&self) -> &'static str {
        "core.ProjectReference"
    }
    fn encode(&self, out: &mut Encoder<'_>) -> Result<(), Error> {
        tsr_jsonrpc::object(
            out,
            &[
                tsr_jsonrpc::field(b"path", &self.0.path),
                tsr_jsonrpc::field(b"originalPath", &self.0.original_path),
                tsr_jsonrpc::field(b"circular", &self.0.circular),
            ],
        )
    }
}
impl Decode for ProjectReferenceValue {
    fn type_name() -> &'static str {
        "core.ProjectReference"
    }
    fn decode(&mut self, input: &mut Decoder<'_>) -> Result<(), Error> {
        if input.peek_kind() == Kind::Null {
            input.read_token()?;
            *self = Self::default();
            return Ok(());
        }
        structure(
            input,
            <Self as Decode>::type_name(),
            &["path", "originalPath", "circular"],
            |index, input| match index {
                0 => input.value(&mut self.0.path),
                1 => input.value(&mut self.0.original_path),
                2 => input.value(&mut self.0.circular),
                _ => input.skip_value(),
            },
        )
    }
}

/// `core.TypeAcquisition`: every field `omitzero`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TypeAcquisitionValue(pub tsr_tsoptions::TypeAcquisition);
impl TypeAcquisitionValue {
    #[must_use]
    pub fn is_json_empty(&self) -> bool {
        self.0 == tsr_tsoptions::TypeAcquisition::default()
    }
}
impl Encode for TypeAcquisitionValue {
    fn type_name(&self) -> &'static str {
        "core.TypeAcquisition"
    }
    fn encode(&self, out: &mut Encoder<'_>) -> Result<(), Error> {
        let value = &self.0;
        tsr_jsonrpc::object(
            out,
            &[
                tsr_jsonrpc::omitted(
                    b"enable",
                    (!value.enable.is_unknown()).then_some(&value.enable as &dyn Encode),
                ),
                tsr_jsonrpc::optional(b"include", value.include.as_ref()),
                tsr_jsonrpc::optional(b"exclude", value.exclude.as_ref()),
                tsr_jsonrpc::omitted(
                    b"disableFilenameBasedTypeAcquisition",
                    (!value.disable_filename_based_type_acquisition.is_unknown())
                        .then_some(&value.disable_filename_based_type_acquisition as &dyn Encode),
                ),
            ],
        )
    }
}
impl Decode for TypeAcquisitionValue {
    fn type_name() -> &'static str {
        "core.TypeAcquisition"
    }
    fn decode(&mut self, input: &mut Decoder<'_>) -> Result<(), Error> {
        if input.peek_kind() == Kind::Null {
            input.read_token()?;
            *self = Self::default();
            return Ok(());
        }
        structure(
            input,
            <Self as Decode>::type_name(),
            &[
                "enable",
                "include",
                "exclude",
                "disableFilenameBasedTypeAcquisition",
            ],
            |index, input| match index {
                0 => input.value(&mut self.0.enable),
                1 => input.value(&mut self.0.include),
                2 => input.value(&mut self.0.exclude),
                3 => input.value(&mut self.0.disable_filename_based_type_acquisition),
                _ => input.skip_value(),
            },
        )
    }
}

/// `collections.OrderedMap[string, []string]`: insertion order on the wire; a
/// nil list is `[]`, as json v2 writes nil slices.
#[derive(Clone, Debug, Default)]
pub struct StringListMap(pub tsr_core::collections::OrderedMap<String, Vec<String>>);
impl PartialEq for StringListMap {
    fn eq(&self, other: &Self) -> bool {
        self.0.len() == other.0.len() && self.0.entries().eq(other.0.entries())
    }
}
impl StringListMap {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.len() == 0
    }
}
impl Encode for StringListMap {
    fn type_name(&self) -> &'static str {
        "collections.OrderedMap[string, []string]"
    }
    fn encode(&self, out: &mut Encoder<'_>) -> Result<(), Error> {
        out.write_token(Token::BeginObject)?;
        for (key, values) in self.0.entries() {
            out.string(key.as_bytes())?;
            out.value(values)?;
        }
        out.write_token(Token::EndObject)
    }
}
impl Decode for StringListMap {
    fn type_name() -> &'static str {
        "collections.OrderedMap[string, []string]"
    }
    fn decode(&mut self, input: &mut Decoder<'_>) -> Result<(), Error> {
        if input.peek_kind() == Kind::Null {
            input.read_token()?;
            *self = Self::default();
            return Ok(());
        }
        let mut map = tsr_core::collections::OrderedMap::default();
        input.object(|key, input| {
            let mut values = Vec::new();
            input.value(&mut values)?;
            let key = String::from_utf8(key.to_vec())
                .map_err(|_| Error::Message("object key is not UTF-8".into()))?;
            if map.insert(key.clone(), values).is_some() {
                return Err(Error::Message(format!("duplicate name {key:?} in object")));
            }
            Ok(())
        })?;
        self.0 = map;
        Ok(())
    }
}

/// A Go map on the wire. Keys are written in sorted order, which json v2 does
/// deterministically only on request; the clients read these as objects.
#[derive(Clone, Debug, PartialEq)]
pub struct JsonMap<K, V>(pub BTreeMap<K, V>);
impl<K, V> Default for JsonMap<K, V> {
    fn default() -> Self {
        Self(BTreeMap::new())
    }
}
impl<K, V> JsonMap<K, V> {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}
impl<K: Key, V: Encode> Encode for JsonMap<K, V> {
    fn type_name(&self) -> &'static str {
        "map"
    }
    fn encode(&self, out: &mut Encoder<'_>) -> Result<(), Error> {
        out.write_token(Token::BeginObject)?;
        for (key, value) in &self.0 {
            out.string(&key.json_key())?;
            out.value(value)?;
        }
        out.write_token(Token::EndObject)
    }
}
impl<K: DecodeKey + Ord, V: Decode + Default> Decode for JsonMap<K, V> {
    fn type_name() -> &'static str {
        "map"
    }
    fn decode(&mut self, input: &mut Decoder<'_>) -> Result<(), Error> {
        if input.peek_kind() == Kind::Null {
            input.read_token()?;
            self.0.clear();
            return Ok(());
        }
        let mut map = BTreeMap::new();
        input.object(|key, input| {
            let mut value = V::default();
            input.value(&mut value)?;
            if map.insert(K::decode_key(key)?, value).is_some() {
                return Err(Error::Message(format!(
                    "duplicate name {:?} in object",
                    String::from_utf8_lossy(key)
                )));
            }
            Ok(())
        })?;
        self.0 = map;
        Ok(())
    }
}

/// `packagejson.JSONValue`: any JSON value, kept raw, classified as the pin's
/// `JSONValueType` names it.
/// port: tsc/internal/packagejson/jsonvalue.go:JSONValue.UnmarshalJSONFrom
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PackageJsonValue(pub RawValue);
impl PackageJsonValue {
    /// port: tsc/internal/packagejson/jsonvalue.go:JSONValueType.String
    #[must_use]
    pub fn kind_name(&self) -> &'static str {
        match self.0.kind() {
            Kind::Null => "null",
            Kind::False | Kind::True => "boolean",
            Kind::String => "string",
            Kind::Number => "number",
            Kind::BeginObject => "object",
            Kind::BeginArray => "array",
            _ => "invalid",
        }
    }
    #[must_use]
    pub fn is_json_empty(&self) -> bool {
        self.0.raw_is_empty()
    }
}
impl Encode for PackageJsonValue {
    fn type_name(&self) -> &'static str {
        "packagejson.JSONValue"
    }
    fn encode(&self, out: &mut Encoder<'_>) -> Result<(), Error> {
        if self.0 .0.is_empty() {
            return out.null();
        }
        self.0.encode(out)
    }
}
impl Decode for PackageJsonValue {
    fn type_name() -> &'static str {
        "packagejson.JSONValue"
    }
    fn decode(&mut self, input: &mut Decoder<'_>) -> Result<(), Error> {
        self.0 = RawValue(input.read_value()?);
        Ok(())
    }
}
