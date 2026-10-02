//! Project graph, bounded project builders and ordered build transcripts.
//! Port of `tsc/internal/execute/build`; the graph owns every diagnostic source
//! until the final cross-project error summary has been formatted.
mod graph;
mod host;
mod status;
mod task;
#[cfg(test)]
mod tests;
mod watch;

use std::collections::HashMap;
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Arc, Condvar, Mutex, MutexGuard,
};
use tsr_ast::Diagnostic;
use tsr_compiler::{diagnostic_writer::DiagnosticSources, Error, FileCache};
use tsr_contentmapper::{Host as _, HostImpl};
use tsr_core::workgroup::WorkGroup;
use tsr_diagnostics as d;
use tsr_ipc::Context;
use tsr_jsstring::JsString;
use tsr_tsc::{CommandLineResult, CommandLineTesting, ExitStatus, SharedWriter, System, Writer};
use tsr_tsoptions::{ParsedBuildCommandLine, ParsedCommandLine};
use tsr_vfs::iofs::Time;

use host::BuildHost;
pub use status::{Status, StatusKind};
pub use task::is_content_mapper_supplemental_build_info_path;
use task::{BuildTask, TaskResult};
pub use watch::start;

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

pub struct Options {
    pub sys: Arc<dyn System>,
    pub command: ParsedBuildCommandLine,
    pub testing: Option<Arc<dyn CommandLineTesting>>,
}

#[derive(Default)]
struct Buffer(Mutex<Vec<u8>>);
impl Writer for Buffer {
    fn write(&self, bytes: &[u8]) -> std::io::Result<usize> {
        lock(&self.0).extend_from_slice(bytes);
        Ok(bytes.len())
    }
}

#[derive(Default)]
struct Completion {
    done: Mutex<bool>,
    changed: Condvar,
}
impl Completion {
    fn wait(&self) {
        let mut done = lock(&self.done);
        while !*done {
            done = self
                .changed
                .wait(done)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
        }
    }
    fn finish(&self) {
        *lock(&self.done) = true;
        self.changed.notify_all();
    }
    fn reset(&self) {
        *lock(&self.done) = false;
    }
}

struct Node {
    task: Arc<BuildTask>,
    upstream: Vec<(usize, usize)>,
    downstream: Vec<usize>,
}

pub struct Orchestrator {
    opts: Options,
    host: Arc<BuildHost>,
    tasks: Vec<Node>,
    by_path: HashMap<JsString, usize>,
    order: Vec<JsString>,
    order_indices: Vec<usize>,
    errors: Vec<Diagnostic>,
    mapper_host: Option<HostImpl>,
    source_files: Mutex<FileCache>,
    #[cfg(test)]
    task_observer: Option<Arc<dyn Fn(bool) + Send + Sync>>,
}

#[cfg(test)]
struct TaskObservation<'a>(&'a (dyn Fn(bool) + Send + Sync));
#[cfg(test)]
impl Drop for TaskObservation<'_> {
    fn drop(&mut self) {
        (self.0)(false);
    }
}

impl Drop for Orchestrator {
    fn drop(&mut self) {
        for node in &self.tasks {
            node.task.close_project();
        }
        if let Some(host) = &self.mapper_host {
            let _ = host.close();
        }
    }
}

impl Orchestrator {
    // port: tsc/internal/execute/build/orchestrator.go:NewOrchestrator
    pub fn new(opts: Options) -> Self {
        let host = Arc::new(BuildHost::new(opts.sys.as_ref()));
        Self {
            opts,
            host,
            tasks: Vec::new(),
            by_path: HashMap::new(),
            order: Vec::new(),
            order_indices: Vec::new(),
            errors: Vec::new(),
            mapper_host: None,
            source_files: Mutex::new(FileCache::new()),
            #[cfg(test)]
            task_observer: None,
        }
    }
    // port: tsc/internal/execute/build/orchestrator.go:Orchestrator.Order
    pub fn order(&self) -> &[JsString] {
        &self.order
    }
    pub fn errors(&self) -> &[Diagnostic] {
        &self.errors
    }
    // port: tsc/internal/execute/build/orchestrator.go:Orchestrator.Upstream
    pub fn upstream(&self, config: &[u8]) -> Vec<JsString> {
        self.tasks[self.index(config)]
            .upstream
            .iter()
            .map(|(index, _)| self.tasks[*index].task.config.clone())
            .collect()
    }
    // port: tsc/internal/execute/build/orchestrator.go:Orchestrator.Downstream
    pub fn downstream(&self, config: &[u8]) -> Vec<JsString> {
        self.tasks[self.index(config)]
            .downstream
            .iter()
            .map(|index| self.tasks[*index].task.config.clone())
            .collect()
    }
    pub fn status(&self, config: &[u8]) -> Option<Status> {
        lock(&self.tasks[self.index(config)].task.state)
            .status
            .clone()
    }
    pub fn invalidate_project(&self, config: &[u8], reparse: bool) {
        let task = &self.tasks[self.index(config)].task;
        task.reset_status();
        if reparse {
            task.dirty.store(true, Ordering::Release);
        }
    }
    pub fn reset_caches(&self) {
        self.host.fs.clear_cache();
        *lock(&self.source_files) = FileCache::new();
        for node in &self.tasks {
            *lock(&node.task.config_time) = std::time::Duration::ZERO;
        }
    }
    fn index(&self, config: &[u8]) -> usize {
        *self.by_path.get(&self.path(config)).unwrap_or_else(|| {
            panic!(
                "No build task found for {}",
                String::from_utf8_lossy(config)
            )
        })
    }
    // port: tsc/internal/execute/build/orchestrator.go:Orchestrator.toPath
    fn path(&self, file: &[u8]) -> JsString {
        self.host.path(file)
    }
    // port: tsc/internal/execute/build/orchestrator.go:Orchestrator.relativeFileName
    fn relative(&self, file: &[u8]) -> JsString {
        JsString::from_bytes(tsr_tspath::convert_to_relative_path(
            file,
            self.opts.sys.get_current_directory(),
            self.host.case_sensitive,
        ))
    }
    fn sources(&self) -> ParsedCommandLine {
        ParsedCommandLine::new(self.opts.command.compiler_options.clone(), Vec::new())
    }
    fn status_report(
        &self,
        writer: SharedWriter,
        message: &'static d::Message,
        args: Vec<JsString>,
    ) -> Result<(), Error> {
        tsr_tsc::create_builder_status_reporter(
            self.opts.sys.as_ref(),
            writer,
            self.opts.command.locale().clone(),
            &self.opts.command.compiler_options,
            self.opts.testing.as_deref(),
        )
        .report(&self.sources(), &Diagnostic::compiler(message, args))
    }
    // port: tsc/internal/execute/build/orchestrator.go:Orchestrator.createDiagnosticReporter
    fn diagnostic_report(&self, writer: SharedWriter) -> tsr_tsc::DiagnosticReporter {
        tsr_tsc::create_diagnostic_reporter(
            self.opts.sys.as_ref(),
            writer,
            self.opts.command.locale().clone(),
            &self.opts.command.compiler_options,
        )
    }

    // port: tsc/internal/execute/build/orchestrator.go:Orchestrator.Start
    pub fn start(&mut self, ctx: &Context) -> Result<CommandLineResult, Error> {
        if self
            .opts
            .command
            .compiler_options
            .run_external_code
            .is_true()
            && self.mapper_host.is_none()
        {
            let logger = self
                .opts
                .sys
                .get_environment_variable("TS_CONTENT_MAPPER_DEBUG")
                .filter(|value| !value.is_empty())
                .map(|_| {
                    let writer = self.opts.sys.error_writer();
                    let mutex = Mutex::new(());
                    Arc::new(move |text: String| {
                        let _guard = lock(&mutex);
                        tsr_tsc::write_all(writer.as_ref(), text.as_bytes());
                        tsr_tsc::write_all(writer.as_ref(), b"\n");
                    }) as tsr_contentmapper::Logger
                });
            self.mapper_host = Some(tsr_contentmapper::new_host_with_options(
                ctx,
                self.opts.sys.clone(),
                self.opts.command.locale().clone(),
                tsr_contentmapper::HostOptions { logger },
            ));
        }
        self.generate_graph()?;
        self.build_or_clean(ctx)
    }

    // port: tsc/internal/execute/build/orchestrator.go:Orchestrator.buildOrClean
    pub fn build_or_clean(&self, ctx: &Context) -> Result<CommandLineResult, Error> {
        let options = &self.opts.command.build_options;
        if !options.clean.is_true() && options.verbose.is_true() {
            let projects = self
                .order
                .iter()
                .flat_map(|path| {
                    [
                        b"\r\n    * ".as_slice(),
                        self.relative(path.as_bytes()).as_bytes(),
                    ]
                    .concat()
                })
                .collect::<Vec<_>>();
            self.status_report(
                self.opts.sys.writer(),
                d::Projects_in_this_build_Colon_0,
                vec![JsString::from_bytes(projects)],
            )?;
        }
        if !self.errors.is_empty() {
            let sources = self.sources();
            let reporter = self.diagnostic_report(self.opts.sys.writer());
            for error in &self.errors {
                reporter.report(&sources, error)?;
            }
            self.report_summary(&sources, &self.errors)?;
            self.report_statistics(&mut tsr_tsc::Statistics::default());
            return Ok(CommandLineResult {
                status: ExitStatus::ProjectReferenceCycle_OutputsSkipped,
                watcher: None,
            });
        }
        for node in &self.tasks {
            node.task.done.reset();
            node.task.report_done.reset();
        }
        let current = AtomicUsize::new(0);
        let failure = Mutex::new(None);
        let first_panic = Mutex::new(None);
        let results: Vec<_> = self.tasks.iter().map(|_| Mutex::new(None)).collect();
        let run = || {
            loop {
                let position = current.fetch_add(1, Ordering::Relaxed);
                let Some(&index) = self.order_indices.get(position) else {
                    break;
                };
                let task = &self.tasks[index].task;
                #[cfg(test)]
                let _observation = self.task_observer.as_deref().map(|observer| {
                    observer(true);
                    TaskObservation(observer)
                });
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    for (upstream, _) in &self.tasks[index].upstream {
                        self.tasks[*upstream].task.done.wait();
                    }
                    // Watch cancellation governs the event loop; the pin still
                    // performs its initial build even when the context is done.
                    if ctx.err().is_some() && !self.opts.command.compiler_options.watch.is_true() {
                        lock(&task.state).status = Some(Status::plain(StatusKind::BuildErrors));
                        return Ok(TaskResult {
                            status: ExitStatus::DiagnosticsPresent_OutputsSkipped,
                            ..TaskResult::default()
                        });
                    }
                    if options.clean.is_true() {
                        task.clean(self)
                    } else {
                        task.build(self, index)
                    }
                }));
                let result = match result {
                    Ok(Ok(result)) => result,
                    Ok(Err(error)) => {
                        lock(&failure).get_or_insert(error);
                        lock(&task.state).status = Some(Status::plain(StatusKind::BuildErrors));
                        TaskResult {
                            status: ExitStatus::DiagnosticsPresent_OutputsSkipped,
                            ..TaskResult::default()
                        }
                    }
                    Err(panic) => {
                        lock(&first_panic).get_or_insert(panic);
                        lock(&task.state).status = Some(Status::plain(StatusKind::BuildErrors));
                        TaskResult {
                            status: ExitStatus::DiagnosticsPresent_OutputsSkipped,
                            ..TaskResult::default()
                        }
                    }
                };
                task.pending.store(false, Ordering::Release);
                task.initial_cycle.store(false, Ordering::Release);
                task.done.finish();
                // A reporter cannot overtake the preceding graph entry, even
                // when that entry failed. No task lock is held while waiting.
                if position != 0 {
                    self.tasks[self.order_indices[position - 1]]
                        .task
                        .report_done
                        .wait();
                }
                let report = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    tsr_tsc::write_all(self.opts.sys.writer().as_ref(), &lock(&result.output.0));
                    if let (Some(testing), Some(program), true) =
                        (&self.opts.testing, &result.program, result.built)
                    {
                        testing.on_program(program);
                    }
                }));
                if let Err(panic) = report {
                    lock(&first_panic).get_or_insert(panic);
                }
                *lock(&results[index]) = Some(result);
                task.report_done.finish();
            }
        };
        let builders = if self.opts.command.compiler_options.single_threaded.is_true() {
            1
        } else {
            options.builders.unwrap_or(4)
        };
        if builders == 1 {
            run();
        } else {
            let group = WorkGroup::new(false);
            for _ in 0..builders {
                group.queue(run);
            }
            group.run_and_wait();
        }
        if let Some(panic) = first_panic
            .into_inner()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
        {
            std::panic::resume_unwind(panic);
        }
        if let Some(error) = failure
            .into_inner()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
        {
            return Err(error);
        }
        let results: Vec<_> = results
            .into_iter()
            .map(|result| {
                result
                    .into_inner()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
            })
            .collect();
        let mut status = ExitStatus::Success;
        let mut errors = Vec::new();
        let mut deletes = Vec::new();
        let mut statistics = tsr_tsc::Statistics {
            projects: self.order.len() as i64,
            ..Default::default()
        };
        for &index in &self.order_indices {
            if let Some(result) = &results[index] {
                status.0 = status.0.max(result.status.0);
                errors.extend_from_slice(&result.errors);
                deletes.extend(result.deletes.iter().cloned());
                if let Some(task_statistics) = &result.statistics {
                    statistics.aggregate(task_statistics);
                }
                statistics.projects_built += i64::from(result.built);
                statistics.timestamp_updates += i64::from(result.pseudo);
            }
        }
        let sources = BuildSources {
            orchestrator: self,
            results: &results,
        };
        self.report_summary(&sources, &errors)?;
        if !deletes.is_empty() {
            let files = deletes
                .iter()
                .flat_map(|file| [b"\r\n * ".as_slice(), file.as_bytes()].concat())
                .collect::<Vec<_>>();
            self.status_report(
                self.opts.sys.writer(),
                d::A_non_dry_build_would_delete_the_following_files_Colon_0,
                vec![JsString::from_bytes(files)],
            )?;
        }
        self.report_statistics(&mut statistics);
        Ok(CommandLineResult {
            status,
            watcher: None,
        })
    }
    fn report_statistics(&self, statistics: &mut tsr_tsc::Statistics) {
        if self.opts.command.compiler_options.diagnostics.is_true()
            || self
                .opts
                .command
                .compiler_options
                .extended_diagnostics
                .is_true()
        {
            statistics.set_total_time(self.opts.sys.since_start());
            statistics.report(&self.opts.sys.writer(), self.opts.testing.as_deref());
        }
    }
    fn report_summary(
        &self,
        sources: &dyn DiagnosticSources,
        errors: &[Diagnostic],
    ) -> Result<(), Error> {
        if self.opts.command.compiler_options.watch.is_true() {
            let message = if errors.len() == 1 {
                d::Found_1_error_Watching_for_file_changes
            } else {
                d::Found_0_errors_Watching_for_file_changes
            };
            tsr_tsc::create_watch_status_reporter(
                self.opts.sys.as_ref(),
                self.opts.command.locale().clone(),
                &self.opts.command.compiler_options,
                self.opts.testing.as_deref(),
            )
            .report(
                sources,
                &Diagnostic::compiler(
                    message,
                    vec![JsString::from_bytes(errors.len().to_string().into_bytes())],
                ),
            )
        } else {
            tsr_tsc::create_report_error_summary(
                self.opts.sys.as_ref(),
                self.opts.command.locale().clone(),
                &self.opts.command.compiler_options,
            )
            .report(sources, errors)
        }
    }
}

struct BuildSources<'a> {
    orchestrator: &'a Orchestrator,
    results: &'a [Option<TaskResult>],
}
impl DiagnosticSources for BuildSources<'_> {
    fn diagnostic_source(&self, id: tsr_ast::NodeId) -> Result<tsr_ast::SourceFileRead<'_>, Error> {
        for result in self.results.iter().flatten() {
            if let Some(program) = &result.program {
                if let Ok(source) = program.program().program().diagnostic_source(id) {
                    return Ok(source);
                }
            }
        }
        for node in &self.orchestrator.tasks {
            if let Some(config) = &node.task.resolved {
                if let Ok(source) = config.diagnostic_source(id) {
                    return Ok(source);
                }
            }
        }
        Err(tsr_arena::Error::WrongOwner.into())
    }
}
