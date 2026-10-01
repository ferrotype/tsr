//! A flat, shareable index of published dependencies. Builders copy the index
//! only when adding a dependency after sharing it; reads never clone owners.
use crate::{ArenaId, FileId, NodeRecord, StorageHandle, StorageOwner, StorageView};
use hashbrown::HashMap;
use std::sync::Arc;

// Importing requires shared payloads, but a builder without imports still
// supports Send-only payloads. Keep that restriction on the retained capability.
trait RetainedImport<N: NodeRecord, S>: Send + Sync {
    fn handle(&self) -> &StorageHandle<N, S>;
}
impl<N, S> RetainedImport<N, S> for StorageHandle<N, S>
where
    N: NodeRecord + Send + Sync,
    N::Aux: Send + Sync,
    N::CoreAux: Send + Sync,
    N::Store: Send + Sync,
    S: Send + Sync,
{
    fn handle(&self) -> &StorageHandle<N, S> {
        self
    }
}

struct ImportTable<N: NodeRecord, S> {
    files: Vec<Box<dyn RetainedImport<N, S>>>,
    arenas: HashMap<ArenaId, usize, crate::hash::FastState>,
}

/// Published files and their transitive dependencies, indexed once and shared
/// by any number of output builders. Each handle retains its complete mapped
/// bundle. Empty sets allocate nothing; extending a clone does not change the
/// original set or broaden an imported owner's own retention capability.
pub struct StorageImports<N: NodeRecord, S = ()>(Option<Arc<ImportTable<N, S>>>);

impl<N: NodeRecord, S> Default for StorageImports<N, S> {
    fn default() -> Self {
        Self(None)
    }
}
impl<N: NodeRecord, S> Clone for StorageImports<N, S> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}
impl<N: NodeRecord, S> std::fmt::Debug for StorageImports<N, S> {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.debug_struct("StorageImports")
            .field(
                "files",
                &self.0.as_ref().map_or(0, |table| table.files.len()),
            )
            .finish()
    }
}

impl<N: NodeRecord, S> StorageImports<N, S> {
    pub(crate) fn is_empty(&self) -> bool {
        self.0.is_none()
    }
    fn get(&self, arena: ArenaId) -> Option<&StorageHandle<N, S>> {
        let table = self.0.as_ref()?;
        Some(table.files[*table.arenas.get(&arena)?].handle())
    }
    pub(crate) fn owner(&self, arena: ArenaId) -> Option<&StorageOwner<N, S>> {
        self.get(arena).map(|handle| &**handle)
    }
    pub(crate) fn view(&self, arena: ArenaId) -> Option<StorageView<'_, N, S>> {
        self.get(arena)?.view().for_arena(arena).ok()
    }
    pub(crate) fn file(&self, id: FileId) -> Option<StorageHandle<N, S>> {
        self.get(id.arena()).cloned()
    }
    pub(crate) fn structural_bytes_with(
        &self,
        store: &impl Fn(&N::Store, &mut crate::StorageCensus) -> (usize, usize),
        census: &mut crate::StorageCensus,
    ) -> (usize, usize) {
        let Some(table) = &self.0 else { return (0, 0) };
        let mut known = census.allocation(
            Arc::as_ptr(table) as usize,
            crate::bundle::arc_bytes::<ImportTable<N, S>>()
                + table.files.capacity() * size_of::<Box<dyn RetainedImport<N, S>>>()
                + table.arenas.allocation_size()
                + table.files.len() * size_of::<StorageHandle<N, S>>(),
        );
        if known == 0 {
            return (0, 0);
        }
        let mut unmeasured = 0;
        for file in &table.files {
            let (bytes, unknown) = file.handle().structural_bytes_with(store, census);
            known += bytes;
            unmeasured += unknown;
        }
        (known, unmeasured)
    }
}

impl<N, S> Clone for ImportTable<N, S>
where
    N: NodeRecord + Send + Sync + 'static,
    N::Aux: Send + Sync + 'static,
    N::CoreAux: Send + Sync + 'static,
    N::Store: Send + Sync + 'static,
    S: Send + Sync + 'static,
{
    fn clone(&self) -> Self {
        Self {
            files: self
                .files
                .iter()
                .map(|file| Box::new(file.handle().clone()) as Box<dyn RetainedImport<N, S>>)
                .collect(),
            arenas: self.arenas.clone(),
        }
    }
}

impl<N, S> StorageImports<N, S>
where
    N: NodeRecord + Send + Sync + 'static,
    N::Aux: Send + Sync + 'static,
    N::CoreAux: Send + Sync + 'static,
    N::Store: Send + Sync + 'static,
    S: Send + Sync + 'static,
{
    pub fn new(files: impl IntoIterator<Item = StorageHandle<N, S>>) -> Self {
        let mut result = Self::default();
        for file in files {
            result.retain_file(file);
        }
        result
    }

    pub(crate) fn retain_file(&mut self, file: StorageHandle<N, S>) {
        if self.get(file.id().arena()).is_some() {
            return;
        }
        for member in file.into_members() {
            self.extend(&member.imports);
            self.insert(member);
        }
    }

    pub(crate) fn extend(&mut self, other: &Self) {
        let Some(other) = &other.0 else { return };
        let Some(own) = &self.0 else {
            self.0 = Some(other.clone());
            return;
        };
        if Arc::ptr_eq(own, other) {
            return;
        }
        for file in &other.files {
            self.insert(file.handle().clone());
        }
    }

    fn insert(&mut self, file: StorageHandle<N, S>) {
        if self.get(file.id().arena()).is_some() {
            return;
        }
        let table = Arc::make_mut(self.0.get_or_insert_with(|| {
            Arc::new(ImportTable {
                files: Vec::new(),
                arenas: HashMap::default(),
            })
        }));
        let index = table.files.len();
        for arena in [
            file.id().arena(),
            file.lazy_arena(),
            file.auxiliary_arena(),
            file.lazy_auxiliary_arena(),
        ] {
            table.arenas.insert(arena, index);
        }
        table.files.push(Box::new(file));
    }
}
