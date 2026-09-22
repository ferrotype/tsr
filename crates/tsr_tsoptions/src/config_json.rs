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
