use super::{Error, Session, SessionEvent};
use crate::ata::{
    TypingsInfo, TypingsInstallRequest, TypingsInstallResult, TypingsInstaller,
    TypingsInstallerOptions,
};
use crate::project::{ProgramUpdateKind, ProjectData, ProjectKind};
use crate::{Project, Snapshot};
use std::collections::BTreeMap;
use std::sync::{atomic::Ordering, Arc, Condvar, Mutex, Weak};
use tsr_ipc::Context;
use tsr_jsstring::JsString;

#[derive(Clone)]
pub(super) struct AtaChange {
    pub info: Arc<TypingsInfo>,
    pub result: TypingsInstallResult,
}
struct Job {
    project: JsString,
    info: Arc<TypingsInfo>,
    context: Context,
}
#[derive(Default)]
struct Jobs {
    closed: bool,
    next: u64,
    running: BTreeMap<u64, Job>,
    panics: Vec<Box<dyn std::any::Any + Send>>,
}
#[derive(Default)]
struct Shared {
    jobs: Mutex<Jobs>,
    done: Condvar,
}
struct Progress {
    events: Option<std::sync::mpsc::Sender<SessionEvent>>,
    name: JsString,
}
impl Progress {
    fn new(events: Option<std::sync::mpsc::Sender<SessionEvent>>, name: JsString) -> Self {
        if let Some(events) = &events {
            let _ = events.send(SessionEvent::InstallingTypes {
                name: name.clone(),
                finished: false,
            });
        }
        Self { events, name }
    }
}
impl Drop for Progress {
    fn drop(&mut self) {
        if let Some(events) = &self.events {
            let _ = events.send(SessionEvent::InstallingTypes {
                name: self.name.clone(),
                finished: true,
            });
        }
    }
}
thread_local! { static CURRENT: std::cell::Cell<usize> = const { std::cell::Cell::new(0) }; }
pub(super) struct BackgroundAta {
    shared: Arc<Shared>,
    session: Weak<Session>,
    installer: Option<Arc<TypingsInstaller>>,
}
impl BackgroundAta {
    pub(super) fn new(
        session: Weak<Session>,
        options: &super::SessionOptions,
        fs: Arc<dyn tsr_vfs::FileSystem>,
    ) -> Self {
        Self {
            shared: Arc::default(),
            session,
            installer: options
                .npm_executor
                .as_ref()
                .filter(|_| !options.typings_location.is_empty())
                .map(|npm| {
                    Arc::new(TypingsInstaller::new(
                        TypingsInstallerOptions {
                            typings_location: options.typings_location.clone(),
                            throttle_limit: 5,
                        },
                        fs,
                        npm.clone(),
                    ))
                }),
        }
    }
    pub(super) fn wait(&self) {
        let own = CURRENT
            .with(|current| usize::from(current.get() == Arc::as_ptr(&self.shared) as usize));
        let mut jobs = self.shared.jobs.lock().expect("ATA jobs");
        while jobs.running.len() > own {
            jobs = self.shared.done.wait(jobs).expect("ATA jobs");
        }
        if let Some(panic) = jobs.panics.pop() {
            drop(jobs);
            std::panic::resume_unwind(panic);
        }
    }
    pub(super) fn cancel(&self, close: bool) {
        let contexts = {
            let mut jobs = self.shared.jobs.lock().expect("ATA jobs");
            jobs.closed |= close;
            jobs.running
                .values()
                .map(|job| job.context.clone())
                .collect::<Vec<_>>()
        };
        for context in contexts {
            context.cancel();
        }
    }
    fn trigger(&self, session: &Session, snapshot: &Snapshot) {
        let Some(installer) = &self.installer else {
            return;
        };
        if session.ata_disabled.load(Ordering::Acquire) {
            return;
        }
        for project in snapshot.projects() {
            let data = project.data().expect("session project");
            let info = Arc::new(data.compute_typings_info());
            if !info.type_acquisition.enable.is_true()
                || !(data
                    .installed_typings_info
                    .as_ref()
                    .is_none_or(|old| !old.equals(&info))
                    || data.last_update == snapshot.id().unwrap()
                        && data.update_kind == ProgramUpdateKind::NewFiles)
            {
                continue;
            }
            let context = session.context.with_cancel();
            let id = {
                let mut jobs = self.shared.jobs.lock().expect("ATA jobs");
                if jobs.closed {
                    return;
                }
                if jobs.running.values().any(|job| {
                    job.project == data.path
                        && job.info.equals(&info)
                        && job.context.err().is_none()
                }) {
                    continue;
                }
                let id = jobs.next;
                jobs.next = jobs.next.checked_add(1).expect("ATA job identity overflow");
                jobs.running.insert(
                    id,
                    Job {
                        project: data.path.clone(),
                        info: info.clone(),
                        context: context.clone(),
                    },
                );
                id
            };
            // npm mutates the shared cache. Let obsolete installs finish;
            // receive_typings rejects their results against current inputs.
            // Only session shutdown cancels a running process.
            let shared = self.shared.clone();
            let weak = self.session.clone();
            let installer = installer.clone();
            let fs = session.fs.clone();
            let logger = session.options.logger.clone();
            let progress_name = project
                .display_name(session.options.current_directory.as_bytes())
                .expect("session project name");
            let events = session.events.get().cloned();
            let project = data.path.clone();
            let root = data.current_directory.clone();
            let names: Vec<_> = data
                .program
                .files()
                .iter()
                .map(|file| {
                    JsString::from_bytes(
                        file.bound()
                            .view()
                            .source_file()
                            .expect("program source")
                            .file_name(),
                    )
                })
                .collect();
            let spawn = std::thread::Builder::new()
                .name("tsr-typings-installer".into())
                .spawn(move || {
                    CURRENT.with(|current| current.set(Arc::as_ptr(&shared) as usize));
                    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        let progress = Progress::new(events, progress_name);
                        let result = installer.install_typings(
                            &context,
                            &TypingsInstallRequest {
                                project_id: project.as_bytes(),
                                typings_info: &info,
                                file_names: &names,
                                project_root_path: root.as_bytes(),
                                fs: fs.as_ref(),
                                logger: &logger,
                            },
                        );
                        drop(progress);
                        if context.err().is_none() {
                            if let Some(session) = weak.upgrade() {
                                match result {
                                    Ok(result) => session.receive_typings(&project, info, result),
                                    Err(error) => logger.log(format_args!(
                                        "ATA installation failed for project {}: {error}",
                                        String::from_utf8_lossy(project.as_bytes())
                                    )),
                                }
                            }
                        }
                    }));
                    let mut jobs = shared.jobs.lock().expect("ATA jobs");
                    jobs.running.remove(&id);
                    if let Err(panic) = outcome {
                        jobs.panics.push(panic);
                        jobs.closed = true;
                    }
                    drop(jobs);
                    shared.done.notify_all();
                    CURRENT.with(|current| current.set(0));
                });
            if let Err(error) = spawn {
                self.shared
                    .jobs
                    .lock()
                    .expect("ATA jobs")
                    .running
                    .remove(&id);
                self.shared.done.notify_all();
                session
                    .options
                    .logger
                    .log(format_args!("ATA worker failed to start: {error}"));
            }
        }
    }
}

impl ProjectData {
    // port: tsc/internal/project/project.go:Project.GetTypeAcquisition
    pub fn type_acquisition(&self) -> tsr_tsoptions::TypeAcquisition {
        type_acquisition(self.kind, &self.command_line)
    }
    // port: tsc/internal/project/project.go:Project.ComputeTypingsInfo
    pub fn compute_typings_info(&self) -> TypingsInfo {
        TypingsInfo {
            type_acquisition: self.type_acquisition(),
            compiler_options: Arc::new(self.command_line.options.clone()),
            unresolved_imports: self.program.unresolved_imports().clone(),
        }
    }
}
pub(super) fn type_acquisition(
    kind: ProjectKind,
    command: &tsr_tsoptions::ParsedCommandLine,
) -> tsr_tsoptions::TypeAcquisition {
    if kind == ProjectKind::Inferred {
        tsr_tsoptions::TypeAcquisition {
            enable: tsr_core::Tristate::TRUE,
            disable_filename_based_type_acquisition: tsr_core::Tristate::FALSE,
            ..Default::default()
        }
    } else {
        command.type_acquisition.clone().unwrap_or_default()
    }
}
// port: tsc/internal/project/watch.go:getTypingsLocationsGlobs
pub(super) fn typings_watch_patterns(
    files: &[JsString],
    typings: &[u8],
    workspace: &[u8],
    cwd: &[u8],
    sensitive: bool,
) -> crate::watch::PatternsAndIgnored {
    let mut roots = BTreeMap::new();
    let mut external = BTreeMap::new();
    for file in files {
        let root = if tsr_tspath::contains_path(typings, file.as_bytes(), cwd, sensitive) {
            Some(typings)
        } else if tsr_tspath::contains_path(workspace, file.as_bytes(), cwd, sensitive) {
            Some(workspace)
        } else {
            None
        };
        if let Some(root) = root {
            roots.insert(
                tsr_tspath::to_path(root, cwd, sensitive),
                JsString::from_bytes(tsr_tspath::combine(root, &[b"**/*"])),
            );
        } else {
            let directory = tsr_tspath::directory(file.as_bytes());
            external.insert(tsr_tspath::to_path(&directory, cwd, sensitive), directory);
        }
    }
    let (mut directories, ignored) = tsr_tspath::common_parents(
        &external.values().map(Vec::as_slice).collect::<Vec<_>>(),
        2,
        crate::watch::components_for_watching,
        cwd,
        sensitive,
    );
    directories.sort();
    crate::watch::PatternsAndIgnored {
        directories_outside_workspace: directories.into_iter().map(JsString::from_bytes).collect(),
        patterns_inside_workspace: roots.into_values().collect(),
        ignored: ignored.into_iter().map(JsString::from_bytes).collect(),
    }
}
impl Session {
    // port: tsc/internal/project/session.go:Session.triggerATAForUpdatedProjects
    pub(super) fn trigger_ata(&self, snapshot: &Snapshot) {
        self.ata.trigger(self, snapshot);
    }
    fn receive_typings(
        &self,
        project: &JsString,
        info: Arc<TypingsInfo>,
        result: TypingsInstallResult,
    ) {
        let _update = self
            .update
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if self.context.err().is_some() || self.ata_disabled.load(Ordering::Acquire) {
            return;
        }
        let Ok(snapshot) = self.snapshot() else {
            return;
        };
        let Some(data) = snapshot
            .project_by_path(project.as_bytes())
            .and_then(Project::data)
        else {
            return;
        };
        if !data.compute_typings_info().equals(&info) || data.typings_files == result.typings_files
        {
            return;
        }
        self.pending
            .lock()
            .expect("session events")
            .ata_changes
            .insert(project.clone(), AtaChange { info, result });
        self.timers.schedule(
            super::timers::Kind::DiagnosticsRefresh,
            self.options.debounce_delay,
        );
    }
    pub fn set_disable_automatic_type_acquisition(&self, disabled: bool) -> Result<(), Error> {
        let current = self.snapshot.read().expect("session snapshot");
        current.as_ref().ok_or(Error::Closed)?;
        if self.ata_disabled.swap(disabled, Ordering::AcqRel) == disabled {
            return Ok(());
        }
        self.pending.lock().expect("session events").ata_changed = true;
        drop(current);
        self.flush(None)?;
        self.send_event(SessionEvent::DiagnosticsRefresh {
            cancellation: tsr_core::CancellationToken::new(),
        });
        Ok(())
    }
}

#[cfg(test)]
mod tests;
