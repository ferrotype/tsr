//! Canonical package.json observations shared by explicitly related resolvers.
use crate::PackageJson;
use std::{
    collections::BTreeMap,
    sync::{Arc, RwLock},
};
use tsr_jsstring::JsString;
use tsr_tspath as path;

#[derive(Clone, Debug)]
pub struct InfoCacheEntry {
    pub package_directory: JsString,
    pub directory_exists: bool,
    pub contents: Option<Arc<PackageJson>>,
}
impl InfoCacheEntry {
    /// port: tsc/internal/packagejson/cache.go:InfoCacheEntry.Exists
    pub fn exists(&self) -> bool {
        self.contents.is_some()
    }
    /// Retain source contents and selection state while adapting the directory
    /// spelling. An unchanged directory returns the same entry identity.
    /// port: tsc/internal/packagejson/cache.go:InfoCacheEntry.WithPackageDirectory
    pub fn with_package_directory(self: &Arc<Self>, directory: &[u8]) -> Arc<Self> {
        if self.package_directory.as_bytes() == directory {
            return self.clone();
        }
        Arc::new(Self {
            package_directory: JsString::from_bytes(directory),
            directory_exists: self.directory_exists,
            contents: self.contents.clone(),
        })
    }
}

/// First-writer-wins, including negative entries. This cache does not observe
/// filesystem invalidation: a caller starts a new cache for a new snapshot or
/// deliberately retains stale observations for the lifetime of a live-host run.
#[derive(Debug)]
pub struct InfoCache {
    current_directory: JsString,
    case_sensitive: bool,
    entries: RwLock<BTreeMap<JsString, Arc<InfoCacheEntry>>>,
}
impl InfoCache {
    /// port: tsc/internal/packagejson/cache.go:NewInfoCache
    pub fn new(current_directory: &[u8], case_sensitive: bool) -> Self {
        Self {
            current_directory: JsString::from_bytes(current_directory),
            case_sensitive,
            entries: RwLock::new(BTreeMap::new()),
        }
    }
    fn key(&self, path: &[u8]) -> JsString {
        path::to_path(path, self.current_directory.as_bytes(), self.case_sensitive)
    }
    /// port: tsc/internal/packagejson/cache.go:InfoCache.Get
    pub fn get(&self, path: &[u8]) -> Option<Arc<InfoCacheEntry>> {
        self.entries
            .read()
            .expect("package cache poisoned")
            .get(&self.key(path))
            .cloned()
    }
    /// Returns the winner, which can be an earlier entry including a miss.
    /// port: tsc/internal/packagejson/cache.go:InfoCache.Set
    pub fn set(&self, path: &[u8], entry: Arc<InfoCacheEntry>) -> Arc<InfoCacheEntry> {
        self.entries
            .write()
            .expect("package cache poisoned")
            .entry(self.key(path))
            .or_insert(entry)
            .clone()
    }
    /// Iteration order is unspecified. Callbacks run without the cache lock,
    /// so they may read or add entries without deadlocking. As with SyncMap,
    /// concurrent insertion need not be reflected in this traversal.
    /// port: tsc/internal/packagejson/cache.go:InfoCache.Range
    pub fn range(&self, mut visit: impl FnMut(&JsString, &Arc<InfoCacheEntry>) -> bool) {
        let entries: Vec<_> = self
            .entries
            .read()
            .expect("package cache poisoned")
            .iter()
            .map(|(key, entry)| (key.clone(), entry.clone()))
            .collect();
        for (key, entry) in &entries {
            if !visit(key, entry) {
                break;
            }
        }
    }
}
