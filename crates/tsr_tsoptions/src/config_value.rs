//! Values crossing the Go config parser's `any` boundary. Text stays byte exact;
//! arrays and object insertion order preserve source traversal and diagnostics.
use tsr_core::collections::OrderedMap;
use tsr_jsstring::JsString;

#[derive(Clone, Debug, Default, PartialEq)]
pub enum ConfigValue {
    #[default]
    Null,
    /// Empty source text returns Go struct{} rather than an ordered object.
    EmptyStruct,
    Boolean(bool),
    Number(f64),
    /// CLI strconv.Atoi result; distinct from JSON's float64 for interface tests.
    Integer(i64),
    /// A typed source enum from an option declaration, before assignment.
    Enum(i32),
    String(JsString),
    Array(Option<Vec<Self>>),
    Object(OrderedMap<JsString, Self>),
    /// Foreign typed input, before normalize_json_value erases slice type.
    StringArray(Option<Vec<JsString>>),
    /// Foreign map input has no stable order; normalization sorts its keys.
    UnorderedObject(std::collections::HashMap<JsString, Self>),
}
impl ConfigValue {
    pub fn as_string(&self) -> Option<&JsString> {
        if let Self::String(value) = self {
            Some(value)
        } else {
            None
        }
    }
    pub fn as_array(&self) -> Option<&[Self]> {
        if let Self::Array(value) = self {
            Some(value.as_deref().unwrap_or_default())
        } else {
            None
        }
    }
    pub fn as_object(&self) -> Option<&OrderedMap<JsString, Self>> {
        if let Self::Object(value) = self {
            Some(value)
        } else {
            None
        }
    }
    pub fn get(&self, key: &[u8]) -> Option<&Self> {
        self.as_object()?.get(key)
    }
    pub fn set(&mut self, key: JsString, value: Self) {
        let Self::Object(entries) = self else {
            panic!("config property assignment requires an object")
        };
        entries.insert(key, value);
    }
    pub fn is_null(&self) -> bool {
        matches!(self, Self::Null)
    }
}

/// Normalize foreign JSON inputs at the raw config boundary. Preserve ordered
/// object traversal; only unordered maps acquire a sorted key order.
/// port: tsc/internal/tsoptions/tsconfigparsing.go:normalizeJsonValue
pub fn normalize_json_value(value: ConfigValue) -> ConfigValue {
    match value {
        ConfigValue::Object(values) => ConfigValue::Object(
            values
                .into_iter()
                .map(|(k, v)| (k, normalize_json_value(v)))
                .collect(),
        ),
        ConfigValue::UnorderedObject(values) => {
            let mut values: Vec<_> = values.into_iter().collect();
            values.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
            ConfigValue::Object(
                values
                    .into_iter()
                    .map(|(k, v)| (k, normalize_json_value(v)))
                    .collect(),
            )
        }
        ConfigValue::Array(values) => ConfigValue::Array(Some(
            values
                .unwrap_or_default()
                .into_iter()
                .map(normalize_json_value)
                .collect(),
        )),
        ConfigValue::StringArray(None) => ConfigValue::Null,
        ConfigValue::StringArray(Some(values)) => {
            ConfigValue::Array(Some(values.into_iter().map(ConfigValue::String).collect()))
        }
        scalar => scalar,
    }
}
