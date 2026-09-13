//! Structural storage census of one checker, following the accounting frozen in
//! `data/s08/type-footprint.json`: every allocation is charged once, to one
//! family, with its full capacity. Type families charge their headers, payload
//! rows, owned lists, owned text and interning caches; the remaining checker
//! storage is reported beside them. A family the adapter cannot measure is
//! listed as unavailable, never charged as zero.
//!
//! The Go P1 adapter (`tools/s08/oracle/families/bridge.go`) measures the original
//! constructor families. P3 adds named Rust measurements; their Go counterparts
//! remain unmeasured until P7 and are not a footprint comparison.

use crate::{CheckerState, LiteralValue, TypeId};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;
use ts_ast::JsString;

/// `Arc<[T]>` and `Arc<[u8]>` allocations carry the strong and weak counts.
const ARC_HEADER: usize = 16;

#[path = "census_p3.rs"]
mod p3;
#[path = "census_reach.rs"]
mod reach;

#[derive(Default)]
struct Family {
    count: usize,
    bytes: usize,
}

#[derive(Default)]
pub(crate) struct Census {
    families: BTreeMap<&'static str, Family>,
    /// Allocations already charged, by address.
    seen: HashSet<usize>,
    unavailable: Vec<&'static str>,
}

impl Census {
    pub(crate) fn add(&mut self, family: &'static str, count: usize, bytes: usize) {
        let entry = self.families.entry(family).or_default();
        entry.count += count;
        entry.bytes += bytes;
    }

    pub(crate) fn list<T>(&mut self, family: &'static str, list: &Arc<[T]>) {
        if self.seen.insert(list.as_ptr() as usize) {
            self.add(family, 0, ARC_HEADER + list.len() * size_of::<T>());
        }
    }

    pub(crate) fn text(&mut self, family: &'static str, text: &JsString) {
        let backing = text.backing_bytes();
        if self.seen.insert(backing.as_ptr() as usize) {
            self.add(family, 0, ARC_HEADER + backing.len());
        }
    }

    pub(crate) fn vec_capacity<T>(&mut self, family: &'static str, vec: &[T], capacity: usize) {
        self.add(family, vec.len(), capacity * size_of::<T>());
    }

    fn finish(self, type_families: &[&str], created: usize, reachable: usize) -> Value {
        let type_storage: usize = self
            .families
            .iter()
            .filter(|(name, _)| type_families.contains(name))
            .map(|(_, family)| family.bytes)
            .sum();
        let total: usize = self.families.values().map(|family| family.bytes).sum();
        json!({
            "families": self.families.iter().map(|(name, family)| {
                (name.to_string(), json!({"count": family.count, "bytes": family.bytes}))
            }).collect::<serde_json::Map<String, Value>>(),
            "type_storage_bytes": type_storage,
            "checker_bytes": total,
            "types": {"created": created, "reachable": reachable, "unreachable_occupied": created - reachable},
            "unavailable": self.unavailable,
            "record_sizes": {
                "TypeRecord": size_of::<crate::TypeRecord>(), "IntrinsicData": size_of::<crate::IntrinsicData>(),
                "LiteralData": size_of::<crate::LiteralData>(), "ObjectData": size_of::<crate::ObjectData>(),
                "ReferenceData": size_of::<crate::ReferenceData>(), "InterfaceData": size_of::<crate::InterfaceData>(),
                "TupleData": size_of::<crate::TupleData>(), "UnionData": size_of::<crate::UnionData>(),
                "TypeParameterData": size_of::<crate::TypeParameterData>(), "TemplateLiteralData": size_of::<crate::TemplateLiteralData>(),
                "TypeAlias": size_of::<crate::TypeAlias>(), "Signature": size_of::<crate::Signature>(),
                "IndexInfo": size_of::<crate::IndexInfo>(), "Symbol": size_of::<ts_ast::Symbol>(),
                "ValueSymbolLinks": size_of::<crate::ValueSymbolLinks>(), "OptionValueSymbolLinks": size_of::<Option<crate::ValueSymbolLinks>>(),
                "MappedData": size_of::<crate::types::MappedData>(), "ReverseMappedData": size_of::<crate::types::ReverseMappedData>(),
                "InstantiationExpressionData": size_of::<crate::types::InstantiationExpressionData>(),
                "IndexData": size_of::<crate::types::IndexData>(), "IndexedAccessData": size_of::<crate::types::IndexedAccessData>(),
                "StringMappingData": size_of::<crate::types::StringMappingData>(), "SubstitutionData": size_of::<crate::types::SubstitutionData>(),
                "ConditionalData": size_of::<crate::types::ConditionalData>(),
            },
        })
    }
}

/// Every Rust family, including the additions not yet measured by the Go P1 adapter.
pub(crate) const ALL_FAMILIES: &[&str] = &[
    "type_records",
    "intrinsic",
    "literal",
    "unique_es_symbol",
    "anonymous",
    "evolving_arrays",
    "reference",
    "interface",
    "tuple",
    "union",
    "intersection",
    "type_parameter",
    "template_literal",
    "mapped",
    "reverse_mapped",
    "instantiation_expression",
    "index",
    "indexed_access",
    "string_mapping",
    "substitution",
    "conditional",
    "alias",
    "type_lists",
    "type_caches",
    "symbols",
    "symbol_tables",
    "signatures",
    "index_infos",
    "type_predicates",
    "value_symbol_links",
    "synthetic_expression_links",
    "checker_ast",
    "mappers",
    "inference",
    "relations",
    "query_links",
    "conditional_roots",
    "variance",
    "late_members",
    "mapped_symbol_links",
    "signature_caches",
    "declarations",
    "program_indices",
    "resolution",
    "diagnostics",
    "flow_analysis",
    "enum_links",
    "enum_relations",
    "body_check_state",
    "call_resolution",
    "deferred_checks",
    "iteration_cache",
    "module_aliases",
];

/// Type families whose bytes sum to the footprint statistic's numerator.
pub(crate) const TYPE_FAMILIES: &[&str] = &[
    "type_records",
    "intrinsic",
    "literal",
    "unique_es_symbol",
    "anonymous",
    "evolving_arrays",
    "reference",
    "interface",
    "tuple",
    "union",
    "intersection",
    "type_parameter",
    "template_literal",
    "mapped",
    "reverse_mapped",
    "instantiation_expression",
    "index",
    "indexed_access",
    "string_mapping",
    "substitution",
    "conditional",
    "alias",
    "type_lists",
    "type_caches",
];

#[test]
fn sliced_text_charges_its_full_shared_backing_once() {
    let text = JsString::from_bytes(vec![b'x'; 4096]);
    let mut census = Census::default();
    census.text("literal", &text.slice(12..15).unwrap());
    census.text("literal", &text.slice(200..201).unwrap());
    census.text("literal", &text);
    assert_eq!(census.families["literal"].bytes, ARC_HEADER + 4096);
}

impl CheckerState {
    /// The census over this checker with `roots` as the retained results.
    pub(crate) fn census(&self, roots: &[TypeId]) -> Result<Value, crate::Error> {
        let mut census = Census::default();
        for &family in ALL_FAMILIES {
            census.add(family, 0, 0);
        }
        let tables = self.types.tables();
        census.vec_capacity("type_records", tables.records, tables.records.capacity());
        census.vec_capacity("intrinsic", tables.intrinsics, tables.intrinsics.capacity());
        for data in tables.intrinsics {
            census.text("intrinsic", &data.name);
        }
        census.vec_capacity("literal", tables.literals, tables.literals.capacity());
        for data in tables.literals {
            match &data.value {
                LiteralValue::String(text) => census.text("literal", text),
                LiteralValue::BigInt(value) => {
                    census.add("literal", 0, value.base10_value.capacity());
                }
                LiteralValue::Number(_) | LiteralValue::Boolean(_) | LiteralValue::ComputedEnum => {
                }
            }
        }
        census.vec_capacity(
            "unique_es_symbol",
            tables.unique_symbols,
            tables.unique_symbols.capacity(),
        );
        for data in tables.unique_symbols {
            census.text("unique_es_symbol", &data.name);
        }
        census.vec_capacity("anonymous", tables.anonymous, tables.anonymous.capacity());
        for data in tables.anonymous {
            Self::census_object(&mut census, "anonymous", data);
        }
        census.vec_capacity(
            "evolving_arrays",
            tables.evolving_arrays,
            tables.evolving_arrays.capacity(),
        );
        for data in tables.evolving_arrays {
            Self::census_object(&mut census, "evolving_arrays", &data.object);
        }
        census.vec_capacity("reference", tables.references, tables.references.capacity());
        for data in tables.references {
            Self::census_reference(&mut census, "reference", data);
        }
        census.vec_capacity("interface", tables.interfaces, tables.interfaces.capacity());
        for data in tables.interfaces {
            Self::census_interface(&mut census, "interface", data);
        }
        census.vec_capacity("tuple", tables.tuples, tables.tuples.capacity());
        for data in tables.tuples {
            Self::census_interface(&mut census, "tuple", &data.interface);
            if census.seen.insert(data.element_infos.as_ptr() as usize) {
                census.add(
                    "tuple",
                    0,
                    ARC_HEADER + data.element_infos.len() * size_of::<crate::TupleElementInfo>(),
                );
            }
        }
        census.vec_capacity("union", tables.unions, tables.unions.capacity());
        for data in tables.unions {
            census.list("type_lists", &data.types);
            Self::census_union_common(&mut census, &data.common);
            if let Some(name) = &data.key_property_name {
                census.text("union", name);
            }
            if let Some(map) = &data.constituent_map {
                census.add(
                    "union",
                    0,
                    size_of::<crate::types::Map<TypeId, TypeId>>() + map.allocation_size(),
                );
            }
        }
        census.vec_capacity(
            "intersection",
            tables.intersections,
            tables.intersections.capacity(),
        );
        for data in tables.intersections {
            census.list("type_lists", &data.types);
            Self::census_union_common(&mut census, &data.common);
        }
        census.vec_capacity(
            "type_parameter",
            tables.type_parameters,
            tables.type_parameters.capacity(),
        );
        census.vec_capacity(
            "template_literal",
            tables.template_literals,
            tables.template_literals.capacity(),
        );
        for data in tables.template_literals {
            census.list("type_lists", &data.types);
            if census.seen.insert(data.texts.as_ptr() as usize) {
                census.add(
                    "template_literal",
                    0,
                    ARC_HEADER + data.texts.len() * size_of::<JsString>(),
                );
            }
            for text in data.texts.iter() {
                census.text("template_literal", text);
            }
        }
        census.vec_capacity("alias", tables.aliases, tables.aliases.capacity());
        for alias in tables.aliases {
            census.list("type_lists", &alias.type_arguments);
        }
        self.census_caches(&mut census);
        self.census_p3(&mut census);
        self.flow.census(&mut |_, count, allocation, keys| {
            census.add("flow_analysis", count, allocation + keys);
        });
        self.census_enums(&mut census);
        self.body_checks.census(&mut census);
        self.calls.census(&mut census);
        self.iteration.census(&mut census);
        self.module_aliases.census(&mut census);
        self.synthetic_scopes.census(&mut census);
        census.links("query_links", &self.emit.visible);
        census.links("query_links", &self.emit.aliases_marked);
        census.links("query_links", &self.emit_checks.node_flags);
        census.links("query_links", &self.emit_checks.requested_helpers);
        census.links("query_links", &self.emit_checks.helpers_module);
        census.links("query_links", &self.emit_checks.computed_names);
        for name in self.emit_checks.computed_names.values().flatten() {
            census.text("query_links", name);
        }

        census.map("type_caches", &self.promises.promised);
        census.map("type_caches", &self.promises.awaited);
        census.vec_capacity(
            "query_links",
            &self.promises.stack,
            self.promises.stack.capacity(),
        );
        census.vec_capacity(
            "deferred_checks",
            &self.deferred_checks.pending,
            self.deferred_checks.pending.capacity(),
        );
        census.set("deferred_checks", &self.deferred_checks.reported_properties);

        // Storage beside the type families.
        census.add(
            "symbols",
            self.symbols.len(),
            self.symbols.structural_bytes(),
        );
        for (_, symbol) in self.symbols.iter() {
            census.text("symbols", &symbol.name);
        }
        census.add(
            "symbol_tables",
            self.tables.table_count(),
            self.tables.structural_bytes(),
        );
        let (signature_capacity, index_info_capacity, predicate_capacity) =
            self.signatures.capacities();
        census.add(
            "signatures",
            self.signatures.len(),
            signature_capacity * size_of::<crate::Signature>(),
        );
        for index in 0..self.signatures.len() {
            let id = crate::SignatureId::new(index as u32 + 1).expect("nonzero");
            let signature = self.signatures.get(id).expect("published signature");
            if let Some(list) = &signature.type_parameters {
                census.list("type_lists", list);
            }
            if let Some(list) = &signature.parameters {
                census.list("signatures", list);
            }
            if let Some(composite) = &signature.composite {
                census.list("signatures", &composite.signatures);
            }
        }
        census.add(
            "index_infos",
            self.signatures.index_info_count(),
            index_info_capacity * size_of::<crate::IndexInfo>(),
        );
        for index in 0..self.signatures.index_info_count() {
            let id = crate::IndexInfoId::new(index as u32 + 1).expect("published index info");
            if let Some(components) = &self
                .signatures
                .index_info(id)
                .expect("published index info")
                .components
            {
                census.list("index_infos", components);
            }
        }
        census.add(
            "type_predicates",
            self.signatures.predicate_count(),
            predicate_capacity * size_of::<crate::TypePredicate>(),
        );
        for index in 0..self.signatures.predicate_count() {
            let id = crate::TypePredicateId::new(index as u32 + 1).expect("nonzero");
            census.text(
                "type_predicates",
                &self
                    .signatures
                    .predicate(id)
                    .expect("published predicate")
                    .parameter_name,
            );
        }
        census.add(
            "value_symbol_links",
            self.value_symbol_links.len(),
            self.value_symbol_links.structural_bytes(),
        );
        census.add(
            "synthetic_expression_links",
            self.synthetic_expression_types.len(),
            self.synthetic_expression_types.allocation_size(),
        );
        // The checker's synthetic AST arena has no byte accounting yet; its node
        // count is reported and the family is named as unavailable.
        census.add(
            "checker_ast",
            usize::try_from(self.factory.node_count()).unwrap_or(0),
            0,
        );
        census.unavailable.push("checker_ast");

        let created = self.types.len();
        let reachable = self.reachable_types(roots)?;
        Ok(census.finish(TYPE_FAMILIES, created, reachable))
    }

    fn census_structured(
        census: &mut Census,
        family: &'static str,
        data: &crate::StructuredMembers,
    ) {
        if let Some(list) = &data.properties {
            census.list(family, list);
        }
        if let Some(list) = &data.signatures {
            census.list(family, list);
        }
        if let Some(list) = &data.index_infos {
            census.list(family, list);
        }
    }

    fn census_object(census: &mut Census, family: &'static str, data: &crate::ObjectData) {
        Self::census_structured(census, family, &data.structured);
        if let Some(map) = &data.instantiations {
            let keys: usize = map.keys().map(|key| key.len()).sum();
            census.add(
                "type_caches",
                map.len(),
                size_of::<crate::types::Map<crate::CacheKey, TypeId>>()
                    + map.allocation_size()
                    + keys,
            );
        }
    }

    fn census_reference(census: &mut Census, family: &'static str, data: &crate::ReferenceData) {
        Self::census_object(census, family, &data.object);
        if let Some(list) = &data.resolved_type_arguments {
            census.list("type_lists", list);
        }
    }

    fn census_interface(census: &mut Census, family: &'static str, data: &crate::InterfaceData) {
        Self::census_reference(census, family, &data.reference);
        for list in [&data.all_type_parameters, &data.resolved_base_types]
            .into_iter()
            .flatten()
        {
            census.list("type_lists", list);
        }
        for list in [
            &data.declared_call_signatures,
            &data.declared_construct_signatures,
        ]
        .into_iter()
        .flatten()
        {
            census.list(family, list);
        }
        if let Some(list) = &data.declared_index_infos {
            census.list(family, list);
        }
    }

    fn census_union_common(census: &mut Census, data: &crate::UnionOrIntersectionMembers) {
        Self::census_structured(census, "union", &data.structured);
        if let Some(list) = &data.resolved_properties {
            census.list("union", list);
        }
    }

    fn census_caches(&self, census: &mut Census) {
        let caches = &self.types.caches;
        let mut charge = |name: &'static str, entries: usize, allocation: usize, keys: usize| {
            census.add("type_caches", entries, allocation + keys);
            let _ = name;
        };
        // String keys share their backing with the literal's value, charged above.
        charge(
            "stringLiteralTypes",
            caches.string_literal_types.len(),
            caches.string_literal_types.allocation_size(),
            0,
        );
        charge(
            "numberLiteralTypes",
            caches.number_literal_types.len(),
            caches.number_literal_types.allocation_size(),
            0,
        );
        charge(
            "bigintLiteralTypes",
            caches.bigint_literal_types.len(),
            caches.bigint_literal_types.allocation_size(),
            caches
                .bigint_literal_types
                .keys()
                .map(|key| key.base10_value.capacity())
                .sum(),
        );
        charge(
            "unionTypes",
            caches.union_types.len(),
            caches.union_types.allocation_size(),
            caches.union_types.keys().map(|key| key.len()).sum(),
        );
        charge(
            "unionOfUnionTypes",
            caches.union_of_union_types.len(),
            caches.union_of_union_types.allocation_size(),
            caches
                .union_of_union_types
                .keys()
                .map(|key| key.alias.len())
                .sum(),
        );
        charge(
            "tupleTypes",
            caches.tuple_types.len(),
            caches.tuple_types.allocation_size(),
            caches.tuple_types.keys().map(|key| key.len()).sum(),
        );
        charge(
            "intersectionTypes",
            caches.intersection_types.len(),
            caches.intersection_types.allocation_size(),
            caches.intersection_types.keys().map(|key| key.len()).sum(),
        );
        charge(
            "templateLiteralTypes",
            caches.template_literal_types.len(),
            caches.template_literal_types.allocation_size(),
            caches
                .template_literal_types
                .keys()
                .map(|key| key.len())
                .sum(),
        );
    }
}
