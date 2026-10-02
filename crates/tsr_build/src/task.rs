use crate::*;
use std::collections::HashSet;
use std::sync::OnceLock;
use std::time::{Duration, Instant};
use tsr_compiler::{CheckedProgram, Program, ProgramOptions};
use tsr_contentmapper::Project;
use tsr_incremental::{BuildInfo, BuildInfoFileInfo, BuildInfoReader};
use tsr_vfs::FileSystem;

#[derive(Clone)]
pub(crate) struct BuildInfoEntry {
    pub info: Option<Arc<BuildInfo>>,
    pub path: JsString,
    pub m_time: Time,
    pub dts_time: Option<Time>,
}
#[derive(Default)]
pub(crate) struct TaskState {
    pub status: Option<Status>,
    pub info: Option<BuildInfoEntry>,
    pub package_jsons: Vec<JsString>,
    pub errors: Vec<Diagnostic>,
    pub error_program: Option<Arc<tsr_incremental::Program>>,
}
pub(crate) struct BuildTask {
    pub config: JsString,
    pub resolved: Option<Arc<ParsedCommandLine>>,
    pub config_time: Mutex<Duration>,
    pub state: Mutex<TaskState>,
    pub done: Completion,
    pub report_done: Completion,
    pub pending: AtomicBool,
    pub initial_cycle: AtomicBool,
    pub dirty: AtomicBool,
    pub(crate) project: OnceLock<Option<Arc<dyn Project>>>,
    pub(crate) project_error: Mutex<Option<Arc<tsr_contentmapper::Error>>>,
}
pub(crate) struct TaskResult {
    pub output: Arc<Buffer>,
    pub status: ExitStatus,
    pub errors: Vec<Diagnostic>,
    pub deletes: Vec<JsString>,
    pub program: Option<Arc<tsr_incremental::Program>>,
    pub built: bool,
    pub pseudo: bool,
    pub statistics: Option<tsr_tsc::Statistics>,
}
impl Default for TaskResult {
    fn default() -> Self {
        Self {
            output: Arc::default(),
            status: ExitStatus::Success,
            errors: Vec::new(),
            deletes: Vec::new(),
            program: None,
            built: false,
            pseudo: false,
            statistics: None,
        }
    }
}

impl BuildTask {
    pub fn new(
        config: JsString,
        resolved: Option<ParsedCommandLine>,
        config_time: Duration,
    ) -> Self {
        Self {
            config,
            resolved: resolved.map(|mut config| {
                config.parse_input_output_names();
                Arc::new(config)
            }),
            config_time: Mutex::new(config_time),
            state: Mutex::default(),
            done: Completion::default(),
            report_done: Completion::default(),
            pending: AtomicBool::new(true),
            initial_cycle: AtomicBool::new(true),
            dirty: AtomicBool::new(false),
            project: OnceLock::new(),
            project_error: Mutex::default(),
        }
    }
    pub fn close_project(&self) {
        if let Some(Some(project)) = self.project.get() {
            let _ = project.close();
        }
    }
    // port: tsc/internal/execute/build/buildtask.go:BuildTask.resetStatus
    pub fn reset_status(&self) {
        let mut state = lock(&self.state);
        state.status = None;
        state.errors.clear();
        state.error_program = None;
        self.pending.store(true, Ordering::Release);
    }
    fn project(&self, o: &Orchestrator) -> Option<Arc<dyn Project>> {
        self.project
            .get_or_init(|| {
                let host = o.mapper_host.as_ref()?;
                let config = self.resolved.as_ref()?;
                if config.content_mappers.as_ref().is_none_or(Vec::is_empty) {
                    return None;
                }
                host.project(tsr_contentmapper::ProjectSpec {
                    config_file_name: config.config_name(),
                    mappers: config.content_mappers.clone().unwrap_or_default().into(),
                    compiler_options: Arc::new(config.options.clone()),
                })
            })
            .clone()
    }
    // port: tsc/internal/execute/build/buildtask.go:BuildTask.loadOrStoreBuildInfo
    fn load_info(&self, o: &Orchestrator) -> BuildInfoEntry {
        let config = self.resolved.as_ref().expect("resolved project");
        let file = config.build_info_file_name();
        let path = o.path(file.as_bytes());
        let mut state = lock(&self.state);
        if let Some(info) = &state.info {
            if info.path == path {
                return info.clone();
            }
        }
        let info = o
            .host
            .fs
            .read_file(file.as_bytes())
            .ok()
            .flatten()
            .and_then(|content| {
                let mut info = BuildInfo::default();
                tsr_json::unmarshal(
                    content.text.as_bytes(),
                    &mut info,
                    tsr_json::Options::default(),
                )
                .ok()
                .map(|_| Arc::new(info))
                .map(Some)
            })
            .flatten();
        let m_time = if info.is_some() {
            o.host.m_time(file.as_bytes())
        } else {
            Time::ZERO
        };
        let entry = BuildInfoEntry {
            info,
            path,
            m_time,
            dts_time: None,
        };
        state.info = Some(entry.clone());
        entry
    }
    // port: tsc/internal/execute/build/buildtask.go:BuildTask.getLatestChangedDtsMTime
    fn changed_dts_time(&self, o: &Orchestrator) -> Time {
        let mut state = lock(&self.state);
        let entry = state
            .info
            .as_mut()
            .expect("runtime error: invalid memory address or nil pointer dereference");
        if let Some(time) = entry.dts_time {
            return time;
        }
        let info = entry
            .info
            .as_ref()
            .expect("runtime error: invalid memory address or nil pointer dereference");
        let time = o.host.m_time(&tsr_tspath::absolute(
            info.latest_changed_dts_file.as_bytes(),
            &tsr_tspath::directory(entry.path.as_bytes()),
        ));
        entry.dts_time = Some(time);
        time
    }
    // port: tsc/internal/execute/build/buildtask.go:BuildTask.reportDiagnostic
    fn report_error(
        &self,
        o: &Orchestrator,
        result: &mut TaskResult,
        error: Diagnostic,
    ) -> Result<(), Error> {
        let fallback = o.sources();
        o.diagnostic_report(result.output.clone())
            .report(self.resolved.as_deref().unwrap_or(&fallback), &error)?;
        result.errors.push(error);
        Ok(())
    }

    // port: tsc/internal/execute/build/buildtask.go:BuildTask.buildProject
    pub fn build(&self, o: &Orchestrator, index: usize) -> Result<TaskResult, Error> {
        let mut result = TaskResult::default();
        if !self.pending.load(Ordering::Acquire) {
            let state = lock(&self.state);
            result.errors = state.errors.clone();
            result.program = state.error_program.clone();
            let status = state.status.clone();
            drop(state);
            if !result.errors.is_empty() {
                if let Some(status) = status {
                    status.report(o, self.config.as_bytes(), result.output.clone())?;
                }
                let fallback = o.sources();
                let sources: &dyn DiagnosticSources = if let Some(program) = &result.program {
                    program.program().program().as_ref()
                } else {
                    self.resolved.as_deref().unwrap_or(&fallback)
                };
                let reporter = o.diagnostic_report(result.output.clone());
                for diagnostic in &result.errors {
                    reporter.report(sources, diagnostic)?;
                }
            }
            return Ok(result);
        }
        let mut status = self.up_to_date_status(o, index)?;
        status.report(o, self.config.as_bytes(), result.output.clone())?;
        let handled = self.handle_without_build(o, &mut result, &mut status)?;
        if handled {
            if let Some(config) = &self.resolved {
                for error in config.config_file_parsing_diagnostics() {
                    self.report_error(o, &mut result, error)?;
                }
            }
            if !result.errors.is_empty() {
                result.status = ExitStatus::DiagnosticsPresent_OutputsSkipped;
            }
        } else {
            self.compile(o, &mut result, &mut status)?;
        }
        {
            let mut state = lock(&self.state);
            state.status = Some(status);
            state.errors = result.errors.clone();
            state.error_program = (!result.errors.is_empty())
                .then(|| result.program.clone())
                .flatten();
        }
        if !handled {
            self.update_downstream(o, index, result.program.as_deref());
        }
        Ok(result)
    }

    // port: tsc/internal/execute/build/buildtask.go:BuildTask.updateDownstream
    fn update_downstream(
        &self,
        o: &Orchestrator,
        index: usize,
        program: Option<&tsr_incremental::Program>,
    ) {
        if self.initial_cycle.load(Ordering::Acquire) {
            return;
        }
        if o.opts.command.build_options.stop_build_on_errors.is_true()
            && lock(&self.state)
                .status
                .as_ref()
                .is_some_and(Status::is_error)
        {
            return;
        }
        for &downstream in &o.tasks[index].downstream {
            let task = &o.tasks[downstream].task;
            if let Some(program) = program {
                let mut state = lock(&task.state);
                if let Some(status) = &mut state.status {
                    match status.kind {
                        StatusKind::UpToDate
                        | StatusKind::UpToDateWithUpstreamTypes
                        | StatusKind::UpToDateWithInputFileText => {
                            if program.has_changed_dts_file() {
                                *status = Status::pair(
                                    StatusKind::InputFileNewer,
                                    self.config.clone(),
                                    status.output.clone(),
                                );
                            } else if status.kind == StatusKind::UpToDate {
                                status.kind = StatusKind::UpToDateWithUpstreamTypes;
                            }
                        }
                        StatusKind::UpstreamErrors => {
                            let reference =
                                tsr_tsoptions::resolve_config_file_name_of_project_reference(
                                    status.input.as_bytes(),
                                );
                            if o.path(reference.as_bytes()) == o.path(self.config.as_bytes()) {
                                state.status = None;
                                state.errors.clear();
                                state.error_program = None;
                            }
                        }
                        _ => {}
                    }
                }
            } else {
                task.reset_status();
            }
            task.pending.store(true, Ordering::Release);
        }
    }

    // port: tsc/internal/execute/build/buildtask.go:BuildTask.handleStatusThatDoesntRequireBuild
    fn handle_without_build(
        &self,
        o: &Orchestrator,
        result: &mut TaskResult,
        status: &mut Status,
    ) -> Result<bool, Error> {
        use StatusKind::*;
        let options = &o.opts.command.build_options;
        match status.kind {
            UpToDate => {
                if options.dry.is_true() {
                    o.status_report(
                        result.output.clone(),
                        d::Project_0_is_up_to_date,
                        vec![self.config.clone()],
                    )?;
                }
                return Ok(true);
            }
            UpstreamErrors => {
                if options.verbose.is_true() {
                    let message = if status.ref_has_upstream_errors {
                        d::Skipping_build_of_project_0_because_its_dependency_1_was_not_built
                    } else {
                        d::Skipping_build_of_project_0_because_its_dependency_1_has_errors
                    };
                    o.status_report(
                        result.output.clone(),
                        message,
                        vec![
                            o.relative(self.config.as_bytes()),
                            o.relative(status.input.as_bytes()),
                        ],
                    )?;
                }
                return Ok(true);
            }
            Solution => return Ok(true),
            ConfigFileNotFound => {
                self.report_error(
                    o,
                    result,
                    Diagnostic::compiler(d::File_0_not_found, vec![self.config.clone()]),
                )?;
                return Ok(true);
            }
            _ => {}
        }
        if status.is_pseudo_build() {
            if options.dry.is_true() {
                o.status_report(
                    result.output.clone(),
                    d::A_non_dry_build_would_update_timestamps_for_output_of_project_0,
                    vec![self.config.clone()],
                )?;
                *status = Status::plain(UpToDate);
            } else {
                self.update_timestamps(
                    o,
                    &[],
                    result.output.clone(),
                    d::Updating_output_timestamps_of_project_0,
                )?;
                status.kind = UpToDate;
                result.pseudo = true;
            }
            return Ok(true);
        }
        if options.dry.is_true() {
            o.status_report(
                result.output.clone(),
                d::A_non_dry_build_would_build_project_0,
                vec![self.config.clone()],
            )?;
            *status = Status::plain(UpToDate);
            return Ok(true);
        }
        Ok(false)
    }

    // port: tsc/internal/execute/build/buildtask.go:BuildTask.getUpToDateStatus
    fn up_to_date_status(&self, o: &Orchestrator, index: usize) -> Result<Status, Error> {
        use StatusKind::*;
        if let Some(status) = &lock(&self.state).status {
            return Ok(status.clone());
        }
        let Some(config) = &self.resolved else {
            return Ok(Status::plain(ConfigFileNotFound));
        };
        if config.root_file_names.is_empty() && config.project_references.is_some() {
            return Ok(Status::plain(Solution));
        }
        for (upstream, ref_index) in &o.tasks[index].upstream {
            let state = lock(&o.tasks[*upstream].task.state);
            if let Some(status) = &state.status {
                if o.opts.command.build_options.stop_build_on_errors.is_true() && status.is_error()
                {
                    return Ok(Status {
                        input: config.project_references.as_ref().unwrap()[*ref_index]
                            .path
                            .clone(),
                        ref_has_upstream_errors: status.kind == UpstreamErrors,
                        ..Status::plain(UpstreamErrors)
                    });
                }
            }
        }
        if o.opts.command.build_options.force.is_true() {
            return Ok(Status::plain(ForceBuild));
        }
        let build_info_path = config.build_info_file_name();
        let entry = self.load_info(o);
        let Some(info) = &entry.info else {
            return Ok(Status::file(OutputMissing, build_info_path));
        };
        if !info.is_valid_version() {
            return Ok(Status::file(TsVersionOutOfDate, info.version.clone()));
        }
        let project = self.project(o);
        let identities = tsr_incremental::content_mapper_identities(project.as_deref());
        let identity_changed = match identities {
            Ok(identities) => !info.content_mapper_identities_match(identities.as_deref()),
            Err(error) => {
                *lock(&self.project_error) = Some(Arc::new(error));
                true
            }
        };
        if identity_changed || lock(&self.project_error).is_some() {
            return Ok(Status::file(OutOfDateOptions, build_info_path));
        }
        if info.errors
            || !config.options.no_check.is_true() && (info.semantic_errors || info.check_pending)
        {
            return Ok(Status::file(OutOfDateBuildInfoWithErrors, build_info_path));
        }
        let directory = tsr_tspath::directory(&tsr_tspath::absolute(
            build_info_path.as_bytes(),
            o.host.cwd.as_bytes(),
        ));
        let is_incremental = BuildInfo::is_incremental(Some(info));
        if config.options.is_incremental() {
            if !is_incremental {
                return Ok(Status::file(OutOfDateOptions, build_info_path));
            }
            if config.options.emit_declarations() && info.emit_diagnostics_per_file.is_some()
                || !config.options.no_check.is_true()
                    && (info.change_file_set.is_some()
                        || info.semantic_diagnostics_per_file.is_some())
            {
                return Ok(Status::file(OutOfDateBuildInfoWithErrors, build_info_path));
            }
            if !config.options.no_emit.is_true()
                && (info.change_file_set.is_some() || info.affected_files_pending_emit.is_some())
            {
                return Ok(Status::file(
                    OutOfDateBuildInfoWithPendingEmit,
                    build_info_path,
                ));
            }
            if info.is_emit_pending(config, &directory) {
                return Ok(Status::file(OutOfDateOptions, build_info_path));
            }
        }
        let roots = info.get_build_info_root_info_reader(&directory, o.host.case_sensitive);
        let mut text_unchanged = false;
        let mut oldest_output = build_info_path.clone();
        let mut oldest_time = entry.m_time;
        let mut newest_input = JsString::default();
        let mut newest_time = Time::ZERO;
        let mut seen = HashSet::new();
        for input in &config.root_file_names {
            let time = o.host.m_time(input.as_bytes());
            if time.is_zero() {
                return Ok(Status::file(InputFileMissing, input.clone()));
            }
            let path = o.path(input.as_bytes());
            if time > oldest_time {
                let (file_info, resolved) = roots.get_build_info_file_info(&path);
                let version = if is_incremental {
                    BuildInfoFileInfo::get_file_info(file_info)
                        .map(|value| value.version().clone())
                        .unwrap_or_default()
                } else {
                    JsString::default()
                };
                if version.is_empty() || !self.same_text(o, resolved.as_bytes(), &version) {
                    return Ok(Status::pair(InputFileNewer, input.clone(), build_info_path));
                }
                text_unchanged = true;
            }
            if time > newest_time {
                newest_input = input.clone();
                newest_time = time;
            }
            seen.insert(path);
        }
        for root in roots.roots() {
            if !seen.contains(root) {
                return Ok(Status::pair(OutOfDateRoots, root.clone(), build_info_path));
            }
        }
        if is_incremental {
            let resolved_roots: HashSet<_> = roots
                .roots()
                .map(|root| roots.get_build_info_file_info(root).1)
                .filter(|root| !root.is_empty())
                .collect();
            for (index, file_info) in info.file_infos.iter().flatten().enumerate() {
                let file_name = &info.file_names.as_ref().expect("incremental filenames")[index];
                if tsr_incremental::is_build_info_file_name_default_library(file_name.as_bytes()) {
                    continue;
                }
                let input =
                    JsString::from_bytes(tsr_tspath::absolute(file_name.as_bytes(), &directory));
                let path = o.path(input.as_bytes());
                if seen.contains(&path) || resolved_roots.contains(&path) {
                    continue;
                }
                if is_content_mapper_supplemental_build_info_path(
                    path.as_bytes(),
                    roots.roots().map(JsString::as_bytes),
                ) && !o.host.fs.file_exists(input.as_bytes()).unwrap_or(false)
                {
                    continue;
                }
                let time = o.host.m_time(input.as_bytes());
                if time.is_zero() {
                    return Ok(Status::file(InputFileMissing, input));
                }
                if time > oldest_time {
                    let version = BuildInfoFileInfo::get_file_info(Some(file_info))
                        .expect("build info file info")
                        .version()
                        .clone();
                    if version.is_empty() || !self.same_text(o, input.as_bytes(), &version) {
                        return Ok(Status::pair(InputFileNewer, input, build_info_path));
                    }
                    text_unchanged = true;
                }
            }
        }
        if !config.options.is_incremental() {
            for output in config.as_ref().clone().output_file_names() {
                let time = o.host.m_time(output.as_bytes());
                if time.is_zero() {
                    return Ok(Status::file(OutputMissing, output));
                }
                if time < newest_time {
                    return Ok(Status::pair(InputFileNewer, newest_input, output));
                }
                if time < oldest_time {
                    oldest_output = output;
                    oldest_time = time;
                }
            }
        }
        let mut dts_unchanged = false;
        for (upstream, ref_index) in &o.tasks[index].upstream {
            let task = &o.tasks[*upstream].task;
            let upstream_state = lock(&task.state);
            if upstream_state
                .status
                .as_ref()
                .is_some_and(|status| status.kind == Solution)
            {
                continue;
            }
            if upstream_state.status.as_ref().is_some_and(|status| {
                status.has_times && !status.input_time.is_zero() && status.input_time < oldest_time
            }) {
                continue;
            }
            let conflicting = upstream_state
                .info
                .as_ref()
                .is_some_and(|upstream| upstream.path == entry.path);
            drop(upstream_state);
            if !conflicting {
                let dts_time = task.changed_dts_time(o);
                if !dts_time.is_zero() && dts_time < oldest_time {
                    dts_unchanged = true;
                    continue;
                }
            }
            return Ok(Status::pair(
                InputFileNewer,
                config.project_references.as_ref().unwrap()[*ref_index]
                    .path
                    .clone(),
                oldest_output,
            ));
        }
        for input in std::iter::once(&self.config).chain(config.extended_source_files()) {
            if o.host.m_time(input.as_bytes()) > oldest_time {
                return Ok(Status::pair(InputFileNewer, input.clone(), oldest_output));
            }
        }
        for package in info.get_package_jsons(&directory) {
            let time = o.host.m_time(&package);
            if time.is_zero() {
                return Ok(Status::file(
                    InputFileMissing,
                    JsString::from_bytes(package),
                ));
            }
            if time > oldest_time {
                return Ok(Status::pair(
                    InputFileNewer,
                    JsString::from_bytes(package),
                    oldest_output,
                ));
            }
        }
        for package in info.get_missing_package_jsons(&directory) {
            if !o.host.m_time(&package).is_zero() {
                return Ok(Status::pair(
                    InputFileNewer,
                    JsString::from_bytes(package),
                    oldest_output,
                ));
            }
        }
        lock(&self.state).package_jsons = info
            .get_package_jsons(&directory)
            .chain(info.get_missing_package_jsons(&directory))
            .map(JsString::from_bytes)
            .collect();
        Ok(Status {
            kind: if dts_unchanged {
                UpToDateWithUpstreamTypes
            } else if text_unchanged {
                UpToDateWithInputFileText
            } else {
                UpToDate
            },
            input: newest_input,
            output: oldest_output,
            input_time: newest_time,
            output_time: oldest_time,
            build_info: build_info_path,
            has_times: true,
            ref_has_upstream_errors: false,
        })
    }
    fn same_text(&self, o: &Orchestrator, file: &[u8], version: &JsString) -> bool {
        o.host
            .fs
            .read_file(file)
            .ok()
            .flatten()
            .is_some_and(|content| {
                tsr_incremental::compute_hash(content.text.as_bytes(), o.opts.testing.is_some())
                    == *version
            })
    }

    // port: tsc/internal/execute/build/buildtask.go:BuildTask.compileAndEmit
    fn compile(
        &self,
        o: &Orchestrator,
        result: &mut TaskResult,
        status: &mut Status,
    ) -> Result<(), Error> {
        let config = self.resolved.as_ref().expect("compile resolved project");
        if o.opts.command.build_options.verbose.is_true() {
            o.status_report(
                result.output.clone(),
                d::Building_project_0,
                vec![o.relative(self.config.as_bytes())],
            )?;
        }
        let mut times = tsr_tsc::CompileTimes {
            config_time: *lock(&self.config_time),
            ..Default::default()
        };
        let start = o.opts.sys.now();
        let project = self.project(o);
        if let Some(error) = lock(&self.project_error).clone() {
            self.report_error(
                o,
                result,
                tsr_compiler::content_mapper_project_diagnostic(&error),
            )?;
            *status = Status::plain(StatusKind::BuildErrors);
            result.status = ExitStatus::DiagnosticsPresent_OutputsSkipped;
            return Ok(());
        }
        let compiler_host = host::CompilerHost {
            host: o.host.clone(),
            project: project.clone(),
        };
        let old = if o.opts.command.build_options.force.is_true() {
            None
        } else {
            tsr_incremental::read_build_info_program(
                config,
                &TaskReader {
                    task: self,
                    orchestrator: o,
                },
                &compiler_host,
            )
        };
        times.build_info_read_time = tsr_tsc::elapsed(o.opts.sys.now(), start);
        let start = o.opts.sys.now();
        let counters = tsr_arena::Counters::new();
        let program = Arc::new(Program::load_live_with_host_services(
            ProgramOptions {
                config: config.as_ref().clone(),
                host: o.host.fs.clone(),
                current_directory: o.host.cwd.clone(),
                default_library_path: o.host.library.clone(),
                skip_module_resolution: false,
                single_threaded: tsr_core::Tristate::UNKNOWN,
            },
            tsr_compiler::ProgramHostServices {
                content_mapper_project: project,
                resolved_project_references: Some(o),
                ..Default::default()
            },
            &mut lock(&o.source_files),
            &counters,
        )?);
        let trace = tsr_tsc::get_trace_with_writer_from_sys(
            result.output.clone(),
            o.opts.command.locale().clone(),
            o.opts.testing.as_deref(),
        );
        tsr_tsc::report_resolution_trace(&program, &trace);
        let program = Arc::new(CheckedProgram::new(program, &counters, None));
        times.parse_time = tsr_tsc::elapsed(o.opts.sys.now(), start);
        let start = o.opts.sys.now();
        let anchor = Instant::now();
        let sys = o.opts.sys.clone();
        let clock = Arc::new(move || {
            let (seconds, nanos) = sys.now().unix();
            let whole = Duration::from_secs(seconds.unsigned_abs());
            let instant = if seconds < 0 {
                anchor.checked_sub(whole)
            } else {
                anchor.checked_add(whole)
            };
            instant
                .and_then(|instant| instant.checked_add(Duration::from_nanos(u64::from(nanos))))
                .expect("system clock fits nested emit clock")
        });
        let incremental = Arc::new(tsr_incremental::new_program(
            program.clone(),
            old.as_ref(),
            o.host.clone(),
            Some(clock),
            o.opts.testing.is_some(),
        )?);
        times.changes_compute_time = tsr_tsc::elapsed(o.opts.sys.now(), start);
        let write_file = |file: &[u8], text: &[u8], data: &mut tsr_compiler::WriteFileData| {
            o.host.fs.write_file(file, text)?;
            if let Some(info) = data
                .build_info
                .as_ref()
                .and_then(|info| info.downcast_ref::<BuildInfo>())
            {
                let time = o.opts.sys.now();
                let mut state = lock(&self.state);
                let dts_time = if incremental.has_changed_dts_file() {
                    Some(time)
                } else {
                    state.info.as_ref().and_then(|entry| entry.dts_time)
                };
                state.info = Some(BuildInfoEntry {
                    info: Some(Arc::new(info.clone())),
                    path: o.path(file),
                    m_time: time,
                    dts_time,
                });
            } else if self.store_output_time(o) {
                o.host.store_m_time(file, o.opts.sys.now());
            }
            Ok(())
        };
        let writer: SharedWriter = result.output.clone();
        let report = o.diagnostic_report(writer.clone());
        let emitted = tsr_tsc::emit_and_report_statistics(tsr_tsc::EmitInput {
            sys: o.opts.sys.as_ref(),
            program_like: incremental.as_ref(),
            program: &program,
            incremental: Some(&incremental),
            config,
            report_diagnostic: &report,
            times: &mut times,
            testing: o.opts.testing.as_deref(),
            writer: Some(&writer),
            skip_error_summary: true,
            write_file: Some(&write_file),
            testing_m_times_cache: Some(&o.host.m_times),
        })?;
        result.status = emitted.status;
        result.statistics = emitted.statistics;
        result.errors = emitted.diagnostics;
        lock(&self.state).package_jsons =
            incremental.package_json_lookup_paths().unwrap_or_default();
        if (!config.options.no_emit_on_error.is_true() || result.errors.is_empty())
            && (!emitted.emit_result.emitted_files.is_empty()
                || status.kind != StatusKind::OutOfDateBuildInfoWithErrors)
        {
            self.update_timestamps(
                o,
                &emitted.emit_result.emitted_files,
                result.output.clone(),
                d::Updating_unchanged_output_timestamps_of_project_0,
            )?;
        }
        *status = if matches!(
            result.status,
            ExitStatus::DiagnosticsPresent_OutputsSkipped
                | ExitStatus::DiagnosticsPresent_OutputsGenerated
        ) {
            Status::plain(StatusKind::BuildErrors)
        } else {
            Status::file(
                StatusKind::UpToDate,
                emitted
                    .emit_result
                    .emitted_files
                    .first()
                    .cloned()
                    .or_else(|| {
                        config
                            .as_ref()
                            .clone()
                            .output_file_names()
                            .into_iter()
                            .next()
                    })
                    .unwrap_or_default(),
            )
        };
        result.program = Some(incremental);
        result.built = true;
        Ok(())
    }

    // port: tsc/internal/execute/build/buildtask.go:BuildTask.storeOutputTimeStamp
    fn store_output_time(&self, o: &Orchestrator) -> bool {
        o.opts.command.compiler_options.watch.is_true()
            && !self.resolved.as_ref().unwrap().options.is_incremental()
    }
    // port: tsc/internal/execute/build/buildtask.go:BuildTask.updateTimeStamps
    fn update_timestamps(
        &self,
        o: &Orchestrator,
        emitted: &[JsString],
        writer: SharedWriter,
        message: &'static d::Message,
    ) -> Result<(), Error> {
        let config = self.resolved.as_ref().unwrap();
        let build_info = config.build_info_file_name();
        let time = o.opts.sys.now();
        let mut reported = false;
        let mut files = if !config.options.no_emit.is_true() && !config.options.is_incremental() {
            config.as_ref().clone().output_file_names()
        } else {
            Vec::new()
        };
        files.push(build_info.clone());
        for file in files {
            if emitted.contains(&file) {
                continue;
            }
            if !reported && o.opts.command.build_options.verbose.is_true() {
                o.status_report(
                    writer.clone(),
                    message,
                    vec![o.relative(self.config.as_bytes())],
                )?;
                reported = true;
            }
            if o.host
                .fs
                .change_times(file.as_bytes(), Time::ZERO, time)
                .is_ok()
            {
                if file == build_info {
                    if let Some(info) = &mut lock(&self.state).info {
                        info.m_time = time;
                    }
                } else if self.store_output_time(o) {
                    o.host.store_m_time(file.as_bytes(), time);
                }
            }
        }
        Ok(())
    }

    // port: tsc/internal/execute/build/buildtask.go:BuildTask.cleanProject
    pub fn clean(&self, o: &Orchestrator) -> Result<TaskResult, Error> {
        let mut result = TaskResult::default();
        let Some(config) = &self.resolved else {
            self.report_error(
                o,
                &mut result,
                Diagnostic::compiler(d::File_0_not_found, vec![self.config.clone()]),
            )?;
            result.status = ExitStatus::DiagnosticsPresent_OutputsSkipped;
            return Ok(result);
        };
        let inputs: HashSet<_> = config
            .root_file_names
            .iter()
            .map(|file| o.path(file.as_bytes()))
            .collect();
        let outputs = config
            .as_ref()
            .clone()
            .output_file_names()
            .into_iter()
            .chain(std::iter::once(config.build_info_file_name()));
        for file in outputs {
            if inputs.contains(&o.path(file.as_bytes()))
                || !o.host.fs.file_exists(file.as_bytes()).unwrap_or(false)
            {
                continue;
            }
            if o.opts.command.build_options.dry.is_true() {
                result.deletes.push(file);
            } else if o.host.fs.remove(file.as_bytes()).is_err() {
                self.report_error(
                    o,
                    &mut result,
                    Diagnostic::compiler(d::Failed_to_delete_file_0, vec![file]),
                )?;
            }
        }
        Ok(result)
    }
}

struct TaskReader<'a> {
    task: &'a BuildTask,
    orchestrator: &'a Orchestrator,
}
impl BuildInfoReader for TaskReader<'_> {
    // port: tsc/internal/execute/build/host.go:host.ReadBuildInfo
    fn read_build_info(&self, _: &ParsedCommandLine) -> Option<BuildInfo> {
        self.task
            .load_info(self.orchestrator)
            .info
            .map(|info| info.as_ref().clone())
    }
}

// port: tsc/internal/execute/build/buildtask.go:isContentMapperSupplementalBuildInfoPath
pub fn is_content_mapper_supplemental_build_info_path<'a>(
    input: &[u8],
    roots: impl IntoIterator<Item = &'a [u8]>,
) -> bool {
    roots.into_iter().any(|root| {
        let Some(suffix) = input
            .strip_prefix(root)
            .and_then(|suffix| suffix.strip_prefix(b"."))
        else {
            return false;
        };
        let Some(dot) = suffix.iter().position(|byte| *byte == b'.') else {
            return false;
        };
        let index = &suffix[..dot];
        let extension = &suffix[dot..];
        std::str::from_utf8(index)
            .ok()
            .and_then(|index| index.parse::<isize>().ok())
            .is_some()
            && tsr_contentmapper::is_supported_virtual_extension(extension)
    })
}
