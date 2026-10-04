//! The pinned protocol's resolved LSP types, including its Corsa extensions.
//! Generated codecs preserve required/nullable fields and union dispatch;
//! JSON-RPC messages and Content-Length framing are shared with `tsr_jsonrpc`.
mod codec;
mod generated;
mod uri;
mod values;
pub use generated::*;
pub use tsr_jsonrpc::{FramingError, Id, Message, Reader, RequestMessage, ResponseError, Writer};
pub use values::{Any, DocumentUri, EmptyObject, Null, UIntPair, URI};

use std::marker::PhantomData;
use tsr_json::{Decode, Decoder, Encode, Encoder, Error, Kind, Options, RawValue};

/// LSP responses always carry an ID, including null for a parse error. The
/// pin's lsp/lsproto/jsonrpc.go differs from generic jsonrpc.go at this field.
pub struct ResponseMessage<'a> {
    pub id: Option<&'a Id>,
    pub result: Option<&'a dyn Encode>,
    pub error: Option<&'a ResponseError>,
}
impl Encode for ResponseMessage<'_> {
    fn type_name(&self) -> &'static str {
        "lsproto.ResponseMessage"
    }
    fn encode(&self, out: &mut Encoder<'_>) -> Result<(), Error> {
        tsr_jsonrpc::object(
            out,
            &[
                tsr_jsonrpc::field(b"jsonrpc", &tsr_jsonrpc::JsonRpcVersion),
                tsr_jsonrpc::field(b"id", &self.id),
                tsr_jsonrpc::omitted(b"result", self.result),
                tsr_jsonrpc::optional(b"error", self.error),
            ],
        )
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct NoParams;
impl Decode for NoParams {
    fn decode(&mut self, _input: &mut Decoder<'_>) -> Result<(), Error> {
        Err(Error::Message("expected no params".into()))
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Request<P, R> {
    pub method: &'static str,
    types: PhantomData<fn(P) -> R>,
}
impl<P, R> Request<P, R> {
    pub const fn new(method: &'static str) -> Self {
        Self {
            method,
            types: PhantomData,
        }
    }
}
#[derive(Clone, Copy, Debug)]
pub struct Notification<P> {
    pub method: &'static str,
    types: PhantomData<fn(P)>,
}
impl<P> Notification<P> {
    pub const fn new(method: &'static str) -> Self {
        Self {
            method,
            types: PhantomData,
        }
    }
}

/// port: tsc/internal/lsp/lsproto/lsp.go:UnmarshalParams
pub fn unmarshal_params<T: Decode + Default + 'static>(
    params: Option<&RawValue>,
) -> Result<T, ResponseError> {
    let mut value = T::default();
    let invalid = |message: String| ResponseError {
        code: ErrorCode::INVALID_PARAMS.0,
        message: format!("{}: {message}", ErrorCode::INVALID_PARAMS),
        data: None,
    };
    let raw = params.map_or(&[][..], |p| p.0.as_slice());
    if std::any::TypeId::of::<T>() == std::any::TypeId::of::<NoParams>() {
        if raw.is_empty() {
            return Ok(value);
        }
        return Err(invalid(format!(
            "expected no params, got {}",
            String::from_utf8_lossy(raw)
        )));
    }
    if !matches!(
        params.map(RawValue::kind),
        Some(Kind::BeginObject | Kind::BeginArray)
    ) {
        return Err(invalid("params must be an object or array".into()));
    }
    tsr_json::unmarshal(raw, &mut value, Options::default()).map_err(|e| invalid(e.to_string()))?;
    Ok(value)
}

/// Registration is externally tagged by `method`. The pin deliberately has no
/// independent writable method field that can disagree with register_options.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Registration {
    pub id: String,
    pub register_options: Option<Box<RegisterOptions>>,
}
impl Encode for Registration {
    fn encode(&self, out: &mut Encoder<'_>) -> Result<(), Error> {
        let (method, options) = self
            .register_options
            .as_ref()
            .expect("registration options must be set")
            .selected();
        tsr_jsonrpc::object(
            out,
            &[
                tsr_jsonrpc::field(b"id", &self.id),
                tsr_jsonrpc::field(b"method", &method),
                tsr_jsonrpc::field(b"registerOptions", options),
            ],
        )
    }
}
impl Decode for Registration {
    fn type_name() -> &'static str {
        "lsproto.Registration"
    }
    fn custom_unmarshal() -> bool {
        true
    }
    fn decode(&mut self, input: &mut Decoder<'_>) -> Result<(), Error> {
        *self = Self::default();
        let mut method = String::new();
        let mut options = RawValue::default();
        codec::structure(
            input,
            "lsproto.Registration",
            true,
            &[
                codec::Field {
                    name: "id",
                    required_bit: Some(0),
                    reject_null: false,
                },
                codec::Field {
                    name: "method",
                    required_bit: Some(1),
                    reject_null: false,
                },
                codec::Field {
                    name: "registerOptions",
                    required_bit: None,
                    reject_null: false,
                },
            ],
            |i, input| match i {
                0 => input.value(&mut self.id),
                1 => input.value(&mut method),
                2 => input.value(&mut options),
                _ => unreachable!(),
            },
        )?;
        if options.0.is_empty() {
            return Err(Error::Message(format!(
                "missing registerOptions for method: {method}"
            )));
        }
        self.register_options
            .insert(Box::default())
            .decode_method(&method, &options.0)
    }
}
