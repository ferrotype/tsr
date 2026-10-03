//! Derived from the pinned fswatch watcher tests; events are unordered batches.
use super::*;
use std::sync::atomic::AtomicUsize;

type Batches = Arc<Mutex<Vec<(Vec<Event>, Option<Error>)>>>;
fn callback() -> (WatchCallback, Batches) {
    let batches = Arc::new(Mutex::new(Vec::new()));
    let captured = batches.clone();
    (
        Arc::new(move |events, error| {
            let mut events = events.to_vec();
            events.sort_by(|a, b| a.path.cmp(&b.path));
            lock(&captured).push((events, error.cloned()));
        }),
        batches,
    )
}
fn register(watch: &DirWatch, dir: &[u8], recursive: bool) -> Batches {
    let (callback, batches) = callback();
    watch.watch(
        dir.to_vec(),
        dir.to_vec(),
        callback,
        WatchOptions {
            recursive,
            ignore: None,
        },
        None,
    );
    batches
}
fn paths(batches: &Batches) -> Vec<Vec<u8>> {
    lock(batches)
        .iter()
        .flat_map(|(events, _)| events.iter().map(|e| e.path.clone()))
        .collect()
}
#[test]
fn direct_and_recursive_callbacks_share_events_without_leaking_children() {
    let watch = DirWatch::for_test(b"/root", b"/root", true);
    let direct = register(&watch, b"/root", false);
    let recursive = register(&watch, b"/root", true);
    let child = register(&watch, b"/root/child", false);
    for path in [
        b"/root/file".as_slice(),
        b"/root/child",
        b"/root/child/file",
        b"/root/child/deep/file",
        b"/root/sibling/file",
    ] {
        watch.events.update(path);
    }
    watch.trigger_callbacks();
    assert_eq!(
        paths(&direct),
        [b"/root/child".to_vec(), b"/root/file".to_vec()]
    );
    assert_eq!(paths(&recursive).len(), 5);
    assert_eq!(paths(&child), [b"/root/child/file".to_vec()]);
}
#[test]
fn covering_watch_includes_only_explicit_root_updates() {
    let watch = DirWatch::for_test(b"/root", b"/root", true);
    let child = register(&watch, b"/root/child", false);
    watch.events.update_at(b"/root/child", 1);
    watch.trigger_callbacks();
    assert!(lock(&child).is_empty());
    watch.events.update_watch_root_at(b"/root/child", 2);
    watch.trigger_callbacks();
    watch.events.remove_watch_root_at(b"/root/child", 3);
    watch.trigger_callbacks();
    let batches = lock(&child);
    assert_eq!(batches.len(), 2);
    assert_eq!(batches[0].0[0].kind, EventKind::EventUpdate);
    assert_eq!(batches[1].0[0].kind, EventKind::EventDelete);
}
#[test]
fn callback_cutoff_preserves_deletion_of_a_preexisting_pending_create() {
    let watch = DirWatch::for_test(b"/root", b"/root", true);
    let early = register(&watch, b"/root", true);
    watch.events.create(b"/root/file");
    let late = register(&watch, b"/root", true);
    watch.events.remove(b"/root/file");
    watch.trigger_callbacks();
    assert!(lock(&early).is_empty());
    assert_eq!(lock(&late)[0].0[0].kind, EventKind::EventDelete);
    watch.events.update_at(b"/root/old", 100);
    let (callback, cutoff) = callback();
    watch.watch(
        b"/root".to_vec(),
        b"/root".to_vec(),
        callback,
        WatchOptions {
            recursive: true,
            ignore: None,
        },
        Some(100),
    );
    watch.events.update_at(b"/root/new", 101);
    watch.trigger_callbacks();
    assert_eq!(paths(&cutoff), [b"/root/new".to_vec()]);
}
#[test]
fn physical_paths_are_rebased_under_each_logical_symlink_root() {
    let watch = DirWatch::for_test(b"/display", b"/physical", true);
    let (callback, batches) = callback();
    watch.watch(
        b"/display/link".to_vec(),
        b"/physical/target".to_vec(),
        callback,
        WatchOptions {
            recursive: true,
            ignore: None,
        },
        None,
    );
    watch.events.update(b"/display/target/file");
    watch.events.update(b"/display/target2/file");
    watch.trigger_callbacks();
    assert_eq!(paths(&batches), [b"/display/link/file".to_vec()]);
    assert_eq!(watch.display_path(b"/physical/file"), b"/display/file");
    assert_eq!(watch.display_path(b"/physical2/file"), b"/physical2/file");
    assert_eq!(rebase_path(b"/file", b"/", b"/logical"), b"/logical/file");
}
#[test]
fn deleted_logical_roots_terminate_older_callbacks_once() {
    let watch = DirWatch::for_test(b"/root", b"/root", true);
    let child = register(&watch, b"/root/child", true);
    let sibling = register(&watch, b"/root/sibling", true);
    let sequence = watch.events.remove_and_get_sequence(b"/root/child");
    let late = register(&watch, b"/root/child", true);
    assert!(watch.terminate_callbacks_for_deleted_root(
        b"/root/child",
        sequence,
        Error::WatchTerminated
    ));
    watch.trigger_callbacks();
    assert_eq!(lock(&child)[0].1, Some(Error::WatchTerminated));
    assert_eq!(lock(&child)[0].0[0].kind, EventKind::EventDelete);
    assert!(lock(&sibling).is_empty());
    assert!(lock(&late).is_empty());
    watch.events.update(b"/root/child/new");
    watch.trigger_callbacks();
    assert_eq!(lock(&child).len(), 1);
    assert_eq!(paths(&late), [b"/root/child/new".to_vec()]);
}
#[test]
fn ignored_events_do_not_hide_overflow_and_watch_stays_active() {
    let watch = DirWatch::for_test(b"/root", b"/root", true);
    let (callback, batches) = callback();
    watch.watch(
        b"/root".to_vec(),
        b"/root".to_vec(),
        callback,
        WatchOptions {
            recursive: true,
            ignore: Some(Arc::new(|path| path.ends_with(b".ignored"))),
        },
        None,
    );
    watch.events.update(b"/root/file.ignored");
    watch.events.set_error(Error::Overflow);
    watch.trigger_callbacks();
    assert_eq!(lock(&batches)[0], (Vec::new(), Some(Error::Overflow)));
    watch.events.update(b"/root/file");
    watch.trigger_callbacks();
    assert_eq!(paths(&batches), [b"/root/file".to_vec()]);
}
#[test]
fn callback_and_ignore_panics_do_not_skip_other_callbacks() {
    let watch = DirWatch::for_test(b"/root", b"/root", true);
    watch.watch(
        b"/root".to_vec(),
        b"/root".to_vec(),
        Arc::new(|_, _| panic!("callback panic")),
        WatchOptions::default(),
        None,
    );
    watch.watch(
        b"/root".to_vec(),
        b"/root".to_vec(),
        Arc::new(|_, _| {}),
        WatchOptions {
            recursive: true,
            ignore: Some(Arc::new(|_| panic!("ignore panic"))),
        },
        None,
    );
    let healthy = register(&watch, b"/root", true);
    watch.events.update(b"/root/file");
    watch.trigger_callbacks();
    assert_eq!(paths(&healthy), [b"/root/file".to_vec()]);
}
#[test]
fn unrecoverable_errors_drain_callbacks_immediately_without_pending_events() {
    let watch = DirWatch::for_test(b"/root", b"/root", true);
    let batches = register(&watch, b"/root", true);
    watch.events.update(b"/root/file");
    watch.notify_error(Error::WatchTerminated);
    assert_eq!(
        *lock(&batches),
        [(Vec::new(), Some(Error::WatchTerminated))]
    );
    watch.trigger_callbacks();
    assert_eq!(lock(&batches).len(), 1);
    assert!(!watch.events.has_pending());
}
#[derive(Default)]
struct LifecycleBackend {
    removed: AtomicUsize,
    stopped: AtomicUsize,
}
impl Backend for LifecycleBackend {
    fn add_many(&self, _: &[Arc<DirWatch>]) -> Result<(), Error> {
        Ok(())
    }
    fn remove(&self, _: &Arc<DirWatch>) -> Result<(), Error> {
        self.removed.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }
    fn shutdown(&self) {
        self.stopped.fetch_add(1, Ordering::Relaxed);
    }
}
#[test]
fn retained_watch_keeps_owner_and_shared_subscription_until_last_close() {
    let watcher = new(Kind::Unsupported);
    let backend = Arc::new(LifecycleBackend::default());
    let dir = DirWatch::for_test(b"/root", b"/root", false);
    let (callback, batches) = callback();
    let id = dir.watch(
        b"/root".to_vec(),
        b"/root".to_vec(),
        callback,
        WatchOptions::default(),
        None,
    );
    let id2 = dir.watch(
        b"/root".to_vec(),
        b"/root".to_vec(),
        Arc::new(|_, _| {}),
        WatchOptions::default(),
        None,
    );
    *lock(&watcher.0.state) = State {
        dirs: vec![dir.clone()],
        backend: Some(backend.clone()),
        debounce: None,
    };
    let watch = Watch {
        owner: watcher.0.clone(),
        dir: dir.clone(),
        id,
        closed: AtomicBool::new(false),
    };
    let second = Watch {
        owner: watcher.0.clone(),
        dir: dir.clone(),
        id: id2,
        closed: AtomicBool::new(false),
    };
    let owner = Arc::downgrade(&watcher.0);
    drop(watcher);
    assert!(owner.upgrade().is_some());
    dir.events.update(b"/root/file");
    dir.trigger_callbacks();
    assert_eq!(paths(&batches), [b"/root/file".to_vec()]);
    watch.close().unwrap();
    watch.close().unwrap();
    assert_eq!(backend.removed.load(Ordering::Relaxed), 0);
    drop(watch);
    drop(second);
    assert_eq!(backend.removed.load(Ordering::Relaxed), 1);
    assert_eq!(backend.stopped.load(Ordering::Relaxed), 1);
    assert!(owner.upgrade().is_none());
}
