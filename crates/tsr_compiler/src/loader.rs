use crate::include_reason::{
    IncludeExplanations, IncludeReason, IncludeReasonData, SyntheticImport,
};
use crate::{metadata, FileCache, ProgramFile, SourceFileMetaData};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};
use tsr_arena::Counters;
use tsr_ast::{Diagnostic, NodeId, SourceFileParseOptions};
use tsr_core::{CompilerOptions, ModuleKind, ScriptKind};
use tsr_jsstring::JsString;
use tsr_module::{ResolvedModule, ResolvedTypeReferenceDirective, Resolver};
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
/// bind and option-verification results. Project-reference loading, content-mapper
/// execution and checker construction remain explicit unsupported boundaries.
pub struct Program {
    pub(crate) owners: crate::resolver_host::OwnerIndex,
    pub(crate) include_reasons: BTreeMap<JsString, Vec<Arc<IncludeReason>>>,
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
    include_diagnostics: Vec<Diagnostic>,
    trace: Vec<tsr_module::DiagAndArgs>,
}
impl Program {
    pub fn load(
        options: ProgramOptions,
        cache: &mut FileCache,
        counters: &Counters,
    ) -> Result<Self, Error> {
        Loader::new(options, cache, counters)?.run()
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
    pub fn files(&self) -> &[Arc<ProgramFile>] {
        &self.files
    }
    pub fn options(&self) -> &CompilerOptions {
        &self.options
    }
    pub fn host(&self) -> &dyn FileSystem {
        self.host.as_ref()
    }
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
    pub fn include_diagnostics(&self) -> &[Diagnostic] {
        &self.include_diagnostics
    }
    /// The verifier runs once during construction. Reading its raw writes does
    /// not force the source include processor's lazy explanations.
    pub fn option_verification(&self) -> &crate::OptionVerification {
        &self.option_verification
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
    missing: Vec<JsString>,
    resolutions: Vec<Resolution>,
    type_resolutions: Vec<TypeResolution>,
    diagnostics: Vec<Diagnostic>,
}
impl<'a> Loader<'a> {
    fn new(
        input: ProgramOptions,
        cache: &'a mut FileCache,
        counters: &'a Counters,
    ) -> Result<Self, Error> {
        if input
            .config
            .project_references
            .as_ref()
            .is_some_and(|references| !references.is_empty())
        {
            return Err(Error::Unsupported("project-reference program loading"));
        }
        if input
            .config
            .content_mappers
            .as_ref()
            .is_some_and(|mappers| !mappers.is_empty())
        {
            return Err(Error::Unsupported("content-mapper execution"));
        }
        let options = Arc::new(input.config.options.clone());
        let resolver = Resolver::new(
            input.host.clone(),
            options.clone(),
            input.current_directory.as_bytes(),
        )?;
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
            lib_path: input.default_library_path,
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
            missing: Vec::new(),
            resolutions: Vec::new(),
            type_resolutions: Vec::new(),
            diagnostics: Vec::new(),
        })
    }
    fn run(mut self) -> Result<Program, Error> {
        let roots = std::mem::take(&mut self.config.root_file_names);
        for (index, root) in roots.iter().enumerate() {
            let absolute = path::absolute(root.as_bytes(), self.cwd.as_bytes());
            if let Some(name) = self.file_reference(&absolute, root.as_bytes(), None)? {
                self.link(None, &name, None, IncludeReasonData::Root { index });
                self.load(&name, false, true, 0);
            } else {
                self.missing.push(JsString::from_bytes(absolute));
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
            if task.elide
                && task.depth
                    > self
                        .options
                        .max_node_module_js_depth
                        .as_ref()
                        .map_or(0, tsr_core::shared::SharedValue::get)
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
        let redirects = self.collect_files();
        let redirect_file_names = redirects
            .keys()
            .map(|key| (key.clone(), loaded_names[key].clone()))
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
        let mut program = Program {
            include_reasons: self.include_reasons,
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
            missing: self.missing,
            resolutions: self.resolutions,
            type_resolutions: self.type_resolutions,
            include_diagnostics: self.diagnostics,
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
        let edge = IncludeEdge {
            path: key,
            reason: Some(Arc::new(IncludeReason::new(reason))),
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
    fn collect_files(&mut self) -> BTreeMap<JsString, JsString> {
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
            loaded_paths: files.keys().cloned().collect(),
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
        };
        for root in &self.roots {
            collector.visit(root);
        }
        self.files = collector.output;
        self.trace = collector.trace;
        collector.redirects
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
    /// port: tsc/internal/compiler/fileloader.go:fileLoader.getSourceFileFromReference
    fn file_reference(
        &mut self,
        name: &[u8],
        reference: &[u8],
        source: Option<(&ProgramFile, tsr_core::TextRange)>,
    ) -> Result<Option<Vec<u8>>, Error> {
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
            } else if source.is_some_and(|(file, _)| {
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
                return Ok(Some(name.to_vec()));
            }
        } else if allow_non_ts {
            if self.host.file_exists(name)? {
                return Ok(Some(name.to_vec()));
            }
            (tsr_diagnostics::File_0_not_found, vec![diagnostic_name])
        } else {
            for ext in &groups[0] {
                let mut candidate = name.to_vec();
                candidate.extend_from_slice(ext.as_bytes());
                if self.host.file_exists(&candidate)? {
                    return Ok(Some(candidate));
                }
            }
            (
                tsr_diagnostics::Could_not_resolve_the_path_0_with_the_extensions_Colon_1,
                vec![diagnostic_name, quoted_extensions()],
            )
        };
        let diagnostic = if let Some((source, loc)) = source {
            Diagnostic::new(Some(source.source()), loc, failure.0, failure.1)
        } else {
            let mut diagnostic = Diagnostic::compiler(failure.0, failure.1);
            let mut because = Diagnostic::compiler(
                tsr_diagnostics::The_file_is_in_the_program_because_Colon,
                Vec::new(),
            );
            because.message_chain.push(Arc::new(Diagnostic::compiler(
                tsr_diagnostics::Root_file_specified_for_compilation,
                Vec::new(),
            )));
            diagnostic.message_chain.push(Arc::new(because));
            diagnostic
        };
        self.diagnostics.push(diagnostic);
        Ok(None)
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
        let meta = metadata::load(&mut self.resolver, &name, &self.options, is_lib)?;
        let Some(content) = self.host.read_file(&name)? else {
            self.missing.push(JsString::from_bytes(name.as_slice()));
            if is_root {
                self.diagnostics.push(missing_root(&name));
                return Ok(());
            }
            return Err(Error::Unsupported("missing dependency include diagnostics"));
        };
        let options = SourceFileParseOptions {
            file_name: JsString::from_bytes(name.as_slice()),
            path: key.clone(),
            external_module_indicator_options: metadata::indicator(&name, &self.options, &meta),
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
        if !self.skip_resolution {
            if !self.options.no_resolve.is_true() {
                for (index, reference) in state.referenced_files()?.iter().enumerate() {
                    let target =
                        path::absolute(reference.file_name.as_bytes(), &path::directory(&name));
                    if let Some(target) = self.file_reference(
                        &target,
                        reference.file_name.as_bytes(),
                        Some((&file, reference.loc)),
                    )? {
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
                        &self.options,
                    );
                    let result = self
                        .resolver
                        .resolve_type_reference(reference.file_name.as_bytes(), &name, mode)?
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
                metadata::jsx_runtime_import(view, file.source(), &self.options)?
            } else {
                None
            };
            if self.options.import_helpers.is_true()
                && (matches!(kind, ScriptKind::JS | ScriptKind::JSX)
                    || !state.is_declaration_file
                        && (self.options.isolated_modules()
                            || state.external_module_indicator.is_some()))
            {
                self.resolve_specifier(
                    &file,
                    &name,
                    &key,
                    JsString::from_bytes(b"tslib".as_slice()),
                    metadata::normal_mode(&name, &meta, &self.options),
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
                    &name,
                    &key,
                    runtime.clone(),
                    metadata::normal_mode(&name, &meta, &self.options),
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
                self.resolve_import(&file, &name, &key, &meta, (usage, index), true)?;
            }
            for &usage in state.module_augmentations()?.iter() {
                let usage = usage.ok_or(tsr_arena::Error::InvalidGraph)?;
                if view.node(usage)?.kind() == tsr_ast::SyntaxKind::StringLiteral {
                    self.resolve_import(&file, &name, &key, &meta, (usage, 0), false)?;
                }
            }
        }
        self.child_tasks.insert(
            key,
            self.pending[pending_start..]
                .iter()
                .map(|task| {
                    let mut task = task.clone();
                    task.depth -= depth;
                    task
                })
                .collect(),
        );
        self.files.push(file);
        Ok(())
    }
    fn resolve_import(
        &mut self,
        file: &ProgramFile,
        name: &[u8],
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
        let mode = metadata::usage_mode(view, name, meta, usage, &self.options)?;
        let is_js = matches!(
            view.source_file(file.source())?.script_kind,
            ScriptKind::JS | ScriptKind::JSX
        );
        self.resolve_specifier(
            file,
            name,
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
        name: &[u8],
        key: &JsString,
        module_name: JsString,
        mode: ModuleKind,
        site: (bool, isize, Option<SyntheticImport>),
    ) -> Result<(), Error> {
        let view = file.bound.view().ast();
        let result = self
            .resolver
            .resolve(module_name.as_bytes(), name, mode)?
            .clone();
        self.file_traces
            .entry(key.clone())
            .or_default()
            .modules
            .extend(self.resolver.take_trace());
        if site.0
            && result.is_resolved()
            && !self.options.no_resolve.is_true()
            && tsr_module::resolution_diagnostic(
                &self.options,
                &result,
                view.source_file(file.source())?.is_declaration_file,
            )
            .is_none()
        {
            let target = result.resolved_file_name.as_bytes();
            let js = matches!(
                ScriptKind::from_file_name(target),
                ScriptKind::JS | ScriptKind::JSX
            );
            if !js || self.options.allow_js() {
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
fn lib_priority(name: &[u8], library_path: &[u8]) -> usize {
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
}
impl Collector<'_> {
    fn visit(&mut self, edge: &IncludeEdge) {
        stacker::maybe_grow(128 * 1024, 2 * 1024 * 1024, || self.visit_worker(edge));
    }
    fn visit_worker(&mut self, edge: &IncludeEdge) {
        let key = &edge.path;
        // Source adds each incoming reason before its per-file visited check.
        // Depth retries reuse the original child edges, retaining reason identity.
        if self.loaded_paths.contains(key) {
            if let Some(reason) = &edge.reason {
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
