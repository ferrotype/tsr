use crate::include_reason::{
    IncludeExplanations, IncludeReason, IncludeReasonData, ProcessingDiagnostic, SyntheticImport,
};
use crate::project_references::{ProjectReferenceFileMapper, ProjectReferenceParser};
use crate::{metadata, FileCache, ProgramFile, SourceFileMetaData};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{Arc, OnceLock},
};
use tsr_arena::Counters;
use tsr_ast::utilities_middle::new_has_file_name;
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
    /// Collection's processing diagnostics in the pin's order, converted when
    /// the diagnostics are first read.
    processing_diagnostics: Vec<ProcessingDiagnostic>,
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
    /// The loading host's current directory.
    /// port: tsc/internal/compiler/program.go:Program.GetCurrentDirectory
    pub fn current_directory(&self) -> &[u8] {
        self.cwd.as_bytes()
    }
    /// Whether the file at `path` was found searching node_modules.
    /// port: tsc/internal/compiler/program.go:Program.IsSourceFileFromExternalLibrary
    pub fn is_external_library(&self, path: &[u8]) -> bool {
        self.external_paths.contains(path)
    }
    /// port: tsc/internal/compiler/program.go:Program.GetSourceFiles
    /// port: tsc/internal/compiler/program.go:Program.SourceFiles
    pub fn files(&self) -> &[Arc<ProgramFile>] {
        &self.files
    }
    /// port: tsc/internal/compiler/program.go:Program.Options
    pub fn options(&self) -> &CompilerOptions {
        &self.options
    }
    /// The pin's compiler host reduces to the loading file system here.
    /// port: tsc/internal/compiler/program.go:Program.Host
    pub fn host(&self) -> &dyn FileSystem {
        self.host.as_ref()
    }
    /// port: tsc/internal/compiler/program.go:Program.UseCaseSensitiveFileNames
    pub fn use_case_sensitive_file_names(&self) -> bool {
        self.host().use_case_sensitive_file_names()
    }
    /// port: tsc/internal/compiler/program.go:Program.toPath
    pub(crate) fn to_path(&self, file_name: &[u8]) -> JsString {
        path::to_path(
            file_name,
            self.current_directory(),
            self.use_case_sensitive_file_names(),
        )
    }
    /// port: tsc/internal/compiler/program.go:Program.GetSourceFileByPath
    pub fn file(&self, path: &[u8]) -> Option<&ProgramFile> {
        self.by_path.get(path).map(|&i| self.files[i].as_ref())
    }
    /// The file loaded for `file_name`, which may be relative or spelled in
    /// another casing on a case-insensitive host.
    /// port: tsc/internal/compiler/program.go:Program.GetSourceFile
    pub fn source_file(&self, file_name: &[u8]) -> Option<&ProgramFile> {
        self.file(self.to_path(file_name).as_bytes())
    }
    pub fn metadata(&self, path: &[u8]) -> Option<&SourceFileMetaData> {
        self.metadata.get(path)
    }
    /// Whether the file at `path` is a default library.
    /// port: tsc/internal/compiler/program.go:Program.IsSourceFileDefaultLibrary
    pub fn is_lib(&self, path: &[u8]) -> bool {
        self.libs.contains(path)
    }
    /// The configuration's syntax diagnostics followed by its option errors.
    /// port: tsc/internal/compiler/program.go:Program.GetConfigFileParsingDiagnostics
    pub fn config_file_parsing_diagnostics(&self) -> Vec<Diagnostic> {
        self.config.config_file_parsing_diagnostics()
    }
    /// The pin's program-like unwrapping accessor: a program is its own program.
    /// port: tsc/internal/compiler/program.go:Program.Program
    pub fn program(&self) -> &Program {
        self
    }
    /// A file's implied module format for emit, under the options it was
    /// loaded with; an unknown path has empty metadata.
    /// port: tsc/internal/compiler/program.go:Program.GetImpliedNodeFormatForEmit
    pub fn implied_node_format_for_emit(&self, path: &[u8], file_name: &[u8]) -> ModuleKind {
        let empty = SourceFileMetaData::default();
        metadata::implied_for_emit(
            file_name,
            self.options_for_file(path, file_name).emit_module_kind(),
            self.metadata(path).unwrap_or(&empty),
        )
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
        let file = new_has_file_name(JsString::from_bytes(file_name), self.to_path(file_name));
        self.references.published_parse_file_redirect(file.path())
    }
    /// The loader's own include diagnostics, computed at first use as the pin's
    /// include processor computes them. Processing diagnostics are converted
    /// then (a root that did not resolve is explained with its root reason
    /// naming the tsconfig spec that listed it), and the collection is ordered
    /// again.
    pub(crate) fn loader_include_diagnostics(&self) -> Result<&[Diagnostic], tsr_arena::Error> {
        self.include_diagnostics
            .get_or_init(|| {
                if self.processing_diagnostics.is_empty() {
                    return Ok(self.loader_diagnostics.clone());
                }
                let mut diagnostics = self.loader_diagnostics.clone();
                diagnostics.extend(self.processing_include_diagnostics()?);
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
    /// Collection's processing diagnostics, converted in collection order.
    pub(crate) fn processing_include_diagnostics(
        &self,
    ) -> Result<Vec<Diagnostic>, tsr_arena::Error> {
        self.processing_diagnostics
            .iter()
            .map(|diagnostic| diagnostic.to_diagnostic(self))
            .collect()
    }
    /// The loader's include diagnostics that are not processing diagnostics:
    /// resolution, reference and root failures.
    pub(crate) fn loader_resolution_diagnostics(&self) -> &[Diagnostic] {
        &self.loader_diagnostics
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
    /// Each path's processing diagnostics in the order its load made them;
    /// collection publishes those of the paths it keeps.
    processing: BTreeMap<JsString, Vec<ProcessingDiagnostic>>,
}
type ReferenceFailure = (&'static tsr_diagnostics::Message, Vec<JsString>);
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
            processing: BTreeMap::new(),
        })
    }
    fn run(mut self) -> Result<Program, Error> {
        let roots = std::mem::take(&mut self.config.root_file_names);
        // port: tsc/internal/compiler/fileloader.go:fileLoader.addRootFileTask
        for (index, root) in roots.iter().enumerate() {
            let absolute = path::absolute(root.as_bytes(), self.cwd.as_bytes());
            match self.file_reference(&absolute, root.as_bytes(), None)? {
                Ok(name) => {
                    self.link(None, &name, None, IncludeReasonData::Root { index });
                    self.load(&name, false, true, 0, false);
                }
                Err((message, args)) => {
                    // The failed root keeps its task: collection records its
                    // reason, lists it as missing in root order and reports
                    // its processing diagnostic, which belongs to this
                    // spelling's task and not to its path.
                    let key = self.to_path(&absolute);
                    let reason = Arc::new(IncludeReason::new(IncludeReasonData::Root { index }));
                    self.roots.push(IncludeEdge {
                        path: key.clone(),
                        name: JsString::from_bytes(absolute.as_slice()),
                        reason: Some(reason.clone()),
                        failure: Some(ProcessingDiagnostic::ExplainingFileInclude {
                            file: JsString::default(),
                            reason: Some(reason),
                            message,
                            args,
                        }),
                    });
                    self.missing
                        .entry(key)
                        .or_insert_with(|| JsString::from_bytes(absolute));
                }
            }
        }
        // port: tsc/internal/compiler/fileloader.go:fileLoader.addRootTask
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
        // port: tsc/internal/compiler/filesparser.go:filesParser.parse
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
        self.parse_first_casings(&collected.renamed)?;
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
            host_trace(&mut self.trace, traces);
        }

        // port: tsc/internal/compiler/fileloader.go:fileLoader.sortLibs
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
            processing_diagnostics: collected.processing,
            include_diagnostics: OnceLock::new(),
            trace: self.trace,
        };
        program.option_verification = crate::verify_compiler_options(&program)?;
        Ok(program)
    }
    /// port: tsc/internal/compiler/fileloader.go:fileLoader.toPath
    fn to_path(&self, file_name: &[u8]) -> JsString {
        path::to_path(
            file_name,
            self.cwd.as_bytes(),
            self.host.use_case_sensitive_file_names(),
        )
    }
    fn link(
        &mut self,
        parent: Option<&JsString>,
        name: &[u8],
        package: Option<&tsr_module::PackageId>,
        reason: IncludeReasonData,
    ) {
        let key = self.to_path(name);
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
            name: JsString::from_bytes(path::absolute(name, self.cwd.as_bytes())),
            reason: Some(reason),
            failure: None,
        };
        if let Some(parent) = parent {
            self.children.entry(parent.clone()).or_default().push(edge);
        } else {
            self.roots.push(edge);
        }
    }
    /// The collectFiles walk: package identity redirects happen before the
    /// subtree walk, postorder source publication after it. Parsing and
    /// collection have separate ownership: unselected duplicate files are dropped.
    /// This is filesParser.getProcessedFiles; its walk is the loop over the roots.
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
            force_consistent_casing: !self
                .options
                .force_consistent_casing_in_file_names
                .is_false(),
            current_directory: self.cwd.as_bytes(),
            seen_by_name_ignore_case: self
                .host
                .use_case_sensitive_file_names()
                .then(BTreeMap::new),
            seen: BTreeMap::new(),
            packages: BTreeMap::new(),
            redirects: BTreeMap::new(),
            output: Vec::new(),
            missing_names: &self.missing,
            missing: Vec::new(),
            redirected: &self.redirected,
            record_output_to_source: !self.can_use_project_reference_source,
            output_to_source: BTreeMap::new(),
            pending_processing: &self.processing,
            processing: Vec::new(),
            renamed: Vec::new(),
        };
        // port: tsc/internal/compiler/filesparser.go:filesParser.getProcessedFiles
        for root in &self.roots {
            collector.visit(root);
        }
        self.files = collector.output;
        self.trace = collector.trace;
        Collected {
            redirects: collector.redirects,
            missing: collector.missing,
            output_to_source: collector.output_to_source,
            processing: collector.processing,
            renamed: collector.renamed,
        }
    }
    /// A path first collected under another spelling than the one that was
    /// parsed is parsed again under that spelling, as the pin parses every
    /// spelling of a path. Only a file without dependencies can be: another
    /// spelling's dependencies would resolve from another directory spelling.
    /// A failed `/// <reference path>` counts as a dependency: its diagnostic
    /// belongs to the replaced parse and would be dropped with it, while the
    /// pin reports it against the kept spelling's own parse.
    fn parse_first_casings(&mut self, renamed: &[(JsString, JsString)]) -> Result<(), Error> {
        for (key, name) in renamed {
            let index = self
                .files
                .iter()
                .position(|loaded| {
                    loaded
                        .bound
                        .view()
                        .source_file()
                        .expect("retained source")
                        .parse_options()
                        .path
                        == *key
                })
                .expect("renamed file was collected");
            let replaced = self.files[index].source();
            let leaf = !self.children.contains_key(key)
                && self.child_tasks.get(key).is_none_or(Vec::is_empty)
                && !self.processing.contains_key(key)
                && !self.resolutions.iter().any(|r| &r.file == key)
                && !self.type_resolutions.iter().any(|r| &r.file == key)
                && !self.diagnostics.iter().any(|d| d.file == Some(replaced));
            if !leaf {
                return Err(Error::Unsupported(
                    "file-name casing variant with its own dependencies",
                ));
            }
            let is_lib = self.libs.contains(key);
            let meta = metadata::load(
                &mut self.resolver,
                name.as_bytes(),
                &self.options,
                is_lib,
                self.skip_resolution,
            )?;
            let kind = ScriptKind::ensure_from_file_name(name.as_bytes());
            let file = self
                .parse_source_file(name.as_bytes(), key, &meta, kind)?
                .ok_or(Error::Unsupported("file-name casing variant without text"))?;
            self.metadata.insert(key.clone(), meta);
            self.files[index] = file;
        }
        Ok(())
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
        let key = self.to_path(&containing);
        // port: tsc/internal/compiler/fileloader.go:fileLoader.addAutomaticTypeDirectiveTasks
        if !names.is_empty() {
            self.roots.push(IncludeEdge {
                path: key.clone(),
                name: JsString::from_bytes(containing.as_slice()),
                reason: None,
                failure: None,
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
                // port: tsc/internal/compiler/filesparser.go:parseTask.loadAutomaticTypeDirectives
                self.add_sub_task(
                    &key,
                    result.resolved_file_name.as_bytes(),
                    Some(&result.package_id),
                    IncludeReasonData::AutomaticType {
                        name: name.clone(),
                        package_id: result.package_id.clone(),
                    },
                    isize::from(result.is_external_library_import),
                    false,
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
    /// The canonical file name's extension is one the options load.
    /// port: tsc/internal/compiler/fileloader.go:fileLoader.isSupportedExtension
    fn is_supported_extension(&self, canonical_file_name: &[u8]) -> bool {
        tsr_tsoptions::supported_extensions_with_json(&self.options, &[])
            .iter()
            .any(|group| {
                group
                    .iter()
                    .any(|ext| canonical_file_name.ends_with(ext.as_bytes()))
            })
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
            if !allow_non_ts && !self.is_supported_extension(&canonical) {
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
    /// `resolveLibrary`; the marker is on its statement site.
    fn resolve_library(
        &mut self,
        library_name: &[u8],
        resolve_from: &[u8],
    ) -> Result<(ResolvedModule, Vec<tsr_module::DiagAndArgs>), Error> {
        let mut resolution = ResolvedModule::default();
        // `p.resolver.ResolveModuleName(libraryName, resolveFrom, CommonJS, nil)`:
        // the skip-statement site leaves the library unresolved, so the bundled
        // file stays in the program instead of the package's replacement.
        // port: tsc/internal/compiler/fileloader.go:fileLoader.resolveLibrary
        resolution.clone_from(self.resolver.resolve(
            library_name,
            resolve_from,
            ModuleKind::COMMON_JS,
        )?);
        Ok((resolution, self.resolver.take_trace()))
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
                false,
            );
            return Ok(());
        }
        let mut filename = path::absolute(
            &path::combine(self.lib_path.as_bytes(), &[name]),
            self.cwd.as_bytes(),
        );
        if !self.skip_resolution && self.options.lib_replacement.is_true() && name != b"lib.d.ts" {
            let library_name = library_name_from_lib_file_name(name);
            let resolve_from =
                inferred_library_name_resolve_from(&self.options, self.cwd.as_bytes(), name);
            let (result, trace) = self.resolve_library(&library_name, &resolve_from)?;
            let key = self.to_path(&resolve_from);
            self.library_traces.insert(key.clone(), trace);
            if result.is_resolved() {
                filename = result.resolved_file_name.as_bytes().to_vec();
            }
            self.resolutions.push(Resolution {
                file: key,
                name: JsString::from_bytes(library_name),
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
            false,
        );
        Ok(())
    }
    fn load(&mut self, name: &[u8], is_lib: bool, is_root: bool, depth: isize, elide: bool) {
        let key = self.to_path(name);
        let (is_lib, is_root) = *self.roles.entry(key).or_insert((is_lib, is_root));
        self.pending.push(LoadTask {
            name: name.to_vec(),
            is_lib,
            is_root,
            depth,
            elide,
        });
    }
    /// A dependency of the file at `parent`: its include edge and its task.
    /// port: tsc/internal/compiler/filesparser.go:parseTask.addSubTask
    fn add_sub_task(
        &mut self,
        parent: &JsString,
        file_name: &[u8],
        package: Option<&tsr_module::PackageId>,
        reason: IncludeReasonData,
        depth: isize,
        elide: bool,
    ) {
        self.link(Some(parent), file_name, package, reason);
        self.load(file_name, false, false, depth, elide);
    }
    /// port: tsc/internal/compiler/filesparser.go:parseTask.load
    fn load_worker(
        &mut self,
        name: &[u8],
        is_lib: bool,
        is_root: bool,
        depth: isize,
    ) -> Result<(), Error> {
        let name = path::absolute(name, self.cwd.as_bytes());
        let key = self.to_path(&name);
        // port: tsc/internal/compiler/filesparser.go:filesParser.start
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
        if path::has_extension(&name) && !self.options.allow_non_ts_extensions.is_true() {
            let canonical = path::canonical(&name, self.host.use_case_sensitive_file_names());
            if !self.is_supported_extension(&canonical) {
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
        let Some(file) = self.parse_source_file(&name, &key, &meta, kind)? else {
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
                    match self.resolve_tripleslash_path_reference(
                        reference.file_name.as_bytes(),
                        &name,
                        &file,
                    )? {
                        Ok(target) => self.add_sub_task(
                            &key,
                            &target,
                            None,
                            IncludeReasonData::ReferenceFile {
                                file: key.clone(),
                                index,
                            },
                            depth,
                            false,
                        ),
                        Err((message, args)) => {
                            self.diagnostics.push(Diagnostic::new(
                                Some(file.source()),
                                reference.loc,
                                message,
                                args,
                            ));
                        }
                    }
                }
                self.resolve_type_reference_directives(
                    &file,
                    &key,
                    &name,
                    &meta,
                    &resolution,
                    depth,
                )?;
            }
            if !self.options.no_lib.is_true() {
                for (index, reference) in state.lib_reference_directives()?.iter().enumerate() {
                    let lower = reference.file_name.as_bytes().to_ascii_lowercase();
                    let reason = IncludeReasonData::LibReference {
                        file: key.clone(),
                        index,
                    };
                    if let Some(lib) = tsr_tsoptions::lib_file_name(&lower) {
                        self.load_lib(lib.as_bytes(), Some(&key), reason)?;
                    } else {
                        self.processing.entry(key.clone()).or_default().push(
                            ProcessingDiagnostic::UnknownReference(Arc::new(IncludeReason::new(
                                reason,
                            ))),
                        );
                    }
                }
            }
            self.resolve_imports_and_module_augmentations(&file, &key, &meta, &resolution, kind)?;
        }
        self.child_tasks
            .insert(key, self.pending_children(pending_start, depth));
        self.files.push(file);
        Ok(())
    }
    /// The file's parse options, then the host's parse; `None` when the host
    /// has no text for it.
    /// port: tsc/internal/compiler/fileloader.go:fileLoader.parseSourceFile
    fn parse_source_file(
        &mut self,
        name: &[u8],
        key: &JsString,
        meta: &SourceFileMetaData,
        kind: ScriptKind,
    ) -> Result<Option<Arc<ProgramFile>>, Error> {
        let options = SourceFileParseOptions {
            file_name: JsString::from_bytes(name),
            path: key.clone(),
            external_module_indicator_options: metadata::indicator(
                name,
                self.references
                    .compiler_options_for_file(&self.options, key.as_bytes(), name)?,
                meta,
            ),
        };
        self.get_source_file(options, kind)
    }
    /// Read the file and parse it through the retained file cache.
    /// port: tsc/internal/compiler/host.go:compilerHost.GetSourceFile
    fn get_source_file(
        &mut self,
        options: SourceFileParseOptions,
        kind: ScriptKind,
    ) -> Result<Option<Arc<ProgramFile>>, Error> {
        let Some(content) = self.host.read_file(options.file_name.as_bytes())? else {
            return Ok(None);
        };
        Ok(Some(self.cache.acquire(
            content.text,
            kind,
            options,
            self.counters,
        )?))
    }
    /// A `/// <reference path>`: its absolute file name, or the diagnostic's
    /// message and arguments (`resolveTripleslashPathReference`; the marker is
    /// on its statement site).
    fn resolve_tripleslash_path_reference(
        &mut self,
        module_name: &[u8],
        containing_file: &[u8],
        source: &ProgramFile,
    ) -> Result<Result<Vec<u8>, ReferenceFailure>, Error> {
        let base_path = path::directory(containing_file);
        let mut referenced = std::borrow::Cow::Borrowed(module_name);
        // `if !IsRootedDiskPath(moduleName) { referencedFileName = CombinePaths(basePath, moduleName) }`:
        // the negated-condition site leaves a relative reference uncombined, so
        // the referenced file is reported missing instead of loaded.
        // port: tsc/internal/compiler/fileloader.go:fileLoader.resolveTripleslashPathReference
        if !path::is_rooted_disk_path(module_name) {
            referenced = std::borrow::Cow::Owned(path::combine(&base_path, &[module_name]));
        }
        let target = tsr_core::path::normalize(&referenced).into_owned();
        self.file_reference(&target, module_name, Some(source))
    }
    /// port: tsc/internal/compiler/fileloader.go:fileLoader.resolveTypeReferenceDirectives
    fn resolve_type_reference_directives(
        &mut self,
        file: &ProgramFile,
        key: &JsString,
        name: &[u8],
        meta: &SourceFileMetaData,
        resolution: &FileResolution,
        depth: isize,
    ) -> Result<(), Error> {
        let view = file.bound.view().ast();
        let state = view.source_file(file.source())?;
        for (index, reference) in state.type_reference_directives()?.iter().enumerate() {
            let mode = metadata::type_reference_mode(
                reference.resolution_mode,
                name,
                meta,
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
            let reason = IncludeReasonData::TypeReference {
                file: key.clone(),
                index,
            };
            if result.is_resolved() {
                self.add_sub_task(
                    key,
                    result.resolved_file_name.as_bytes(),
                    Some(&result.package_id),
                    reason,
                    depth + isize::from(result.is_external_library_import),
                    false,
                );
            } else {
                self.processing.entry(key.clone()).or_default().push(
                    ProcessingDiagnostic::UnknownReference(Arc::new(IncludeReason::new(reason))),
                );
            }
            self.type_resolutions.push(TypeResolution {
                file: key.clone(),
                name: reference.file_name.clone(),
                mode,
                result,
            });
        }
        Ok(())
    }
    /// The synthetic importHelpers and JSX runtime imports, then the file's
    /// imports and string-named module augmentations.
    /// port: tsc/internal/compiler/fileloader.go:fileLoader.resolveImportsAndModuleAugmentations
    fn resolve_imports_and_module_augmentations(
        &mut self,
        file: &ProgramFile,
        key: &JsString,
        meta: &SourceFileMetaData,
        resolution: &FileResolution,
        kind: ScriptKind,
    ) -> Result<(), Error> {
        let view = file.bound.view().ast();
        let state = view.source_file(file.source())?;
        let name = resolution.name.clone();
        let runtime = if matches!(kind, ScriptKind::JS | ScriptKind::JSX | ScriptKind::TSX) {
            metadata::jsx_runtime_import(
                metadata::jsx_implicit_import_base(view, file.source(), resolution.options())?
                    .as_bytes(),
                resolution.options(),
            )
        } else {
            JsString::default()
        };
        if resolution.options().import_helpers.is_true()
            && (matches!(kind, ScriptKind::JS | ScriptKind::JSX)
                || !state.is_declaration_file
                    && (resolution.options().isolated_modules()
                        || state.external_module_indicator.is_some()))
        {
            self.resolve_specifier(
                file,
                resolution,
                key,
                JsString::from_bytes(b"tslib".as_slice()),
                metadata::normal_mode(name.as_bytes(), meta, resolution.options()),
                (
                    true,
                    -1 - isize::from(!runtime.is_empty()),
                    Some(SyntheticImport {
                        name: JsString::from_bytes(b"tslib".as_slice()),
                        helpers: true,
                    }),
                ),
            )?;
        }
        if !runtime.is_empty() {
            self.resolve_specifier(
                file,
                resolution,
                key,
                runtime.clone(),
                metadata::normal_mode(name.as_bytes(), meta, resolution.options()),
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
            self.resolve_import(file, resolution, key, meta, (usage, index), true)?;
        }
        for &usage in state.module_augmentations()?.iter() {
            let usage = usage.ok_or(tsr_arena::Error::InvalidGraph)?;
            if view.node(usage)?.kind() == tsr_ast::SyntaxKind::StringLiteral {
                self.resolve_import(file, resolution, key, meta, (usage, 0), false)?;
            }
        }
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
        let output_key = self.to_path(&output);
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
                name: JsString::from_bytes(output.as_slice()),
                reason,
                failure: None,
            });
        self.redirect_outputs.insert(output_key);
        self.load(&output, is_lib, false, depth, false);
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
            ) && {
                let target = new_has_file_name(JsString::from_bytes(target), self.to_path(target));
                self.references
                    .redirect_parsed_command_line_for_resolution(target.path(), target.file_name())?
                    .is_none()
            }
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
                let depth = self.depths[key] + isize::from(result.is_external_library_import);
                let elide = result.is_external_library_import
                    && js
                    && target.windows(14).any(|w| w == b"/node_modules/");
                // The include reason keeps the specifier's resolved package,
                // which the pin looks up again when it explains the reason.
                // port: tsc/internal/compiler/program.go:Program.GetResolvedModuleFromModuleSpecifier
                let package_id = result.package_id.clone();
                self.add_sub_task(
                    key,
                    target,
                    Some(&result.package_id),
                    IncludeReasonData::Import {
                        file: key.clone(),
                        index: site.1,
                        synthetic: site.2,
                        package_id,
                    },
                    depth,
                    elide,
                );
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
/// The host's trace callback: the program keeps the log, in emission order.
/// port: tsc/internal/compiler/host.go:compilerHost.Trace
fn host_trace(log: &mut Vec<tsr_module::DiagAndArgs>, traces: Vec<tsr_module::DiagAndArgs>) {
    log.extend(traces);
}
/// `lib.dom.iterable.d.ts` is `@typescript/lib-dom/iterable`, and
/// `lib.es2015.symbol.wellknown.d.ts` is `@typescript/lib-es2015/symbol-wellknown`.
/// port: tsc/internal/compiler/fileloader.go:getLibraryNameFromLibFileName
fn library_name_from_lib_file_name(lib_file_name: &[u8]) -> Vec<u8> {
    let components: Vec<_> = lib_file_name.split(|&c| c == b'.').collect();
    let mut path = b"@typescript/lib-".to_vec();
    if let Some(first) = components.get(1) {
        path.extend_from_slice(first);
    }
    let mut i = 2;
    while i < components.len() && !components[i].is_empty() && components[i] != b"d" {
        path.push(if i == 2 { b'/' } else { b'-' });
        path.extend_from_slice(components[i]);
        i += 1;
    }
    path
}
/// The synthetic file a replacement library resolves from: beside the
/// configuration file, or in the current directory.
/// port: tsc/internal/compiler/fileloader.go:getInferredLibraryNameResolveFrom
fn inferred_library_name_resolve_from(
    options: &CompilerOptions,
    current_directory: &[u8],
    lib_file_name: &[u8],
) -> Vec<u8> {
    let containing_directory = if options.config_file_path.is_empty() {
        current_directory.to_vec()
    } else {
        path::directory(options.config_file_path.as_bytes())
    };
    let mut synthetic = b"__lib_node_modules_lookup_".to_vec();
    synthetic.extend_from_slice(lib_file_name);
    synthetic.extend_from_slice(b"__.ts");
    path::combine(&containing_directory, &[&synthetic])
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
    /// The normalized file name the task was made for; one path may be spelled
    /// in several casings.
    name: JsString,
    reason: Option<Arc<IncludeReason>>,
    /// A root task's lookup failure. The pin keeps it on the task, one per
    /// spelling, and collection reports only the task it reaches first; a
    /// later spelling adds only its casing diagnostic.
    failure: Option<ProcessingDiagnostic>,
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
    processing: Vec<ProcessingDiagnostic>,
    /// Paths first collected under another spelling than the parsed one.
    renamed: Vec<(JsString, JsString)>,
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
    force_consistent_casing: bool,
    current_directory: &'a [u8],
    /// On a case-sensitive host, the first path and name of each lowercased path.
    seen_by_name_ignore_case: Option<BTreeMap<Vec<u8>, (JsString, JsString)>>,
    /// The name each path was first collected under.
    seen: BTreeMap<JsString, JsString>,
    packages: BTreeMap<tsr_module::PackageId, JsString>,
    redirects: BTreeMap<JsString, JsString>,
    output: Vec<Arc<ProgramFile>>,
    missing_names: &'a BTreeMap<JsString, JsString>,
    missing: Vec<JsString>,
    redirected: &'a BTreeMap<JsString, RedirectedSource>,
    record_output_to_source: bool,
    output_to_source: BTreeMap<JsString, JsString>,
    pending_processing: &'a BTreeMap<JsString, Vec<ProcessingDiagnostic>>,
    processing: Vec<ProcessingDiagnostic>,
    renamed: Vec<(JsString, JsString)>,
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
        // port: tsc/internal/compiler/filesparser.go:filesParser.addIncludeReason
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
        let inferred = key
            .as_bytes()
            .ends_with(tsr_module::INFERRED_TYPES_CONTAINING_FILE);
        if redirected.is_none() && !inferred && !self.loaded_paths.contains(key) {
            // A task that was never loaded, such as one elided by depth.
            return;
        }
        if let Some(checked) = self.seen.get(key) {
            if self.force_consistent_casing
                && path::normalized_absolute_path_without_root(
                    checked.as_bytes(),
                    self.current_directory,
                ) != path::normalized_absolute_path_without_root(
                    edge.name.as_bytes(),
                    self.current_directory,
                )
            {
                let checked = checked.clone();
                self.add_processing_diagnostics_for_file_casing(
                    key.clone(),
                    checked,
                    edge.name.clone(),
                    edge.reason.clone(),
                );
            }
            return;
        }
        self.seen.insert(key.clone(), edge.name.clone());
        if let Some(by_name) = &mut self.seen_by_name_ignore_case {
            let lower = path::file_name_lower_case(key.as_bytes()).into_owned();
            if let Some((path, name)) = by_name.get(&lower) {
                let (path, name) = (path.clone(), name.clone());
                self.add_processing_diagnostics_for_file_casing(
                    path,
                    name,
                    edge.name.clone(),
                    edge.reason.clone(),
                );
            } else {
                by_name.insert(lower, (key.clone(), edge.name.clone()));
            }
        }
        if let Some(traces) = self.file_traces.remove(key) {
            host_trace(&mut self.trace, traces.types);
            host_trace(&mut self.trace, traces.modules);
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
            if inferred {
                if let Some(children) = self.children.get(key) {
                    for child in children {
                        self.visit(child);
                    }
                }
            } else if let Some(name) = self.missing_names.get(key) {
                self.processing.extend(edge.failure.iter().cloned());
                self.emit_processing(key);
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
        self.emit_processing(key);
        let parsed = file
            .bound()
            .view()
            .source_file()
            .expect("retained source")
            .parse_options()
            .file_name
            .clone();
        if parsed != edge.name {
            self.renamed.push((key.clone(), edge.name.clone()));
        }
        self.output.push(file);
    }
    fn emit_processing(&mut self, key: &JsString) {
        if let Some(diagnostics) = self.pending_processing.get(key) {
            self.processing.extend(diagnostics.iter().cloned());
        }
    }
    /// port: tsc/internal/compiler/includeprocessor.go:includeProcessor.addProcessingDiagnosticsForFileCasing
    fn add_processing_diagnostics_for_file_casing(
        &mut self,
        file: JsString,
        existing_casing: JsString,
        current_casing: JsString,
        reason: Option<Arc<IncludeReason>>,
    ) {
        let (message, args) = if !reason.as_deref().is_some_and(IncludeReason::is_referenced)
            && self
                .include_reasons
                .get(&file)
                .is_some_and(|reasons| reasons.iter().any(|reason| reason.is_referenced()))
        {
            (
                tsr_diagnostics::Already_included_file_name_0_differs_from_file_name_1_only_in_casing,
                vec![existing_casing, current_casing],
            )
        } else {
            (
                tsr_diagnostics::File_name_0_differs_from_already_included_file_name_1_only_in_casing,
                vec![current_casing, existing_casing],
            )
        };
        self.processing
            .push(ProcessingDiagnostic::ExplainingFileInclude {
                file,
                reason,
                message,
                args,
            });
    }
}
