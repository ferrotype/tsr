//! Typed core payload rows keep owner-relative storage out of factory inputs.

use serde_json::Value;
use std::collections::BTreeMap;

use super::ast::{array, boxed, fields, header, nullable, rust_type, snake, string};

fn has_facts(node: &Value) -> Result<bool, String> {
    Ok(array(node, "baseTypes")?
        .iter()
        .any(|base| base == "CompositeBase"))
}

fn has_row(node: &Value) -> Result<bool, String> {
    Ok(!fields(node)?.is_empty() || has_facts(node)? || !binding_fields(node)?.is_empty())
}

#[derive(Clone, Copy)]
struct BindingField {
    source: &'static str,
    name: &'static str,
    codec: &'static str,
    tag: u16,
}

const BINDING_FIELDS: [BindingField; 8] = [
    BindingField {
        source: "Symbol",
        name: "symbol",
        codec: "symbol",
        tag: 0x8000,
    },
    BindingField {
        source: "LocalSymbol",
        name: "local_symbol",
        codec: "symbol",
        tag: 0x8001,
    },
    BindingField {
        source: "Locals",
        name: "locals",
        codec: "symbol_table",
        tag: 0x8002,
    },
    BindingField {
        source: "NextContainer",
        name: "next_container",
        codec: "node",
        tag: 0x8003,
    },
    BindingField {
        source: "FlowNode",
        name: "flow_node",
        codec: "flow",
        tag: 0x8004,
    },
    BindingField {
        source: "ReturnFlowNode",
        name: "return_flow_node",
        codec: "flow",
        tag: 0x8005,
    },
    BindingField {
        source: "EndFlowNode",
        name: "end_flow_node",
        codec: "flow",
        tag: 0x8006,
    },
    BindingField {
        source: "FallthroughFlowNode",
        name: "fallthrough_flow_node",
        codec: "flow",
        tag: 0x8007,
    },
];

fn binding_fields(node: &Value) -> Result<Vec<BindingField>, String> {
    let fields = array(node, "fields")?;
    Ok(BINDING_FIELDS
        .into_iter()
        .filter(|definition| {
            fields
                .iter()
                .any(|field| field["name"] == definition.source && field["goOnly"] == true)
        })
        .collect())
}

pub(super) fn has_flow_node(node: &Value) -> Result<bool, String> {
    Ok(binding_fields(node)?
        .iter()
        .any(|field| field.name == "flow_node"))
}

pub(super) fn row_type(field: &Value) -> Result<String, String> {
    Ok(match rust_type(&field["type"])?.as_str() {
        "NodeId" | "NodeListId" | "JsString" => "u32".into(),
        "NodeSlice" | "TextSlice" => "CompactSlice".into(),
        other => other.into(),
    })
}

pub(super) fn field_key(shape: usize, field: usize) -> String {
    format!("FieldKey::new({shape}, ordinal, {field})")
}

fn pack_field(shape: usize, ordinal: usize, field: &Value) -> Result<String, String> {
    let name = snake(string(field, "name")?);
    let key = field_key(shape, ordinal);
    Ok(match rust_type(&field["type"])?.as_str() {
        "NodeId" => format!("context.encode_node({key}, data.{name})"),
        "NodeListId" => format!("context.encode_list({key}, data.{name})"),
        "NodeSlice" => format!("context.encode_node_slice({key}, data.{name})"),
        "TextSlice" => format!("context.encode_text_slice({key}, data.{name})"),
        "JsString" => format!("context.encode_text({key}, data.{name})"),
        _ => format!("data.{name}"),
    })
}

pub(super) fn emit(nodes: &[Value], pin: &str) -> Result<String, String> {
    if nodes.len() > 0x8000 {
        return Err(
            "AST compact: shape count exceeds header tag below binding-presence bit".into(),
        );
    }
    let mut code = header(pin, "ast_generated.go");
    code.push_str("#[allow(clippy::wildcard_imports)] // Generated storage consumes every schema payload.\nuse crate::*;\nuse crate::compact::{CompactContext, CompactSlice, FieldKey, PackingContext, RowPages, StoredNode};\nuse std::sync::atomic::{AtomicU32, Ordering};\n\n");
    for node in nodes {
        let name = string(node, "name")?;
        if fields(node)?.len() >= 0x8000 {
            return Err(format!("AST compact: {name} fields overlap binding tags"));
        }
        code.push_str(&format!(
            "#[derive(Debug, Default)]\npub(crate) struct {name}Row {{\n"
        ));
        for field in fields(node)? {
            code.push_str(&format!(
                "    pub(crate) {}: {},\n",
                snake(string(field, "name")?),
                row_type(field)?
            ));
        }
        if has_facts(node)? {
            code.push_str("    facts: AtomicU32,\n");
        }
        for field in binding_fields(node)? {
            code.push_str(&format!("    {}: u32,\n", field.name));
        }
        code.push_str("}\n\n");
    }
    code.push_str("/// Only populated concrete shapes allocate a typed page directory.\n#[derive(Default)]\npub struct AstPayloadStore {\n");
    for node in nodes {
        if has_row(node)? {
            code.push_str(&format!(
                "    {}: Option<Box<RowPages<{}Row>>>,\n",
                snake(string(node, "name")?),
                string(node, "name")?
            ));
        }
    }
    code.push_str("}\n\nimpl AstPayloadStore {\n    /// Reserved page bytes of every populated typed directory, including the\n    /// boxed directory records (storage census).\n    pub(crate) fn structural_bytes(&self) -> usize {\n        let mut bytes = 0;\n");
    for node in nodes {
        if has_row(node)? {
            code.push_str(&format!(
                "        if let Some(pages) = &self.{} {{\n            bytes += size_of::<RowPages<{}Row>>() + pages.structural_bytes();\n        }}\n",
                snake(string(node, "name")?),
                string(node, "name")?
            ));
        }
    }
    code.push_str("        bytes\n    }\n\n    pub(crate) fn shape_of(data: &NodeData) -> u16 {\n        match data {\n");
    for (shape, node) in nodes.iter().enumerate() {
        code.push_str(&format!(
            "            NodeData::{}(_) => {shape},\n",
            string(node, "name")?
        ));
    }
    code.push_str("        }\n    }\n    pub(crate) fn insert(&mut self, data: NodeData, context: &mut PackingContext<'_>) -> (u16, u32) {\n        match data {\n");
    for node in nodes {
        let name = string(node, "name")?;
        let member = snake(name);
        let data = if boxed(node)? { "*data" } else { "data" };
        code.push_str(&format!(
            "            NodeData::{name}(data) => self.insert_{member}({data}, context),\n"
        ));
    }
    code.push_str("        }\n    }\n");
    for (shape, node) in nodes.iter().enumerate() {
        let name = string(node, "name")?;
        let member = snake(name);
        let fields = fields(node)?;
        let data = if fields.is_empty() { "_data" } else { "data" };
        let mut uses_context = false;
        for field in &fields {
            uses_context |= matches!(
                rust_type(&field["type"])?.as_str(),
                "NodeId" | "NodeListId" | "NodeSlice" | "TextSlice" | "JsString"
            );
        }
        let context = if uses_context { "context" } else { "_context" };
        if !has_row(node)? {
            code.push_str("    #[allow(clippy::unused_self)] // Empty shapes share the uniform concrete insertion boundary.\n");
        }
        code.push_str("    #[allow(clippy::needless_pass_by_value)] // The uniform insertion boundary consumes owned factory inputs, including shapes that transfer strings.\n");
        code.push_str(&format!("    pub(crate) fn insert_{member}(&mut self, {data}: {name}Data, {context}: &mut PackingContext<'_>) -> (u16, u32) {{\n"));
        if !has_row(node)? {
            code.push_str(&format!("        ({shape}, 0)\n    }}\n"));
            continue;
        }
        code.push_str(&format!("        let rows = self.{member}.get_or_insert_with(Default::default);\n        let ordinal = rows.len();\n        let row = {name}Row {{\n"));
        for (index, field) in fields.iter().enumerate() {
            code.push_str(&format!(
                "            {}: {},\n",
                snake(string(field, "name")?),
                pack_field(shape, index, field)?
            ));
        }
        if has_facts(node)? {
            code.push_str("            facts: AtomicU32::new(0),\n");
        }
        for field in binding_fields(node)? {
            code.push_str(&format!("            {}: 0,\n", field.name));
        }
        code.push_str(&format!("        }};\n        assert_eq!(rows.push(row), ordinal, \"compact row identity\");\n        ({shape}, ordinal)\n    }}\n"));
    }
    for node in nodes {
        let name = string(node, "name")?;
        let member = snake(name);
        let stored = has_row(node)?;
        let row = if stored {
            format!("self.{member}.as_ref().expect(\"compact shape directory\").get(ordinal).expect(\"compact payload ordinal\")")
        } else {
            format!("&{name}Row {{}}")
        };
        if !stored {
            code.push_str("\n    #[allow(clippy::unused_self)] // Payloadless shapes retain the uniform borrowed selector signature.\n");
        }
        code.push_str(&format!("\n    #[inline]\n    pub(crate) fn read_{member}<'a>(&'a self, ordinal: u32, context: CompactContext<'a>, end: i32) -> {name}DataRead<'a> {{\n        {name}DataRead::from_stored({row}, context, ordinal, end)\n    }}\n"));
        if !stored {
            code.push_str("\n    #[allow(clippy::unused_self)] // Payloadless shapes have no page or ordinal to resolve.\n");
        }
        let ordinal = if stored { "ordinal" } else { "_ordinal" };
        code.push_str(&format!("\n    #[inline]\n    pub(crate) fn local_{member}_row(&self, {ordinal}: u32) -> &{name}Row {{\n        {row}\n    }}\n"));
    }
    code.push_str("\n    pub(crate) fn read<'a>(&'a self, shape: u16, ordinal: u32, context: CompactContext<'a>, end: i32) -> NodeDataRead<'a> {\n        match shape {\n");
    for (shape, node) in nodes.iter().enumerate() {
        let name = string(node, "name")?;
        let member = snake(name);
        code.push_str(&format!("            {shape} => NodeDataRead::{name}(self.read_{member}(ordinal, context, end)),\n"));
    }
    code.push_str("            _ => panic!(\"compact payload shape\"),\n        }\n    }\n\n    /// Replace an existing payload in its original row; changes of shape allocate\n    /// a new row through `insert` and update the owning header separately.\n    pub(crate) fn replace(&mut self, shape: u16, ordinal: u32, data: NodeData, context: &mut PackingContext<'_>) {\n        match (shape, data) {\n");
    let mut empty_replacements = Vec::new();
    for (shape, node) in nodes.iter().enumerate() {
        let name = string(node, "name")?;
        let fields = fields(node)?;
        if fields.is_empty() {
            empty_replacements.push(format!("({shape}, NodeData::{name}(_))"));
            continue;
        }
        code.push_str(&format!(
            "            ({shape}, NodeData::{name}(data)) => {{\n"
        ));
        if fields
            .iter()
            .any(|field| rust_type(&field["type"]).as_deref() == Ok("JsString"))
        {
            code.push_str("                self.release_text(shape, ordinal, context);\n");
        }
        code.push_str(&format!("                let row = self.{}.as_mut().expect(\"compact shape directory\").get_mut(ordinal).expect(\"compact payload ordinal\");\n",snake(name)));
        for (index, field) in fields.iter().enumerate() {
            code.push_str(&format!(
                "                row.{} = {};\n",
                snake(string(field, "name")?),
                pack_field(shape, index, field)?
            ));
        }
        code.push_str("            }\n");
    }
    if !empty_replacements.is_empty() {
        code.push_str(&format!(
            "            {} => {{}},\n",
            empty_replacements.join(" | ")
        ));
    }
    code.push_str(
        "            _ => panic!(\"compact payload replacement shape\"),\n        }\n    }\n",
    );
    emit_reference_validation(&mut code, nodes)?;
    emit_text_operations(&mut code, nodes)?;
    emit_facts_operations(&mut code, nodes)?;
    emit_binding_operations(&mut code, nodes)?;
    code.push_str("\n    pub(crate) fn is_identifier_text(shape: u16, field: u16) -> bool {\n");
    let mut identifier_fields = BTreeMap::<usize, Vec<usize>>::new();
    for (shape, node) in nodes.iter().enumerate() {
        if identifier_text(node) {
            for (index, field) in fields(node)?.iter().enumerate() {
                if field["name"] == "Text" {
                    identifier_fields.entry(index).or_default().push(shape);
                }
            }
        }
    }
    if identifier_fields.is_empty() {
        code.push_str("        let _ = (shape, field);\n        false\n");
    } else {
        let patterns = identifier_fields
            .into_iter()
            .map(|(field, shapes)| {
                format!(
                    "({}, {field})",
                    shapes
                        .iter()
                        .map(usize::to_string)
                        .collect::<Vec<_>>()
                        .join(" | ")
                )
            })
            .collect::<Vec<_>>();
        code.push_str(&format!(
            "        matches!((shape, field), {})\n",
            patterns.join(" | ")
        ));
    }
    code.push_str("    }\n}\n");
    Ok(code)
}

fn emit_facts_operations(code: &mut String, nodes: &[Value]) -> Result<(), String> {
    code.push_str("\n    pub(crate) fn cached_subtree_facts(&self, shape: u16, ordinal: u32) -> u32 {\n        match shape {\n");
    for (shape, node) in nodes.iter().enumerate() {
        if has_facts(node)? {
            code.push_str(&format!("            {shape} => self.{}.as_ref().expect(\"compact shape directory\").get(ordinal).expect(\"compact payload ordinal\").facts.load(Ordering::SeqCst),\n",snake(string(node,"name")?)));
        }
    }
    code.push_str("            _ => 0,\n        }\n    }\n\n    pub(crate) fn store_subtree_facts(&self, shape: u16, ordinal: u32, facts: u32) {\n        match shape {\n");
    for (shape, node) in nodes.iter().enumerate() {
        if has_facts(node)? {
            code.push_str(&format!("            {shape} => self.{}.as_ref().expect(\"compact shape directory\").get(ordinal).expect(\"compact payload ordinal\").facts.store(facts, Ordering::SeqCst),\n",snake(string(node,"name")?)));
        }
    }
    code.push_str("            _ => {},\n        }\n    }\n");
    Ok(())
}

fn emit_binding_operations(code: &mut String, nodes: &[Value]) -> Result<(), String> {
    code.push_str("\n    /// Decode fields carried by this concrete shape. The header separately records\n    /// whether a binding record has ever been materialized for this node.\n    pub(crate) fn read_binding(&self, shape: u16, ordinal: u32, context: CompactContext<'_>) -> NodeBinding {\n        match shape {\n");
    for (shape, node) in nodes.iter().enumerate() {
        let bindings = binding_fields(node)?;
        if bindings.is_empty() {
            continue;
        }
        code.push_str(&format!("            {shape} => {{\n                let row = self.{}.as_ref().expect(\"compact shape directory\").get(ordinal).expect(\"compact payload ordinal\");\n                NodeBinding {{\n",snake(string(node,"name")?)));
        for field in &bindings {
            code.push_str(&format!("                    {}: context.decode_{}(FieldKey::new({shape}, ordinal, {}), row.{}),\n",field.name,field.codec,field.tag,field.name));
        }
        if bindings.len() < BINDING_FIELDS.len() {
            code.push_str("                    ..NodeBinding::default()\n");
        }
        code.push_str("                }\n            }\n");
    }
    code.push_str("            _ => NodeBinding::default(),\n        }\n    }\n");
    for field in BINDING_FIELDS {
        let eligible = nodes
            .iter()
            .enumerate()
            .filter_map(|(shape, node)| match binding_fields(node) {
                Ok(fields) if fields.iter().any(|candidate| candidate.name == field.name) => {
                    Some(Ok((shape, node)))
                }
                Ok(_) => None,
                Err(error) => Some(Err(error)),
            })
            .collect::<Result<Vec<_>, String>>()?;
        code.push_str(&format!("\n    pub(crate) fn {}_key(shape: u16, ordinal: u32) -> Option<FieldKey> {{\n        match shape {{\n",field.name));
        for (shape, _) in &eligible {
            code.push_str(&format!(
                "            {shape} => Some(FieldKey::new({shape}, ordinal, {})),\n",
                field.tag
            ));
        }
        code.push_str("            _ => None,\n        }\n    }\n");
        if matches!(field.name, "symbol" | "locals") {
            code.push_str(&format!("\n    /// Read a stored binding word without decoding its owner namespace.\n    /// Zero remains Some(0); unsupported shapes return None.\n    pub(crate) fn local_{}_word(&self, shape: u16, ordinal: u32) -> Option<u32> {{\n        match shape {{\n", field.name));
            for (shape, node) in &eligible {
                code.push_str(&format!("            {shape} => Some(self.{}.as_ref().expect(\"compact shape directory\").get(ordinal).expect(\"compact payload ordinal\").{}),\n", snake(string(node, "name")?), field.name));
            }
            code.push_str("            _ => None,\n        }\n    }\n");
        }
        let setter = if field.name == "flow_node" {
            code.push_str("\n    #[inline]\n    pub(crate) fn set_flow_node(&mut self, shape: u16, ordinal: u32, word: u32) -> bool {\n        self.set_local_flow_node(shape, ordinal, word)\n    }\n");
            "set_local_flow_node".into()
        } else {
            format!("set_{}", field.name)
        };
        code.push_str(&format!("\n    /// Narrow binding writes do not modify syntax edges or its validation proof.\n    pub(crate) fn {setter}(&mut self, shape: u16, ordinal: u32, word: u32) -> bool {{\n        match shape {{\n"));
        for (shape, node) in eligible {
            code.push_str(&format!("            {shape} => {{ self.{}.as_mut().expect(\"compact shape directory\").get_mut(ordinal).expect(\"compact payload ordinal\").{} = word; }},\n",snake(string(node,"name")?),field.name));
        }
        code.push_str("            _ => return false,\n        }\n        true\n    }\n");
        let output = match field.name {
            "symbol" => Some("SymbolId"),
            "locals" => Some("SymbolTableId"),
            "flow_node" => Some("FlowId"),
            _ => None,
        };
        if let Some(output) = output {
            code.push_str(&format!("\n    pub(crate) fn read_{}(&self, shape: u16, ordinal: u32, context: CompactContext<'_>) -> Option<{output}> {{\n        match shape {{\n", field.name));
            for (shape, node) in nodes.iter().enumerate() {
                if binding_fields(node)?
                    .iter()
                    .any(|candidate| candidate.name == field.name)
                {
                    code.push_str(&format!("            {shape} => context.decode_{}(FieldKey::new({shape}, ordinal, {}), self.{}.as_ref().expect(\"compact shape directory\").get(ordinal).expect(\"compact payload ordinal\").{}),\n",field.codec,field.tag,snake(string(node,"name")?),field.name));
                }
            }
            code.push_str("            _ => None,\n        }\n    }\n");
        }
    }
    Ok(())
}

fn emit_reference_validation(code: &mut String, nodes: &[Value]) -> Result<(), String> {
    code.push_str("\n    /// Validate every stored reference in schema field order without constructing\n    /// a semantic payload enum. Parent edges remain the caller's preceding check.\n    pub(crate) fn validate_references<E>(&self, header: &StoredNode, context: CompactContext<'_>, mut node: impl FnMut(NodeId) -> Result<(), E>, mut list: impl FnMut(NodeListId) -> Result<(), E>, mut raw: impl FnMut(NodeSlice) -> Result<(), E>, mut text: impl FnMut(TextSlice) -> Result<(), E>) -> Result<(), E> {\n        let ordinal = header.ordinal;\n        match header.actual_shape() {\n");
    let mut empty_shapes = Vec::new();
    for (shape, definition) in nodes.iter().enumerate() {
        if !has_row(definition)? {
            empty_shapes.push(shape.to_string());
            continue;
        }
        let mut references = Vec::new();
        for (index, field) in fields(definition)?.into_iter().enumerate() {
            let typ = rust_type(&field["type"])?;
            let (codec, callback) = match typ.as_str() {
                "NodeId" => ("node", "node"),
                "NodeListId" => ("list", "list"),
                "NodeSlice" => ("node_slice", "raw"),
                "TextSlice" => ("text_slice", "text"),
                _ => continue,
            };
            references.push((index, field, codec, callback));
        }
        let binding = if references.is_empty() { "_" } else { "row" };
        code.push_str(&format!("            {shape} => {{\n                let {binding} = self.{}.as_ref().expect(\"compact shape directory\").get(ordinal).expect(\"compact payload ordinal\");\n", snake(string(definition,"name")?)));
        for (index, field, codec, callback) in references {
            let member = snake(string(field, "name")?);
            let key = field_key(shape, index);
            let decoded = format!("context.decode_{codec}({key}, row.{member})");
            if nullable(field)? {
                code.push_str(&format!(
                    "                if let Some(id) = {decoded} {{ {callback}(id)?; }}\n"
                ));
            } else {
                code.push_str(&format!("                {callback}({decoded})?;\n"));
            }
        }
        code.push_str("            }\n");
    }
    if !empty_shapes.is_empty() {
        code.push_str(&format!(
            "            {} => {{}},\n",
            empty_shapes.join(" | ")
        ));
    }
    code.push_str(
        "            _ => panic!(\"compact payload shape\"),\n        }\n        Ok(())\n    }\n",
    );
    Ok(())
}

fn identifier_text(node: &Value) -> bool {
    matches!(
        node["name"].as_str(),
        Some("Identifier" | "PrivateIdentifier")
    )
}

fn emit_text_operations(code: &mut String, nodes: &[Value]) -> Result<(), String> {
    let shapes = nodes
        .iter()
        .enumerate()
        .filter(|(_, node)| identifier_text(node))
        .map(|(shape, _)| shape.to_string())
        .collect::<Vec<_>>();
    code.push_str(
        "\n    #[inline]\n    pub(crate) fn has_source_relative_text(shape: u16) -> bool {\n",
    );
    if shapes.is_empty() {
        code.push_str("        let _ = shape;\n        false\n");
    } else {
        code.push_str(&format!(
            "        matches!(shape, {})\n",
            shapes.join(" | ")
        ));
    }
    code.push_str("    }\n");
    for (method, signature) in [
        ("release_text", "shape: u16, ordinal: u32, context: &mut PackingContext<'_>"),
        ("change_text_end", "shape: u16, ordinal: u32, old_end: i32, new_end: i32, context: &mut PackingContext<'_>"),
    ] {
        code.push_str(&format!("\n    pub(crate) fn {method}(&mut self, {signature}) {{\n        match shape {{\n"));
        for (shape,node) in nodes.iter().enumerate() {
            if method == "change_text_end" && !identifier_text(node) { continue; }
            let mut texts = Vec::new();
            for (index,field) in fields(node)?.iter().enumerate() {
                if rust_type(&field["type"])? == "JsString" {
                    texts.push((index, snake(string(field,"name")?)));
                }
            }
            if texts.is_empty() { continue; }
            code.push_str(&format!("            {shape} => {{\n                let row = self.{}.as_mut().expect(\"compact shape directory\").get_mut(ordinal).expect(\"compact payload ordinal\");\n",snake(string(node,"name")?)));
            for (index,field) in texts {
                let key=field_key(shape,index);
                if method == "release_text" {
                    code.push_str(&format!("                context.release_text({key}, row.{field});\n"));
                } else {
                    code.push_str(&format!("                row.{field} = context.change_text_end({key}, row.{field}, old_end, new_end);\n"));
                }
            }
            code.push_str("            }\n");
        }
        code.push_str("            _ => {},\n        }\n    }\n");
    }
    Ok(())
}
