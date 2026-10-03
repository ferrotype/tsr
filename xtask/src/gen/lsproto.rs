//! Rust spelling of the pinned LSP generator's resolved types and dispatch plans.
//! The access adapter executes its normalization and union-discriminator logic.
use super::{exclusive_lock, format_rust, workspace_edition};
use serde::Deserialize;
use serde_json::Value;
use std::{fs, path::Path, process::Command};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Schema {
    version: u32,
    pin: String,
    model_version: String,
    structures: Vec<Structure>,
    enumerations: Vec<Enumeration>,
    unions: Vec<Union>,
    literals: Vec<Literal>,
    aliases: Vec<Alias>,
    methods: Vec<Method>,
    registrations: Vec<Registration>,
}
#[derive(Deserialize)]
struct Structure {
    name: String,
    fields: Vec<Field>,
    strict: bool,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Field {
    name: String,
    go_type: String,
    required: bool,
    reject_null: bool,
    omit_zero: bool,
}
#[derive(Deserialize)]
struct Enumeration {
    name: String,
    r#type: String,
    values: Vec<EnumValue>,
    bitflags: bool,
}
#[derive(Deserialize)]
struct EnumValue {
    name: String,
    value: Value,
}
#[derive(Deserialize)]
struct Union {
    name: String,
    nullable: bool,
    members: Vec<Member>,
    dispatch: bool,
    groups: Vec<Group>,
    fallback: Dispatch,
}
#[derive(Deserialize)]
struct Member {
    name: String,
    r#type: String,
}
#[derive(Deserialize)]
struct Group {
    kind: String,
    fields: Vec<String>,
    plan: Dispatch,
}
#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
enum Dispatch {
    Direct {
        field: String,
    },
    Try {
        fields: Vec<String>,
    },
    Discriminator {
        name: String,
        cases: Vec<Case>,
        fallback: Box<Self>,
    },
    Presence {
        checks: Vec<Check>,
        fallback: Box<Self>,
    },
}
#[derive(Deserialize)]
struct Case {
    value: String,
    field: String,
}
#[derive(Deserialize)]
struct Check {
    name: String,
    field: String,
}
#[derive(Deserialize)]
struct Literal {
    name: String,
    json: String,
}
#[derive(Deserialize)]
struct Alias {
    name: String,
    target: Resolved,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Resolved {
    name: String,
    needs_pointer: bool,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Method {
    #[serde(rename = "method")]
    wire_name: String,
    name: String,
    params: Option<Resolved>,
    result: Option<Resolved>,
    null_result: bool,
    request: bool,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Registration {
    #[serde(rename = "registrationMethod")]
    method: String,
    field_name: String,
    options_type_name: String,
}

fn ident(name: &str) -> String {
    let chars: Vec<_> = name.trim_start_matches('_').chars().collect();
    let mut result = String::new();
    for (i, &c) in chars.iter().enumerate() {
        if c.is_ascii_uppercase()
            && i > 0
            && chars[i - 1] != '_'
            && (chars[i - 1].is_ascii_lowercase()
                || chars[i - 1].is_ascii_digit()
                || chars.get(i + 1).is_some_and(char::is_ascii_lowercase))
        {
            result.push('_');
        }
        result.push(c.to_ascii_lowercase());
    }
    match result.as_str() {
        "type" | "ref" | "static" | "default" | "abstract" | "async" | "await" | "match"
        | "loop" | "in" | "where" | "use" | "move" | "box" | "const" | "fn" | "self" | "super"
        | "crate" | "mod" | "pub" | "trait" | "impl" | "return" | "yield" | "as" | "break"
        | "continue" | "else" | "enum" | "extern" | "false" | "for" | "if" | "let" | "mut"
        | "struct" | "true" | "unsafe" | "while" | "dyn" => format!("r#{result}"),
        _ => result,
    }
}
fn constant(name: &str) -> String {
    ident(name).trim_start_matches("r#").to_ascii_uppercase()
}
fn ty(go: &str) -> String {
    if let Some(inner) = go.strip_prefix('*') {
        return format!("Option<Box<{}>>", ty(inner));
    }
    if let Some(inner) = go.strip_prefix("[]") {
        return format!("Vec<{}>", ty(inner));
    }
    if let Some(map) = go.strip_prefix("map[") {
        let (key, value) = map.split_once(']').expect("resolver exports a Go map");
        return format!("std::collections::HashMap<{}, {}>", ty(key), ty(value));
    }
    match go {
        "string" => "String",
        "int32" => "i32",
        "uint32" => "u32",
        "uint64" => "u64",
        "float64" => "f64",
        "any" => "Any",
        "struct{}" => "EmptyObject",
        "[2]uint32" => "UIntPair",
        _ => go,
    }
    .into()
}
fn direct(field: &str, buffered: bool) -> String {
    let field = ident(field);
    let decode = if buffered {
        "tsr_json::unmarshal(&data, value, tsr_json::Options::default())"
    } else {
        "input.value(value)"
    };
    format!("let value = self.{field}.insert(Box::default()); return {decode};\n")
}
fn dispatch(plan: &Dispatch) -> String {
    match plan {
        Dispatch::Direct { field } => direct(field, true),
        Dispatch::Try { fields } => {
            let mut code = String::new();
            for field in fields {
                code.push_str(&format!(
                    "if crate::codec::try_member(&data, &mut self.{}) {{ return Ok(()); }}\n",
                    ident(field)
                ));
            }
            code
        }
        Dispatch::Discriminator {
            name,
            cases,
            fallback,
        } => {
            let mut code =
                format!("match crate::codec::raw_field(&data, {name:?}).as_deref() {{\n");
            for case in cases {
                let json = serde_json::to_string(&case.value).expect("string JSON");
                code.push_str(&format!(
                    "Some(b{json:?}) => {{ {} }},\n",
                    direct(&case.field, true)
                ));
            }
            code.push_str(&format!("_ => {{ {} }}\n}}\n", dispatch(fallback)));
            code
        }
        Dispatch::Presence { checks, fallback } => {
            let names = checks
                .iter()
                .map(|c| format!("{:?}", c.name))
                .collect::<Vec<_>>()
                .join(",");
            let mut code = format!("match crate::codec::first_key(&data, &[{names}]) {{\n");
            for (i, check) in checks.iter().enumerate() {
                code.push_str(&format!(
                    "Some({i}) => {{ {} }},\n",
                    direct(&check.field, true)
                ));
            }
            code.push_str(&format!("_ => {{ {} }}\n}}\n", dispatch(fallback)));
            code
        }
    }
}
fn exhaustive(plan: &Dispatch) -> bool {
    match plan {
        Dispatch::Direct { .. } => true,
        Dispatch::Try { .. } => false,
        Dispatch::Discriminator { fallback, .. } | Dispatch::Presence { fallback, .. } => {
            exhaustive(fallback)
        }
    }
}
fn emit(schema: &Schema) -> Result<String, String> {
    let mut out = format!("// Generated by cargo xtask gen lsproto; do not edit.\n// Upstream {}, metamodel {} plus the pinned Corsa resolver.\n// Dispatch bodies also appear nested inside discriminator fallbacks.\n#![allow(clippy::needless_return, clippy::single_match, clippy::single_match_else, reason = \"Keep generated dispatch and fallback bodies identical\")]\n#![allow(clippy::type_complexity, reason = \"Protocol maps retain their exact nested optional types\")]\n\nuse crate::{{Any, DocumentUri, EmptyObject, NoParams, Null, Registration, UIntPair, URI}};\nuse tsr_json::{{Decode, Decoder, Encode, Encoder, Error, Kind}};\n", schema.pin, schema.model_version);
    for s in &schema.structures {
        if s.name == "Registration" {
            continue;
        } // The wire method selects the options type.
        out.push_str(&format!(
            "#[derive(Clone, Debug, Default, PartialEq)]\npub struct {} {{\n",
            s.name
        ));
        for f in &s.fields {
            out.push_str(&format!("pub {}: {},\n", ident(&f.name), ty(&f.go_type)));
        }
        out.push_str("}\n");
        out.push_str(&format!("impl Encode for {} {{ fn type_name(&self) -> &'static str {{ {:?} }}\nfn encode(&self, out: &mut Encoder<'_>) -> Result<(), Error> {{ tsr_jsonrpc::object(out, &[\n", s.name, format!("lsproto.{}", s.name)));
        for f in &s.fields {
            let n = &f.name;
            let field = ident(n);
            let code = if f.omit_zero && f.go_type.starts_with('*') {
                format!("tsr_jsonrpc::optional(b{n:?}, self.{field}.as_deref())")
            } else if f.omit_zero {
                format!("tsr_jsonrpc::optional(b{n:?}, (self.{field} != <{}>::default()).then_some(&self.{field}))", ty(&f.go_type))
            } else {
                format!("tsr_jsonrpc::field(b{n:?}, &self.{field})")
            };
            out.push_str(&format!("{code},\n"));
        }
        out.push_str("]) } }\n");
        out.push_str(&format!("impl Decode for {} {{ fn type_name() -> &'static str {{ {:?} }}\nfn decode(&mut self, input: &mut Decoder<'_>) -> Result<(), Error> {{\n", s.name, format!("lsproto.{}", s.name)));
        if !s.strict {
            out.push_str("if input.peek_kind() == Kind::Null { input.read_token()?; *self = Self::default(); return Ok(()); }\n");
        }
        out.push_str(&format!(
            "crate::codec::structure(input, <Self as Decode>::type_name(), {}, &[\n",
            s.strict
        ));
        let mut required = 0;
        for f in &s.fields {
            let bit = if f.required {
                let bit = format!("Some({required})");
                required += 1;
                bit
            } else {
                "None".into()
            };
            out.push_str(&format!(
                "crate::codec::Field {{ name: {:?}, required_bit: {bit}, reject_null: {} }},\n",
                f.name, f.reject_null
            ));
        }
        if required > 64 {
            return Err(format!("too many required fields in {}", s.name));
        }
        if s.fields.is_empty() {
            out.push_str("], |_, input| input.skip_value()) } }\n");
        } else {
            out.push_str("], |index, input| match index {\n");
            for (i, f) in s.fields.iter().enumerate() {
                out.push_str(&format!(
                    "{i} => input.value(&mut self.{}),\n",
                    ident(&f.name)
                ));
            }
            out.push_str("_ => input.skip_value(),\n}) } }\n");
        }
    }
    for e in &schema.enumerations {
        let scalar = ty(&e.r#type);
        out.push_str(&format!("#[derive(Clone, Debug, Default, PartialEq, Eq, Hash{} )]\npub struct {}(pub {});\nimpl {} {{\n", if scalar == "String" { "" } else { ", Copy" }, e.name, scalar, e.name));
        for value in &e.values {
            let name = constant(&value.name);
            let value = &value.value;
            if scalar == "String" {
                out.push_str(&format!("pub const {name}: &'static str = {value};\n"));
            } else {
                out.push_str(&format!("pub const {name}: Self = Self({value});\n"));
            }
        }
        out.push_str("}\n");
        out.push_str(&format!("impl Encode for {} {{ fn encode(&self, out: &mut Encoder<'_>) -> Result<(), Error> {{ out.value(&self.0) }} }}\nimpl Decode for {} {{ fn decode(&mut self, input: &mut Decoder<'_>) -> Result<(), Error> {{ input.value(&mut self.0) }} }}\n", e.name, e.name));
        out.push_str(&enum_display(e));
    }
    for u in &schema.unions {
        out.push_str(&format!(
            "#[derive(Clone, Debug, Default, PartialEq)]\npub struct {} {{\n",
            u.name
        ));
        for m in &u.members {
            out.push_str(&format!(
                "pub {}: Option<Box<{}>>,\n",
                ident(&m.name),
                ty(&m.r#type)
            ));
        }
        out.push_str("}\n");
        out.push_str(&format!("impl Encode for {} {{ fn encode(&self, out: &mut Encoder<'_>) -> Result<(), Error> {{ crate::codec::union(out, {:?}, {}, &[\n", u.name, u.name, u.nullable));
        for m in &u.members {
            out.push_str(&format!(
                "self.{}.as_deref().map(|v| v as &dyn Encode),\n",
                ident(&m.name)
            ));
        }
        out.push_str("]) } }\n");
        out.push_str(&format!("impl Decode for {} {{ fn decode(&mut self, input: &mut Decoder<'_>) -> Result<(), Error> {{\n*self = Self::default();\n", u.name));
        if u.dispatch {
            out.push_str("match input.peek_kind() {\n");
            if u.nullable {
                out.push_str("Kind::Null => { input.read_token()?; Ok(()) },\n");
            }
            for g in &u.groups {
                let k = match g.kind.as_str() {
                    "string" => "Kind::String",
                    "number" => "Kind::Number",
                    "boolean" => "Kind::True | Kind::False",
                    "object" => "Kind::BeginObject",
                    "array" => "Kind::BeginArray",
                    other => return Err(format!("unknown JSON kind {other}")),
                };
                out.push_str(&format!("{k} => {{\n"));
                if g.fields.len() == 1 {
                    out.push_str(&direct(&g.fields[0], false));
                } else {
                    out.push_str("let data = input.read_value()?;\n");
                    out.push_str(&dispatch(&g.plan));
                    if !exhaustive(&g.plan) {
                        out.push_str(&format!(
                            "Err(crate::codec::invalid_value({:?}, &data))\n",
                            u.name
                        ));
                    }
                }
                out.push_str("},\n");
            }
            out.push_str(&format!(
                "kind => Err(crate::codec::invalid_kind({:?}, kind)),\n}}\n",
                u.name
            ));
        } else {
            out.push_str("let data = input.read_value()?;\n");
            if u.nullable {
                out.push_str("if data == b\"null\" { return Ok(()); }\n");
            }
            out.push_str(&dispatch(&u.fallback));
            if !exhaustive(&u.fallback) {
                out.push_str(&format!(
                    "Err(crate::codec::invalid_value({:?}, &data))\n",
                    u.name
                ));
            }
        }
        out.push_str("} }\n");
    }
    for literal in &schema.literals {
        let name = &literal.name;
        let json = &literal.json;
        out.push_str(&format!("#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]\npub struct {name};\nimpl Encode for {name} {{ fn encode(&self, out: &mut Encoder<'_>) -> Result<(), Error> {{ out.write_value(b{json:?}) }} }}\nimpl Decode for {name} {{ fn decode(&mut self, input: &mut Decoder<'_>) -> Result<(), Error> {{ crate::codec::literal(input, {name:?}, b{json:?}) }} }}\n"));
    }
    for a in &schema.aliases {
        out.push_str(&format!("pub type {} = {};\n", a.name, ty(&a.target.name)));
    }
    emit_registration(&mut out, &schema.registrations);
    out.push_str("pub mod methods { #[allow(clippy::wildcard_imports, reason = \"Method signatures are generated from the enclosing type inventory\")] use super::*;\n");
    for m in &schema.methods {
        let name = constant(&m.name);
        let params = m
            .params
            .as_ref()
            .map_or_else(|| "NoParams".into(), |t| ty(&t.name));
        if m.request {
            let result = if m.null_result {
                "Null".into()
            } else {
                m.result.as_ref().map_or_else(
                    || "Null".into(),
                    |t| {
                        if t.needs_pointer {
                            ty(&format!("*{}", t.name))
                        } else {
                            ty(&t.name)
                        }
                    },
                )
            };
            out.push_str(&format!("pub const {name}: crate::Request<{params}, {result}> = crate::Request::new({:?});\n", m.wire_name));
        } else {
            out.push_str(&format!("pub const {name}: crate::Notification<{params}> = crate::Notification::new({:?});\n", m.wire_name));
        }
    }
    out.push_str("}\n");
    Ok(out)
}

fn enum_display(e: &Enumeration) -> String {
    let mut out = format!("impl std::fmt::Display for {} {{ fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {{\n", e.name);
    if e.r#type == "string" {
        out.push_str("f.write_str(&self.0)\n");
    } else if e.bitflags {
        out.push_str("if self.0 == 0 { return f.write_str(\"0\"); }\nlet mut any = false;\n");
        let mut values: Vec<_> = e.values.iter().collect();
        values.sort_by_key(|v| v.value.as_i64().expect("numeric enum"));
        for value in values {
            out.push_str(&format!("if self.0 & {} != 0 {{ if any {{ f.write_str(\"|\")?; }} f.write_str({:?})?; any = true; }}\n", value.value, value.name));
        }
        out.push_str(&format!(
            "if any {{ Ok(()) }} else {{ write!(f, \"{}({{}})\", self.0) }}\n",
            e.name
        ));
    } else {
        out.push_str("match self.0 {\n");
        for value in &e.values {
            out.push_str(&format!(
                "{} => f.write_str({:?}),\n",
                value.value, value.name
            ));
        }
        out.push_str(&format!(
            "value => write!(f, \"{}({{value}})\"),\n}}\n",
            e.name
        ));
    }
    out.push_str("} }\n");
    if e.name == "ErrorCode" {
        out.push_str("impl std::error::Error for ErrorCode {}\n");
    }
    out
}

fn emit_registration(out: &mut String, registrations: &[Registration]) {
    out.push_str("#[derive(Clone, Debug, Default, PartialEq)]\npub struct RegisterOptions {\n");
    for r in registrations {
        out.push_str(&format!(
            "pub {}: Option<Box<{}>>,\n",
            ident(&r.field_name),
            ty(&r.options_type_name)
        ));
    }
    out.push_str("}\nimpl RegisterOptions { pub(crate) fn selected(&self) -> (&'static str, &dyn Encode) {\nlet values: &[Option<(&str, &dyn Encode)>] = &[\n");
    for r in registrations {
        out.push_str(&format!(
            "self.{}.as_deref().map(|v| ({:?}, v as &dyn Encode)),\n",
            ident(&r.field_name),
            r.method
        ));
    }
    out.push_str("];\nlet mut selected = values.iter().flatten();\nlet result = *selected.next().expect(\"RegisterOptions must have exactly one value set\");\nassert!(selected.next().is_none(), \"RegisterOptions must have exactly one value set\");\nresult\n}\npub(crate) fn decode_method(&mut self, method: &str, data: &[u8]) -> Result<(), Error> {\nmatch method {\n");
    for r in registrations {
        out.push_str(&format!("{:?} => {{ let mut value = Box::<{}>::default(); tsr_json::unmarshal(data, &mut value, tsr_json::Options::default())?; self.{} = Some(value); Ok(()) }},\n", r.method, ty(&r.options_type_name), ident(&r.field_name)));
    }
    out.push_str(
        "_ => Err(Error::Message(format!(\"unknown registration method: {method}\"))),\n} } }\n",
    );
}

pub(super) fn run(root: &Path, args: &[String], pin: &str) -> Result<bool, String> {
    let check = match args {
        [] => false,
        [arg] if arg == "--check" => true,
        _ => return Err("usage: cargo xtask gen lsproto [--check]".into()),
    };
    let stage = root.join("target/phase5");
    fs::create_dir_all(&stage).map_err(|e| e.to_string())?;
    let _lock = exclusive_lock(&stage.join("lsproto-generation.lock"))?;
    let schema_path = stage.join("lsproto.json");
    let status = Command::new("node")
        .arg(root.join("tools/phase5/lsproto/export.mjs"))
        .arg(&schema_path)
        .current_dir(root)
        .status()
        .map_err(|e| format!("LSP resolver: {e}"))?;
    if !status.success() {
        return Err(format!("LSP resolver failed: {status}"));
    }
    let schema: Schema =
        serde_json::from_slice(&fs::read(&schema_path).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
    if schema.version != 1 || schema.pin != pin {
        return Err("LSP export version/pin mismatch".into());
    }
    let bytes = format_rust(root, &emit(&schema)?, &workspace_edition(root)?)?;
    let path = root.join("crates/tsr_lsproto/src/generated.rs");
    let same = fs::read(&path).is_ok_and(|old| old == bytes);
    if !check {
        fs::write(&path, bytes).map_err(|e| e.to_string())?;
    }
    eprintln!(
        "LSP protocol: {} structures, {} unions, {} enums, {} methods{}",
        schema.structures.len(),
        schema.unions.len(),
        schema.enumerations.len(),
        schema.methods.len(),
        if check && !same {
            " (generated file differs)"
        } else {
            ""
        }
    );
    Ok(!check || same)
}
