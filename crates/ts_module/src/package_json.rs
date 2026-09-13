//! `internal/packagejson/{packagejson,expected,jsonvalue,exportsorimports}.go`.
//!
//! The source parser accepts duplicate document keys but rejects malformed JSON.
//! Expected fields decode with a fresh (duplicate-rejecting) decoder. A failed
//! repeated typed field can retain prior validity and a partially updated value.
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use serde::de::{Deserialize, Deserializer, MapAccess, Visitor};
pub use serde_json::Value;
use serde_json::{value::RawValue, Map};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ExpectedState {
    pub actual_type: &'static str,
    pub valid: bool,
    pub null: bool,
}
impl ExpectedState {
    pub fn is_present(&self) -> bool {
        !self.actual_type.is_empty()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Expected<T> {
    pub state: ExpectedState,
    pub value: T,
}
impl<T: Default> Default for Expected<T> {
    fn default() -> Self {
        Self {
            state: ExpectedState::default(),
            value: T::default(),
        }
    }
}
impl<T> Expected<T> {
    pub fn get_value(&self) -> Option<&T> {
        self.state.valid.then_some(&self.value)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ContentMapper {
    typed: BTreeMap<&'static str, Expected<Value>>,
}
impl ContentMapper {
    pub fn field(&self, name: &str) -> Option<&Expected<Value>> {
        self.typed.get(name)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Fields {
    typed: BTreeMap<&'static str, Expected<Value>>,
    untyped: Map<String, Value>,
    pub content_mapper: Expected<ContentMapper>,
}
impl Fields {
    /// Valid typed values and present untyped values. Failed typed fields keep
    /// their source state available through `field` but do not produce a value.
    pub fn get(&self, name: &str) -> Option<&Value> {
        if let Some(field) = self.typed.get(name) {
            field.get_value()
        } else {
            self.untyped.get(name)
        }
    }
    pub fn field(&self, name: &str) -> Option<&Expected<Value>> {
        self.typed.get(name)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ParsedPackageJson {
    pub fields: Fields,
    pub parseable: bool,
}

/// The resolver keeps a present package on a parse failure, with zero fields.
/// Error text is deliberately not surfaced: the source resolver discards it.
pub fn parse(data: &[u8]) -> ParsedPackageJson {
    parse_fields(data).map_or_else(ParsedPackageJson::default, |fields| ParsedPackageJson {
        fields,
        parseable: true,
    })
}

#[derive(Clone, Copy)]
enum ExpectedKind {
    String,
    StringMap,
    StringArray,
    Boolean,
}
fn zero(kind: ExpectedKind) -> Value {
    match kind {
        ExpectedKind::String => Value::String(String::new()),
        ExpectedKind::Boolean => Value::Bool(false),
        ExpectedKind::StringMap | ExpectedKind::StringArray => Value::Null,
    }
}
fn field_kind(name: &str) -> Option<(&'static str, ExpectedKind)> {
    Some(match name {
        "name" => ("name", ExpectedKind::String),
        "version" => ("version", ExpectedKind::String),
        "type" => ("type", ExpectedKind::String),
        "tsconfig" => ("tsconfig", ExpectedKind::String),
        "main" => ("main", ExpectedKind::String),
        "types" => ("types", ExpectedKind::String),
        "typings" => ("typings", ExpectedKind::String),
        "dependencies" => ("dependencies", ExpectedKind::StringMap),
        "devDependencies" => ("devDependencies", ExpectedKind::StringMap),
        "peerDependencies" => ("peerDependencies", ExpectedKind::StringMap),
        "optionalDependencies" => ("optionalDependencies", ExpectedKind::StringMap),
        _ => return None,
    })
}
fn mapper_kind(name: &str) -> Option<(&'static str, ExpectedKind)> {
    Some(match name {
        "exec" => ("exec", ExpectedKind::StringArray),
        "compilerOptions" => ("compilerOptions", ExpectedKind::StringArray),
        "dynamicConfig" => ("dynamicConfig", ExpectedKind::Boolean),
        _ => return None,
    })
}
fn actual_type(raw: &RawValue) -> &'static str {
    match raw.get().as_bytes()[0] {
        b'n' => "null",
        b'"' => "string",
        b't' | b'f' => "boolean",
        b'[' => "array",
        b'{' => "object",
        _ => "number",
    }
}

fn parse_fields(data: &[u8]) -> Option<Fields> {
    let text = std::str::from_utf8(data).ok()?;
    let raw: &RawValue = serde_json::from_str(text).ok()?;
    validate_strings(raw)?;
    if raw.get() == "null" {
        return Some(Fields::default());
    }
    let mut result = Fields::default();
    let mut typescript: Expected<Expected<ContentMapper>> = Expected::default();
    for (name, raw) in object(raw)? {
        if let Some((name, kind)) = field_kind(&name) {
            let field = result.typed.entry(name).or_insert_with(|| Expected {
                state: ExpectedState::default(),
                value: zero(kind),
            });
            decode_expected(field, kind, raw);
        } else if matches!(name.as_str(), "exports" | "imports" | "typesVersions") {
            result.untyped.insert(name, json_value(raw)?);
        } else if name == "typescript" {
            decode_typescript(&mut typescript, raw);
        }
    }
    result.content_mapper = typescript.value;
    Some(result)
}

// RawValue validates grammar without forcing unknown numbers through f64 and
// without allocating an AST. Validate every string, including ignored fields:
// serde's raw skip path otherwise accepts unpaired surrogate escapes.
fn validate_strings(raw: &RawValue) -> Option<()> {
    let text = raw.get();
    let bytes = text.as_bytes();
    let mut index = 0;
    let mut depth = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'"' => {
                let start = index;
                index += 1;
                while bytes[index] != b'"' {
                    if bytes[index] == b'\\' {
                        index += 1;
                    }
                    index += 1;
                }
                index += 1;
                if bytes[start..index].contains(&b'\\') {
                    let _: String = serde_json::from_str(&text[start..index]).ok()?;
                }
                continue;
            }
            b'[' | b'{' => {
                depth += 1;
                if depth > 10_000 {
                    return None;
                }
            }
            b']' | b'}' => depth -= 1,
            _ => {}
        }
        index += 1;
    }
    Some(())
}

struct Object<'a>(Vec<(String, &'a RawValue)>);
impl<'de> Deserialize<'de> for Object<'de> {
    fn deserialize<D: Deserializer<'de>>(decoder: D) -> Result<Self, D::Error> {
        struct ObjectVisitor;
        impl<'de> Visitor<'de> for ObjectVisitor {
            type Value = Object<'de>;
            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("an object")
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                let mut entries = Vec::new();
                while let Some(entry) = map.next_entry()? {
                    entries.push(entry);
                }
                Ok(Object(entries))
            }
        }
        decoder.deserialize_map(ObjectVisitor)
    }
}
fn object(raw: &RawValue) -> Option<Vec<(String, &RawValue)>> {
    serde_json::from_str::<Object<'_>>(raw.get())
        .ok()
        .map(|object| object.0)
}
fn array(raw: &RawValue) -> Option<Vec<&RawValue>> {
    serde_json::from_str(raw.get()).ok()
}

fn unique_names(raw: &RawValue) -> bool {
    let mut pending = vec![raw];
    while let Some(raw) = pending.pop() {
        match actual_type(raw) {
            "object" => {
                let Some(entries) = object(raw) else {
                    return false;
                };
                let mut names = BTreeSet::new();
                for (name, value) in entries {
                    if !names.insert(name) {
                        return false;
                    }
                    pending.push(value);
                }
            }
            "array" => {
                let Some(values) = array(raw) else {
                    return false;
                };
                pending.extend(values);
            }
            _ => {}
        }
    }
    true
}

fn decode_expected(field: &mut Expected<Value>, kind: ExpectedKind, raw: &RawValue) {
    let actual = actual_type(raw);
    if actual == "null" {
        *field = Expected {
            state: ExpectedState {
                actual_type: "null",
                valid: false,
                null: true,
            },
            value: zero(kind),
        };
        return;
    }
    if decode_value(&mut field.value, kind, raw) {
        field.state.valid = true;
    }
    field.state.actual_type = actual;
}
fn decode_value(value: &mut Value, kind: ExpectedKind, raw: &RawValue) -> bool {
    if raw.get() == "null" {
        *value = zero(kind);
        return true;
    }
    match kind {
        ExpectedKind::String => {
            if let Ok(parsed) = serde_json::from_str::<String>(raw.get()) {
                *value = Value::String(parsed);
                true
            } else {
                false
            }
        }
        ExpectedKind::Boolean => {
            if let Ok(parsed) = serde_json::from_str::<bool>(raw.get()) {
                *value = Value::Bool(parsed);
                true
            } else {
                false
            }
        }
        ExpectedKind::StringMap => {
            let Some(entries) = object(raw) else {
                return false;
            };
            if value.is_null() {
                *value = Value::Object(Map::new());
            }
            let map = value
                .as_object_mut()
                .expect("string map fields retain their value type");
            let mut names = BTreeSet::new();
            for (name, raw) in entries {
                if !names.insert(name.clone()) {
                    return false;
                }
                let item = map
                    .entry(name)
                    .or_insert_with(|| zero(ExpectedKind::String));
                if !decode_value(item, ExpectedKind::String, raw) {
                    return false;
                }
            }
            true
        }
        ExpectedKind::StringArray => {
            let Some(entries) = array(raw) else {
                return false;
            };
            let mut values = Vec::with_capacity(entries.len());
            let mut valid = true;
            for raw in entries {
                let mut value = zero(ExpectedKind::String);
                valid = decode_value(&mut value, ExpectedKind::String, raw);
                values.push(value);
                if !valid {
                    break;
                }
            }
            *value = Value::Array(values);
            valid
        }
    }
}

fn decode_typescript(field: &mut Expected<Expected<ContentMapper>>, raw: &RawValue) {
    let actual = actual_type(raw);
    if actual == "null" {
        *field = Expected {
            state: ExpectedState {
                actual_type: "null",
                null: true,
                valid: false,
            },
            value: Expected::default(),
        };
        return;
    }
    if let Some(entries) = object(raw) {
        let mut names = BTreeSet::new();
        let mut valid = true;
        for (name, raw) in entries {
            if !names.insert(name.clone()) || !unique_names(raw) {
                valid = false;
                break;
            }
            if name == "contentMapper" {
                decode_mapper(&mut field.value, raw);
            }
        }
        if valid {
            field.state.valid = true;
        }
    }
    field.state.actual_type = actual;
}
fn decode_mapper(field: &mut Expected<ContentMapper>, raw: &RawValue) {
    let actual = actual_type(raw);
    if actual == "null" {
        *field = Expected {
            state: ExpectedState {
                actual_type: "null",
                null: true,
                valid: false,
            },
            value: ContentMapper::default(),
        };
        return;
    }
    if let Some(entries) = object(raw) {
        let mut names = BTreeSet::new();
        let mut valid = true;
        for (name, raw) in entries {
            if !names.insert(name.clone()) || !unique_names(raw) {
                valid = false;
                break;
            }
            if let Some((name, kind)) = mapper_kind(&name) {
                let field = field.value.typed.entry(name).or_insert_with(|| Expected {
                    state: ExpectedState::default(),
                    value: zero(kind),
                });
                decode_expected(field, kind, raw);
            }
        }
        if valid {
            field.state.valid = true;
        }
    }
    field.state.actual_type = actual;
}

// Materialize only the three untyped fields. Iterative tasks keep input nesting
// out of the Rust call stack; map insertion preserves first-key position.
fn json_value(raw: &RawValue) -> Option<Value> {
    enum Task<'a> {
        Read(&'a RawValue),
        Array(usize),
        Object(Vec<String>),
    }
    let mut tasks = vec![Task::Read(raw)];
    let mut values = Vec::new();
    while let Some(task) = tasks.pop() {
        match task {
            Task::Read(raw) => match actual_type(raw) {
                "object" => {
                    let (keys, entries): (Vec<_>, Vec<_>) = object(raw)?.into_iter().unzip();
                    tasks.push(Task::Object(keys));
                    for value in entries.into_iter().rev() {
                        tasks.push(Task::Read(value));
                    }
                }
                "array" => {
                    let entries = array(raw)?;
                    tasks.push(Task::Array(entries.len()));
                    for value in entries.into_iter().rev() {
                        tasks.push(Task::Read(value));
                    }
                }
                "number" => {
                    values.push(Value::Number(serde_json::Number::from_f64(
                        raw.get().parse().ok()?,
                    )?));
                }
                _ => values.push(serde_json::from_str(raw.get()).ok()?),
            },
            Task::Array(len) => {
                let elements = values.split_off(values.len() - len);
                values.push(Value::Array(elements));
            }
            Task::Object(keys) => {
                let elements = values.split_off(values.len() - keys.len());
                let mut map = Map::new();
                for (key, value) in keys.into_iter().zip(elements) {
                    map.insert(key, value);
                }
                values.push(Value::Object(map));
            }
        }
    }
    values.pop()
}

/// The source's number payload is float64; its interface comparison with the
/// integer `0` is false, so even numeric zero is truthy at this pin.
pub fn is_falsy(value: Option<&Value>) -> bool {
    match value {
        None | Some(Value::Null) => true,
        Some(Value::Bool(value)) => !value,
        Some(Value::String(value)) => value.is_empty(),
        _ => false,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ObjectKind {
    Subpaths,
    Conditions,
    Imports,
    Invalid,
}
pub fn object_kind(value: &Value) -> Option<ObjectKind> {
    let mut dot = false;
    let mut hash = false;
    let mut other = false;
    for key in value.as_object()?.keys().filter(|key| !key.is_empty()) {
        match key.as_bytes()[0] {
            b'.' => dot = true,
            b'#' => hash = true,
            _ => other = true,
        }
        if other && (dot || hash) {
            return Some(ObjectKind::Invalid);
        }
    }
    Some(if dot {
        ObjectKind::Subpaths
    } else if hash {
        ObjectKind::Imports
    } else {
        ObjectKind::Conditions
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn state<T>(field: &Expected<T>, value: &Value) -> Value {
        json!({"present":field.state.is_present(),"valid":field.state.valid,"null":field.state.null,"actual":field.state.actual_type,"value":value})
    }
    fn observed_field(field: Option<&Expected<Value>>, kind: ExpectedKind) -> Value {
        let absent = Expected {
            state: ExpectedState::default(),
            value: zero(kind),
        };
        let field = field.unwrap_or(&absent);
        state(field, &field.value)
    }
    fn observed_value(value: Option<&Value>, exports: bool) -> Value {
        let kind = match value {
            None => 0,
            Some(Value::Null) => 1,
            Some(Value::String(_)) => 2,
            Some(Value::Number(_)) => 3,
            Some(Value::Bool(_)) => 4,
            Some(Value::Array(_)) => 5,
            Some(Value::Object(_)) => 6,
        };
        let observed = match value {
            Some(Value::Object(map)) => Value::Array(map.iter().map(|(key,value)|json!({"key":key,"value":observed_value(Some(value),exports)})).collect()),
            Some(Value::Array(items)) => Value::Array(items.iter().map(|value|observed_value(Some(value),exports)).collect()),
            Some(Value::Number(number)) => {
                // Go's observation JSON formats integral float64 as an integer.
                let number = number.as_f64().unwrap();
                if number == 0.0 { json!(0) } else { value.unwrap().clone() }
            }
            _ => value.cloned().unwrap_or(Value::Null),
        };
        let mut result = json!({"kind":kind,"value":observed,"falsy":is_falsy(value)});
        if let Some(kind) = value.filter(|_| exports).and_then(object_kind) {
            result["object_kind"] = json!(match kind {
                ObjectKind::Subpaths => "subpaths",
                ObjectKind::Imports => "imports",
                ObjectKind::Conditions => "conditions",
                ObjectKind::Invalid => "invalid",
            });
        }
        result
    }
    #[test]
    fn parsed_fields_match_original_go() {
        let requests: Vec<Value> =
            serde_json::from_str(include_str!("../../../tools/s07/packagejson/requests.json"))
                .unwrap();
        let observations: Vec<Value> = serde_json::from_str(include_str!(
            "../../../data/s07/packagejson-observations.json"
        ))
        .unwrap();
        assert_eq!(requests.len(), observations.len());
        for (request, row) in requests.iter().zip(observations) {
            assert_eq!(request["id"], row["id"]);
            let input: Vec<_> = request["hex"]
                .as_str()
                .unwrap()
                .as_bytes()
                .chunks_exact(2)
                .map(|p| u8::from_str_radix(std::str::from_utf8(p).unwrap(), 16).unwrap())
                .collect();
            let parsed = parse(&input);
            let fields = &parsed.fields;
            assert_eq!(
                parsed.parseable,
                row["parseable"].as_bool().unwrap(),
                "{}",
                request["id"]
            );
            for (name, expected) in row["fields"].as_object().unwrap() {
                let actual = if name == "contentMapper" {
                    let mut value = Map::new();
                    for name in ["exec", "compilerOptions", "dynamicConfig"] {
                        value.insert(
                            name.into(),
                            observed_field(
                                fields.content_mapper.value.field(name),
                                mapper_kind(name).unwrap().1,
                            ),
                        );
                    }
                    state(&fields.content_mapper, &Value::Object(value))
                } else {
                    observed_field(fields.field(name), field_kind(name).unwrap().1)
                };
                assert_eq!(&actual, expected, "{} field {name}", request["id"]);
            }
            for name in ["exports", "imports", "typesVersions"] {
                assert_eq!(
                    observed_value(fields.get(name), name != "typesVersions"),
                    row[name],
                    "{} field {name}",
                    request["id"]
                );
            }
        }
    }
}
