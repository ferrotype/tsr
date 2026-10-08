//! The JSON plumbing the build-info codecs share: `omitzero` struct fields,
//! the pin's `any` values, and the `json.Unmarshal` attempts its custom
//! decoders chain.
use tsr_json::{Decode, Decoder, Encode, Encoder, Error as JsonError, Kind, Options, Token};
use tsr_jsstring::JsString;
use tsr_tsoptions::ConfigValue;

/// One struct being encoded: its fields in declaration order, an `omitzero`
/// field only when it is not its type's zero value.
pub(crate) struct Object<'e, 'a> {
    out: &'e mut Encoder<'a>,
}

impl<'e, 'a> Object<'e, 'a> {
    pub(crate) fn begin(out: &'e mut Encoder<'a>) -> Result<Self, JsonError> {
        out.write_token(Token::BeginObject)?;
        Ok(Self { out })
    }

    pub(crate) fn field(
        &mut self,
        name: &[u8],
        value: &(impl Encode + ?Sized),
    ) -> Result<(), JsonError> {
        self.out.string(name)?;
        self.out.value(value)
    }

    pub(crate) fn string_omitzero(
        &mut self,
        name: &[u8],
        value: &JsString,
    ) -> Result<(), JsonError> {
        if value.is_empty() {
            return Ok(());
        }
        self.field(name, value)
    }

    pub(crate) fn bool_omitzero(&mut self, name: &[u8], value: bool) -> Result<(), JsonError> {
        if !value {
            return Ok(());
        }
        self.field(name, &value)
    }

    pub(crate) fn int_omitzero(&mut self, name: &[u8], value: i64) -> Result<(), JsonError> {
        if value == 0 {
            return Ok(());
        }
        self.field(name, &value)
    }

    /// A nullable slice or pointer tagged `omitzero`.
    pub(crate) fn option_omitzero<T: Encode + ?Sized>(
        &mut self,
        name: &[u8],
        value: Option<&T>,
    ) -> Result<(), JsonError> {
        if let Some(value) = value {
            self.field(name, value)?;
        }
        Ok(())
    }

    pub(crate) fn end(self) -> Result<(), JsonError> {
        self.out.write_token(Token::EndObject)
    }
}

/// A struct's fields: unknown names are skipped and `null` leaves the
/// destination as it is, as the pin's unmarshal into a struct does.
pub(crate) fn object(
    input: &mut Decoder<'_>,
    field: impl FnMut(&[u8], &mut Decoder<'_>) -> Result<(), JsonError>,
) -> Result<(), JsonError> {
    if input.peek_kind() == Kind::Null {
        input.read_token()?;
        return Ok(());
    }
    input.object(field)
}

/// `json.Unmarshal(data, &value)` into a fresh value.
pub(crate) fn unmarshal<T: Decode + Default>(data: &[u8]) -> Result<T, JsonError> {
    let mut value = T::default();
    tsr_json::unmarshal(data, &mut value, Options::default())?;
    Ok(value)
}

/// `json.Unmarshal(data, &pair)` into a `[2]int`.
pub(crate) fn unmarshal_pair(data: &[u8]) -> Result<[i64; 2], JsonError> {
    let mut decoder = Decoder::from_slice(data);
    if decoder.peek_kind() != Kind::BeginArray {
        return decoder.type_error("[2]int").map(|()| [0, 0]);
    }
    decoder.read_token()?;
    let mut pair = [0_i64; 2];
    let mut length = 0;
    while decoder.peek_kind() != Kind::EndArray {
        if length == 2 {
            return Err(JsonError::Message(
                "cannot unmarshal JSON array into Go [2]int: too many elements".into(),
            ));
        }
        decoder.value(&mut pair[length])?;
        length += 1;
    }
    decoder.read_token()?;
    decoder.finish()?;
    Ok(pair)
}

/// The `fmt.Errorf("invalid <type>: %s", data)` of a failed custom decode.
pub(crate) fn invalid(type_name: &str, data: &[u8]) -> JsonError {
    JsonError::Message(format!(
        "invalid {type_name}: {}",
        String::from_utf8_lossy(data)
    ))
}

/// The Go type name `%T` prints for a decoded `any`.
pub(crate) fn go_type_name(value: &ConfigValue) -> &'static str {
    match value {
        ConfigValue::Null => "<nil>",
        ConfigValue::Boolean(_) => "bool",
        ConfigValue::Number(_) | ConfigValue::Integer(_) | ConfigValue::Enum(_) => "float64",
        ConfigValue::String(_) => "string",
        ConfigValue::Array(_) | ConfigValue::StringArray(_) => "[]interface {}",
        ConfigValue::Object(_) | ConfigValue::UnorderedObject(_) | ConfigValue::EmptyStruct => {
            "map[string]interface {}"
        }
    }
}

/// A Go `any`: an option value of the build info's `options`, which the
/// pin marshals from the option's Go value and unmarshals as `nil`, `bool`,
/// `float64`, `string`, `[]any` or `map[string]any`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AnyValue(pub ConfigValue);

impl Encode for AnyValue {
    fn encode(&self, out: &mut Encoder<'_>) -> Result<(), JsonError> {
        self.0.encode(out)
    }
}

impl Decode for AnyValue {
    fn decode(&mut self, input: &mut Decoder<'_>) -> Result<(), JsonError> {
        self.0 = decode_any(input)?;
        Ok(())
    }
}

fn decode_any(input: &mut Decoder<'_>) -> Result<ConfigValue, JsonError> {
    Ok(match input.peek_kind() {
        Kind::Null => {
            input.read_token()?;
            ConfigValue::Null
        }
        Kind::True | Kind::False => {
            ConfigValue::Boolean(input.read_token()? == Token::Boolean(true))
        }
        Kind::Number => {
            let mut number = 0.0_f64;
            input.value(&mut number)?;
            ConfigValue::Number(number)
        }
        Kind::String => {
            let mut text = JsString::default();
            input.value(&mut text)?;
            ConfigValue::String(text)
        }
        Kind::BeginArray => {
            input.read_token()?;
            let mut values = Vec::new();
            while input.peek_kind() != Kind::EndArray {
                let mut value = AnyValue::default();
                input.value(&mut value)?;
                values.push(value.0);
            }
            input.read_token()?;
            ConfigValue::Array(Some(values))
        }
        Kind::BeginObject => {
            let mut values = std::collections::HashMap::new();
            input.object(|name, input| {
                let mut value = AnyValue::default();
                input.value(&mut value)?;
                values.insert(JsString::from_bytes(name), value.0);
                Ok(())
            })?;
            ConfigValue::UnorderedObject(values)
        }
        _ => {
            input.type_error("interface {}")?;
            ConfigValue::Null
        }
    })
}
