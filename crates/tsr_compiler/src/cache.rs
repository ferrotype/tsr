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
}
impl ProgramFile {
    pub(crate) fn new(bound: CompletedFile) -> Self {
        Self { bound }
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
type RetainedSources = Mutex<Vec<Box<dyn Send + Sync>>>;
#[derive(Clone, Default)]
pub(crate) struct ProgramRetention {
    _sources: Option<Arc<RetainedSources>>,
}
pub trait SourceFileCache: Send + Sync {
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
    // port: tsc/internal/execute/build/host.go:host.GetSourceFile
    pub(crate) fn load(
        &mut self,
        host: &dyn tsr_vfs::FileSystem,
        kind: ScriptKind,
        options: SourceFileParseOptions,
        counters: &Counters,
        tracing: Option<&Arc<dyn tsr_checker::TraceSink>>,
    ) -> Result<Option<Arc<ProgramFile>>, Error> {
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
