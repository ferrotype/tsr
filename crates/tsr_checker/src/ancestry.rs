//! Bounded memoization of ancestry in the checker's immutable program inputs.
//! Factory nodes stay uncached: their kind and parent can still change.

use crate::{CheckerState, Error};
use tsr_arena::NodeId;
use tsr_ast::{node_flags as nf, AstView, SyntaxKind as K};

const CACHE_ENTRIES: usize = 1024;

/// Full identities are tags, so a collision only discards a previous answer.
/// Each table occupies at most 16 KiB; no AST-sized side table is allocated.
struct DirectCache<T> {
    entries: Box<[(Option<NodeId>, T)]>,
}

impl<T> Default for DirectCache<T> {
    fn default() -> Self {
        Self {
            entries: Box::default(),
        }
    }
}

impl<T: Copy + Default> DirectCache<T> {
    fn index(node: NodeId) -> usize {
        (node.slot() as usize ^ node.arena().get() as usize) & (CACHE_ENTRIES - 1)
    }

    fn get(&self, node: NodeId) -> Option<T> {
        let &(key, value) = self.entries.get(Self::index(node))?;
        (key == Some(node)).then_some(value)
    }

    fn insert_path(&mut self, path: &[NodeId], value: T) {
        if path.is_empty() {
            return;
        }
        if self.entries.is_empty() {
            self.entries = vec![(None, T::default()); CACHE_ENTRIES].into_boxed_slice();
        }
        // Keep the nodes nearest the query when a long path collides with itself.
        for &node in path.iter().rev() {
            self.entries[Self::index(node)] = (Some(node), value);
        }
    }
}

#[derive(Default)]
pub(crate) struct AncestryCache {
    type_nodes: DirectCache<bool>,
    containers: DirectCache<Option<NodeId>>,
    /// Reused, bounded scratch storage; failed walks publish no entries.
    path: Vec<NodeId>,
}

impl AncestryCache {
    #[cfg(any(test, feature = "storage-pilot"))]
    pub(crate) fn census(&self, census: &mut crate::census::Census) {
        // These index immutable program syntax, like the owner's arena routing
        // tables. Keep them in the existing non-type family shared with Go.
        census.add(
            "program_indices",
            self.type_nodes
                .entries
                .iter()
                .filter(|entry| entry.0.is_some())
                .count()
                + self
                    .containers
                    .entries
                    .iter()
                    .filter(|entry| entry.0.is_some())
                    .count(),
            std::mem::size_of_val(self.type_nodes.entries.as_ref())
                + std::mem::size_of_val(self.containers.entries.as_ref())
                + self.path.capacity() * size_of::<NodeId>(),
        );
    }

    fn remember(&mut self, node: NodeId, enabled: bool) {
        if enabled && self.path.len() < CACHE_ENTRIES {
            self.path.push(node);
        }
    }

    pub(crate) fn in_ambient_or_type_node(
        &mut self,
        view: AstView<'_>,
        node: NodeId,
        enabled: bool,
    ) -> Result<bool, Error> {
        let first = view.node(node)?;
        // Ambient is a property of the original node, not an ancestor predicate.
        // It must never enter the type-ancestry cache or spread to descendants.
        if first.flags() & nf::AMBIENT != 0 {
            return Ok(true);
        }
        let mut same_owner = enabled;
        let result = (|| {
            let mut current = Some(node);
            let mut first = Some(first);
            while let Some(id) = current {
                same_owner &= id.arena() == node.arena();
                if same_owner {
                    if let Some(result) = self.type_nodes.get(id) {
                        return Ok(result);
                    }
                }
                let read = match first.take() {
                    Some(read) => read,
                    None => view.node(id)?,
                };
                self.remember(id, same_owner);
                if matches!(
                    read.kind().known(),
                    Some(
                        K::InterfaceDeclaration
                            | K::TypeAliasDeclaration
                            | K::JSTypeAliasDeclaration
                            | K::TypeLiteral
                    )
                ) {
                    return Ok(true);
                }
                current = read.parent();
            }
            Ok(false)
        })();
        if same_owner {
            if let Ok(value) = result {
                self.type_nodes.insert_path(&self.path, value);
            }
        }
        self.path.clear();
        result
    }

    pub(crate) fn control_flow_container(
        &mut self,
        view: AstView<'_>,
        node: NodeId,
        enabled: bool,
    ) -> Result<Option<NodeId>, Error> {
        if enabled {
            if let Some(result) = self.containers.get(node) {
                return Ok(result);
            }
        }
        let mut same_owner = enabled;
        let result = (|| {
            let mut current = view.node(node)?.parent();
            self.remember(node, same_owner);
            while let Some(id) = current {
                same_owner &= id.arena() == node.arena();
                let read = view.node(id)?;
                if matches!(
                    read.kind().known(),
                    Some(K::ModuleBlock | K::SourceFile | K::PropertyDeclaration)
                ) || tsr_ast::utilities::is_function_like(Some(&read))
                    && tsr_ast::get_immediately_invoked_function_expression(view, id)?.is_none()
                {
                    // The answer excludes the starting node. The terminal
                    // container itself therefore cannot receive this entry.
                    return Ok(Some(id));
                }
                if same_owner {
                    if let Some(result) = self.containers.get(id) {
                        return Ok(result);
                    }
                }
                self.remember(id, same_owner);
                current = read.parent();
            }
            Ok(None)
        })();
        if same_owner {
            if let Ok(value) = result {
                self.containers.insert_path(&self.path, value);
            }
        }
        self.path.clear();
        result
    }
}

impl CheckerState {
    /// The program is assigned once and retains immutable completed files for
    /// this checker's lifetime. Only its indexed core arenas may seed a cache;
    /// factories, lazy namespaces and imported fallback owners keep checked walks.
    pub(crate) fn ancestry_view(
        &mut self,
        node: NodeId,
    ) -> Result<(AstView<'_>, &mut AncestryCache, bool), Error> {
        if self.factory.id().arena() == node.arena() {
            let view = self.factory.view();
            view.node(node)?;
            return Ok((view, &mut self.ancestry, false));
        }
        let program = self
            .program
            .as_ref()
            .ok_or(Error::Unsupported("query without a checker program"))?;
        let enabled = program.core_file_index(node).is_some();
        Ok((program.ast(node)?, &mut self.ancestry, enabled))
    }
}

#[cfg(test)]
mod tests;
