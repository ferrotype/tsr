//! Parsing ahead of the loader.
//!
//! The pin's `filesParser` parses every queued file on its own goroutine and
//! collects the results afterwards. The loader here walks its task list on one
//! thread, because resolution mutates its state as it goes, so the files it
//! has queued but not reached yet are parsed and bound by reserved-stack
//! workers in the meantime. A worker produces exactly what the loader's own
//! `FileCache::load` would: the same read, the same parse options and, with a
//! project cache, the same cache lease. The loader takes a finished file when
//! it reaches the task, waits for one in flight, and parses a file itself when
//! no worker has started it.
//!
//! Nothing observable depends on which thread parsed a file: the loader keeps
//! its order of tasks, resolutions and diagnostics. Tracing runs, resolution
//! traces and single-threaded programs do not use the workers.
use crate::{cache::ProgramFile, Error, SourceFileCache};
use std::any::Any;
use std::collections::{HashMap, VecDeque};
use std::panic::{catch_unwind, resume_unwind, AssertUnwindSafe};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::thread::JoinHandle;
use tsr_arena::Counters;
use tsr_ast::SourceFileParseOptions;
use tsr_core::ScriptKind;
use tsr_jsstring::JsString;
use tsr_vfs::FileSystem;

/// A file the loader will load: everything `FileCache::load` needs.
pub(crate) struct Job {
    pub path: JsString,
    pub kind: ScriptKind,
    pub options: SourceFileParseOptions,
}

/// A file a worker finished, with the lease its project cache handed out.
pub(crate) struct Preloaded {
    pub kind: ScriptKind,
    pub options: SourceFileParseOptions,
    pub file: Arc<ProgramFile>,
    pub retention: Option<Box<dyn Send + Sync>>,
}

enum Slot {
    Queued(Job),
    Running,
    /// `None`: the read found no file or failed; the loader repeats it and
    /// reports the outcome itself.
    Done(Option<Preloaded>),
    Panicked(Box<dyn Any + Send>),
}

struct State {
    queue: VecDeque<JsString>,
    slots: HashMap<JsString, Slot>,
    idle: usize,
    closed: bool,
}

struct Shared {
    state: Mutex<State>,
    work: Condvar,
    done: Condvar,
}

/// What a worker parses with: the program's host and, when the program has
/// one, the project cache its loader would have acquired from.
#[derive(Clone)]
pub(crate) struct Producer {
    pub host: Arc<dyn FileSystem>,
    pub project: Option<Arc<dyn SourceFileCache>>,
    pub counters: Counters,
}

impl Producer {
    fn load(&self, job: Job) -> Result<Option<Preloaded>, Error> {
        let Some(content) = self.host.read_file(job.options.file_name.as_bytes())? else {
            return Ok(None);
        };
        let (file, retention) = match &self.project {
            Some(cache) => {
                let acquired = cache.acquire(
                    content.text,
                    job.kind,
                    job.options.clone(),
                    &self.counters,
                    None,
                )?;
                (acquired.file, Some(acquired.retention))
            }
            None => (
                ProgramFile::parse_and_bind(
                    content.text,
                    job.kind,
                    job.options.clone(),
                    &self.counters,
                    None,
                    None,
                )?,
                None,
            ),
        };
        Ok(Some(Preloaded {
            kind: job.kind,
            options: job.options,
            file,
            retention,
        }))
    }
}

pub(crate) struct Preloader {
    shared: Arc<Shared>,
    producer: Producer,
    workers: Vec<JoinHandle<()>>,
    bound: usize,
}

impl Preloader {
    /// A pool that starts a worker per submitted job while every worker is
    /// busy, up to the work-group bound.
    pub fn new(producer: Producer) -> Self {
        Self {
            shared: Arc::new(Shared {
                state: Mutex::new(State {
                    queue: VecDeque::new(),
                    slots: HashMap::new(),
                    idle: 0,
                    closed: false,
                }),
                work: Condvar::new(),
                done: Condvar::new(),
            }),
            producer,
            workers: Vec::new(),
            bound: tsr_core::workgroup::worker_bound(),
        }
    }

    /// Queue a file. A path queued before is left as it is.
    pub fn submit(&mut self, job: Job) {
        let spawn = {
            let mut state = self
                .shared
                .state
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            if state.slots.contains_key(&job.path) {
                return;
            }
            state.queue.push_back(job.path.clone());
            state.slots.insert(job.path.clone(), Slot::Queued(job));
            state.idle == 0 && self.workers.len() < self.bound
        };
        if spawn {
            self.spawn_worker();
        }
        self.shared.work.notify_one();
    }

    fn spawn_worker(&mut self) {
        let shared = self.shared.clone();
        let producer = self.producer.clone();
        #[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
        let worker = tsr_parser::spawn_parser_thread(move || work(&shared, &producer));
        #[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
        let worker: std::io::Result<JoinHandle<()>> = {
            drop((shared, producer));
            Err(std::io::Error::other("no threads"))
        };
        if let Ok(worker) = worker {
            self.workers.push(worker);
        }
    }

    /// The loader reached `path`: its finished file, after waiting for a
    /// worker that is on it. `None` when no worker has started it (the queued
    /// job is withdrawn) or when the worker's read produced nothing.
    pub fn take(&self, path: &JsString) -> Option<Preloaded> {
        let mut state = self
            .shared
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        loop {
            match state.slots.get(path) {
                None | Some(Slot::Queued(_)) => {
                    // The queue entry, if any, is skipped by the worker that
                    // pops it, since its slot is gone.
                    state.slots.remove(path);
                    return None;
                }
                Some(Slot::Running) => {
                    state = self
                        .shared
                        .done
                        .wait(state)
                        .unwrap_or_else(PoisonError::into_inner);
                }
                Some(Slot::Done(_) | Slot::Panicked(_)) => break,
            }
        }
        match state.slots.remove(path) {
            Some(Slot::Done(preloaded)) => preloaded,
            Some(Slot::Panicked(panic)) => {
                drop(state);
                resume_unwind(panic)
            }
            _ => unreachable!("the slot was finished above"),
        }
    }
}

impl Drop for Preloader {
    fn drop(&mut self) {
        {
            let mut state = self
                .shared
                .state
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            state.closed = true;
            state.queue.clear();
        }
        self.shared.work.notify_all();
        // Join rather than detach: a finished worker's reserved stack is
        // unmapped at once, which glibc's pthread_detach (BZ #19951) may read.
        for worker in self.workers.drain(..) {
            let _ = worker.join();
        }
    }
}

fn work(shared: &Shared, producer: &Producer) {
    loop {
        let job = {
            let mut state = shared.state.lock().unwrap_or_else(PoisonError::into_inner);
            let job = loop {
                if state.closed {
                    return;
                }
                let Some(path) = state.queue.pop_front() else {
                    state.idle += 1;
                    state = shared
                        .work
                        .wait(state)
                        .unwrap_or_else(PoisonError::into_inner);
                    state.idle -= 1;
                    continue;
                };
                // A withdrawn job has no slot any more.
                if let Some(slot @ Slot::Queued(_)) = state.slots.get_mut(&path) {
                    let Slot::Queued(job) = std::mem::replace(slot, Slot::Running) else {
                        unreachable!()
                    };
                    break job;
                }
            };
            job
        };
        let path = job.path.clone();
        let outcome = catch_unwind(AssertUnwindSafe(|| producer.load(job)));
        let slot = match outcome {
            Ok(Ok(preloaded)) => Slot::Done(preloaded),
            Ok(Err(_)) => Slot::Done(None),
            Err(panic) => Slot::Panicked(panic),
        };
        let mut state = shared.state.lock().unwrap_or_else(PoisonError::into_inner);
        // The loader may have withdrawn the job while it ran; it parses the
        // file itself then and this result is dropped.
        if matches!(state.slots.get(&path), Some(Slot::Running)) {
            state.slots.insert(path, slot);
        }
        drop(state);
        shared.done.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use tsr_vfs::{iovfs, vfstest};

    fn host(files: &[(&str, &str)]) -> Arc<dyn FileSystem> {
        let files: BTreeMap<Vec<u8>, vfstest::InputFile> = files
            .iter()
            .map(|(name, text)| {
                (
                    name.as_bytes().to_vec(),
                    vfstest::InputFile::Text(text.as_bytes().to_vec()),
                )
            })
            .collect();
        Arc::new(iovfs::from(Arc::new(vfstest::from_map(&files, true)), true))
    }

    fn job(name: &str) -> Job {
        Job {
            path: JsString::from_bytes(name.as_bytes()),
            kind: ScriptKind::TS,
            options: SourceFileParseOptions {
                file_name: JsString::from_bytes(name.as_bytes()),
                path: JsString::from_bytes(name.as_bytes()),
                ..Default::default()
            },
        }
    }

    /// Wait for the workers to finish `path`; the loader would take it over
    /// instead if no worker had started it yet.
    fn wait_finished(preloader: &Preloader, path: &str) {
        let path = JsString::from_bytes(path.as_bytes());
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        loop {
            {
                let state = preloader.shared.state.lock().unwrap();
                if matches!(
                    state.slots.get(&path),
                    Some(Slot::Done(_) | Slot::Panicked(_))
                ) {
                    return;
                }
            }
            assert!(
                std::time::Instant::now() < deadline,
                "worker did not finish {path:?}"
            );
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    }

    #[test]
    fn finished_files_are_taken_and_missing_ones_report_nothing() {
        let mut preloader = Preloader::new(Producer {
            host: host(&[("/a.ts", "export const a = 1;")]),
            project: None,
            counters: Counters::default(),
        });
        preloader.submit(job("/a.ts"));
        preloader.submit(job("/missing.ts"));
        wait_finished(&preloader, "/a.ts");
        wait_finished(&preloader, "/missing.ts");
        let a = preloader
            .take(&JsString::from_bytes(b"/a.ts".as_slice()))
            .expect("the worker parsed the file");
        assert_eq!(a.kind, ScriptKind::TS);
        assert_eq!(
            a.file
                .bound()
                .view()
                .source_file()
                .unwrap()
                .text()
                .as_bytes(),
            b"export const a = 1;"
        );
        assert!(preloader
            .take(&JsString::from_bytes(b"/missing.ts".as_slice()))
            .is_none());
        assert!(preloader
            .take(&JsString::from_bytes(b"/never-submitted.ts".as_slice()))
            .is_none());
    }

    #[test]
    fn a_job_no_worker_started_is_withdrawn() {
        let mut preloader = Preloader::new(Producer {
            host: host(&[("/a.ts", "let a;")]),
            project: None,
            counters: Counters::default(),
        });
        preloader.bound = 0;
        preloader.submit(job("/a.ts"));
        assert!(preloader
            .take(&JsString::from_bytes(b"/a.ts".as_slice()))
            .is_none());
        let state = preloader.shared.state.lock().unwrap();
        assert!(state.slots.is_empty());
    }
}
