//! Programs own cache leases separately from their immutable file handles.
//! Escaped files keep syntax alive without keeping a cache reference alive.
use crate::ref_count_cache::{RefCountCache, RefCountCacheOptions};
use std::sync::Arc;
use tsr_arena::Counters;
use tsr_ast::SourceFileParseOptions;
use tsr_checker::TraceSink;
use tsr_compiler::{CachedProgramFile, Error, ProgramFile, SourceFileCache};
use tsr_core::ScriptKind;
use tsr_jsstring::{JsString, SourceText};

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ParseCacheKey {
    file_name: JsString,
    path: JsString,
    jsx: bool,
    force_module: bool,
    kind: ScriptKind,
    hash: u128,
}
impl ParseCacheKey {
    // port: tsc/internal/project/parsecache.go:NewParseCacheKey
    pub fn new(options: &SourceFileParseOptions, hash: u128, kind: ScriptKind) -> Self {
        let kind = if kind == ScriptKind::UNKNOWN {
            ScriptKind::ensure_from_file_name(options.file_name.as_bytes())
        } else {
            kind
        };
        Self {
            file_name: options.file_name.clone(),
            path: options.path.clone(),
            jsx: options.external_module_indicator_options.jsx,
            force_module: options.external_module_indicator_options.force,
            kind,
            hash,
        }
    }
}
#[derive(Clone)]
pub struct ParseCache {
    entries: Arc<RefCountCache<ParseCacheKey, Arc<ProgramFile>>>,
}
impl ParseCache {
    // port: tsc/internal/project/parsecache.go:NewParseCache
    pub fn new(options: RefCountCacheOptions) -> Self {
        Self {
            entries: Arc::new(RefCountCache::new(options)),
        }
    }
    pub fn len(&self) -> usize {
        self.entries.len()
    }
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
    pub fn reference_count(&self, key: &ParseCacheKey) -> Option<isize> {
        self.entries.reference_count(key)
    }
}
struct Lease {
    cache: ParseCache,
    key: ParseCacheKey,
}
impl Drop for Lease {
    fn drop(&mut self) {
        self.cache.entries.release(&self.key);
    }
}
impl SourceFileCache for ParseCache {
    fn retain(&self, file: &Arc<ProgramFile>) -> Result<Box<dyn Send + Sync>, Error> {
        let state = file.bound().view().source_file()?;
        if !state.content_mapper().is_empty() {
            return Err(Error::Unsupported(
                "mapped project reuse requires a bundle-cache lease",
            ));
        }
        let hash = (u128::from(state.hash.hi) << 64) | u128::from(state.hash.lo);
        // A file first built outside a project has no stored project hash.
        let hash = if hash == 0 {
            xxhash_rust::xxh3::xxh3_128(state.text().as_bytes())
        } else {
            hash
        };
        let key = ParseCacheKey::new(state.parse_options(), hash, state.script_kind);
        let cached = self.entries.acquire(&key, || file.clone());
        let lease = Box::new(Lease {
            cache: self.clone(),
            key,
        });
        if cached.source() != file.source() {
            return Err(Error::Unsupported(
                "program reuse across different live parse cache identities",
            ));
        }
        Ok(lease)
    }
    fn acquire(
        &self,
        source: SourceText,
        kind: ScriptKind,
        options: SourceFileParseOptions,
        counters: &Counters,
        tracing: Option<&Arc<dyn TraceSink>>,
    ) -> Result<CachedProgramFile, Error> {
        let key = ParseCacheKey::new(
            &options,
            xxhash_rust::xxh3::xxh3_128(source.as_bytes()),
            kind,
        );
        let value = self.entries.acquire_or_error(&key, || {
            ProgramFile::parse_and_bind(
                source,
                key.kind,
                options,
                counters,
                tracing,
                Some(tsr_ast::SourceHash {
                    hi: (key.hash >> 64) as u64,
                    lo: key.hash as u64,
                }),
            )
        })?;
        Ok(CachedProgramFile {
            file: value,
            retention: Box::new(Lease {
                cache: self.clone(),
                key,
            }),
        })
    }
}

/// The mapper cache owns a complete bundle under one key. Its production
/// mapper caller arrives in L6; the ordinary parse cache never admits one.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ContentMappedParseCacheKey {
    file_name: JsString,
    path: JsString,
    jsx: bool,
    force_module: bool,
    hash: u128,
}
impl ContentMappedParseCacheKey {
    // port: tsc/internal/project/parsecache.go:contentMappedParseCacheKey
    pub fn new(
        options: &SourceFileParseOptions,
        raw_hash: u128,
        transform_identity: u128,
        locale: &str,
    ) -> Self {
        let mut bytes = Vec::with_capacity(32 + locale.len());
        for value in [raw_hash, transform_identity] {
            bytes.extend_from_slice(&((value >> 64) as u64).to_le_bytes());
            bytes.extend_from_slice(&(value as u64).to_le_bytes());
        }
        bytes.extend_from_slice(locale.as_bytes());
        Self {
            file_name: options.file_name.clone(),
            path: options.path.clone(),
            jsx: options.external_module_indicator_options.jsx,
            force_module: options.external_module_indicator_options.force,
            hash: xxhash_rust::xxh3::xxh3_128(&bytes),
        }
    }
}

#[cfg(test)]
mod tests;
