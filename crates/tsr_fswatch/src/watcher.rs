#![forbid(unsafe_code)]
use crate::{debounce::Debounce, event::EventList, lock, Error, Event, EventKind};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};

pub type WatchCallback = Arc<dyn Fn(&[Event], Option<&Error>) + Send + Sync>;
pub type Ignore = Arc<dyn Fn(&[u8]) -> bool + Send + Sync>;
#[derive(Clone, Default)]
pub struct WatchOptions {
    pub recursive: bool,
    pub ignore: Option<Ignore>,
}
#[derive(Clone)]
pub struct WatchDirectoryRequest {
    pub dir: Vec<u8>,
    pub callback: WatchCallback,
    pub options: WatchOptions,
}
pub(crate) trait Backend: Send + Sync {
    fn add_many(&self, watches: &[Arc<DirWatch>]) -> Result<(), Error>;
    fn remove(&self, watch: &Arc<DirWatch>) -> Result<(), Error>;
    fn shutdown(&self);
    fn sequence(&self) -> Option<u64> {
        None
    }
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Inotify,
    Fanotify,
    #[cfg(all(test, target_os = "linux"))]
    FanotifyNoRename,
    Fsevents,
    Kqueue,
    Windows,
    Unsupported,
}
#[derive(Default)]
struct State {
    dirs: Vec<Arc<DirWatch>>,
    backend: Option<Arc<dyn Backend>>,
    debounce: Option<Arc<Debounce>>,
}
#[cfg(test)]
type TestBackendStartup = fn() -> Result<Arc<dyn Backend>, Error>;
struct Owner {
    kind: Kind,
    #[cfg(test)]
    startup_for_test: Option<TestBackendStartup>,
    state: Mutex<State>,
}
/// A shared watcher owner. Retained watches keep it alive. Use `close` to
/// explicitly close all subscriptions; ordinary Watch drops close individually.
#[derive(Clone)]
pub struct Watcher(Arc<Owner>);
impl std::fmt::Display for Watcher {
    // port: tsc/internal/fswatch/watcher.go:watcher.String
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name())
    }
}
fn new(kind: Kind) -> Watcher {
    Watcher(Arc::new(Owner {
        kind,
        #[cfg(test)]
        startup_for_test: None,
        state: Mutex::new(State::default()),
    }))
}
fn shared(kind: Kind, slot: &'static OnceLock<Mutex<Weak<Owner>>>) -> Watcher {
    let mut slot = lock(slot.get_or_init(|| Mutex::new(Weak::new())));
    if let Some(owner) = slot.upgrade() {
        return Watcher(owner);
    }
    let watcher = new(kind);
    *slot = Arc::downgrade(&watcher.0);
    watcher
}
// port: tsc/internal/fswatch/watcher.go:Inotify
pub fn inotify() -> Watcher {
    static INSTANCE: OnceLock<Mutex<Weak<Owner>>> = OnceLock::new();
    shared(Kind::Inotify, &INSTANCE)
}
// port: tsc/internal/fswatch/watcher.go:Fanotify
pub fn fanotify() -> Watcher {
    static INSTANCE: OnceLock<Mutex<Weak<Owner>>> = OnceLock::new();
    shared(Kind::Fanotify, &INSTANCE)
}
// port: tsc/internal/fswatch/watcher.go:FSEvents
pub fn fsevents() -> Watcher {
    static INSTANCE: OnceLock<Mutex<Weak<Owner>>> = OnceLock::new();
    shared(Kind::Fsevents, &INSTANCE)
}
// port: tsc/internal/fswatch/watcher.go:Kqueue
pub fn kqueue() -> Watcher {
    new(Kind::Kqueue)
}
// port: tsc/internal/fswatch/watcher.go:Windows
pub fn windows() -> Watcher {
    new(Kind::Windows)
}
// port: tsc/internal/fswatch/watcher.go:AllWatchers
pub fn all_watchers() -> Vec<Watcher> {
    vec![inotify(), fsevents(), kqueue(), windows(), fanotify()]
}
// port: tsc/internal/fswatch/watcher.go:Default
pub fn default_watcher() -> Watcher {
    #[cfg(target_os = "linux")]
    {
        let watcher = fanotify();
        if watcher.available() {
            return watcher;
        }
        return inotify();
    }
    #[cfg(target_os = "macos")]
    {
        return fsevents();
    }
    #[allow(unreachable_code)]
    new(Kind::Unsupported)
}
impl Watcher {
    #[cfg(test)]
    pub(crate) fn with_backend_for_test(backend: Arc<dyn Backend>) -> Self {
        let watcher = new(Kind::Fsevents);
        lock(&watcher.0.state).backend = Some(backend);
        watcher
    }
    // port: tsc/internal/fswatch/watcher.go:watcher.Name
    pub fn name(&self) -> &'static str {
        match self.0.kind {
            Kind::Inotify => "inotify",
            Kind::Fanotify => "fanotify",
            #[cfg(all(test, target_os = "linux"))]
            Kind::FanotifyNoRename => "fanotify-no-rename",
            Kind::Fsevents => "fsevents",
            Kind::Kqueue => "kqueue",
            Kind::Windows => "windows",
            Kind::Unsupported => "unsupported",
        }
    }
    // port: tsc/internal/fswatch/watcher.go:watcher.Available
    pub fn available(&self) -> bool {
        #[cfg(test)]
        if self.0.startup_for_test.is_some() {
            return true;
        }
        match self.0.kind {
            #[cfg(target_os = "linux")]
            Kind::Inotify => true,
            #[cfg(target_os = "linux")]
            Kind::Fanotify => crate::linux::fanotify_available(),
            #[cfg(all(test, target_os = "linux"))]
            Kind::FanotifyNoRename => crate::linux::fanotify_available(),
            #[cfg(target_os = "macos")]
            Kind::Fsevents => crate::macos::available(),
            _ => false,
        }
    }
    // port: tsc/internal/fswatch/watcher.go:watcher.HasFastRecursiveBackend
    pub fn has_fast_recursive_backend(&self) -> bool {
        matches!(self.0.kind, Kind::Fsevents | Kind::Windows)
    }
    fn backend(&self) -> Result<Arc<dyn Backend>, Error> {
        #[cfg(test)]
        if let Some(start) = self.0.startup_for_test {
            return start();
        }
        match self.0.kind {
            #[cfg(target_os = "linux")]
            Kind::Inotify => crate::linux::inotify(),
            #[cfg(target_os = "linux")]
            Kind::Fanotify => crate::linux::fanotify(),
            #[cfg(all(test, target_os = "linux"))]
            Kind::FanotifyNoRename => crate::linux::fanotify_no_rename(),
            #[cfg(target_os = "macos")]
            Kind::Fsevents => crate::macos::new(),
            _ => Err(Error::Unavailable),
        }
    }
    /// A callback is required by the type system, including for empty batches.
    /// ```compile_fail
    /// use tsr_fswatch::{default_watcher, WatchOptions};
    /// default_watcher().watch_directory(b"/tmp", None, WatchOptions::default());
    /// ```
    // source: tsc/internal/fswatch/watcher_test.go:TestSubscribeRejectsNilCallback
    // port: tsc/internal/fswatch/watcher.go:watcher.WatchDirectory
    pub fn watch_directory(
        &self,
        dir: &[u8],
        callback: WatchCallback,
        options: WatchOptions,
    ) -> Result<Watch, Error> {
        self.watch_directories(&[WatchDirectoryRequest {
            dir: dir.to_vec(),
            callback,
            options,
        }])
        .map(|mut watches| watches.remove(0))
    }
    pub fn watch_directories(
        &self,
        requests: &[WatchDirectoryRequest],
    ) -> Result<Vec<Watch>, Error> {
        if let Some(secondary) = self.fallback_watcher() {
            return crate::fallback::watch_directories(
                requests,
                |requests| self.watch_directories_native(requests),
                |request| {
                    secondary.watch_directory(
                        &request.dir,
                        request.callback.clone(),
                        request.options.clone(),
                    )
                },
            );
        }
        self.watch_directories_native(requests)
    }
    fn fallback_watcher(&self) -> Option<Watcher> {
        (self.0.kind == Kind::Fanotify).then(inotify)
    }
    fn watch_directories_native(
        &self,
        requests: &[WatchDirectoryRequest],
    ) -> Result<Vec<Watch>, Error> {
        if !self.available() {
            return Err(Error::Unavailable);
        }
        self.register_directories(requests)
    }
    // port: tsc/internal/fswatch/watcher.go:watcher.WatchDirectories
    fn register_directories(
        &self,
        requests: &[WatchDirectoryRequest],
    ) -> Result<Vec<Watch>, Error> {
        if requests.is_empty() {
            return Ok(Vec::new());
        }
        // Validate the complete batch before registering any callbacks.
        let mut prepared = Vec::with_capacity(requests.len());
        for request in requests {
            let dir = clean(&request.dir);
            if !dir.starts_with(b"/") {
                return Err(Error::Message("fswatch: path must be absolute".into()));
            }
            let dir = canonicalize(&dir);
            let metadata = std::fs::metadata(os_path(&dir))?;
            if !metadata.is_dir() {
                return Err(std::io::Error::from_raw_os_error(20).into());
            }
            let physical = physical_dir_for(&dir);
            prepared.push((dir, physical, request));
        }
        let mut state = lock(&self.0.state);
        if state.backend.is_none() {
            state.backend = Some(self.backend()?);
        }
        if state.debounce.is_none() {
            state.debounce = Some(Debounce::new());
        }
        let backend = state.backend.as_ref().unwrap().clone();
        let debounce = state.debounce.as_ref().unwrap().clone();
        let mut registrations = Vec::new();
        let mut added = Vec::new();
        for (dir, physical, request) in prepared {
            let mut root = dir.clone();
            let mut physical_root = physical.clone();
            let mut recursive = request.options.recursive;
            let covering = if self.0.kind == Kind::Fsevents {
                state
                    .dirs
                    .iter()
                    .filter(|watch| {
                        watch.recursive
                            && is_in_directory_or_self(&watch.dir, &dir)
                            && is_in_directory_or_self(&watch.physical_dir, &physical)
                    })
                    .max_by_key(|watch| watch.dir.len())
                    .cloned()
            } else {
                None
            };
            let watch = if let Some(watch) = covering {
                watch
            } else {
                if self.0.kind == Kind::Fsevents {
                    let mut parent = parent_dir(&dir);
                    while parent != b"/" && parent != b"." {
                        let physical_parent = physical_dir_for(&parent);
                        if !is_in_directory_or_self(&physical_parent, &physical) {
                            break;
                        }
                        let count = state
                            .dirs
                            .iter()
                            .filter(|watch| {
                                is_in_directory_or_self(&parent, &watch.dir)
                                    && is_in_directory_or_self(
                                        &physical_parent,
                                        &watch.physical_dir,
                                    )
                            })
                            .count()
                            + 1;
                        if count >= 10 {
                            root = parent;
                            physical_root = physical_parent;
                            recursive = true;
                            break;
                        }
                        let next = parent_dir(&parent);
                        if next == parent {
                            break;
                        }
                        parent = next;
                    }
                }
                if let Some(watch) = state
                    .dirs
                    .iter()
                    .find(|watch| watch.dir == root && watch.recursive == recursive)
                {
                    watch.clone()
                } else {
                    let watch = Arc::new(DirWatch {
                        dir: root,
                        physical_dir: physical_root,
                        recursive,
                        events: EventList::default(),
                        callbacks: Mutex::new(Callbacks::default()),
                        debounce: Arc::downgrade(&debounce),
                    });
                    debounce.add(&watch);
                    state.dirs.push(watch.clone());
                    added.push(watch.clone());
                    watch
                }
            };
            let id = watch.watch(
                dir,
                physical,
                request.callback.clone(),
                request.options.clone(),
                backend.sequence(),
            );
            registrations.push((watch, id));
        }
        if let Err(error) = backend.add_many(&added) {
            for (watch, id) in &registrations {
                watch.unwatch(*id);
            }
            for watch in &added {
                let _ = backend.remove(watch);
                state.dirs.retain(|existing| !Arc::ptr_eq(existing, watch));
            }
            let stopped = if state.dirs.is_empty() {
                Some((state.backend.take(), state.debounce.take()))
            } else {
                None
            };
            drop(state);
            if let Some((backend, debounce)) = stopped {
                stop(backend, debounce);
            }
            return Err(error);
        }
        Ok(registrations
            .into_iter()
            .map(|(dir, id)| Watch {
                owner: self.0.clone(),
                dir,
                id,
                closed: AtomicBool::new(false),
            })
            .collect())
    }
    // port: tsc/internal/fswatch/watcher.go:watcher.WatchFile
    pub fn watch_file(&self, path: &[u8], callback: WatchCallback) -> Result<Watch, Error> {
        if !self.available() {
            return Err(Error::Unavailable);
        }
        let path = canonicalize(&clean(path));
        if !path.starts_with(b"/") {
            return Err(Error::Message("fswatch: path must be absolute".into()));
        }
        let parent = parent_dir(&path);
        if parent == path {
            return Err(Error::Message("fswatch: cannot watch a root path".into()));
        }
        self.watch_directory(
            &parent,
            file_callback(path, callback),
            WatchOptions::default(),
        )
    }
    pub fn close(&self) {
        self.0.close();
    }
}
impl Owner {
    fn close(&self) {
        let (backend, debounce) = {
            let mut state = lock(&self.state);
            for watch in &state.dirs {
                lock(&watch.callbacks).entries.clear();
            }
            state.dirs.clear();
            (state.backend.take(), state.debounce.take())
        };
        stop(backend, debounce);
    }
}
fn stop(backend: Option<Arc<dyn Backend>>, debounce: Option<Arc<Debounce>>) {
    if let Some(backend) = backend {
        backend.shutdown();
    }
    if let Some(debounce) = debounce {
        debounce.shutdown();
    }
}
impl Drop for Owner {
    fn drop(&mut self) {
        self.close();
    }
}
/// A subscription. Close and Drop are idempotent and may run in its callback.
pub struct Watch {
    owner: Arc<Owner>,
    dir: Arc<DirWatch>,
    id: u64,
    closed: AtomicBool,
}
impl Watch {
    // port: tsc/internal/fswatch/watcher.go:watch.Close
    pub fn close(&self) -> Result<(), Error> {
        if self.closed.swap(true, Ordering::AcqRel) {
            return Ok(());
        }
        let stopped = {
            let mut state = lock(&self.owner.state);
            if self.dir.unwatch(self.id) {
                if let Some(backend) = &state.backend {
                    // The pin removes the native watch and always returns nil
                    // from public Close, including a native teardown failure.
                    let _ = backend.remove(&self.dir);
                }
                state.dirs.retain(|watch| !Arc::ptr_eq(watch, &self.dir));
            }
            if state.dirs.is_empty() {
                Some((state.backend.take(), state.debounce.take()))
            } else {
                None
            }
        };
        if let Some((backend, debounce)) = stopped {
            stop(backend, debounce);
        }
        Ok(())
    }
}
impl Drop for Watch {
    fn drop(&mut self) {
        let _ = self.close();
    }
}
#[derive(Clone)]
struct Callback {
    id: u64,
    dir: Vec<u8>,
    physical: Vec<u8>,
    recursive: bool,
    ignore: Option<Ignore>,
    function: WatchCallback,
    since: u64,
    terminal: Option<Error>,
    delivered: bool,
}
#[derive(Default)]
struct Callbacks {
    next: u64,
    entries: Vec<Callback>,
}
pub(crate) struct DirWatch {
    pub(crate) dir: Vec<u8>,
    pub(crate) physical_dir: Vec<u8>,
    pub(crate) recursive: bool,
    pub(crate) events: EventList,
    callbacks: Mutex<Callbacks>,
    debounce: Weak<Debounce>,
}
impl DirWatch {
    #[cfg(test)]
    pub(crate) fn for_test(dir: &[u8], physical: &[u8], recursive: bool) -> Arc<Self> {
        Arc::new(Self {
            dir: dir.to_vec(),
            physical_dir: physical.to_vec(),
            recursive,
            events: EventList::default(),
            callbacks: Mutex::new(Callbacks::default()),
            debounce: Weak::new(),
        })
    }
    // port: tsc/internal/fswatch/watcher.go:dirWatch.watch
    fn watch(
        &self,
        dir: Vec<u8>,
        physical: Vec<u8>,
        function: WatchCallback,
        options: WatchOptions,
        sequence: Option<u64>,
    ) -> u64 {
        let mut callbacks = lock(&self.callbacks);
        callbacks.next = callbacks.next.wrapping_add(1);
        let id = callbacks.next;
        callbacks.entries.push(Callback {
            id,
            dir,
            physical,
            recursive: options.recursive,
            ignore: options.ignore,
            function,
            since: sequence.unwrap_or_else(|| self.events.sequence()),
            terminal: None,
            delivered: false,
        });
        id
    }
    // port: tsc/internal/fswatch/watcher.go:dirWatch.unwatch
    fn unwatch(&self, id: u64) -> bool {
        let mut c = lock(&self.callbacks);
        c.entries.retain(|cb| cb.id != id);
        c.entries.is_empty()
    }
    // port: tsc/internal/fswatch/watcher.go:dirWatch.displayPath
    pub(crate) fn display_path(&self, path: &[u8]) -> Vec<u8> {
        rebase_path(path, &self.physical_dir, &self.dir)
    }
    // port: tsc/internal/fswatch/watcher.go:dirWatch.physicalPath
    pub(crate) fn physical_path(&self, path: &[u8]) -> Vec<u8> {
        rebase_path(path, &self.dir, &self.physical_dir)
    }
    // port: tsc/internal/fswatch/watcher.go:dirWatch.notify
    pub(crate) fn notify(&self) {
        if let Some(debounce) = self.debounce.upgrade() {
            debounce.trigger();
        }
    }
    #[cfg(any(target_os = "linux", test))]
    // port: tsc/internal/fswatch/watcher.go:dirWatch.notifyError
    pub(crate) fn notify_error(&self, error: Error) {
        let callbacks = std::mem::take(&mut lock(&self.callbacks).entries);
        for cb in callbacks {
            let _ = catch_unwind(AssertUnwindSafe(|| (cb.function)(&[], Some(&error))));
        }
    }
    #[cfg(any(target_os = "macos", test))]
    // port: tsc/internal/fswatch/watcher.go:dirWatch.terminateCallbacksForDeletedRoot
    pub(crate) fn terminate_callbacks_for_deleted_root(
        &self,
        path: &[u8],
        sequence: u64,
        error: Error,
    ) -> bool {
        let mut callbacks = lock(&self.callbacks);
        let mut changed = false;
        for cb in &mut callbacks.entries {
            if cb.delivered || cb.terminal.is_some() || cb.since >= sequence {
                continue;
            }
            let physical = self.physical_path(path);
            if is_in_directory_or_self(path, &cb.dir)
                || (cb.physical != cb.dir && is_in_directory_or_self(&physical, &cb.physical))
            {
                cb.terminal = Some(error.clone());
                changed = true;
            }
        }
        changed
    }
    // port: tsc/internal/fswatch/watcher.go:dirWatch.triggerCallbacks
    pub(crate) fn trigger_callbacks(&self) {
        let mut callbacks = lock(&self.callbacks);
        let ready: Vec<_> = callbacks
            .entries
            .iter()
            .filter(|cb| !cb.delivered)
            .cloned()
            .collect();
        if !self.events.has_pending() && !ready.iter().any(|cb| cb.terminal.is_some()) {
            return;
        }
        let starts: Vec<_> = ready.iter().map(|cb| cb.since).collect();
        let (events, error) = self.events.drain_for_sequences(&starts);
        for cb in &mut callbacks.entries {
            if cb.terminal.is_some() {
                cb.delivered = true;
            }
        }
        drop(callbacks);
        for (cb, events) in ready.into_iter().zip(events) {
            let _ = catch_unwind(AssertUnwindSafe(|| {
                let mut filtered = Vec::with_capacity(events.len());
                for pending in events {
                    let mut event = pending.event;
                    if cb.physical != cb.dir {
                        let physical = self.physical_path(&event.path);
                        if is_in_directory_or_self(&cb.physical, &physical) {
                            event.path = rebase_path(&physical, &cb.physical, &cb.dir);
                        }
                    }
                    if cb.ignore.as_ref().is_some_and(|ignore| ignore(&event.path)) {
                        continue;
                    }
                    if cb.dir != self.dir
                        && !pending.included_watch_root
                        && event.path == cb.dir
                        && event.kind == EventKind::EventUpdate
                    {
                        continue;
                    }
                    if cb.recursive {
                        if cb.dir != self.dir && !is_in_directory_or_self(&cb.dir, &event.path) {
                            continue;
                        }
                    } else if !is_direct_child(&cb.dir, &event.path)
                        && !(cb.dir != self.dir && event.path == cb.dir)
                    {
                        continue;
                    }
                    filtered.push(event);
                }
                let error = cb.terminal.as_ref().or(error.as_ref());
                if !filtered.is_empty() || error.is_some() {
                    (cb.function)(&filtered, error);
                }
            }));
        }
    }
}
// port: tsc/internal/fswatch/watcher.go:fileCallback
fn file_callback(path: Vec<u8>, callback: WatchCallback) -> WatchCallback {
    Arc::new(move |events, error| {
        let filtered: Vec<_> = events
            .iter()
            .filter(|event| event.path == path)
            .cloned()
            .collect();
        if !filtered.is_empty() || error.is_some() {
            callback(&filtered, error);
        }
    })
}
pub(crate) fn os_path(path: &[u8]) -> &Path {
    Path::new(std::ffi::OsStr::from_bytes(path))
}
fn clean(path: &[u8]) -> Vec<u8> {
    let mut result = PathBuf::new();
    for component in os_path(path).components() {
        match component {
            Component::ParentDir => {
                if !result.pop() && !result.has_root() {
                    result.push("..");
                }
            }
            _ => result.push(component.as_os_str()),
        }
    }
    if result.as_os_str().is_empty() {
        b".".to_vec()
    } else {
        result.into_os_string().into_vec()
    }
}
fn parent_dir(path: &[u8]) -> Vec<u8> {
    os_path(path)
        .parent()
        .map(|p| p.as_os_str().as_bytes().to_vec())
        .unwrap_or_else(|| path.to_vec())
}
pub(crate) fn canonicalize(path: &[u8]) -> Vec<u8> {
    #[cfg(target_os = "macos")]
    return crate::macos::canonicalize(path);
    #[cfg(not(target_os = "macos"))]
    path.to_vec()
}
// port: tsc/internal/fswatch/watcher.go:physicalDirFor
fn physical_dir_for(path: &[u8]) -> Vec<u8> {
    std::fs::canonicalize(os_path(path))
        .map(|p| canonicalize(p.as_os_str().as_bytes()))
        .unwrap_or_else(|_| path.to_vec())
}
// port: tsc/internal/fswatch/watcher.go:isInDirectoryOrSelf
pub(crate) fn is_in_directory_or_self(dir: &[u8], path: &[u8]) -> bool {
    !dir.is_empty()
        && (path == dir
            || path.strip_prefix(dir).is_some_and(|rest| {
                !rest.is_empty() && (dir.ends_with(b"/") || rest.starts_with(b"/"))
            }))
}
// port: tsc/internal/fswatch/watcher.go:isDirectChild
pub(crate) fn is_direct_child(dir: &[u8], path: &[u8]) -> bool {
    path.strip_prefix(dir)
        .and_then(|rest| {
            if dir.ends_with(b"/") {
                Some(rest)
            } else {
                rest.strip_prefix(b"/")
            }
        })
        .is_some_and(|rest| !rest.is_empty() && !rest.contains(&b'/'))
}
// port: tsc/internal/fswatch/watcher.go:rebasePath
pub(crate) fn rebase_path(path: &[u8], from: &[u8], to: &[u8]) -> Vec<u8> {
    if from == to || !is_in_directory_or_self(from, path) {
        return path.to_vec();
    }
    join_suffix(to, &path[from.len()..])
}
// port: tsc/internal/fswatch/watcher.go:joinPathSuffix
pub(crate) fn join_suffix(root: &[u8], suffix: &[u8]) -> Vec<u8> {
    let mut path = root.to_vec();
    if !suffix.is_empty() {
        if path.ends_with(b"/") && suffix.starts_with(b"/") {
            path.extend_from_slice(&suffix[1..]);
        } else {
            if !path.ends_with(b"/") && !suffix.starts_with(b"/") {
                path.push(b'/');
            }
            path.extend_from_slice(suffix);
        }
    }
    path
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod roster;
