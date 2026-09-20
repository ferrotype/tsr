//! Physical owned storage only. No traversal of lazy edges or assigning getters.
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use tsr_ast::{AstStorageData, BoundView, FlowData, NodeData, SourceMetadataData};

fn count(map: &mut BTreeMap<String, usize>, key: impl ToString) {
    *map.entry(key.to_string()).or_default() += 1;
}
fn arena([len, capacity, pages, directory_capacity, element_bytes]: [usize; 5]) -> Value {
    json!({"len":len,"capacity":capacity,"pages":pages,"directory_capacity":directory_capacity,
           "element_bytes":element_bytes})
}

/// Source-suffix candidate: no decoding or assumption about node.pos().
fn suffix_class(source: &[u8], end: i32, text: &[u8]) -> &'static str {
    if text.len() > (u32::MAX >> 1) as usize {
        return "length_escape";
    }
    let Some(end) = usize::try_from(end).ok() else {
        return "invalid_range";
    };
    let Some(start) = end.checked_sub(text.len()) else {
        return "invalid_range";
    };
    match source.get(start..end) {
        Some(raw) if raw == text => "source_suffix",
        Some(_) => "byte_mismatch",
        None => "invalid_range",
    }
}

pub fn observe(index: usize, view: BoundView<'_>, bound_in_place: bool) -> Value {
    let ast = view.ast();
    let result = view.result();
    let lazy_before = ast.owner_census_lazy();
    let mut core_shapes = BTreeMap::new();
    let mut node_runtime_ids_by_shape = BTreeMap::new();
    let mut identifier_shapes = BTreeMap::new();
    let mut identifier_classes = BTreeMap::new();
    let mut fallback_lengths = BTreeMap::new();
    let mut fallback_values = BTreeSet::<Vec<u8>>::new();
    let (mut identifier_bytes, mut suffix_bytes, mut fallback_bytes) = (0, 0, 0);
    for node in ast.owner_census_nodes() {
        count(&mut core_shapes, node.data().name());
        if tsr_ast::existing_runtime_node_id(node) != 0 {
            count(&mut node_runtime_ids_by_shape, node.data().name());
        }
        let text = match node.data() {
            NodeData::Identifier(data) => Some(&data.text),
            NodeData::PrivateIdentifier(data) => Some(&data.text),
            _ => None,
        };
        if let Some(text) = text {
            let bytes = text.as_bytes();
            let class = suffix_class(ast.source().as_bytes(), node.end(), bytes);
            count(&mut identifier_shapes, node.data().name());
            count(&mut identifier_classes, class);
            identifier_bytes += bytes.len();
            if class == "source_suffix" {
                suffix_bytes += bytes.len();
            } else {
                fallback_bytes += bytes.len();
                count(&mut fallback_lengths, bytes.len());
                fallback_values.insert(bytes.to_vec());
            }
        }
    }
    let mut aux_variants = BTreeMap::new();
    let mut metadata_variants = BTreeMap::new();
    let mut node_backing_lengths = BTreeMap::new();
    let mut node_backing_lengths_in_aux_order = Vec::new();
    let mut node_list_lengths = BTreeMap::new();
    let mut node_list_descriptors = BTreeSet::new();
    let (mut node_backing_nonnull, mut text_elements, mut metadata_text_elements) = (0, 0, 0);
    let (mut nil_lists, mut missing_lists, mut allocated_empty_lists, mut shifted_lists) =
        (0, 0, 0, 0);
    for aux in ast.owner_census_aux() {
        let name = match aux {
            AstStorageData::List(list) => {
                let nodes = list.nodes();
                count(&mut node_list_lengths, nodes.len());
                node_list_descriptors.insert((
                    nodes.backing_id().map(|id| id.bits()),
                    nodes.start(),
                    nodes.len(),
                ));
                nil_lists += usize::from(nodes.is_nil());
                missing_lists += usize::from(list.is_missing());
                allocated_empty_lists +=
                    usize::from(nodes.backing_id().is_some() && nodes.is_empty());
                shifted_lists += usize::from(nodes.start() != 0 && nodes.backing_id().is_some());
                "List"
            }
            AstStorageData::Nodes(nodes) => {
                count(&mut node_backing_lengths, nodes.len());
                node_backing_lengths_in_aux_order.push(nodes.len());
                node_backing_nonnull += nodes.iter().filter(|node| node.is_some()).count();
                "Nodes"
            }
            AstStorageData::Text(text) => {
                text_elements += text.len();
                "Text"
            }
            AstStorageData::File(_) => "File",
            AstStorageData::SourceFiles(_) => "SourceFiles",
            AstStorageData::SourceMetadata(metadata) => {
                let name = match metadata {
                    SourceMetadataData::Nodes(_) => "Nodes",
                    SourceMetadataData::Text(text) => {
                        metadata_text_elements += text.len();
                        "Text"
                    }
                    SourceMetadataData::Comments(_) => "Comments",
                    SourceMetadataData::Pragmas(_) => "Pragmas",
                    SourceMetadataData::References(_) => "References",
                    SourceMetadataData::DiagnosticDirectives(_) => "DiagnosticDirectives",
                };
                count(&mut metadata_variants, name);
                "SourceMetadata"
            }
        };
        count(&mut aux_variants, name);
    }
    let mut symbol_runtime_ids = 0;
    let mut declaration_descriptors = BTreeSet::new();
    let mut referenced_declaration_backings = BTreeSet::new();
    let mut declaration_slice_lengths_capacities = BTreeMap::new();
    for (_, symbol) in result.symbols().iter() {
        symbol_runtime_ids += usize::from(tsr_ast::existing_runtime_symbol_id(symbol) != 0);
        let slice = symbol.declarations;
        declaration_descriptors.insert((
            slice.backing_id().map(|id| id.bits()),
            slice.start(),
            slice.len(),
            slice.capacity(),
        ));
        if let Some(id) = slice.backing_id() {
            referenced_declaration_backings.insert(id);
        }
        count(
            &mut declaration_slice_lengths_capacities,
            format!("{}/{}", slice.len(), slice.capacity()),
        );
    }
    let mut declaration_backing_lengths = BTreeMap::new();
    let mut declaration_backing_nonnull = 0;
    let mut declaration_backings_not_referenced_by_symbols = 0;
    for (id, values) in result.declarations().iter() {
        count(&mut declaration_backing_lengths, values.len());
        declaration_backing_nonnull += values.iter().filter(|node| node.is_some()).count();
        declaration_backings_not_referenced_by_symbols +=
            usize::from(!referenced_declaration_backings.contains(&id));
    }
    let mut table_lengths_capacities = BTreeMap::new();
    for (_, table) in result.tables().iter() {
        count(
            &mut table_lengths_capacities,
            format!("{}/{}", table.len(), table.capacity()),
        );
    }
    let mut flow_data = BTreeMap::from([
        ("None", 0usize),
        ("Ast", 0),
        ("SwitchClause", 0),
        ("ReduceLabel", 0),
    ]);
    for (_, flow) in result.flows().iter() {
        let name = match flow.node {
            None => "None",
            Some(FlowData::Ast(_)) => "Ast",
            Some(FlowData::SwitchClause(_)) => "SwitchClause",
            Some(FlowData::ReduceLabel(_)) => "ReduceLabel",
        };
        *flow_data.get_mut(name).expect("enumerated flow class") += 1;
    }
    let [core, aux] = ast.owner_census_arenas();
    let [overlay, bindings] = result.owner_census_maps();
    let lazy_after = ast.owner_census_lazy();
    assert_eq!(lazy_before, lazy_after, "observer initialized lazy storage");
    json!({
        "version":1, "index":index, "bound_in_place":bound_in_place,
        "core_shapes":core_shapes, "node_runtime_ids_by_shape":node_runtime_ids_by_shape,
        "symbol_runtime_ids_assigned":symbol_runtime_ids,
        "identifiers":{"shapes":identifier_shapes,"classes":identifier_classes,"selected_bytes":identifier_bytes,
            "suffix_selected_bytes":suffix_bytes,"fallback_selected_bytes":fallback_bytes,
            "fallback_unique_values":fallback_values.len(),"fallback_unique_selected_bytes":fallback_values.iter().map(Vec::len).sum::<usize>(),
            "fallback_length_histogram":fallback_lengths},
        "arenas":{"core_nodes":arena(core),"core_aux":arena(aux),"symbols":arena(result.symbols().owner_census()),
            "tables":arena(result.tables().owner_census()),"declarations":arena(result.declarations().owner_census()),
            "flows":arena(result.flows().owner_census()),"flow_lists":arena(result.flow_lists().owner_census())},
        "aux_variants":aux_variants,"source_metadata_variants":metadata_variants,
        "text_slice_elements":text_elements,"metadata_text_slice_elements":metadata_text_elements,
        "node_lists":{"exposed_length_histogram":node_list_lengths,"distinct_descriptors":node_list_descriptors.len(),
            "nil":nil_lists,"missing":missing_lists,"allocated_empty":allocated_empty_lists,"nonzero_start":shifted_lists},
        "node_backings":{"physical_length_histogram":node_backing_lengths,"physical_lengths_in_aux_order":node_backing_lengths_in_aux_order,"nonnull_elements":node_backing_nonnull},
        "declarations":{"physical_length_histogram":declaration_backing_lengths,"nonnull_elements":declaration_backing_nonnull,
            "not_referenced_by_physical_symbols":declaration_backings_not_referenced_by_symbols,
            "distinct_symbol_descriptors":declaration_descriptors.len(),"symbol_length_capacity_histogram":declaration_slice_lengths_capacities},
        "tables":{"length_capacity_histogram":table_lengths_capacities},"flow_data":flow_data,
        "binding_maps":{"overlay":{"len":overlay[0],"capacity":overlay[1]},"bindings":{"len":bindings[0],"capacity":bindings[1]},
            "flow_slots":result.owner_census_flow_slots()},
        "lazy_slots_before":lazy_before,"lazy_slots_after":lazy_after,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn suffix_uses_bytes_and_checked_end_without_decoding() {
        assert_eq!(suffix_class(b" alpha", 6, b"alpha"), "source_suffix");
        assert_eq!(suffix_class(br"\u0061", 6, b"a"), "byte_mismatch");
        assert_eq!(
            suffix_class(&[0xff, 0xed, 0xa0, 0x80], 4, &[0xed, 0xa0, 0x80]),
            "source_suffix"
        );
        assert_eq!(suffix_class(b"a", -1, b"a"), "invalid_range");
        assert_eq!(suffix_class(b"a", 2, b"a"), "invalid_range");
        assert_eq!(suffix_class(b"a", 0, b"a"), "invalid_range");
        assert_eq!(suffix_class(b"", 0, b""), "source_suffix");
    }
    #[test]
    fn census_observes_allocated_core_and_does_not_assign_ids_or_lazy_nodes() {
        let mut parsed=tsr_parser::parse_source_file(tsr_jsstring::SourceText::from_bytes(br"/** deferred */ namespace N { export const \u0061 = 1; } function f(x:number){ return x; }".as_slice()),tsr_core::ScriptKind::TS,tsr_ast::SourceFileParseOptions { file_name: tsr_ast::JsString::from_bytes(b"/owner-census.ts".as_slice()), path: tsr_ast::JsString::from_bytes(b"/owner-census.ts".as_slice()), ..Default::default() });
        let unattached = parsed.builder_mut().node_slice(vec![None; 3]).unwrap();
        parsed
            .builder_mut()
            .new_list(tsr_core::TextRange::new(-1, -1), unattached)
            .unwrap();
        let file = tsr_binder::bind_parsed_file(parsed).unwrap();
        let original_ids: Vec<_> = file
            .view()
            .ast()
            .owner_census_nodes()
            .map(tsr_ast::existing_runtime_node_id)
            .collect();
        let original_symbol_ids: Vec<_> = file
            .view()
            .result()
            .symbols()
            .iter()
            .map(|(_, symbol)| tsr_ast::existing_runtime_symbol_id(symbol))
            .collect();
        let first = observe(0, file.view(), file.bound_in_place());
        let second = observe(0, file.view(), file.bound_in_place());
        assert_eq!(first, second);
        assert_eq!(
            original_ids,
            file.view()
                .ast()
                .owner_census_nodes()
                .map(tsr_ast::existing_runtime_node_id)
                .collect::<Vec<_>>()
        );
        assert_eq!(
            original_symbol_ids,
            file.view()
                .result()
                .symbols()
                .iter()
                .map(|(_, symbol)| tsr_ast::existing_runtime_symbol_id(symbol))
                .collect::<Vec<_>>()
        );
        assert_eq!(
            first["node_backings"]["physical_lengths_in_aux_order"]
                .as_array()
                .unwrap()
                .last(),
            Some(&json!(3))
        );
        assert_eq!(first["lazy_slots_before"], json!([0, 0, 0, 0]));
        assert!(
            first["identifiers"]["classes"]["byte_mismatch"]
                .as_u64()
                .unwrap()
                > 0
        );
        assert!(
            first["node_backings"]["physical_length_histogram"]
                .as_object()
                .unwrap()
                .values()
                .map(|v| v.as_u64().unwrap())
                .sum::<u64>()
                > 0
        );
    }
}
