use std::collections::HashMap;
use tsr_json::{Decode, DecodeKey, Decoder, Encode, Encoder, Error, Kind, Token};

/// An interface-valued JSON property: Go's `any` decodes numbers as binary64.
/// This is intentionally different from RawValue's lossless number bytes.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum Any {
    #[default]
    Null,
    Boolean(bool),
    Number(f64),
    String(String),
    Array(Vec<Self>),
    Object(HashMap<String, Self>),
}
impl Encode for Any {
    fn encode(&self, out: &mut Encoder<'_>) -> Result<(), Error> {
        match self {
            Self::Null => out.write_token(Token::Null),
            Self::Boolean(v) => out.value(v),
            Self::Number(v) => out.value(v),
            Self::String(v) => out.value(v),
            Self::Array(v) => out.value(v),
            Self::Object(v) => out.value(v),
        }
    }
}
impl Decode for Any {
    fn decode(&mut self, input: &mut Decoder<'_>) -> Result<(), Error> {
        if matches!(self, Self::Null)
            && !matches!(input.peek_kind(), Kind::BeginObject | Kind::BeginArray)
        {
            // A nil Go interface is not assigned a concrete scalar until its
            // JSON syntax has been read. A malformed number/string leaves nil;
            // a valid but overflowing number leaves the decoded infinity.
            let data = input.read_value()?;
            let mut value = match data.first().copied().map(Kind::from_byte) {
                Some(Kind::True | Kind::False) => Self::Boolean(false),
                Some(Kind::Number) => Self::Number(0.0),
                Some(Kind::String) => Self::String(String::new()),
                Some(Kind::Null) => return Ok(()),
                _ => unreachable!("read_value validated a scalar"),
            };
            let result = tsr_json::unmarshal(&data, &mut value, tsr_json::Options::default());
            *self = value;
            return result;
        }
        // Go preserves a concrete value already in an interface. Start with a
        // zero value of the inferred concrete type only when the interface is nil.
        if matches!(self, Self::Null) && input.peek_kind() != Kind::Null {
            *self = match input.peek_kind() {
                Kind::True | Kind::False => Self::Boolean(false),
                Kind::Number => Self::Number(0.0),
                Kind::String => Self::String(String::new()),
                Kind::BeginArray => Self::Array(Vec::new()),
                Kind::BeginObject => Self::Object(HashMap::new()),
                _ => return input.type_error("interface {}"),
            };
        }
        if input.peek_kind() == Kind::Null {
            input.read_token()?;
            *self = Self::Null;
            return Ok(());
        }
        match self {
            Self::Null => unreachable!(),
            Self::Boolean(v) => input.value(v),
            Self::Number(v) => input.value(v),
            Self::String(v) => input.value(v),
            Self::Array(v) => input.value(v),
            Self::Object(v) => input.value(v),
        }
    }
}

macro_rules! uri {
    ($name:ident) => {
        #[derive(Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(pub String);
        impl Encode for $name {
            fn encode(&self, out: &mut Encoder<'_>) -> Result<(), Error> {
                out.value(&self.0)
            }
        }
        impl Decode for $name {
            fn decode(&mut self, input: &mut Decoder<'_>) -> Result<(), Error> {
                input.value(&mut self.0)
            }
        }
        impl tsr_json::Key for $name {
            fn json_key(&self) -> std::borrow::Cow<'_, [u8]> {
                self.0.as_bytes().into()
            }
        }
        impl DecodeKey for $name {
            fn decode_key(bytes: &[u8]) -> Result<Self, Error> {
                String::decode_key(bytes).map(Self)
            }
        }
        impl From<&str> for $name {
            fn from(value: &str) -> Self {
                Self(value.into())
            }
        }
    };
}
uri!(DocumentUri);
uri!(URI);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Null;
impl Encode for Null {
    /// port: tsc/internal/lsp/lsproto/lsp.go:Null.MarshalJSONTo
    fn encode(&self, out: &mut Encoder<'_>) -> Result<(), Error> {
        out.write_token(Token::Null)
    }
}
impl Decode for Null {
    /// port: tsc/internal/lsp/lsproto/lsp.go:Null.UnmarshalJSONFrom
    fn decode(&mut self, input: &mut Decoder<'_>) -> Result<(), Error> {
        let data = input.read_value()?;
        if data == b"null" {
            Ok(())
        } else {
            Err(Error::Message(format!(
                "expected null, got {}",
                String::from_utf8_lossy(&data)
            )))
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct EmptyObject;
impl Encode for EmptyObject {
    fn encode(&self, out: &mut Encoder<'_>) -> Result<(), Error> {
        tsr_jsonrpc::object(out, &[])
    }
}
impl Decode for EmptyObject {
    fn decode(&mut self, input: &mut Decoder<'_>) -> Result<(), Error> {
        crate::codec::structure(input, "struct {}", false, &[], |_, input| {
            input.skip_value()
        })
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct UIntPair(pub [u32; 2]);
impl Encode for UIntPair {
    fn encode(&self, out: &mut Encoder<'_>) -> Result<(), Error> {
        out.write_token(Token::BeginArray)?;
        for value in self.0 {
            out.value(&value)?;
        }
        out.write_token(Token::EndArray)
    }
}
impl Decode for UIntPair {
    fn decode(&mut self, input: &mut Decoder<'_>) -> Result<(), Error> {
        if input.peek_kind() == Kind::Null {
            input.read_token()?;
            *self = Self::default();
            return Ok(());
        }
        if input.peek_kind() != Kind::BeginArray {
            return input.type_error("[2]uint32");
        }
        input.read_token()?;
        let mut count = 0;
        while input.peek_kind() != Kind::EndArray {
            if let Some(value) = self.0.get_mut(count) {
                *value = 0;
                input.value(value)?;
            } else {
                input.skip_value()?;
            }
            count += 1;
        }
        if count < 2 {
            self.0[count..].fill(0);
        }
        input.read_token()?;
        match count.cmp(&2) {
            std::cmp::Ordering::Less => Err(Error::Message("too few array elements".into())),
            std::cmp::Ordering::Greater => Err(Error::Message("too many array elements".into())),
            std::cmp::Ordering::Equal => Ok(()),
        }
    }
}
