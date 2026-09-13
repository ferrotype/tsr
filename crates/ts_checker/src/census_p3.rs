//! Storage introduced by type instantiation, inference and source queries.
//! This extends the diagnostic census; full Go/Rust footprint acceptance is P7.

use super::Census;
use crate::CheckerState;

impl Census {
    pub(crate) fn diagnostic(&mut self, value: &ts_ast::Diagnostic) {
        for text in [&value.source, &value.message_text, &value.message_key] {
            self.text("diagnostics", text);
        }
        self.vec_capacity(
            "diagnostics",
            &value.message_args,
            value.message_args.capacity(),
        );
        for text in &value.message_args {
            self.text("diagnostics", text);
        }
        for chain in [&value.message_chain, &value.related_information] {
            self.vec_capacity("diagnostics", chain, chain.capacity());
            for diagnostic in chain {
                if self
                    .seen
                    .insert(std::sync::Arc::as_ptr(diagnostic) as usize)
                {
                    self.add(
                        "diagnostics",
                        0,
                        super::ARC_HEADER + size_of::<ts_ast::Diagnostic>(),
                    );
                    self.diagnostic(diagnostic);
                }
            }
        }
    }
    pub(crate) fn map<K: Eq + std::hash::Hash, V>(
        &mut self,
        family: &'static str,
        map: &crate::types::Map<K, V>,
    ) {
        self.add(family, map.len(), map.allocation_size());
    }
    pub(crate) fn set<T: Eq + std::hash::Hash>(
        &mut self,
        family: &'static str,
        set: &crate::types::Set<T>,
    ) {
        self.add(family, set.len(), set.allocation_size());
    }
    pub(crate) fn links<K: crate::links::LinkKey, V>(
        &mut self,
        family: &'static str,
        links: &crate::LinkStore<K, V>,
    ) {
        self.add(family, links.len(), links.structural_bytes());
    }
    pub(crate) fn key_map<V>(
        &mut self,
        family: &'static str,
        map: &crate::types::Map<crate::CacheKey, V>,
    ) {
        self.map(family, map);
        self.add(family, 0, map.keys().map(|key| key.len()).sum());
    }
}

impl CheckerState {
    pub(super) fn census_p3(&self, census: &mut Census) {
        let tables = self.types.tables();
        census.vec_capacity("mapped", tables.mapped, tables.mapped.capacity());
        for data in tables.mapped {
            Self::census_object(census, "mapped", &data.object);
        }
        census.vec_capacity(
            "reverse_mapped",
            tables.reverse_mapped,
            tables.reverse_mapped.capacity(),
        );
        for data in tables.reverse_mapped {
            Self::census_object(census, "reverse_mapped", &data.object);
        }
        census.vec_capacity(
            "instantiation_expression",
            tables.instantiation_expressions,
            tables.instantiation_expressions.capacity(),
        );
        for data in tables.instantiation_expressions {
            Self::census_object(census, "instantiation_expression", &data.object);
        }
        census.vec_capacity("index", tables.indexes, tables.indexes.capacity());
        census.vec_capacity(
            "indexed_access",
            tables.indexed_accesses,
            tables.indexed_accesses.capacity(),
        );
        census.vec_capacity(
            "string_mapping",
            tables.string_mappings,
            tables.string_mappings.capacity(),
        );
        census.vec_capacity(
            "substitution",
            tables.substitutions,
            tables.substitutions.capacity(),
        );
        census.vec_capacity(
            "conditional",
            tables.conditionals,
            tables.conditionals.capacity(),
        );

        let caches = &self.types.caches;
        census.key_map("type_caches", &caches.subtype_reductions);
        for types in caches.subtype_reductions.values() {
            census.list("type_lists", types);
        }
        census.key_map("type_caches", &caches.indexed_access_types);
        census.map("type_caches", &caches.index_types);
        census.map("type_caches", &caches.simplified_types);
        census.map("type_caches", &caches.equivalent_bases);
        census.map("type_caches", &caches.literal_union_bases);
        census.map("type_caches", &caches.substitution_types);
        census.map("type_caches", &caches.regular_object_literals);
        census.map("type_caches", &caches.string_mapping_types);
        census.map("type_caches", &caches.property_types);

        let query = &self.query;
        census.links("query_links", &query.declared_types);
        census.links("query_links", &query.type_nodes);
        census.links("query_links", &query.resolved_symbols);
        census.map("query_links", &query.global_types);
        census.map("query_links", &query.global_type_aliases);
        census.map("query_links", &query.this_assignments);
        census.links("query_links", &query.references);
        census.links("query_links", &query.scope_changes);
        census.links("query_links", &query.deferred_property_types);
        census.links("query_links", &query.deferred_property_write_types);
        for types in query
            .deferred_property_types
            .values()
            .chain(query.deferred_property_write_types.values())
            .flatten()
        {
            census.list("type_lists", types);
        }
        census.links("query_links", &query.type_aliases);
        for links in query.type_aliases.values() {
            if let Some(types) = &links.parameters {
                census.list("type_lists", types);
            }
            census.key_map("type_caches", &links.instantiations);
        }
        census.links("query_links", &query.outer_type_parameters);
        for types in query.outer_type_parameters.values().flatten() {
            census.list("type_lists", types);
        }
        census.links("query_links", &query.source_signatures);
        census.map("query_links", &query.apparent_types);
        census.set("query_links", &query.type_parameters_checked);
        census.set("query_links", &query.index_constraints_checked);
        census.set("query_links", &query.accessor_pairs_checked);
        census.map("query_links", &query.context_free_types);
        census.map("type_caches", &query.array_literal_types);
        census.map("type_caches", &query.widened_types);
        census.map("query_links", &query.assertion_types);
        census.map("query_links", &query.unresolved_symbols);
        for name in query.unresolved_symbols.keys() {
            census.text("query_links", name);
        }
        census.map("query_links", &query.undefined_properties);
        for name in query.undefined_properties.keys() {
            census.text("query_links", name);
        }
        census.map("query_links", &self.bindings.pattern_for_type);
        census.map("query_links", &self.bindings.spread_links);
        census.map("query_links", &self.bindings.discriminated_contexts);
        census.vec_capacity(
            "query_links",
            &self.bindings.contextual_patterns,
            self.bindings.contextual_patterns.capacity(),
        );
        census.set("query_links", &query.function_symbols_checked);
        census.set("query_links", &query.reported_unreachable);
        census.map("query_links", &self.merged_symbols);
        census.map("query_links", &self.source_checks);
        census.links("mapped_symbol_links", &self.mapped_symbol_links);

        census.links("late_members", &self.late_members.members);
        census.links("late_members", &self.late_members.declarations);
        census.links("late_members", &self.late_members.symbols);
        census.map("type_caches", &self.late_members.unique_types);

        census.vec_capacity(
            "conditional_roots",
            &self.conditional.roots,
            self.conditional.roots.capacity(),
        );
        for root in &self.conditional.roots {
            census.list("type_lists", &root.infer_parameters);
            census.list("type_lists", &root.outer_parameters);
        }
        census.map("type_caches", &self.conditional.instantiations);
        census.add(
            "type_caches",
            0,
            self.conditional
                .instantiations
                .keys()
                .map(|(_, key)| key.len())
                .sum(),
        );
        census.map("type_caches", &self.conditional.permissive);
        census.map("type_caches", &self.conditional.restrictive);
        census.map("type_caches", &self.conditional.restrictive_parameters);

        census.vec_capacity(
            "inference",
            &self.inference.contexts,
            self.inference.contexts.capacity(),
        );
        for context in &self.inference.contexts {
            census.vec_capacity(
                "inference",
                &context.inferred_type_parameters,
                context.inferred_type_parameters.capacity(),
            );
            census.vec_capacity(
                "inference",
                &context.intra_expression_sites,
                context.intra_expression_sites.capacity(),
            );
            census.vec_capacity(
                "inference",
                &context.inferences,
                context.inferences.capacity(),
            );
            for info in &context.inferences {
                census.vec_capacity("inference", &info.candidates, info.candidates.capacity());
                census.vec_capacity(
                    "inference",
                    &info.contra_candidates,
                    info.contra_candidates.capacity(),
                );
            }
        }
        self.inference.reverse.census(census);
        self.instantiation.census(census);
        self.relations.census(census);
        census.map("variance", &self.variance.links);
        for flags in self.variance.links.values() {
            census.vec_capacity("variance", flags, flags.capacity());
        }
        census.vec_capacity(
            "variance",
            &self.variance.stack,
            self.variance.stack.capacity(),
        );
        for (_, types) in &self.variance.stack {
            census.list("type_lists", types);
        }
        census.set("variance", &self.variance.markers);
        census.map("signature_caches", &self.signatures.instantiations);
        census.add(
            "signature_caches",
            0,
            self.signatures
                .instantiations
                .keys()
                .map(|(_, key)| key.len())
                .sum(),
        );
        let (known, escapes) = self.declarations.storage_bytes();
        census.add("declarations", self.declarations.iter().count(), known);
        if escapes != 0 {
            census.unavailable.push("declaration_escape_tree");
        }
        if let Some(program) = &self.program {
            program.census(census);
        }
        self.resolution.census(census);
        self.diagnostics.census(census);
        self.suggestions.census(census);
    }
}
