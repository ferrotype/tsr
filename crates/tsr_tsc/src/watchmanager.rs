//! Directory subscriptions, event accumulation and compiler-cycle signalling.
use crate::{fswatch, write_all, SharedWriter};
use std::collections::HashMap;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use tsr_jsstring::JsString;

pub trait Closer: Send + Sync {
    fn close(&self) -> Result<(), fswatch::Error>;
}
impl Closer for fswatch::Watch {
    fn close(&self) -> Result<(), fswatch::Error> {
        self.close()
    }
}
pub type Ignore = Arc<dyn Fn(&[u8]) -> bool + Send + Sync>;
pub type DirectoryExists = Arc<dyn Fn(&[u8]) -> bool + Send + Sync>;
#[derive(Clone)]
pub struct WatchDirectoryRequest {
    pub dir: Vec<u8>,
    pub callback: fswatch::WatchCallback,
    pub recursive: bool,
    pub ignore: Option<Ignore>,
}
pub trait WatchBackend: Send + Sync {
    fn watch_directory(
        &self,
        dir: &[u8],
        callback: fswatch::WatchCallback,
        recursive: bool,
        ignore: Option<Ignore>,
    ) -> Result<Arc<dyn Closer>, fswatch::Error>;
    fn watch_directories(
        &self,
        requests: Vec<WatchDirectoryRequest>,
    ) -> Result<Vec<Arc<dyn Closer>>, fswatch::Error>;
}
pub trait CommandLineTestingWithWatchBackend: Send + Sync {
    fn watch_backend(&self) -> Arc<dyn WatchBackend>;
}
pub struct FSWatchBackend {
    pub inner: fswatch::Watcher,
}
impl WatchBackend for FSWatchBackend {
    /// port: tsc/internal/execute/watchmanager/watchbackend.go:FSWatchBackend.WatchDirectory
    fn watch_directory(
        &self,
        dir: &[u8],
        callback: fswatch::WatchCallback,
        recursive: bool,
        ignore: Option<Ignore>,
    ) -> Result<Arc<dyn Closer>, fswatch::Error> {
        self.watch_directories(vec![WatchDirectoryRequest {
            dir: dir.to_vec(),
            callback,
            recursive,
            ignore,
        }])
        .map(|mut watches| watches.remove(0))
    }
    /// port: tsc/internal/execute/watchmanager/watchbackend.go:FSWatchBackend.WatchDirectories
    fn watch_directories(
        &self,
        requests: Vec<WatchDirectoryRequest>,
    ) -> Result<Vec<Arc<dyn Closer>>, fswatch::Error> {
        let requests: Vec<_> = requests
            .into_iter()
            .map(|r| fswatch::WatchDirectoryRequest {
                dir: r.dir,
                callback: r.callback,
                options: fswatch::WatchOptions {
                    recursive: r.recursive,
                    ignore: r.ignore,
                },
            })
            .collect();
        self.inner.watch_directories(&requests).map(|watches| {
            watches
                .into_iter()
                .map(|w| Arc::new(w) as Arc<dyn Closer>)
                .collect()
        })
    }
}
/// port: tsc/internal/execute/watchmanager/watchbackend.go:ShouldIgnoreWatchPath
pub fn should_ignore_watch_path(path: &[u8]) -> bool {
    let path = tsr_tspath::normalize_slashes(path);
    path.ends_with(b"/.git")
        || [b"/.git/".as_slice(), b"/node_modules/.", b"/.#"]
            .iter()
            .any(|pattern| path.windows(pattern.len()).any(|part| part == *pattern))
}
/// port: tsc/internal/execute/watchmanager/watchbackend.go:CanWatchDirectory
pub fn can_watch_directory(dir: &[u8]) -> bool {
    let components = tsr_tspath::path_components(dir, b"");
    components.len() > 2
        && components.len() > perceived_os_root_length_for_watching(&components) + 1
}
/// port: tsc/internal/execute/watchmanager/watchbackend.go:PerceivedOsRootLengthForWatching
pub fn perceived_os_root_length_for_watching(components: &[Vec<u8>]) -> usize {
    if components.len() <= 1 {
        return 1;
    }
    let root = &components[0];
    let mut index = 1;
    let mut dos = root.len() >= 2 && tsr_tspath::is_volume_character(root[0]) && root[1] == b':';
    if root != b"/"
        && !dos
        && components[1].len() >= 2
        && tsr_tspath::is_volume_character(components[1][0])
        && components[1].ends_with(b"$")
    {
        if components.len() == 2 {
            return 2;
        }
        index = 2;
        dos = true;
    }
    if dos && (index >= components.len() || !tsr_tspath::equal_fold(&components[index], b"users")) {
        return index;
    }
    if index < components.len() && tsr_tspath::equal_fold(&components[index], b"workspaces") {
        return index + 1;
    }
    index + 2
}
#[derive(Clone)]
pub struct ComparePathsOptions {
    pub current_directory: JsString,
    pub use_case_sensitive_file_names: bool,
}
pub struct DirWatchSet {
    options: ComparePathsOptions,
    dirs: HashMap<JsString, bool>,
}
impl DirWatchSet {
    /// port: tsc/internal/execute/watchmanager/watchmanager.go:NewDirWatchSet
    pub fn new(options: ComparePathsOptions) -> Self {
        Self {
            options,
            dirs: HashMap::new(),
        }
    }
    /// port: tsc/internal/execute/watchmanager/watchmanager.go:DirWatchSet.Set
    pub fn set(&mut self, dir: &[u8], recursive: bool) {
        let dir = JsString::from_bytes(
            tsr_tspath::canonical(dir, self.options.use_case_sensitive_file_names).into_owned(),
        );
        *self.dirs.entry(dir).or_default() |= recursive;
    }
    /// port: tsc/internal/execute/watchmanager/watchmanager.go:DirWatchSet.Covered
    pub fn covered(&self, dir: &[u8]) -> bool {
        let mut dir =
            tsr_tspath::canonical(dir, self.options.use_case_sensitive_file_names).into_owned();
        if self.dirs.contains_key(dir.as_slice()) {
            return true;
        }
        let root = tsr_tspath::root_length(&dir);
        while dir.len() > root {
            dir = tsr_tspath::directory(&dir);
            if self.dirs.get(dir.as_slice()).copied().unwrap_or(false) {
                return true;
            }
        }
        false
    }
    /// port: tsc/internal/execute/watchmanager/watchmanager.go:DirWatchSet.Dirs
    pub fn dirs(&self) -> &HashMap<JsString, bool> {
        &self.dirs
    }
}
struct WatchedDir {
    recursive: bool,
    closer: Mutex<Option<Arc<dyn Closer>>>,
}
#[derive(Default)]
struct State {
    backend: Option<Arc<dyn WatchBackend>>,
    watched: HashMap<JsString, Arc<WatchedDir>>,
}
#[derive(Default)]
struct Changes {
    paths: HashMap<JsString, fswatch::EventKind>,
    overflow: bool,
    signalled: bool,
}
struct Inner {
    state: Mutex<State>,
    cycle: Mutex<()>,
    changes: Mutex<Changes>,
    ready: Condvar,
    warn_writer: SharedWriter,
    dir_exists: DirectoryExists,
    debug: Mutex<Option<SharedWriter>>,
}
/// Hold [`Self::lock`] for a complete compile cycle. Native callbacks only
/// acquire the state/event locks, and no native close runs while either is held.
#[derive(Clone)]
pub struct WatchManager(Arc<Inner>);
fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}
impl WatchManager {
    /// port: tsc/internal/execute/watchmanager/watchmanager.go:NewWatchManager
    pub fn new(warn_writer: SharedWriter, dir_exists: DirectoryExists) -> Self {
        Self(Arc::new(Inner {
            state: Mutex::new(State::default()),
            cycle: Mutex::new(()),
            changes: Mutex::new(Changes::default()),
            ready: Condvar::new(),
            warn_writer,
            dir_exists,
            debug: Mutex::new(None),
        }))
    }
    /// port: tsc/internal/execute/watchmanager/watchmanager.go:WatchManager.Lock
    pub fn lock(&self) -> MutexGuard<'_, ()> {
        lock(&self.0.cycle)
    }
    /// port: tsc/internal/execute/watchmanager/watchmanager.go:WatchManager.SetBackend
    pub fn set_backend(&self, backend: Arc<dyn WatchBackend>) {
        lock(&self.0.state).backend = Some(backend);
    }
    /// port: tsc/internal/execute/watchmanager/watchmanager.go:WatchManager.Backend
    pub fn backend(&self) -> Option<Arc<dyn WatchBackend>> {
        lock(&self.0.state).backend.clone()
    }
    pub fn set_debug_log(&self, writer: Option<SharedWriter>) {
        *lock(&self.0.debug) = writer;
    }
    pub fn debug_log(&self, message: &str) {
        if let Some(writer) = lock(&self.0.debug).clone() {
            write_all(writer.as_ref(), message.as_bytes());
        }
    }
    fn debug_path(&self, prefix: &[u8], path: &[u8], suffix: &[u8]) {
        if let Some(writer) = lock(&self.0.debug).clone() {
            let mut text = Vec::with_capacity(prefix.len() + path.len() + suffix.len());
            text.extend_from_slice(prefix);
            text.extend_from_slice(path);
            text.extend_from_slice(suffix);
            write_all(writer.as_ref(), &text);
        }
    }
    /// port: tsc/internal/execute/watchmanager/watchmanager.go:WatchManager.EnsureDefaultBackend
    pub fn ensure_default_backend(&self) {
        let mut state = lock(&self.0.state);
        if state.backend.is_none() {
            let inner = fswatch::default_watcher();
            self.debug_log(&format!("[watch] using {} backend\n", inner.name()));
            state.backend = Some(Arc::new(FSWatchBackend { inner }));
        }
    }
    /// port: tsc/internal/execute/watchmanager/watchmanager.go:WatchManager.DrainEvents
    pub fn drain_events(&self) -> (HashMap<JsString, fswatch::EventKind>, bool) {
        let mut changes = lock(&self.0.changes);
        (
            std::mem::take(&mut changes.paths),
            std::mem::take(&mut changes.overflow),
        )
    }
    /// port: tsc/internal/execute/watchmanager/watchmanager.go:WatchManager.ForceOverflow
    pub fn force_overflow(&self) {
        lock(&self.0.changes).overflow = true;
    }
    /// port: tsc/internal/execute/watchmanager/watchmanager.go:WatchManager.signalDoCycle
    fn signal_cycle(&self) {
        lock(&self.0.changes).signalled = true;
        self.0.ready.notify_one();
    }
    /// port: tsc/internal/execute/watchmanager/watchmanager.go:WatchManager.onWatchEvents
    fn on_events(&self, events: &[fswatch::Event], error: Option<&fswatch::Error>) {
        if let Some(error) = error {
            if error.is_overflow() {
                self.debug_log("[watch] event overflow, triggering rebuild\n");
                self.force_overflow();
                self.signal_cycle();
            } else {
                write_all(
                    self.0.warn_writer.as_ref(),
                    format!("Warning: File watch error: {error}\n").as_bytes(),
                );
            }
            return;
        }
        if events.is_empty() {
            return;
        }
        if let Some(writer) = lock(&self.0.debug).clone() {
            let mut text = format!("[watch] {} event(s): ", events.len()).into_bytes();
            for (index, event) in events.iter().enumerate() {
                if index != 0 {
                    text.extend_from_slice(b", ");
                }
                if index >= 5 {
                    text.extend_from_slice(
                        format!("... and {} more", events.len() - index).as_bytes(),
                    );
                    break;
                }
                text.extend_from_slice(event.kind.to_string().as_bytes());
                text.push(b' ');
                text.extend_from_slice(&event.path);
            }
            text.push(b'\n');
            write_all(writer.as_ref(), &text);
        }
        let mut changes = lock(&self.0.changes);
        for event in events {
            changes
                .paths
                .insert(JsString::from_bytes(event.path.clone()), event.kind);
        }
        changes.signalled = true;
        drop(changes);
        self.0.ready.notify_one();
    }
    /// port: tsc/internal/execute/watchmanager/watchmanager.go:WatchManager.handleWatchTerminated
    fn terminated(&self, dir: &JsString, identity: &Arc<WatchedDir>) {
        self.debug_log(&format!(
            "[watch] watch terminated: {}\n",
            String::from_utf8_lossy(dir.as_bytes())
        ));
        let removed = {
            let mut state = lock(&self.0.state);
            if state
                .watched
                .get(dir)
                .is_some_and(|entry| Arc::ptr_eq(entry, identity))
            {
                state.watched.remove(dir)
            } else {
                None
            }
        };
        if let Some(entry) = removed {
            if let Some(closer) = lock(&entry.closer).take() {
                let _ = closer.close();
            }
        }
        self.force_overflow();
        self.signal_cycle();
    }
    /// port: tsc/internal/execute/watchmanager/watchmanager.go:WatchManager.CloseAllWatches
    pub fn close_all_watches(&self) {
        let entries = std::mem::take(&mut lock(&self.0.state).watched);
        for entry in entries.into_values() {
            if let Some(closer) = lock(&entry.closer).take() {
                let _ = closer.close();
            }
        }
    }
    /// port: tsc/internal/execute/watchmanager/watchmanager.go:WatchManager.ResolveDesiredDirs
    pub fn resolve_desired_dirs(
        &self,
        desired: &HashMap<JsString, bool>,
    ) -> HashMap<JsString, bool> {
        let mut resolved = HashMap::with_capacity(desired.len());
        for (dir, &recursive) in desired {
            let mut watched = dir.as_bytes().to_vec();
            let mut recursive = recursive;
            while !(self.0.dir_exists)(&watched) {
                let parent = tsr_tspath::directory(&watched);
                if parent == watched {
                    break;
                }
                watched = parent;
                recursive = false;
            }
            if !(self.0.dir_exists)(&watched) || !can_watch_directory(&watched) {
                self.debug_log(&format!(
                    "[watch] no watchable ancestor for {}\n",
                    String::from_utf8_lossy(dir.as_bytes())
                ));
                continue;
            }
            if watched != dir.as_bytes() {
                self.debug_log(&format!(
                    "[watch] resolved {} to ancestor {}\n",
                    String::from_utf8_lossy(dir.as_bytes()),
                    String::from_utf8_lossy(&watched)
                ));
            }
            *resolved.entry(JsString::from_bytes(watched)).or_default() |= recursive;
        }
        resolved
    }
    /// port: tsc/internal/execute/watchmanager/watchmanager.go:WatchManager.ReconcileWatches
    pub fn reconcile_watches(
        &self,
        desired: &HashMap<JsString, bool>,
    ) -> Result<(), fswatch::Error> {
        let (backend, closed, additions) = {
            let mut state = lock(&self.0.state);
            let Some(backend) = state.backend.clone() else {
                return Ok(());
            };
            if lock(&self.0.debug).is_some() {
                for (dir, recursive) in desired {
                    if !state.watched.contains_key(dir) {
                        self.debug_path(
                            b"[watch] watching directory ",
                            dir.as_bytes(),
                            format!(" (recursive={recursive})\n").as_bytes(),
                        );
                    }
                }
            }
            let removed: Vec<_> = state
                .watched
                .iter()
                .filter(|(dir, entry)| desired.get(*dir) != Some(&entry.recursive))
                .map(|(dir, _)| dir.clone())
                .collect();
            let closed: Vec<_> = removed
                .into_iter()
                .filter_map(|dir| {
                    let entry = state.watched.remove(&dir)?;
                    if let Some(recursive) = desired.get(&dir) {
                        self.debug_path(
                            b"[watch] recreating dir watch ",
                            dir.as_bytes(),
                            format!(" (recursive {}→{recursive})\n", entry.recursive).as_bytes(),
                        );
                    } else {
                        self.debug_path(
                            b"[watch] closing stale dir watch: ",
                            dir.as_bytes(),
                            b"\n",
                        );
                    }
                    Some((dir, entry))
                })
                .collect();
            let mut additions = Vec::new();
            for (dir, &recursive) in desired {
                if !state.watched.contains_key(dir) {
                    let entry = Arc::new(WatchedDir {
                        recursive,
                        closer: Mutex::new(None),
                    });
                    state.watched.insert(dir.clone(), entry.clone());
                    additions.push((dir.clone(), entry));
                }
            }
            (backend, closed, additions)
        };
        for (_, entry) in closed {
            if let Some(closer) = lock(&entry.closer).take() {
                let _ = closer.close();
            }
        }
        if additions.is_empty() {
            return Ok(());
        }
        let requests = additions
            .iter()
            .map(|(dir, entry)| {
                let weak = Arc::downgrade(&self.0);
                let identity = Arc::downgrade(entry);
                let dir = dir.clone();
                WatchDirectoryRequest {
                    dir: dir.as_bytes().to_vec(),
                    recursive: entry.recursive,
                    ignore: Some(Arc::new(should_ignore_watch_path)),
                    callback: Arc::new(move |events, error| {
                        if let Some(inner) = weak.upgrade() {
                            let manager = Self(inner);
                            if error.is_some_and(fswatch::Error::is_watch_terminated) {
                                if let Some(identity) = identity.upgrade() {
                                    manager.terminated(&dir, &identity);
                                } else {
                                    manager.force_overflow();
                                    manager.signal_cycle();
                                }
                            } else {
                                manager.on_events(events, error);
                            }
                        }
                    }),
                }
            })
            .collect();
        match backend.watch_directories(requests) {
            Ok(closers) => {
                assert_eq!(
                    closers.len(),
                    additions.len(),
                    "watch backend returns one closer per request"
                );
                for ((dir, entry), closer) in additions.into_iter().zip(closers) {
                    let state = lock(&self.0.state);
                    if state
                        .watched
                        .get(&dir)
                        .is_some_and(|current| Arc::ptr_eq(current, &entry))
                    {
                        *lock(&entry.closer) = Some(closer);
                    } else {
                        drop(state);
                        let _ = closer.close();
                    }
                }
                Ok(())
            }
            Err(error) => {
                let mut state = lock(&self.0.state);
                for (dir, entry) in additions {
                    self.debug_path(
                        b"[watch] failed to watch directory ",
                        dir.as_bytes(),
                        format!(": {error}\n").as_bytes(),
                    );
                    if state
                        .watched
                        .get(&dir)
                        .is_some_and(|current| Arc::ptr_eq(current, &entry))
                    {
                        state.watched.remove(&dir);
                    }
                }
                Err(error)
            }
        }
    }
    /// port: tsc/internal/execute/watchmanager/watchmanager.go:WatchManager.IsPathUnderWatch
    pub fn is_path_under_watch(&self, path: &[u8], options: &ComparePathsOptions) -> bool {
        lock(&self.0.state).watched.keys().any(|dir| {
            tsr_tspath::contains_path(
                dir.as_bytes(),
                path,
                options.current_directory.as_bytes(),
                options.use_case_sensitive_file_names,
            )
        })
    }
    /// Drives cycles until cancellation or a cycle error, closing subscriptions
    /// on either return path.
    /// port: tsc/internal/execute/watchmanager/watchmanager.go:WatchManager.RunLoop
    pub fn run_loop<E>(
        &self,
        context: &tsr_ipc::Context,
        mut do_cycle: impl FnMut() -> Result<(), E>,
    ) -> Result<(), E> {
        let weak = Arc::downgrade(&self.0);
        let stop = context.after_func(move || {
            if let Some(inner) = weak.upgrade() {
                Self(inner).signal_cycle();
            }
        });
        let mut result = Ok(());
        while context.err().is_none() {
            let mut changes = lock(&self.0.changes);
            while !changes.signalled && context.err().is_none() {
                changes = self
                    .0
                    .ready
                    .wait(changes)
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
            }
            changes.signalled = false;
            drop(changes);
            if context.err().is_none() {
                if let Err(error) = do_cycle() {
                    result = Err(error);
                    break;
                }
            }
        }
        stop.stop();
        self.close_all_watches();
        result
    }
}
impl Drop for Inner {
    fn drop(&mut self) {
        let state = self
            .state
            .get_mut()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for entry in std::mem::take(&mut state.watched).into_values() {
            if let Some(closer) = lock(&entry.closer).take() {
                let _ = closer.close();
            }
        }
    }
}

#[cfg(test)]
mod tests;
