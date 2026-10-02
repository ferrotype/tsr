use crate::*;
use std::collections::HashSet;
use tsr_tsc::fswatch::EventKind;
use tsr_tsc::watchmanager::{can_watch_directory, ComparePathsOptions, DirWatchSet, WatchManager};
use tsr_vfs::FileSystem;

/// Start a build, retaining its graph and mapper projects for watch cycles.
pub fn start(ctx: &Context, options: Options) -> Result<CommandLineResult, Error> {
    let watching = options.command.compiler_options.watch.is_true();
    let mut o = Orchestrator::new(options);
    if !watching {
        return o.start(ctx);
    }
    let fs = o.opts.sys.fs();
    let manager = WatchManager::new(
        o.opts.sys.writer(),
        Arc::new(move |directory| fs.directory_exists(directory).unwrap_or(false)),
    );
    if let Some(backend) = o
        .opts
        .testing
        .as_ref()
        .and_then(|testing| testing.as_with_watch_backend())
        .map(|testing| testing.watch_backend())
    {
        manager.set_backend(backend);
    }
    o.watch_status(d::Starting_compilation_in_watch_mode)?;
    let mut result = o.start(ctx)?;
    let testing = o.opts.testing.is_some();
    let debug = !testing
        && o.opts
            .sys
            .get_environment_variable("TS_WATCH_DEBUG")
            .is_some_and(|value| !value.is_empty());
    if !testing {
        if debug {
            manager.set_debug_log(Some(o.opts.sys.writer()));
        }
        manager.ensure_default_backend();
    }
    {
        let _guard = manager.lock();
        o.update_watch();
        o.reconcile(&manager);
        o.reset_caches();
    }
    let watcher = Arc::new(BuildWatcher {
        orchestrator: Mutex::new(o),
        manager: manager.clone(),
        context: ctx.clone(),
        debug,
    });
    if !testing {
        manager.run_loop(ctx, || tsr_tsc::Watcher::do_cycle(watcher.as_ref()))?;
    }
    result.watcher = Some(watcher);
    Ok(result)
}

struct BuildWatcher {
    orchestrator: Mutex<Orchestrator>,
    manager: WatchManager,
    context: Context,
    debug: bool,
}
impl Drop for BuildWatcher {
    fn drop(&mut self) {
        self.manager.close_all_watches();
    }
}
impl tsr_tsc::Watcher for BuildWatcher {
    // port: tsc/internal/execute/build/orchestrator.go:Orchestrator.DoCycle
    fn do_cycle(&self) -> Result<(), Error> {
        let _guard = self.manager.lock();
        let (changes, overflow) = self.manager.drain_events();
        let mut o = lock(&self.orchestrator);
        if changes.is_empty() && !overflow {
            if self.debug {
                tsr_tsc::write_all(
                    o.opts.sys.writer().as_ref(),
                    b"[watch] DoCycle: no events, skipping\n",
                );
            }
            return Ok(());
        }
        let (reparse, update) = if overflow {
            for node in &o.tasks {
                node.task.dirty.store(true, Ordering::Release);
            }
            (true, true)
        } else {
            o.check_event_changes(&changes, &self.manager)?
        };
        if !update {
            o.reset_caches();
            return Ok(());
        }
        o.watch_status(d::File_change_detected_Starting_incremental_compilation)?;
        if reparse {
            o.generate_graph_reusing_old_tasks()?;
        }
        o.build_or_clean(&self.context)?;
        o.update_watch();
        o.reconcile(&self.manager);
        o.reset_caches();
        Ok(())
    }
}

impl Orchestrator {
    fn watch_status(&self, message: &'static d::Message) -> Result<(), Error> {
        tsr_tsc::create_watch_status_reporter(
            self.opts.sys.as_ref(),
            self.opts.command.locale().clone(),
            &self.opts.command.compiler_options,
            self.opts.testing.as_deref(),
        )
        .report(&self.sources(), &Diagnostic::compiler(message, Vec::new()))
    }
    fn compare_options(&self) -> ComparePathsOptions {
        ComparePathsOptions {
            current_directory: self.host.cwd.clone(),
            use_case_sensitive_file_names: self.host.case_sensitive,
        }
    }
    fn reconcile(&self, manager: &WatchManager) {
        let desired = self.desired_watches(manager);
        if let Err(error) = manager.reconcile_watches(&desired) {
            tsr_tsc::write_all(
                self.opts.sys.writer().as_ref(),
                format!("{error}\n").as_bytes(),
            );
            manager.force_overflow();
        }
    }
    // port: tsc/internal/execute/build/orchestrator.go:Orchestrator.updateWatch
    fn update_watch(&self) {
        let previous = self.host.m_times.to_map();
        self.host.m_times.clear();
        for node in &self.tasks {
            if let Some(config) = &node.task.resolved {
                if !config.options.no_emit.is_true() && !config.options.is_incremental() {
                    for file in config.as_ref().clone().output_file_names() {
                        let path = self.path(file.as_bytes());
                        if let Some(time) = previous.get(&path) {
                            self.host.m_times.store(path, *time);
                        }
                    }
                }
            }
        }
    }
    // port: tsc/internal/execute/build/orchestrator.go:Orchestrator.resolveBuildInfoFileName
    fn resolve_info_file(&self, file: &[u8], directory: &[u8]) -> Vec<u8> {
        if tsr_incremental::is_build_info_file_name_default_library(file) {
            tsr_tspath::combine(self.host.library.as_bytes(), &[file])
        } else {
            tsr_tspath::absolute(file, directory)
        }
    }
    // port: tsc/internal/execute/build/orchestrator.go:Orchestrator.packageJsonLookupChanged
    fn package_lookup_changed(
        &self,
        package: &[u8],
        changes: &HashMap<JsString, EventKind>,
    ) -> bool {
        let path = self.path(package);
        changes.contains_key(&path)
            || changes.iter().any(|(changed, kind)| {
                *kind == EventKind::EventDelete
                    && tsr_tspath::contains_path(
                        changed.as_bytes(),
                        path.as_bytes(),
                        self.host.cwd.as_bytes(),
                        self.host.case_sensitive,
                    )
            })
    }
    // port: tsc/internal/execute/build/orchestrator.go:Orchestrator.checkTasksForEventChanges
    fn check_event_changes(
        &mut self,
        changes: &HashMap<JsString, EventKind>,
        manager: &WatchManager,
    ) -> Result<(bool, bool), Error> {
        let normalized: HashMap<_, _> = changes
            .iter()
            .map(|(path, kind)| (self.path(path.as_bytes()), *kind))
            .collect();
        let (mut reparse, mut update) = (false, false);
        let mut reloaded = Vec::new();
        for &index in &self.order_indices {
            let task = &self.tasks[index].task;
            if normalized.contains_key(&self.path(task.config.as_bytes())) {
                task.dirty.store(true, Ordering::Release);
                reparse = true;
                update = true;
                continue;
            }
            let Some(config) = &task.resolved else {
                continue;
            };
            let config_changed = config
                .extended_source_files()
                .iter()
                .any(|file| normalized.contains_key(&self.path(file.as_bytes())))
                || config
                    .content_mappers
                    .iter()
                    .flatten()
                    .filter(|mapper| !mapper.package_directory.is_empty())
                    .any(|mapper| {
                        normalized.contains_key(&self.path(&tsr_tspath::combine(
                            mapper.package_directory.as_bytes(),
                            &[b"package.json"],
                        )))
                    });
            if config_changed {
                task.dirty.store(true, Ordering::Release);
                reparse = true;
                update = true;
                continue;
            }
            let mut root_changed = false;
            if let Some(Some(project)) = task.project.get() {
                match project.watched_files() {
                    Err(error) => {
                        *lock(&task.project_error) = Some(Arc::new(error));
                        task.reset_status();
                        update = true;
                        root_changed = true;
                    }
                    Ok(files) => {
                        if files
                            .iter()
                            .any(|file| normalized.contains_key(&self.path(file.as_bytes())))
                        {
                            *lock(&task.project_error) = project.refresh().err().map(Arc::new);
                            task.reset_status();
                            update = true;
                            root_changed = true;
                        }
                    }
                }
            }
            let roots: HashSet<_> = config
                .root_file_names
                .iter()
                .map(|file| self.path(file.as_bytes()))
                .collect();
            if !root_changed && roots.iter().any(|root| normalized.contains_key(root)) {
                task.reset_status();
                update = true;
                root_changed = true;
            }
            if !root_changed {
                let state = lock(&task.state);
                let info_entry = state.info.clone();
                let packages = state.package_jsons.clone();
                drop(state);
                if let Some(entry) = info_entry {
                    if let Some(info) = entry.info {
                        let directory = tsr_tspath::directory(entry.path.as_bytes());
                        let changed_file = info.file_names.iter().flatten().any(|file| {
                            let path =
                                self.path(&self.resolve_info_file(file.as_bytes(), &directory));
                            !roots.contains(&path) && normalized.contains_key(&path)
                        });
                        let changed_package = info
                            .get_package_jsons(&directory)
                            .chain(info.get_missing_package_jsons(&directory))
                            .any(|package| self.package_lookup_changed(&package, &normalized));
                        if changed_file || changed_package {
                            task.reset_status();
                            update = true;
                        }
                    }
                }
                if packages
                    .iter()
                    .any(|package| self.package_lookup_changed(package.as_bytes(), &normalized))
                {
                    task.reset_status();
                    update = true;
                }
            }
            let new_config =
                config.reload_file_names_of_parsed_command_line(self.host.fs.as_ref())?;
            if config.root_file_names != new_config.root_file_names {
                task.reset_status();
                reloaded.push((index, new_config));
                update = true;
            }
        }
        for (index, mut config) in reloaded {
            config.parse_input_output_names();
            Arc::get_mut(&mut self.tasks[index].task)
                .expect("completed graph uniquely owns each task")
                .resolved = Some(Arc::new(config));
        }
        if !update
            && changes.keys().any(|path| {
                self.host
                    .fs
                    .directory_exists(path.as_bytes())
                    .unwrap_or(false)
                    && manager.is_path_under_watch(path.as_bytes(), &self.compare_options())
            })
        {
            for node in &self.tasks {
                node.task.reset_status();
            }
            update = true;
        }
        Ok((reparse, update))
    }
    // port: tsc/internal/execute/build/orchestrator.go:Orchestrator.computeDesiredWatches
    fn desired_watches(&self, manager: &WatchManager) -> HashMap<JsString, bool> {
        let mut desired = DirWatchSet::new(self.compare_options());
        let realpath = |path: &[u8]| {
            self.host
                .fs
                .realpath(path)
                .unwrap_or_else(|_| JsString::from_bytes(path))
        };
        let add = |desired: &mut DirWatchSet, directory: &[u8]| {
            if !desired.covered(directory) && can_watch_directory(directory) {
                desired.set(directory, false);
            }
        };
        for &index in &self.order_indices {
            let task = &self.tasks[index].task;
            desired.set(
                realpath(&tsr_tspath::directory(task.config.as_bytes())).as_bytes(),
                false,
            );
            let Some(config) = &task.resolved else {
                continue;
            };
            for file in config.extended_source_files() {
                desired.set(
                    &tsr_tspath::directory(realpath(file.as_bytes()).as_bytes()),
                    false,
                );
            }
            for (directory, recursive) in config.wildcard_directories().into_iter().flatten() {
                desired.set(realpath(directory.as_bytes()).as_bytes(), *recursive);
            }
            for file in &config.root_file_names {
                add(
                    &mut desired,
                    &tsr_tspath::directory(&tsr_tspath::absolute(
                        file.as_bytes(),
                        self.host.cwd.as_bytes(),
                    )),
                );
                for mapper in config
                    .content_mappers
                    .iter()
                    .flatten()
                    .filter(|mapper| !mapper.package_directory.is_empty())
                {
                    add(&mut desired, mapper.package_directory.as_bytes());
                }
            }
            if let Some(Some(project)) = task.project.get() {
                match project.watched_files() {
                    Ok(files) => {
                        for file in files {
                            add(
                                &mut desired,
                                &tsr_tspath::directory(realpath(file.as_bytes()).as_bytes()),
                            );
                        }
                    }
                    Err(error) => {
                        *lock(&task.project_error) = Some(Arc::new(error));
                    }
                }
            }
            let state = lock(&task.state);
            let entry = state.info.clone();
            let packages = state.package_jsons.clone();
            drop(state);
            if let Some(entry) = entry {
                if let Some(info) = entry.info {
                    let directory = tsr_tspath::directory(entry.path.as_bytes());
                    let roots: HashSet<_> = config
                        .root_file_names
                        .iter()
                        .map(|file| self.path(file.as_bytes()))
                        .collect();
                    for file in info.file_names.iter().flatten() {
                        let file = realpath(&self.resolve_info_file(file.as_bytes(), &directory));
                        if !roots.contains(&self.path(file.as_bytes())) {
                            add(&mut desired, &tsr_tspath::directory(file.as_bytes()));
                        }
                    }
                    for package in info
                        .get_package_jsons(&directory)
                        .chain(info.get_missing_package_jsons(&directory))
                    {
                        self.add_package_watches(&mut desired, &package);
                    }
                }
            }
            for package in packages {
                self.add_package_watches(&mut desired, package.as_bytes());
            }
        }
        manager.resolve_desired_dirs(desired.dirs())
    }
    // port: tsc/internal/execute/build/orchestrator.go:Orchestrator.addPackageJsonWatchDirs
    fn add_package_watches(&self, desired: &mut DirWatchSet, package: &[u8]) {
        let directory = tsr_tspath::directory(package);
        let mut dirs = vec![directory.clone()];
        let mut current = directory.clone();
        let mut found = false;
        loop {
            let parent = tsr_tspath::directory(&current);
            if parent.is_empty() || parent == current {
                break;
            }
            dirs.push(parent.clone());
            if tsr_tspath::base_name(&parent) == b"node_modules" {
                found = true;
                let grandparent = tsr_tspath::directory(&parent);
                if !grandparent.is_empty() && grandparent != parent {
                    dirs.push(grandparent);
                }
                break;
            }
            current = parent;
        }
        if !found {
            dirs = vec![directory];
        }
        for dir in dirs {
            if !desired.covered(&dir) && can_watch_directory(&dir) {
                desired.set(&dir, false);
            }
        }
    }
}
