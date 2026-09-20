//! Rust syntax emitters consume only the pinned resolver's normalized output.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use serde_json::Value;

pub(super) fn array<'a>(value: &'a Value, key: &str) -> Result<&'a [Value], String> {
    value[key]
        .as_array()
        .map(Vec::as_slice)
        .ok_or_else(|| format!("AST: missing array {key}"))
}
pub(super) fn string<'a>(value: &'a Value, key: &str) -> Result<&'a str, String> {
    value[key]
        .as_str()
        .ok_or_else(|| format!("AST: missing string {key}"))
}
pub(super) fn flag(value: &Value, key: &str) -> bool {
    // validate_flags checks every normalized flag before any emission.
    value[key] == true
}

fn validate_flags(schema: &Value) -> Result<(), String> {
    for node in array(schema, "nodes")? {
        let node_name = string(node, "name")?;
        for flag in ["handWritten", "handWrittenVisitor"] {
            if !node[flag].is_boolean() {
                return Err(format!("AST: {node_name}.{flag} must be a boolean"));
            }
        }
        for collection in ["fields", "members"] {
            for field in array(node, collection)? {
                let field_name = string(field, "name")?;
                for flag in [
                    "optional",
                    "private",
                    "inherited",
                    "goOnly",
                    "noGo",
                    "noTS",
                    "noFactory",
                    "kindParameter",
                    "child",
                ] {
                    if !field[flag].is_boolean() {
                        return Err(format!(
                            "AST: {node_name}.{field_name}.{flag} must be a boolean"
                        ));
                    }
                }
                for flag in ["visit", "bitmask"] {
                    if !field
                        .get(flag)
                        .is_some_and(|value| value.is_null() || value.is_string())
                    {
                        return Err(format!(
                            "AST: {node_name}.{field_name}.{flag} must be a string or null"
                        ));
                    }
                }
            }
        }
    }
    Ok(())
}
pub(super) fn snake(name: &str) -> String {
    let chars: Vec<_> = name.chars().collect();
    let mut result = String::new();
    for (i, ch) in chars.iter().copied().enumerate() {
        if ch.is_uppercase()
            && i > 0
            && (chars[i - 1].is_lowercase()
                || chars[i - 1].is_ascii_digit()
                || chars.get(i + 1).is_some_and(|next| next.is_lowercase()))
        {
            result.push('_');
        }
        result.extend(ch.to_lowercase());
    }
    // Raw identifiers cannot spell these keywords; casing has already mapped
    // `Self` to `self`. A suffix also handles the reserved single underscore.
    // https://doc.rust-lang.org/reference/identifiers.html#raw-identifiers
    if matches!(result.as_str(), "_" | "crate" | "self" | "super") {
        result.push('_');
    } else if matches!(
        result.as_str(),
        "as" | "async"
            | "await"
            | "break"
            | "const"
            | "continue"
            | "dyn"
            | "else"
            | "enum"
            | "extern"
            | "false"
            | "fn"
            | "for"
            | "if"
            | "impl"
            | "in"
            | "let"
            | "loop"
            | "match"
            | "mod"
            | "move"
            | "mut"
            | "pub"
            | "ref"
            | "return"
            | "static"
            | "struct"
            | "trait"
            | "true"
            | "type"
            | "unsafe"
            | "use"
            | "where"
            | "while"
            | "abstract"
            | "become"
            | "box"
            | "do"
            | "final"
            | "gen"
            | "macro"
            | "override"
            | "priv"
            | "try"
            | "typeof"
            | "unsized"
            | "virtual"
            | "yield"
    ) {
        // All strict/reserved keywords, including the 2024 reservation `gen`.
        // https://doc.rust-lang.org/reference/keywords.html
        result = format!("r#{result}");
    }
    result
}

pub(super) fn rust_type(value: &Value) -> Result<String, String> {
    Ok(match string(value, "kind")? {
        "node" => "NodeId".into(),
        "kind" => "NodeKind".into(),
        "alias" | "typeParameter" => rust_type(&value["resolved"])?,
        "primitive" => match string(value, "name")? {
            "string" => "JsString".into(),
            "bool" => "bool".into(),
            "TokenFlags" => "i32".into(),
            "NodeFlags" => "u32".into(),
            other => return Err(format!("AST: unhandled primitive {other}")),
        },
        "list" => {
            let element = rust_type(&value["element"])?;
            if element == "NodeId" {
                if value["list"] == "raw" {
                    "NodeSlice".into()
                } else {
                    "NodeListId".into()
                }
            } else if value["list"] == "raw" && element == "JsString" {
                "TextSlice".into()
            } else {
                return Err(format!("AST: unsupported non-node list {value}"));
            }
        }
        "union" => {
            // The resolver can classify unions containing hand-written TS-only
            // types (JsxTagNamePropertyAccess) as nodes. Keep its decision.
            if value["baseKind"] == "node" {
                return Ok("NodeId".into());
            }
            let members = array(value, "types")?
                .iter()
                .map(rust_type)
                .collect::<Result<BTreeSet<_>, _>>()?;
            if members.len() != 1 {
                return Err(format!("AST: incompatible resolved union {value}"));
            }
            members.into_iter().next().ok_or("AST: empty union")?
        }
        other => return Err(format!("AST: unknown normalized type {other}")),
    })
}

pub(super) fn deferred(node: &str, field: &Value) -> Result<Option<&'static str>, String> {
    let name = string(field, "name")?;
    if flag(field, "goOnly") {
        let (upstream_type, owner) = match name {
            "Symbol" | "LocalSymbol" => ("*Symbol", "binder"),
            "Locals" => ("SymbolTable", "binder"),
            "NextContainer" => ("*Node", "binder"),
            "FlowNode" | "FallthroughFlowNode" | "EndFlowNode" | "ReturnFlowNode" => {
                ("*FlowNode", "control-flow")
            }
            "facts" => ("atomic.Uint32", "subtree-facts cache"),
            _ => {
                return Err(format!(
                    "AST: new Go-only field needs an ownership decision: {name}"
                ))
            }
        };
        if field["type"]["kind"] != "primitive" || field["type"]["name"] != upstream_type {
            return Err(format!(
                "AST: changed runtime field needs an ownership decision: {node}.{name}"
            ));
        }
        return Ok(Some(owner));
    }
    if field["type"]["kind"] == "primitive" && field["type"]["name"] == "any" {
        if node == "SyntheticExpression" && name == "Type" {
            return Ok(Some("checker type"));
        }
        return Err(format!(
            "AST: new any field needs an ownership decision: {node}.{name}"
        ));
    }
    Ok(None)
}

pub(super) fn fields(node: &Value) -> Result<Vec<&Value>, String> {
    let name = string(node, "name")?;
    array(node, "fields")?
        .iter()
        .filter_map(|field| match deferred(name, field) {
            Ok(None) if field["name"] != "Flags" => Some(Ok(field)),
            Ok(_) => None,
            Err(error) => Some(Err(error)),
        })
        .collect()
}

pub(super) fn nullable(field: &Value) -> Result<bool, String> {
    // Go pointers remain nullable even for required TS schema properties, e.g.
    // DefaultClause.Expression. Optional scalar kinds remain scalar: Go uses
    // KindUnknown for ImportClause.PhaseModifier's absence. The TS `optional`
    // flag is retained in normalized metadata, not treated as storage evidence.
    Ok(matches!(
        rust_type(&field["type"])?.as_str(),
        "NodeId" | "NodeListId"
    ))
}

pub(super) fn boxed(node: &Value) -> Result<bool, String> {
    // S07's retained workload measures the enum's per-node cost. Keep common
    // identifiers and up to four graph edges inline; box larger payloads. The
    // generated crate's layout test verifies the ceiling on actual Rust.
    let mut budget = 0;
    for field in fields(node)? {
        budget += match rust_type(&field["type"])?.as_str() {
            "JsString" => 32,
            "NodeListId" => 8,
            "NodeSlice" | "TextSlice" => 16,
            typ if typ.starts_with("Box<[") => 16,
            _ => 8,
        };
    }
    Ok(budget > 32)
}

pub(super) fn children(node: &Value) -> Result<Vec<&Value>, String> {
    Ok(array(node, "members")?
        .iter()
        .filter(|field| flag(field, "child") && !flag(field, "noFactory") && !flag(field, "noGo"))
        .collect())
}

pub(super) fn has_declaration_name(node: &Value) -> Result<bool, String> {
    Ok(!flag(node, "handWritten")
        && array(node, "members")?.iter().any(|member| {
            member["name"] == "name" && flag(member, "private") && !flag(member, "noFactory")
        }))
}

pub(super) fn header(pin: &str, upstream: &str) -> String {
    format!("// Generated by cargo xtask gen from the pinned SchemaAPI; do not edit.\n// Upstream: {pin}\n// upstream: tsc/internal/ast/{upstream}\n\n")
}

pub(super) fn emit(schema: &Value, pin: &str) -> Result<BTreeMap<PathBuf, String>, String> {
    if schema["version"] != 1 {
        return Err("AST: unsupported normalized schema version".into());
    }
    validate_flags(schema)?;
    let kinds = array(schema, "kinds")?;
    let nodes = array(schema, "nodes")?;
    if kinds.is_empty() || nodes.is_empty() {
        return Err("AST: normalized kinds and nodes must not be empty".into());
    }
    let mut kind_names = BTreeSet::new();
    let mut kinds_code = header(pin, "kind_generated.go");
    kinds_code.push_str("#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]\n#[repr(u16)]\npub enum SyntaxKind {\n");
    for (ordinal, kind) in kinds.iter().enumerate() {
        let name = string(kind, "name")?;
        if kind["value"].as_u64() != Some(ordinal as u64)
            || !kind_names.insert(name)
            || ordinal > usize::from(u16::MAX)
        {
            return Err(format!("AST: invalid or duplicate kind {name}"));
        }
        kinds_code.push_str(&format!("    {name} = {ordinal},\n"));
    }
    kinds_code.push_str("}\nimpl SyntaxKind {\n");
    kinds_code.push_str(&format!("    pub const COUNT: usize = {};\n", kinds.len()));
    kinds_code.push_str("    pub const ALL: &'static [Self] = &[\n");
    for kind in kinds {
        kinds_code.push_str(&format!("        Self::{},\n", string(kind, "name")?));
    }
    kinds_code.push_str("    ];\n    pub fn from_u16(value: u16) -> Option<Self> { Self::ALL.get(usize::from(value)).copied() }\n");
    kinds_code.push_str("    pub fn as_str(self) -> &'static str { match self {\n");
    for kind in kinds {
        let name = string(kind, "name")?;
        kinds_code.push_str(&format!("        Self::{name} => \"{name}\",\n"));
    }
    kinds_code.push_str("    }}\n");
    for marker in array(schema, "markers")? {
        let name = string(marker, "name")?;
        let value = string(marker, "value")?;
        if !kind_names.contains(value) {
            return Err(format!("AST: unknown marker target {value}"));
        }
        kinds_code.push_str(&format!("    #[allow(non_upper_case_globals)] // Preserve upstream marker names.\n    pub const {name}: Self = Self::{value};\n"));
    }
    for alias in array(schema, "kindAliases")? {
        let name = snake(string(alias, "name")?).replace("_syntax", "");
        let members = array(alias, "kinds")?
            .iter()
            .map(|kind| {
                let kind = kind.as_str().ok_or("AST: malformed alias kind")?;
                if !kind_names.contains(kind) {
                    return Err(format!("AST: unknown alias kind {kind}"));
                }
                Ok(format!("Self::{kind}"))
            })
            .collect::<Result<Vec<_>, String>>()?;
        if members.is_empty() {
            return Err(format!("AST: empty kind alias {name}"));
        }
        kinds_code.push_str(&format!(
            "    pub fn is_{name}(self) -> bool {{ matches!(self, {}) }}\n",
            members.join(" | ")
        ));
    }
    kinds_code.push_str("}\n");

    let mut data = header(pin, "ast_generated.go");
    data.push_str("use crate::{DeferredField, JsString, NodeId, NodeListId, NodeSlice, TextSlice, NodeKind};\n\n");
    let mut accessors = header(pin, "ast_generated.go");
    accessors.push_str("use crate::{NodeData, SyntaxKind,\n");
    for node in nodes {
        accessors.push_str(&format!("    {}Data,\n", string(node, "name")?));
    }
    accessors.push_str("};\n\nimpl NodeData {\n");
    let mut visitors = header(pin, "ast_generated.go");
    visitors.push_str("use crate::{ChildMapper, ChildRole, ChildVisitor, NodeData};\nuse std::ops::ControlFlow;\n\nimpl NodeData {\n    pub fn for_each_child(&self, visitor: &mut impl ChildVisitor) -> ControlFlow<()> {\n        match self {\n");
    let mut node_names = BTreeSet::new();
    let mut leaf_patterns = Vec::new();
    for node in nodes {
        let name = string(node, "name")?;
        if !node_names.insert(name) {
            return Err(format!("AST: duplicate node {name}"));
        }
        if flag(node, "handWritten") && name != "SourceFile" {
            return Err(format!(
                "AST: handwritten node needs an explicit storage decision: {name}"
            ));
        }
        if flag(node, "handWrittenVisitor") && name != "JSDocParameterOrPropertyTag" {
            return Err(format!(
                "AST: missing handwritten visitor adapter for {name}"
            ));
        }
        let node_flags = array(node, "fields")?
            .iter()
            .filter(|field| field["name"] == "Flags")
            .collect::<Vec<_>>();
        if node_flags.len() != 1
            || node_flags[0]["type"] != serde_json::json!({"kind":"primitive", "name":"NodeFlags"})
        {
            return Err(format!(
                "AST: missing or changed common NodeFlags field for {name}"
            ));
        }
        data.push_str(&format!(
            "#[derive(Clone, Debug, PartialEq, Eq)]\npub struct {name}Data {{\n"
        ));
        let node_fields = fields(node)?;
        let mut field_names = BTreeSet::new();
        for field in &node_fields {
            let field_name = snake(string(field, "name")?);
            if !field_names.insert(field_name.clone()) {
                return Err(format!("AST: duplicate field {name}.{field_name}"));
            }
            let mut typ = rust_type(&field["type"])?;
            if nullable(field)? {
                typ = format!("Option<{typ}>");
            }
            data.push_str(&format!("    pub {field_name}: {typ},\n"));
        }
        data.push_str("}\n\n");
        let snake_name = snake(name);
        accessors.push_str(&format!("    pub fn as_{snake_name}(&self) -> Option<&{name}Data> {{\n        if let Self::{name}(data) = self {{ Some(data) }} else {{ None }}\n    }}\n"));
        let child_fields = children(node)?;
        for child in &child_fields {
            let stored = node_fields
                .iter()
                .find(|field| field["name"] == child["name"])
                .ok_or_else(|| {
                    format!("AST: child has no storage field: {name}.{}", child["name"])
                })?;
            if rust_type(&stored["type"])? != rust_type(&child["type"])? {
                return Err(format!(
                    "AST: child/storage type mismatch: {name}.{}",
                    child["name"]
                ));
            }
        }
        if child_fields.is_empty() {
            leaf_patterns.push(format!("Self::{name}(_)"));
        } else {
            visitors.push_str(&format!("            Self::{name}(data) => {{\n"));
            if flag(node, "handWrittenVisitor") {
                if name != "JSDocParameterOrPropertyTag" {
                    return Err(format!(
                        "AST: missing handwritten visitor adapter for {name}"
                    ));
                }
                emit_visit(
                    &mut visitors,
                    child_fields
                        .iter()
                        .find(|field| field["name"] == "TagName")
                        .ok_or("AST: missing tag name")?,
                    "                ",
                )?;
                visitors.push_str("                if data.is_name_first {\n");
                for member in ["name", "TypeExpression"] {
                    emit_visit(
                        &mut visitors,
                        child_fields
                            .iter()
                            .find(|field| field["name"] == member)
                            .ok_or("AST: missing JSDoc ordered child")?,
                        "                    ",
                    )?;
                }
                visitors.push_str("                } else {\n");
                for member in ["TypeExpression", "name"] {
                    emit_visit(
                        &mut visitors,
                        child_fields
                            .iter()
                            .find(|field| field["name"] == member)
                            .ok_or("AST: missing JSDoc ordered child")?,
                        "                    ",
                    )?;
                }
                visitors.push_str("                }\n");
                emit_visit(
                    &mut visitors,
                    child_fields
                        .iter()
                        .find(|field| field["name"] == "Comment")
                        .ok_or("AST: missing tag comment")?,
                    "                ",
                )?;
            } else {
                for field in child_fields {
                    emit_visit(&mut visitors, field, "                ")?;
                }
            }
            visitors.push_str("            },\n");
        }
    }
    visitors.push_str(&format!(
        "            {} => {{}},\n",
        leaf_patterns.join(" | ")
    ));
    visitors.push_str("        }\n        ControlFlow::Continue(())\n    }\n    #[must_use]\n    pub fn map_children(&self, mapper: &mut impl ChildMapper) -> Self {\n        let mut result = self.clone();\n        match &mut result {\n");
    for node in nodes {
        let name = string(node, "name")?;
        let child_fields = children(node)?;
        if child_fields.is_empty() {
            continue;
        }
        visitors.push_str(&format!("            Self::{name}(data) => {{\n"));
        for field in child_fields {
            let field_name = string(field, "name")?;
            let role = child_role(name, field)?;
            let name = snake(field_name);
            let method = match rust_type(&field["type"])?.as_str() {
                "NodeListId" => "map_list",
                "NodeSlice" => "map_node_slice",
                _ => "map_node",
            };
            let expression = if nullable(field)? {
                format!("data.{name}.map(|child| mapper.{method}(child, ChildRole::{role}))")
            } else {
                format!("mapper.{method}(data.{name}, ChildRole::{role})")
            };
            visitors.push_str(&format!("                data.{name} = {expression};\n"));
        }
        visitors.push_str("            },\n");
    }
    visitors.push_str(&format!(
        "            {} => {{}},\n",
        leaf_patterns.join(" | ")
    ));
    visitors.push_str("        }\n        result\n    }\n}\n");
    data.push_str("// Payloads with a conservative field budget above 32 bytes are boxed.\n// Common identifier/edge payloads stay inline; accessors hide storage.\n#[derive(Clone, Debug, PartialEq, Eq)]\npub enum NodeData {\n");
    for node in nodes {
        let name = string(node, "name")?;
        let typ = if boxed(node)? {
            format!("Box<{name}Data>")
        } else {
            format!("{name}Data")
        };
        data.push_str(&format!("    {name}({typ}),\n"));
    }
    data.push_str("}\n\n");
    for node in nodes {
        let name = string(node, "name")?;
        let value = if boxed(node)? {
            "Box::new(data)"
        } else {
            "data"
        };
        data.push_str(&format!("impl From<{name}Data> for NodeData {{\n    fn from(data: {name}Data) -> Self {{ Self::{name}({value}) }}\n}}\n"));
    }
    data.push_str("\npub const DEFERRED_FIELDS: &[DeferredField] = &[\n");
    for node in nodes {
        for field in array(node, "fields")? {
            if let Some(owner) = deferred(string(node, "name")?, field)? {
                data.push_str(&format!("    DeferredField {{ node: {:?}, field: {:?}, upstream_type: {:?}, owner: {owner:?} }},\n", string(node, "name")?, string(field, "name")?, string(&field["type"], "name")?));
            }
        }
    }
    data.push_str("];\n");
    accessors.push_str("    pub fn name(&self) -> &'static str { match self {\n");
    for node in nodes {
        let name = string(node, "name")?;
        accessors.push_str(&format!("        Self::{name}(_) => \"{name}\",\n"));
    }
    accessors.push_str(
        "    }}\n    pub fn supports_kind(&self, kind: SyntaxKind) -> bool { match self {\n",
    );
    for node in nodes {
        let name = string(node, "name")?;
        let node_kinds = array(node, "kinds")?
            .iter()
            .map(|kind| {
                let kind = kind.as_str().ok_or("AST: malformed node kind")?;
                if !kind_names.contains(kind) {
                    return Err(format!("AST: unknown {name} kind {kind}"));
                }
                Ok(format!("SyntaxKind::{kind}"))
            })
            .collect::<Result<Vec<_>, String>>()?;
        if node_kinds.is_empty() {
            return Err(format!("AST: no syntax kinds for {name}"));
        }
        accessors.push_str(&format!(
            "        Self::{name}(_) => matches!(kind, {}),\n",
            node_kinds.join(" | ")
        ));
    }
    accessors.push_str("    }}\n}\n");
    Ok([
        ("kinds_generated.rs", kinds_code),
        ("data_generated.rs", data),
        ("accessors_generated.rs", accessors),
        (
            "compact_generated.rs",
            super::ast_compact::emit(nodes, pin)?,
        ),
        ("node_read_generated.rs", super::ast_read::emit(nodes, pin)?),
        (
            "local_read_generated.rs",
            super::ast_local_read::emit(nodes, pin)?,
        ),
        ("visitors_generated.rs", visitors),
    ]
    .into_iter()
    .map(|(name, code)| (PathBuf::from("crates/tsr_ast/src").join(name), code))
    .collect())
}

fn emit_visit(code: &mut String, field: &Value, indent: &str) -> Result<(), String> {
    let name = snake(string(field, "name")?);
    let method = match rust_type(&field["type"])?.as_str() {
        "NodeListId" => "visit_list",
        "NodeSlice" => "visit_node_slice",
        _ => "visit_node",
    };
    if nullable(field)? {
        code.push_str(&format!(
            "{indent}if let Some(child) = data.{name} {{ visitor.{method}(child)?; }}\n"
        ));
    } else {
        code.push_str(&format!("{indent}visitor.{method}(data.{name})?;\n"));
    }
    Ok(())
}

pub(super) fn child_role<'a>(node_name: &str, field: &'a Value) -> Result<&'a str, String> {
    if node_name == "SourceFile" && field["name"] == "EndOfFileToken" {
        // SourceFile's visitor is hand-written in ast.go.
        return Ok("Token");
    }
    Ok(match field["visit"].as_str() {
        Some("embeddedStatement") => "EmbeddedStatement",
        Some("iterationBody") => "IterationBody",
        Some("modifiers") => "Modifiers",
        Some("parameters") => "Parameters",
        Some("functionBody") => "FunctionBody",
        Some("topLevelStatements") => "TopLevelStatements",
        Some(other) => return Err(format!("AST: unsupported visitor role {other}")),
        None if field["type"]["list"] == "raw" => "RawNodes",
        None if field["type"]["list"] == "ModifierList" => "Modifiers",
        None if rust_type(&field["type"])? == "NodeListId" => "Nodes",
        None => "Node",
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn preserves_acronym_boundaries_and_rust_keywords() {
        assert_eq!(snake("JSDocTypeExpression"), "js_doc_type_expression");
        assert_eq!(snake("Type"), "r#type");
        assert_eq!(snake("EOFToken"), "eof_token");
    }

    #[test]
    fn keyword_fields_compile_without_illegal_raw_identifiers() {
        use std::io::Write;
        use std::process::{Command, Stdio};

        // Strict and reserved keywords from the Rust Reference. Compile the
        // resulting field identifiers so a mistaken raw spelling fails too.
        let keywords = [
            "_", "as", "async", "await", "break", "const", "continue", "crate", "dyn", "else",
            "enum", "extern", "false", "fn", "for", "if", "impl", "in", "let", "loop", "match",
            "mod", "move", "mut", "pub", "ref", "return", "self", "Self", "static", "struct",
            "super", "trait", "true", "type", "unsafe", "use", "where", "while", "abstract",
            "become", "box", "do", "final", "gen", "macro", "override", "priv", "try", "typeof",
            "unsized", "virtual", "yield",
        ];
        let mut source = String::from("#![allow(dead_code)]\n");
        for (index, keyword) in keywords.iter().enumerate() {
            let escaped = snake(keyword);
            let expected = match *keyword {
                "_" => "__".into(),
                "Self" | "self" => "self_".into(),
                "crate" | "super" => format!("{keyword}_"),
                other => format!("r#{other}"),
            };
            assert_eq!(escaped, expected, "keyword {keyword}");
            source.push_str(&format!("struct Keyword{index} {{ {escaped}: u8 }}\n"));
        }
        for keyword in ["macro_rules", "raw", "safe", "union"] {
            assert_eq!(snake(keyword), keyword, "weak keywords remain field names");
        }
        for keyword in ["Return", "Impl", "Crate", "Super"] {
            assert_eq!(snake(keyword), snake(&keyword.to_lowercase()));
        }
        let mut child = Command::new("rustc")
            .args([
                "--edition=2021",
                "--crate-type=lib",
                "--emit=metadata",
                "-o",
                "-",
                "-",
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .expect("Rust toolchain is required for emitter tests");
        child
            .stdin
            .take()
            .unwrap()
            .write_all(source.as_bytes())
            .unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn refuses_unresolved_or_mixed_types() {
        assert!(rust_type(&json!({"kind":"primitive", "name":"FutureRuntimeType"})).is_err());
        assert!(rust_type(&json!({"kind":"union", "types":[{"kind":"node","name":"Node"},{"kind":"primitive","name":"string"}]})).is_err());
        assert!(deferred("Node", &json!({"name":"NewCache","goOnly":true})).is_err());
        assert!(deferred(
            "Node",
            &json!({"name":"Type","type":{"kind":"primitive","name":"any"}})
        )
        .is_err());
    }

    fn minimal_schema() -> Value {
        json!({
            "version": 1,
            "kinds": [{"name": "Unknown", "value": 0}],
            "markers": [], "kindAliases": [],
            "nodes": [{"name": "Token", "kinds": ["Unknown"], "members": [],
                "handWritten": false, "handWrittenVisitor": false, "baseTypes": [],
                "fields": [member("Flags", &json!({"kind":"primitive","name":"NodeFlags"}))]}]
        })
    }

    fn member(name: &str, typ: &Value) -> Value {
        json!({"name": name, "type": typ, "optional": false, "private": false,
            "inherited": false, "goOnly": false, "noGo": false, "noTS": false,
            "noFactory": false, "kindParameter": false, "child": false,
            "visit": null, "bitmask": null})
    }

    #[test]
    fn refuses_duplicate_kinds_unknown_targets_and_changed_header() {
        let schema = minimal_schema();
        assert!(emit(&schema, "pin").is_ok());
        let mut changed = schema.clone();
        changed["kinds"]
            .as_array_mut()
            .unwrap()
            .push(json!({"name":"Unknown","value":1}));
        assert!(emit(&changed, "pin")
            .unwrap_err()
            .contains("duplicate kind"));
        let mut changed = schema.clone();
        changed["nodes"][0]["kinds"] = json!(["Missing"]);
        assert!(emit(&changed, "pin")
            .unwrap_err()
            .contains("unknown Token kind"));
        let mut changed = schema;
        changed["nodes"][0]["fields"][0]["type"]["name"] = json!("int");
        assert!(emit(&changed, "pin").unwrap_err().contains("NodeFlags"));
    }

    #[test]
    fn refuses_unhandled_handwritten_nodes_and_dangling_children() {
        let schema = minimal_schema();
        let mut changed = schema.clone();
        changed["nodes"][0]["handWrittenVisitor"] = json!(true);
        assert!(emit(&changed, "pin")
            .unwrap_err()
            .contains("handwritten visitor"));
        let mut changed = schema.clone();
        changed["nodes"][0]["handWritten"] = json!(true);
        assert!(emit(&changed, "pin")
            .unwrap_err()
            .contains("handwritten node"));
        let mut changed = schema;
        let mut child = member("Child", &json!({"kind":"node","name":"Node"}));
        child["child"] = json!(true);
        changed["nodes"][0]["members"] = json!([child]);
        assert!(emit(&changed, "pin")
            .unwrap_err()
            .contains("child has no storage field"));
        assert!(child_role("Node", &json!({"visit":"futureRole"})).is_err());
    }

    #[test]
    fn malformed_or_missing_flags_cannot_remove_special_visitors_or_children() {
        for invalid in [json!("true"), json!(1), Value::Null] {
            let mut schema = minimal_schema();
            schema["nodes"][0]["handWrittenVisitor"] = invalid.clone();
            assert!(emit(&schema, "pin")
                .unwrap_err()
                .contains("handWrittenVisitor must be a boolean"));
            let mut schema = minimal_schema();
            schema["nodes"][0]["fields"][0]["child"] = invalid;
            assert!(emit(&schema, "pin")
                .unwrap_err()
                .contains("child must be a boolean"));
        }
        let mut schema = minimal_schema();
        schema["nodes"][0]
            .as_object_mut()
            .unwrap()
            .remove("handWrittenVisitor");
        assert!(emit(&schema, "pin")
            .unwrap_err()
            .contains("handWrittenVisitor must be a boolean"));
        let mut schema = minimal_schema();
        schema["nodes"][0]["fields"][0]
            .as_object_mut()
            .unwrap()
            .remove("optional");
        assert!(emit(&schema, "pin")
            .unwrap_err()
            .contains("optional must be a boolean"));
        let mut schema = minimal_schema();
        schema["nodes"][0]["fields"][0]["visit"] = json!(42);
        assert!(emit(&schema, "pin")
            .unwrap_err()
            .contains("visit must be a string or null"));
    }
}
