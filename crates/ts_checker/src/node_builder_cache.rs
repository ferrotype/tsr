//! Persistent diagnostic-builder caches with independently owned allocation frames.
//! A request builds exclusively, then publishes only frames containing reusable
//! entries. Cache hits import published storage before cloning its syntax.
use super::{class_emit::SymbolIdentity, NodeBuilder};
use crate::{object_flags as of, type_flags as tf, types::Map, CheckerState, Error, TypeId};
use ts_arena::{NodeId, SymbolId};
use ts_ast::{symbol_flags as sf, AstFile};
use ts_printer::EmitContext;

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct SerializedKey {
    pub enclosing: Option<NodeId>,
    pub ty: TypeId,
    pub flags: ts_nodebuilder::Flags,
    pub internal_flags: ts_nodebuilder::InternalFlags,
}
#[derive(Clone)]
pub(super) struct TrackedSymbol {
    pub symbol: SymbolId,
    pub enclosing: Option<NodeId>,
    pub meaning: ts_ast::SymbolFlags,
}
#[derive(Clone)]
pub(super) struct SerializedType {
    pub node: NodeId,
    pub added_length: usize,
    pub truncating: bool,
    pub symbols: Vec<TrackedSymbol>,
}
#[derive(Clone)]
struct CachedType {
    value: SerializedType,
    owner: AstFile,
}

#[derive(Default)]
pub(crate) struct CachedBuilder {
    emit: EmitContext,
    entries: Map<SerializedKey, CachedType>,
    identifiers: Map<NodeId, Option<SymbolId>>,
    /// Nested diagnostic display shares emit metadata. Sweep only after the
    /// last construction frame releases it, including normal-error exits.
    active: usize,
}

impl CachedBuilder {
    #[cfg(any(test, feature = "storage-pilot"))]
    pub(crate) fn type_roots(&self) -> impl Iterator<Item = TypeId> + '_ {
        self.entries.keys().map(|key| key.ty)
    }

    #[cfg(any(test, feature = "storage-pilot"))]
    pub(crate) fn census(&self, census: &mut crate::census::Census) {
        census.add(
            "display_cache",
            self.entries.len() + self.identifiers.len(),
            self.entries.allocation_size() + self.identifiers.allocation_size(),
        );
        let mut frames = std::collections::HashSet::new();
        for entry in self.entries.values() {
            census.add(
                "display_cache",
                0,
                entry.value.symbols.capacity() * size_of::<TrackedSymbol>(),
            );
            if frames.insert(entry.value.node.arena()) {
                census.add(
                    "display_ast",
                    entry.owner.view().file_info().node_count as usize,
                    0,
                );
            }
        }
        census.add("display_emit", self.emit.metadata_entries(), 0);
        // AST page capacity and std-map/comment backing accounting still need
        // a measurement adapter. These families must never appear as zero-cost.
    }
}

impl<'a> NodeBuilder<'a> {
    // port: tsc/internal/checker/nodebuilder.go:Checker.getNodeBuilder
    pub(crate) fn with_cached(
        checker: &'a mut CheckerState,
        flags: ts_nodebuilder::Flags,
        action: impl FnOnce(&mut Self) -> Result<ts_ast::JsString, Error>,
    ) -> Result<ts_ast::JsString, Error> {
        let emit = checker.display_builder.emit.clone();
        let mut builder = Self::with_emit(checker, flags, emit);
        builder.cached = true;
        builder.checker.display_builder.active += 1;
        let result = action(&mut builder);
        // A panic unwinds the exclusive operation and retires its owner; the
        // partially built frame and its unpublished cache entries are dropped.
        builder.release_cached_frame(result.is_ok())?;
        result
    }

    fn release_cached_frame(self, success: bool) -> Result<(), Error> {
        let Self {
            checker,
            ast,
            serialized,
            id_to_symbol,
            ..
        } = self;
        checker.display_builder.active -= 1;
        // Go never looks up the nil-enclosing cache. Avoid retaining entries
        // that cannot be reused, while preserving context-free display behavior.
        let pending: Vec<_> = serialized
            .into_iter()
            .filter(|(key, _)| success && key.enclosing.is_some())
            .collect();
        let publication = if let Some((_, first)) = pending.first() {
            ast.complete(first.node)
                .map(|parsed| {
                    let file = parsed.publish_unbound();
                    for (key, value) in pending {
                        checker.display_builder.entries.insert(
                            key,
                            CachedType {
                                value,
                                owner: file.clone(),
                            },
                        );
                    }
                    checker.display_builder.identifiers.extend(id_to_symbol);
                })
                .map_err(Error::from)
        } else {
            drop(ast);
            Ok(())
        };
        if checker.display_builder.active == 0 {
            let cache = &mut checker.display_builder;
            // An AstFile retains its source/import closure. This predicate covers
            // all cached frames and their dependencies, including metadata keys.
            // Resolve a metadata arena once per sweep, rather than probing every
            // cache entry for every metadata key. Still check each node's slot.
            let mut owners: Map<_, Option<&AstFile>> = cache
                .entries
                .values()
                .map(|entry| (entry.value.node.arena(), Some(&entry.owner)))
                .collect();
            let mut retained = |node: NodeId| {
                owners
                    .entry(node.arena())
                    .or_insert_with(|| {
                        cache
                            .entries
                            .values()
                            .find(|entry| entry.owner.view().node(node).is_ok())
                            .map(|entry| &entry.owner)
                    })
                    .is_some_and(|owner| owner.view().node(node).is_ok())
            };
            cache.emit.retain_metadata(&mut retained)?;
            cache.identifiers.retain(|&node, _| retained(node));
        }
        publication
    }

    pub(super) fn identifier_symbol(&self, node: NodeId) -> Option<&Option<SymbolId>> {
        self.id_to_symbol.get(&node).or_else(|| {
            self.cached
                .then(|| self.checker.display_builder.identifiers.get(&node))
                .flatten()
        })
    }

    fn cached_type_node(&mut self, key: SerializedKey) -> Result<Option<NodeId>, Error> {
        if key.enclosing.is_none() {
            return Ok(None);
        }
        let value = if let Some(value) = self.serialized.get(&key) {
            value.clone()
        } else if self.cached {
            let Some(entry) = self.checker.display_builder.entries.get(&key).cloned() else {
                return Ok(None);
            };
            self.ast.retain_file(entry.owner);
            entry.value
        } else {
            return Ok(None);
        };
        for tracked in &value.symbols {
            let enclosing = self.enclosing;
            self.enclosing = tracked.enclosing;
            let result = self.track_symbol(tracked.symbol, tracked.meaning);
            self.enclosing = enclosing;
            result?;
        }
        self.truncating |= value.truncating;
        self.approximate_length += value.added_length;
        Ok(ts_ast::deep_clone_node(&mut self.ast, Some(value.node)))
    }

    // port: tsc/internal/checker/nodebuilderimpl.go:NodeBuilderImpl.typeToTypeNodeOrCircularityElision
    pub(super) fn type_node_or_circularity_elision(&mut self, ty: TypeId) -> Result<NodeId, Error> {
        if self.checker.types.flags(ty)? & tf::UNION == 0 {
            return self.type_node(ty);
        }
        if self.visited.contains(&ty) {
            if self.flags & ts_nodebuilder::flags::ALLOW_ANONYMOUS_IDENTIFIER == 0 {
                self.encountered_error = true;
                self.report(ts_printer::emit_resolver::DeclarationTrackerEvent::CyclicStructure);
            }
            return Ok(self.elided_type());
        }
        self.visit_transform_type(ty, Self::type_node)
    }

    // port: tsc/internal/checker/nodebuilderimpl.go:NodeBuilderImpl.visitAndTransformType
    pub(super) fn visit_transform_type(
        &mut self,
        ty: TypeId,
        action: impl FnOnce(&mut Self, TypeId) -> Result<NodeId, Error>,
    ) -> Result<NodeId, Error> {
        let record = *self.checker.types.get(ty)?;
        let mut identity = if record.object_flags & of::REFERENCE != 0 {
            self.checker
                .types
                .type_reference(ty)?
                .node
                .map(SymbolIdentity::Node)
        } else if record.flags & tf::CONDITIONAL != 0 {
            let conditional = self.checker.types.conditional(ty)?;
            Some(SymbolIdentity::Node(
                self.checker.conditional_root(conditional.root)?.node,
            ))
        } else {
            None
        };
        if identity.is_none() {
            if let Some(symbol) = record.symbol {
                identity = Some(SymbolIdentity::Symbol {
                    constructor: record.object_flags & of::ANONYMOUS != 0
                        && self.checker.symbol(symbol)?.flags() & sf::CLASS != 0,
                    symbol,
                });
            }
        }
        let key = SerializedKey {
            enclosing: self.enclosing,
            ty,
            flags: self.flags,
            internal_flags: self.internal_flags,
        };
        if let Some(node) = self.cached_type_node(key)? {
            return Ok(node);
        }
        if let Some(identity) = identity {
            if self
                .symbol_depth
                .iter()
                .filter(|id| **id == identity)
                .count()
                > 10
            {
                return Ok(self.elided_type());
            }
            self.symbol_depth.push(identity);
        }
        self.visited.push(ty);
        let tracked = std::mem::take(&mut self.tracked_symbols);
        let start_length = self.approximate_length;
        let result = action(self, ty);
        let symbols = std::mem::replace(&mut self.tracked_symbols, tracked);
        if let Ok(node) = result {
            if !self.reported_diagnostic && !self.encountered_error {
                self.serialized.insert(
                    key,
                    SerializedType {
                        node,
                        added_length: self.approximate_length - start_length,
                        truncating: self.truncating,
                        symbols,
                    },
                );
            }
        }
        self.visited.pop();
        if identity.is_some() {
            self.symbol_depth.pop();
        }
        result
    }
}

#[cfg(test)]
#[path = "node_builder_cache_tests.rs"]
mod tests;
