use crate::NodeRecord;
use crate::{counters::Track, FileId, StorageBuilder, StorageOwner};
use std::{ops::Deref, sync::Arc};

/// One retention root owns every file in a content-mapped parse result.
pub struct StorageBundle<N: NodeRecord, S = ()> {
    files: Vec<Arc<StorageOwner<N, S>>>,
    _owner: Track,
}

impl<N: NodeRecord, S> std::fmt::Debug for StorageBundle<N, S> {
    fn fmt(&self, output: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        output
            .debug_struct("StorageBundle")
            .field("files", &self.files)
            .finish_non_exhaustive()
    }
}

impl<N: NodeRecord, S> StorageBundle<N, S> {
    /// Consumes builders so a mapped member never escapes as a standalone owner.
    pub fn new(
        canonical: StorageBuilder<N, S>,
        supplemental: Vec<StorageBuilder<N, S>>,
    ) -> Arc<Self> {
        let counters = canonical.counters.clone();
        let canonical_id = canonical.id();
        let ids = supplemental.iter().map(StorageBuilder::id).collect();
        let mut files = vec![Arc::new(canonical.publish(None, ids))];
        files.extend(
            supplemental
                .into_iter()
                .map(|file| Arc::new(file.publish(Some(canonical_id), Vec::new()))),
        );
        Arc::new(Self {
            files,
            _owner: counters.owner(),
        })
    }

    pub fn len(&self) -> usize {
        self.files.len()
    }
    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    pub fn file(self: &Arc<Self>, index: usize) -> Option<StorageHandle<N, S>> {
        (index < self.files.len()).then(|| StorageHandle {
            root: Root::Bundle(self.clone(), index),
        })
    }
}

pub(crate) enum Root<N: NodeRecord, S> {
    File(Arc<StorageOwner<N, S>>),
    Bundle(Arc<StorageBundle<N, S>>, usize),
}

/// An owning file reference. Every mapped handle keeps all sibling files alive.
pub struct StorageHandle<N: NodeRecord, S = ()> {
    pub(crate) root: Root<N, S>,
}

impl<N: NodeRecord, S> std::fmt::Debug for StorageHandle<N, S> {
    fn fmt(&self, output: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        output
            .debug_struct("StorageHandle")
            .field("id", &self.id())
            .field("bundled", &matches!(self.root, Root::Bundle(..)))
            .finish()
    }
}

impl<N: NodeRecord, S> Clone for StorageHandle<N, S> {
    fn clone(&self) -> Self {
        Self {
            root: match &self.root {
                Root::File(file) => Root::File(file.clone()),
                Root::Bundle(bundle, index) => Root::Bundle(bundle.clone(), *index),
            },
        }
    }
}

impl<N: NodeRecord, S> Deref for StorageHandle<N, S> {
    type Target = StorageOwner<N, S>;
    fn deref(&self) -> &Self::Target {
        match &self.root {
            Root::File(file) => file,
            Root::Bundle(bundle, index) => &bundle.files[*index],
        }
    }
}

impl<N: NodeRecord, S> StorageHandle<N, S> {
    pub(crate) fn owner_retention(
        &self,
        arena: crate::ArenaId,
    ) -> Option<crate::StorageView<'_, N, S>> {
        let contains = |owner: &&StorageOwner<N, S>| {
            owner.core.id == arena
                || owner.lazy_arena() == arena
                || owner.auxiliary_arena() == arena
                || owner.lazy_auxiliary_arena() == arena
        };
        match &self.root {
            Root::File(owner) => {
                if contains(&&**owner) {
                    self.view().for_arena(arena).ok()
                } else {
                    owner.imported_view(arena)
                }
            }
            Root::Bundle(bundle, _) => {
                if bundle
                    .files
                    .iter()
                    .map(|owner| &**owner)
                    .any(|owner| contains(&owner))
                {
                    self.view().for_arena(arena).ok()
                } else {
                    bundle
                        .files
                        .iter()
                        .find_map(|owner| owner.imported_view(arena))
                }
            }
        }
    }
    pub fn view(&self) -> crate::StorageView<'_, N, S> {
        crate::StorageView::retained(self)
    }

    pub(crate) fn retained_owner(&self, arena: crate::ArenaId) -> Option<&StorageOwner<N, S>> {
        let contains = |owner: &&StorageOwner<N, S>| {
            owner.core.id == arena
                || owner.lazy_arena() == arena
                || owner.auxiliary_arena() == arena
                || owner.lazy_auxiliary_arena() == arena
        };
        match &self.root {
            Root::File(owner) => Some(&**owner)
                .filter(contains)
                .or_else(|| owner.imported_owner(arena)),
            Root::Bundle(bundle, _) => bundle
                .files
                .iter()
                .map(|file| &**file)
                .find(contains)
                .or_else(|| {
                    bundle
                        .files
                        .iter()
                        .find_map(|owner| owner.imported_owner(arena))
                }),
        }
    }
    /// Follow a mapper link through its complete retention group.
    pub fn file(&self, id: FileId) -> Option<Self> {
        match &self.root {
            Root::File(file) => (file.id() == id)
                .then(|| self.clone())
                .or_else(|| file.imported_file(id)),
            Root::Bundle(bundle, _) => bundle
                .files
                .iter()
                .position(|file| file.id() == id)
                .and_then(|index| bundle.file(index))
                .or_else(|| {
                    bundle
                        .files
                        .iter()
                        .find_map(|owner| owner.imported_file(id))
                }),
        }
    }

    pub(crate) fn into_members(self) -> Vec<Self> {
        match self.root {
            Root::File(file) => vec![Self {
                root: Root::File(file),
            }],
            Root::Bundle(bundle, _) => (0..bundle.len())
                .map(|index| Self {
                    root: Root::Bundle(bundle.clone(), index),
                })
                .collect(),
        }
    }
}

impl<N: NodeRecord, S> StorageHandle<N, S> {
    /// Structural storage of the owner(s) behind this handle; see
    /// [`StorageOwner::structural_bytes`].
    pub fn structural_bytes(&self, store: impl Fn(&N::Store) -> (usize, usize)) -> (usize, usize) {
        match &self.root {
            Root::File(owner) => owner.structural_bytes(&store),
            Root::Bundle(bundle, _) => {
                let mut known = bundle.files.capacity() * size_of::<Arc<StorageOwner<N, S>>>();
                let mut unmeasured = 0;
                for owner in &bundle.files {
                    let (k, u) = owner.structural_bytes(&store);
                    known += 16 + size_of::<StorageOwner<N, S>>() + k;
                    unmeasured += u;
                }
                (known, unmeasured)
            }
        }
    }
}
