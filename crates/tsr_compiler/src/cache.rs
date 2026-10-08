use crate::Error;
use std::{
    collections::{BTreeMap, HashMap},
    sync::{Arc, Mutex, Weak},
};
use tsr_arena::Counters;
use tsr_ast::{CompletedFile, SourceFileParseOptions};
use tsr_core::ScriptKind;
use tsr_jsstring::{JsString, SourceText};
/// An escaped file retains its complete parsed and bound owner. Graph edges in
/// a Program are IDs; they never retain another Program or create Arc cycles.
#[derive(Debug)]
pub struct ProgramFile {
    pub(crate) bound: CompletedFile,
    mapped_bundle: Option<Arc<[CompletedFile]>>,
}
impl ProgramFile {
    pub(crate) fn new(bound: CompletedFile) -> Self {
        Self {
            bound,
            mapped_bundle: None,
        }
    }
    pub fn bound(&self) -> &CompletedFile {
        &self.bound
    }
    pub fn source(&self) -> tsr_ast::NodeId {
        self.bound.source()
    }
}

impl ProgramFile {
    /// Shared construction primitive for loader and project caches. Binding
    /// finishes before a cache can publish the result to another program.
    pub fn parse_and_bind(
        source: SourceText,
        kind: ScriptKind,
        options: SourceFileParseOptions,
        counters: &Counters,
        tracing: Option<&Arc<dyn tsr_checker::TraceSink>>,
        source_hash: Option<tsr_ast::SourceHash>,
    ) -> Result<Arc<Self>, Error> {
        let parsed = tsr_parser::parse_source_file_with_counters(source, kind, options, counters);
        Self::bind_parsed(parsed, tracing, source_hash)
    }
    /// Bind a parsed mapper output using the same publication and trace path as
    /// ordinary source files. A bundle cache publishes only after all outputs
    /// have successfully crossed this boundary.
    pub fn bind_parsed(
        mut parsed: tsr_ast::ParsedFile,
        tracing: Option<&Arc<dyn tsr_checker::TraceSink>>,
        source_hash: Option<tsr_ast::SourceHash>,
    ) -> Result<Arc<Self>, Error> {
        if let Some(hash) = source_hash {
            parsed.root_source_file_mut()?.hash = hash;
        }
        // port: tsc/internal/compiler/program.go:Program.BindSourceFiles
        let _trace = tsr_checker::TraceScope::new(
            tracing,
            tsr_checker::TracePhase::Bind,
            "bindSourceFile",
            || {
                [(
                    "path".into(),
                    tsr_checker::TraceValue::Str(
                        String::from_utf8_lossy(
                            parsed
                                .view()
                                .source_file(parsed.root())
                                .expect("parsed source file")
                                .parse_options()
                                .path
                                .as_bytes(),
                        )
                        .into_owned(),
                    ),
                )]
                .into_iter()
                .collect()
            },
            true,
        );
        let bound = tsr_binder::bind_parsed_file(parsed)?;
        Ok(Arc::new(Self::new(bound)))
    }
}

/// Project-owned parse caches plug into the compiler without a reverse crate
/// dependency. The program holds the cache lease; escaped ASTs independently
/// retain their owner after the program has released its cache references.
pub struct CachedProgramFile {
    pub file: Arc<ProgramFile>,
    pub retention: Box<dyn Send + Sync>,
}
/// All bound outputs of one transform. Every escaped member retains the entire
/// syntax bundle, independently of its cache lease and without a Program cycle.
pub struct MappedProgramFiles {
    pub canonical: Arc<ProgramFile>,
    pub supplemental: Vec<Arc<ProgramFile>>,
}
impl MappedProgramFiles {
    pub fn bind(
        parsed: tsr_contentmapper::SourceFiles,
        tracing: Option<&Arc<dyn tsr_checker::TraceSink>>,
        hash: Option<tsr_ast::SourceHash>,
    ) -> Result<Self, Error> {
        let canonical = ProgramFile::bind_parsed(parsed.canonical, tracing, hash)?;
        let supplemental = parsed
            .supplemental
            .into_iter()
            .map(|file| ProgramFile::bind_parsed(file, tracing, hash))
            .collect::<Result<Vec<_>, _>>()?;
        let bundle: Arc<[CompletedFile]> = std::iter::once(&canonical)
            .chain(&supplemental)
            .map(|file| file.bound.clone())
            .collect();
        let retain = |mut file: Arc<ProgramFile>| {
            Arc::get_mut(&mut file)
                .expect("newly bound mapper output has not been published")
                .mapped_bundle = Some(bundle.clone());
            file
        };
        Ok(Self {
            canonical: retain(canonical),
            supplemental: supplemental.into_iter().map(retain).collect(),
        })
    }
    pub fn check_collisions(
        &self,
        host: &dyn tsr_vfs::FileSystem,
    ) -> Result<(), tsr_contentmapper::Error> {
        for file in &self.supplemental {
            let source = file
                .bound()
                .view()
                .source_file()
                .map_err(|error| tsr_contentmapper::Error::Message(format!("{error:?}")))?;
            if host.file_exists(source.file_name()).unwrap_or(false) {
                return Err(tsr_contentmapper::Error::SupplementalFileCollision(
                    JsString::from_bytes(source.file_name()),
                ));
            }
        }
        Ok(())
    }
}
pub struct CachedMappedProgramFiles {
    pub files: Arc<MappedProgramFiles>,
    pub retention: Box<dyn Send + Sync>,
}
pub struct MappedSourceFileRequest<'a> {
    pub options: &'a SourceFileParseOptions,
    pub content: &'a [u8],
    pub mapper: &'a tsr_tsoptions::config_mappers::ContentMapper,
    pub mapper_index: usize,
    pub project: &'a dyn tsr_contentmapper::Project,
    pub counters: &'a Counters,
    pub tracing: Option<&'a Arc<dyn tsr_checker::TraceSink>>,
}
pub type MappedFileResult<T> = Result<Result<T, tsr_contentmapper::Error>, Error>;
impl MappedSourceFileRequest<'_> {
    pub fn transform(
        &self,
        hash: Option<tsr_ast::SourceHash>,
    ) -> MappedFileResult<Arc<MappedProgramFiles>> {
        let parsed = match tsr_contentmapper::transform_and_parse(
            self.options,
            self.content,
            self.mapper,
            self.mapper_index,
            self.project,
            self.counters,
        ) {
            Ok(parsed) => parsed,
            Err(error) => return Ok(Err(error)),
        };
        Ok(Ok(Arc::new(MappedProgramFiles::bind(
            parsed,
            self.tracing,
            hash,
        )?)))
    }
}
type RetainedSources = Mutex<Vec<Box<dyn Send + Sync>>>;
#[derive(Clone, Default)]
pub(crate) struct ProgramRetention {
    _sources: Option<Arc<RetainedSources>>,
}
pub trait SourceFileCache: Send + Sync {
    fn acquire_mapped(
        &self,
        request: &MappedSourceFileRequest<'_>,
    ) -> MappedFileResult<CachedMappedProgramFiles> {
        Ok(request
            .transform(None)?
            .map(|files| CachedMappedProgramFiles {
                files,
                retention: Box::new(()),
            }))
    }
    /// Editor overlays can supply a language independently of the file suffix.
    fn script_kind(&self, name: &[u8]) -> ScriptKind {
        ScriptKind::ensure_from_file_name(name)
    }
    /// Retain an unchanged file in a cloned program, preserving its identity.
    fn retain(&self, file: &Arc<ProgramFile>) -> Result<Box<dyn Send + Sync>, Error>;
    fn acquire(
        &self,
        source: SourceText,
        kind: ScriptKind,
        options: SourceFileParseOptions,
        counters: &Counters,
        tracing: Option<&Arc<dyn tsr_checker::TraceSink>>,
    ) -> Result<CachedProgramFile, Error>;
}

#[derive(PartialEq, Eq, Hash)]
struct ParseKey {
    file_name: JsString,
    path: JsString,
    jsx: bool,
    force: bool,
    kind: ScriptKind,
}
impl ParseKey {
    fn new(options: &SourceFileParseOptions, kind: ScriptKind) -> Self {
        Self {
            file_name: options.file_name.clone(),
            path: options.path.clone(),
            jsx: options.external_module_indicator_options.jsx,
            force: options.external_module_indicator_options.force,
            kind,
        }
    }
}
type ParseEntry = Arc<Mutex<Option<Arc<ProgramFile>>>>;

/// Declaration/JSON owners shared by the projects in one build cycle. The
/// caller supplies a stable filesystem for that cycle and resets this cache
/// after its project workers have joined. Ordinary source files are never
/// admitted. Entries retain owners until reset; escaped files remain valid.
#[derive(Default)]
pub struct SharedSourceFileCache {
    entries: Mutex<HashMap<ParseKey, ParseEntry>>,
}
impl SharedSourceFileCache {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn reset(&self) {
        self.entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();
    }
    /// The map lock only selects an entry. Reading, parsing and binding happen
    /// under its own lock, so distinct files can load concurrently. Missing or
    /// failed reads are retried; a panic cannot publish a partial owner.
    // port: tsc/internal/execute/build/parseCache.go:parseCache.loadOrStore
    fn load_or_store(
        &self,
        key: ParseKey,
        load: impl FnOnce() -> Result<Option<Arc<ProgramFile>>, Error>,
    ) -> Result<Option<Arc<ProgramFile>>, Error> {
        let entry = self
            .entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .entry(key)
            .or_default()
            .clone();
        let mut value = entry
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(file) = value.as_ref() {
            return Ok(Some(file.clone()));
        }
        *value = load()?;
        Ok(value.clone())
    }
}
/// Explicit reuse cache. Normal caches use weak entries, releasing files after
/// the final snapshot/response drops. Build caches additionally retain shared
/// declaration/JSON owners through their build cycle. Local reuse compares
/// source bytes and parse context. Each load exclusively borrows its own cache.
#[derive(Default)]
pub struct FileCache {
    files: BTreeMap<JsString, Vec<Weak<ProgramFile>>>,
    shared: Option<Arc<SharedSourceFileCache>>,
    project: Option<Arc<dyn SourceFileCache>>,
    project_retention: Weak<RetainedSources>,
}
impl FileCache {
    pub(crate) fn script_kind(&self, name: &[u8]) -> ScriptKind {
        self.project.as_ref().map_or_else(
            || ScriptKind::ensure_from_file_name(name),
            |cache| cache.script_kind(name),
        )
    }
    pub(crate) fn retain_project_file(&self, file: &Arc<ProgramFile>) -> Result<(), Error> {
        if let Some(cache) = &self.project {
            let lease = cache.retain(file)?;
            self.project_retention
                .upgrade()
                .expect("program retention scope")
                .lock()
                .expect("program retention poisoned")
                .push(lease);
        }
        Ok(())
    }
    pub(crate) fn acquire_mapped(
        &self,
        request: &MappedSourceFileRequest<'_>,
        host: &dyn tsr_vfs::FileSystem,
    ) -> MappedFileResult<Arc<MappedProgramFiles>> {
        let acquired = match &self.project {
            Some(cache) => cache.acquire_mapped(request)?,
            None => request
                .transform(None)?
                .map(|files| CachedMappedProgramFiles {
                    files,
                    retention: Box::new(()),
                }),
        };
        let acquired = match acquired {
            Ok(acquired) => acquired,
            Err(error) => return Ok(Err(error)),
        };
        // A collision is filesystem-dependent and must be checked even on cache hits.
        if let Err(error) = acquired.files.check_collisions(host) {
            return Ok(Err(error));
        }
        if let Some(retention) = self.project_retention.upgrade() {
            retention
                .lock()
                .expect("program retention poisoned")
                .push(acquired.retention);
        }
        Ok(Ok(acquired.files))
    }
    pub(crate) fn begin_program(&mut self) -> ProgramRetention {
        let retention = self
            .project
            .as_ref()
            .map(|_| Arc::new(Mutex::new(Vec::new())));
        self.project_retention = retention.as_ref().map_or_else(Weak::new, Arc::downgrade);
        ProgramRetention {
            _sources: retention,
        }
    }
    pub fn new() -> Self {
        Self::default()
    }
    pub fn for_project(cache: Arc<dyn SourceFileCache>) -> Self {
        Self {
            project: Some(cache),
            ..Self::default()
        }
    }
    /// A project-local cache, with only declaration and JSON files shared with
    /// the other projects in this build cycle. Give every load its own instance.
    pub fn for_build(shared: Arc<SharedSourceFileCache>) -> Self {
        Self {
            shared: Some(shared),
            ..Self::default()
        }
    }
    /// Drop the reuse candidates for a changed source path. Existing programs
    /// still retain their bound owners until the replacement program is ready.
    pub fn evict(&mut self, path: &[u8]) {
        self.files.remove(path);
    }
    pub fn prune(&mut self) {
        self.files.retain(|_, entries| {
            entries.retain(|entry| entry.strong_count() != 0);
            !entries.is_empty()
        });
    }
    /// What the loader's parse-ahead workers load with: this cache's project
    /// cache, so their files carry the leases the loader's own loads would.
    pub(crate) fn producer(
        &self,
        host: &Arc<dyn tsr_vfs::FileSystem>,
        counters: &Counters,
    ) -> crate::preload::Producer {
        crate::preload::Producer {
            host: host.clone(),
            project: self.project.clone(),
            counters: counters.clone(),
        }
    }
    /// Whether a worker may load the file ahead of the loader: not through
    /// the build-shared cache, and not when a local reuse candidate exists,
    /// which only the loader's own read can compare.
    pub(crate) fn preloadable(&self, options: &SourceFileParseOptions) -> bool {
        let file_name = options.file_name.as_bytes();
        if self.shared.is_some()
            && (tsr_tspath::is_declaration_file_name(file_name)
                || tsr_tspath::file_extension_is(file_name, b".json"))
        {
            return false;
        }
        self.project.is_some()
            || !self
                .files
                .get(&options.path)
                .is_some_and(|entries| entries.iter().any(|entry| entry.strong_count() != 0))
    }
    /// A file a worker loaded, taking the place of the loader's own load.
    fn adopt(&mut self, preloaded: crate::preload::Preloaded) -> Arc<ProgramFile> {
        match preloaded.retention {
            Some(retention) => self
                .project_retention
                .upgrade()
                .expect("project parse cache requires a program load")
                .lock()
                .expect("program retention poisoned")
                .push(retention),
            None => self
                .files
                .entry(preloaded.options.path.clone())
                .or_default()
                .push(Arc::downgrade(&preloaded.file)),
        }
        preloaded.file
    }
    // port: tsc/internal/execute/build/host.go:host.GetSourceFile
    pub(crate) fn load(
        &mut self,
        host: &dyn tsr_vfs::FileSystem,
        kind: ScriptKind,
        options: SourceFileParseOptions,
        counters: &Counters,
        tracing: Option<&Arc<dyn tsr_checker::TraceSink>>,
        preloader: Option<&crate::preload::Preloader>,
    ) -> Result<Option<Arc<ProgramFile>>, Error> {
        if let Some(preloaded) = preloader.and_then(|preloader| preloader.take(&options.path)) {
            if preloaded.kind == kind && preloaded.options == options {
                return Ok(Some(self.adopt(preloaded)));
            }
        }
        let shared = self
            .shared
            .as_ref()
            .filter(|_| {
                tsr_tspath::is_declaration_file_name(options.file_name.as_bytes())
                    || tsr_tspath::file_extension_is(options.file_name.as_bytes(), b".json")
            })
            .cloned();
        let key = shared.as_ref().map(|_| ParseKey::new(&options, kind));
        let load = || {
            let Some(content) = host.read_file(options.file_name.as_bytes())? else {
                return Ok(None);
            };
            self.acquire(content.text, kind, options, counters, tracing)
                .map(Some)
        };
        if let (Some(shared), Some(key)) = (shared, key) {
            shared.load_or_store(key, load)
        } else {
            load()
        }
    }
    pub(crate) fn acquire(
        &mut self,
        source: SourceText,
        kind: ScriptKind,
        options: SourceFileParseOptions,
        counters: &Counters,
        tracing: Option<&Arc<dyn tsr_checker::TraceSink>>,
    ) -> Result<Arc<ProgramFile>, Error> {
        if let Some(cache) = &self.project {
            let acquired = cache.acquire(source, kind, options, counters, tracing)?;
            let retention = self
                .project_retention
                .upgrade()
                .expect("project parse cache requires a program load");
            retention
                .lock()
                .expect("program retention poisoned")
                .push(acquired.retention);
            return Ok(acquired.file);
        }
        let entries = self.files.entry(options.path.clone()).or_default();
        entries.retain(|entry| entry.strong_count() != 0);
        for entry in entries.iter() {
            if let Some(file) = entry.upgrade() {
                let state = file.bound.view().source_file()?;
                if state.script_kind == kind
                    && state.parse_options() == &options
                    && state.text().as_bytes() == source.as_bytes()
                {
                    return Ok(file);
                }
            }
        }
        let file = ProgramFile::parse_and_bind(source, kind, options, counters, tracing, None)?;
        entries.push(Arc::downgrade(&file));
        Ok(file)
    }
}

#[cfg(test)]
mod tests;
