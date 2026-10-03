//! Retained compiler watch sessions. Each published cycle owns an incremental
//! program; unchanged bound files may be shared through the session's cache.
use crate::{
    watchmanager::{self, ComparePathsOptions, DirWatchSet, WatchManager},
    CommandLineResult, CommandLineTesting, DiagnosticReporter, ExitStatus, System,
};
use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tsr_arena::Counters;
use tsr_ast::Diagnostic;
use tsr_compiler::{CheckedProgram, CompilerConfigHost, Error, FileCache, Program, ProgramOptions};
use tsr_contentmapper::{Host, Project};
use tsr_core::{collections::OrderedMap, CompilerOptions};
use tsr_diagnostics as d;
use tsr_ipc::Context;
use tsr_jsstring::JsString;
use tsr_tsoptions::{ConfigValue, ParsedCommandLine};
use tsr_vfs::{iofs::Time, FileSystem};

pub struct Options {
    pub sys: Arc<dyn System>,
    pub config: ParsedCommandLine,
    pub compiler_options_from_command_line: CompilerOptions,
    pub command_line_raw: ConfigValue,
    pub report_diagnostic: DiagnosticReporter,
    pub testing: Option<Arc<dyn CommandLineTesting>>,
}
struct CompilationHost {
    fs: Arc<dyn FileSystem>,
    cwd: JsString,
    library: JsString,
    project: Option<Arc<dyn Project>>,
}
impl tsr_incremental::CompilerHost for CompilationHost {
    fn fs(&self) -> &dyn FileSystem {
        self.fs.as_ref()
    }
    fn get_current_directory(&self) -> &[u8] {
        self.cwd.as_bytes()
    }
    fn default_library_path(&self) -> &[u8] {
        self.library.as_bytes()
    }
    fn content_mapper_project(&self) -> Option<&Arc<dyn Project>> {
        self.project.as_ref()
    }
}
struct State {
    options: Options,
    status_options: CompilerOptions,
    status_locale: tsr_locale::Locale,
    config_name: JsString,
    config_paths: Vec<JsString>,
    config_mtimes: HashMap<JsString, Time>,
    config_modified: bool,
    config_has_errors: bool,
    watch_set_dirty: bool,
    force_full_rebuild: bool,
    program: Option<tsr_incremental::Program>,
    cache: FileCache,
    counters: Counters,
    seen: HashSet<JsString>,
    mapper_host: Option<tsr_contentmapper::HostImpl>,
    project: Option<Arc<dyn Project>>,
    full_builds: usize,
    fast_path_builds: usize,
    changed_sources: HashSet<JsString>,
}
/// Owns the last completed program and all resources needed for another cycle.
pub struct CompilerWatcher {
    state: Mutex<State>,
    manager: WatchManager,
}
fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}
impl Drop for CompilerWatcher {
    fn drop(&mut self) {
        self.manager.close_all_watches();
    }
}
impl Drop for State {
    fn drop(&mut self) {
        if let Some(project) = &self.project {
            let _ = project.close();
        }
        if let Some(host) = &self.mapper_host {
            let _ = host.close();
        }
    }
}
/// Start with one build. Native sessions then run until their context ends;
/// testing sessions return a retained watcher for deterministic DoCycle calls.
pub fn start(context: &Context, options: Options) -> Result<CommandLineResult, Error> {
    let fs = options.sys.fs();
    let manager = WatchManager::new(
        options.sys.writer(),
        Arc::new(move |dir| fs.directory_exists(dir).unwrap_or(false)),
    );
    if let Some(backend) = options
        .testing
        .as_ref()
        .and_then(|t| t.as_with_watch_backend())
        .map(watchmanager::CommandLineTestingWithWatchBackend::watch_backend)
    {
        manager.set_backend(backend);
    }
    if options
        .sys
        .get_environment_variable("TS_WATCH_DEBUG")
        .is_some_and(|value| !value.is_empty())
    {
        manager.set_debug_log(Some(options.sys.writer()));
    }
    let testing = options.testing.is_some();
    if !testing {
        manager.ensure_default_backend();
    }
    let mapper_host = options.config.options.run_external_code.is_true().then(|| {
        let logger = if options
            .sys
            .get_environment_variable("TS_CONTENT_MAPPER_DEBUG")
            .is_some_and(|v| !v.is_empty())
        {
            let writer = options.sys.error_writer();
            let mutex = Mutex::new(());
            Some(Arc::new(move |text: String| {
                let _guard = lock(&mutex);
                crate::write_all(writer.as_ref(), text.as_bytes());
                crate::write_all(writer.as_ref(), b"\n");
            }) as tsr_contentmapper::Logger)
        } else {
            None
        };
        tsr_contentmapper::new_host_with_options(
            context,
            options.sys.clone(),
            options.config.locale().clone(),
            tsr_contentmapper::HostOptions { logger },
        )
    });
    let config_name = options.config.config_name();
    let config_paths = if config_name.is_empty() {
        Vec::new()
    } else {
        std::iter::once(config_name.clone())
            .chain(options.config.extended_source_files().iter().cloned())
            .collect()
    };
    let mut state = State {
        status_options: options.config.options.clone(),
        status_locale: options.config.locale().clone(),
        options,
        config_name,
        config_paths,
        config_mtimes: HashMap::new(),
        config_modified: false,
        config_has_errors: false,
        watch_set_dirty: true,
        force_full_rebuild: false,
        program: None,
        cache: FileCache::new(),
        counters: Counters::new(),
        seen: HashSet::new(),
        mapper_host,
        project: None,
        full_builds: 0,
        fast_path_builds: 0,
        changed_sources: HashSet::new(),
    };
    state.replace_project();
    let host = state.host(state.options.sys.fs());
    let reader = tsr_incremental::new_build_info_reader(host.clone());
    state.program = tsr_incremental::read_build_info_program(
        &state.options.config,
        reader.as_ref(),
        host.as_ref(),
    );
    state.status(d::Starting_compilation_in_watch_mode, Vec::new())?;
    state.build(&manager)?;
    let watcher = Arc::new(CompilerWatcher {
        state: Mutex::new(state),
        manager: manager.clone(),
    });
    if !testing {
        manager.run_loop(context, || crate::Watcher::do_cycle(watcher.as_ref()))?;
    }
    Ok(CommandLineResult {
        status: ExitStatus::Success,
        watcher: Some(watcher),
    })
}
impl crate::Watcher for CompilerWatcher {
    fn do_cycle(&self) -> Result<(), Error> {
        let _cycle = self.manager.lock();
        lock(&self.state).cycle(&self.manager)
    }
}
impl CompilerWatcher {
    /// port: tsc/internal/execute/watcher.go:Watcher.FastPathBuilds
    pub fn fast_path_builds(&self) -> usize {
        lock(&self.state).fast_path_builds
    }
    /// port: tsc/internal/execute/watcher.go:Watcher.FullBuilds
    pub fn full_builds(&self) -> usize {
        lock(&self.state).full_builds
    }
}
impl State {
    fn host(&self, fs: Arc<dyn FileSystem>) -> Arc<dyn tsr_incremental::CompilerHost> {
        Arc::new(CompilationHost {
            fs,
            cwd: JsString::from_bytes(self.options.sys.get_current_directory()),
            library: JsString::from_bytes(self.options.sys.default_library_path()),
            project: self.project.clone(),
        })
    }
    /// port: tsc/internal/execute/watcher.go:Watcher.comparePathsOptions
    fn compare_options(&self) -> ComparePathsOptions {
        ComparePathsOptions {
            current_directory: JsString::from_bytes(self.options.sys.get_current_directory()),
            use_case_sensitive_file_names: self.options.sys.fs().use_case_sensitive_file_names(),
        }
    }
    fn path(&self, path: &[u8]) -> JsString {
        tsr_tspath::to_path(
            path,
            self.options.sys.get_current_directory(),
            self.options.sys.fs().use_case_sensitive_file_names(),
        )
    }
    fn status(&self, message: &'static d::Message, args: Vec<JsString>) -> Result<(), Error> {
        crate::create_watch_status_reporter(
            self.options.sys.as_ref(),
            self.status_locale.clone(),
            &self.status_options,
            self.options.testing.as_deref(),
        )
        .report(&self.options.config, &Diagnostic::compiler(message, args))
    }
    fn report_error_count(&self, count: usize) -> Result<(), Error> {
        if count == 1 {
            self.status(d::Found_1_error_Watching_for_file_changes, Vec::new())
        } else {
            self.status(
                d::Found_0_errors_Watching_for_file_changes,
                vec![JsString::from_bytes(count.to_string().as_bytes())],
            )
        }
    }
    /// port: tsc/internal/execute/watcher.go:Watcher.replaceContentMapperProject
    fn replace_project(&mut self) {
        let project = self.mapper_host.as_ref().and_then(|host| {
            host.project(tsr_contentmapper::ProjectSpec {
                config_file_name: self.options.config.config_name(),
                mappers: self
                    .options
                    .config
                    .content_mappers
                    .clone()
                    .unwrap_or_default()
                    .into(),
                compiler_options: Arc::new(self.options.config.options.clone()),
            })
        });
        if let Some(old) = self.project.take() {
            let _ = old.close();
        }
        self.project = project;
    }
    // CLI/config mappers have no ContributionID. Plugin-contributed mappers
    // belong to Phase 5; when represented, exclude their manifests as Go does.
    /// port: tsc/internal/execute/watcher.go:Watcher.contentMapperWatchedFiles
    fn mapper_watched_files(&self) -> Result<Vec<JsString>, Error> {
        let mut files: Vec<_> = self
            .options
            .config
            .content_mappers
            .iter()
            .flatten()
            .filter(|mapper| !mapper.package_directory.is_empty())
            .map(|mapper| {
                JsString::from_bytes(tsr_tspath::combine(
                    mapper.package_directory.as_bytes(),
                    &[b"package.json"],
                ))
            })
            .collect();
        if let Some(project) = &self.project {
            match project.watched_files() {
                Ok(dynamic) => files.extend(
                    dynamic
                        .into_iter()
                        .map(|file| JsString::from_bytes(file.into_bytes())),
                ),
                Err(error) => self.options.report_diagnostic.report(
                    &self.options.config,
                    &tsr_compiler::content_mapper_project_diagnostic(&error),
                )?,
            }
        }
        files.sort();
        files.dedup();
        Ok(files)
    }
    /// port: tsc/internal/execute/watcher.go:Watcher.contentMapperManifestChanged
    fn mapper_manifest_changed(
        &self,
        changes: &HashMap<JsString, crate::fswatch::EventKind>,
    ) -> bool {
        self.options
            .config
            .content_mappers
            .iter()
            .flatten()
            .any(|mapper| {
                !mapper.package_directory.is_empty()
                    && changes.contains_key(
                        tsr_tspath::combine(
                            mapper.package_directory.as_bytes(),
                            &[b"package.json"],
                        )
                        .as_slice(),
                    )
            })
    }
    fn notify_program(&self) {
        if let (Some(testing), Some(program)) = (&self.options.testing, &self.program) {
            testing.on_program(program);
        }
    }
    fn cycle(&mut self, manager: &WatchManager) -> Result<(), Error> {
        self.changed_sources.clear();
        let (changes, overflow) = manager.drain_events();
        let has_events = !changes.is_empty() || overflow;
        if self.recheck_config(self.mapper_manifest_changed(&changes))? {
            return Ok(());
        }
        if has_events && !overflow && !self.config_modified {
            if !self.relevant_change(&changes, manager)? {
                manager.debug_log(&format!(
                    "[watch] DoCycle: {} event(s) not relevant to compilation, skipping rebuild\n",
                    changes.len()
                ));
                self.notify_program();
                return Ok(());
            }
            let mapper_files: HashSet<_> = self
                .mapper_watched_files()?
                .iter()
                .map(|file| self.path(file.as_bytes()))
                .collect();
            let mut refresh = false;
            for event in changes.keys() {
                let path = self.path(event.as_bytes());
                self.cache.evict(path.as_bytes());
                if self.options.sys.fs().directory_exists(event.as_bytes())? {
                    self.watch_set_dirty = true;
                    continue;
                }
                if mapper_files.contains(&path) {
                    refresh = true;
                    self.force_full_rebuild = true;
                }
                if self.options.config.config_file.is_some()
                    && self
                        .options
                        .config
                        .possibly_matches_file_name(event.as_bytes())
                    && !self.seen.contains(&path)
                {
                    self.watch_set_dirty = true;
                    self.force_full_rebuild = true;
                    continue;
                }
                if let Some(program) = &self.program {
                    match program.get_source_file(path.as_bytes()) {
                        Some(file)
                            if !file
                                .bound()
                                .view()
                                .source_file()?
                                .content_mapper()
                                .is_empty() =>
                        {
                            self.force_full_rebuild = true;
                        }
                        None if self.seen.contains(&path) => self.force_full_rebuild = true,
                        Some(_) => {
                            self.changed_sources.insert(path);
                        }
                        _ => {}
                    }
                }
            }
            if refresh {
                if let Some(project) = &self.project {
                    if project.refresh().is_err() {
                        self.options.report_diagnostic.report(
                            &self.options.config,
                            &Diagnostic::compiler(
                                d::The_content_mapper_process_could_not_be_started_or_initialized,
                                Vec::new(),
                            ),
                        )?;
                        return Ok(());
                    }
                }
            }
        } else if overflow {
            self.cache = FileCache::new();
            self.watch_set_dirty = true;
            self.force_full_rebuild = true;
        } else if !has_events && !self.config_modified {
            manager.debug_log("[watch] DoCycle: no events, skipping\n");
            self.notify_program();
            return Ok(());
        }
        self.status(
            d::File_change_detected_Starting_incremental_compilation,
            Vec::new(),
        )?;
        self.build(manager)
    }
    /// port: tsc/internal/execute/watcher.go:Watcher.isRelevantChange
    fn relevant_change(
        &self,
        changes: &HashMap<JsString, crate::fswatch::EventKind>,
        manager: &WatchManager,
    ) -> Result<bool, Error> {
        let mapper_files: HashSet<_> = self
            .mapper_watched_files()?
            .iter()
            .map(|file| self.path(file.as_bytes()))
            .collect();
        for event in changes.keys() {
            let path = self.path(event.as_bytes());
            if mapper_files.contains(&path) || self.seen.contains(&path) {
                return Ok(true);
            }
            if self.options.config.config_file.is_some()
                && (self
                    .options
                    .config
                    .possibly_matches_file_name(event.as_bytes())
                    || self
                        .options
                        .config
                        .possibly_matches_directory_name(&tsr_tspath::Path::from(path)))
            {
                return Ok(true);
            }
            if self.options.sys.fs().directory_exists(event.as_bytes())?
                && manager.is_path_under_watch(event.as_bytes(), &self.compare_options())
            {
                return Ok(true);
            }
        }
        Ok(false)
    }
    /// port: tsc/internal/execute/watcher.go:Watcher.recheckTsConfig
    fn recheck_config(&mut self, force: bool) -> Result<bool, Error> {
        if self.config_name.is_empty() {
            return Ok(false);
        }
        if !force && !self.config_has_errors && !self.config_paths.is_empty() {
            let mut changed = false;
            for path in &self.config_paths {
                if self
                    .options
                    .sys
                    .fs()
                    .stat(path.as_bytes())?
                    .map(|s| s.mod_time)
                    != self.config_mtimes.get(path).copied()
                {
                    changed = true;
                    break;
                }
            }
            if !changed {
                return Ok(false);
            }
        }
        let host = CompilerConfigHost::new_live(
            self.options.sys.fs(),
            JsString::from_bytes(self.options.sys.get_current_directory()),
        );
        let cache = tsr_tsoptions::ExtendedConfigCache::new(&host);
        let mut raw = OrderedMap::default();
        raw.insert(
            JsString::from_bytes(b"compilerOptions".as_slice()),
            self.options.command_line_raw.clone(),
        );
        let result = cache.read_config_file(
            self.config_name.as_bytes(),
            &self.options.compiler_options_from_command_line,
            &ConfigValue::Object(raw),
        )?;
        if !result.read_errors.is_empty() {
            for diagnostic in &result.read_errors {
                self.options
                    .report_diagnostic
                    .report(&self.options.config, diagnostic)?;
            }
            self.config_has_errors = true;
            self.report_error_count(result.read_errors.len())?;
            return Ok(true);
        }
        let config = result.command_line.expect("successful configuration read");
        self.config_modified |= self.config_has_errors
            || force
            || self.options.config.options != config.options
            || self.options.config.raw != config.raw
            || self.options.config.root_file_names != config.root_file_names;
        self.config_has_errors = false;
        self.config_paths = std::iter::once(self.config_name.clone())
            .chain(config.extended_source_files().iter().cloned())
            .collect();
        self.options.config = config;
        self.replace_project();
        Ok(false)
    }
    /// port: tsc/internal/execute/watcher.go:Watcher.computeDesiredWatches
    fn desired_watches(
        &self,
        manager: &WatchManager,
        seen: &[JsString],
    ) -> Result<HashMap<JsString, bool>, Error> {
        let fs = self.options.sys.fs();
        let mut desired = HashMap::new();
        if self.options.config.config_file.is_some() {
            for (dir, recursive) in self
                .options
                .config
                .wildcard_directories()
                .into_iter()
                .flatten()
            {
                desired.insert(fs.realpath(dir.as_bytes())?, *recursive);
            }
        }
        if self.options.config.config_file.is_none() && desired.is_empty() {
            desired.insert(
                fs.realpath(self.options.sys.get_current_directory())?,
                false,
            );
        }
        for name in &self.config_paths {
            let path = fs.realpath(name.as_bytes())?;
            desired
                .entry(JsString::from_bytes(tsr_tspath::directory(path.as_bytes())))
                .or_insert(false);
        }
        if self.options.config.config_file.is_none() {
            for name in &self.options.config.root_file_names {
                let path = fs.realpath(&tsr_tspath::absolute(
                    name.as_bytes(),
                    self.options.sys.get_current_directory(),
                ))?;
                desired
                    .entry(JsString::from_bytes(tsr_tspath::directory(path.as_bytes())))
                    .or_insert(false);
            }
        }
        let mut coverage = DirWatchSet::new(self.compare_options());
        for (dir, recursive) in manager.resolve_desired_dirs(&desired) {
            coverage.set(dir.as_bytes(), recursive);
        }
        for file in seen {
            let dir = tsr_tspath::directory(file.as_bytes());
            if !coverage.covered(&dir) && watchmanager::can_watch_directory(&dir) {
                coverage.set(&dir, false);
            }
        }
        Ok(manager.resolve_desired_dirs(coverage.dirs()))
    }
    /// port: tsc/internal/execute/watcher.go:Watcher.doBuild
    fn build(&mut self, manager: &WatchManager) -> Result<(), Error> {
        if self.config_modified {
            self.cache = FileCache::new();
            self.watch_set_dirty = true;
        }
        let mut reloaded_file_names = false;
        if self.watch_set_dirty {
            if self.has_wildcards() {
                let config = self
                    .options
                    .config
                    .reload_file_names_of_parsed_command_line(self.options.sys.fs().as_ref())?;
                reloaded_file_names = true;
                if self.options.config.root_file_names == config.root_file_names {
                    self.watch_set_dirty = false;
                }
                self.options.config = config;
            } else if !self.config_modified {
                self.watch_set_dirty = false;
            }
        }
        // Retain a speculative owner until fallback loading has consumed the
        // weak cache entry. Failed reuse must not parse the same edit twice.
        let mut speculative_file = None;
        if self.full_builds != 0
            && !self.config_modified
            && !self.watch_set_dirty
            && !self.force_full_rebuild
            && self.changed_sources.len() == 1
        {
            let cached = Arc::new(tsr_vfs::cached::CachedFs::new(self.options.sys.fs()));
            let _disable = DisableCache(cached.clone());
            let path = self
                .changed_sources
                .iter()
                .next()
                .expect("one changed source");
            let reuse = self
                .program
                .as_ref()
                .expect("published watch program")
                .get_program()
                .program()
                .reuse_program(
                    path.as_bytes(),
                    cached.clone(),
                    &mut self.cache,
                    &self.counters,
                )?;
            speculative_file = reuse.file;
            if let Some(program) = reuse.program {
                self.fast_path_builds += 1;
                let result = self.publish_and_emit(Arc::new(program))?;
                cached.disable_and_clear_cache();
                self.update_config_mtimes()?;
                self.config_modified = false;
                self.report_error_count(result.diagnostics.len())?;
                self.notify_program();
                return Ok(());
            }
        }
        if self.has_wildcards() && !reloaded_file_names && !self.watch_set_dirty {
            self.options.config = self
                .options
                .config
                .reload_file_names_of_parsed_command_line(self.options.sys.fs().as_ref())?;
        }
        let cached = Arc::new(tsr_vfs::cached::CachedFs::new(self.options.sys.fs()));
        let _disable = DisableCache(cached.clone());
        let tracking = Arc::new(tsr_vfs::tracking::TrackingFs::new(cached.clone()));
        for (dir, _) in self
            .options
            .config
            .wildcard_directories()
            .into_iter()
            .flatten()
        {
            tracking.seen_files.insert(dir.clone());
        }
        for file in self
            .config_paths
            .iter()
            .cloned()
            .chain(self.mapper_watched_files()?)
        {
            tracking.seen_files.insert(file);
        }
        let program = Arc::new(Program::load_live_with_content_mapper_project(
            ProgramOptions {
                config: self.options.config.clone(),
                host: tracking.clone(),
                current_directory: JsString::from_bytes(self.options.sys.get_current_directory()),
                default_library_path: JsString::from_bytes(self.options.sys.default_library_path()),
                skip_module_resolution: false,
                single_threaded: tsr_core::Tristate::UNKNOWN,
            },
            self.project.clone(),
            &mut self.cache,
            &self.counters,
        )?);
        crate::report_resolution_trace(
            &program,
            &crate::get_trace_with_writer_from_sys(
                self.options.sys.writer(),
                self.options.config.locale().clone(),
                self.options.testing.as_deref(),
            ),
        );
        self.full_builds += 1;
        let result = self.publish_and_emit(program)?;
        drop(speculative_file);
        cached.disable_and_clear_cache();
        let seen = tracking.seen_files.to_vec();
        self.seen = seen.iter().map(|file| self.path(file.as_bytes())).collect();
        self.update_config_mtimes()?;
        if let Err(error) = manager.reconcile_watches(&self.desired_watches(manager, &seen)?) {
            crate::write_all(
                self.options.sys.writer().as_ref(),
                format!("{error}\n").as_bytes(),
            );
            manager.force_overflow();
            return Ok(());
        }
        self.watch_set_dirty = false;
        self.config_modified = false;
        self.force_full_rebuild = false;
        self.cache.prune();
        self.report_error_count(result.diagnostics.len())?;
        self.notify_program();
        Ok(())
    }
    fn has_wildcards(&self) -> bool {
        self.options.config.config_file.is_some()
            && self
                .options
                .config
                .wildcard_directories()
                .is_some_and(|dirs| !dirs.is_empty())
    }
    fn update_config_mtimes(&mut self) -> Result<(), Error> {
        self.config_mtimes.clear();
        for path in &self.config_paths {
            if let Some(stat) = self.options.sys.fs().stat(path.as_bytes())? {
                self.config_mtimes.insert(path.clone(), stat.mod_time);
            }
        }
        Ok(())
    }
    /// port: tsc/internal/execute/watcher.go:Watcher.compileAndEmit
    fn publish_and_emit(
        &mut self,
        program: Arc<Program>,
    ) -> Result<crate::CompileAndEmitResult, Error> {
        let host = tsr_incremental::create_host(Arc::new(
            tsr_incremental::ProgramCompilerHost::new(program.clone()),
        ));
        let checked = Arc::new(CheckedProgram::new(program, &self.counters, None));
        let anchor = Instant::now();
        let clock_sys = self.options.sys.clone();
        let now = Arc::new(move || {
            let (seconds, nanos) = clock_sys.now().unix();
            let whole = Duration::from_secs(seconds.unsigned_abs());
            let value = if seconds < 0 {
                anchor.checked_sub(whole)
            } else {
                anchor.checked_add(whole)
            };
            value
                .and_then(|value| value.checked_add(Duration::from_nanos(u64::from(nanos))))
                .expect("watch system time fits incremental clock")
        });
        self.program = Some(tsr_incremental::new_program(
            checked.clone(),
            self.program.as_ref(),
            host,
            Some(now),
            self.options.testing.is_some(),
        )?);
        let program = self.program.as_ref().unwrap();
        crate::emit_files_and_report_errors(crate::EmitInput {
            sys: self.options.sys.as_ref(),
            program_like: program,
            program: &checked,
            incremental: Some(program),
            config: &self.options.config,
            report_diagnostic: &self.options.report_diagnostic,
            times: &mut crate::CompileTimes::default(),
            testing: self.options.testing.as_deref(),
            writer: None,
            skip_error_summary: false,
            write_file: None,
            testing_m_times_cache: None,
        })
    }
}

struct DisableCache(Arc<tsr_vfs::cached::CachedFs>);
impl Drop for DisableCache {
    fn drop(&mut self) {
        self.0.disable_and_clear_cache();
    }
}
