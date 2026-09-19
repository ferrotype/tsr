//! Endpoint export of physical core syntax, distinct from binder access events.
//! All runtime-only accessors are added to a frozen diagnostic source copy.
//! The registry names omissions; this is not complete initial-state replay.

use crate::access_trace::{blob, event};
use crate::{AstStorageData, AstView, NodeData, SourceFileState, SourceMetadataData};
use std::sync::atomic::Ordering;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StateSummary {
    pub physical_core_nodes: u64,
    pub physical_core_auxiliary: u64,
    pub payload_fields: u64,
    /// Includes repeated selected values, file names and repeated source bytes.
    /// This is exported logical bytes, not unique backing or retained heap bytes.
    pub selected_bytes_exported: u64,
}

fn count(value: usize) -> u64 {
    u64::try_from(value).expect("trace count fits u64")
}

fn signed(value: i64) -> u64 {
    // Protocol preserves the full signed value as its two's-complement bits.
    value as u64
}

fn bytes(op: u16, site: u16, a: u64, b: u64, value: &[u8], summary: &mut StateSummary) {
    blob(op, site, a, b, value);
    summary.selected_bytes_exported = summary
        .selected_bytes_exported
        .checked_add(count(value.len()))
        .expect("exported byte count fits u64");
}

include!("access_trace_state_generated.rs");

/// Call while the recorder's domain is state, immediately before binding.
///
/// `ParsedFile::view()` borrows the exclusive parse without clearing its
/// validation proof. Physical iteration includes obsolete/unreachable slots and
/// preserves node/auxiliary allocation order independently. The endpoint cannot
/// recover the interleaving between those arenas or parser temporary allocations.
/// No getter assigns a runtime ID, computes facts/maps, or initializes lazy AST.
pub fn export(view: AstView<'_>) -> StateSummary {
    assert!(
        view.1.is_none(),
        "pre-bind state export cannot include a binding overlay"
    );
    let lazy_before = view.0.access_trace_lazy_counts();
    let layouts = view.0.access_trace_layout();
    let owner = layouts[0][0];
    event(
        10,
        0,
        owner,
        layouts[1][0],
        view.0.metadata().map_or(0, |id| id.bits()),
        count(view.source().as_bytes().len()),
    );
    for (index, [arena, len, capacity, pages, directory, element, alignment, page_descriptor]) in
        layouts.into_iter().enumerate()
    {
        let site = u16::try_from(index + 1).expect("two core arenas");
        event(11, site, arena, len, capacity, pages);
        event(12, site, arena, directory, element, page_descriptor);
        event(74, site, arena, alignment, 0, 0);
        for (page, (len, capacity)) in view.0.access_trace_pages(index == 1).enumerate() {
            event(13, site, arena, count(page), count(len), count(capacity));
        }
    }
    let [imports, imported_routes, supplemental] = view.0.access_trace_external_counts();
    event(
        75,
        0,
        owner,
        count(imports),
        count(imported_routes),
        count(supplemental),
    );
    let [reserved_nodes, reserved_aux, initialized_nodes, initialized_aux] = lazy_before;
    event(
        76,
        0,
        count(reserved_nodes),
        count(reserved_aux),
        count(initialized_nodes),
        count(initialized_aux),
    );
    let mut summary = StateSummary::default();
    bytes(23, 0, owner, 0, view.source().as_bytes(), &mut summary);
    for (id, node) in view.0.access_trace_core_nodes() {
        // Direct fields avoid accidentally observing assigning/computing getters.
        event(
            14,
            shape(&node.data),
            id.bits(),
            signed(i64::from(node.kind.raw())),
            node.parent.map_or(0, |id| id.bits()),
            u64::from(node.flags),
        );
        event(
            15,
            0,
            id.bits(),
            signed(i64::from(node.pos)),
            signed(i64::from(node.end)),
            0,
        );
        event(
            16,
            0,
            id.bits(),
            u64::from(node.subtree_facts.load(Ordering::SeqCst)),
            node.runtime_id.load(Ordering::SeqCst),
            0,
        );
        export_payload(id.bits(), &node.data, &mut summary);
        summary.physical_core_nodes += 1;
    }
    for (id, value) in view.0.access_trace_core_auxiliary() {
        let id = id.bits();
        summary.physical_core_auxiliary += 1;
        match value {
            AstStorageData::List(list) => {
                let nodes = list.nodes();
                event(24, 1, id, count(nodes.len()), 0, 0);
                event(
                    25,
                    0,
                    id,
                    signed(list.loc().pos()),
                    signed(list.loc().end()),
                    u64::from(list.modifier_flags()),
                );
                event(
                    26,
                    0,
                    id,
                    nodes.backing_id().map_or(0, |id| id.bits()),
                    u64::from(nodes.start()),
                    count(nodes.len()),
                );
            }
            AstStorageData::Nodes(nodes) => {
                event(24, 2, id, count(nodes.len()), 0, 0);
                for (index, node) in nodes.iter().enumerate() {
                    event(27, 0, id, count(index), node.map_or(0, |id| id.bits()), 0);
                }
            }
            AstStorageData::Text(text) => {
                event(24, 3, id, count(text.len()), 0, 0);
                for (index, text) in text.iter().enumerate() {
                    bytes(28, 0, id, count(index), text.as_bytes(), &mut summary);
                }
            }
            AstStorageData::File(file) => {
                event(24, 4, id, 1, 0, 0);
                event(
                    29,
                    0,
                    id,
                    file.root.map_or(0, |id| id.bits()),
                    signed(file.node_count),
                    signed(file.text_count),
                );
                event(30, 0, id, file.source_files.map_or(0, |id| id.bits()), 0, 0);
            }
            AstStorageData::SourceMetadata(metadata) => {
                event(24, 5, id, 1, 0, 0);
                export_metadata(id, metadata, &mut summary);
            }
            AstStorageData::SourceFiles(files) => {
                event(24, 6, id, count(files.len()), 0, 0);
                for (node, file) in files {
                    event(40, 0, id, node.bits(), 0, 0);
                    export_source(node.bits(), file, &mut summary);
                }
            }
        }
    }
    assert_eq!(
        lazy_before,
        view.0.access_trace_lazy_counts(),
        "state export initialized lazy storage"
    );
    assert_eq!(
        summary.physical_core_nodes, layouts[0][1],
        "physical node export count differs from arena"
    );
    assert_eq!(
        summary.physical_core_auxiliary, layouts[1][1],
        "physical auxiliary export count differs from arena"
    );
    event(
        79,
        0,
        summary.physical_core_nodes,
        summary.physical_core_auxiliary,
        summary.payload_fields,
        summary.selected_bytes_exported,
    );
    summary
}

fn export_metadata(id: u64, metadata: &SourceMetadataData, summary: &mut StateSummary) {
    match metadata {
        SourceMetadataData::Nodes(nodes) => {
            event(31, 1, id, count(nodes.len()), 0, 0);
            for (index, node) in nodes.iter().enumerate() {
                event(32, 0, id, count(index), node.map_or(0, |id| id.bits()), 0);
            }
        }
        SourceMetadataData::Text(text) => {
            event(31, 2, id, count(text.len()), 0, 0);
            for (index, text) in text.iter().enumerate() {
                bytes(33, 0, id, count(index), text.as_bytes(), summary);
            }
        }
        SourceMetadataData::Comments(comments) => {
            event(31, 3, id, count(comments.len()), 0, 0);
            for (index, comment) in comments.iter().enumerate() {
                event(
                    34,
                    3,
                    id,
                    count(index),
                    signed(comment.loc.pos()),
                    signed(comment.loc.end()),
                );
                event(
                    35,
                    1,
                    id,
                    count(index),
                    signed(i64::from(comment.kind as i32)),
                    0,
                );
            }
        }
        SourceMetadataData::References(references) => {
            event(31, 5, id, count(references.len()), 0, 0);
            for (index, reference) in references.iter().enumerate() {
                event(
                    34,
                    5,
                    id,
                    count(index),
                    signed(reference.loc.pos()),
                    signed(reference.loc.end()),
                );
                event(
                    35,
                    2,
                    id,
                    count(index),
                    signed(reference.resolution_mode),
                    0,
                );
                event(35, 3, id, count(index), u64::from(reference.preserve), 0);
                bytes(
                    36,
                    0,
                    id,
                    count(index),
                    reference.file_name.as_bytes(),
                    summary,
                );
            }
        }
        SourceMetadataData::Pragmas(pragmas) => {
            event(31, 4, id, count(pragmas.len()), 0, 0);
            event(37, 4, id, count(pragmas.len()), 0, 0);
        }
        SourceMetadataData::DiagnosticDirectives(directives) => {
            event(31, 6, id, count(directives.len()), 0, 0);
            event(37, 6, id, count(directives.len()), 0, 0);
        }
    }
}

fn export_source(id: u64, source: &SourceFileState, summary: &mut StateSummary) {
    let options = source.parse_options();
    let scalars = [
        signed(i64::from(source.language_variant.0)),
        signed(i64::from(source.script_kind.0)),
        u64::from(source.is_declaration_file),
        u64::from(source.uses_uri_style_node_core_modules.0),
        signed(source.identifier_count),
        u64::from(source.has_lazy_jsdoc),
        signed(source.node_count),
        signed(source.text_count),
        source
            .common_js_module_indicator()
            .map_or(0, |id| id.bits()),
        source.external_module_indicator.map_or(0, |id| id.bits()),
        source.hash.hi,
        source.hash.lo,
        u64::from(options.external_module_indicator_options.jsx),
        u64::from(options.external_module_indicator_options.force),
        u64::from(source.check_js_directive.is_some()),
    ];
    for (index, value) in scalars.into_iter().enumerate() {
        event(
            41,
            u16::try_from(index + 1).expect("15 source scalar sites"),
            id,
            value,
            0,
            0,
        );
    }
    bytes(42, 1, id, 0, source.file_name(), summary);
    bytes(42, 2, id, 0, source.path(), summary);
    bytes(42, 3, id, 0, source.text().as_bytes(), summary);
    let slices = [
        source.imports.access_trace_descriptor(),
        source.module_augmentations.access_trace_descriptor(),
        source.ambient_module_names.access_trace_descriptor(),
        source.comment_directives.access_trace_descriptor(),
        source.pragmas.access_trace_descriptor(),
        source.referenced_files.access_trace_descriptor(),
        source.type_reference_directives.access_trace_descriptor(),
        source.lib_reference_directives.access_trace_descriptor(),
    ];
    for (index, (backing, start, len)) in slices.into_iter().enumerate() {
        event(
            43,
            u16::try_from(index + 1).expect("eight source slice sites"),
            id,
            backing,
            u64::from(start),
            u64::from(len),
        );
    }
    for (index, node) in source.reparsed_clones.iter().enumerate() {
        event(44, 1, id, count(index), node.bits(), 0);
    }
    for (index, (len, capacity)) in [
        (
            source.reparsed_clones.len(),
            source.reparsed_clones.capacity(),
        ),
        (source.diagnostics.len(), source.diagnostics.capacity()),
        (
            source.js_diagnostics.len(),
            source.js_diagnostics.capacity(),
        ),
        (
            source.jsdoc_diagnostics.len(),
            source.jsdoc_diagnostics.capacity(),
        ),
    ]
    .into_iter()
    .enumerate()
    {
        event(
            45,
            u16::try_from(index + 1).expect("four source vector sites"),
            id,
            count(len),
            count(capacity),
            0,
        );
    }
    if let Some(directive) = source.check_js_directive {
        event(
            46,
            0,
            id,
            signed(directive.range.loc.pos()),
            signed(directive.range.loc.end()),
            u64::from(directive.enabled),
        );
        event(
            47,
            0,
            id,
            signed(i64::from(directive.range.kind.raw())),
            u64::from(directive.range.has_trailing_new_line),
            0,
        );
    }
    event(
        48,
        0,
        id,
        u64::from(source.content_mapper_info().is_some()),
        0,
        0,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{self, Write};
    use std::sync::{Arc, Mutex};

    #[derive(Clone, Default)]
    struct Sink(Arc<Mutex<Vec<u8>>>);
    impl Write for Sink {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn signed_protocol_preserves_negative_positions_and_open_kinds() {
        assert_eq!(signed(-1), u64::MAX);
        assert_eq!(signed(i64::MIN), 1 << 63);
        assert_eq!(signed(i64::from(i32::MIN)), 0xffff_ffff_8000_0000);
        assert_eq!(signed(i64::from(i16::MIN)), 0xffff_ffff_ffff_8000);
    }

    #[test]
    fn payload_shape_is_independent_of_open_kind_and_cached_state() {
        let mut node = crate::Node::from_factory_parts(
            crate::NodeKind::from_raw(-32768),
            NodeData::Identifier(crate::IdentifierData {
                text: crate::JsString::from_bytes(&[0xff, 0xed, 0xa0, 0x80][..]),
            }),
        );
        node.flags = u32::MAX;
        node.subtree_facts.store(0x8000_0001, Ordering::SeqCst);
        node.runtime_id
            .store(0xffff_ffff_ffff_ffff, Ordering::SeqCst);
        assert_eq!(shape(&node.data), 2);
        let mut summary = StateSummary::default();
        export_payload(0x1_0000_0001, &node.data, &mut summary);
        assert_eq!(summary.payload_fields, 1);
        assert_eq!(summary.selected_bytes_exported, 4);
        assert_eq!(node.runtime_id.load(Ordering::SeqCst), u64::MAX);
        assert_eq!(node.subtree_facts.load(Ordering::SeqCst), 0x8000_0001);
    }

    #[test]
    fn physical_export_keeps_obsolete_slots_raw_bytes_and_shared_subranges() {
        use crate::Factory;
        let counters = tsr_arena::Counters::new();
        let mut builder = crate::AstBuilder::new(
            tsr_jsstring::SourceText::from_bytes(&b"source"[..]),
            &counters,
        );
        let first = builder.new_node(
            crate::NodeKind::from_raw(-1),
            NodeData::Identifier(crate::IdentifierData {
                text: crate::JsString::from_bytes(&[0xff, 0xed, 0xa0, 0x80][..]),
            }),
        );
        let mut allocated = vec![first];
        for _ in 1..511 {
            allocated.push(builder.new_node(
                crate::SyntaxKind::EndOfFile.into(),
                NodeData::Token(crate::TokenData {}),
            ));
        }
        let last = allocated[510];
        let backing = builder
            .node_slice(vec![Some(first), None, Some(last)])
            .unwrap();
        let shared = backing.slice(1..3).unwrap();
        let full_header = builder
            .new_list(tsr_core::TextRange::new(-1, 2), backing)
            .unwrap();
        let sub_header = builder
            .new_list(tsr_core::TextRange::new(0, 2), shared)
            .unwrap();
        let allocated_empty = builder.node_slice(Vec::new()).unwrap();
        let empty_header = builder
            .new_list(tsr_core::TextRange::default(), allocated_empty)
            .unwrap();
        let nil_header = builder
            .new_list(tsr_core::TextRange::default(), crate::NodeSlice::empty())
            .unwrap();
        let missing_header = builder
            .new_list(tsr_core::TextRange::default(), crate::NodeSlice::empty())
            .unwrap();
        builder.mark_list_missing(missing_header).unwrap();
        let first_node = builder.node_mut(first).unwrap();
        first_node.subtree_facts.store(17, Ordering::SeqCst);
        first_node.runtime_id.store(23, Ordering::SeqCst);
        builder.seed_jsdoc(last, vec![first]).unwrap();
        // The root has no edges. All preceding nodes/list records are physical
        // retained slots, so a root-reachability walk would miss them.
        let parsed = builder.complete(last).unwrap();
        let before = counters.snapshot();
        let sink = Sink::default();
        crate::access_trace::start(Box::new(sink.clone()), 1_000_000).unwrap();
        crate::access_trace::context(0, 1);
        let summary = export(parsed.view());
        crate::access_trace::finish().unwrap();
        assert_eq!(counters.snapshot(), before);
        assert_eq!(summary.physical_core_nodes, 511);
        assert_eq!(summary.physical_core_auxiliary, 8); // file, two backings, five headers
        assert_eq!(summary.payload_fields, 1);
        assert_eq!(summary.selected_bytes_exported, 10);
        assert_eq!(
            crate::existing_runtime_node_id(&parsed.view().node(last).unwrap()),
            0
        );
        assert_eq!(
            crate::existing_runtime_node_id(&parsed.view().node(first).unwrap()),
            23
        );
        assert_eq!(
            parsed.view().node(first).unwrap().cached_subtree_facts(),
            17
        );

        let raw = sink.0.lock().unwrap();
        assert_eq!(&raw[..8], b"S07TRC01");
        assert_eq!(&raw[8..12], b"BLK1");
        let payload_len = u32::from_le_bytes(raw[24..28].try_into().unwrap()) as usize;
        assert_eq!(&raw[60 + payload_len..64 + payload_len], b"END1");
        let mut payload = &raw[60..60 + payload_len];
        let mut records = Vec::new();
        while !payload.is_empty() {
            let len = u32::from_le_bytes(payload[..4].try_into().unwrap()) as usize + 4;
            let op = u16::from_le_bytes(payload[4..6].try_into().unwrap());
            let site = u16::from_le_bytes(payload[6..8].try_into().unwrap());
            let values: [u64; 4] = std::array::from_fn(|index| {
                u64::from_le_bytes(payload[20 + index * 8..28 + index * 8].try_into().unwrap())
            });
            records.push((op, site, values, payload[52..len].to_vec()));
            payload = &payload[len..];
        }
        let observed: Vec<_> = records
            .iter()
            .filter(|row| row.0 == 14)
            .map(|row| row.2[0])
            .collect();
        assert_eq!(
            observed,
            allocated.iter().map(|id| id.bits()).collect::<Vec<_>>()
        );
        assert!(records.iter().any(|row| row.0 == 14
            && row.1 == 2
            && row.2[0] == first.bits()
            && row.2[1] == u64::MAX));
        assert!(records.iter().any(|row| row.0 == 22
            && row.2[0] == first.bits()
            && row.3 == [0xff, 0xed, 0xa0, 0x80]));
        for (header, descriptor) in [
            (full_header, [backing.backing_id().unwrap().bits(), 0, 3]),
            (sub_header, [backing.backing_id().unwrap().bits(), 1, 2]),
            (
                empty_header,
                [allocated_empty.backing_id().unwrap().bits(), 0, 0],
            ),
            (nil_header, [0, 0, 0]),
            (missing_header, [0, 1, 0]),
        ] {
            assert!(records.iter().any(|row| row.0 == 26
                && row.2 == [header.bits(), descriptor[0], descriptor[1], descriptor[2]]));
        }
        assert!(records
            .iter()
            .any(|row| row.0 == 27 && row.2 == [backing.backing_id().unwrap().bits(), 1, 0, 0]));
        assert_eq!(records.last().unwrap().0, 79);
    }
}
