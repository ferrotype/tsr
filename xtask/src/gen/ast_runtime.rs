//! Runtime algorithms follow the pinned generate-go-ast.ts rules over resolver facts.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use serde_json::{json, Value};

use super::ast::{array, child_role, fields, flag, header, nullable, rust_type, snake, string};

const SOURCE: &str = "tsc/internal/ast/ast_generated.go";

fn marker(receiver: &str, name: &str) -> String {
    if receiver.is_empty() {
        format!("{SOURCE}:{name}")
    } else {
        format!("{SOURCE}:{receiver}.{name}")
    }
}

fn mapped(code: &mut String, scope: &mut BTreeSet<String>, receiver: &str, name: &str) {
    let id = marker(receiver, name);
    code.push_str(&format!("    // upstream: {id}\n"));
    assert!(scope.insert(id), "duplicate generated runtime function");
}

fn members(node: &Value) -> Result<Vec<&Value>, String> {
    Ok(array(node, "members")?
        .iter()
        .filter(|m| !flag(m, "noFactory"))
        .collect())
}

/// Public child enumeration selects kinds before checking the concrete shape.
/// Payloads on unhandled token/unknown kinds have no public children.
pub(super) fn child_kind_pattern(node: &Value) -> Result<Option<String>, String> {
    if !members(node)?.iter().any(|member| flag(member, "child")) && !flag(node, "handWritten") {
        return Ok(None);
    }
    Ok(Some(format!(
        "Some({})",
        strings(array(node, "kinds")?)?
            .iter()
            .map(|kind| format!("SyntaxKind::{kind}"))
            .collect::<Vec<_>>()
            .join(" | ")
    )))
}

fn parameter_type(member: &Value) -> Result<String, String> {
    let typ = rust_type(&member["type"])?;
    Ok(if nullable(member)? {
        format!("Option<{typ}>")
    } else {
        typ
    })
}

fn params(members: &[&Value]) -> Result<String, String> {
    members
        .iter()
        .map(|m| {
            Ok(format!(
                ", {}: {}",
                snake(string(m, "name")?),
                parameter_type(m)?
            ))
        })
        .collect()
}

fn flags_member(member: &Value) -> bool {
    member["type"]["kind"] == "primitive" && member["type"]["name"] == "NodeFlags"
}

fn mask(member: &Value) -> Result<Option<&'static str>, String> {
    Ok(match member["bitmask"].as_str() {
        None => None,
        Some("NodeFlagsOptionalChain") => Some("crate::node_flags::OPTIONAL_CHAIN"),
        Some("TokenFlagsNumericLiteralFlags") => Some("crate::token_flags::NUMERIC_LITERAL_FLAGS"),
        Some("TokenFlagsRegularExpressionLiteralFlags") => {
            Some("crate::token_flags::REGULAR_EXPRESSION_LITERAL_FLAGS")
        }
        Some("TokenFlagsStringLiteralFlags") => Some("crate::token_flags::STRING_LITERAL_FLAGS"),
        Some("TokenFlagsTemplateLiteralLikeFlags") => {
            Some("crate::token_flags::TEMPLATE_LITERAL_LIKE_FLAGS")
        }
        Some(other) => return Err(format!("AST runtime: unsupported factory bitmask {other}")),
    })
}

fn value(member: &Value) -> Result<String, String> {
    let name = snake(string(member, "name")?);
    Ok(mask(member)?.map_or_else(|| name.clone(), |mask| format!("{name} & {mask}")))
}

fn default_value(member: &Value) -> Result<&'static str, String> {
    if nullable(member)? {
        return Ok("None");
    }
    Ok(match rust_type(&member["type"])?.as_str() {
        "NodeKind" => "SyntaxKind::Unknown.into()",
        "bool" => "false",
        "i32" | "u32" => "0",
        "JsString" => "JsString::default()",
        "NodeSlice" => "NodeSlice::default()",
        "TextSlice" => "TextSlice::default()",
        other => return Err(format!("AST runtime: no source zero value for {other}")),
    })
}

fn text_content(member: &Value) -> bool {
    let typ = &member["type"];
    (typ["kind"] == "primitive" && typ["name"] == "string")
        || (typ["kind"] == "list"
            && typ["list"] == "raw"
            && typ["element"]["kind"] == "primitive"
            && typ["element"]["name"] == "string")
}

/// Concrete entry points preserve interception by custom Factory implementations.
/// The ordinary AstBuilder path selects the checked row without an owned enum.
fn emit_construction_methods(code: &mut String, nodes: &[Value]) -> Result<(), String> {
    code.push_str("macro_rules! factory_construction_methods {\n    (defaults) => {\n");
    for node in nodes {
        let name = string(node, "name")?;
        let member = snake(name);
        code.push_str(&format!("        fn new_{member}_data(&mut self, kind: $crate::NodeKind, data: $crate::{name}Data) -> $crate::NodeId {{\n            self.new_node(kind, data.into())\n        }}\n"));
    }
    code.push_str("    };\n    (builder) => {\n");
    for node in nodes {
        let name = string(node, "name")?;
        let member = snake(name);
        code.push_str(&format!("        fn new_{member}_data(&mut self, kind: $crate::NodeKind, data: $crate::{name}Data) -> $crate::NodeId {{\n"));
        let mut references = Vec::new();
        for field in fields(node)? {
            let accessor = match rust_type(&field["type"])?.as_str() {
                "NodeId" => "node",
                "NodeListId" => "list",
                "NodeSlice" => "node_slice",
                "TextSlice" => "text_slice",
                _ => continue,
            };
            references.push((field, accessor));
        }
        if !references.is_empty() {
            code.push_str("            let view = self.view();\n");
        }
        for (field, accessor) in references {
            let member = snake(string(field, "name")?);
            if nullable(field)? {
                code.push_str(&format!("            if let Some(id) = data.{member} {{ view.{accessor}(id).expect(\"factory edges belong to retained storage\"); }}\n"));
            } else {
                code.push_str(&format!("            view.{accessor}(data.{member}).expect(\"factory edges belong to retained storage\");\n"));
            }
        }
        code.push_str(&format!("            let created = self.new_typed_node_before_hook(kind, |payloads, context| payloads.insert_{member}(data, context));\n            self.run_create_hook(created);\n            created\n        }}\n"));
    }
    code.push_str("    };\n    (forward, $field:tt) => {\n");
    for node in nodes {
        let name = string(node, "name")?;
        let member = snake(name);
        code.push_str(&format!("        fn new_{member}_data(&mut self, kind: $crate::NodeKind, data: $crate::{name}Data) -> $crate::NodeId {{\n            self.$field.new_{member}_data(kind, data)\n        }}\n"));
    }
    code.push_str("    };\n}\npub(crate) use factory_construction_methods;\n\n");
    Ok(())
}

fn emit_new(
    code: &mut String,
    scope: &mut BTreeSet<String>,
    node: &Value,
    alias: Option<&str>,
) -> Result<(), String> {
    let name = string(node, "name")?;
    let factory_name = alias.unwrap_or(name);
    let ms = members(node)?;
    mapped(code, scope, "NodeFactory", &format!("New{factory_name}"));
    code.push_str(&format!(
        "    fn new_{}(&mut self{}) -> NodeId {{\n        let data = {name}Data {{\n",
        snake(factory_name),
        params(&ms)?
    ));
    for field in fields(node)? {
        let field_name = string(field, "name")?;
        let initializer = if let Some(member) = ms.iter().find(|m| m["name"] == field_name) {
            value(member)?
        } else {
            default_value(field)?.into()
        };
        code.push_str(&format!(
            "            {}: {initializer},\n",
            snake(field_name)
        ));
    }
    code.push_str("        };\n");
    if ms.iter().any(|member| text_content(member)) {
        code.push_str("        self.increment_text_count();\n");
    }
    let kind = if let Some(member) = ms.iter().find(|m| flag(m, "kindParameter")) {
        snake(string(member, "name")?)
    } else {
        format!(
            "SyntaxKind::{}.into()",
            alias.unwrap_or(string(node, "syntaxKindName")?)
        )
    };
    if !ms.iter().any(|m| flags_member(m)) {
        code.push_str(&format!(
            "        self.new_{}_data({kind}, data)\n    }}\n",
            snake(name)
        ));
        return Ok(());
    }
    code.push_str(&format!(
        "        let created = self.new_{}_data({kind}, data);\n",
        snake(name)
    ));
    for member in ms.iter().filter(|m| flags_member(m)) {
        let assigned = value(member)?;
        let assigned = if mask(member)?.is_some() {
            format!("self.node(created).flags() | ({assigned})")
        } else {
            assigned
        };
        if mask(member)?.is_some() {
            code.push_str(&format!("        let created_flags = {assigned};\n        self.set_node_flags(created, created_flags);\n"));
        } else {
            code.push_str(&format!(
                "        self.set_node_flags(created, {assigned});\n"
            ));
        }
    }
    code.push_str("        created\n    }\n");
    Ok(())
}

fn field_access(member: &Value) -> Result<String, String> {
    if flags_member(member) {
        Ok("original.flags()".into())
    } else {
        Ok(format!("data.{}()", snake(string(member, "name")?)))
    }
}

fn emit_creation_dispatch(
    code: &mut String,
    node: &Value,
    operation: &str,
    args: &str,
) -> Result<(), String> {
    let name = string(node, "name")?;
    let aliases = array(node, "kindAliases")?;
    if aliases.is_empty() {
        code.push_str(&format!(
            "        let created = self.new_{}({args});\n",
            snake(name)
        ));
    } else {
        code.push_str("        let created = match original_kind.known() {\n");
        for (kind, factory) in std::iter::once((string(node, "syntaxKindName")?, name)).chain(
            aliases
                .iter()
                .map(|v| {
                    v.as_str()
                        .map(|s| (s, s))
                        .ok_or("AST runtime: malformed factory alias")
                })
                .collect::<Result<Vec<_>, _>>()?,
        ) {
            code.push_str(&format!(
                "            Some(SyntaxKind::{kind}) => self.new_{}({args}),\n",
                snake(factory)
            ));
        }
        let message = if operation == "Update" {
            format!("unexpected kind in Update{name}: ")
        } else {
            format!("unexpected kind in {name}.Clone: ")
        };
        code.push_str(&format!(
            "            _ => panic!(\"{{}}{{}}\", {message:?}, original_kind),\n        }};\n"
        ));
    }
    Ok(())
}

fn emit_update(
    code: &mut String,
    scope: &mut BTreeSet<String>,
    node: &Value,
) -> Result<(), String> {
    let name = string(node, "name")?;
    let ms = members(node)?;
    let updates = ms
        .iter()
        .copied()
        .filter(|m| !flag(m, "kindParameter"))
        .collect::<Vec<_>>();
    if !updates.iter().any(|m| flag(m, "child")) {
        return Ok(());
    }
    mapped(code, scope, "NodeFactory", &format!("Update{name}"));
    code.push_str(&format!("    fn update_{}(&mut self, original_id: NodeId{}) -> NodeId {{\n        let original = self.node(original_id);\n        let data = original.as_{}().expect(\"Update{name} requires {name} payload\");\n",snake(name),params(&updates)?,snake(name)));
    let comparisons = updates
        .iter()
        .map(|m| {
            let param = snake(string(m, "name")?);
            let access = field_access(m)?;
            Ok(match rust_type(&m["type"])?.as_str() {
                "NodeSlice" | "TextSlice" => format!("{param}.same({access})"),
                "JsString" => format!("{param}.as_bytes() == {access}"),
                _ => format!("{param} == {access}"),
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    code.push_str(&format!(
        "        if {} {{ return original_id; }}\n",
        comparisons.join(" && ")
    ));
    let needs_kind =
        !array(node, "kindAliases")?.is_empty() || ms.iter().any(|m| flag(m, "kindParameter"));
    if needs_kind {
        code.push_str("        let original_kind = original.kind();\n");
    }
    code.push_str("        drop(original);\n");
    let args = ms
        .iter()
        .map(|m| {
            Ok(if flag(m, "kindParameter") {
                "original_kind".into()
            } else {
                snake(string(m, "name")?)
            })
        })
        .collect::<Result<Vec<String>, String>>()?
        .join(", ");
    emit_creation_dispatch(code, node, "Update", &args)?;
    code.push_str("        self.finish_update(created, original_id)\n    }\n");
    Ok(())
}

fn snapshot(
    code: &mut String,
    node: &Value,
    ms: &[&Value],
    retain_kind: bool,
) -> Result<(), String> {
    let name = string(node, "name")?;
    code.push_str("        let original = self.node(original_id);\n");
    if ms
        .iter()
        .any(|m| !flag(m, "kindParameter") && !flags_member(m))
    {
        code.push_str(&format!(
            "        let data = original.as_{}().expect(\"operation requires {name} payload\");\n",
            snake(name)
        ));
    } else {
        code.push_str(&format!(
            "        original.as_{}().expect(\"operation requires {name} payload\");\n",
            snake(name)
        ));
    }
    if retain_kind
        && (!array(node, "kindAliases")?.is_empty() || ms.iter().any(|m| flag(m, "kindParameter")))
    {
        code.push_str("        let original_kind = original.kind();\n");
    }
    for m in ms.iter().filter(|m| !flag(m, "kindParameter")) {
        let param = snake(string(m, "name")?);
        let access = if rust_type(&m["type"])? == "JsString" {
            format!("data.{}()", super::ast_read::owned_method(&param))
        } else {
            field_access(m)?
        };
        code.push_str(&format!("        let {param} = {access};\n"));
    }
    code.push_str("        drop(original);\n");
    Ok(())
}

fn emit_clone(code: &mut String, scope: &mut BTreeSet<String>, node: &Value) -> Result<(), String> {
    let name = string(node, "name")?;
    let ms = members(node)?;
    mapped(code, scope, name, "Clone");
    code.push_str(&format!(
        "    fn clone_{}(&mut self, original_id: NodeId) -> NodeId {{\n",
        snake(name)
    ));
    snapshot(code, node, &ms, true)?;
    let args = ms
        .iter()
        .map(|m| {
            Ok(if flag(m, "kindParameter") {
                "original_kind".into()
            } else {
                snake(string(m, "name")?)
            })
        })
        .collect::<Result<Vec<String>, String>>()?
        .join(", ");
    emit_creation_dispatch(code, node, "Clone", &args)?;
    code.push_str("        self.finish_clone(created, original_id)\n    }\n");
    Ok(())
}

fn emit_transform(
    code: &mut String,
    scope: &mut BTreeSet<String>,
    node: &Value,
) -> Result<(), String> {
    let name = string(node, "name")?;
    let ms = members(node)?
        .into_iter()
        .filter(|m| !flag(m, "kindParameter"))
        .collect::<Vec<_>>();
    if !ms.iter().any(|m| flag(m, "child")) {
        return Ok(());
    }
    mapped(code, scope, name, "VisitEachChild");
    code.push_str(&format!(
        "    fn visit_each_child_{}(&mut self, original_id: NodeId) -> NodeId {{\n",
        snake(name)
    ));
    snapshot(code, node, &ms, false)?;
    // Go computes SameMap raw-list locals before evaluating Update arguments.
    for m in ms
        .iter()
        .filter(|m| flag(m, "child") && m["type"]["list"] == "raw")
    {
        let param = snake(string(m, "name")?);
        code.push_str(&format!(
            "        let {param} = self.map_raw_nodes({param});\n"
        ));
    }
    for m in ms
        .iter()
        .filter(|m| flag(m, "child") && m["type"]["list"] != "raw")
    {
        let param = snake(string(m, "name")?);
        let method = if rust_type(&m["type"])? == "NodeListId" {
            "visit_list"
        } else {
            "visit_node"
        };
        code.push_str(&format!(
            "        let {param} = self.{method}({param}, ChildRole::{});\n",
            child_role(name, m)?
        ));
    }
    let args = ms
        .iter()
        .map(|m| Ok(snake(string(m, "name")?)))
        .collect::<Result<Vec<_>, String>>()?
        .join(", ");
    code.push_str(&format!(
        "        self.update_{}(original_id, {args})\n    }}\n",
        snake(name)
    ));
    Ok(())
}

/// The read-only counterpart of emit_transform: preserve nil slots and the
/// SameMap-before-Update argument order without requiring a mutable factory.
fn emit_child_slots(code: &mut String, nodes: &[Value]) -> Result<(), String> {
    code.push_str("impl NodeRead<'_> {\n    /// Snapshot immediate VisitEachChild hook arguments, including absent slots.\n    /// No node borrow escapes; callers can materialize lazy children afterwards.\n    pub fn child_slots(&self) -> Vec<(ChildRole, ChildSlot)> {\n        match self.data() {\n");
    for node in nodes {
        let name = string(node, "name")?;
        let children = members(node)?
            .into_iter()
            .filter(|m| flag(m, "child"))
            .collect::<Vec<_>>();
        if children.is_empty() {
            continue;
        }
        code.push_str(&format!(
            "            NodeDataRead::{name}(data) => vec![\n"
        ));
        for raw in [true, false] {
            for member in children
                .iter()
                .filter(|m| (m["type"]["list"] == "raw") == raw)
            {
                let field = snake(string(member, "name")?);
                let variant = match rust_type(&member["type"])?.as_str() {
                    "NodeSlice" => "Nodes",
                    "NodeListId" => "List",
                    _ => "Node",
                };
                code.push_str(&format!(
                    "                (ChildRole::{}, ChildSlot::{variant}(data.{field}())),\n",
                    child_role(name, member)?
                ));
            }
        }
        code.push_str("            ],\n");
    }
    code.push_str("            _ => Vec::new(),\n        }\n    }\n}\n");
    Ok(())
}

fn child_call(code: &mut String, m: &Value, indent: &str) -> Result<(), String> {
    let field = snake(string(m, "name")?);
    let method = match rust_type(&m["type"])?.as_str() {
        "NodeListId" => "visit_list",
        "NodeSlice" => "visit_node_slice",
        _ => "visit_node",
    };
    if nullable(m)? {
        code.push_str(&format!(
            "{indent}if let Some(child) = self.{field} {{ visitor.{method}(child)?; }}\n"
        ));
    } else {
        code.push_str(&format!("{indent}visitor.{method}(self.{field})?;\n"));
    }
    Ok(())
}

fn emit_children(
    code: &mut String,
    scope: &mut BTreeSet<String>,
    node: &Value,
) -> Result<(), String> {
    let name = string(node, "name")?;
    let children = members(node)?
        .into_iter()
        .filter(|m| flag(m, "child"))
        .collect::<Vec<_>>();
    if children.is_empty() {
        return Ok(());
    }
    code.push_str(&format!("impl {name}Data {{\n"));
    mapped(code, scope, name, "ForEachChild");
    code.push_str(
        "    pub fn for_each_child(&self, visitor: &mut impl ChildVisitor) -> ControlFlow<()> {\n",
    );
    if flag(node, "handWrittenVisitor") {
        if name != "JSDocParameterOrPropertyTag" {
            return Err(format!("AST runtime: unhandled dynamic visitor {name}"));
        }
        let field = |name: &str| {
            children
                .iter()
                .copied()
                .find(|m| m["name"] == name)
                .ok_or("AST runtime: missing dynamic child")
        };
        child_call(code, field("TagName")?, "        ")?;
        code.push_str("        if self.is_name_first {\n");
        child_call(code, field("name")?, "            ")?;
        child_call(code, field("TypeExpression")?, "            ")?;
        code.push_str("        } else {\n");
        child_call(code, field("TypeExpression")?, "            ")?;
        child_call(code, field("name")?, "            ")?;
        code.push_str("        }\n");
        child_call(code, field("Comment")?, "        ")?;
    } else {
        for child in children {
            child_call(code, child, "        ")?;
        }
    }
    code.push_str("        ControlFlow::Continue(())\n    }\n}\n");
    Ok(())
}

fn emit_predicate(code: &mut String, scope: &mut BTreeSet<String>, name: &str, kinds: &[&str]) {
    mapped(code, scope, "", &format!("Is{name}"));
    code.push_str(&format!(
        "pub fn is_{}(node: &(impl NodeAccess + ?Sized)) -> bool {{ matches!(node.kind().known(), Some({})) }}\n",
        snake(name),
        kinds
            .iter()
            .map(|k| format!("SyntaxKind::{k}"))
            .collect::<Vec<_>>()
            .join(" | ")
    ));
}

fn strings(values: &[Value]) -> Result<Vec<&str>, String> {
    values
        .iter()
        .map(|v| {
            v.as_str()
                .ok_or_else(|| "AST runtime: expected string array".into())
        })
        .collect()
}

#[derive(Debug)]
pub(super) struct Emission {
    pub files: BTreeMap<PathBuf, String>,
    pub scope: Value,
}

pub(super) fn emit(schema: &Value, pin: &str) -> Result<Emission, String> {
    if schema["runtimeVersion"] != 1 {
        return Err("AST runtime: missing or unsupported resolver runtimeVersion".into());
    }
    let nodes = array(schema, "nodes")?;
    for node in nodes {
        for key in ["multiKind", "generateSubtreeFacts"] {
            if !node[key].is_boolean() {
                return Err(format!("AST runtime: {key} must be boolean"));
            }
        }
        string(node, "syntaxKindName")?;
        array(node, "kindAliases")?;
        array(node, "kindTypes")?;
        strings(array(node, "baseTypes")?)?;
    }
    let mut scope = BTreeSet::new();
    let imports = "#[allow(clippy::wildcard_imports)] // Generated methods consume the complete schema API.\nuse crate::*;\nuse std::ops::ControlFlow;\n\n";
    let mut factory = header(pin, "ast_generated.go");
    emit_construction_methods(&mut factory, nodes)?;
    factory.push_str("#[allow(clippy::wildcard_imports)] // Generated methods consume the complete schema API.\nuse crate::*;\n\n/// Pinned generated constructors, identity-preserving updates and shallow clones.\n#[allow(clippy::too_many_arguments)] // Positional factory signatures follow the pinned schema.\npub trait FactoryMethods: Factory {\n");
    let mut transform = header(pin, "ast_generated.go");
    transform.push_str("#[allow(clippy::wildcard_imports)] // Generated methods consume the complete schema API.\nuse crate::*;\n\n/// Source-specific visitor hooks. Raw mapping is SameMap, not list flattening.\npub trait VisitContext: Factory {\n    fn visit_node(&mut self, node: Option<NodeId>, role: ChildRole) -> Option<NodeId>;\n    fn visit_list(&mut self, list: Option<NodeListId>, role: ChildRole) -> Option<NodeListId>;\n    fn map_raw_nodes(&mut self, nodes: NodeSlice) -> NodeSlice;\n    fn visit_each_child_source_file(&mut self, node: NodeId) -> NodeId;\n}\n\npub trait VisitorMethods: VisitContext {\n");
    let mut runtime = header(pin, "ast_generated.go");
    runtime.push_str(imports);
    let mut names = String::new();
    let mut facts = header(pin, "ast_generated.go");
    facts.push_str("#[allow(clippy::wildcard_imports)] // Generated methods consume the complete schema API.\nuse crate::*;\n\n/// Propagation delegates to the owner's pinned caches and exclusion rules.\npub trait SubtreeContext {\n    fn propagate_node(&mut self, node: Option<NodeId>) -> u32;\n    fn propagate_list(&mut self, list: Option<NodeListId>) -> u32;\n    fn propagate_modifiers(&mut self, list: Option<NodeListId>) -> u32;\n}\n");
    let mut fact_dispatch=String::from("impl NodeData {\n    pub fn compute_subtree_facts_generated(&self, context: &mut impl SubtreeContext) -> Option<u32> {\n        match self {\n");
    for node in nodes {
        let name = string(node, "name")?;
        if !flag(node, "handWritten") {
            if name != "SyntheticExpression" {
                emit_new(&mut factory, &mut scope, node, None)?;
                for alias in strings(array(node, "kindAliases")?)? {
                    emit_new(&mut factory, &mut scope, node, Some(alias))?;
                }
                emit_update(&mut factory, &mut scope, node)?;
                emit_clone(&mut factory, &mut scope, node)?;
                emit_transform(&mut transform, &mut scope, node)?;
            }
            emit_children(&mut runtime, &mut scope, node)?;
            if super::ast::has_declaration_name(node)? {
                runtime.push_str(&format!("impl {name}Data {{\n"));
                mapped(&mut runtime, &mut scope, name, "Name");
                runtime.push_str(
                    "    pub fn declaration_name(&self) -> Option<NodeId> { self.name }\n}\n",
                );
                names.push_str(&format!(
                    "            Self::{name}(data) => data.declaration_name(),\n"
                ));
            }
            if flag(node, "generateSubtreeFacts") {
                facts.push_str(&format!("impl {name}Data {{\n"));
                mapped(&mut facts, &mut scope, name, "computeSubtreeFacts");
                facts.push_str("    pub fn compute_subtree_facts(&self, context: &mut impl SubtreeContext) -> u32 {\n");
                let terms = members(node)?
                    .into_iter()
                    .filter(|m| flag(m, "child"))
                    .map(|m| {
                        let method = if m["type"]["list"] == "ModifierList" {
                            "propagate_modifiers"
                        } else if rust_type(&m["type"])? == "NodeListId" {
                            "propagate_list"
                        } else {
                            "propagate_node"
                        };
                        Ok(format!(
                            "context.{method}(self.{})",
                            snake(string(m, "name")?)
                        ))
                    })
                    .collect::<Result<Vec<_>, String>>()?;
                if terms.is_empty() {
                    facts.push_str("        let _ = context;\n        0\n");
                } else {
                    facts.push_str(&format!("        {}\n", terms.join(" | ")));
                }
                facts.push_str("    }\n}\n");
                fact_dispatch.push_str(&format!("            Self::{name}(data) => Some(data.compute_subtree_facts(context)),\n"));
            }
        }
        let kinds = strings(array(node, "kindTypes")?)?;
        if node["kindType"]["kind"] == "typeParameter" {
            emit_predicate(&mut runtime, &mut scope, name, &kinds);
        } else if flag(node, "multiKind") {
            for kind in kinds {
                emit_predicate(&mut runtime, &mut scope, kind, &[kind]);
            }
        } else {
            let primary = string(node, "syntaxKindName")?;
            emit_predicate(&mut runtime, &mut scope, name, &[primary]);
            for alias in strings(array(node, "kindAliases")?)? {
                emit_predicate(&mut runtime, &mut scope, alias, &[alias]);
            }
        }
    }
    factory.push_str("    /// Payload dispatch; None identifies handwritten/deferred clone implementations.\n    fn clone_node_generated(&mut self, original_id: NodeId) -> Option<NodeId> {\n        let clone: fn(&mut Self, NodeId) -> NodeId = {\n            let original = self.node(original_id);\n            match original.data() {\n");
    for node in nodes {
        let name = string(node, "name")?;
        if !flag(node, "handWritten") && name != "SyntheticExpression" {
            factory.push_str(&format!(
                "                NodeDataRead::{name}(_) => Self::clone_{},\n",
                snake(name)
            ));
        }
    }
    factory.push_str("                _ => return None,\n            }\n        };\n        Some(clone(self, original_id))\n    }\n}\nimpl<T: Factory + ?Sized> FactoryMethods for T {}\n");
    transform.push_str("    fn visit_each_child_generated(&mut self, original_id: NodeId) -> NodeId {\n        let visit: fn(&mut Self, NodeId) -> NodeId = {\n            let original = self.node(original_id);\n            match original.data() {\n");
    mapped(&mut runtime, &mut scope, "Node", "ForEachChild");
    runtime.push_str("pub fn for_each_child_generated(node: &(impl NodeAccess + ?Sized), visitor: &mut impl ChildVisitor) -> ControlFlow<()> {\n        match node.kind().known() {\n");
    let mut stored_children = String::from("impl AstPayloadStore {\n    pub(crate) fn for_each_stored_child(&self, kind: NodeKind, shape: u16, ordinal: u32, end: i32, context: crate::compact::CompactContext<'_>, visitor: &mut impl ChildVisitor) -> ControlFlow<()> {\n        match kind.known() {\n");
    for (shape, node) in nodes.iter().enumerate() {
        let name = string(node, "name")?;
        let Some(pattern) = child_kind_pattern(node)? else {
            continue;
        };
        runtime.push_str(&format!("            {pattern} => node.data_source().as_{}().expect(\"{name} kind requires {name} payload\").for_each_child(visitor),\n",snake(name)));
        stored_children.push_str(&format!("            {pattern} => {{\n                assert!(shape == {shape}, \"{name} kind requires {name} payload\");\n                self.read_{}(ordinal, context, end).for_each_child(visitor)\n            }},\n", snake(name)));
        if name == "SyntheticExpression" {
            transform.push_str(&format!("                NodeDataRead::{name}(_) => panic!(\"SyntheticExpression transformation requires checker-owned Type\"),\n"));
        } else {
            transform.push_str(&format!(
                "                NodeDataRead::{name}(_) => Self::visit_each_child_{},\n",
                snake(name)
            ));
        }
    }
    runtime.push_str("            _ => ControlFlow::Continue(()),\n        }\n}\nimpl Node {\n    pub fn for_each_child_generated(&self, visitor: &mut impl ChildVisitor) -> ControlFlow<()> { for_each_child_generated(self, visitor) }\n}\nimpl NodeRead<'_> {\n    pub fn for_each_child_generated(&self, visitor: &mut impl ChildVisitor) -> ControlFlow<()> { for_each_child_generated(self, visitor) }\n}\n");
    stored_children.push_str("            _ => ControlFlow::Continue(()),\n        }\n    }\n}\n");
    runtime.push_str(&stored_children);
    transform.push_str("                _ => return original_id,\n            }\n        };\n        visit(self, original_id)\n    }\n}\nimpl<T: VisitContext + ?Sized> VisitorMethods for T {}\n");
    emit_child_slots(&mut transform, nodes)?;
    runtime.push_str(&format!("impl NodeData {{\n    pub fn declaration_name_generated(&self) -> Option<NodeId> {{\n        match self {{\n{names}            _ => None,\n        }}\n    }}\n}}\n"));
    runtime.push_str("impl NodeData {\n    /// Validate every stored identity, including fields omitted by Go child visitors.\n    pub fn validate_references<E>(&self, mut node: impl FnMut(NodeId) -> Result<(), E>, mut list: impl FnMut(NodeListId) -> Result<(), E>, mut raw: impl FnMut(NodeSlice) -> Result<(), E>, mut text: impl FnMut(TextSlice) -> Result<(), E>) -> Result<(), E> {\n        match self {\n");
    let mut no_references = Vec::new();
    for definition in nodes {
        let name = string(definition, "name")?;
        let references = fields(definition)?
            .into_iter()
            .filter_map(|field| match rust_type(&field["type"]) {
                Ok(typ)
                    if matches!(
                        typ.as_str(),
                        "NodeId" | "NodeListId" | "NodeSlice" | "TextSlice"
                    ) =>
                {
                    Some(Ok((field, typ)))
                }
                Ok(_) => None,
                Err(error) => Some(Err(error)),
            })
            .collect::<Result<Vec<_>, String>>()?;
        if references.is_empty() {
            no_references.push(format!("Self::{name}(_)"));
            continue;
        }
        runtime.push_str(&format!("            Self::{name}(data) => {{\n"));
        for (field, typ) in references {
            let callback = match typ.as_str() {
                "NodeId" => "node",
                "NodeListId" => "list",
                "NodeSlice" => "raw",
                "TextSlice" => "text",
                _ => unreachable!(),
            };
            let field_name = snake(string(field, "name")?);
            if nullable(field)? {
                runtime.push_str(&format!(
                    "                if let Some(id) = data.{field_name} {{ {callback}(id)?; }}\n"
                ));
            } else {
                runtime.push_str(&format!(
                    "                {callback}(data.{field_name})?;\n"
                ));
            }
        }
        runtime.push_str("            }\n");
    }
    runtime.push_str(&format!(
        "            {} => {{}},\n        }}\n        Ok(())\n    }}\n}}\n",
        no_references.join(" | ")
    ));
    for guard in array(schema, "kindGuards")? {
        let alias = string(guard, "alias")?;
        let name = alias.replace("Syntax", "");
        if name == "JSDocKind" {
            continue;
        }
        mapped(&mut runtime, &mut scope, "", &format!("Is{name}"));
        runtime.push_str(&format!(
            "pub fn is_{}(kind: NodeKind) -> bool {{\n",
            snake(&name)
        ));
        match string(guard, "form")? {
            "range" => runtime.push_str(&format!(
                "    kind.raw() >= SyntaxKind::{} as i16 && kind.raw() <= SyntaxKind::{} as i16\n",
                string(guard, "first")?,
                string(guard, "last")?
            )),
            "enumerated" => runtime.push_str(&format!(
                "    matches!(kind.known(), Some({}))\n",
                strings(array(guard, "kinds")?)?
                    .iter()
                    .map(|k| format!("SyntaxKind::{k}"))
                    .collect::<Vec<_>>()
                    .join(" | ")
            )),
            other => return Err(format!("AST runtime: unknown kind guard form {other}")),
        }
        runtime.push_str("}\n");
    }
    fact_dispatch.push_str("            _ => None,\n        }\n    }\n}\n");
    facts.push_str(&fact_dispatch);
    facts.push_str("impl NodeData {\n");
    for (method, base) in [
        ("uses_subtree_cache", "CompositeBase"),
        ("is_type_syntax", "TypeSyntaxBase"),
    ] {
        let patterns = nodes
            .iter()
            .filter(|node| {
                node["baseTypes"]
                    .as_array()
                    .is_some_and(|bases| bases.iter().any(|value| value == base))
            })
            .map(|node| Ok(format!("Self::{}(_)", string(node, "name")?)))
            .collect::<Result<Vec<_>, String>>()?;
        if patterns.is_empty() {
            return Err(format!("AST runtime: no node inherits {base}"));
        }
        facts.push_str(&format!(
            "    pub fn {method}(&self) -> bool {{ matches!(self, {}) }}\n",
            patterns.join(" | ")
        ));
    }
    facts.push_str("}\n");
    let deferred = [
        "NodeFactory.NewSyntheticExpression",
        "NodeFactory.UpdateSyntheticExpression",
        "SyntheticExpression.VisitEachChild",
        "SyntheticExpression.Clone",
    ]
    .map(|name| format!("{SOURCE}:{name}"));
    let scope_json = json!({"version":1,"upstreamPin":pin,"authority":"tools/scripts/tsc/schema.ts + generate-go-ast.ts","functions":scope,"deferred":deferred,"excluded":"Assertion casts are not represented by optional schema accessors.","tracking":"Generated-source provenance only. These functions use upstream: comments and contribute zero to source-kind function coverage."});
    Ok(Emission {
        files: [
            ("factory_generated.rs", factory),
            ("transform_generated.rs", transform),
            ("runtime_generated.rs", runtime),
            ("subtree_generated.rs", facts),
        ]
        .into_iter()
        .map(|(name, code)| (PathBuf::from("crates/tsr_ast/src").join(name), code))
        .collect(),
        scope: scope_json,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn schema() -> Value {
        serde_json::from_str(include_str!("../../../data/s03/schema/ast.json")).unwrap()
    }

    #[test]
    fn generated_scope_is_exact_pinned_inventory_without_assertion_cast_credit() {
        let emitted = emit(&schema(), "pin").unwrap();
        let actual: BTreeSet<_> = emitted.scope["functions"]
            .as_array()
            .unwrap()
            .iter()
            .map(|value| value.as_str().unwrap())
            .collect();
        let all: BTreeSet<_> = include_str!("../../../data/go-functions.tsv")
            .lines()
            .skip(2)
            .filter_map(|line| line.split('\t').nth(6))
            .collect();
        let expected: BTreeSet<_> = all
            .iter()
            .copied()
            .filter(|id| id.starts_with(&format!("{SOURCE}:")))
            .filter(|id| !id.starts_with(&format!("{SOURCE}:Node.As")))
            .filter(|id| {
                !matches!(
                    id.rsplit(':').next().unwrap(),
                    "NodeFactory.NewSyntheticExpression"
                        | "NodeFactory.UpdateSyntheticExpression"
                        | "SyntheticExpression.VisitEachChild"
                        | "SyntheticExpression.Clone"
                )
            })
            .collect();
        assert_eq!(actual, expected);
        assert_eq!(actual.len(), 1190);
        assert!(emitted
            .files
            .values()
            .all(|code| !code.contains("// port:")));
        assert!(emitted.scope["tracking"]
            .as_str()
            .unwrap()
            .contains("contribute zero"));
    }

    #[test]
    fn runtime_schema_omissions_and_unknown_masks_fail_before_emission() {
        let original = schema();
        let mut changed = original.clone();
        changed.as_object_mut().unwrap().remove("runtimeVersion");
        assert!(emit(&changed, "pin")
            .unwrap_err()
            .contains("runtimeVersion"));
        let mut changed = original.clone();
        changed["nodes"][0]
            .as_object_mut()
            .unwrap()
            .remove("generateSubtreeFacts");
        assert!(emit(&changed, "pin")
            .unwrap_err()
            .contains("generateSubtreeFacts"));
        let mut changed = original.clone();
        changed["nodes"][0]["baseTypes"] = json!(null);
        assert!(emit(&changed, "pin").unwrap_err().contains("baseTypes"));
        let mut changed = original;
        let identifier = changed["nodes"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|node| node["name"] == "Identifier")
            .unwrap();
        identifier["members"][0]["bitmask"] = json!("FutureUnreviewedMask");
        assert!(emit(&changed, "pin")
            .unwrap_err()
            .contains("unsupported factory bitmask"));
    }
}
