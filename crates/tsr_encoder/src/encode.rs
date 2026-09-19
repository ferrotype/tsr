//! Protocol-8 serialization over borrowed AST storage and an explicit JSDoc service.
use crate::{
    string_table::StringTable,
    structured,
    walk::{Edge, Walk},
    DataType,
};
use std::{collections::BTreeMap, sync::Arc};
use ts_arena::Error;
use ts_ast::{
    AstView, JsDocProvider, NodeAccess, NodeId, NodeIndexCache, NodeKind, SourceFileState,
    SyntaxKind,
};
use ts_jsstring::PositionMap;

#[derive(Debug)]
pub struct Encoded {
    pub bytes: Vec<u8>,
    pub index: Option<Arc<NodeIndexCache>>,
}
// port: tsc/internal/api/encoder/encoder.go:SourceFileHash
pub fn source_file_hash(source: &SourceFileState) -> String {
    format!("{:016x}{:016x}", source.hash.hi, source.hash.lo)
}
// port: tsc/internal/api/encoder/encoder.go:BuildNodeIndexTable
pub fn build_node_index_table(
    view: AstView<'_>,
    source: NodeId,
    provider: &mut impl JsDocProvider,
) -> Result<NodeIndexCache, Error> {
    let mut walk = Walk::new(view, source, Some(source));
    let capacity = view.source_file(source)?.node_count.wrapping_add(1);
    assert!(capacity >= 1, "runtime error: makeslice: cap out of range");
    let mut nodes = Vec::with_capacity(capacity as usize);
    nodes.push(None);
    while let Some((_, _, edge)) = walk.next(provider)? {
        nodes.push(match edge {
            Edge::Node(id) => Some(id),
            Edge::List(_) => None,
        });
    }
    Ok(NodeIndexCache::new(nodes))
}
// port: tsc/internal/api/encoder/encoder.go:GetNodeIndexTable
pub fn get_node_index_table(
    view: AstView<'_>,
    source: NodeId,
    provider: &mut impl JsDocProvider,
) -> Result<Option<Arc<NodeIndexCache>>, Error> {
    let state = view.source_file(source)?;
    // Cache construction must propagate an ownership/provider error without publishing
    // a partial table. The file service therefore accepts a fallible initializer.
    let cached = state.try_node_index_cache(|| build_node_index_table(view, source, provider))?;
    Ok(cached.cloned())
}
// port: tsc/internal/api/encoder/encoder.go:EncodeSourceFile
pub fn encode_source_file(
    view: AstView<'_>,
    source: NodeId,
    provider: &mut impl JsDocProvider,
) -> Result<Encoded, Error> {
    let mut encoded = encode_node(view, source, Some(source), provider)?;
    let state = view.source_file(source)?;
    let fresh = encoded
        .index
        .take()
        .expect("fresh encoding supplies an index");
    encoded.index = state
        .node_index_cache(|| Arc::try_unwrap(fresh).expect("fresh encoding owns its index"))
        .cloned();
    Ok(encoded)
}
// port: tsc/internal/api/encoder/encoder.go:appendUint32s
pub(crate) fn words(out: &mut Vec<u8>, values: impl IntoIterator<Item = u32>) {
    for value in values {
        out.extend_from_slice(&value.to_le_bytes());
    }
}
fn patch(out: &mut [u8], offset: usize, value: u32) {
    out[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}
// port: tsc/internal/api/encoder/encoder.go:encodeParseOptions
fn parse_options(state: &SourceFileState) -> u32 {
    u32::from(state.parse_options().external_module_indicator_options.jsx)
        | (u32::from(
            state
                .parse_options()
                .external_module_indicator_options
                .force,
        ) << 1)
}
// port: tsc/internal/api/encoder/encoder.go:EncodeNode
/// Preserve Go's nullable root boundary for callers decoding arbitrary wire data.
pub fn encode_optional_node(
    view: AstView<'_>,
    root: Option<NodeId>,
    source: Option<NodeId>,
    provider: &mut impl JsDocProvider,
) -> Result<Encoded, Error> {
    let root = root.expect("nil root passed to EncodeNode");
    encode_node(view, root, source, provider)
}
// port: tsc/internal/api/encoder/encoder.go:encodeTree
pub fn encode_node(
    view: AstView<'_>,
    root: NodeId,
    source: Option<NodeId>,
    provider: &mut impl JsDocProvider,
) -> Result<Encoded, Error> {
    let root_is_file = view.node(root)?.kind() == SyntaxKind::SourceFile;
    let state = source.map(|id| view.source_file(id)).transpose()?;
    let empty_map = PositionMap::new(b"");
    let positions = state
        .as_ref()
        .map_or(&empty_map, |state| state.position_map());
    let text = if root_is_file {
        state
            .as_ref()
            .expect("SourceFile encoding requires source context")
            .text()
            .as_bytes()
    } else {
        b""
    };
    let mut strings = StringTable::new(
        text,
        if root_is_file {
            state.as_ref().expect("source context").text_count
        } else {
            0
        },
    );
    let mut extended = Vec::new();
    let mut structured = Vec::new();
    let capacity = state
        .as_ref()
        .map_or(0, |state| state.node_count)
        .wrapping_add(1);
    let node_bytes = capacity.wrapping_mul(28);
    assert!(
        node_bytes >= 0,
        "runtime error: makeslice: cap out of range"
    );
    let mut nodes = Vec::with_capacity(node_bytes as usize);
    nodes.resize(28, 0);
    assert!(capacity >= 1, "runtime error: makeslice: cap out of range");
    let mut ids = Vec::with_capacity(capacity as usize);
    ids.push(None);
    let mut previous = vec![0u32];
    let mut indices = BTreeMap::new();
    if root_is_file {
        let root_state = view.source_file(root)?;
        let imports = root_state.imports()?;
        let augmentations = root_state.module_augmentations()?;
        for id in imports.iter().chain(augmentations.iter()).flatten() {
            indices.insert(*id, 0);
        }
        if let Some(id) = root_state
            .external_module_indicator
            .filter(|id| *id != root)
        {
            indices.insert(id, 0);
        }
    }
    let mut walk = Walk::new(view, root, source);
    while let Some((index, parent, edge)) = walk.next(provider)? {
        if previous[parent as usize] != 0 {
            patch(
                &mut nodes,
                previous[parent as usize] as usize * 28 + 12,
                index,
            );
        }
        previous[parent as usize] = index;
        previous.push(0);
        match edge {
            Edge::Node(id) => {
                let node = view.node(id)?;
                let data = node_data(
                    view,
                    id,
                    &node,
                    &mut strings,
                    positions,
                    &mut extended,
                    &mut structured,
                )?;
                words(
                    &mut nodes,
                    [
                        node.kind().raw() as u32,
                        positions.utf8_to_utf16(node.pos() as isize) as u32,
                        positions.utf8_to_utf16(node.end() as isize) as u32,
                        0,
                        parent,
                        data,
                        node.flags(),
                    ],
                );
                ids.push(Some(id));
                if index != 1 {
                    if let Some(slot) = indices.get_mut(&id) {
                        *slot = index;
                    }
                }
            }
            Edge::List(id) => {
                let list = view.list(id)?;
                words(
                    &mut nodes,
                    [
                        u32::MAX,
                        positions.utf8_to_utf16(list.loc().pos() as isize) as u32,
                        positions.utf8_to_utf16(list.loc().end() as isize) as u32,
                        0,
                        parent,
                        list.nodes().len() as u32,
                        u32::from(view.list_has_trailing_comma(id)?),
                    ],
                );
                ids.push(None);
            }
        }
    }
    let mut hash = ts_ast::SourceHash::default();
    let mut options = 0;
    if root_is_file {
        let header_state = state.as_ref().expect("source context");
        hash = header_state.hash;
        options = parse_options(header_state);
        let root_state = view.source_file(root)?;
        let imports = node_indices(&root_state.imports()?, &indices, &mut structured);
        let augmentations = node_indices(
            &root_state.module_augmentations()?,
            &indices,
            &mut structured,
        );
        let names = structured::strings(
            root_state
                .ambient_module_names()?
                .iter()
                .map(ts_ast::JsString::as_bytes),
            &mut structured,
        );
        for (offset, value) in [
            (32, imports),
            (36, augmentations),
            (40, names),
            (
                44,
                root_state.external_module_indicator.map_or(0, |id| {
                    if id == root {
                        1
                    } else {
                        indices.get(&id).copied().unwrap_or(0)
                    }
                }),
            ),
        ] {
            patch(&mut extended, offset, value);
        }
    }
    let table_offset = 44usize;
    let string_offset = table_offset + strings.offsets.len() * 4;
    let extended_offset = string_offset + strings.string_length();
    let structured_offset = extended_offset + extended.len();
    let node_offset = structured_offset + structured.len();
    let mut bytes = Vec::with_capacity(
        44 + strings.encoded_length() + extended.len() + structured.len() + nodes.len(),
    );
    words(
        &mut bytes,
        [
            8 << 24,
            hash.lo as u32,
            (hash.lo >> 32) as u32,
            hash.hi as u32,
            (hash.hi >> 32) as u32,
            options,
            table_offset as u32,
            string_offset as u32,
            extended_offset as u32,
            structured_offset as u32,
            node_offset as u32,
        ],
    );
    strings.encode(&mut bytes);
    bytes.extend(extended);
    bytes.extend(structured);
    bytes.extend(nodes);
    Ok(Encoded {
        bytes,
        index: Some(Arc::new(NodeIndexCache::new(ids))),
    })
}
// port: tsc/internal/api/encoder/encoder.go:encodeNodeIndexArray
// port: tsc/internal/api/encoder/encoder.go:encodeModuleAugmentations
fn node_indices(
    values: &[Option<NodeId>],
    indices: &BTreeMap<NodeId, u32>,
    out: &mut Vec<u8>,
) -> u32 {
    if values.is_empty() {
        return structured::NONE;
    }
    let offset = out.len() as u32;
    structured::array(out, values.len());
    for value in values {
        structured::uint(
            out,
            value.and_then(|id| indices.get(&id).copied()).unwrap_or(0),
        );
    }
    offset
}
// port: tsc/internal/api/encoder/encoder.go:getNodeData
fn node_data(
    view: AstView<'_>,
    id: NodeId,
    node: &(impl NodeAccess + ?Sized),
    strings: &mut StringTable<'_>,
    positions: &PositionMap,
    extended: &mut Vec<u8>,
    structured: &mut Vec<u8>,
) -> Result<u32, Error> {
    let common = crate::runtime_generated::common_data(node);
    Ok(match crate::runtime_generated::data_type(node.kind()) {
        DataType::Children => common | crate::runtime_generated::child_mask(view, node),
        DataType::String => {
            0x4000_0000 | common | crate::runtime_generated::record_string(view, node, strings)
        }
        DataType::Extended => {
            let offset = extended.len() as u32;
            record_extended(view, id, node, strings, positions, extended, structured)?;
            0x8000_0000 | common | offset
        }
    })
}
// port: tsc/internal/api/encoder/encoder.go:recordExtendedData_SourceFile
fn record_source_file(
    view: AstView<'_>,
    id: NodeId,
    node: &(impl NodeAccess + ?Sized),
    strings: &mut StringTable<'_>,
    positions: &PositionMap,
    extended: &mut Vec<u8>,
    structured: &mut Vec<u8>,
) -> Result<(), Error> {
    let state = view.source_file(id)?;
    let text = strings.add(state.text().as_bytes(), node.kind(), node.pos(), node.end());
    let original = if state.original_text() == state.text().as_bytes() {
        text
    } else {
        strings.add(state.original_text(), NodeKind::default(), 0, 0)
    };
    let filename = strings.add(state.file_name(), NodeKind::default(), 0, 0);
    let path = strings.add(state.path(), NodeKind::default(), 0, 0);
    let references = structured::references(&state.referenced_files()?, positions, structured);
    let type_refs =
        structured::references(&state.type_reference_directives()?, positions, structured);
    let lib_refs =
        structured::references(&state.lib_reference_directives()?, positions, structured);
    let original_positions = PositionMap::new(state.original_text());
    let spans = structured::spans(state.span_map(), positions, &original_positions, structured);
    let supplemental = state
        .supplemental_source_files()?
        .iter()
        .map(|id| {
            view.source_file(
                id.expect("runtime error: invalid memory address or nil pointer dereference"),
            )
        })
        .collect::<Result<Vec<_>, _>>()?;
    let supplements = structured::strings(
        supplemental.iter().map(ts_ast::SourceFileRead::file_name),
        structured,
    );
    let canonical = state
        .canonical_source_file()
        .map(|id| view.source_file(id))
        .transpose()?;
    let canonical = canonical.map_or(structured::NONE, |file| {
        strings.add(file.file_name(), NodeKind::default(), 0, 0)
    });
    let mapper = if state.content_mapper().is_empty() {
        structured::NONE
    } else {
        strings.add(state.content_mapper(), NodeKind::default(), 0, 0)
    };
    let virtual_name = if state.virtual_file_name().is_empty() {
        structured::NONE
    } else {
        strings.add(state.virtual_file_name(), NodeKind::default(), 0, 0)
    };
    let directives = structured::directives(
        &state.diagnostic_directives()?,
        positions,
        &original_positions,
        structured,
    );
    words(
        extended,
        [
            text,
            filename,
            path,
            state.language_variant.0 as u32,
            state.script_kind.0 as u32,
            references,
            type_refs,
            lib_refs,
            structured::NONE,
            structured::NONE,
            structured::NONE,
            0,
            original,
            spans,
            supplements,
            canonical,
            mapper,
            virtual_name,
            directives,
        ],
    );
    Ok(())
}
// upstream: tsc/internal/api/encoder/encoder_generated.go:recordExtendedData
fn record_extended(
    view: AstView<'_>,
    id: NodeId,
    node: &(impl NodeAccess + ?Sized),
    strings: &mut StringTable<'_>,
    positions: &PositionMap,
    extended: &mut Vec<u8>,
    structured: &mut Vec<u8>,
) -> Result<(), Error> {
    macro_rules! literal {
        ($access:ident,$flag:ident) => {{
            let n = node
                .data()
                .$access()
                .expect("literal payload matches Go kind");
            let text = strings.add(n.text(), node.kind(), node.pos(), node.end());
            words(extended, [text, n.$flag() as u32]);
        }};
    }
    macro_rules! template {
        ($access:ident) => {{
            let n = node
                .data()
                .$access()
                .expect("template payload matches Go kind");
            let text = strings.add(n.text(), node.kind(), node.pos(), node.end());
            let raw = strings.add(n.raw_text(), node.kind(), node.pos(), node.end());
            words(extended, [text, raw, n.template_flags() as u32]);
        }};
    }
    match node.kind().known() {
        Some(SyntaxKind::SourceFile) => {
            record_source_file(view, id, node, strings, positions, extended, structured)?;
        }
        // port: tsc/internal/api/encoder/encoder.go:recordExtendedData_StringLiteral
        Some(SyntaxKind::StringLiteral) => literal!(as_string_literal, token_flags),
        // port: tsc/internal/api/encoder/encoder.go:recordExtendedData_NumericLiteral
        Some(SyntaxKind::NumericLiteral) => literal!(as_numeric_literal, token_flags),
        // port: tsc/internal/api/encoder/encoder.go:recordExtendedData_BigIntLiteral
        Some(SyntaxKind::BigIntLiteral) => literal!(as_big_int_literal, token_flags),
        // port: tsc/internal/api/encoder/encoder.go:recordExtendedData_RegularExpressionLiteral
        Some(SyntaxKind::RegularExpressionLiteral) => {
            literal!(as_regular_expression_literal, token_flags);
        }
        // port: tsc/internal/api/encoder/encoder.go:recordExtendedData_NoSubstitutionTemplateLiteral
        Some(SyntaxKind::NoSubstitutionTemplateLiteral) => {
            literal!(as_no_substitution_template_literal, template_flags);
        }
        // port: tsc/internal/api/encoder/encoder.go:recordExtendedData_TemplateHead
        Some(SyntaxKind::TemplateHead) => template!(as_template_head),
        // port: tsc/internal/api/encoder/encoder.go:recordExtendedData_TemplateMiddle
        Some(SyntaxKind::TemplateMiddle) => template!(as_template_middle),
        // port: tsc/internal/api/encoder/encoder.go:recordExtendedData_TemplateTail
        Some(SyntaxKind::TemplateTail) => template!(as_template_tail),
        _ => panic!("unexpected extended data node kind {}", node.kind()),
    }
    Ok(())
}
