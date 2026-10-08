//! Rust spelling of the pinned `gen-proto` export (`data/s03/schema/api.json`):
//! the API's wire methods, handle scalars and DTOs with the json v2 field
//! rules their struct tags declare. The special mappings the export names
//! stay handwritten in `tsr_api::proto::codecs`; this emitter only refers to
//! them. The export itself is regenerated from the pin by `scripts/s03.py`.
use super::{format_rust, workspace_edition};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::{fs, path::Path};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Schema {
    #[serde(rename = "schemaVersion")]
    version: u32,
    generator: String,
    methods: Vec<Method>,
    types: Vec<Type>,
    declaration_order: Vec<String>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Method {
    constant: String,
    id: String,
    params: TypeRef,
}
#[derive(Deserialize, Default)]
struct TypeRef {
    go: Option<GoType>,
}
#[derive(Deserialize, Clone)]
struct GoType {
    kind: String,
    name: Option<String>,
    element: Option<Box<GoType>>,
    key: Option<Box<GoType>>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Type {
    id: String,
    kind: String,
    fields: Option<Vec<Field>>,
    underlying: Option<GoType>,
    codec_methods: Option<Vec<String>>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Field {
    go_name: String,
    json_name: String,
    go: GoType,
    client_visible: bool,
    tag: String,
}

const API: &str = "github.com/microsoft/TypeScript/tsc/internal/api.";

/// Named types the export references outside the API package, and the Rust
/// spelling each one takes in generated code. Everything else named is an API
/// scalar, DTO or handwritten mapping.
const FOREIGN: &[(&str, &str)] = &[
    ("encoding/json/jsontext.Value", "tsr_json::RawValue"),
    (
        "github.com/microsoft/TypeScript/tsc/internal/core.CompilerOptions",
        "CompilerOptionsValue",
    ),
    (
        "github.com/microsoft/TypeScript/tsc/internal/core.ProjectReference",
        "ProjectReferenceValue",
    ),
    (
        "github.com/microsoft/TypeScript/tsc/internal/core.TypeAcquisition",
        "TypeAcquisitionValue",
    ),
    (
        "github.com/microsoft/TypeScript/tsc/internal/collections.OrderedMap[string, []string]",
        "StringListMap",
    ),
    (
        "github.com/microsoft/TypeScript/tsc/internal/core.Tristate",
        "tsr_core::Tristate",
    ),
    (
        "github.com/microsoft/TypeScript/tsc/internal/packagejson.JSONValue",
        "PackageJsonValue",
    ),
    (
        "github.com/microsoft/TypeScript/tsc/internal/core.JsxEmit",
        "tsr_core::compiler_options::JsxEmit",
    ),
    (
        "github.com/microsoft/TypeScript/tsc/internal/core.ModuleDetectionKind",
        "tsr_core::compiler_options::ModuleDetectionKind",
    ),
    (
        "github.com/microsoft/TypeScript/tsc/internal/core.ModuleKind",
        "tsr_core::compiler_options::ModuleKind",
    ),
    (
        "github.com/microsoft/TypeScript/tsc/internal/core.ModuleResolutionKind",
        "tsr_core::compiler_options::ModuleResolutionKind",
    ),
    (
        "github.com/microsoft/TypeScript/tsc/internal/core.NewLineKind",
        "tsr_core::compiler_options::NewLineKind",
    ),
    (
        "github.com/microsoft/TypeScript/tsc/internal/core.ScriptTarget",
        "tsr_core::ScriptTarget",
    ),
];

/// Foreign integer enumerations: numbers on the wire, never omitted by
/// `omitempty`.
const FOREIGN_NUMBERS: &[&str] = &[
    "github.com/microsoft/TypeScript/tsc/internal/core.JsxEmit",
    "github.com/microsoft/TypeScript/tsc/internal/core.ModuleDetectionKind",
    "github.com/microsoft/TypeScript/tsc/internal/core.ModuleKind",
    "github.com/microsoft/TypeScript/tsc/internal/core.ModuleResolutionKind",
    "github.com/microsoft/TypeScript/tsc/internal/core.NewLineKind",
    "github.com/microsoft/TypeScript/tsc/internal/core.ScriptTarget",
];

/// Handwritten API mappings, by the name `codecs` gives them.
const HANDWRITTEN: &[(&str, &str)] = &[
    ("DocumentIdentifier", "DocumentIdentifier"),
    ("ImportAdderActionKind", "ImportAdderActionKind"),
    ("Method", "Method"),
];

/// Foreign DTOs the export renders for the client but Rust already owns.
const FOREIGN_DTOS: &[&str] = &[
    "github.com/microsoft/TypeScript/tsc/internal/core.CompilerOptions",
    "github.com/microsoft/TypeScript/tsc/internal/core.ProjectReference",
    "github.com/microsoft/TypeScript/tsc/internal/core.TypeAcquisition",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Omit {
    Always,
    /// json v2 `omitempty`: left out when the encoded value is null, `""`,
    /// `{}` or `[]`; never for a number or a boolean.
    Empty,
    /// json v2 `omitzero`: left out when the Go value is its zero value.
    Zero,
}

fn omit(tag: &str) -> Omit {
    let json = tag
        .split_once("json:\"")
        .map(|(_, rest)| rest.split('"').next().unwrap_or_default())
        .unwrap_or_default();
    if json.split(',').skip(1).any(|option| option == "omitzero") {
        Omit::Zero
    } else if json.split(',').skip(1).any(|option| option == "omitempty") {
        Omit::Empty
    } else {
        Omit::Always
    }
}

fn simple_name(id: &str) -> &str {
    id.rsplit('.').next().unwrap_or(id)
}

/// `SnapshotID` becomes `SnapshotId`, `UTF16Offset` becomes `Utf16Offset`.
fn type_name(go: &str) -> String {
    let mut out = String::new();
    let chars: Vec<char> = simple_name(go).chars().collect();
    for (i, &c) in chars.iter().enumerate() {
        let previous_upper = i > 0 && chars[i - 1].is_ascii_uppercase();
        let next_lower = chars.get(i + 1).is_some_and(char::is_ascii_lowercase);
        if c.is_ascii_uppercase() && previous_upper && !next_lower {
            out.push(c.to_ascii_lowercase());
        } else {
            out.push(c);
        }
    }
    out
}

/// Go field names to snake case; runs of capitals are one word.
fn field_name(go: &str) -> String {
    let chars: Vec<char> = go.chars().collect();
    let mut out = String::new();
    for (i, &c) in chars.iter().enumerate() {
        if c.is_ascii_uppercase() {
            let previous_lower = i > 0 && !chars[i - 1].is_ascii_uppercase();
            let next_lower = chars.get(i + 1).is_some_and(char::is_ascii_lowercase);
            let previous_upper = i > 0 && chars[i - 1].is_ascii_uppercase();
            if i > 0 && (previous_lower || (previous_upper && next_lower)) {
                out.push('_');
            }
            out.push(c.to_ascii_lowercase());
        } else {
            out.push(c);
        }
    }
    match out.as_str() {
        "type" | "ref" | "mod" | "use" | "match" | "move" | "fn" | "struct" | "enum" | "impl"
        | "self" | "super" | "const" | "static" | "trait" | "where" | "as" | "in" | "if"
        | "else" | "for" | "loop" | "while" | "break" | "continue" | "return" | "let" | "pub"
        | "crate" | "true" | "false" | "async" | "await" | "dyn" | "box" => {
            format!("r#{out}")
        }
        _ => out,
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Shape {
    String,
    Bool,
    Number,
    Pointer,
    Slice,
    Map,
    Interface,
    Struct,
    Raw,
    Tristate,
    Handwritten,
}

struct Catalog {
    scalars: BTreeMap<String, GoType>,
    dtos: Vec<String>,
    mappings: Vec<String>,
}

impl Catalog {
    fn new(schema: &Schema) -> Result<Self, String> {
        let mut catalog = Self {
            scalars: BTreeMap::new(),
            dtos: Vec::new(),
            mappings: Vec::new(),
        };
        for ty in &schema.types {
            match ty.kind.as_str() {
                "scalar" => {
                    catalog.scalars.insert(
                        ty.id.clone(),
                        ty.underlying
                            .clone()
                            .ok_or_else(|| format!("scalar {} without underlying type", ty.id))?,
                    );
                }
                "dto" => catalog.dtos.push(ty.id.clone()),
                "mapping" => catalog.mappings.push(ty.id.clone()),
                other => return Err(format!("unknown type kind {other} for {}", ty.id)),
            }
        }
        Ok(catalog)
    }

    fn shape(&self, go: &GoType) -> Result<Shape, String> {
        Ok(match go.kind.as_str() {
            "basic" => match go.name.as_deref().unwrap_or_default() {
                "string" => Shape::String,
                "bool" => Shape::Bool,
                "int" | "int32" | "int64" | "uint32" | "uint64" | "float64" => Shape::Number,
                other => return Err(format!("unknown basic type {other}")),
            },
            "pointer" => Shape::Pointer,
            "slice" => Shape::Slice,
            "map" => Shape::Map,
            "interface" => Shape::Interface,
            "named" => {
                let name = go.name.as_deref().unwrap_or_default();
                if let Some(underlying) = self.scalars.get(name) {
                    return self.shape(underlying);
                }
                if self.dtos.iter().any(|id| id == name) {
                    if FOREIGN_DTOS.contains(&name) {
                        return Ok(Shape::Handwritten);
                    }
                    return Ok(Shape::Struct);
                }
                if name == "encoding/json/jsontext.Value" {
                    return Ok(Shape::Raw);
                }
                if name == "github.com/microsoft/TypeScript/tsc/internal/core.Tristate" {
                    return Ok(Shape::Tristate);
                }
                if name.ends_with("api.Method") || name.ends_with("api.ImportAdderActionKind") {
                    return Ok(Shape::String);
                }
                if FOREIGN_NUMBERS.contains(&name) {
                    return Ok(Shape::Number);
                }
                Shape::Handwritten
            }
            other => return Err(format!("unknown Go type kind {other}")),
        })
    }

    fn rust(&self, go: &GoType) -> Result<String, String> {
        Ok(match go.kind.as_str() {
            "basic" => match go.name.as_deref().unwrap_or_default() {
                "string" => "String".into(),
                "bool" => "bool".into(),
                "int" | "int64" => "i64".into(),
                "int32" => "i32".into(),
                "uint32" => "u32".into(),
                "uint64" => "u64".into(),
                "float64" => "f64".into(),
                other => return Err(format!("unknown basic type {other}")),
            },
            "pointer" => format!(
                "Option<Box<{}>>",
                self.rust(go.element.as_deref().ok_or("pointer without element")?)?
            ),
            "slice" => format!(
                "Vec<{}>",
                self.rust(go.element.as_deref().ok_or("slice without element")?)?
            ),
            "map" => format!(
                "JsonMap<{}, {}>",
                self.rust(go.key.as_deref().ok_or("map without key")?)?,
                self.rust(go.element.as_deref().ok_or("map without element")?)?
            ),
            "interface" => "Option<tsr_json::RawValue>".into(),
            "named" => {
                let name = go.name.as_deref().unwrap_or_default();
                if let Some((_, rust)) = FOREIGN.iter().find(|(id, _)| *id == name) {
                    return Ok((*rust).into());
                }
                if let Some(rest) = name.strip_prefix(API) {
                    if let Some((_, rust)) = HANDWRITTEN.iter().find(|(id, _)| *id == rest) {
                        return Ok((*rust).into());
                    }
                    return Ok(type_name(rest));
                }
                if self.scalars.contains_key(name) {
                    return Ok(type_name(name));
                }
                return Err(format!("no Rust spelling for named type {name}"));
            }
            other => return Err(format!("unknown Go type kind {other}")),
        })
    }
}

fn is_string_scalar(catalog: &Catalog, go: &GoType) -> bool {
    go.kind == "named"
        && go
            .name
            .as_deref()
            .and_then(|name| catalog.scalars.get(name))
            .is_some_and(|underlying| underlying.name.as_deref() == Some("string"))
}

fn encode_field(catalog: &Catalog, field: &Field) -> Result<String, String> {
    let name = &field.json_name;
    let rust = field_name(&field.go_name);
    let access = format!("self.{rust}");
    let shape = catalog.shape(&field.go)?;
    let always = format!("tsr_jsonrpc::field(b{name:?}, &{access})");
    Ok(match (omit(&field.tag), shape) {
        // json v2 `omitempty` never leaves out a number or a boolean.
        (Omit::Always, _) | (Omit::Empty, Shape::Bool | Shape::Number) => always,
        // `omitempty` looks through a pointer at the encoded value: a pointer
        // to `""`, `{}` or `[]` is left out like a nil one; `omitzero` leaves
        // out the nil pointer only.
        (Omit::Empty, Shape::Pointer) => {
            let pointee = field
                .go
                .element
                .as_deref()
                .ok_or_else(|| format!("pointer field {name} without an element type"))?;
            let present = format!("{access}.as_deref()");
            match catalog.shape(pointee)? {
                Shape::Bool | Shape::Number | Shape::Tristate | Shape::Pointer => {
                    format!("tsr_jsonrpc::optional(b{name:?}, {present})")
                }
                Shape::String => {
                    let empty = if is_string_scalar(catalog, pointee) {
                        "value.0.is_empty()"
                    } else {
                        "value.is_empty()"
                    };
                    format!("tsr_jsonrpc::omitted(b{name:?}, {present}.filter(|value| !{empty}).map(|value| value as &dyn Encode))")
                }
                Shape::Slice | Shape::Map => format!(
                    "tsr_jsonrpc::omitted(b{name:?}, {present}.filter(|value| !value.is_empty()).map(|value| value as &dyn Encode))"
                ),
                Shape::Interface | Shape::Raw => format!(
                    "tsr_jsonrpc::omitted(b{name:?}, {present}.filter(|value| !raw_is_empty(*value)).map(|value| value as &dyn Encode))"
                ),
                Shape::Struct | Shape::Handwritten => format!(
                    "tsr_jsonrpc::omitted(b{name:?}, {present}.filter(|value| !value.is_json_empty()).map(|value| value as &dyn Encode))"
                ),
            }
        }
        (_, Shape::Pointer) => format!("tsr_jsonrpc::optional(b{name:?}, {access}.as_deref())"),
        (_, Shape::Slice | Shape::Map) => format!(
            "tsr_jsonrpc::omitted(b{name:?}, (!{access}.is_empty()).then_some(&{access} as &dyn Encode))"
        ),
        (_, Shape::String) => {
            let empty = if is_string_scalar(catalog, &field.go) {
                format!("{access}.0.is_empty()")
            } else {
                format!("{access}.is_empty()")
            };
            format!("tsr_jsonrpc::omitted(b{name:?}, (!{empty}).then_some(&{access} as &dyn Encode))")
        }
        (Omit::Zero, Shape::Bool | Shape::Number) => format!(
            "tsr_jsonrpc::omitted(b{name:?}, ({access} != <{}>::default()).then_some(&{access} as &dyn Encode))",
            catalog.rust(&field.go)?
        ),
        (_, Shape::Interface | Shape::Raw) => format!(
            "tsr_jsonrpc::omitted(b{name:?}, (!raw_is_empty(&{access})).then_some(&{access} as &dyn Encode))"
        ),
        (_, Shape::Tristate) => format!(
            "tsr_jsonrpc::omitted(b{name:?}, (!{access}.is_unknown()).then_some(&{access} as &dyn Encode))"
        ),
        (_, Shape::Struct | Shape::Handwritten) => format!(
            "tsr_jsonrpc::omitted(b{name:?}, (!{access}.is_json_empty()).then_some(&{access} as &dyn Encode))"
        ),
    })
}

fn emit(schema: &Schema, pin: &str) -> Result<String, String> {
    let catalog = Catalog::new(schema)?;
    let mut out = String::new();
    // Scalars: newtypes over their Go underlying type.
    for (id, underlying) in &catalog.scalars {
        let name = type_name(id);
        let inner = catalog.rust(underlying)?;
        let copy = if inner == "String" {
            ", PartialOrd, Ord"
        } else {
            ", Copy, PartialOrd, Ord"
        };
        out.push_str(&format!(
            "/// Go `{}`.\n#[derive(Clone, Debug, Default, PartialEq, Eq, Hash{copy})]\npub struct {name}(pub {inner});\nimpl Encode for {name} {{ fn type_name(&self) -> &'static str {{ {:?} }} fn encode(&self, out: &mut Encoder<'_>) -> Result<(), Error> {{ out.value(&self.0) }} }}\nimpl Decode for {name} {{ fn type_name() -> &'static str {{ {:?} }} fn decode(&mut self, input: &mut Decoder<'_>) -> Result<(), Error> {{ input.value(&mut self.0) }} }}\n",
            simple_name(id).rsplit('/').next().unwrap_or_default(),
            format!("api.{}", simple_name(id)),
            format!("api.{}", simple_name(id)),
        ));
        if inner == "String" {
            out.push_str(&format!(
                "impl tsr_json::Key for {name} {{ fn json_key(&self) -> std::borrow::Cow<'_, [u8]> {{ self.0.as_bytes().into() }} }}\nimpl tsr_json::DecodeKey for {name} {{ fn decode_key(bytes: &[u8]) -> Result<Self, Error> {{ Ok(Self(String::from_utf8(bytes.to_vec()).map_err(|_| Error::Message(\"object key is not UTF-8\".into()))?)) }} }}\nimpl From<&str> for {name} {{ fn from(value: &str) -> Self {{ Self(value.into()) }} }}\n"
            ));
        }
    }
    // DTOs in the export's declaration order.
    let types: BTreeMap<&str, &Type> = schema.types.iter().map(|t| (t.id.as_str(), t)).collect();
    for id in &schema.declaration_order {
        let ty = types
            .get(id.as_str())
            .ok_or_else(|| format!("declaration order names unknown type {id}"))?;
        if ty.kind != "dto" || FOREIGN_DTOS.contains(&id.as_str()) {
            continue;
        }
        let name = type_name(id);
        // The one DTO with a custom marshaler, BatchRequestsResponse, writes
        // the same bytes the ordinary field rules produce: its private
        // pre-encoded responses are carried as raw result values here.
        let custom_encode = false;
        let custom_decode = ty
            .codec_methods
            .as_ref()
            .is_some_and(|methods| methods.iter().any(|m| m.starts_with("Unmarshal")));
        let fields: Vec<&Field> = ty
            .fields
            .as_deref()
            .unwrap_or_default()
            .iter()
            .filter(|f| f.client_visible || !f.json_name.is_empty())
            .filter(|f| f.go_name != "_")
            .collect();
        out.push_str(&format!(
            "/// Go `api.{}`.\n#[derive(Clone, Debug, Default, PartialEq)]\npub struct {name} {{\n",
            simple_name(id)
        ));
        for f in &fields {
            out.push_str(&format!(
                "pub {}: {},\n",
                field_name(&f.go_name),
                catalog.rust(&f.go)?
            ));
        }
        out.push_str("}\n");
        if !custom_encode {
            out.push_str(&format!(
                "impl Encode for {name} {{ fn type_name(&self) -> &'static str {{ {:?} }}\nfn encode(&self, out: &mut Encoder<'_>) -> Result<(), Error> {{ tsr_jsonrpc::object(out, &[\n",
                format!("api.{}", simple_name(id))
            ));
            for f in &fields {
                out.push_str(&format!("{},\n", encode_field(&catalog, f)?));
            }
            out.push_str("]) } }\n");
        }
        out.push_str(&format!(
            "impl {name} {{ /// Whether `omitempty` leaves this value out: it encodes as `{{}}`.\n#[must_use] pub fn is_json_empty(&self) -> bool {{ "
        ));
        if fields.is_empty() {
            out.push_str("true");
        } else {
            out.push_str("*self == Self::default()");
        }
        out.push_str(" } }\n");
        if !custom_decode {
            out.push_str(&format!(
                "impl Decode for {name} {{ fn type_name() -> &'static str {{ {:?} }}\nfn decode(&mut self, input: &mut Decoder<'_>) -> Result<(), Error> {{\nif input.peek_kind() == Kind::Null {{ input.read_token()?; *self = Self::default(); return Ok(()); }}\nstructure(input, <Self as Decode>::type_name(), &[\n",
                format!("api.{}", simple_name(id))
            ));
            for f in &fields {
                out.push_str(&format!("{:?},\n", f.json_name));
            }
            if fields.is_empty() {
                out.push_str("], |_, input| input.skip_value()) } }\n");
            } else {
                out.push_str("], |index, input| match index {\n");
                for (i, f) in fields.iter().enumerate() {
                    out.push_str(&format!(
                        "{i} => input.value(&mut self.{}),\n",
                        field_name(&f.go_name)
                    ));
                }
                out.push_str("_ => input.skip_value(),\n}) } }\n");
            }
        }
    }
    // Methods: the wire ids, their Go constants and their typed parameters.
    out.push_str("/// The protocol's request methods, in the export's order.\n#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]\npub enum Method {\n");
    for m in &schema.methods {
        out.push_str(&format!("/// `{}`\n{},\n", m.id, variant(&m.constant)));
    }
    out.push_str("}\nimpl Method {\n");
    out.push_str(&format!(
        "pub const ALL: [Self; {}] = [\n",
        schema.methods.len()
    ));
    for m in &schema.methods {
        out.push_str(&format!("Self::{},\n", variant(&m.constant)));
    }
    out.push_str("];\n/// The method's wire name.\n#[must_use] pub const fn wire(self) -> &'static str { match self {\n");
    for m in &schema.methods {
        out.push_str(&format!("Self::{} => {:?},\n", variant(&m.constant), m.id));
    }
    out.push_str("} }\n/// The pinned Go constant's name.\n#[must_use] pub const fn constant(self) -> &'static str { match self {\n");
    for m in &schema.methods {
        out.push_str(&format!(
            "Self::{} => {:?},\n",
            variant(&m.constant),
            m.constant
        ));
    }
    out.push_str("} }\n/// The method a wire name denotes, if any.\n#[must_use] pub fn from_wire(name: &str) -> Option<Self> { match name {\n");
    for m in &schema.methods {
        out.push_str(&format!(
            "{:?} => Some(Self::{}),\n",
            m.id,
            variant(&m.constant)
        ));
    }
    out.push_str("_ => None,\n} }\n}\n");
    out.push_str("impl Encode for Method { fn type_name(&self) -> &'static str { \"api.Method\" } fn encode(&self, out: &mut Encoder<'_>) -> Result<(), Error> { out.string(self.wire().as_bytes()) } }\n");
    out.push_str("impl Decode for Method { fn type_name() -> &'static str { \"api.Method\" } fn decode(&mut self, input: &mut Decoder<'_>) -> Result<(), Error> { let mut name = String::new(); input.value(&mut name)?; *self = Self::from_wire(&name).ok_or_else(|| Error::Message(format!(\"unknown method: {name}\")))?; Ok(()) } }\n");
    out.push_str("impl Default for Method { fn default() -> Self { Self::ALL[0] } }\n");
    // Params: one variant per method, decoded as the pin's unmarshalPayload does.
    out.push_str("/// A request's decoded parameters.\n#[derive(Clone, Debug, PartialEq)]\npub enum Params {\n");
    for m in &schema.methods {
        match &m.params.go {
            Some(go) => out.push_str(&format!(
                "{}(Box<{}>),\n",
                variant(&m.constant),
                params_type(&catalog, go)?
            )),
            None => out.push_str(&format!("{},\n", variant(&m.constant))),
        }
    }
    out.push_str("}\nimpl Params {\n/// Decodes `payload` for `method`. Methods without parameters ignore it.\npub fn decode(method: Method, payload: &[u8]) -> Result<Self, Error> {\nOk(match method {\n");
    for m in &schema.methods {
        match &m.params.go {
            Some(go) => out.push_str(&format!(
                "Method::{v} => {{ let mut value = {t}::default(); tsr_json::unmarshal(payload, &mut value, tsr_json::Options::default())?; Self::{v}(Box::new(value)) }}\n",
                v = variant(&m.constant),
                t = params_type(&catalog, go)?
            )),
            None => out.push_str(&format!(
                "Method::{v} => Self::{v},\n",
                v = variant(&m.constant)
            )),
        }
    }
    out.push_str("})\n}\n/// The method these parameters belong to.\n#[must_use] pub const fn method(&self) -> Method { match self {\n");
    for m in &schema.methods {
        match &m.params.go {
            Some(_) => out.push_str(&format!(
                "Self::{v}(_) => Method::{v},\n",
                v = variant(&m.constant)
            )),
            None => out.push_str(&format!(
                "Self::{v} => Method::{v},\n",
                v = variant(&m.constant)
            )),
        }
    }
    out.push_str("} }\n}\n");
    let handwritten = [
        "raw_is_empty",
        "structure",
        "CompilerOptionsValue",
        "DocumentIdentifier",
        "ImportAdderActionKind",
        "JsonMap",
        "PackageJsonValue",
        "ProjectReferenceValue",
        "StringListMap",
        "TypeAcquisitionValue",
    ]
    .into_iter()
    .filter(|name| out.contains(name))
    .collect::<Vec<_>>()
    .join(", ");
    let header = format!(
        "// Generated by cargo xtask gen api; do not edit.\n// Upstream {pin}: data/s03/schema/api.json, the pinned gen-proto export.\n#![allow(clippy::upper_case_acronyms, clippy::struct_excessive_bools, clippy::struct_field_names, clippy::module_name_repetitions, clippy::too_many_lines, clippy::large_enum_variant, reason = \"Generated names and shapes follow the pinned protocol\")]\n\nuse super::codecs::{{{handwritten}}};\nuse tsr_json::{{Decode, Decoder, Encode, Encoder, Error, Kind}};\n\n"
    );
    Ok(header + &out)
}

fn variant(constant: &str) -> &str {
    constant.strip_prefix("Method").unwrap_or(constant)
}

/// A method's parameter type: the pin decodes into the pointed-to struct.
fn params_type(catalog: &Catalog, go: &GoType) -> Result<String, String> {
    match go.kind.as_str() {
        "pointer" => catalog.rust(go.element.as_deref().ok_or("pointer without element")?),
        _ => catalog.rust(go),
    }
}

pub(super) fn run(root: &Path, args: &[String], pin: &str) -> Result<bool, String> {
    let check = match args {
        [] => false,
        [arg] if arg == "--check" => true,
        _ => return Err("usage: cargo xtask gen api [--check]".into()),
    };
    let schema_path = root.join("data/s03/schema/api.json");
    let schema: Schema =
        serde_json::from_slice(&fs::read(&schema_path).map_err(|e| e.to_string())?)
            .map_err(|e| format!("{}: {e}", schema_path.display()))?;
    if schema.version != 1 || schema.generator != "tools/gen-proto" {
        return Err("API export version/generator mismatch".into());
    }
    let bytes = format_rust(root, &emit(&schema, pin)?, &workspace_edition(root)?)?;
    let path = root.join("crates/tsr_api/src/proto/generated.rs");
    let same = fs::read(&path).is_ok_and(|old| old == bytes);
    if !check {
        fs::write(&path, bytes).map_err(|e| e.to_string())?;
    }
    eprintln!(
        "API protocol: {} methods, {} types{}",
        schema.methods.len(),
        schema.types.len(),
        if check && !same {
            " (generated file differs)"
        } else {
            ""
        }
    );
    Ok(!check || same)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_follow_rust_conventions() {
        assert_eq!(type_name("api.SnapshotID"), "SnapshotId");
        assert_eq!(type_name("core.UTF16Offset"), "Utf16Offset");
        assert_eq!(type_name("api.JSDocTagInfo"), "JsDocTagInfo");
        assert_eq!(field_name("ParsedCommandLine"), "parsed_command_line");
        assert_eq!(field_name("URI"), "uri");
        assert_eq!(field_name("FileName"), "file_name");
        assert_eq!(field_name("IsTupleType"), "is_tuple_type");
        assert_eq!(field_name("Type"), "r#type");
        assert_eq!(field_name("ToURI"), "to_uri");
    }

    #[test]
    fn tags_select_the_json_v2_omission_rule() {
        assert_eq!(omit("json:\"id\""), Omit::Always);
        assert_eq!(omit("json:\"flags,omitempty\""), Omit::Empty);
        assert_eq!(omit("json:\"parent,omitzero\""), Omit::Zero);
        assert_eq!(omit("json:\"responses\" nonnil:\"true\""), Omit::Always);
    }
}
