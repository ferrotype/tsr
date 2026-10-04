//! Native LSP watch fallback. One actor owns registration and reconciliation;
//! backend callbacks only enqueue, including callbacks during installation.
use crate::dynamic_queue::DynamicQueue;
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex, Weak},
    thread::JoinHandle,
    time::Duration,
};
use tsr_fswatch::{Event, EventKind, WatchCallback, WatchOptions};
use tsr_ipc::{Context, ContextError};
use tsr_lsproto as lsp;
use tsr_vfs::FileSystem;

trait Subscription: Send {}
impl Subscription for tsr_fswatch::Watch {}
type LiveSubscription = Mutex<Option<Box<dyn Subscription>>>;
type Subscriptions = Arc<Mutex<Vec<Weak<LiveSubscription>>>>;
trait Backend: Send + Sync {
    fn watch(
        &self,
        directory: &[u8],
        callback: WatchCallback,
        recursive: bool,
    ) -> Result<Box<dyn Subscription>, tsr_fswatch::Error>;
}
struct Native(tsr_fswatch::Watcher);
impl Backend for Native {
    fn watch(
        &self,
        directory: &[u8],
        callback: WatchCallback,
        recursive: bool,
    ) -> Result<Box<dyn Subscription>, tsr_fswatch::Error> {
        self.0
            .watch_directory(
                directory,
                callback,
                WatchOptions {
                    recursive,
                    ..Default::default()
                },
            )
            .map(|w| Box::new(w) as Box<dyn Subscription>)
    }
}
type Reply = DynamicQueue<Result<(), String>>;
enum Command {
    Add {
        id: String,
        patterns: Vec<lsp::FileSystemWatcher>,
        context: Context,
        reply: Reply,
    },
    Remove {
        id: String,
        reply: Reply,
    },
    Events {
        id: String,
        index: usize,
        epoch: u64,
        events: Vec<Event>,
        error: Option<tsr_fswatch::Error>,
    },
}
struct Watch {
    requested: Vec<u8>,
    kind: u32,
    recursive: bool,
    subscription: Option<Arc<LiveSubscription>>,
    watched: Vec<u8>,
    target: bool,
    epoch: u64,
}
struct State {
    fs: Arc<dyn FileSystem>,
    backend: Arc<dyn Backend>,
    queue: DynamicQueue<Command>,
    context: Context,
    watches: BTreeMap<String, Vec<Watch>>,
    next_epoch: u64,
    pending: BTreeMap<lsp::DocumentUri, lsp::FileEvent>,
    logger: Arc<crate::logger::Logger>,
    subscriptions: Subscriptions,
}
pub struct Watcher {
    queue: DynamicQueue<Command>,
    context: Context,
    worker: Mutex<Option<JoinHandle<()>>>,
    subscriptions: Subscriptions,
}
impl Watcher {
    // port: tsc/internal/lsp/lspwatcher/lspwatcher.go:New
    pub fn new(
        fs: Arc<dyn FileSystem>,
        changed: Arc<dyn Fn(Vec<lsp::FileEvent>) + Send + Sync>,
        logger: Arc<crate::logger::Logger>,
    ) -> Arc<Self> {
        Self::with_backend(
            fs,
            Arc::new(Native(tsr_fswatch::default_watcher())),
            changed,
            logger,
        )
    }
    fn with_backend(
        fs: Arc<dyn FileSystem>,
        backend: Arc<dyn Backend>,
        changed: Arc<dyn Fn(Vec<lsp::FileEvent>) + Send + Sync>,
        logger: Arc<crate::logger::Logger>,
    ) -> Arc<Self> {
        let context = Context::background().with_cancel();
        let queue = DynamicQueue::new();
        let subscriptions = Subscriptions::default();
        let mut state = State {
            fs,
            backend,
            queue: queue.clone(),
            context: context.clone(),
            watches: BTreeMap::new(),
            next_epoch: 0,
            pending: BTreeMap::new(),
            logger,
            subscriptions: subscriptions.clone(),
        };
        let worker = std::thread::spawn(move || {
            let mut timer: Option<Context> = None;
            loop {
                match state.queue.get(timer.as_ref().unwrap_or(&state.context)) {
                    Ok(command) => state.process(command),
                    Err(ContextError::DeadlineExceeded) if state.context.err().is_none() => {
                        timer = None;
                        let changes = std::mem::take(&mut state.pending)
                            .into_values()
                            .collect::<Vec<_>>();
                        if !changes.is_empty() {
                            changed(changes);
                        }
                    }
                    Err(_) => break,
                }
                if timer.is_none() && !state.pending.is_empty() {
                    timer = Some(state.context.with_timeout(Duration::from_millis(75)));
                }
            }
            if let Some(timer) = timer {
                timer.cancel();
            }
            state.watches.clear();
        });
        Arc::new(Self {
            queue,
            context,
            worker: Mutex::new(Some(worker)),
            subscriptions,
        })
    }
    // port: tsc/internal/lsp/lspwatcher/lspwatcher.go:Watcher.WatchFiles
    pub fn watch_files(
        &self,
        context: &Context,
        id: String,
        patterns: Vec<lsp::FileSystemWatcher>,
    ) -> Result<(), String> {
        let reply = Reply::new();
        self.queue
            .put(
                &self.context,
                Command::Add {
                    id,
                    patterns,
                    context: context.clone(),
                    reply: reply.clone(),
                },
            )
            .map_err(|e| e.to_string())?;
        self.wait_reply(context, &reply)
    }
    // port: tsc/internal/lsp/lspwatcher/lspwatcher.go:Watcher.UnwatchFiles
    pub fn unwatch_files(&self, context: &Context, id: String) -> Result<(), String> {
        let reply = Reply::new();
        self.queue
            .put(
                &self.context,
                Command::Remove {
                    id,
                    reply: reply.clone(),
                },
            )
            .map_err(|e| e.to_string())?;
        self.wait_reply(context, &reply)
    }
    fn wait_reply(&self, context: &Context, reply: &Reply) -> Result<(), String> {
        let scope = context.with_cancel();
        let cancel = scope.clone();
        let stop = self.context.after_func(move || cancel.cancel());
        let result = reply.get(&scope).map_err(|e| e.to_string());
        stop.stop();
        result?
    }
    // port: tsc/internal/lsp/lspwatcher/lspwatcher.go:Watcher.Close
    pub fn close(&self) {
        self.context.cancel();
        // Do not wait for an in-flight WatchDirectory call. The pin permits
        // Close while registration is blocked. Retire installed subscriptions
        // now; a late installation checks cancellation before publishing.
        let live = std::mem::take(&mut *self.subscriptions.lock().unwrap());
        for weak in live {
            if let Some(subscription) = weak.upgrade() {
                subscription.lock().unwrap().take();
            }
        }
        if let Some(worker) = self.worker.lock().unwrap().take() {
            if worker.is_finished() {
                let _ = worker.join();
            }
        }
    }
}
impl Drop for Watcher {
    fn drop(&mut self) {
        self.close();
    }
}
impl tsr_project::watch::WatchClient for Watcher {
    fn watch_files(
        &self,
        context: &Context,
        id: &tsr_jsstring::JsString,
        watcher: &tsr_project::watch::Watcher,
    ) -> Result<(), String> {
        self.watch_files(
            context,
            String::from_utf8(id.as_bytes().to_vec()).map_err(|e| e.to_string())?,
            vec![watcher.to_protocol().map_err(|e| e.to_string())?],
        )
    }
    fn unwatch_files(&self, context: &Context, id: &tsr_jsstring::JsString) -> Result<(), String> {
        self.unwatch_files(
            context,
            String::from_utf8(id.as_bytes().to_vec()).map_err(|e| e.to_string())?,
        )
    }
}
impl State {
    fn process(&mut self, command: Command) {
        match command {
            Command::Add {
                id,
                patterns,
                context,
                reply,
            } => {
                let mut result = if context.err().is_some() {
                    Err("request canceled".into())
                } else {
                    self.add(&id, patterns)
                };
                if result.is_ok() && context.err().is_some() {
                    self.watches.remove(&id);
                    result = Err("request canceled".into());
                }
                let _ = reply.put(&Context::background(), result);
            }
            Command::Remove { id, reply } => {
                let result = self
                    .watches
                    .remove(&id)
                    .map(|_| ())
                    .ok_or_else(|| format!("lspwatcher: no watcher with id {id:?}"));
                let _ = reply.put(&Context::background(), result);
            }
            Command::Events {
                id,
                index,
                epoch,
                events,
                error,
            } => {
                let Some(mut watches) = self.watches.remove(&id) else {
                    return;
                };
                if let Some(watch) = watches.get_mut(index).filter(|w| w.epoch == epoch) {
                    if watch.target {
                        if let Some(error) = &error {
                            self.logger.send(
                                lsp::MessageType::INFO,
                                format!(
                                    "lspwatcher: watch error in {:?}: {error}",
                                    String::from_utf8_lossy(&watch.watched)
                                ),
                            );
                        }
                        self.forward(watch.kind, &events);
                        if error
                            .as_ref()
                            .is_some_and(tsr_fswatch::Error::is_watch_terminated)
                        {
                            watch.subscription = None;
                            watch.watched.clear();
                            watch.target = false;
                            if let Err(e) = self.reconcile(&id, index, watch, true) {
                                self.logger.send(lsp::MessageType::ERROR, e.to_string());
                            }
                        }
                    } else if let Err(e) = self.reconcile(&id, index, watch, true) {
                        self.logger.send(lsp::MessageType::ERROR, e.to_string());
                    }
                }
                self.watches.insert(id, watches);
            }
        }
    }
    fn add(&mut self, id: &str, patterns: Vec<lsp::FileSystemWatcher>) -> Result<(), String> {
        if self.watches.contains_key(id) {
            return Err(format!("lspwatcher: watcher {id:?} already exists"));
        }
        let mut watches = Vec::new();
        for pattern in patterns {
            let Some(root) = watch_root(&pattern).filter(|root| !root.is_empty()) else {
                continue;
            };
            let mut watch = Watch {
                requested: root,
                kind: pattern.kind.as_deref().map_or(7, |k| k.0),
                recursive: pattern_string(&pattern).contains("**"),
                subscription: None,
                watched: Vec::new(),
                target: false,
                epoch: 0,
            };
            self.reconcile(id, watches.len(), &mut watch, false)
                .map_err(|e| format!("lspwatcher: failed to register watcher {id:?}: {e}"))?;
            watches.push(watch);
        }
        self.watches.insert(id.into(), watches);
        Ok(())
    }
    // port: tsc/internal/lsp/lspwatcher/lspwatcher.go:watch.reconcile
    fn reconcile(
        &mut self,
        id: &str,
        index: usize,
        watch: &mut Watch,
        mut synthetic: bool,
    ) -> Result<(), tsr_fswatch::Error> {
        loop {
            if self.context.err().is_some() {
                return Ok(());
            }
            let target = self.fs.directory_exists(&watch.requested).unwrap_or(false);
            let directory = if target {
                Some(watch.requested.clone())
            } else {
                nearest_existing_ancestor(self.fs.as_ref(), &watch.requested)
            };
            let Some(directory) = directory else {
                watch.subscription = None;
                watch.watched.clear();
                watch.target = false;
                return Ok(());
            };
            if watch.subscription.is_some() && watch.target == target && watch.watched == directory
            {
                return Ok(());
            }
            self.next_epoch += 1;
            let epoch = self.next_epoch;
            let queue = self.queue.clone();
            let context = self.context.clone();
            let id = id.to_owned();
            let subscription = self.backend.watch(
                &directory,
                Arc::new(move |events, error| {
                    let _ = queue.put(
                        &context,
                        Command::Events {
                            id: id.clone(),
                            index,
                            epoch,
                            events: events.to_vec(),
                            error: error.cloned(),
                        },
                    );
                }),
                target && watch.recursive,
            )?;
            let subscription = Arc::new(Mutex::new(Some(subscription)));
            {
                let mut live = self.subscriptions.lock().unwrap();
                if self.context.err().is_some() {
                    return Ok(());
                }
                live.retain(|entry| entry.strong_count() != 0);
                live.push(Arc::downgrade(&subscription));
            }
            watch.subscription = Some(subscription);
            watch.watched = directory;
            watch.target = target;
            watch.epoch = epoch;
            if target {
                if synthetic {
                    self.synthetic_creates(&watch.requested, watch.kind, watch.recursive);
                }
                return Ok(());
            }
            synthetic = true;
        }
    }
    // port: tsc/internal/lsp/lspwatcher/lspwatcher.go:Watcher.forwardEvents
    fn forward(&mut self, kind: u32, events: &[Event]) {
        for event in events {
            let kind = match event.kind {
                EventKind::EventUpdate if kind & 3 != 0 => lsp::FileChangeType::CHANGED,
                EventKind::EventDelete if kind & 4 != 0 => lsp::FileChangeType::DELETED,
                _ => continue,
            };
            let uri = lsp::DocumentUri::from_file_name(
                tsr_tspath::normalize_slashes(&event.path).as_ref(),
            );
            self.pending
                .insert(uri.clone(), lsp::FileEvent { uri, r#type: kind });
        }
    }
    // port: tsc/internal/lsp/lspwatcher/lspwatcher.go:Watcher.emitSyntheticCreates
    fn synthetic_creates(&mut self, directory: &[u8], kind: u32, recursive: bool) {
        if kind & 1 == 0 {
            return;
        }
        let mut paths = vec![directory.to_vec()];
        if recursive {
            let _ = self.fs.walk_dir(directory, &mut |path, _, error| {
                if error.is_none() && path != directory {
                    paths.push(tsr_tspath::normalize_slashes(path).into_owned());
                }
                Ok(tsr_vfs::WalkControl::Continue)
            });
        } else if let Ok(entries) = self.fs.entries(directory) {
            paths.extend(
                entries
                    .files
                    .iter()
                    .chain(&entries.directories)
                    .flatten()
                    .map(|name| tsr_tspath::combine(directory, &[name.as_bytes()])),
            );
        }
        for path in paths {
            let uri = lsp::DocumentUri::from_file_name(&path);
            self.pending.entry(uri.clone()).or_insert(lsp::FileEvent {
                uri,
                r#type: lsp::FileChangeType::CREATED,
            });
        }
    }
}
// port: tsc/internal/lsp/lspwatcher/lspwatcher.go:nearestExistingAncestor
fn nearest_existing_ancestor(fs: &dyn FileSystem, path: &[u8]) -> Option<Vec<u8>> {
    let mut path = path.to_vec();
    loop {
        if fs.directory_exists(&path).unwrap_or(false) {
            return Some(path);
        }
        let parent = tsr_tspath::directory(&path);
        if parent == path {
            return None;
        }
        path = parent;
    }
}
// port: tsc/internal/lsp/lspwatcher/lspwatcher.go:rootFromGlob
fn root_from_glob(pattern: &[u8]) -> Vec<u8> {
    let pattern = tsr_tspath::normalize_slashes(pattern);
    let end = pattern
        .iter()
        .position(|c| b"*?[{".contains(c))
        .unwrap_or(pattern.len());
    let mut root = &pattern[..end];
    while root.ends_with(b"/") {
        root = &root[..root.len() - 1];
    }
    if root.is_empty() {
        return Vec::new();
    }
    tsr_tspath::normalize(root).into_owned()
}
// port: tsc/internal/lsp/lspwatcher/lspwatcher.go:watchRoot
fn watch_root(watcher: &lsp::FileSystemWatcher) -> Option<Vec<u8>> {
    if let Some(pattern) = watcher.glob_pattern.pattern.as_deref() {
        return Some(root_from_glob(pattern.as_bytes()));
    }
    let relative = watcher.glob_pattern.relative_pattern.as_deref()?;
    let base = relative.base_uri.uri.as_deref()?;
    Some(root_from_glob(&tsr_tspath::combine(
        lsp::DocumentUri(base.0.clone()).file_name().as_bytes(),
        &[relative.pattern.as_bytes()],
    )))
}
// port: tsc/internal/lsp/lspwatcher/lspwatcher.go:watchPatternString
fn pattern_string(watcher: &lsp::FileSystemWatcher) -> String {
    if let Some(pattern) = watcher.glob_pattern.pattern.as_deref() {
        return pattern.clone();
    }
    watcher
        .glob_pattern
        .relative_pattern
        .as_deref()
        .map(|p| {
            format!(
                "{}/{}",
                p.base_uri.uri.as_deref().map_or("", |u| &u.0),
                p.pattern
            )
        })
        .unwrap_or_default()
}
#[cfg(test)]
mod tests;
