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

/// The mapper cache owns a complete bound bundle under one key.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ContentMappedParseCacheKey {
    file_name: JsString,
    path: JsString,
    jsx: bool,
    force_module: bool,
    hash: u128,
}
impl ContentMappedParseCacheKey {
    pub fn from_options_hash(options: &SourceFileParseOptions, hash: u128) -> Self {
        Self {
            file_name: options.file_name.clone(),
            path: options.path.clone(),
            jsx: options.external_module_indicator_options.jsx,
            force_module: options.external_module_indicator_options.force,
            hash,
        }
    }
    // port: tsc/internal/project/parsecache.go:contentMappedParseCacheKeyForFile
    pub fn from_file(file: &ProgramFile) -> Result<Self, Error> {
        let source = file.bound().view().source_file()?;
        Ok(Self::from_options_hash(
            &source.content_mapper_parse_options(),
            (u128::from(source.hash.hi) << 64) | u128::from(source.hash.lo),
        ))
    }
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
        Self::from_options_hash(options, xxhash_rust::xxh3::xxh3_128(&bytes))
    }
}

pub use tsr_compiler::MappedProgramFiles as ContentMappedSourceFiles;
pub struct ContentMappedParseCache {
    entries: RefCountCache<ContentMappedParseCacheKey, Arc<ContentMappedSourceFiles>>,
}
impl ContentMappedParseCache {
    // port: tsc/internal/project/parsecache.go:NewContentMappedParseCache
    pub fn new(options: RefCountCacheOptions) -> Self {
        Self {
            entries: RefCountCache::new(options),
        }
    }
    pub fn len(&self) -> usize {
        self.entries.len()
    }
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
    pub fn reference_count(&self, key: &ContentMappedParseCacheKey) -> Option<isize> {
        self.entries.reference_count(key)
    }
    /// A single lease covers canonical and supplemental outputs. Construction
    /// failures publish nothing; dropping the last lease evicts the whole row.
    pub fn acquire_or_error<E>(
        self: &Arc<Self>,
        key: &ContentMappedParseCacheKey,
        produce: impl FnOnce() -> Result<ContentMappedSourceFiles, E>,
    ) -> Result<ContentMappedLease, E> {
        let files = self
            .entries
            .acquire_or_error(key, || produce().map(Arc::new))?;
        Ok(ContentMappedLease {
            cache: self.clone(),
            key: key.clone(),
            files,
        })
    }
    pub fn acquire_mapped(
        self: &Arc<Self>,
        request: &tsr_compiler::MappedSourceFileRequest<'_>,
        locale: &str,
    ) -> tsr_compiler::MappedFileResult<tsr_compiler::CachedMappedProgramFiles> {
        let identity = match request.project.identity(request.mapper_index) {
            Ok(identity) => identity,
            Err(error) => {
                return Ok(Err(tsr_contentmapper::transform_error(
                    tsr_contentmapper::TransformErrorKind::Project,
                    Some(error),
                )));
            }
        };
        let key = ContentMappedParseCacheKey::new(
            request.options,
            xxhash_rust::xxh3::xxh3_128(request.content),
            xxhash_rust::xxh3::xxh3_128(identity.as_bytes()),
            locale,
        );
        enum Failure {
            Compiler(Error),
            Mapper(tsr_contentmapper::Error),
        }
        let files = self.entries.acquire_or_error(&key, || {
            request
                .transform(Some(tsr_ast::SourceHash {
                    hi: (key.hash >> 64) as u64,
                    lo: key.hash as u64,
                }))
                .map_err(Failure::Compiler)?
                .map_err(Failure::Mapper)
        });
        match files {
            Ok(files) => Ok(Ok(tsr_compiler::CachedMappedProgramFiles {
                files: files.clone(),
                retention: Box::new(ContentMappedLease {
                    cache: self.clone(),
                    key,
                    files,
                }),
            })),
            Err(Failure::Compiler(error)) => Err(error),
            Err(Failure::Mapper(error)) => Ok(Err(error)),
        }
    }
    pub fn retain(
        self: &Arc<Self>,
        file: &Arc<ProgramFile>,
    ) -> Result<Box<dyn Send + Sync>, Error> {
        let source = file.bound().view().source_file()?;
        if source.is_content_mapper_supplemental() || source.is_content_mapper_failure_stub() {
            return Ok(Box::new(()));
        }
        let key = ContentMappedParseCacheKey::from_file(file)?;
        let files = self.entries.acquire_or_error(&key, || {
            Err(Error::Unsupported(
                "mapped program reuse requires a live bundle-cache entry",
            ))
        })?;
        let lease = ContentMappedLease {
            cache: self.clone(),
            key,
            files,
        };
        if lease.files.canonical.source() != file.source() {
            return Err(Error::Unsupported(
                "mapped program reuse across different bundle-cache identities",
            ));
        }
        Ok(Box::new(lease))
    }
}
pub struct ContentMappedLease {
    cache: Arc<ContentMappedParseCache>,
    key: ContentMappedParseCacheKey,
    files: Arc<ContentMappedSourceFiles>,
}
impl ContentMappedLease {
    pub fn files(&self) -> &Arc<ContentMappedSourceFiles> {
        &self.files
    }
}
impl Drop for ContentMappedLease {
    fn drop(&mut self) {
        self.cache.entries.release(&self.key);
    }
}

#[cfg(test)]
mod tests;
