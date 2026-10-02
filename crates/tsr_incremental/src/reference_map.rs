//! The files each file references and, computed once on first use, the files
//! that reference each file (`referencemap.go`).
use crate::snapshot::{lock, Path};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex, OnceLock};

/// A file's referenced files.
pub type ReferenceSet = Arc<BTreeSet<Path>>;

#[derive(Debug, Default)]
pub struct ReferenceMap {
    references: Mutex<BTreeMap<Path, ReferenceSet>>,
    referenced_by: OnceLock<BTreeMap<Path, BTreeSet<Path>>>,
}

impl ReferenceMap {
    // port: tsc/internal/execute/incremental/referencemap.go:referenceMap.storeReferences
    pub(crate) fn store_references(&self, path: &Path, refs: ReferenceSet) {
        lock(&self.references).insert(path.clone(), refs);
    }

    // port: tsc/internal/execute/incremental/referencemap.go:referenceMap.getReferences
    pub(crate) fn get_references(&self, path: &Path) -> Option<ReferenceSet> {
        lock(&self.references).get(path).cloned()
    }

    // port: tsc/internal/execute/incremental/referencemap.go:referenceMap.getPathsWithReferences
    pub(crate) fn get_paths_with_references(&self) -> Vec<Path> {
        lock(&self.references).keys().cloned().collect()
    }

    /// The files that reference `path`, in path order (the pin's map order).
    // port: tsc/internal/execute/incremental/referencemap.go:referenceMap.getReferencedBy
    pub(crate) fn get_referenced_by(&self, path: &Path) -> Vec<Path> {
        let referenced_by = self.referenced_by.get_or_init(|| {
            let mut referenced_by: BTreeMap<Path, BTreeSet<Path>> = BTreeMap::new();
            for (key, value) in lock(&self.references).iter() {
                for reference in value.iter() {
                    referenced_by
                        .entry(reference.clone())
                        .or_default()
                        .insert(key.clone());
                }
            }
            referenced_by
        });
        referenced_by
            .get(path)
            .map(|refs| refs.iter().cloned().collect())
            .unwrap_or_default()
    }
}
