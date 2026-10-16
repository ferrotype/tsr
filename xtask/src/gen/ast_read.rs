//! Contextual payload borrows keep semantic field access independent of storage.

use serde_json::Value;
use std::collections::BTreeSet;

use super::ast::{array, children, fields, flag, header, nullable, rust_type, snake, string};

pub(super) fn owned_method(field: &str) -> String {
    format!("{}_owned", field.strip_prefix("r#").unwrap_or(field))
}

pub(super) fn emit(nodes: &[Value], pin: &str) -> Result<String, String> {
    let mut code = header(pin, "ast_generated.go");
    code.push_str("#[allow(clippy::wildcard_imports)] // Generated views consume the complete schema API.\nuse crate::*;\nuse crate::compact::{CompactContext, FieldKey, StoredNode};\n#[allow(clippy::wildcard_imports)] // Every concrete row in this schema is referenced below.\nuse crate::compact_generated::*;\nuse std::ops::ControlFlow;\n\n");
    emit_source(&mut code, nodes)?;
    let mut accessors = String::from("impl<'owner> NodeRead<'owner> {\n");
    for (shape, node) in nodes.iter().enumerate() {
        let name = string(node, "name")?;
        let view = format!("{name}PayloadRead");
        let read = format!("{name}DataRead");
        let accessor = format!("as_{}", snake(name));
        let fields = fields(node)?;
        let mut names = [
            "node",
            "from_owned",
            "from_stored",
            "to_owned",
            "for_each_child",
        ]
        .map(str::to_owned)
        .into_iter()
        .collect::<BTreeSet<_>>();
        for field in &fields {
            let field_name = snake(string(field, "name")?);
            if !names.insert(field_name.clone()) {
                return Err(format!(
                    "AST read: duplicate or reserved method {name}.{field_name}"
                ));
            }
            if rust_type(&field["type"])? == "JsString" {
                let owned = owned_method(&field_name);
                if !names.insert(owned.clone()) {
                    return Err(format!("AST read: duplicate method {name}.{owned}"));
                }
            }
        }
        emit_read_storage(&mut code, name, &fields)?;
        for (index, field) in fields.iter().enumerate() {
            let field_name = snake(string(field, "name")?);
            let typ = rust_type(&field["type"])?;
            let key = super::ast_compact::field_key(shape, index);
            let (output, decoded, owned) = match typ.as_str() {
                "NodeId" => (
                    "Option<NodeId>".into(),
                    format!("context.decode_node({key}, row.{field_name})"),
                    format!("data.{field_name}"),
                ),
                "NodeListId" => (
                    "Option<NodeListId>".into(),
                    format!("context.decode_list({key}, row.{field_name})"),
                    format!("data.{field_name}"),
                ),
                "NodeSlice" => (
                    typ,
                    format!("context.decode_node_slice({key}, row.{field_name})"),
                    format!("data.{field_name}"),
                ),
                "TextSlice" => (
                    typ,
                    format!("context.decode_text_slice({key}, row.{field_name})"),
                    format!("data.{field_name}"),
                ),
                "JsString" => (
                    "&'a [u8]".into(),
                    format!("context.text({key}, row.{field_name}, end)"),
                    format!("data.{field_name}.as_bytes()"),
                ),
                _ => (
                    typ,
                    format!("row.{field_name}"),
                    format!("data.{field_name}"),
                ),
            };
            let uses_context = matches!(
                rust_type(&field["type"])?.as_str(),
                "NodeId" | "NodeListId" | "NodeSlice" | "TextSlice" | "JsString"
            );
            let context_fields = if uses_context {
                if rust_type(&field["type"])? == "JsString" {
                    "context, ordinal, end"
                } else {
                    "context, ordinal, .."
                }
            } else {
                ".."
            };
            code.push_str(&format!("    pub fn {field_name}(&self) -> {output} {{\n        match self.storage {{\n            {name}ReadStorage::Owned(data) => {owned},\n            {name}ReadStorage::Stored {{ row, {context_fields} }} => {decoded},\n        }}\n    }}\n"));
            if rust_type(&field["type"])? == "JsString" {
                let owned_method = owned_method(&field_name);
                code.push_str(&format!("    pub fn {owned_method}(&self) -> JsString {{\n        match self.storage {{\n            {name}ReadStorage::Owned(data) => data.{field_name}.clone(),\n            {name}ReadStorage::Stored {{ row, context, ordinal, end }} => context.text_owned({key}, row.{field_name}, end),\n        }}\n    }}\n"));
            }
        }
        code.push_str(&format!(
            "    pub fn to_owned(&self) -> {name}Data {{\n        {name}Data {{\n"
        ));
        for field in &fields {
            let member = snake(string(field, "name")?);
            let getter = if rust_type(&field["type"])? == "JsString" {
                owned_method(&member)
            } else {
                member.clone()
            };
            code.push_str(&format!("            {member}: self.{getter}(),\n"));
        }
        code.push_str("        }\n    }\n");
        emit_children(&mut code, node, "ChildVisitor")?;
        code.push_str("}\n\n");
        code.push_str(&format!(
            "/// Borrowed {name} payload selected by its concrete storage shape.\n/// The view cannot outlive its originating node read.\npub struct {view}<'read, 'owner> {{\n    node: &'read NodeRead<'owner>,\n"
        ));
        if !fields.is_empty() {
            code.push_str(&format!("    data: {read}<'read>,\n"));
        }
        code.push_str(&format!(
            "}}\nimpl<'read, 'owner> {view}<'read, 'owner> {{\n    /// Identity and physical owner/source context for this payload.\n    pub fn node(&self) -> &'read NodeRead<'owner> {{ self.node }}\n"
        ));
        for field in &fields {
            let field_name = snake(string(field, "name")?);
            let typ = rust_type(&field["type"])?;
            if typ == "JsString" {
                let owned = owned_method(&field_name);
                code.push_str(&format!(
                    "    /// Borrow the selected bytes without retaining or copying their backing.\n    pub fn {field_name}(&self) -> &'read [u8] {{ self.data.{field_name}() }}\n    /// Explicitly retain independently usable text.\n    pub fn {owned}(&self) -> JsString {{ self.data.{owned}() }}\n"
                ));
            } else {
                let typ = if nullable(field)? {
                    format!("Option<{typ}>")
                } else {
                    typ
                };
                code.push_str(&format!(
                    "    pub fn {field_name}(&self) -> {typ} {{ self.data.{field_name}() }}\n"
                ));
            }
        }
        code.push_str("}\n\n");
        accessors.push_str(&format!(
            "    #[inline]\n    pub fn {accessor}<'read>(&'read self) -> Option<{view}<'read, 'owner>> {{\n"
        ));
        if fields.is_empty() {
            accessors.push_str(&format!(
                "        self.data_source().{accessor}()?;\n        Some({view} {{ node: self }})\n"
            ));
        } else {
            accessors.push_str(&format!(
            "        let data = self.data_source().{accessor}()?;\n        Some({view} {{ node: self, data }})\n"
        ));
        }
        accessors.push_str("    }\n");
    }
    accessors.push_str("}\n");
    code.push_str(&accessors);
    emit_data(&mut code, nodes)?;
    Ok(code)
}

fn emit_source(code: &mut String, nodes: &[Value]) -> Result<(), String> {
    code.push_str("/// Borrowed payload selection for callers that already know the concrete shape.\n/// Selection borrows the checked node read, including its lazy publication guard.\n#[derive(Clone, Copy)]\npub struct NodeDataSource<'a> { storage: NodeDataSourceStorage<'a> }\n#[derive(Clone, Copy)]\nenum NodeDataSourceStorage<'a> {\n    Owned(&'a NodeData),\n    Stored { header: &'a StoredNode, node: &'a NodeRead<'a> },\n}\nimpl<'a> NodeDataSource<'a> {\n    #[inline]\n    pub(crate) fn from_owned(data: &'a NodeData) -> Self { Self { storage: NodeDataSourceStorage::Owned(data) } }\n    #[inline]\n    pub(crate) fn from_stored(header: &'a StoredNode, node: &'a NodeRead<'a>) -> Self { Self { storage: NodeDataSourceStorage::Stored { header, node } } }\n");
    for (shape, node) in nodes.iter().enumerate() {
        let name = string(node, "name")?;
        let member = snake(name);
        code.push_str(&format!("    #[inline]\n    pub fn as_{member}(self) -> Option<{name}DataRead<'a>> {{\n        match self.storage {{\n            NodeDataSourceStorage::Owned(data) => data.as_{member}().map({name}DataRead::from_owned),\n            NodeDataSourceStorage::Stored {{ header, node }} => {{\n                if header.actual_shape() != {shape} {{ return None; }}\n                let context = node.compact_context();\n                Some(context.store.payloads.read_{member}(header.ordinal, context, header.end))\n            }}\n        }}\n    }}\n"));
    }
    code.push_str("    #[cfg(feature = \"contextual-payload-experiment\")]\n    pub(crate) fn declaration_name_typed(self, shape: u16) -> Option<NodeId> {\n        match shape {\n");
    for (shape, node) in nodes.iter().enumerate() {
        if super::ast::has_declaration_name(node)? {
            let name = string(node, "name")?;
            let member = snake(name);
            code.push_str(&format!("            {shape} => self.as_{member}().expect(\"stored shape selects its typed payload\").name(),\n"));
        }
    }
    code.push_str("            _ => None,\n        }\n    }\n");
    code.push_str("    pub fn name(self) -> &'static str {\n        match self.storage {\n            NodeDataSourceStorage::Owned(data) => data.name(),\n            NodeDataSourceStorage::Stored { header, .. } => match header.actual_shape() {\n");
    for (shape, node) in nodes.iter().enumerate() {
        let name = string(node, "name")?;
        code.push_str(&format!("                {shape} => \"{name}\",\n"));
    }
    code.push_str("                _ => panic!(\"compact payload shape\"),\n            },\n        }\n    }\n");
    code.push_str("}\n\n");
    Ok(())
}

fn emit_read_storage(code: &mut String, name: &str, fields: &[&Value]) -> Result<(), String> {
    let read = format!("{name}DataRead");
    let types = fields
        .iter()
        .map(|field| rust_type(&field["type"]))
        .collect::<Result<Vec<_>, _>>()?;
    let has_text = types.iter().any(|typ| typ == "JsString");
    let has_context = types.iter().any(|typ| {
        matches!(
            typ.as_str(),
            "NodeId" | "NodeListId" | "NodeSlice" | "TextSlice" | "JsString"
        )
    });
    if fields.is_empty() {
        code.push_str(&format!("/// Semantic empty payload borrow, tied to the originating record lifetime.\n#[derive(Clone, Copy, Debug)]\npub struct {read}<'a> {{ marker: std::marker::PhantomData<&'a ()> }}\nimpl<'a> {read}<'a> {{\n    pub(crate) fn from_owned(_: &'a {name}Data) -> Self {{ Self {{ marker: std::marker::PhantomData }} }}\n    pub(crate) fn from_stored(_: &'a {name}Row, _: CompactContext<'a>, _: u32, _: i32) -> Self {{ Self {{ marker: std::marker::PhantomData }} }}\n"));
        return Ok(());
    }
    let context_fields = if has_context {
        "context: CompactContext<'a>, ordinal: u32,"
    } else {
        ""
    };
    let end_field = if has_text { "end: i32," } else { "" };
    let context_args = if has_context {
        "context: CompactContext<'a>, ordinal: u32"
    } else {
        "_: CompactContext<'a>, _: u32"
    };
    let end_arg = if has_text { "end: i32" } else { "_: i32" };
    let context_init = if has_context { "context, ordinal," } else { "" };
    let end_init = if has_text { "end," } else { "" };
    code.push_str(&format!("#[derive(Clone, Copy)]\nenum {name}ReadStorage<'a> {{\n    Owned(&'a {name}Data),\n    Stored {{ row: &'a {name}Row, {context_fields} {end_field} }},\n}}\n\n/// Semantic payload borrow from construction input or one physical owner's rows.\n#[derive(Clone, Copy)]\npub struct {read}<'a> {{ storage: {name}ReadStorage<'a> }}\n\nimpl std::fmt::Debug for {read}<'_> {{\n    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {{ out.debug_struct(\"{read}\").finish_non_exhaustive() }}\n}}\n\nimpl<'a> {read}<'a> {{\n    pub(crate) fn from_owned(data: &'a {name}Data) -> Self {{ Self {{ storage: {name}ReadStorage::Owned(data) }} }}\n    pub(crate) fn from_stored(row: &'a {name}Row, {context_args}, {end_arg}) -> Self {{ Self {{ storage: {name}ReadStorage::Stored {{ row, {context_init} {end_init} }} }} }}\n"));
    Ok(())
}

fn emit_child_call(code: &mut String, field: &Value, indent: &str) -> Result<(), String> {
    let name = snake(string(field, "name")?);
    let method = match rust_type(&field["type"])?.as_str() {
        "NodeListId" => "visit_list",
        "NodeSlice" => "visit_node_slice",
        _ => "visit_node",
    };
    if nullable(field)? {
        code.push_str(&format!(
            "{indent}if let Some(child) = self.{name}() {{ visitor.{method}(child)?; }}\n"
        ));
    } else {
        code.push_str(&format!("{indent}visitor.{method}(self.{name}())?;\n"));
    }
    Ok(())
}

pub(super) fn emit_children(code: &mut String, node: &Value, visitor: &str) -> Result<(), String> {
    let children = children(node)?;
    code.push_str(&format!(
        "    pub fn for_each_child(&self, visitor: &mut impl {visitor}) -> ControlFlow<()> {{\n",
    ));
    if children.is_empty() {
        code.push_str("        let _ = visitor;\n");
    } else if flag(node, "handWrittenVisitor") {
        if node["name"] != "JSDocParameterOrPropertyTag" {
            return Err("AST read: unknown dynamic visitor".into());
        }
        let field = |name: &str| {
            children
                .iter()
                .copied()
                .find(|field| field["name"] == name)
                .ok_or("AST read: missing dynamic child")
        };
        emit_child_call(code, field("TagName")?, "        ")?;
        code.push_str("        if self.is_name_first() {\n");
        emit_child_call(code, field("name")?, "            ")?;
        emit_child_call(code, field("TypeExpression")?, "            ")?;
        code.push_str("        } else {\n");
        emit_child_call(code, field("TypeExpression")?, "            ")?;
        emit_child_call(code, field("name")?, "            ")?;
        code.push_str("        }\n");
        emit_child_call(code, field("Comment")?, "        ")?;
    } else {
        for field in children {
            emit_child_call(code, field, "        ")?;
        }
    }
    code.push_str("        ControlFlow::Continue(())\n    }\n");
    Ok(())
}

fn emit_data(code: &mut String, nodes: &[Value]) -> Result<(), String> {
    code.push_str("/// Semantic payload dispatch over a borrowed construction input or typed core row.\n#[derive(Clone, Copy, Debug)]\npub enum NodeDataRead<'a> {\n");
    for node in nodes {
        let name = string(node, "name")?;
        code.push_str(&format!("    {name}({name}DataRead<'a>),\n"));
    }
    code.push_str("}\n\nimpl<'a> NodeDataRead<'a> {\n    pub fn from_owned(data: &'a NodeData) -> Self {\n        match data {\n");
    for node in nodes {
        let name = string(node, "name")?;
        code.push_str(&format!("            NodeData::{name}(data) => Self::{name}({name}DataRead::from_owned(data)),\n"));
    }
    code.push_str("        }\n    }\n");
    for node in nodes {
        let name = string(node, "name")?;
        let method = snake(name);
        code.push_str(&format!("    pub fn as_{method}(self) -> Option<{name}DataRead<'a>> {{ if let Self::{name}(data) = self {{ Some(data) }} else {{ None }} }}\n"));
    }
    code.push_str("    pub fn name(&self) -> &'static str {\n        match self {\n");
    for node in nodes {
        let name = string(node, "name")?;
        code.push_str(&format!("            Self::{name}(_) => \"{name}\",\n"));
    }
    code.push_str("        }\n    }\n    pub fn supports_kind(&self, kind: SyntaxKind) -> bool {\n        match self {\n");
    for node in nodes {
        let name = string(node, "name")?;
        let kinds = array(node, "kinds")?
            .iter()
            .map(|kind| {
                kind.as_str()
                    .map(|kind| format!("SyntaxKind::{kind}"))
                    .ok_or("AST read: malformed kind")
            })
            .collect::<Result<Vec<_>, _>>()?;
        code.push_str(&format!(
            "            Self::{name}(_) => matches!(kind, {}),\n",
            kinds.join(" | ")
        ));
    }
    code.push_str(
        "        }\n    }\n    pub fn to_owned(&self) -> NodeData {\n        match self {\n",
    );
    for node in nodes {
        let name = string(node, "name")?;
        code.push_str(&format!(
            "            Self::{name}(data) => data.to_owned().into(),\n"
        ));
    }
    code.push_str("        }\n    }\n    #[must_use]\n    pub fn map_children(&self, mapper: &mut impl ChildMapper) -> NodeData { self.to_owned().map_children(mapper) }\n    pub fn for_each_child(&self, visitor: &mut impl ChildVisitor) -> ControlFlow<()> {\n        match self {\n");
    for node in nodes {
        let name = string(node, "name")?;
        code.push_str(&format!(
            "            Self::{name}(data) => data.for_each_child(visitor),\n"
        ));
    }
    code.push_str("        }\n    }\n");
    emit_runtime_data(code, nodes)?;
    code.push_str("}\n");
    Ok(())
}

fn emit_runtime_data(code: &mut String, nodes: &[Value]) -> Result<(), String> {
    code.push_str("    /// Check every stored reference, including edges omitted by child visitors.\n    pub fn validate_references<E>(&self, mut node: impl FnMut(NodeId) -> Result<(), E>, mut list: impl FnMut(NodeListId) -> Result<(), E>, mut raw: impl FnMut(NodeSlice) -> Result<(), E>, mut text: impl FnMut(TextSlice) -> Result<(), E>) -> Result<(), E> {\n        match self {\n");
    let mut no_references = Vec::new();
    for definition in nodes {
        let name = string(definition, "name")?;
        let references = fields(definition)?
            .into_iter()
            .map(|field| Ok((field, rust_type(&field["type"])?)))
            .collect::<Result<Vec<_>, String>>()?
            .into_iter()
            .filter(|(_, typ)| {
                matches!(
                    typ.as_str(),
                    "NodeId" | "NodeListId" | "NodeSlice" | "TextSlice"
                )
            })
            .collect::<Vec<_>>();
        if references.is_empty() {
            no_references.push(format!("Self::{name}(_)"));
            continue;
        }
        code.push_str(&format!("            Self::{name}(data) => {{\n"));
        for (field, typ) in references {
            let member = snake(string(field, "name")?);
            let callback = match typ.as_str() {
                "NodeId" => "node",
                "NodeListId" => "list",
                "NodeSlice" => "raw",
                "TextSlice" => "text",
                _ => unreachable!(),
            };
            if nullable(field)? {
                code.push_str(&format!(
                    "                if let Some(id) = data.{member}() {{ {callback}(id)?; }}\n"
                ));
            } else {
                code.push_str(&format!("                {callback}(data.{member}())?;\n"));
            }
        }
        code.push_str("            }\n");
    }
    if !no_references.is_empty() {
        code.push_str(&format!(
            "            {} => {{}},\n",
            no_references.join(" | ")
        ));
    }
    code.push_str("        }\n        Ok(())\n    }\n    pub fn declaration_name_generated(&self) -> Option<NodeId> {\n        match self {\n");
    for node in nodes {
        if super::ast::has_declaration_name(node)? {
            let name = string(node, "name")?;
            code.push_str(&format!("            Self::{name}(data) => data.name(),\n"));
        }
    }
    code.push_str("            _ => None,\n        }\n    }\n    pub fn compute_subtree_facts_generated(&self, context: &mut impl SubtreeContext) -> Option<u32> {\n        match self {\n");
    for node in nodes {
        if flag(node, "handWritten") || !flag(node, "generateSubtreeFacts") {
            continue;
        }
        let name = string(node, "name")?;
        let terms = array(node, "members")?
            .iter()
            .filter(|member| flag(member, "child") && !flag(member, "noFactory"))
            .map(|member| {
                let method = if member["type"]["list"] == "ModifierList" {
                    "propagate_modifiers"
                } else if rust_type(&member["type"])? == "NodeListId" {
                    "propagate_list"
                } else {
                    "propagate_node"
                };
                Ok(format!(
                    "context.{method}(data.{}())",
                    snake(string(member, "name")?)
                ))
            })
            .collect::<Result<Vec<_>, String>>()?;
        if terms.is_empty() {
            code.push_str(&format!("            Self::{name}(_) => Some(0),\n"));
        } else {
            code.push_str(&format!(
                "            Self::{name}(data) => Some({}),\n",
                terms.join(" | ")
            ));
        }
    }
    code.push_str("            _ => None,\n        }\n    }\n");
    for (method, base) in [
        ("uses_subtree_cache", "CompositeBase"),
        ("is_type_syntax", "TypeSyntaxBase"),
    ] {
        let variants = nodes
            .iter()
            .filter(|node| {
                node["baseTypes"]
                    .as_array()
                    .is_some_and(|bases| bases.iter().any(|value| value == base))
            })
            .map(|node| Ok(format!("Self::{}(_)", string(node, "name")?)))
            .collect::<Result<Vec<_>, String>>()?;
        code.push_str(&format!(
            "    pub fn {method}(&self) -> bool {{ matches!(self, {}) }}\n",
            variants.join(" | ")
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn field(name: &str, typ: &Value) -> Value {
        json!({"name": name, "type": typ, "goOnly": false})
    }

    #[test]
    fn field_names_cannot_shadow_context_or_owned_text_methods() {
        let text = json!({"kind": "primitive", "name": "string"});
        for members in [
            vec![field("Node", &text)],
            vec![field("ToOwned", &text)],
            vec![field("ForEachChild", &text)],
            vec![field("Text", &text), field("TextOwned", &text)],
            vec![field("TextOwned", &text), field("Text", &text)],
            vec![field("Type", &text), field("TypeOwned", &text)],
        ] {
            assert!(emit(&[json!({"name": "Fixture", "fields": members})], "pin").is_err());
        }
    }
}
