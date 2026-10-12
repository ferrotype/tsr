//! Passive compiler ownership observations over real native watch events.
//! The public command returns its retained watcher through testing hooks; only
//! the scheduler is controlled here, never the filesystem events or compiler.
#[path = "../../../../crates/tsrust/src/allocation.rs"]
mod allocation;
#[allow(dead_code)]
#[path = "../../../../crates/tsrust/src/process.rs"]
mod process;
#[allow(dead_code)]
#[path = "../../../../crates/tsrust/src/signals.rs"]
mod signals;
#[path = "../../../../crates/tsrust/src/system.rs"]
mod system;

#[global_allocator]
static ALLOCATOR: allocation::CountingAllocator = allocation::CountingAllocator;

use sha2::{Digest, Sha256};
use std::fs::{File, OpenOptions};
use std::io::{self, BufRead, Write};
use std::os::unix::ffi::OsStrExt;
use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex, Weak};
use std::time::{Duration, Instant};
use tsr_arena::{Counters, Counts};
use tsr_compiler::{CheckedProgram, EmitResult, ProgramFile};
use tsr_core::collections::SyncMap;
use tsr_ipc::Context;
use tsr_jsstring::JsString;
use tsr_locale::Locale;
use tsr_tsc::watchmanager::{
    Closer, CommandLineTestingWithWatchBackend, FSWatchBackend, Ignore, WatchBackend,
    WatchDirectoryRequest,
};
use tsr_tsc::{CommandLineTesting, SharedWriter, Trace};
use tsr_vfs::iofs::Time;

#[derive(Default)]
struct Events {
    epoch: Mutex<u64>,
    error: Mutex<Option<String>>,
    ready: Condvar,
}
struct NativeBackend {
    native: FSWatchBackend,
    events: Arc<Events>,
}
impl WatchBackend for NativeBackend {
    fn watch_directory(
        &self,
        dir: &[u8],
        callback: tsr_tsc::fswatch::WatchCallback,
        recursive: bool,
        ignore: Option<Ignore>,
    ) -> Result<Arc<dyn Closer>, tsr_tsc::fswatch::Error> {
        self.watch_directories(vec![WatchDirectoryRequest {
            dir: dir.to_vec(),
            callback,
            recursive,
            ignore,
        }])
        .map(|mut watches| watches.remove(0))
    }
    fn watch_directories(
        &self,
        mut requests: Vec<WatchDirectoryRequest>,
    ) -> Result<Vec<Arc<dyn Closer>>, tsr_tsc::fswatch::Error> {
        for request in &mut requests {
            let callback = request.callback.clone();
            let events = self.events.clone();
            request.callback = Arc::new(move |batch, error| {
                if let Some(error) = error {
                    *events.error.lock().expect("native error lock") = Some(error.to_string());
                }
                // Publish to the real WatchManager before waking the probe.
                callback(batch, error);
                let mut epoch = events.epoch.lock().expect("native event lock");
                *epoch += 1;
                events.ready.notify_all();
            });
        }
        self.native.watch_directories(requests)
    }
}
struct Observed {
    counters: Counters,
    file: Weak<ProgramFile>,
    source: Vec<u8>,
    source_id: String,
    files: usize,
    compilations: u64,
}
struct Observer {
    edited: Vec<u8>,
    backend: Arc<NativeBackend>,
    current: Mutex<Option<Observed>>,
}
impl CommandLineTestingWithWatchBackend for Observer {
    fn watch_backend(&self) -> Arc<dyn WatchBackend> {
        self.backend.clone()
    }
}
impl CommandLineTesting for Observer {
    fn on_compiler_program(&self, checked: &CheckedProgram) {
        let files = checked.program().files();
        let file = files
            .iter()
            .find(|file| {
                file.bound()
                    .view()
                    .source_file()
                    .expect("source file")
                    .file_name()
                    == self.edited
            })
            .expect("edited source must participate in the actual compiler program");
        let mut current = self.current.lock().expect("observation lock");
        let compilations = current.as_ref().map_or(1, |old| old.compilations + 1);
        *current = Some(Observed {
            counters: file.bound().view().ast().counters().clone(),
            file: Arc::downgrade(file),
            source: file.bound().view().ast().source().as_bytes().to_vec(),
            source_id: format!("{:?}", file.source()),
            files: files.len(),
            compilations,
        });
    }
    fn on_program(&self, _: &tsr_incremental::Program) {}
    fn on_emitted_files(&self, _: Option<&EmitResult>, _: Option<&SyncMap<JsString, Time>>) {}
    fn on_list_files_start(&self, _: &SharedWriter) {}
    fn on_list_files_end(&self, _: &SharedWriter) {}
    fn on_statistics_start(&self, _: &SharedWriter) {}
    fn on_statistics_end(&self, _: &SharedWriter) {}
    fn on_build_status_report_start(&self, _: &SharedWriter) {}
    fn on_build_status_report_end(&self, _: &SharedWriter) {}
    fn on_watch_status_report_start(&self) {}
    fn on_watch_status_report_end(&self) {}
    fn get_trace(&self, writer: SharedWriter, locale: Locale) -> Trace {
        // Same trace formatting as the production no-testing path.
        Box::new(move |message, args| {
            tsr_tsc::write_all(writer.as_ref(), &message.localize(&locale, args));
            tsr_tsc::write_all(writer.as_ref(), b"\n");
        })
    }
    fn as_with_watch_backend(&self) -> Option<&dyn CommandLineTestingWithWatchBackend> {
        Some(self)
    }
}
fn report(output: &mut File, value: serde_json::Value) -> io::Result<()> {
    serde_json::to_writer(&mut *output, &value)?;
    output.write_all(b"\n")?;
    output.flush()
}
fn sample(observer: &Observer, phase: &str, released: Option<bool>) -> serde_json::Value {
    let current = observer.current.lock().expect("observation lock");
    let current = current.as_ref().expect("compiler observation");
    let counts = current.counters.snapshot();
    let memory = allocation::snapshot();
    serde_json::json!({
        "phase": phase,
        "owners": counts.owners,
        "storage_allocations": counts.allocations,
        "source_sha256": format!("{:x}", Sha256::digest(&current.source)),
        "source_id": current.source_id,
        "program_files": current.files,
        "compiler_callbacks": current.compilations,
        "native_callbacks": *observer.backend.events.epoch.lock().expect("native event lock"),
        "native_error": *observer.backend.events.error.lock().expect("native error lock"),
        "superseded_file_released": released,
        "allocator_live_requested_bytes": memory.live_bytes,
    })
}
fn command(input: &mut impl Iterator<Item = io::Result<String>>) -> io::Result<String> {
    input
        .next()
        .transpose()?
        .ok_or_else(|| io::Error::new(io::ErrorKind::UnexpectedEof, "probe controller closed"))
}
fn run() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let arguments: Vec<_> = std::env::args_os().skip(1).collect();
    if arguments.len() != 4 {
        return Err("expected project-config edited-file report-file cycle-timeout-seconds".into());
    }
    let config = PathBuf::from(&arguments[0]).canonicalize()?;
    let edited = PathBuf::from(&arguments[1]).canonicalize()?;
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&arguments[2])?;
    let timeout = Duration::from_secs(arguments[3].to_str().ok_or("non-UTF8 timeout")?.parse()?);
    let native = tsr_tsc::fswatch::default_watcher();
    let name = native.name();
    if !native.available() {
        return Err("default native watch backend unavailable".into());
    }
    let observer = Arc::new(Observer {
        edited: edited.as_os_str().as_bytes().to_vec(),
        backend: Arc::new(NativeBackend {
            native: FSWatchBackend { inner: native },
            events: Arc::new(Events::default()),
        }),
        current: Mutex::new(None),
    });
    report(
        &mut output,
        serde_json::json!({"phase": "baseline", "pid": std::process::id(), "backend": name}),
    )?;
    let stdin = io::stdin();
    let mut input = stdin.lock().lines();
    if command(&mut input)? != "start" {
        return Err("expected start".into());
    }
    let args = [
        "--watch",
        "--project",
        config.to_str().ok_or("non-UTF8 config")?,
        "--pretty",
        "false",
        "--noEmit",
        "--incremental",
        "false",
        "--composite",
        "false",
        "--runExternalCode",
        "false",
    ]
    .map(|argument| JsString::from_bytes(argument.as_bytes()));
    let result = tsr_execute::command_line(
        &Context::background(),
        Arc::new(system::OsSystem::new()?),
        &args,
        Some(observer.clone()),
    )?;
    if result.status != tsr_tsc::ExitStatus::Success {
        return Err(format!("watch command status {}", result.status.0).into());
    }
    let watcher = result.watcher.ok_or("command returned no watcher")?;
    let mut epoch = *observer
        .backend
        .events
        .epoch
        .lock()
        .expect("native event lock");
    report(&mut output, sample(&observer, "ready", None))?;
    loop {
        match command(&mut input)?.as_str() {
            "cycle" => {
                let expected = tsr_vfs::iovfs::decode_bytes(&std::fs::read(&edited)?);
                let (old, before) = {
                    let current = observer.current.lock().expect("observation lock");
                    let current = current.as_ref().expect("compiler observation");
                    (current.file.clone(), current.compilations)
                };
                let deadline = Instant::now() + timeout;
                loop {
                    let events = &observer.backend.events;
                    let mut event = events.epoch.lock().expect("native event lock");
                    while *event <= epoch {
                        let remaining = deadline
                            .checked_duration_since(Instant::now())
                            .ok_or("native watch cycle timed out")?;
                        let waited = events
                            .ready
                            .wait_timeout(event, remaining)
                            .expect("native event wait");
                        event = waited.0;
                    }
                    epoch = *event;
                    drop(event);
                    watcher.do_cycle()?;
                    let current = observer.current.lock().expect("observation lock");
                    if current.as_ref().is_some_and(|current| {
                        current.compilations > before && current.source == expected
                    }) {
                        break;
                    }
                    if Instant::now() >= deadline {
                        return Err("native callback did not compile the edited source".into());
                    }
                }
                let released = old.upgrade().is_none();
                report(&mut output, sample(&observer, "cycle", Some(released)))?;
                if !released {
                    return Err("superseded edited ProgramFile remains retained".into());
                }
            }
            "close" => break,
            other => return Err(format!("unexpected controller command {other:?}").into()),
        }
    }
    let (counters, last) = {
        let current = observer.current.lock().expect("observation lock");
        let current = current.as_ref().expect("compiler observation");
        (current.counters.clone(), current.file.clone())
    };
    drop(watcher);
    observer.backend.native.inner.close();
    drop(observer);
    let counts = counters.snapshot();
    let released = last.upgrade().is_none();
    report(
        &mut output,
        serde_json::json!({"phase": "closed", "owners": counts.owners, "storage_allocations": counts.allocations, "last_file_released": released}),
    )?;
    if counts != Counts::default() || !released {
        return Err("watch teardown retained tracked owners/storage".into());
    }
    if command(&mut input)? != "finish" {
        return Err("expected finish after OS teardown sample".into());
    }
    Ok(())
}
fn main() -> std::process::ExitCode {
    // Match the production CLI's reserved compiler thread stack.
    match std::thread::Builder::new()
        .name("phase7-live-watch".into())
        .stack_size(tsr_core::workgroup::RESERVED_STACK)
        .spawn(run)
        .expect("start watch probe")
        .join()
    {
        Ok(Ok(())) => std::process::ExitCode::SUCCESS,
        Ok(Err(error)) => {
            eprintln!("watch probe failed: {error}");
            std::process::ExitCode::FAILURE
        }
        Err(payload) => std::panic::resume_unwind(payload),
    }
}
