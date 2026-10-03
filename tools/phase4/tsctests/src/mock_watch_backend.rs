//! `mock_watch_backend.go`: the watch backend of the fake system. It records
//! every watch the command line registers, delivers events only through
//! watches whose paths match, and renders the registrations for the baseline.
use crate::execute::fswatch::{self, Event, EventKind, WatchCallback};
use crate::execute::watchmanager::{Closer, Ignore, WatchBackend, WatchDirectoryRequest};
use crate::fsbaselineutil::FileChange;
use crate::goutil::path_dir;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

/// The `DirectoryExists` the backend consults; `None` accepts every
/// directory.
pub type DirectoryExists = Arc<dyn Fn(&[u8]) -> bool + Send + Sync>;

/// MockWatchBackend implements `watchmanager.WatchBackend` for testing. It
/// records all `WatchDirectory` calls so tests can verify that the correct
/// watches are registered. Events can be delivered through `send_events`,
/// which routes them only through watches whose paths match, enforcing that
/// tests fail if the wrong watches are set up.
///
/// `dirs` is ordered by path where the pin's map is not; the pin invokes the
/// matched callbacks in map order, which nothing may depend on.
pub struct MockWatchBackend {
    dirs: Mutex<BTreeMap<Vec<u8>, Arc<MockWatch>>>,
    /// if set, `watch_directory` fails for non-existent dirs
    pub directory_exists: Option<DirectoryExists>,
    pub use_case_sensitive_file_names: bool,
}

/// MockWatch records a single registered watch.
pub struct MockWatch {
    pub path: Vec<u8>,
    pub callback: WatchCallback,
    pub recursive: bool,
    pub ignore: Option<Ignore>,
    closed: AtomicBool,
}

impl MockWatch {
    pub fn closed(&self) -> bool {
        self.closed.load(Ordering::Acquire)
    }
}

impl Closer for MockWatch {
    // port: tsc/internal/execute/tsctests/mock_watch_backend.go:MockWatch.Close
    fn close(&self) -> Result<(), fswatch::Error> {
        self.closed.store(true, Ordering::Release);
        Ok(())
    }
}

/// NewMockWatchBackend creates a ready-to-use mock backend.
// port: tsc/internal/execute/tsctests/mock_watch_backend.go:NewMockWatchBackend
pub fn new_mock_watch_backend(
    directory_exists: Option<DirectoryExists>,
    use_case_sensitive_file_names: bool,
) -> MockWatchBackend {
    MockWatchBackend {
        dirs: Mutex::new(BTreeMap::new()),
        directory_exists,
        use_case_sensitive_file_names,
    }
}

impl MockWatchBackend {
    fn lock(&self) -> MutexGuard<'_, BTreeMap<Vec<u8>, Arc<MockWatch>>> {
        self.dirs.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// HasWatches reports whether any watches have been registered.
    // port: tsc/internal/execute/tsctests/mock_watch_backend.go:MockWatchBackend.HasWatches
    pub fn has_watches(&self) -> bool {
        !self.lock().is_empty()
    }

    /// The registered watch of `dir`, closed or not.
    pub fn watch(&self, dir: &[u8]) -> Option<Arc<MockWatch>> {
        self.lock().get(dir).cloned()
    }

    /// SendEvents routes events through the registered watch callbacks that
    /// match each event's path. Directory watches match if the event path is a
    /// child (or recursive descendant) of the watched directory. Events that
    /// match no watch are silently dropped — this is by design so that tests
    /// fail when the production code doesn't register the needed watches.
    // port: tsc/internal/execute/tsctests/mock_watch_backend.go:MockWatchBackend.SendEvents
    pub fn send_events(&self, events: &[Event]) {
        // Snapshot callbacks under the lock, then invoke outside the lock to
        // avoid deadlock if the callback re-enters the mock.
        let mut targets: BTreeMap<Vec<u8>, (WatchCallback, Vec<Event>)> = BTreeMap::new();
        {
            let dirs = self.lock();
            for e in events {
                // Check directory watches.
                for w in dirs.values() {
                    if w.closed() {
                        continue;
                    }
                    if w.ignore.as_ref().is_some_and(|ignore| ignore(&e.path)) {
                        continue;
                    }
                    if !path_is_under(
                        &e.path,
                        &w.path,
                        w.recursive,
                        self.use_case_sensitive_file_names,
                    ) {
                        continue;
                    }
                    targets
                        .entry(w.path.clone())
                        .or_insert_with(|| (w.callback.clone(), Vec::new()))
                        .1
                        .push(e.clone());
                }
            }
        }

        for (callback, events) in targets.into_values() {
            callback(&events, None);
        }
    }

    /// SendOverflow simulates a kernel event-queue overflow by invoking every
    /// active watch callback with `fswatch.ErrOverflow`. The watch manager
    /// treats this as a signal that events were dropped and a full rebuild is
    /// required.
    // port: tsc/internal/execute/tsctests/mock_watch_backend.go:MockWatchBackend.SendOverflow
    pub fn send_overflow(&self) {
        let callbacks: Vec<WatchCallback> = self
            .lock()
            .values()
            .filter(|w| !w.closed())
            .map(|w| w.callback.clone())
            .collect();
        for callback in callbacks {
            callback(&[], Some(&fswatch::Error::Overflow));
        }
    }

    /// SendChangedPaths converts a list of file changes into fswatch events
    /// with appropriate event kinds and routes them through registered
    /// watches via `send_events`. For new/modified files, it also emits update
    /// events for their parent directories, simulating how real filesystem
    /// watchers report directory events.
    // port: tsc/internal/execute/tsctests/mock_watch_backend.go:MockWatchBackend.SendChangedPaths
    pub fn send_changed_paths(&self, changes: &[FileChange]) {
        self.send_events(&changed_path_events(changes));
    }

    /// WatchState returns a deterministic, human-readable summary of all
    /// active watches. This is intended to be included in test baselines so
    /// that watch registration correctness is verified via snapshot diffs.
    // port: tsc/internal/execute/tsctests/mock_watch_backend.go:MockWatchBackend.WatchState
    pub fn watch_state(&self) -> Vec<u8> {
        let dirs = self.lock();

        let mut b = Vec::new();
        b.extend_from_slice(b"Watch Registrations::\n");

        // Directory watches, sorted by path.
        let active: BTreeSet<&Vec<u8>> = dirs
            .iter()
            .filter(|(_, w)| !w.closed())
            .map(|(dir, _)| dir)
            .collect();

        b.extend_from_slice(b"Directory watches::\n");
        if active.is_empty() {
            b.extend_from_slice(b"  (none)\n");
        }
        for d in active {
            let w = &dirs[d];
            b.extend_from_slice(b"  ");
            b.extend_from_slice(d);
            if w.recursive {
                b.extend_from_slice(b" (recursive)");
            }
            b.push(b'\n');
        }

        b
    }
}

/// The events `SendChangedPaths` sends for `changes`, in its order.
pub fn changed_path_events(changes: &[FileChange]) -> Vec<Event> {
    let mut events = Vec::with_capacity(changes.len() * 2);
    let mut seen_dirs: BTreeSet<Vec<u8>> = BTreeSet::new();
    for c in changes {
        let kind = if c.deleted {
            EventKind::EventDelete
        } else {
            EventKind::EventUpdate
        };
        events.push(Event {
            kind,
            path: c.path.clone(),
        });
        // Emit update events for parent directories of changed files.
        // Real filesystem watchers deliver events to non-recursive watches
        // when a child directory is created, which the mock must replicate.
        let mut dir = path_dir(&c.path);
        while !dir.is_empty() && dir != b"/" && dir != b"." {
            if seen_dirs.contains(&dir) {
                break;
            }
            seen_dirs.insert(dir.clone());
            events.push(Event {
                kind: EventKind::EventUpdate,
                path: dir.clone(),
            });
            let parent = path_dir(&dir);
            if parent == dir {
                break;
            }
            dir = parent;
        }
    }
    events
}

impl WatchBackend for MockWatchBackend {
    // port: tsc/internal/execute/tsctests/mock_watch_backend.go:MockWatchBackend.WatchDirectory
    fn watch_directory(
        &self,
        dir: &[u8],
        callback: WatchCallback,
        recursive: bool,
        ignore: Option<Ignore>,
    ) -> Result<Arc<dyn Closer>, fswatch::Error> {
        let mut closers = self.watch_directories(vec![WatchDirectoryRequest {
            dir: dir.to_vec(),
            callback,
            recursive,
            ignore,
        }])?;
        Ok(closers.swap_remove(0))
    }

    // port: tsc/internal/execute/tsctests/mock_watch_backend.go:MockWatchBackend.WatchDirectories
    fn watch_directories(
        &self,
        requests: Vec<WatchDirectoryRequest>,
    ) -> Result<Vec<Arc<dyn Closer>>, fswatch::Error> {
        let mut dirs = self.lock();
        for request in &requests {
            if let Some(directory_exists) = &self.directory_exists {
                if !directory_exists(&request.dir) {
                    return Err(fswatch::Error::Message(format!(
                        "directory does not exist: {}",
                        String::from_utf8_lossy(&request.dir)
                    )));
                }
            }
        }
        let mut closers: Vec<Arc<dyn Closer>> = Vec::with_capacity(requests.len());
        for request in requests {
            let w = Arc::new(MockWatch {
                path: request.dir.clone(),
                callback: request.callback,
                recursive: request.recursive,
                ignore: request.ignore,
                closed: AtomicBool::new(false),
            });
            dirs.insert(request.dir, w.clone());
            closers.push(w);
        }
        Ok(closers)
    }
}

/// pathIsUnder reports whether `event_path` is inside `dir`. If `recursive`
/// is false, only direct children match.
// port: tsc/internal/execute/tsctests/mock_watch_backend.go:pathIsUnder
pub fn path_is_under(
    event_path: &[u8],
    dir: &[u8],
    recursive: bool,
    use_case_sensitive_file_names: bool,
) -> bool {
    let (event_path, dir) = if use_case_sensitive_file_names {
        (event_path.to_vec(), dir.to_vec())
    } else {
        (
            tsr_tspath::canonical(event_path, false).into_owned(),
            tsr_tspath::canonical(dir, false).into_owned(),
        )
    };
    let Some(rest) = event_path.strip_prefix(dir.as_slice()) else {
        return false;
    };
    if rest.is_empty() {
        return false; // exact match = the dir itself, not a child
    }
    if rest[0] != b'/' {
        return false; // e.g. dir="/foo", path="/foobar"
    }
    if !recursive {
        // Direct child only: no further '/' after the separator.
        return !rest[1..].contains(&b'/');
    }
    true
}
