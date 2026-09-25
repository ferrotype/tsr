use crate::include_reason::{
    IncludeExplanations, IncludeReason, IncludeReasonData, SyntheticImport,
};
use crate::project_references::{ProjectReferenceFileMapper, ProjectReferenceParser};
use crate::{metadata, FileCache, ProgramFile, SourceFileMetaData};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{Arc, OnceLock},
};
use tsr_arena::Counters;
use tsr_ast::{Diagnostic, NodeId, SourceFileParseOptions};
use tsr_core::{CompilerOptions, ModuleKind, ScriptKind};
use tsr_jsstring::JsString;
use tsr_module::{ResolvedModule, ResolvedTypeReferenceDirective, Resolver};
use tsr_tsoptions::ParsedCommandLine;
use tsr_tspath as path;
use tsr_vfs::FileSystem;
#[derive(Debug)]
pub enum Error {
    Checker(tsr_checker::Error),
    Host(tsr_vfs::Error),
    Resolution(tsr_module::Error),
    Ast(tsr_arena::Error),
    Bind(tsr_ast::BindError),
    Unsupported(&'static str),
}
impl From<tsr_checker::Error> for Error {
    fn from(error: tsr_checker::Error) -> Self {
        Self::Checker(error)
    }
}
impl From<tsr_vfs::Error> for Error {
    fn from(e: tsr_vfs::Error) -> Self {
        Self::Host(e)
    }
}
impl From<tsr_module::Error> for Error {
    fn from(e: tsr_module::Error) -> Self {
        Self::Resolution(e)
    }
}
impl From<tsr_arena::Error> for Error {
    fn from(e: tsr_arena::Error) -> Self {
        Self::Ast(e)
    }
}
impl From<tsr_ast::BindError> for Error {
    fn from(e: tsr_ast::BindError) -> Self {
        Self::Bind(e)
    }
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for Error {}
pub struct ProgramOptions {
    pub config: tsr_tsoptions::ParsedCommandLine,
    pub host: Arc<dyn FileSystem>,
    pub current_directory: JsString,
    pub default_library_path: JsString,
    pub skip_module_resolution: bool,
}
#[derive(Clone, Debug)]
pub struct Resolution {
    pub file: JsString,
    pub name: JsString,
    pub mode: ModuleKind,
    pub result: ResolvedModule,
}
#[derive(Clone, Debug)]
pub struct TypeResolution {
    pub file: JsString,
    pub name: JsString,
    pub mode: ModuleKind,
    pub result: ResolvedTypeReferenceDirective,
}
/// Published only after every required operation succeeds. This contains loader,
/// bind and option-verification results. Content-mapper execution, the editor's
/// source-of-reference mode and checker construction remain explicit
/// unsupported boundaries.
pub struct Program {
    pub(crate) owners: crate::resolver_host::OwnerIndex,
    pub(crate) include_reasons: BTreeMap<JsString, Vec<Arc<IncludeReason>>>,
    pub(crate) references: ProjectReferenceFileMapper,
    /// When a referenced project's source was replaced by its output, the
    /// output's path maps to the source's file name.
    output_file_to_project_reference_source: BTreeMap<JsString, JsString>,
    pub(crate) redirect_paths: BTreeMap<JsString, JsString>,
    pub(crate) redirect_file_names: BTreeMap<JsString, JsString>,
    pub(crate) package_resolver: std::sync::Mutex<Resolver>,
    pub(crate) include_explanations: IncludeExplanations,
    pub(crate) declaration_diagnostics:
        std::sync::Mutex<std::collections::HashMap<NodeId, Vec<Diagnostic>>>,
    pub(crate) diagnostic_snapshot: crate::program_diagnostics::ProgramDiagnostics,
    option_verification: crate::OptionVerification,
    config: tsr_tsoptions::ParsedCommandLine,
    cwd: JsString,
    external_paths: BTreeSet<JsString>,
    options: Arc<CompilerOptions>,
    host: Arc<dyn FileSystem>,
    files: Vec<Arc<ProgramFile>>,
    by_path: BTreeMap<JsString, usize>,
    pub(crate) metadata: BTreeMap<JsString, SourceFileMetaData>,
    libs: BTreeSet<JsString>,
    missing: Vec<JsString>,
    resolutions: Vec<Resolution>,
    type_resolutions: Vec<TypeResolution>,
    loader_diagnostics: Vec<Diagnostic>,
    /// Roots that did not resolve, explained when the diagnostics are first read.
    root_failures: Vec<RootFailure>,
    include_diagnostics: OnceLock<Result<Vec<Diagnostic>, tsr_arena::Error>>,
    trace: Vec<tsr_module::DiagAndArgs>,
}
impl Program {
    pub fn load(
        options: ProgramOptions,
        cache: &mut FileCache,
        counters: &Counters,
    ) -> Result<Self, Error> {
        Loader::new(options, cache, counters, false, false)?.run()
    }
    /// `load` for a host that asks for the source of each project reference
    /// instead of its built output (the pin's `UseSourceOfProjectReference`).
    /// That mode is the editor's; a program whose references have outputs is
    /// rejected unless `disableSourceOfProjectReferenceRedirect` is set.
    pub fn load_with_source_of_project_reference(
        options: ProgramOptions,
        use_source_of_project_reference: bool,
        cache: &mut FileCache,
        counters: &Counters,
    ) -> Result<Self, Error> {
        Loader::new(
            options,
            cache,
            counters,
            false,
            use_source_of_project_reference,
        )?
        .run()
    }
    /// Load one program from a live host, with fresh resolution caches.
    ///
    /// The caller must keep the filesystem stable for the duration of the load
    /// and create a new program after mutations. Loaded source files remain
    /// owned and immutable, but subsequent host-dependent operations may observe
    /// the live filesystem. `load` retains its immutable-host requirement.
    pub fn load_live(
        options: ProgramOptions,
        cache: &mut FileCache,
        counters: &Counters,
    ) -> Result<Self, Error> {
        Loader::new(options, cache, counters, true, false)?.run()
    }
    pub fn config(&self) -> &tsr_tsoptions::ParsedCommandLine {
        &self.config
    }
    pub fn current_directory(&self) -> &[u8] {
        self.cwd.as_bytes()
    }
    pub fn is_external_library(&self, path: &[u8]) -> bool {
        self.external_paths.contains(path)
    }
    /// port: tsc/internal/compiler/program.go:Program.GetSourceFiles
    pub fn files(&self) -> &[Arc<ProgramFile>] {
        &self.files
    }
    pub fn options(&self) -> &CompilerOptions {
        &self.options
    }
    pub fn host(&self) -> &dyn FileSystem {
        self.host.as_ref()
    }
    /// port: tsc/internal/compiler/program.go:Program.GetSourceFileByPath
    pub fn file(&self, path: &[u8]) -> Option<&ProgramFile> {
        self.by_path.get(path).map(|&i| self.files[i].as_ref())
    }
    pub fn metadata(&self, path: &[u8]) -> Option<&SourceFileMetaData> {
        self.metadata.get(path)
    }
    pub fn is_lib(&self, path: &[u8]) -> bool {
        self.libs.contains(path)
    }
    pub fn missing_files(&self) -> &[JsString] {
        &self.missing
    }
    pub fn resolutions(&self) -> &[Resolution] {
        &self.resolutions
    }
    pub fn type_resolutions(&self) -> &[TypeResolution] {
        &self.type_resolutions
    }
    pub fn trace(&self) -> &[tsr_module::DiagAndArgs] {
        &self.trace
    }
    /// The loader's include diagnostics: root, reference and resolution
    /// failures. Unresolved roots are explained at first read.
    pub fn include_diagnostics(&self) -> Result<&[Diagnostic], Error> {
        Ok(self.loader_include_diagnostics()?)
    }
    /// The verifier runs once during construction. Reading its raw writes does
    /// not force the source include processor's lazy explanations.
    pub fn option_verification(&self) -> &crate::OptionVerification {
        &self.option_verification
    }
    /// The original source file name when the file at `path` is a referenced
    /// project's output that replaced that source; otherwise `file_name`.
    /// port: tsc/internal/compiler/program.go:Program.GetSourceOfProjectReferenceIfOutputIncluded
    pub fn source_of_project_reference_if_output_included<'a>(
        &'a self,
        path: &[u8],
        file_name: &'a [u8],
    ) -> &'a [u8] {
        self.output_file_to_project_reference_source
            .get(path)
            .map_or(file_name, JsString::as_bytes)
    }
    /// Walks the resolved references in preorder, each config path once. `f`
    /// receives the reference's config path, its parsed config (None when it
    /// could not be read), the referencing config and the reference's index
    /// there; returning false stops the walk. The result is false when the walk
    /// was stopped or the program has no references.
    /// port: tsc/internal/compiler/program.go:Program.RangeResolvedProjectReference
    pub fn range_resolved_project_reference(
        &self,
        mut f: impl FnMut(&[u8], Option<&ParsedCommandLine>, &ParsedCommandLine, usize) -> bool,
    ) -> bool {
        self.references
            .range_resolved_project_reference(&self.config, &mut |path, config, parent, index| {
                f(path.as_bytes(), config, parent, index)
            })
    }
    /// The options a file was loaded with: its project reference's when it is
    /// that reference's source or output, otherwise the program's.
    pub fn options_for_file(&self, path: &[u8], file_name: &[u8]) -> &CompilerOptions {
        self.references
            .published_compiler_options_for_file(&self.options, path, file_name)
    }
    /// The referenced project whose options resolve this file's imports
    /// (the pin's Program.GetRedirectForResolution, a checker accessor).
    pub fn redirect_for_resolution(
        &self,
        path: &[u8],
        file_name: &[u8],
    ) -> Option<&ParsedCommandLine> {
        self.references
            .published_redirect_for_resolution(path, file_name)
            .0
            .map(|index| self.references.config(index))
    }
    /// The referenced project a source belongs to (the pin's
    /// Program.GetProjectReferenceFromSource, a checker accessor).
    pub fn project_reference_from_source(&self, path: &[u8]) -> Option<&ParsedCommandLine> {
        self.references
            .project_reference_from_source(path)
            .map(|file| self.references.config(file.config))
    }
    /// The referenced project a declaration output belongs to (the pin's
    /// Program.GetProjectReferenceFromOutputDts, a checker accessor).
    pub fn project_reference_from_output_dts(&self, path: &[u8]) -> Option<&ParsedCommandLine> {
        self.references
            .project_reference_from_output_dts(path)
            .map(|file| self.references.config(file.config))
    }
    /// The output that replaced a referenced project's source, if any (the
    /// pin's Program.GetParseFileRedirect, a checker accessor).
    pub fn parse_file_redirect(&self, file_name: &[u8]) -> Option<JsString> {
        let path = path::to_path(
            file_name,
            self.cwd.as_bytes(),
            self.host.use_case_sensitive_file_names(),
        );
        self.references
            .published_parse_file_redirect(path.as_bytes())
    }
    /// The loader's own include diagnostics, computed at first use as the pin's
    /// include processor computes them. A root that did not resolve is
    /// explained then, with its root reason naming the tsconfig spec that
    /// listed it, and the collection is ordered again.
    pub(crate) fn loader_include_diagnostics(&self) -> Result<&[Diagnostic], tsr_arena::Error> {
        self.include_diagnostics
            .get_or_init(|| {
                if self.root_failures.is_empty() {
                    return Ok(self.loader_diagnostics.clone());
                }
                let mut diagnostics = self.loader_diagnostics.clone();
                for (reason, message, args) in &self.root_failures {
                    diagnostics.push(self.explain_file_include_with_reason(
                        b"",
                        Some(reason),
                        message,
                        args.clone(),
                    )?);
                }
                let name =
                    |id| crate::program_diagnostics::source_names(self, id).map(|(name, _)| name);
                for diagnostic in &diagnostics {
                    for related in &diagnostic.related_information {
                        if let Some(file) = related.file {
                            name(file)?;
                        }
                    }
                }
                diagnostics.sort_by(|a, b| {
                    tsr_ast::compare_diagnostics(a, b, &name)
                        .expect("all include diagnostic source identities validated")
                });
                diagnostics.dedup_by(|a, b| {
                    tsr_ast::equal_diagnostics(a, b, &name)
                        .expect("all include diagnostic source identities validated")
                });
                Ok(diagnostics)
            })
            .as_deref()
            .map_err(|error| *error)
    }
    /// The configuration source that owns a diagnostic's file identity: the
    /// program's config, a config it extends, or a referenced config.
    pub fn config_source(&self, id: NodeId) -> Option<&tsr_tsoptions::TsConfigSourceFile> {
        std::iter::once(&self.config)
            .chain(self.references.configs())
            .flat_map(|config| config.config_file.iter().chain(&config.config_dependencies))
            .find(|config| config.root == id)
            .map(AsRef::as_ref)
    }
}
struct Loader<'a> {
    config: tsr_tsoptions::ParsedCommandLine,
    pending: Vec<LoadTask>,
    roles: BTreeMap<JsString, (bool, bool)>,
    child_tasks: BTreeMap<JsString, Vec<LoadTask>>,
    file_traces: BTreeMap<JsString, FileTraces>,
    library_traces: BTreeMap<JsString, Vec<tsr_module::DiagAndArgs>>,
    trace: Vec<tsr_module::DiagAndArgs>,
    options: Arc<CompilerOptions>,
    host: Arc<dyn FileSystem>,
    cwd: JsString,
    lib_path: JsString,
    lib_files: BTreeMap<Vec<u8>, Vec<u8>>,
    skip_resolution: bool,
    resolver: Resolver,
    cache: &'a mut FileCache,
    counters: &'a Counters,
    depths: BTreeMap<JsString, isize>,
    roots: Vec<IncludeEdge>,
    children: BTreeMap<JsString, Vec<IncludeEdge>>,
    include_reasons: BTreeMap<JsString, Vec<Arc<IncludeReason>>>,
    package_ids: BTreeMap<JsString, tsr_module::PackageId>,
    files: Vec<Arc<ProgramFile>>,
    metadata: BTreeMap<JsString, SourceFileMetaData>,
    libs: BTreeSet<JsString>,
    /// Every path whose load produced no file, with the name it was loaded by.
    /// Collection lists them in its own order.
    missing: BTreeMap<JsString, JsString>,
    resolutions: Vec<Resolution>,
    type_resolutions: Vec<TypeResolution>,
    diagnostics: Vec<Diagnostic>,
    references: ProjectReferenceFileMapper,
    can_use_project_reference_source: bool,
    /// The reason of each path's first task: the task that loads the file and,
    /// for a referenced source, owns its redirect. Kept only while a redirect
    /// is possible.
    first_reasons: BTreeMap<JsString, Arc<IncludeReason>>,
    redirected: BTreeMap<JsString, RedirectedSource>,
    redirect_outputs: BTreeSet<JsString>,
    /// Roots that did not resolve, explained once the program exists.
    root_failures: Vec<RootFailure>,
}
type ReferenceFailure = (&'static tsr_diagnostics::Message, Vec<JsString>);
type RootFailure = (
    Arc<IncludeReason>,
    &'static tsr_diagnostics::Message,
    Vec<JsString>,
);
/// port: tsc/internal/compiler/program.go:ProgramOptions.canUseProjectReferenceSource
fn can_use_project_reference_source(
    use_source_of_project_reference: bool,
    config: &ParsedCommandLine,
) -> bool {
    use_source_of_project_reference
        && !config
            .options
            .disable_source_of_project_reference_redirect
            .is_true()
}
/// Builds the mapper every program has, reading the referenced configs when
/// there are any. The source-of-reference mode swaps in a declaration-faking
/// resolution host when references have outputs; that host is the editor's
/// and is an explicit boundary here.
/// port: tsc/internal/compiler/fileloader.go:fileLoader.addProjectReferenceTasks
fn add_project_reference_tasks(
    config: &ParsedCommandLine,
    can_use_source: bool,
    host: &Arc<dyn FileSystem>,
    cwd: &JsString,
) -> Result<ProjectReferenceFileMapper, Error> {
    let mut mapper = ProjectReferenceFileMapper::new(config, can_use_source);
    let references = config.resolved_project_reference_paths();
    if references.is_empty() {
        return Ok(mapper);
    }
    ProjectReferenceParser::new(&mut mapper, host.clone(), cwd.clone())
        .parse(references, config.config_file.as_ref())?;
    if can_use_source && mapper.has_outputs() {
        return Err(Error::Unsupported(
            "project-reference source redirection (newProjectReferenceDtsFakingHost)",
        ));
    }
    Ok(mapper)
}
impl<'a> Loader<'a> {
    fn new(
        input: ProgramOptions,
        cache: &'a mut FileCache,
        counters: &'a Counters,
        allow_live_host: bool,
        use_source_of_project_reference: bool,
    ) -> Result<Self, Error> {
        if input
            .config
            .content_mappers
            .as_ref()
            .is_some_and(|mappers| !mappers.is_empty())
        {
            return Err(Error::Unsupported("content-mapper execution"));
        }
        let can_use_project_reference_source =
            can_use_project_reference_source(use_source_of_project_reference, &input.config);
        let references = add_project_reference_tasks(
            &input.config,
            can_use_project_reference_source,
            &input.host,
            &input.current_directory,
        )?;
        let options = Arc::new(input.config.options.clone());
        let resolver = Resolver::with_options(
            input.host.clone(),
            options.clone(),
            input.current_directory.as_bytes(),
            tsr_module::ResolverOptions {
                allow_live_host,
                ..Default::default()
            },
        )?;
        let lib_path = JsString::from_bytes(path::absolute(
            input.default_library_path.as_bytes(),
            input.current_directory.as_bytes(),
        ));
        Ok(Self {
            config: input.config,
            pending: Vec::new(),
            roles: BTreeMap::new(),
            child_tasks: BTreeMap::new(),
            file_traces: BTreeMap::new(),
            library_traces: BTreeMap::new(),
            trace: Vec::new(),
            options,
            host: input.host,
            cwd: input.current_directory,
            lib_path,
            lib_files: BTreeMap::new(),
            skip_resolution: input.skip_module_resolution,
            resolver,
            cache,
            counters,
            depths: BTreeMap::new(),
            roots: Vec::new(),
            children: BTreeMap::new(),
            include_reasons: BTreeMap::new(),
            package_ids: BTreeMap::new(),
            files: Vec::new(),
            metadata: BTreeMap::new(),
            libs: BTreeSet::new(),
            missing: BTreeMap::new(),
            resolutions: Vec::new(),
            type_resolutions: Vec::new(),
            diagnostics: Vec::new(),
            references,
            can_use_project_reference_source,
            first_reasons: BTreeMap::new(),
            redirected: BTreeMap::new(),
            redirect_outputs: BTreeSet::new(),
            root_failures: Vec::new(),
        })
    }
    fn run(mut self) -> Result<Program, Error> {
        let roots = std::mem::take(&mut self.config.root_file_names);
        for (index, root) in roots.iter().enumerate() {
            let absolute = path::absolute(root.as_bytes(), self.cwd.as_bytes());
            match self.file_reference(&absolute, root.as_bytes(), None)? {
                Ok(name) => {
                    self.link(None, &name, None, IncludeReasonData::Root { index });
                    self.load(&name, false, true, 0);
                }
                Err((message, args)) => {
                    // The failed root keeps its task: collection records its
                    // reason and lists it as missing in root order, and the
                    // published program explains it with that reason.
                    let key = path::to_path(
                        &absolute,
                        self.cwd.as_bytes(),
                        self.host.use_case_sensitive_file_names(),
                    );
                    let reason = Arc::new(IncludeReason::new(IncludeReasonData::Root { index }));
                    self.roots.push(IncludeEdge {
                        path: key.clone(),
                        reason: Some(reason.clone()),
                    });
                    self.root_failures.push((reason, message, args));
                    self.missing
                        .entry(key)
                        .or_insert_with(|| JsString::from_bytes(absolute));
                }
            }
        }
        if !roots.is_empty() && !self.options.no_lib.is_true() {
            let libraries = self.options.lib.clone();
            if let Some(libs) = libraries {
                for (index, lib) in libs.into_iter().enumerate() {
                    if let Some(name) = tsr_tsoptions::lib_file_name(lib.as_bytes()) {
                        self.load_lib(
                            name.as_bytes(),
                            None,
                            IncludeReasonData::Lib { index: Some(index) },
                        )?;
                    }
                }
            } else {
                self.load_lib(
                    tsr_tsoptions::default_lib_file_name(&self.options).as_bytes(),
                    None,
                    IncludeReasonData::Lib { index: None },
                )?;
            }
        }
        if !roots.is_empty() && !self.skip_resolution {
            self.load_automatic_types()?;
        }
        self.config.root_file_names = roots;
        while let Some(task) = self.pending.pop() {
            if task.elide && task.depth > self.options.max_node_module_js_depth.unwrap_or_default()
            {
                continue;
            }
            self.load_worker(&task.name, task.is_lib, task.is_root, task.depth)?;
        }
        let loaded_names: BTreeMap<_, _> = self
            .files
            .iter()
            .map(|file| {
                let source = file.bound().view().source_file().expect("retained source");
                (
                    source.parse_options().path.clone(),
                    source.parse_options().file_name.clone(),
                )
            })
            .collect();
        let collected = self.collect_files();
        let redirects = collected.redirects;
        let redirect_file_names = redirects
            .keys()
            .map(|key| {
                let name = loaded_names
                    .get(key)
                    .or_else(|| self.redirected.get(key).map(|source| &source.name))
                    .expect("package redirect of a loaded or redirected file");
                (key.clone(), name.clone())
            })
            .collect();
        for traces in self.library_traces.into_values() {
            self.trace.extend(traces);
        }

        self.files.sort_by_key(|file| {
            let state = file.bound.view().source_file().expect("retained source");
            let name = state.parse_options().file_name.as_bytes();
            if self.libs.contains(state.parse_options().path.as_bytes()) {
                (0, lib_priority(name, self.lib_path.as_bytes()))
            } else {
                (1, 0)
            }
        });
        let mut by_path: BTreeMap<JsString, usize> = self
            .files
            .iter()
            .enumerate()
            .map(|(i, f)| {
                (
                    f.bound
                        .view()
                        .source_file()
                        .expect("retained source")
                        .parse_options()
                        .path
                        .clone(),
                    i,
                )
            })
            .collect();
        let retained_paths: BTreeSet<_> = by_path.keys().cloned().collect();
        for (alias, target) in &redirects {
            if let Some(&index) = by_path.get(target) {
                by_path.insert(alias.clone(), index);
            }
        }
        self.resolutions.retain(|r| {
            retained_paths.contains(&r.file)
                || self.lib_files.keys().any(|lib| {
                    let mut suffix = b"__lib_node_modules_lookup_".to_vec();
                    suffix.extend_from_slice(lib);
                    suffix.extend_from_slice(b"__.ts");
                    r.file.as_bytes().ends_with(&suffix)
                })
        });
        self.type_resolutions.retain(|r| {
            retained_paths.contains(&r.file)
                || r.file
                    .as_bytes()
                    .ends_with(tsr_module::INFERRED_TYPES_CONTAINING_FILE)
        });
        self.resolutions
            .sort_by(|a, b| (&a.file, &a.name, a.mode).cmp(&(&b.file, &b.name, b.mode)));
        self.resolutions
            .dedup_by(|a, b| a.file == b.file && a.name == b.name && a.mode == b.mode);
        self.type_resolutions
            .sort_by(|a, b| (&a.file, &a.name, a.mode).cmp(&(&b.file, &b.name, b.mode)));
        self.type_resolutions
            .dedup_by(|a, b| a.file == b.file && a.name == b.name && a.mode == b.mode);
        for resolution in &self.resolutions {
            self.diagnostics
                .extend(resolution.result.resolution_diagnostics.iter().cloned());
        }
        for resolution in &self.type_resolutions {
            self.diagnostics
                .extend(resolution.result.resolution_diagnostics.iter().cloned());
        }
        let source_states: Vec<_> = self
            .files
            .iter()
            .map(|file| file.bound.view().source_file().expect("retained source"))
            .collect();
        let names: BTreeMap<_, _> = self
            .files
            .iter()
            .zip(&source_states)
            .map(|(file, state)| (file.source(), state.parse_options().file_name.as_bytes()))
            .collect();
        self.diagnostics
            .retain(|d| d.file.is_none_or(|file| names.contains_key(&file)));
        let name = |id| names.get(&id).copied().ok_or(tsr_arena::Error::WrongOwner);
        self.diagnostics.sort_by(|a, b| {
            tsr_ast::compare_diagnostics(a, b, &name)
                .expect("all include diagnostic source identities validated")
        });
        self.diagnostics.dedup_by(|a, b| {
            tsr_ast::equal_diagnostics(a, b, &name)
                .expect("all include diagnostic source identities validated")
        });
        let external_paths = self
            .depths
            .iter()
            .filter_map(|(key, &depth)| {
                (depth > 0 && by_path.contains_key(key)).then_some(key.clone())
            })
            .collect();
        let mut references = self.references;
        references.release_loader();
        let mut program = Program {
            include_reasons: self.include_reasons,
            references,
            output_file_to_project_reference_source: collected.output_to_source,
            redirect_paths: redirects,
            redirect_file_names,
            package_resolver: std::sync::Mutex::new(self.resolver),
            include_explanations: IncludeExplanations::default(),
            diagnostic_snapshot: crate::program_diagnostics::ProgramDiagnostics::default(),
            declaration_diagnostics: std::sync::Mutex::default(),
            option_verification: crate::OptionVerification {
                diagnostics: Vec::new(),
                include_diagnostics: Vec::new(),
                blocked_output_paths: BTreeSet::new(),
            },
            config: self.config,
            cwd: self.cwd,
            external_paths,
            owners: crate::resolver_host::OwnerIndex::from_files(&self.files),
            options: self.options,
            host: self.host,
            files: self.files,
            by_path,
            metadata: self.metadata,
            libs: self.libs,
            missing: collected.missing,
            resolutions: self.resolutions,
            type_resolutions: self.type_resolutions,
            loader_diagnostics: self.diagnostics,
            root_failures: self.root_failures,
            include_diagnostics: OnceLock::new(),
            trace: self.trace,
        };
        program.option_verification = crate::verify_compiler_options(&program)?;
        Ok(program)
    }
    fn link(
        &mut self,
        parent: Option<&JsString>,
        name: &[u8],
        package: Option<&tsr_module::PackageId>,
        reason: IncludeReasonData,
    ) {
        let key = path::to_path(
            name,
            self.cwd.as_bytes(),
            self.host.use_case_sensitive_file_names(),
        );
        if let Some(package) = package.filter(|p| !p.name.is_empty()) {
            self.package_ids
                .entry(key.clone())
                .or_insert_with(|| package.clone());
        }
        let reason = Arc::new(IncludeReason::new(reason));
        if self.references.has_sources() {
            self.first_reasons
                .entry(key.clone())
                .or_insert_with(|| reason.clone());
        }
        let edge = IncludeEdge {
            path: key,
            reason: Some(reason),
        };
        if let Some(parent) = parent {
            self.children.entry(parent.clone()).or_default().push(edge);
        } else {
            self.roots.push(edge);
        }
    }
    // filesparser.go:collectFiles. Package identity redirects happen before the
    // subtree walk; postorder source publication happens after it. Parsing and
    // collection have separate ownership: unselected duplicate files are dropped.
    fn collect_files(&mut self) -> Collected {
        let mut files: BTreeMap<JsString, Arc<ProgramFile>> = std::mem::take(&mut self.files)
            .into_iter()
            .map(|f| {
                let key = f
                    .bound
                    .view()
                    .source_file()
                    .expect("retained source")
                    .parse_options()
                    .path
                    .clone();
                (key, f)
            })
            .collect();
        let mut collector = Collector {
            loaded_paths: files.keys().chain(self.missing.keys()).cloned().collect(),
            include_reasons: &mut self.include_reasons,
            files: &mut files,
            file_traces: &mut self.file_traces,
            trace: Vec::new(),
            children: &self.children,
            package_ids: &self.package_ids,
            deduplicate: !self.options.deduplicate_packages.is_false(),
            seen: BTreeSet::new(),
            packages: BTreeMap::new(),
            redirects: BTreeMap::new(),
            output: Vec::new(),
            missing_names: &self.missing,
            missing: Vec::new(),
            redirected: &self.redirected,
            record_output_to_source: !self.can_use_project_reference_source,
            output_to_source: BTreeMap::new(),
        };
        for root in &self.roots {
            collector.visit(root);
        }
        self.files = collector.output;
        self.trace = collector.trace;
        Collected {
            redirects: collector.redirects,
            missing: collector.missing,
            output_to_source: collector.output_to_source,
        }
    }
    /// port: tsc/internal/compiler/fileloader.go:fileLoader.resolveAutomaticTypeDirectives
    fn load_automatic_types(&mut self) -> Result<(), Error> {
        let names = self.resolver.automatic_type_directive_names()?;
        let directory = if self.options.config_file_path.is_empty() {
            self.cwd.as_bytes().to_vec()
        } else {
            path::directory(self.options.config_file_path.as_bytes())
        };
        let containing = path::combine(&directory, &[tsr_module::INFERRED_TYPES_CONTAINING_FILE]);
        let key = path::to_path(
            &containing,
            self.cwd.as_bytes(),
            self.host.use_case_sensitive_file_names(),
        );
        if !names.is_empty() {
            self.roots.push(IncludeEdge {
                path: key.clone(),
                reason: None,
            });
        }
        for name in names {
            let result = self
                .resolver
                .resolve_type_reference(name.as_bytes(), &containing, ModuleKind::NONE)?
                .clone();
            self.file_traces
                .entry(key.clone())
                .or_default()
                .types
                .extend(self.resolver.take_trace());
            if result.is_resolved() {
                self.link(
                    Some(&key),
                    result.resolved_file_name.as_bytes(),
                    Some(&result.package_id),
                    IncludeReasonData::AutomaticType {
                        name: name.clone(),
                        package_id: result.package_id.clone(),
                    },
                );
                self.load(
                    result.resolved_file_name.as_bytes(),
                    false,
                    false,
                    isize::from(result.is_external_library_import),
                );
            } else {
                let reason = Diagnostic::compiler(
                    if self.options.uses_wildcard_types() {
                        tsr_diagnostics::Entry_point_for_implicit_type_library_0
                    } else {
                        tsr_diagnostics::Entry_point_of_type_library_0_specified_in_compilerOptions
                    },
                    vec![name.clone()],
                );
                let mut because = Diagnostic::compiler(
                    tsr_diagnostics::The_file_is_in_the_program_because_Colon,
                    Vec::new(),
                );
                because.message_chain.push(Arc::new(reason));
                let mut diagnostic = Diagnostic::compiler(
                    tsr_diagnostics::Cannot_find_type_definition_file_for_0,
                    vec![name.clone()],
                );
                diagnostic.message_chain.push(Arc::new(because));
                self.diagnostics.push(diagnostic);
            }
            self.type_resolutions.push(TypeResolution {
                file: key.clone(),
                name,
                mode: ModuleKind::NONE,
                result,
            });
        }
        Ok(())
    }
    /// The failure is the message and arguments of the caller's diagnostic.
    /// port: tsc/internal/compiler/fileloader.go:fileLoader.getSourceFileFromReference
    fn file_reference(
        &mut self,
        name: &[u8],
        reference: &[u8],
        source: Option<&ProgramFile>,
    ) -> Result<Result<Vec<u8>, ReferenceFailure>, Error> {
        let diagnostic_name = JsString::from_bytes(path::normalize_slashes(reference).into_owned());
        let groups = tsr_tsoptions::supported_extensions(&self.options, &[]);
        let quoted_extensions = || {
            let mut text = Vec::new();
            for ext in groups.iter().flatten() {
                if !text.is_empty() {
                    text.extend_from_slice(b", ");
                }
                text.push(b'\'');
                text.extend_from_slice(ext.as_bytes());
                text.push(b'\'');
            }
            JsString::from_bytes(text)
        };
        let allow_non_ts = self.options.allow_non_ts_extensions.is_true();
        let failure = if path::has_extension(name) {
            let canonical = path::canonical(name, self.host.use_case_sensitive_file_names());
            let supported = tsr_tsoptions::supported_extensions_with_json(&self.options, &[])
                .iter()
                .flatten()
                .any(|ext| canonical.as_ref().ends_with(ext.as_bytes()));
            if !allow_non_ts && !supported {
                if matches!(
                    ScriptKind::from_file_name(&canonical),
                    ScriptKind::JS | ScriptKind::JSX
                ) {
                    (tsr_diagnostics::File_0_is_a_JavaScript_file_Did_you_mean_to_enable_the_allowJs_option, vec![diagnostic_name])
                } else {
                    (tsr_diagnostics::File_0_has_an_unsupported_extension_The_only_supported_extensions_are_1, vec![diagnostic_name, quoted_extensions()])
                }
            } else if !self.host.file_exists(name)? {
                (tsr_diagnostics::File_0_not_found, vec![diagnostic_name])
            } else if source.is_some_and(|file| {
                let state = file.bound.view().source_file().expect("retained source");
                path::canonical(
                    state.parse_options().file_name.as_bytes(),
                    self.host.use_case_sensitive_file_names(),
                ) == canonical
            }) {
                (
                    tsr_diagnostics::A_file_cannot_have_a_reference_to_itself,
                    Vec::new(),
                )
            } else {
                return Ok(Ok(name.to_vec()));
            }
        } else if allow_non_ts {
            if self.host.file_exists(name)? {
                return Ok(Ok(name.to_vec()));
            }
            (tsr_diagnostics::File_0_not_found, vec![diagnostic_name])
        } else {
            for ext in &groups[0] {
                let mut candidate = name.to_vec();
                candidate.extend_from_slice(ext.as_bytes());
                if self.host.file_exists(&candidate)? {
                    return Ok(Ok(candidate));
                }
            }
            (
                tsr_diagnostics::Could_not_resolve_the_path_0_with_the_extensions_Colon_1,
                vec![diagnostic_name, quoted_extensions()],
            )
        };
        Ok(Err(failure))
    }
    /// port: tsc/internal/compiler/fileloader.go:fileLoader.pathForLibFile
    fn load_lib(
        &mut self,
        name: &[u8],
        parent: Option<&JsString>,
        reason: IncludeReasonData,
    ) -> Result<(), Error> {
        if let Some(filename) = self.lib_files.get(name) {
            let filename = filename.clone();
            self.link(parent, &filename, None, reason);
            self.load(
                &filename,
                true,
                false,
                parent.map_or(0, |key| self.depths[key]),
            );
            return Ok(());
        }
        let mut filename = path::absolute(
            &path::combine(self.lib_path.as_bytes(), &[name]),
            self.cwd.as_bytes(),
        );
        if !self.skip_resolution && self.options.lib_replacement.is_true() && name != b"lib.d.ts" {
            let components: Vec<_> = name.split(|&c| c == b'.').collect();
            let mut module = b"@typescript/lib-".to_vec();
            if let Some(first) = components.get(1) {
                module.extend_from_slice(first);
            }
            for (index, part) in components.iter().enumerate().skip(2) {
                if part.is_empty() || *part == b"d" {
                    break;
                }
                module.push(if index == 2 { b'/' } else { b'-' });
                module.extend_from_slice(part);
            }
            let directory = if self.options.config_file_path.is_empty() {
                self.cwd.as_bytes().to_vec()
            } else {
                path::directory(self.options.config_file_path.as_bytes())
            };
            let mut synthetic = b"__lib_node_modules_lookup_".to_vec();
            synthetic.extend_from_slice(name);
            synthetic.extend_from_slice(b"__.ts");
            let containing = path::combine(&directory, &[&synthetic]);
            let result = self
                .resolver
                .resolve(&module, &containing, ModuleKind::COMMON_JS)?
                .clone();
            self.library_traces.insert(
                path::to_path(
                    &containing,
                    self.cwd.as_bytes(),
                    self.host.use_case_sensitive_file_names(),
                ),
                self.resolver.take_trace(),
            );
            if result.is_resolved() {
                filename = result.resolved_file_name.as_bytes().to_vec();
            }
            self.resolutions.push(Resolution {
                file: path::to_path(
                    &containing,
                    self.cwd.as_bytes(),
                    self.host.use_case_sensitive_file_names(),
                ),
                name: JsString::from_bytes(module),
                mode: ModuleKind::COMMON_JS,
                result,
            });
        }
        self.lib_files.insert(name.to_vec(), filename.clone());
        self.link(parent, &filename, None, reason);
        self.load(
            &filename,
            true,
            false,
            parent.map_or(0, |key| self.depths[key]),
        );
        Ok(())
    }
    fn load(&mut self, name: &[u8], is_lib: bool, is_root: bool, depth: isize) {
        let key = path::to_path(
            name,
            self.cwd.as_bytes(),
            self.host.use_case_sensitive_file_names(),
        );
        let (is_lib, is_root) = *self.roles.entry(key).or_insert((is_lib, is_root));
        self.pending.push(LoadTask {
            name: name.to_vec(),
            is_lib,
            is_root,
            depth,
            elide: false,
        });
    }
    fn load_worker(
        &mut self,
        name: &[u8],
        is_lib: bool,
        is_root: bool,
        depth: isize,
    ) -> Result<(), Error> {
        let name = path::absolute(name, self.cwd.as_bytes());
        let key = path::to_path(&name, b"", self.host.use_case_sensitive_file_names());
        if self
            .depths
            .get(&key)
            .is_some_and(|&previous| previous <= depth)
        {
            return Ok(());
        }
        self.depths.insert(key.clone(), depth);
        if let Some(children) = self.child_tasks.get(&key) {
            for child in children {
                let mut task = child.clone();
                task.depth += depth;
                self.pending.push(task);
            }
            return Ok(());
        }
        let pending_start = self.pending.len();
        if let Some(output) = self.references.parse_file_redirect(key.as_bytes(), &name)? {
            self.redirect_task(&key, &name, output.as_bytes(), is_lib, depth);
            self.child_tasks
                .insert(key, self.pending_children(pending_start, depth));
            return Ok(());
        }
        let kind = ScriptKind::ensure_from_file_name(&name);
        if !self.options.allow_non_ts_extensions.is_true() {
            let extensions = tsr_tsoptions::supported_extensions_with_json(&self.options, &[]);
            if !extensions
                .iter()
                .flatten()
                .any(|ext| name.ends_with(ext.as_bytes()))
            {
                return Err(Error::Unsupported(
                    "unsupported root/reference extension diagnostics",
                ));
            }
        }
        let meta = metadata::load(
            &mut self.resolver,
            &name,
            &self.options,
            is_lib,
            self.skip_resolution,
        )?;
        let Some(content) = self.host.read_file(&name)? else {
            self.missing
                .entry(key.clone())
                .or_insert_with(|| JsString::from_bytes(name.as_slice()));
            if is_root {
                self.diagnostics.push(missing_root(&name));
                return Ok(());
            }
            if self.redirect_outputs.contains(&key) {
                // A referenced project that was not built: the output is
                // missing without a loader diagnostic.
                return Ok(());
            }
            return Err(Error::Unsupported("missing dependency include diagnostics"));
        };
        let options = SourceFileParseOptions {
            file_name: JsString::from_bytes(name.as_slice()),
            path: key.clone(),
            external_module_indicator_options: metadata::indicator(
                &name,
                self.references
                    .compiler_options_for_file(&self.options, key.as_bytes(), &name)?,
                &meta,
            ),
        };
        let file = self
            .cache
            .acquire(content.text, kind, options, self.counters)?;
        if is_lib {
            self.libs.insert(key.clone());
        }
        self.metadata.insert(key.clone(), meta.clone());
        let view = file.bound.view().ast();
        let state = view.source_file(file.source())?;
        let resolution = self.file_resolution(key.as_bytes(), &name)?;
        if !self.skip_resolution {
            if !self.options.no_resolve.is_true() {
                for (index, reference) in state.referenced_files()?.iter().enumerate() {
                    let target =
                        path::absolute(reference.file_name.as_bytes(), &path::directory(&name));
                    let target = match self.file_reference(
                        &target,
                        reference.file_name.as_bytes(),
                        Some(&file),
                    )? {
                        Ok(target) => Some(target),
                        Err((message, args)) => {
                            self.diagnostics.push(Diagnostic::new(
                                Some(file.source()),
                                reference.loc,
                                message,
                                args,
                            ));
                            None
                        }
                    };
                    if let Some(target) = target {
                        self.link(
                            Some(&key),
                            &target,
                            None,
                            IncludeReasonData::ReferenceFile {
                                file: key.clone(),
                                index,
                            },
                        );
                        self.load(&target, false, false, depth);
                    }
                }
                for (index, reference) in state.type_reference_directives()?.iter().enumerate() {
                    let mode = metadata::type_reference_mode(
                        reference.resolution_mode,
                        &name,
                        &meta,
                        resolution.options(),
                    );
                    let result = self
                        .resolver
                        .resolve_type_reference_with_redirect(
                            reference.file_name.as_bytes(),
                            resolution.containing.as_bytes(),
                            mode,
                            resolution.reference(),
                        )?
                        .clone();
                    self.file_traces
                        .entry(key.clone())
                        .or_default()
                        .types
                        .extend(self.resolver.take_trace());
                    if result.is_resolved() {
                        self.link(
                            Some(&key),
                            result.resolved_file_name.as_bytes(),
                            Some(&result.package_id),
                            IncludeReasonData::TypeReference {
                                file: key.clone(),
                                index,
                            },
                        );
                        self.load(
                            result.resolved_file_name.as_bytes(),
                            false,
                            false,
                            depth + isize::from(result.is_external_library_import),
                        );
                    } else {
                        self.diagnostics.push(Diagnostic::new(
                            Some(file.source()),
                            reference.loc,
                            tsr_diagnostics::Cannot_find_type_definition_file_for_0,
                            vec![reference.file_name.clone()],
                        ));
                    }
                    self.type_resolutions.push(TypeResolution {
                        file: key.clone(),
                        name: reference.file_name.clone(),
                        mode,
                        result,
                    });
                }
            }
            if !self.options.no_lib.is_true() {
                for (index, reference) in state.lib_reference_directives()?.iter().enumerate() {
                    let lower = reference.file_name.as_bytes().to_ascii_lowercase();
                    if let Some(lib) = tsr_tsoptions::lib_file_name(&lower) {
                        self.load_lib(
                            lib.as_bytes(),
                            Some(&key),
                            IncludeReasonData::LibReference {
                                file: key.clone(),
                                index,
                            },
                        )?;
                    } else {
                        return Err(Error::Unsupported("unknown lib directive diagnostic"));
                    }
                }
            }
            let runtime = if matches!(kind, ScriptKind::JS | ScriptKind::JSX | ScriptKind::TSX) {
                metadata::jsx_runtime_import(view, file.source(), resolution.options())?
            } else {
                None
            };
            if resolution.options().import_helpers.is_true()
                && (matches!(kind, ScriptKind::JS | ScriptKind::JSX)
                    || !state.is_declaration_file
                        && (resolution.options().isolated_modules()
                            || state.external_module_indicator.is_some()))
            {
                self.resolve_specifier(
                    &file,
                    &resolution,
                    &key,
                    JsString::from_bytes(b"tslib".as_slice()),
                    metadata::normal_mode(&name, &meta, resolution.options()),
                    (
                        true,
                        -1 - isize::from(runtime.is_some()),
                        Some(SyntheticImport {
                            name: JsString::from_bytes(b"tslib".as_slice()),
                            helpers: true,
                        }),
                    ),
                )?;
            }
            if let Some(runtime) = runtime {
                self.resolve_specifier(
                    &file,
                    &resolution,
                    &key,
                    runtime.clone(),
                    metadata::normal_mode(&name, &meta, resolution.options()),
                    (
                        true,
                        -1,
                        Some(SyntheticImport {
                            name: runtime,
                            helpers: false,
                        }),
                    ),
                )?;
            }
            for (index, &usage) in state.imports()?.iter().enumerate() {
                let usage = usage.ok_or(tsr_arena::Error::InvalidGraph)?;
                self.resolve_import(&file, &resolution, &key, &meta, (usage, index), true)?;
            }
            for &usage in state.module_augmentations()?.iter() {
                let usage = usage.ok_or(tsr_arena::Error::InvalidGraph)?;
                if view.node(usage)?.kind() == tsr_ast::SyntaxKind::StringLiteral {
                    self.resolve_import(&file, &resolution, &key, &meta, (usage, 0), false)?;
                }
            }
        }
        self.child_tasks
            .insert(key, self.pending_children(pending_start, depth));
        self.files.push(file);
        Ok(())
    }
    /// The tasks queued since `start`, stored relative to the parent's depth so
    /// a later, shallower visit can requeue them.
    fn pending_children(&self, start: usize, depth: isize) -> Vec<LoadTask> {
        self.pending[start..]
            .iter()
            .map(|task| {
                let mut task = task.clone();
                task.depth -= depth;
                task
            })
            .collect()
    }
    /// The redirected task keeps the library role and the include reason of the
    /// source task that owns the redirect; depth increase and elision are not
    /// copied, or the depth would be counted twice.
    /// port: tsc/internal/compiler/filesparser.go:parseTask.redirect
    fn redirect_task(
        &mut self,
        source: &JsString,
        source_name: &[u8],
        output: &[u8],
        is_lib: bool,
        depth: isize,
    ) {
        let output = path::normalize(output).into_owned();
        let output_key = path::to_path(
            &output,
            self.cwd.as_bytes(),
            self.host.use_case_sensitive_file_names(),
        );
        let reason = self.first_reasons.get(source).cloned();
        if let Some(reason) = &reason {
            self.first_reasons
                .entry(output_key.clone())
                .or_insert_with(|| reason.clone());
        }
        self.redirected.insert(
            source.clone(),
            RedirectedSource {
                name: JsString::from_bytes(source_name),
                output: output_key.clone(),
                owner: reason.clone(),
                // Only the output's first task is loaded; a later one defers to it.
                output_loaded: !self.roles.contains_key(&output_key),
            },
        );
        self.children
            .entry(source.clone())
            .or_default()
            .push(IncludeEdge {
                path: output_key.clone(),
                reason,
            });
        self.redirect_outputs.insert(output_key);
        self.load(&output, is_lib, false, depth);
    }
    /// The containing file and options a file's references resolve with: its
    /// project reference's, from the source location, when it is that
    /// reference's output.
    fn file_resolution(&self, key: &[u8], name: &[u8]) -> Result<FileResolution, Error> {
        let (redirect, containing) = self.references.redirect_for_resolution(key, name)?;
        let redirect = redirect.map(|index| self.references.config_arc(index));
        Ok(FileResolution {
            name: JsString::from_bytes(name),
            containing,
            config_name: redirect
                .as_ref()
                .map(|config| config.config_name())
                .unwrap_or_default(),
            redirect,
            base: self.options.clone(),
        })
    }
    fn resolve_import(
        &mut self,
        file: &ProgramFile,
        resolution: &FileResolution,
        key: &JsString,
        meta: &SourceFileMetaData,
        usage: (NodeId, usize),
        include: bool,
    ) -> Result<(), Error> {
        let (usage, index) = usage;
        let view = file.bound.view().ast();
        let module_name = view.node_text(usage)?.into_js_string();
        if module_name.as_bytes().is_empty() {
            return Ok(());
        }
        let mode = metadata::usage_mode(
            view,
            resolution.name.as_bytes(),
            meta,
            usage,
            resolution.options(),
        )?;
        let is_js = matches!(
            view.source_file(file.source())?.script_kind,
            ScriptKind::JS | ScriptKind::JSX
        );
        self.resolve_specifier(
            file,
            resolution,
            key,
            module_name,
            mode,
            (
                include && (is_js || view.node(usage)?.flags() & tsr_ast::node_flags::JS_DOC == 0),
                index as isize,
                None,
            ),
        )
    }
    fn resolve_specifier(
        &mut self,
        file: &ProgramFile,
        resolution: &FileResolution,
        key: &JsString,
        module_name: JsString,
        mode: ModuleKind,
        site: (bool, isize, Option<SyntheticImport>),
    ) -> Result<(), Error> {
        let view = file.bound.view().ast();
        let result = self
            .resolver
            .resolve_with_redirect(
                module_name.as_bytes(),
                resolution.containing.as_bytes(),
                mode,
                resolution.reference(),
            )?
            .clone();
        self.file_traces
            .entry(key.clone())
            .or_default()
            .modules
            .extend(self.resolver.take_trace());
        // Don't treat redirected files as JS files.
        let js = result.is_resolved() && {
            let target = result.resolved_file_name.as_bytes();
            matches!(
                ScriptKind::from_file_name(target),
                ScriptKind::JS | ScriptKind::JSX
            ) && self
                .references
                .redirect_parsed_command_line_for_resolution(
                    path::to_path(
                        target,
                        self.cwd.as_bytes(),
                        self.host.use_case_sensitive_file_names(),
                    )
                    .as_bytes(),
                    target,
                )?
                .is_none()
        };
        let options = resolution.options();
        if site.0
            && result.is_resolved()
            && !options.no_resolve.is_true()
            && tsr_module::resolution_diagnostic(
                options,
                &result,
                view.source_file(file.source())?.is_declaration_file,
            )
            .is_none()
        {
            let target = result.resolved_file_name.as_bytes();
            if !js || options.allow_js() {
                self.link(
                    Some(key),
                    target,
                    Some(&result.package_id),
                    IncludeReasonData::Import {
                        file: key.clone(),
                        index: site.1,
                        synthetic: site.2,
                        package_id: result.package_id.clone(),
                    },
                );
                let depth = self.depths[key] + isize::from(result.is_external_library_import);
                let elide = result.is_external_library_import
                    && js
                    && target.windows(14).any(|w| w == b"/node_modules/");
                self.load(target, false, false, depth);
                self.pending.last_mut().expect("queued dependency").elide = elide;
            }
        }
        self.resolutions.push(Resolution {
            file: key.clone(),
            name: module_name,
            mode,
            result,
        });
        Ok(())
    }
}
/// port: tsc/internal/compiler/fileloader.go:fileLoader.getDefaultLibFilePriority
fn lib_priority(name: &[u8], library_path: &[u8]) -> usize {
    let library_path = path::remove_trailing_directory_separator(library_path);
    if !name
        .strip_prefix(library_path)
        .is_some_and(|suffix| suffix.starts_with(b"/"))
    {
        return tsr_tsoptions::LIB_MAP.len() + 2;
    }

    let base = path::base_name(name);
    if matches!(base, b"lib.d.ts" | b"lib.es6.d.ts") {
        return 0;
    }
    let key = base
        .strip_prefix(b"lib.")
        .and_then(|s| s.strip_suffix(b".d.ts"));
    key.and_then(|key| {
        tsr_tsoptions::LIB_MAP
            .iter()
            .position(|(name, _)| name.as_bytes() == key)
    })
    .map_or(tsr_tsoptions::LIB_MAP.len() + 2, |i| i + 1)
}
fn missing_root(name: &[u8]) -> Diagnostic {
    let root = Diagnostic::compiler(
        tsr_diagnostics::Root_file_specified_for_compilation,
        Vec::new(),
    );
    let mut because = Diagnostic::compiler(
        tsr_diagnostics::The_file_is_in_the_program_because_Colon,
        Vec::new(),
    );
    because.message_chain.push(Arc::new(root));
    let mut result = Diagnostic::compiler(
        tsr_diagnostics::File_0_not_found,
        vec![JsString::from_bytes(name)],
    );
    result.message_chain.push(Arc::new(because));
    result
}

#[derive(Default)]
struct FileTraces {
    types: Vec<tsr_module::DiagAndArgs>,
    modules: Vec<tsr_module::DiagAndArgs>,
}
#[derive(Clone)]
struct LoadTask {
    name: Vec<u8>,
    is_lib: bool,
    is_root: bool,
    depth: isize,
    elide: bool,
}
struct IncludeEdge {
    path: JsString,
    reason: Option<Arc<IncludeReason>>,
}
/// A referenced project's source whose task was redirected to its output.
struct RedirectedSource {
    name: JsString,
    output: JsString,
    /// The reason of the source's own task, which the redirected task carries.
    owner: Option<Arc<IncludeReason>>,
    /// Whether the redirected task was the output's first task. Otherwise it
    /// defers to that task, and the source's other reasons are dropped.
    output_loaded: bool,
}
/// Resolution inputs of one loaded file.
struct FileResolution {
    name: JsString,
    containing: JsString,
    config_name: JsString,
    redirect: Option<Arc<ParsedCommandLine>>,
    base: Arc<CompilerOptions>,
}
impl FileResolution {
    fn options(&self) -> &CompilerOptions {
        self.redirect
            .as_ref()
            .map_or(&*self.base, |config| &config.options)
    }
    fn reference(&self) -> Option<tsr_module::ResolvedProjectReference<'_>> {
        self.redirect
            .as_ref()
            .map(|config| tsr_module::ResolvedProjectReference {
                config_name: self.config_name.as_bytes(),
                compiler_options: Some(&config.options),
            })
    }
}
struct Collected {
    redirects: BTreeMap<JsString, JsString>,
    missing: Vec<JsString>,
    output_to_source: BTreeMap<JsString, JsString>,
}
struct Collector<'a> {
    loaded_paths: BTreeSet<JsString>,
    include_reasons: &'a mut BTreeMap<JsString, Vec<Arc<IncludeReason>>>,
    file_traces: &'a mut BTreeMap<JsString, FileTraces>,
    trace: Vec<tsr_module::DiagAndArgs>,
    files: &'a mut BTreeMap<JsString, Arc<ProgramFile>>,
    children: &'a BTreeMap<JsString, Vec<IncludeEdge>>,
    package_ids: &'a BTreeMap<JsString, tsr_module::PackageId>,
    deduplicate: bool,
    seen: BTreeSet<JsString>,
    packages: BTreeMap<tsr_module::PackageId, JsString>,
    redirects: BTreeMap<JsString, JsString>,
    output: Vec<Arc<ProgramFile>>,
    missing_names: &'a BTreeMap<JsString, JsString>,
    missing: Vec<JsString>,
    redirected: &'a BTreeMap<JsString, RedirectedSource>,
    record_output_to_source: bool,
    output_to_source: BTreeMap<JsString, JsString>,
}
impl Collector<'_> {
    fn visit(&mut self, edge: &IncludeEdge) {
        stacker::maybe_grow(128 * 1024, 2 * 1024 * 1024, || self.visit_worker(edge));
    }
    fn visit_worker(&mut self, edge: &IncludeEdge) {
        let key = &edge.path;
        // Source adds each incoming reason before its per-file visited check.
        // Depth retries reuse the original child edges, retaining reason identity.
        let redirected = self.redirected.get(key);
        if let Some(reason) = &edge.reason {
            if let Some(redirected) = redirected {
                // The owning task's reason reaches the output through the
                // redirected task. Any other task adds its reason through the
                // owner, which forwards it only if the redirect was loaded.
                let owner = redirected
                    .owner
                    .as_ref()
                    .is_some_and(|owner| Arc::ptr_eq(owner, reason));
                if !owner && redirected.output_loaded {
                    self.include_reasons
                        .entry(redirected.output.clone())
                        .or_default()
                        .push(reason.clone());
                }
            } else if self.loaded_paths.contains(key) {
                self.include_reasons
                    .entry(key.clone())
                    .or_default()
                    .push(reason.clone());
            }
        }
        if !self.seen.insert(key.clone()) {
            return;
        }
        if let Some(traces) = self.file_traces.remove(key) {
            self.trace.extend(traces.types);
            self.trace.extend(traces.modules);
        }
        if let Some(redirected) = redirected {
            // A redirected source was never parsed. Package deduplication may
            // still alias it to an earlier file, which skips the output.
            if self.deduplicate {
                if let Some(package) = self.package_ids.get(key) {
                    if let Some(first) = self.packages.get(package) {
                        self.redirects.insert(key.clone(), first.clone());
                        return;
                    }
                }
            }
            if let Some(children) = self.children.get(key) {
                for child in children {
                    self.visit(child);
                }
            }
            if self.record_output_to_source {
                self.output_to_source
                    .insert(redirected.output.clone(), redirected.name.clone());
            }
            return;
        }
        let Some(file) = self.files.remove(key) else {
            if key
                .as_bytes()
                .ends_with(tsr_module::INFERRED_TYPES_CONTAINING_FILE)
            {
                if let Some(children) = self.children.get(key) {
                    for child in children {
                        self.visit(child);
                    }
                }
            } else if let Some(name) = self.missing_names.get(key) {
                self.missing.push(name.clone());
            }
            return;
        };
        if self.deduplicate {
            if let Some(package) = self.package_ids.get(key) {
                if let Some(first) = self.packages.get(package) {
                    self.redirects.insert(key.clone(), first.clone());
                    return;
                }
                self.packages.insert(package.clone(), key.clone());
            }
        }
        if let Some(children) = self.children.get(key) {
            for child in children {
                self.visit(child);
            }
        }
        self.output.push(file);
    }
}
