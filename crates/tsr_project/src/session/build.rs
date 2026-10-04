use super::{
    Arc, BTreeMap, CompilerOptions, ConfigRegistryBuilder, Counters, DocumentUri, Error,
    FileChangeSummary, JsString, ParseCache, Session, SessionSnapshot, SnapshotFsBuilder,
};
use crate::{
    config::AffectedConfigs,
    overlay::Overlays,
    project::{ProgramUpdateKind, ProjectData, ProjectKind, INFERRED_PROJECT_NAME},
    source_fs::SourceFs,
    Project,
};
use std::collections::{BTreeSet, VecDeque};
use tsr_compiler::{
    CachedProgramFile, FileCache, Program, ProgramFile, ProgramOptions, SourceFileCache,
};
use tsr_core::{JsxEmit, ModuleKind, ModuleResolutionKind, ScriptKind, ScriptTarget, Tristate};
use tsr_jsstring::SourceText;
use tsr_tsoptions::ParsedCommandLine;

pub(super) struct ProjectBuilder<'a> {
    pub(super) session: &'a Session,
    old: &'a SessionSnapshot,
    snapshot_id: u64,
    fs: Arc<SnapshotFsBuilder>,
    overlays: Overlays,
    changes: &'a FileChangeSummary,
    affected: &'a AffectedConfigs,
    configs: &'a mut ConfigRegistryBuilder,
    inferred_options: Option<Arc<CompilerOptions>>,
    projects: BTreeMap<JsString, Project>,
    updated: BTreeSet<JsString>,
    keep: BTreeSet<JsString>,
}
pub(super) struct BuildInput<'a> {
    pub snapshot_id: u64,
    pub fs: Arc<SnapshotFsBuilder>,
    pub overlays: Overlays,
    pub changes: &'a FileChangeSummary,
    pub affected: &'a AffectedConfigs,
    pub inferred_options: Option<Arc<CompilerOptions>>,
}
type Collection = (BTreeMap<JsString, Project>, BTreeMap<JsString, JsString>);
impl<'a> ProjectBuilder<'a> {
    pub(super) fn new(
        session: &'a Session,
        old: &'a SessionSnapshot,
        configs: &'a mut ConfigRegistryBuilder,
        input: BuildInput<'a>,
    ) -> Self {
        let BuildInput {
            snapshot_id,
            fs,
            overlays,
            changes,
            affected,
            inferred_options,
        } = input;
        Self {
            session,
            old,
            snapshot_id,
            fs,
            overlays,
            changes,
            affected,
            configs,
            inferred_options,
            projects: old.projects.clone(),
            updated: BTreeSet::new(),
            keep: BTreeSet::new(),
        }
    }
    pub(super) fn build(mut self, requested: Option<&DocumentUri>) -> Result<Collection, Error> {
        let mut files: BTreeMap<_, _> = self
            .overlays
            .iter()
            .map(|(key, file)| (key.clone(), (file.file_name().clone(), file.kind())))
            .collect();
        if let Some(uri) = requested {
            let name = uri.file_name();
            let path = self.configs.path(name.as_bytes());
            files.entry(path).or_insert_with(|| {
                let kind = ScriptKind::from_file_name(name.as_bytes());
                (name, kind)
            });
        }
        let mut defaults = BTreeMap::new();
        let mut inferred_roots = Vec::new();
        for (path, (name, _kind)) in files {
            if !tsr_tspath::is_dynamic_file_name(name.as_bytes())
                && ScriptKind::from_file_name(name.as_bytes()) == ScriptKind::UNKNOWN
                && (!self.overlays.contains_key(&path)
                    || tsr_tspath::has_extension(name.as_bytes()))
            {
                continue;
            }
            if let Some(project) = self.select_configured(&name, &path)? {
                defaults.insert(path, project);
            } else {
                inferred_roots.push(name);
                defaults.insert(path, JsString::from_bytes(INFERRED_PROJECT_NAME));
            }
        }
        inferred_roots.sort();
        if !inferred_roots.is_empty() {
            let key = JsString::from_bytes(INFERRED_PROJECT_NAME);
            let old = self.old.projects.get(&key).and_then(Project::data);
            let same_options = match (&self.inferred_options, &self.old.inferred_options) {
                (None, None) => true,
                (Some(a), Some(b)) => **a == **b,
                _ => false,
            };
            let command = old
                .filter(|old| same_options && old.command_line.root_file_names == inferred_roots)
                .map_or_else(
                    || {
                        Arc::new(ParsedCommandLine::new(
                            self.inferred_options
                                .as_deref()
                                .cloned()
                                .unwrap_or_else(default_inferred_options),
                            inferred_roots,
                        ))
                    },
                    |old| old.command_line.clone(),
                );
            self.update_project(&key, &key, ProjectKind::Inferred, command)?;
            self.keep.insert(key);
        }
        let cleanup = self.changes.opened.is_some() || self.changes.reopened.is_some();
        if !cleanup {
            self.keep.extend(
                self.projects
                    .keys()
                    .filter(|key| key.as_bytes() != INFERRED_PROJECT_NAME)
                    .cloned(),
            );
        }
        let removed: Vec<_> = self
            .projects
            .keys()
            .filter(|key| !self.keep.contains(*key))
            .cloned()
            .collect();
        for key in removed {
            self.projects.remove(&key);
            self.configs.release_project(&key);
        }
        Ok((self.projects, defaults))
    }
    fn select_configured(
        &mut self,
        file: &JsString,
        path: &JsString,
    ) -> Result<Option<JsString>, Error> {
        let mut config = self.configs.config_file_name(file.as_bytes())?;
        let mut visited = BTreeSet::new();
        let mut retained = BTreeSet::new();
        let mut fallback = None;
        while !config.is_empty() {
            let mut queue = VecDeque::from([(config.clone(), true, Vec::<JsString>::new())]);
            while let Some((name, create, mut chain)) = queue.pop_front() {
                let key = self.configs.path(name.as_bytes());
                if !visited.insert((key.clone(), create)) {
                    continue;
                }
                let command = if create {
                    self.configs.acquire_for_file(&name, path)?
                } else {
                    self.projects
                        .get(&key)
                        .and_then(Project::data)
                        .map(|p| p.command_line.clone())
                };
                let Some(command) = command else {
                    continue;
                };
                chain.push(key.clone());
                retained.insert(key.clone());
                if create && self.projects.contains_key(&key) && command.root_file_names.is_empty()
                {
                    let command = self
                        .configs
                        .acquire_for_project(&name, &key)?
                        .expect("loaded config");
                    self.update_project(&key, &name, ProjectKind::Configured, command)?;
                }
                if !command.root_file_names.is_empty()
                    && (!command.options.composite.is_true()
                        || command.file_names_by_path().contains_key(path))
                {
                    if create {
                        let command = self
                            .configs
                            .acquire_for_project(&name, &key)?
                            .expect("loaded config");
                        self.update_project(&key, &name, ProjectKind::Configured, command)?;
                    }
                    if let Some(project) = self
                        .projects
                        .get(&key)
                        .filter(|p| p.contains_file(path.as_bytes()))
                    {
                        self.keep.extend(chain.iter().cloned());
                        if !project
                            .program()
                            .unwrap()
                            .is_source_from_project_reference(path.as_bytes())
                        {
                            self.configs
                                .retain_file_configs(path, &chain.into_iter().collect());
                            return Ok(Some(key));
                        }
                        fallback.get_or_insert(key.clone());
                    }
                }
                let create_children =
                    create && !command.options.disable_referenced_project_load.is_true();
                for reference in command.resolved_project_reference_paths() {
                    queue.push_back((reference.clone(), create_children, chain.clone()));
                }
            }
            if fallback
                .as_ref()
                .and_then(|key| self.projects.get(key))
                .and_then(Project::data)
                .is_some_and(|p| p.command_line.options.disable_solution_searching.is_true())
            {
                break;
            }
            let next = self
                .configs
                .ancestor_config_file_name(file.as_bytes(), &config)?;
            if next == config {
                break;
            }
            config = next;
        }
        self.keep.extend(
            retained
                .iter()
                .filter(|key| self.projects.contains_key(*key))
                .cloned(),
        );
        self.configs.retain_file_configs(path, &retained);
        Ok(fallback)
    }
    fn update_project(
        &mut self,
        key: &JsString,
        name: &JsString,
        kind: ProjectKind,
        command: Arc<ParsedCommandLine>,
    ) -> Result<(), Error> {
        if !self.updated.insert(key.clone()) {
            return Ok(());
        }
        let old = self.old.projects.get(key).and_then(Project::data);
        let command_changed = old.is_none_or(|old| !Arc::ptr_eq(&old.command_line, &command));
        let relevant = |uri: &DocumentUri| {
            old.is_some_and(|old| {
                old.host.seen_file_or_missing_parent_directory(
                    &self.configs.path(uri.file_name().as_bytes()),
                ) || old
                    .program
                    .source_file(uri.file_name().as_bytes())
                    .is_some()
            })
        };
        let changed: Vec<_> = self
            .changes
            .changed
            .iter()
            .filter(|uri| relevant(uri))
            .collect();
        let structure = self.changes.invalidate_all
            || self.affected.projects.contains(key)
            || self
                .changes
                .created
                .iter()
                .chain(&self.changes.deleted)
                .any(relevant);
        if !command_changed && !structure && changed.is_empty() {
            return Ok(());
        }
        let display = if kind == ProjectKind::Configured {
            tsr_tspath::convert_to_relative_path(
                name.as_bytes(),
                self.session.options.current_directory.as_bytes(),
                true,
            )
        } else {
            tsr_tspath::base_name(self.session.options.current_directory.as_bytes()).to_vec()
        };
        let _loading = Loading::new(self.session, JsString::from_bytes(display));
        let cwd = if kind == ProjectKind::Configured {
            JsString::from_bytes(tsr_tspath::directory(name.as_bytes()))
        } else {
            self.session.options.current_directory.clone()
        };
        let host = Arc::new(SourceFs::new(
            self.fs.clone(),
            self.session.options.current_directory.clone(),
            true,
        ));
        let mut cache = FileCache::for_project(Arc::new(OverlayParses {
            shared: self.session.parse_cache.clone(),
            overlays: self.overlays.clone(),
            cwd: self.session.options.current_directory.clone(),
            case_sensitive: self.session.fs.use_case_sensitive_file_names(),
        }));
        let mut reuse = None;
        if !command_changed && !structure && changed.len() == 1 {
            reuse = Some(
                old.unwrap().program.reuse_program(
                    self.configs
                        .path(changed[0].file_name().as_bytes())
                        .as_bytes(),
                    host.clone(),
                    &mut cache,
                    &self.session.counters,
                )?,
            );
        }
        let (program, update_kind) =
            if let Some(program) = reuse.as_mut().and_then(|reuse| reuse.program.take()) {
                // Reuse skips resolution; carry forward the dependency observations
                // of that unchanged graph before freezing the new host.
                host.inherit_dependencies(&old.unwrap().host);
                (program, ProgramUpdateKind::Cloned)
            } else {
                let program = Program::load_live_for_project(
                    ProgramOptions {
                        config: (*command).clone(),
                        host: host.clone(),
                        current_directory: cwd.clone(),
                        default_library_path: self.session.options.default_library_path.clone(),
                        skip_module_resolution: false,
                        single_threaded: Tristate::UNKNOWN,
                    },
                    &mut cache,
                    &self.session.counters,
                )?;
                let same_names = old.is_some_and(|old| {
                    old.program.files().len() == program.files().len()
                        && old.program.files().iter().all(|file| {
                            let view = file.bound().view();
                            let source = view.source_file().expect("bound source");
                            program.source_file(source.file_name()).is_some()
                        })
                });
                (
                    program,
                    if same_names {
                        ProgramUpdateKind::SameFileNames
                    } else {
                        ProgramUpdateKind::NewFiles
                    },
                )
            };
        host.disable_tracking();
        let watch = old.map_or_else(
            || {
                crate::watch::WatchedFiles::new(
                    JsString::from_bytes(
                        [b"program files for ".as_slice(), key.as_bytes()].concat(),
                    ),
                    crate::watch::ALL_CHANGES,
                    self.session.options.relative_watch_patterns,
                )
            },
            |old| old.program_files_watch.clone(),
        );
        let program_files_watch = if update_kind == ProgramUpdateKind::NewFiles {
            watch.with_input(crate::watch::resolution_patterns(
                &host.seen_files(),
                self.session.options.current_directory.as_bytes(),
                self.session.options.default_library_path.as_bytes(),
                cwd.as_bytes(),
                self.session.fs.use_case_sensitive_file_names(),
            ))
        } else {
            watch
        };
        let project = Project::from_program(
            ProjectData {
                program_files_watch,
                name: name.clone(),
                path: key.clone(),
                kind,
                current_directory: cwd,
                command_line: command,
                program: Arc::new(program),
                update_kind,
                last_update: self.snapshot_id,
                host,
            },
            &self.session.counters,
            self.session.options.query_checkers,
        );
        self.projects.insert(key.clone(), project);
        Ok(())
    }
}
struct Loading<'a> {
    session: &'a Session,
    name: JsString,
}
impl<'a> Loading<'a> {
    fn new(session: &'a Session, name: JsString) -> Self {
        session.send_event(super::SessionEvent::ProjectLoading {
            name: name.clone(),
            finished: false,
        });
        Self { session, name }
    }
}
impl Drop for Loading<'_> {
    fn drop(&mut self) {
        self.session
            .send_event(super::SessionEvent::ProjectLoading {
                name: self.name.clone(),
                finished: true,
            });
    }
}
fn default_inferred_options() -> CompilerOptions {
    CompilerOptions {
        allow_js: Tristate::TRUE,
        module: ModuleKind::ESNEXT,
        module_resolution: ModuleResolutionKind::BUNDLER,
        target: ScriptTarget::LATEST_STANDARD,
        jsx: JsxEmit::REACT_JSX,
        allow_importing_ts_extensions: Tristate::TRUE,
        strict_null_checks: Tristate::TRUE,
        strict_function_types: Tristate::TRUE,
        source_map: Tristate::TRUE,
        allow_non_ts_extensions: Tristate::TRUE,
        resolve_json_module: Tristate::TRUE,
        ..Default::default()
    }
}
struct OverlayParses {
    shared: Arc<ParseCache>,
    overlays: Overlays,
    cwd: JsString,
    case_sensitive: bool,
}
impl SourceFileCache for OverlayParses {
    fn script_kind(&self, name: &[u8]) -> ScriptKind {
        let path = tsr_tspath::to_path(name, self.cwd.as_bytes(), self.case_sensitive);
        self.overlays.get(&path).map_or_else(
            || ScriptKind::ensure_from_file_name(name),
            |file| {
                if file.kind() == ScriptKind::UNKNOWN {
                    ScriptKind::ensure_from_file_name(name)
                } else {
                    file.kind()
                }
            },
        )
    }
    fn acquire(
        &self,
        text: SourceText,
        kind: ScriptKind,
        options: tsr_ast::SourceFileParseOptions,
        counters: &Counters,
        tracing: Option<&Arc<dyn tsr_checker::TraceSink>>,
    ) -> Result<CachedProgramFile, tsr_compiler::Error> {
        self.shared.acquire(text, kind, options, counters, tracing)
    }
    fn retain(&self, file: &Arc<ProgramFile>) -> Result<Box<dyn Send + Sync>, tsr_compiler::Error> {
        self.shared.retain(file)
    }
}
