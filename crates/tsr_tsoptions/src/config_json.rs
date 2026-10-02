//! Compact JSON at the pinned core.StringifyJson / json.Marshal boundary.
use crate::ConfigValue;
pub use tsr_json::Error as JsonError;
use tsr_json::{Encode, Encoder};

/// Typed nil slices encode as `[]`, and nil interfaces as `null`.
pub fn stringify_json(value: &ConfigValue) -> Result<Vec<u8>, JsonError> {
    stringify_json_indent(value, "", "")
}
/// port: tsc/internal/core/core.go:StringifyJson
pub fn stringify_json_indent(
    value: &ConfigValue,
    prefix: &str,
    indent: &str,
) -> Result<Vec<u8>, JsonError> {
    tsr_json::marshal_indent(value, prefix, indent)
}
impl Encode for ConfigValue {
    fn encode(&self, out: &mut Encoder<'_>) -> Result<(), JsonError> {
        match self {
            Self::Null => out.null(),
            Self::EmptyStruct => out.object(std::iter::empty::<(&[u8], &Self)>()),
            Self::Boolean(value) => value.encode(out),
            Self::Integer(value) => value.encode(out),
            Self::Enum(value) => i64::from(*value).encode(out),
            Self::Number(value) => value.encode(out),
            Self::String(value) => value.encode(out),
            Self::Array(values) => out.array(values.iter().flatten()),
            Self::Object(values) => values.encode(out),
            Self::StringArray(values) => out.array(values.iter().flatten()),
            Self::UnorderedObject(values) => {
                let mut entries: Vec<_> = values.iter().collect();
                entries.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
                out.object(
                    entries
                        .into_iter()
                        .map(|(key, value)| (key.as_bytes(), value)),
                )
            }
        }
    }
}

/// The field order and omitzero rules of tsoptions.TSConfig, used by the
/// production --showConfig path. Empty allocated lists are emitted.
impl Encode for crate::show_config::TsConfig {
    fn encode(&self, out: &mut Encoder<'_>) -> Result<(), JsonError> {
        use tsr_json::Token;
        out.write_token(Token::BeginObject)?;
        out.string(b"compilerOptions")?;
        out.value(&self.compiler_options)?;
        if let Some(references) = &self.references {
            out.string(b"references")?;
            out.write_token(Token::BeginArray)?;
            for (path, circular) in references {
                out.write_token(Token::BeginObject)?;
                out.string(b"path")?;
                out.value(path)?;
                if *circular {
                    out.string(b"circular")?;
                    out.value(circular)?;
                }
                out.write_token(Token::EndObject)?;
            }
            out.write_token(Token::EndArray)?;
        }
        for (name, values) in [
            (b"files".as_slice(), &self.files),
            (b"include", &self.include),
            (b"exclude", &self.exclude),
        ] {
            if let Some(values) = values {
                out.string(name)?;
                out.value(values)?;
            }
        }
        if let Some(value) = self.compile_on_save {
            out.string(b"compileOnSave")?;
            out.value(&value)?;
        }
        out.write_token(Token::EndObject)
    }
}
