mod resources;
use super::{
    Arc, BTreeMap, CompilerOptions, ConfigRegistryBuilder, Counters, Error, FileChangeSummary,
    JsString, ParseCache, Session, SessionSnapshot, SnapshotFsBuilder,
};
use crate::{
    config::AffectedConfigs,
    file_change::FileChangeKind,
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
    contributions: Arc<crate::content_mappers::Contributions>,
    projects: BTreeMap<JsString, Project>,
    delayed_projects: BTreeMap<JsString, crate::DelayedProject>,
    updated: BTreeSet<JsString>,
    keep: BTreeSet<JsString>,
    api_state: crate::api::ApiState,
}
pub(super) struct BuildInput<'a> {
    pub ata_changes: &'a BTreeMap<JsString, super::ata::AtaChange>,
    pub snapshot_id: u64,
    pub fs: Arc<SnapshotFsBuilder>,
    pub overlays: Overlays,
    pub changes: &'a FileChangeSummary,
    pub affected: &'a AffectedConfigs,
    pub inferred_options: Option<Arc<CompilerOptions>>,
    pub contributions: Arc<crate::content_mappers::Contributions>,
}
pub(super) struct BuildOutput {
    pub projects: BTreeMap<JsString, Project>,
    pub delayed_projects: BTreeMap<JsString, crate::DelayedProject>,
    pub defaults: BTreeMap<JsString, JsString>,
    pub api_state: crate::api::ApiState,
    pub api_error: Option<JsString>,
}
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
            contributions,
            ata_changes,
        } = input;
        let mut projects = old.projects.clone();
        for (key, change) in ata_changes {
            if let Some(project) = projects.get_mut(key) {
                let data = Arc::make_mut(project.data.as_mut().expect("session project"));
                if !data.compute_typings_info().equals(&change.info) {
                    continue;
                }
                data.installed_typings_info = Some(change.info.clone());
                data.typings_files.clone_from(&change.result.typings_files);
                let watch = data.typings_watch.clone().unwrap_or_else(|| {
                    crate::watch::WatchedFiles::new(
                        JsString::from_bytes(b"typings installer files".as_slice()),
                        crate::watch::ALL_CHANGES,
                        session.options.relative_watch_patterns,
                    )
                });
                data.typings_watch = Some(watch.with_input(super::ata::typings_watch_patterns(
                    &change.result.files_to_watch,
                    session.options.typings_location.as_bytes(),
                    session.options.current_directory.as_bytes(),
                    data.current_directory.as_bytes(),
                    session.fs.use_case_sensitive_file_names(),
                )));
                data.dirty = true;
                data.dirty_file = None;
            }
        }
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
            contributions,
            projects,
            delayed_projects: old.delayed_projects.clone(),
            updated: BTreeSet::new(),
            keep: BTreeSet::new(),
            api_state: old.api_state.clone(),
        }
    }
    pub(super) fn build(
        mut self,
        request: &crate::api::ResourceRequest,
    ) -> Result<BuildOutput, Error> {
        // Watch batches are consumed once, including changes to projects with
        // no open files. Keep their dirtiness until a later request rebuilds them.
        self.mark_projects_dirty();
        let api_error = self.handle_api_request(request.api.as_ref())?;
        let cleanup = self.changes.opened.is_some()
            || self.changes.reopened.is_some()
            || request
                .api
                .as_ref()
                .is_some_and(|api| api.open_files.is_some() || api.close_files.is_some());
        let mut files: BTreeMap<_, _> = self
            .overlays
            .iter()
            .map(|(key, file)| (key.clone(), (file.file_name().clone(), file.kind())))
            .collect();
        if !cleanup {
            if let Some(old) = self
                .old
                .projects
                .get(&JsString::from_bytes(INFERRED_PROJECT_NAME))
                .and_then(Project::data)
            {
                for name in &old.command_line.root_file_names {
                    if self
                        .changes
                        .closed
                        .contains(&tsr_lsproto::DocumentUri::from_file_name(name.as_bytes()))
                    {
                        continue;
                    }
                    let path = self.configs.path(name.as_bytes());
                    files.entry(path).or_insert_with(|| {
                        (name.clone(), ScriptKind::from_file_name(name.as_bytes()))
                    });
                }
            }
        }
        for (path, file) in &self.api_state.files {
            files.entry(path.clone()).or_insert_with(|| {
                (
                    file.name.clone(),
                    ScriptKind::from_file_name(file.name.as_bytes()),
                )
            });
        }
        for uri in &request.documents {
            let name = uri.file_name();
            let path = self.configs.path(name.as_bytes());
            files.entry(path).or_insert_with(|| {
                let kind = ScriptKind::from_file_name(name.as_bytes());
                (name, kind)
            });
        }
        let mut defaults = BTreeMap::new();
        let mut inferred_roots = Vec::new();
        let opened = self
            .changes
            .opened
            .as_ref()
            .or(self.changes.reopened.as_ref())
            .map(|uri| self.configs.path(uri.file_name().as_bytes()));
        // The open event loads its project before cleanup considers existing
        // overlays. That project can now contain a formerly inferred file.
        let opened_project = if let Some((path, (name, _))) = opened
            .as_ref()
            .and_then(|path| files.get_key_value(path))
            .filter(|(_, (name, _))| !tsr_tspath::is_dynamic_file_name(name.as_bytes()))
        {
            self.select_configured(name, path)?
        } else {
            None
        };
        for (path, (name, _kind)) in files {
            if !tsr_tspath::is_dynamic_file_name(name.as_bytes()) {
                // DidChangeFiles ensures the newly opened file's project.
                // Cleanup only retains other open files' existing projects;
                // it must not eagerly rebuild their dirty programs. A later
                // file/project-tree request performs that update.
                let retained = if opened.as_ref() == Some(&path) && opened_project.is_some() {
                    opened_project.clone()
                } else if opened.is_some()
                    && !self.changes.invalidate_all
                    && !self.configs.custom_name_changed()
                    && !self.affected.files.contains(&path)
                    && request.api.is_none()
                    && !request
                        .documents
                        .iter()
                        .any(|uri| self.configs.path(uri.file_name().as_bytes()) == path)
                {
                    let (project, ambiguous) = self.configured_project_containing(&path);
                    if ambiguous {
                        None
                    } else {
                        project
                    }
                } else {
                    None
                };
                let selected = if let Some(key) = retained {
                    self.keep.insert(key.clone());
                    Some(key)
                } else {
                    let project = self
                        .select_configured(&name, &path)?
                        .or_else(|| self.configured_project_containing(&path).0);
                    if let Some(key) = &project {
                        self.keep.insert(key.clone());
                    }
                    project
                };
                if let Some(project) = selected {
                    if cleanup {
                        // Like cleanupConfiguredProjects, retain loaded and
                        // delayed nearest/ancestor configs even when they did
                        // not contain the file and selection continued upward.
                        for config in self.configs.searched_config_names(&path) {
                            let key = self.configs.path(config.as_bytes());
                            if self.projects.contains_key(&key)
                                || self.delayed_projects.contains_key(&key)
                            {
                                self.keep.insert(key);
                            }
                        }
                    }
                    defaults.insert(path, project);
                    continue;
                }
                if ScriptKind::from_file_name(name.as_bytes()) == ScriptKind::UNKNOWN
                    && (!self.overlays.contains_key(&path)
                        || tsr_tspath::has_extension(name.as_bytes()))
                    && !self
                        .contributions
                        .extensions
                        .iter()
                        .any(|ext| name.as_bytes().ends_with(ext.as_bytes()))
                {
                    continue;
                }
            }
            inferred_roots.push(name);
            defaults.insert(path, JsString::from_bytes(INFERRED_PROJECT_NAME));
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
                .filter(|old| {
                    same_options
                        && Arc::ptr_eq(&self.contributions, &self.old.contributions)
                        && old.command_line.root_file_names == inferred_roots
                })
                .map_or_else(
                    || {
                        let mut command = ParsedCommandLine::new(
                            self.inferred_options
                                .as_deref()
                                .cloned()
                                .unwrap_or_else(default_inferred_options),
                            inferred_roots,
                        );
                        if !self.contributions.mappers.is_empty() {
                            command.content_mappers = Some(self.contributions.mappers.clone());
                        }
                        Arc::new(command)
                    },
                    |old| old.command_line.clone(),
                );
            self.update_project(&key, &key, ProjectKind::Inferred, command)?;
            self.keep.insert(key);
        }
        self.load_resources(request)?;
        self.keep.extend(self.api_state.projects.keys().cloned());
        if !cleanup {
            self.keep.extend(
                self.projects
                    .keys()
                    .filter(|key| key.as_bytes() != INFERRED_PROJECT_NAME)
                    .cloned(),
            );
            self.keep.extend(self.delayed_projects.keys().cloned());
        }
        // The default project's loaded references remain live across another
        // file open, including references of an already-loaded ancestor.
        if cleanup {
            for key in self.keep.clone() {
                if let Some(program) = self.projects.get(&key).and_then(Project::program) {
                    program.range_resolved_project_reference(|path, _, _, _| {
                        let reference = self.configs.path(path);
                        if self.projects.contains_key(&reference) {
                            self.keep.insert(reference);
                        }
                        true
                    });
                }
            }
        }
        self.delayed_projects
            .retain(|key, _| self.keep.contains(key));
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
        Ok(BuildOutput {
            projects: self.projects,
            delayed_projects: self.delayed_projects,
            defaults,
            api_state: self.api_state,
            api_error,
        })
    }

    // port: tsc/internal/project/projectcollectionbuilder.go:ProjectCollectionBuilder.markFilesChanged
    /// One synthetic inferred project from `command`'s explicit roots and
    /// options, seeded from the old program's project when the client passes
    /// one; every other project of the base leaves this snapshot.
    /// port: tsc/internal/project/snapshot.go:Snapshot.cloneForProgram
    pub(super) fn build_program(
        mut self,
        command: Arc<ParsedCommandLine>,
        old_project: Option<&Project>,
    ) -> Result<BuildOutput, Error> {
        let key = JsString::from_bytes(INFERRED_PROJECT_NAME);
        let roots = command.root_file_names.clone();
        self.projects.clear();
        self.delayed_projects.clear();
        if let Some(old) = old_project {
            self.projects.insert(key.clone(), old.clone());
        }
        self.update_project(&key, &key, ProjectKind::Inferred, command)?;
        let defaults = roots
            .iter()
            .map(|name| (self.configs.path(name.as_bytes()), key.clone()))
            .collect();
        for other in self.old.projects.keys().filter(|other| **other != key) {
            self.configs.release_project(other);
        }
        Ok(BuildOutput {
            projects: self.projects,
            delayed_projects: BTreeMap::new(),
            defaults,
            api_state: crate::api::ApiState::default(),
            api_error: None,
        })
    }

    fn mark_projects_dirty(&mut self) {
        let changes = [
            (FileChangeKind::WatchChange, &self.changes.changed),
            (FileChangeKind::WatchDelete, &self.changes.deleted),
            (FileChangeKind::WatchCreate, &self.changes.created),
        ]
        .map(|(kind, uris)| {
            let paths: Vec<_> = uris
                .iter()
                .map(|uri| self.configs.path(uri.file_name().as_bytes()))
                .collect();
            (kind, paths)
        });
        for (key, project) in &mut self.projects {
            if project.auto_import_cache().is_some_and(|cache| {
                let dependencies = cache.dependencies();
                self.changes.invalidate_all
                    || changes.iter().any(|(_, paths)| {
                        paths.iter().any(|p| {
                            dependencies.affected(
                                p.as_bytes(),
                                self.session.fs.use_case_sensitive_file_names(),
                            )
                        })
                    })
            }) {
                // Keep the retained snapshot's cache untouched. Auxiliary
                // package changes need not force a compiler-program rebuild.
                project.auto_imports = Some(Arc::new(tsr_autoimport::Cache::new(
                    project.program().unwrap(),
                )));
            }
            let old = project.data().unwrap();
            let mut dirty = old.dirty;
            let mut dirty_file = old.dirty_file.clone();
            if self.session.mapper_host.is_some()
                && !old.content_mapper_watched_files.is_empty()
                && (self.changes.invalidate_all
                    || changes.iter().any(|(_, paths)| {
                        paths
                            .iter()
                            .any(|path| old.content_mapper_watched_files.contains(path))
                    }))
            {
                if let Some(project) = old.program.content_mapper_project() {
                    let _ = project.refresh();
                }
                dirty = true;
                dirty_file = None;
            }
            if self.changes.invalidate_all || self.affected.projects.contains(key) {
                dirty = true;
                dirty_file = None;
            }
            'kinds: for (kind, paths) in &changes {
                if dirty && dirty_file.is_none() {
                    break;
                }
                for path in paths {
                    if project.contains_file(path.as_bytes()) {
                        dirty = true;
                        if *kind == FileChangeKind::WatchDelete
                            || tsr_tspath::base_name(path.as_bytes()) == b"package.json"
                        {
                            dirty_file = None;
                            break 'kinds;
                        }
                        if let Some(previous) = &dirty_file {
                            if previous != path {
                                dirty_file = None;
                                break 'kinds;
                            }
                        } else {
                            dirty_file = Some(path.clone());
                        }
                    } else if if *kind == FileChangeKind::WatchCreate {
                        old.host.seen_file_or_missing_parent_directory(path)
                    } else {
                        old.host.seen_file(path)
                    } {
                        dirty = true;
                        dirty_file = None;
                        break 'kinds;
                    }
                }
            }
            if dirty != old.dirty || dirty_file != old.dirty_file {
                let data = Arc::make_mut(project.data.as_mut().unwrap());
                data.dirty = dirty;
                data.dirty_file = dirty_file;
            }
        }
    }
    // GetDefaultProject's program-inclusion fallback also covers dependencies
    // under node_modules, where nearest-config discovery deliberately stops.
    // The pin's ProjectCollectionBuilder.findDefaultConfiguredProject sorts
    // config paths before findDefaultConfiguredProjectFromProgramInclusion;
    // BTreeMap iteration preserves that first-containing-project tie-break.
    // One direct inclusion wins; zero or multiple direct inclusions use that
    // sorted fallback (or None if no project contains the file).
    // Ambiguous direct candidates continue through normal config discovery.
    fn configured_project_containing(&self, path: &JsString) -> (Option<JsString>, bool) {
        let mut containing = Vec::new();
        let mut direct = Vec::new();
        for (key, project) in &self.projects {
            if key.as_bytes() == INFERRED_PROJECT_NAME || !project.contains_file(path.as_bytes()) {
                continue;
            }
            containing.push(key);
            if !project
                .program()
                .unwrap()
                .is_source_from_project_reference(path.as_bytes())
            {
                direct.push(key);
            }
        }
        if direct.len() == 1 {
            return (Some(direct[0].clone()), false);
        }
        (
            containing.first().map(|key| (*key).clone()),
            direct.len() > 1,
        )
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
                    self.configs.existing_config(&key)
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
                            self.discover_ancestor_projects(file, path, &key)?;
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
        if let Some(key) = &fallback {
            self.discover_ancestor_projects(file, path, key)?;
        }
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
        self.delayed_projects.remove(key);
        let old = self.old.projects.get(key).and_then(Project::data);
        let command_changed = old.is_none_or(|old| !Arc::ptr_eq(&old.command_line, &command));
        let pending = self.projects.get(key).and_then(Project::data);
        if !command_changed && pending.is_none_or(|data| !data.dirty) {
            return Ok(());
        }
        let dirty_file = pending.and_then(|data| data.dirty_file.clone());
        let installed_typings_info = pending.and_then(|data| data.installed_typings_info.clone());
        let typings_files = pending.map_or_else(Vec::new, |data| data.typings_files.clone());
        let typings_watch = pending.and_then(|data| data.typings_watch.clone());
        let ata_enabled = !self
            .session
            .ata_disabled
            .load(std::sync::atomic::Ordering::Acquire)
            && super::ata::type_acquisition(kind, &command)
                .enable
                .is_true();
        let mut program_command = (*command).clone();
        if ata_enabled {
            program_command
                .root_file_names
                .extend(typings_files.iter().cloned());
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
            mapped: self.session.mapped_parse_cache.clone(),
            locale: self
                .session
                .mapper_host
                .as_ref()
                .map_or_else(String::new, crate::content_mappers::MapperHost::locale),
            overlays: self.overlays.clone(),
            cwd: self.session.options.current_directory.clone(),
            case_sensitive: self.session.fs.use_case_sensitive_file_names(),
        }));
        let mut reuse = None;
        if let Some(path) = dirty_file.as_ref().filter(|_| !command_changed) {
            reuse = Some(old.unwrap().program.reuse_program(
                path.as_bytes(),
                host.clone(),
                &mut cache,
                &self.session.counters,
            )?);
        }
        let (program, update_kind) =
            if let Some(program) = reuse.as_mut().and_then(|reuse| reuse.program.take()) {
                // Reuse skips resolution; carry forward the dependency observations
                // of that unchanged graph before freezing the new host.
                host.inherit_dependencies(&old.unwrap().host);
                (program, ProgramUpdateKind::Cloned)
            } else {
                let program = Program::load_live_for_project_with_host_services(
                    ProgramOptions {
                        config: program_command,
                        host: host.clone(),
                        current_directory: cwd.clone(),
                        default_library_path: self.session.options.default_library_path.clone(),
                        skip_module_resolution: false,
                        single_threaded: Tristate::UNKNOWN,
                    },
                    tsr_compiler::ProgramHostServices {
                        typings_location: if ata_enabled {
                            self.session.options.typings_location.clone()
                        } else {
                            JsString::default()
                        },
                        content_mapper_project: self
                            .session
                            .mapper_host
                            .as_ref()
                            .and_then(|host| host.project(&command)),
                        ..Default::default()
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
        let has_mapped_files = program.files().iter().any(|file| {
            file.bound()
                .view()
                .source_file()
                .is_ok_and(|source| !source.content_mapper().is_empty())
        });
        let mapper_paths = crate::content_mappers::watched_files(
            &command,
            if has_mapped_files {
                program.content_mapper_project()
            } else {
                None
            },
        );
        let content_mapper_watched_files = mapper_paths
            .into_iter()
            .map(|path| self.configs.path(path.as_bytes()))
            .collect();
        let mapper_watch = old.map_or_else(
            || {
                crate::watch::WatchedFiles::new(
                    JsString::from_bytes(
                        [b"content mapper files for ".as_slice(), key.as_bytes()].concat(),
                    ),
                    crate::watch::ALL_CHANGES,
                    self.session.options.relative_watch_patterns,
                )
            },
            |old| old.content_mapper_watch.clone(),
        );
        let content_mapper_watch = mapper_watch.with_input(crate::watch::resolution_patterns(
            &content_mapper_watched_files,
            self.session.options.current_directory.as_bytes(),
            self.session.options.default_library_path.as_bytes(),
            cwd.as_bytes(),
            self.session.fs.use_case_sensitive_file_names(),
        ));
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
        // The pin rebuilds its project bucket for added files, not removals
        // alone. Retain it across a root-list-only config change; package,
        // option and source changes still invalidate it. Cache::get checks all
        // remaining source identities and rejects newly added files.
        let roots_only = command_changed
            && !self.changes.invalidate_all
            && self.changes.created.is_empty()
            && self.changes.deleted.is_empty()
            && self
                .changes
                .changed
                .iter()
                .all(|uri| self.configs.path(uri.file_name().as_bytes()) == *key)
            && old.is_some_and(|old| {
                !old.dirty
                    && old.command_line.options == command.options
                    && old.command_line.project_references == command.project_references
                    && old.command_line.content_mappers.is_none()
                    && command.content_mappers.is_none()
            });
        let inherited_auto_imports = self
            .projects
            .get(key)
            .and_then(Project::auto_import_cache)
            .and_then(|previous| {
                if roots_only {
                    Some(tsr_autoimport::Cache::for_root_change(&program, &previous))
                } else {
                    dirty_file
                        .as_ref()
                        .filter(|_| !command_changed)
                        .map(|dirty| tsr_autoimport::Cache::for_update(&program, &previous, dirty))
                }
            })
            .map(Arc::new);
        // Compiler reference resolution uses the program host; mirror its
        // ownership edges into the session registry, including transitive
        // references. Release only dropped edges so unchanged entries retain
        // their identity across a program rebuild.
        let mut retained_configs = BTreeSet::from([key.clone()]);
        let mut reference_names = Vec::new();
        program.range_resolved_project_reference(|path, config, _, _| {
            if let Some(config) = config {
                retained_configs.insert(self.configs.path(path));
                reference_names.push(config.config_name());
            }
            true
        });
        for reference in reference_names {
            self.configs.acquire_for_project(&reference, key)?;
        }
        self.configs.retain_project_configs(key, &retained_configs);
        let mut project = Project::from_program(
            ProjectData {
                installed_typings_info,
                typings_files,
                typings_watch,
                content_mapper_watch,
                content_mapper_watched_files,
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
                dirty: false,
                dirty_file: None,
            },
            &self.session.counters,
            self.session.options.query_checkers,
        );
        project.completion_host = Some(self.session.fs.clone());
        if let Some(cache) = inherited_auto_imports {
            project.auto_imports = Some(cache);
        }
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
    mapped: Arc<crate::parse_cache::ContentMappedParseCache>,
    locale: String,
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
    fn acquire_mapped(
        &self,
        request: &tsr_compiler::MappedSourceFileRequest<'_>,
    ) -> tsr_compiler::MappedFileResult<tsr_compiler::CachedMappedProgramFiles> {
        self.mapped.acquire_mapped(request, &self.locale)
    }
    fn retain(&self, file: &Arc<ProgramFile>) -> Result<Box<dyn Send + Sync>, tsr_compiler::Error> {
        if file
            .bound()
            .view()
            .source_file()?
            .content_mapper()
            .is_empty()
        {
            self.shared.retain(file)
        } else {
            self.mapped.retain(file)
        }
    }
}
