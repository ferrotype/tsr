//! Borrowed views of package fields; the parsed contents remain immutable.
use super::{
    field_kind, mapper_kind, ContentMapper, Expected, ExpectedKind, ExpectedState, Fields, Value,
};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    fmt,
    hash::BuildHasher,
};

pub trait DeclaredJsonType {
    const JSON_TYPE: &'static str;
}
impl DeclaredJsonType for String {
    const JSON_TYPE: &'static str = "string";
}
impl DeclaredJsonType for bool {
    const JSON_TYPE: &'static str = "boolean";
}
impl<T> DeclaredJsonType for Vec<T> {
    const JSON_TYPE: &'static str = "array";
}
impl<T, const N: usize> DeclaredJsonType for [T; N] {
    const JSON_TYPE: &'static str = "array";
}
impl<K, V> DeclaredJsonType for BTreeMap<K, V> {
    const JSON_TYPE: &'static str = "object";
}
impl<K, V, S: BuildHasher> DeclaredJsonType for HashMap<K, V, S> {
    const JSON_TYPE: &'static str = "object";
}
impl<T: DeclaredJsonType> DeclaredJsonType for Option<T> {
    const JSON_TYPE: &'static str = T::JSON_TYPE;
}
impl DeclaredJsonType for ContentMapper {
    const JSON_TYPE: &'static str = "unknown";
}
macro_rules! integer_type {($($t:ty),*)=>{$(impl DeclaredJsonType for $t {const JSON_TYPE: &'static str="number";})*};}
integer_type!(u8, u16, u32, u64, u128, usize, i8, i16, i32, i64, i128, isize);
impl<T: DeclaredJsonType> Expected<T> {
    /// This depends only on the declared type, including for an absent receiver.
    /// port: tsc/internal/packagejson/expected.go:Expected.ExpectedJSONType
    pub fn expected_json_type() -> &'static str {
        T::JSON_TYPE
    }
    /// port: tsc/internal/packagejson/expected.go:ExpectedOf
    pub fn of(value: T) -> Self {
        Self {
            state: ExpectedState {
                actual_type: T::JSON_TYPE,
                valid: true,
                null: false,
            },
            value,
        }
    }
}

/// The field's declaration supplies its expected type even before parsing.
/// Dynamic field values use this view instead of deriving a type from JSON.
#[derive(Clone, Copy)]
pub struct ValidatedField<'a> {
    pub state: &'a ExpectedState,
    pub expected_json_type: &'static str,
}
const ABSENT: ExpectedState = ExpectedState {
    actual_type: "",
    valid: false,
    null: false,
};
impl ExpectedKind {
    fn json_type(self) -> &'static str {
        match self {
            Self::String => "string",
            Self::Boolean => "boolean",
            Self::StringArray => "array",
            Self::StringMap => "object",
        }
    }
}
impl Fields {
    pub fn validated_field(&self, name: &str) -> Option<ValidatedField<'_>> {
        let (state, expected_json_type) = if name == "contentMapper" {
            (&self.content_mapper.state, "unknown")
        } else if let Some(name) = name.strip_prefix("contentMapper.") {
            (
                self.content_mapper
                    .value
                    .field(name)
                    .map_or(&ABSENT, |f| &f.state),
                mapper_kind(name)?.1.json_type(),
            )
        } else {
            (
                self.field(name).map_or(&ABSENT, |f| &f.state),
                field_kind(name)?.1.json_type(),
            )
        };
        Some(ValidatedField {
            state,
            expected_json_type,
        })
    }
    pub fn json_value(&self, name: &str) -> JsonValue<'_> {
        JsonValue(self.get(name))
    }
    /// port: tsc/internal/packagejson/packagejson.go:DependencyFields.HasDependency
    pub fn has_dependency(&self, name: &str) -> bool {
        DEPENDENCY_FIELDS.iter().any(|field| {
            self.get(field)
                .and_then(Value::as_object)
                .is_some_and(|map| map.contains_key(name))
        })
    }
    /// Field order is defined; entry order within each map is not.
    /// port: tsc/internal/packagejson/packagejson.go:DependencyFields.RangeDependencies
    pub fn range_dependencies(&self, mut visit: impl FnMut(&str, &str, &'static str) -> bool) {
        for field in DEPENDENCY_FIELDS {
            if let Some(map) = self.get(field).and_then(Value::as_object) {
                for (name, value) in map {
                    if !visit(
                        name,
                        value.as_str().expect("validated dependency string"),
                        field,
                    ) {
                        return;
                    }
                }
            }
        }
    }
    /// port: tsc/internal/packagejson/packagejson.go:DependencyFields.GetRuntimeDependencyNames
    pub fn runtime_dependency_names(&self) -> BTreeSet<&str> {
        ["dependencies", "peerDependencies", "optionalDependencies"]
            .into_iter()
            .filter_map(|field| self.get(field).and_then(Value::as_object))
            .flat_map(|map| map.keys().map(String::as_str))
            .collect()
    }
}
const DEPENDENCY_FIELDS: [&str; 4] = [
    "dependencies",
    "devDependencies",
    "peerDependencies",
    "optionalDependencies",
];
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct JsonValueType(pub i8);
impl JsonValueType {
    pub const NOT_PRESENT: Self = Self(0);
    pub const NULL: Self = Self(1);
    pub const STRING: Self = Self(2);
    pub const NUMBER: Self = Self(3);
    pub const BOOLEAN: Self = Self(4);
    pub const ARRAY: Self = Self(5);
    pub const OBJECT: Self = Self(6);
}
impl fmt::Display for JsonValueType {
    /// port: tsc/internal/packagejson/jsonvalue.go:JSONValueType.String
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::NULL => f.write_str("null"),
            Self::STRING => f.write_str("string"),
            Self::NUMBER => f.write_str("number"),
            Self::BOOLEAN => f.write_str("boolean"),
            Self::ARRAY => f.write_str("array"),
            Self::OBJECT => f.write_str("object"),
            _ => write!(f, "unknown({})", self.0),
        }
    }
}
#[derive(Clone, Copy)]
pub struct JsonValue<'a>(pub Option<&'a Value>);
impl<'a> JsonValue<'a> {
    pub fn kind(self) -> JsonValueType {
        match self.0 {
            None => JsonValueType::NOT_PRESENT,
            Some(Value::Null) => JsonValueType::NULL,
            Some(Value::String(_)) => JsonValueType::STRING,
            Some(Value::Number(_)) => JsonValueType::NUMBER,
            Some(Value::Bool(_)) => JsonValueType::BOOLEAN,
            Some(Value::Array(_)) => JsonValueType::ARRAY,
            Some(Value::Object(_)) => JsonValueType::OBJECT,
        }
    }
    /// port: tsc/internal/packagejson/jsonvalue.go:JSONValue.IsPresent
    pub fn is_present(self) -> bool {
        self.0.is_some()
    }
    /// port: tsc/internal/packagejson/jsonvalue.go:JSONValue.AsString
    pub fn as_string(self) -> &'a str {
        self.0
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("expected string, got {}", self.kind()))
    }
}
