//! Generic JSON-RPC 2.0 types shared between the content-mapper protocol and
//! other JSON-RPC based protocols, and the Content-Length base protocol.
//!
//! Params and results stay raw JSON, as the pinned package keeps them, and the
//! codecs follow the pinned `internal/json` (json v2) rules for these fields:
//! case-sensitive names, unknown names skipped, duplicate names rejected, and
//! `omitzero` fields left out when absent.
mod base_proto;

pub use base_proto::{Error as FramingError, Reader, Writer};

use tsr_json::{Decode, Decoder, Encode, Encoder, Error, Kind, RawValue, SemanticError, Token};

/// The JSON-RPC version field, always `"2.0"`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct JsonRpcVersion;

const JSON_RPC_VERSION: &[u8] = br#""2.0""#;

impl Encode for JsonRpcVersion {
    fn type_name(&self) -> &'static str {
        "jsonrpc.JSONRPCVersion"
    }
    /// port: tsc/internal/jsonrpc/jsonrpc.go:JSONRPCVersion.MarshalJSON
    fn encode(&self, out: &mut Encoder<'_>) -> Result<(), Error> {
        out.write_value(JSON_RPC_VERSION)
    }
}

impl Decode for JsonRpcVersion {
    fn type_name() -> &'static str {
        "jsonrpc.JSONRPCVersion"
    }
    /// port: tsc/internal/jsonrpc/jsonrpc.go:JSONRPCVersion.UnmarshalJSON
    fn decode(&mut self, input: &mut Decoder<'_>) -> Result<(), Error> {
        let (offset, pointer) = input.next_location()?;
        let kind = input.peek_kind();
        if input.read_value()? != JSON_RPC_VERSION {
            return Err(custom_error(
                <Self as Decode>::type_name(),
                offset,
                pointer,
                kind,
                "invalid JSON-RPC version",
            ));
        }
        Ok(())
    }
}

/// A codec's own failure, wrapped as json v2 wraps an error a type's
/// unmarshal method returns.
pub fn custom_error(
    type_name: &'static str,
    offset: usize,
    pointer: String,
    kind: Kind,
    message: impl Into<String>,
) -> Error {
    Error::Semantic(Box::new(SemanticError {
        marshal: false,
        offset,
        pointer,
        kind,
        value: None,
        type_name,
        cause: Some(Error::Message(message.into())),
    }))
}

/// A JSON-RPC message ID: a string, or an integer when the string is empty.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct Id {
    string: String,
    integer: i32,
}

/// The raw value an ID is made from.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct IntegerOrString {
    pub integer: Option<i32>,
    pub string: Option<String>,
}

impl Id {
    /// port: tsc/internal/jsonrpc/jsonrpc.go:NewID
    pub fn new(raw: &IntegerOrString) -> Self {
        match &raw.string {
            Some(string) => Self::string(string.clone()),
            None => Self::int(raw.integer.expect("an ID has an integer or a string")),
        }
    }
    /// port: tsc/internal/jsonrpc/jsonrpc.go:NewIDString
    pub fn string(string: impl Into<String>) -> Self {
        Self {
            string: string.into(),
            integer: 0,
        }
    }
    /// port: tsc/internal/jsonrpc/jsonrpc.go:NewIDInt
    pub fn int(integer: i32) -> Self {
        Self {
            string: String::new(),
            integer,
        }
    }
    /// port: tsc/internal/jsonrpc/jsonrpc.go:ID.TryInt
    pub fn try_int(&self) -> Option<i32> {
        self.string.is_empty().then_some(self.integer)
    }
    /// port: tsc/internal/jsonrpc/jsonrpc.go:ID.MustInt
    pub fn must_int(&self) -> i32 {
        assert!(self.string.is_empty(), "ID is not an integer");
        self.integer
    }
}

impl std::fmt::Display for Id {
    /// port: tsc/internal/jsonrpc/jsonrpc.go:ID.String
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.string.is_empty() {
            write!(f, "{}", self.integer)
        } else {
            f.write_str(&self.string)
        }
    }
}

impl Encode for Id {
    fn type_name(&self) -> &'static str {
        "jsonrpc.ID"
    }
    /// port: tsc/internal/jsonrpc/jsonrpc.go:ID.MarshalJSON
    fn encode(&self, out: &mut Encoder<'_>) -> Result<(), Error> {
        if self.string.is_empty() {
            self.integer.encode(out)
        } else {
            self.string.encode(out)
        }
    }
}

impl Decode for Id {
    fn type_name() -> &'static str {
        "jsonrpc.ID"
    }
    /// port: tsc/internal/jsonrpc/jsonrpc.go:ID.UnmarshalJSON
    fn decode(&mut self, input: &mut Decoder<'_>) -> Result<(), Error> {
        *self = Self::default();
        if input.peek_kind() == Kind::String {
            input.value(&mut self.string)
        } else {
            input.value(&mut self.integer)
        }
    }
}

/// A JSON-RPC error response.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ResponseError {
    pub code: i32,
    pub message: String,
    /// Absent when the error carries no data (`omitzero`).
    pub data: Option<RawValue>,
}

impl std::fmt::Display for ResponseError {
    /// port: tsc/internal/jsonrpc/jsonrpc.go:ResponseError.String
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "[{}]: {}", self.code, self.message)
    }
}

/// port: tsc/internal/jsonrpc/jsonrpc.go:ResponseError.Error
impl std::error::Error for ResponseError {}

impl Encode for ResponseError {
    fn type_name(&self) -> &'static str {
        "jsonrpc.ResponseError"
    }
    fn encode(&self, out: &mut Encoder<'_>) -> Result<(), Error> {
        object(
            out,
            &[
                field(b"code", &self.code),
                field(b"message", &self.message),
                optional(b"data", self.data.as_ref()),
            ],
        )
    }
}

impl Decode for ResponseError {
    fn type_name() -> &'static str {
        "jsonrpc.ResponseError"
    }
    fn decode(&mut self, input: &mut Decoder<'_>) -> Result<(), Error> {
        if null(input)? {
            *self = Self::default();
            return Ok(());
        }
        input.object(|name, input| match name {
            b"code" => input.value(&mut self.code),
            b"message" => input.value(&mut self.message),
            b"data" => input.value(&mut self.data),
            _ => input.skip_value(),
        })
    }
}

/// Standard JSON-RPC error codes.
pub const CODE_PARSE_ERROR: i32 = -32700;
pub const CODE_INVALID_REQUEST: i32 = -32600;
pub const CODE_METHOD_NOT_FOUND: i32 = -32601;
pub const CODE_INVALID_PARAMS: i32 = -32602;
pub const CODE_INTERNAL_ERROR: i32 = -32603;

/// What type of message a [`Message`] is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MessageKind {
    Notification,
    Request,
    Response,
}

/// A raw JSON-RPC message: a request, a notification or a response, whose
/// params and result stay raw JSON.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Message {
    pub jsonrpc: JsonRpcVersion,
    pub id: Option<Id>,
    pub method: String,
    pub params: Option<RawValue>,
    pub result: Option<RawValue>,
    pub error: Option<ResponseError>,
}

impl Message {
    /// port: tsc/internal/jsonrpc/jsonrpc.go:Message.Kind
    pub fn kind(&self) -> MessageKind {
        if self.id.is_some() && self.method.is_empty() {
            MessageKind::Response
        } else if self.id.is_none() {
            MessageKind::Notification
        } else {
            MessageKind::Request
        }
    }
    /// port: tsc/internal/jsonrpc/jsonrpc.go:Message.IsRequest
    pub fn is_request(&self) -> bool {
        self.id.is_some() && !self.method.is_empty()
    }
    /// port: tsc/internal/jsonrpc/jsonrpc.go:Message.IsNotification
    pub fn is_notification(&self) -> bool {
        self.id.is_none() && !self.method.is_empty()
    }
    /// port: tsc/internal/jsonrpc/jsonrpc.go:Message.IsResponse
    pub fn is_response(&self) -> bool {
        self.id.is_some() && self.method.is_empty()
    }
    /// The params, empty when absent (a nil `json.Value`).
    pub fn params_bytes(&self) -> &[u8] {
        self.params.as_ref().map_or(&[], |value| &value.0)
    }
}

impl Encode for Message {
    fn type_name(&self) -> &'static str {
        "jsonrpc.Message"
    }
    fn encode(&self, out: &mut Encoder<'_>) -> Result<(), Error> {
        object(
            out,
            &[
                field(b"jsonrpc", &self.jsonrpc),
                optional(b"id", self.id.as_ref()),
                optional(b"method", Some(&self.method).filter(|m| !m.is_empty())),
                omitted(b"params", raw(self.params.as_ref())),
                omitted(b"result", raw(self.result.as_ref())),
                optional(b"error", self.error.as_ref()),
            ],
        )
    }
}

impl Decode for Message {
    fn type_name() -> &'static str {
        "jsonrpc.Message"
    }
    fn decode(&mut self, input: &mut Decoder<'_>) -> Result<(), Error> {
        if null(input)? {
            return Ok(());
        }
        input.object(|name, input| match name {
            b"jsonrpc" => input.value(&mut self.jsonrpc),
            b"id" => input.value(&mut self.id),
            b"method" => input.value(&mut self.method),
            b"params" => raw_field(input, &mut self.params),
            b"result" => raw_field(input, &mut self.result),
            b"error" => input.value(&mut self.error),
            _ => input.skip_value(),
        })
    }
}

/// A request or notification to write; notifications have no ID.
pub struct RequestMessage<'a> {
    pub id: Option<&'a Id>,
    pub method: &'a str,
    /// Omitted when absent (a nil `any`).
    pub params: Option<&'a dyn Encode>,
}

impl Encode for RequestMessage<'_> {
    fn type_name(&self) -> &'static str {
        "jsonrpc.RequestMessage"
    }
    fn encode(&self, out: &mut Encoder<'_>) -> Result<(), Error> {
        object(
            out,
            &[
                field(b"jsonrpc", &JsonRpcVersion),
                optional(b"id", self.id),
                field(b"method", &self.method),
                omitted(b"params", self.params),
            ],
        )
    }
}

/// A response to write: a result, or an error.
pub struct ResponseMessage<'a> {
    pub id: Option<&'a Id>,
    /// Omitted when absent (a nil `any`).
    pub result: Option<&'a dyn Encode>,
    pub error: Option<&'a ResponseError>,
}

impl Encode for ResponseMessage<'_> {
    fn type_name(&self) -> &'static str {
        "jsonrpc.ResponseMessage"
    }
    fn encode(&self, out: &mut Encoder<'_>) -> Result<(), Error> {
        object(
            out,
            &[
                field(b"jsonrpc", &JsonRpcVersion),
                optional(b"id", self.id),
                omitted(b"result", self.result),
                optional(b"error", self.error),
            ],
        )
    }
}

/// One struct field to encode: its JSON name and its value, `None` when the
/// field is left out (`omitzero`).
pub type Field<'a> = (&'a [u8], Option<&'a dyn Encode>);

pub fn field<'a>(name: &'a [u8], value: &'a dyn Encode) -> Field<'a> {
    (name, Some(value))
}

pub fn optional<'a, T: Encode>(name: &'a [u8], value: Option<&'a T>) -> Field<'a> {
    (name, value.map(|value| value as &dyn Encode))
}

pub fn omitted<'a>(name: &'a [u8], value: Option<&'a dyn Encode>) -> Field<'a> {
    (name, value)
}

/// Writes an object from its fields in order, leaving out absent ones as
/// `omitzero` does.
pub fn object(out: &mut Encoder<'_>, fields: &[Field<'_>]) -> Result<(), Error> {
    out.write_token(Token::BeginObject)?;
    for (name, value) in fields {
        if let Some(value) = value {
            out.string(name)?;
            out.value(*value)?;
        }
    }
    out.write_token(Token::EndObject)
}

/// Consumes a `null` in place of a struct, which leaves the destination as it is.
pub fn null(input: &mut Decoder<'_>) -> Result<bool, Error> {
    if input.peek_kind() == Kind::Null {
        input.read_token()?;
        return Ok(true);
    }
    Ok(false)
}

/// A raw `json.Value` field: an empty value is the zero value and is omitted.
pub fn raw(value: Option<&RawValue>) -> Option<&dyn Encode> {
    value
        .filter(|value| !value.0.is_empty())
        .map(|value| value as &dyn Encode)
}

/// Decodes a raw `json.Value` field, which keeps `null` as its bytes.
pub fn raw_field(input: &mut Decoder<'_>, value: &mut Option<RawValue>) -> Result<(), Error> {
    let mut raw = RawValue::default();
    input.value(&mut raw)?;
    *value = Some(raw);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn marshal(value: &dyn Encode) -> String {
        String::from_utf8(tsr_json::marshal(value, tsr_json::Options::default()).unwrap()).unwrap()
    }

    fn unmarshal(bytes: &[u8]) -> Result<Message, Error> {
        let mut message = Message::default();
        tsr_json::unmarshal(bytes, &mut message, tsr_json::Options::default())?;
        Ok(message)
    }

    #[test]
    fn requests_and_responses_encode_their_present_fields_in_order() {
        let id = Id::string("api1");
        let params = RawValue(br#"{"a":1}"#.to_vec());
        assert_eq!(
            marshal(&RequestMessage {
                id: Some(&id),
                method: "initialize",
                params: Some(&params)
            }),
            r#"{"jsonrpc":"2.0","id":"api1","method":"initialize","params":{"a":1}}"#
        );
        assert_eq!(
            marshal(&RequestMessage {
                id: None,
                method: "exit",
                params: None
            }),
            r#"{"jsonrpc":"2.0","method":"exit"}"#
        );
        let null = RawValue(b"null".to_vec());
        assert_eq!(
            marshal(&ResponseMessage {
                id: Some(&Id::int(3)),
                result: Some(&null),
                error: None
            }),
            r#"{"jsonrpc":"2.0","id":3,"result":null}"#
        );
        let error = ResponseError {
            code: CODE_INTERNAL_ERROR,
            message: "boom".into(),
            data: None,
        };
        assert_eq!(
            marshal(&ResponseMessage {
                id: Some(&id),
                result: None,
                error: Some(&error)
            }),
            r#"{"jsonrpc":"2.0","id":"api1","error":{"code":-32603,"message":"boom"}}"#
        );
        assert_eq!(error.to_string(), "[-32603]: boom");
        // An empty string ID is the integer zero, as in the pin.
        assert_eq!(marshal(&Id::string("")), "0");
    }

    #[test]
    fn messages_decode_and_classify() {
        let request = unmarshal(br#"{"jsonrpc":"2.0","id":"api1","method":"transform","params":{"x":[1]},"extra":true}"#).unwrap();
        assert_eq!(request.kind(), MessageKind::Request);
        assert!(request.is_request());
        assert_eq!(request.params_bytes(), br#"{"x":[1]}"#);
        assert_eq!(request.id, Some(Id::string("api1")));
        let response = unmarshal(br#"{"jsonrpc":"2.0","id":7,"result":null}"#).unwrap();
        assert!(response.is_response());
        assert_eq!(response.id.as_ref().and_then(Id::try_int), Some(7));
        assert_eq!(response.result, Some(RawValue(b"null".to_vec())));
        let notification = unmarshal(br#"{"jsonrpc":"2.0","method":"log"}"#).unwrap();
        assert!(notification.is_notification());
        assert_eq!(notification.kind(), MessageKind::Notification);
        let failed =
            unmarshal(br#"{"jsonrpc":"2.0","id":"a","error":{"code":-32603,"message":"m"}}"#)
                .unwrap();
        assert_eq!(failed.error.unwrap().message, "m");
        assert!(unmarshal(br#"{"jsonrpc":"1.0","id":1}"#).is_err());
        assert!(unmarshal(br#"{"jsonrpc":"2.0","id":1,"id":2}"#).is_err());
        assert_eq!(
            marshal(&request),
            r#"{"jsonrpc":"2.0","id":"api1","method":"transform","params":{"x":[1]}}"#
        );
    }
}
