//! Reachability through occupied records, without running a lazy initializer.
//! Tables of arena records are storage, not graph roots. Named builtins, caches,
//! source links and explicitly retained results supply the roots.

use crate::{
    CheckerState, ConditionalRootId, Error, InferenceId, MapperId, SignatureId, TypeId, TypeKind,
};
use std::collections::HashSet;

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Edge {
    Type(TypeId),
    Signature(SignatureId),
    Mapper(MapperId),
    Inference(InferenceId),
    Conditional(ConditionalRootId),
}

#[derive(Default)]
struct Work {
    pending: Vec<Edge>,
    seen: HashSet<Edge>,
    types: usize,
}
impl Work {
    fn types(&mut self, values: impl IntoIterator<Item = TypeId>) {
        self.pending.extend(values.into_iter().map(Edge::Type));
    }
    fn signatures(&mut self, values: impl IntoIterator<Item = SignatureId>) {
        self.pending.extend(values.into_iter().map(Edge::Signature));
    }
    fn mappers(&mut self, values: impl IntoIterator<Item = MapperId>) {
        self.pending.extend(values.into_iter().map(Edge::Mapper));
    }
    fn structured(
        &mut self,
        data: &crate::StructuredMembers,
        checker: &CheckerState,
    ) -> Result<(), Error> {
        self.types(data.resolved_base_constraint);
        self.types(data.object_type_without_abstract_construct_signatures);
        self.signatures(data.signatures.iter().flat_map(|ids| ids.iter().copied()));
        for &id in data.index_infos.iter().flat_map(|ids| ids.iter()) {
            let info = checker.signatures.index_info(id)?;
            self.types([info.key_type, info.value_type]);
        }
        Ok(())
    }
}

impl CheckerState {
    pub(crate) fn reachable_types(&self, roots: &[TypeId]) -> Result<usize, Error> {
        let mut work = Work::default();
        for name in crate::BUILTIN_TYPE_NAMES {
            work.types(self.builtins.type_by_name(name));
        }
        work.signatures([
            self.builtins.any_signature,
            self.builtins.unknown_signature,
            self.builtins.resolving_signature,
            self.builtins.silent_never_signature,
        ]);
        work.types(roots.iter().copied());
        work.types(self.display_builder.type_roots());
        work.types(self.flow.type_roots());
        work.types(self.module_aliases.type_roots());
        work.types(self.iteration.type_roots());
        work.signatures(self.flow.signature_roots());
        work.types(self.calls.census_types());
        work.signatures(self.calls.census_signatures());
        for context in &self.calls.contexts {
            work.pending.extend(context.inference.map(Edge::Inference));
        }
        for (_, inference) in &self.calls.inference_contexts {
            work.pending.extend(inference.map(Edge::Inference));
        }
        work.types(self.enums.string_literals.values().copied());
        work.types(self.enums.number_literals.values().copied());
        work.types(self.enums.nan_literals.values().copied());
        for check in &self.deferred_checks.pending {
            match check {
                crate::deferred_checks::DeferredCheck::MissingProperty { containing, .. } => {
                    work.types([*containing]);
                }
                crate::deferred_checks::DeferredCheck::Iteration { .. }
                | crate::deferred_checks::DeferredCheck::WeakMapSetCollision { .. }
                | crate::deferred_checks::DeferredCheck::ReflectCollision { .. } => {}
            }
        }
        let caches = &self.types.caches;
        for map in [
            &caches.union_types,
            &caches.tuple_types,
            &caches.intersection_types,
            &caches.template_literal_types,
            &caches.indexed_access_types,
        ] {
            work.types(map.values().copied());
        }
        work.types(caches.string_literal_types.values().copied());
        work.types(caches.number_literal_types.values().copied());
        work.types(caches.nan_type);
        work.types(caches.bigint_literal_types.values().copied());
        work.types(caches.union_of_union_types.values().copied());
        for types in caches.subtype_reductions.values() {
            work.types(types.iter().copied());
        }
        for (&(ty, _), &value) in &caches.index_types {
            work.types([ty, value]);
        }
        for (&(ty, _), &value) in &caches.simplified_types {
            work.types([ty, value]);
        }
        for map in [
            &caches.equivalent_bases,
            &caches.literal_union_bases,
            &caches.regular_object_literals,
        ] {
            for (&key, &value) in map {
                work.types([key, value]);
            }
        }
        for (&(base, constraint), &value) in &caches.substitution_types {
            work.types([base, constraint, value]);
        }
        for (&(_, target), &value) in &caches.string_mapping_types {
            work.types([target, value]);
        }
        for (&(ty, _, _, _), &value) in &caches.property_types {
            work.types([ty, value]);
        }

        work.types(
            self.query
                .declared_types
                .values()
                .chain(self.query.type_nodes.values())
                .copied()
                .flatten(),
        );
        work.types(self.query.global_types.values().copied());
        work.types(self.query.context_free_types.values().copied());
        work.types(self.query.array_literal_types.keys().copied());
        work.types(self.query.array_literal_types.values().copied());
        work.types(self.query.widened_types.keys().copied());
        work.types(self.query.widened_types.values().copied());
        work.types(self.query.assertion_types.values().copied());
        work.types(self.query.error_types.values().copied());
        // Undefined-property value links are roots in the common symbol-link walk below.
        work.types(self.bindings.pattern_for_type.keys().copied());
        for (&(_, ty), &result) in &self.bindings.discriminated_contexts {
            work.types([ty, result]);
        }
        for cache in [&self.promises.promised, &self.promises.awaited] {
            for (&source, &result) in cache {
                work.types([source, result]);
            }
        }
        work.types(self.promises.stack.iter().copied());
        work.types(self.query.index_constraints_checked.iter().copied());
        for links in self.query.type_aliases.values() {
            work.types(
                links
                    .parameters
                    .iter()
                    .flat_map(|types| types.iter().copied()),
            );
            work.types(links.instantiations.values().copied());
        }
        for types in self
            .query
            .outer_type_parameters
            .values()
            .chain(self.query.deferred_property_types.values())
            .chain(self.query.deferred_property_write_types.values())
            .flatten()
        {
            work.types(types.iter().copied());
        }
        work.signatures(self.query.source_signatures.values().copied().flatten());
        for (&ty, &apparent) in &self.query.apparent_types {
            work.types([ty, apparent]);
        }
        for links in self.value_symbol_links.values() {
            work.types(links.resolved_type);
            work.types(links.write_type);
            work.types(links.name_type);
            work.types(links.containing_type);
            work.mappers(links.mapper);
        }
        for links in self.mapped_symbol_links.values() {
            work.types(links.key_type);
        }
        work.types(self.late_members.unique_types.values().copied());
        work.types(self.synthetic_expression_types.values().copied());
        for ((root, _), &ty) in &self.conditional.instantiations {
            work.pending.push(Edge::Conditional(*root));
            work.types([ty]);
        }
        for map in [
            &self.conditional.permissive,
            &self.conditional.restrictive,
            &self.conditional.restrictive_parameters,
        ] {
            for (&key, &value) in map {
                work.types([key, value]);
            }
        }
        for ((signature, _), &instantiated) in &self.signatures.instantiations {
            work.signatures([*signature, instantiated]);
        }
        for (_, types) in &self.variance.stack {
            work.types(types.iter().copied());
        }
        work.types(self.variance.markers.iter().copied());
        work.types(self.variance.checked_parameter);
        let mut active = Vec::new();
        self.inference.reverse.census_type_roots(&mut active);
        self.relations.census_type_roots(&mut active);
        work.types(active);
        work.mappers(self.instantiation.census_active_mappers());
        work.types(self.instantiation.census_active_types());
        for entity in self.resolution.census_entities() {
            match entity {
                crate::TypeSystemEntity::Type(ty) => work.types([ty]),
                crate::TypeSystemEntity::Signature(signature) => work.signatures([signature]),
                crate::TypeSystemEntity::Symbol(_) | crate::TypeSystemEntity::Node(_) => {}
            }
        }

        while let Some(edge) = work.pending.pop() {
            if !work.seen.insert(edge) {
                continue;
            }
            match edge {
                Edge::Type(ty) => {
                    self.census_type_edges(ty, &mut work)?;
                    work.types += 1;
                }
                Edge::Signature(id) => {
                    let signature = self.signatures.get(id)?;
                    work.types(signature.resolved_return_type);
                    work.types(signature.isolated_signature_type);
                    work.types(
                        signature
                            .type_parameters
                            .iter()
                            .flat_map(|types| types.iter().copied()),
                    );
                    work.signatures(signature.target);
                    work.signatures(signature.erased);
                    work.signatures(signature.base);
                    if let Some(composite) = &signature.composite {
                        work.signatures(composite.signatures.iter().copied());
                    }
                    if let Some(predicate) = signature.resolved_type_predicate {
                        work.types(self.signatures.predicate(predicate)?.t);
                    }
                    work.mappers(signature.mapper);
                }
                Edge::Mapper(id) => {
                    use crate::mapper::Mapper;
                    match self.mapper(id)? {
                        Mapper::Simple { source, target } => work.types([*source, *target]),
                        Mapper::Array { sources, targets } => {
                            work.types(sources.iter().copied());
                            work.types(targets.iter().copied());
                        }
                        Mapper::DeferredArguments { sources, .. } => {
                            work.types(sources.iter().copied());
                        }
                        Mapper::Merged { first, second } | Mapper::Composite { first, second } => {
                            work.mappers([*first, *second]);
                        }
                        Mapper::Inference { context, .. } => {
                            work.pending.push(Edge::Inference(*context));
                        }
                        Mapper::UniqueLiteral
                        | Mapper::Permissive
                        | Mapper::Restrictive
                        | Mapper::ReportReliability { .. } => {}
                    }
                }
                Edge::Inference(id) => {
                    let context = self.inference_context(id)?;
                    work.signatures(context.signature);
                    work.mappers([context.mapper, context.non_fixing_mapper]);
                    work.mappers(context.return_mapper);
                    work.mappers(context.outer_return_mapper);
                    work.types(context.inferred_type_parameters.iter().copied());
                    work.types(context.intra_expression_sites.iter().map(|(_, ty)| *ty));
                    for info in &context.inferences {
                        work.types([info.parameter]);
                        work.types(info.inferred);
                        work.types(info.candidates.iter().copied());
                        work.types(info.contra_candidates.iter().copied());
                    }
                }
                Edge::Conditional(id) => {
                    let root = self.conditional_root(id)?;
                    work.types([root.check_type, root.extends_type]);
                    work.types(root.infer_parameters.iter().copied());
                    work.types(root.outer_parameters.iter().copied());
                    if let Some(alias) = root.alias {
                        work.types(self.types.alias(alias)?.type_arguments.iter().copied());
                    }
                }
            }
        }
        Ok(work.types)
    }

    fn census_type_edges(&self, ty: TypeId, work: &mut Work) -> Result<(), Error> {
        let record = self.types.get(ty)?;
        if let Some(alias) = record.alias {
            work.types(self.types.alias(alias)?.type_arguments.iter().copied());
        }
        match record.kind {
            TypeKind::Anonymous
            | TypeKind::EvolvingArray
            | TypeKind::Reference
            | TypeKind::Interface
            | TypeKind::Tuple
            | TypeKind::Mapped
            | TypeKind::ReverseMapped
            | TypeKind::InstantiationExpression => {
                let object = self.types.object(ty)?;
                if record.kind == TypeKind::EvolvingArray {
                    let evolving = self.types.evolving_array(ty)?;
                    work.types([evolving.element_type]);
                    work.types(evolving.final_array_type);
                }
                work.structured(&object.structured, self)?;
                work.types(object.target);
                work.mappers(object.mapper);
                if let Some(map) = &object.instantiations {
                    work.types(map.values().copied());
                }
                if matches!(
                    record.kind,
                    TypeKind::Reference | TypeKind::Interface | TypeKind::Tuple
                ) {
                    work.types(
                        self.types
                            .type_reference(ty)?
                            .resolved_type_arguments
                            .iter()
                            .flat_map(|types| types.iter().copied()),
                    );
                }
                if matches!(record.kind, TypeKind::Interface | TypeKind::Tuple) {
                    let interface = self.types.interface(ty)?;
                    work.types(
                        interface
                            .all_type_parameters
                            .iter()
                            .flat_map(|types| types.iter().copied()),
                    );
                    work.types(
                        interface
                            .resolved_base_types
                            .iter()
                            .flat_map(|types| types.iter().copied()),
                    );
                    work.types(interface.this_type);
                    work.types(interface.resolved_base_constructor_type);
                    work.signatures(
                        interface
                            .declared_call_signatures
                            .iter()
                            .chain(interface.declared_construct_signatures.iter())
                            .flat_map(|signatures| signatures.iter().copied()),
                    );
                    for &index in interface
                        .declared_index_infos
                        .iter()
                        .flat_map(|indexes| indexes.iter())
                    {
                        let info = self.signatures.index_info(index)?;
                        work.types([info.key_type, info.value_type]);
                    }
                }
                if record.kind == TypeKind::Mapped {
                    let data = self.types.mapped(ty)?;
                    for id in [
                        data.type_parameter,
                        data.constraint_type,
                        data.name_type,
                        data.template_type,
                        data.modifiers_type,
                        data.resolved_apparent_type,
                    ] {
                        work.types(id);
                    }
                }
                if record.kind == TypeKind::ReverseMapped {
                    let data = self.types.reverse_mapped(ty)?;
                    for id in [data.source, data.mapped_type, data.constraint_type] {
                        work.types(id);
                    }
                }
            }
            TypeKind::Literal => {
                let data = self.types.literal(ty)?;
                work.types(data.fresh);
                work.types([data.regular]);
            }
            TypeKind::Union => {
                let data = self.types.union(ty)?;
                work.structured(&data.common.structured, self)?;
                work.types(data.types.iter().copied());
                work.types(data.origin);
                work.types(data.regular_type);
                work.types(data.resolved_reduced_type);
                if let Some(map) = &data.constituent_map {
                    for (&key, &value) in map.iter() {
                        work.types([key, value]);
                    }
                }
            }
            TypeKind::Intersection => {
                let data = self.types.intersection(ty)?;
                work.structured(&data.common.structured, self)?;
                work.types(data.types.iter().copied());
                work.types(data.resolved_apparent_type);
                work.types(data.unique_literal_filled_instantiation);
            }
            TypeKind::TypeParameter => {
                let data = self.types.type_parameter(ty)?;
                work.types(data.constraint);
                work.types(data.target);
                work.types(data.resolved_default_type);
                work.types(data.resolved_base_constraint);
                work.mappers(data.mapper);
            }
            TypeKind::TemplateLiteral => {
                let data = self.types.template_literal(ty)?;
                work.types(data.types.iter().copied());
                work.types(data.resolved_base_constraint);
            }
            TypeKind::Index => {
                let data = self.types.index_type(ty)?;
                work.types([data.target]);
                work.types(data.resolved_base_constraint);
            }
            TypeKind::IndexedAccess => {
                let data = self.types.indexed_access(ty)?;
                work.types([data.object_type, data.index_type]);
                work.types(data.resolved_base_constraint);
            }
            TypeKind::StringMapping => {
                let data = self.types.string_mapping(ty)?;
                work.types([data.target]);
                work.types(data.resolved_base_constraint);
            }
            TypeKind::Substitution => {
                let data = self.types.substitution(ty)?;
                work.types([data.base, data.constraint]);
                work.types(data.resolved_base_constraint);
            }
            TypeKind::Conditional => {
                let data = self.types.conditional(ty)?;
                work.pending.push(Edge::Conditional(data.root));
                work.types([data.check_type, data.extends_type]);
                work.mappers(data.mapper);
                work.mappers(data.combined_mapper);
                for id in [
                    data.resolved_base_constraint,
                    data.true_type,
                    data.false_type,
                    data.inferred_true_type,
                    data.default_constraint,
                    data.distributive_constraint,
                ] {
                    work.types(id);
                }
            }
            TypeKind::Intrinsic | TypeKind::UniqueEsSymbol => {}
        }
        Ok(())
    }
}
