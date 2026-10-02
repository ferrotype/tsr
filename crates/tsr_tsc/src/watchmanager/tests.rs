use super::*;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
fn options(sensitive: bool) -> ComparePathsOptions {
    ComparePathsOptions {
        current_directory: JsString::from_bytes(b"/repo".as_slice()),
        use_case_sensitive_file_names: sensitive,
    }
}
#[test]
// source: tsc/internal/execute/watchmanager/watchmanager_test.go:TestDirWatchSetCoverage
fn dir_watch_set_coverage() {
    let mut set = DirWatchSet::new(options(true));
    set.set(b"/repo/src", true);
    set.set(b"/repo/config", false);
    set.set(b"/repo/node_modules/a", false);
    for (dir, want) in [
        ("/repo/src", true),
        ("/repo/src/nested", true),
        ("/repo/src/nested/deep", true),
        ("/repo/config", true),
        ("/repo/config/nested", false),
        ("/repo/node_modules/a", true),
        ("/repo/node_modules/b", false),
        ("/repo", false),
        ("/other", false),
    ] {
        assert_eq!(set.covered(dir.as_bytes()), want, "{dir}");
    }
}
#[test]
// source: tsc/internal/execute/watchmanager/watchmanager_test.go:TestDirWatchSetCaseSensitive
fn dir_watch_set_case_sensitive() {
    let mut set = DirWatchSet::new(options(true));
    set.set(b"/repo/node_modules/a", false);
    set.set(b"/repo/Src", true);
    assert!(set.covered(b"/repo/node_modules/a"));
    assert!(!set.covered(b"/repo/node_modules/A"));
    assert!(set.covered(b"/repo/Src/nested"));
    assert!(!set.covered(b"/repo/src/nested"));
}
#[test]
// source: tsc/internal/execute/watchmanager/watchmanager_test.go:TestDirWatchSetCaseInsensitive
fn dir_watch_set_case_insensitive() {
    let mut set = DirWatchSet::new(options(false));
    set.set(b"/repo/node_modules/a", false);
    set.set(b"/repo/Src", true);
    assert!(set.covered(b"/repo/node_modules/A"));
    assert!(set.covered(b"/REPO/NODE_MODULES/a"));
    assert!(set.covered(b"/repo/src/nested/deep"));
}
#[test]
// source: tsc/internal/execute/watchmanager/watchmanager_test.go:TestDirWatchSetCanonicalDedup
fn dir_watch_set_canonical_dedup() {
    for (sensitive, count) in [(false, 1), (true, 2)] {
        let mut set = DirWatchSet::new(options(sensitive));
        set.set(b"/repo/Node_Modules/PkgName", false);
        set.set(b"/repo/node_modules/pkgname", false);
        assert_eq!(set.dirs().len(), count);
        assert!(set
            .dirs()
            .contains_key(b"/repo/node_modules/pkgname".as_slice()));
    }
}
#[test]
// source: tsc/internal/execute/watchmanager/watchmanager_test.go:TestDirWatchSetUpgradeToRecursive
fn dir_watch_set_upgrade_to_recursive() {
    let mut set = DirWatchSet::new(options(true));
    set.set(b"/repo/src", false);
    assert!(set.covered(b"/repo/src"));
    assert!(!set.covered(b"/repo/src/nested"));
    set.set(b"/repo/src", true);
    assert!(set.covered(b"/repo/src/nested"));
    assert_eq!(set.dirs().get(b"/repo/src".as_slice()), Some(&true));
}
#[test]
// source: tsc/internal/execute/watchmanager/watchmanager_test.go:TestDirWatchSetNeverDowngrades
fn dir_watch_set_never_downgrades() {
    let mut set = DirWatchSet::new(options(true));
    set.set(b"/repo/src", true);
    set.set(b"/repo/src", false);
    assert!(set.covered(b"/repo/src/nested"));
    assert_eq!(set.dirs().get(b"/repo/src".as_slice()), Some(&true));
}
#[test]
// source: tsc/internal/execute/watchmanager/watchmanager_test.go:TestDirWatchSetDirs
fn dir_watch_set_dirs() {
    let mut set = DirWatchSet::new(options(true));
    set.set(b"/repo/a", false);
    set.set(b"/repo/b", true);
    set.set(b"/repo/a", false);
    assert_eq!(
        *set.dirs(),
        HashMap::from([
            (JsString::from_bytes(b"/repo/a".as_slice()), false),
            (JsString::from_bytes(b"/repo/b".as_slice()), true)
        ])
    );
}
#[derive(Default)]
struct Writer(Mutex<Vec<u8>>);
impl crate::Writer for Writer {
    fn write(&self, bytes: &[u8]) -> std::io::Result<usize> {
        lock(&self.0).extend_from_slice(bytes);
        Ok(bytes.len())
    }
}
struct Close {
    closed: AtomicBool,
    count: Arc<AtomicUsize>,
}
impl Closer for Close {
    fn close(&self) -> Result<(), fswatch::Error> {
        if !self.closed.swap(true, Ordering::AcqRel) {
            self.count.fetch_add(1, Ordering::Relaxed);
        }
        Ok(())
    }
}
#[derive(Default)]
struct Backend {
    requests: Mutex<Vec<WatchDirectoryRequest>>,
    closed: Arc<AtomicUsize>,
    fail: AtomicBool,
}
impl WatchBackend for Backend {
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
        .map(|mut c| c.remove(0))
    }
    fn watch_directories(
        &self,
        requests: Vec<WatchDirectoryRequest>,
    ) -> Result<Vec<Arc<dyn Closer>>, fswatch::Error> {
        if self.fail.load(Ordering::Relaxed) {
            return Err(fswatch::Error::Unavailable);
        }
        let closers = requests
            .iter()
            .map(|_| {
                Arc::new(Close {
                    closed: AtomicBool::new(false),
                    count: self.closed.clone(),
                }) as Arc<dyn Closer>
            })
            .collect();
        lock(&self.requests).extend(requests);
        Ok(closers)
    }
}
fn fixture() -> (WatchManager, Arc<Backend>, Arc<Writer>) {
    let writer = Arc::new(Writer::default());
    let backend = Arc::new(Backend::default());
    let manager = WatchManager::new(
        writer.clone(),
        Arc::new(|path| path == b"/home/user/project/src" || path == b"/"),
    );
    manager.set_backend(backend.clone());
    (manager, backend, writer)
}
fn desired(recursive: bool) -> HashMap<JsString, bool> {
    HashMap::from([(
        JsString::from_bytes(b"/home/user/project/src".as_slice()),
        recursive,
    )])
}
#[test]
fn ancestor_fallback_is_shallow_and_merges_recursive_requests() {
    let (manager, _, _) = fixture();
    assert_eq!(
        manager.resolve_desired_dirs(&HashMap::from([(
            JsString::from_bytes(b"/home/user/project/src/missing/child".as_slice()),
            true
        )])),
        desired(false)
    );
    assert_eq!(
        manager.resolve_desired_dirs(&HashMap::from([
            (
                JsString::from_bytes(b"/home/user/project/src".as_slice()),
                true
            ),
            (
                JsString::from_bytes(b"/home/user/project/src/missing".as_slice()),
                false
            )
        ])),
        desired(true)
    );
    assert!(manager
        .resolve_desired_dirs(&HashMap::from([(
            JsString::from_bytes(b"/missing".as_slice()),
            true
        )]))
        .is_empty());
}
#[test]
fn reconciliation_reuses_identical_watches_and_recreates_recursive_changes() {
    let (manager, backend, _) = fixture();
    manager.reconcile_watches(&desired(false)).unwrap();
    manager.reconcile_watches(&desired(false)).unwrap();
    assert_eq!(lock(&backend.requests).len(), 1);
    manager.reconcile_watches(&desired(true)).unwrap();
    assert_eq!(lock(&backend.requests).len(), 2);
    assert_eq!(backend.closed.load(Ordering::Relaxed), 1);
    manager.close_all_watches();
    assert_eq!(backend.closed.load(Ordering::Relaxed), 2);
}
#[test]
fn event_accumulation_overwrites_path_kind_and_overflow_is_latched() {
    let (manager, backend, writer) = fixture();
    manager.reconcile_watches(&desired(false)).unwrap();
    let callback = lock(&backend.requests)[0].callback.clone();
    callback(
        &[fswatch::Event {
            path: b"/home/user/project/src/file".to_vec(),
            kind: fswatch::EventKind::EventUpdate,
        }],
        None,
    );
    callback(
        &[fswatch::Event {
            path: b"/home/user/project/src/file".to_vec(),
            kind: fswatch::EventKind::EventDelete,
        }],
        None,
    );
    callback(
        &[],
        Some(&fswatch::Error::Overflow.context_prefix("kernel")),
    );
    let (paths, overflow) = manager.drain_events();
    assert!(overflow);
    assert_eq!(
        paths.get(b"/home/user/project/src/file".as_slice()),
        Some(&fswatch::EventKind::EventDelete)
    );
    assert_eq!(manager.drain_events(), (HashMap::new(), false));
    callback(&[], Some(&fswatch::Error::Message("other".into())));
    assert_eq!(*lock(&writer.0), b"Warning: File watch error: other\n");
}
#[test]
fn terminated_watch_closes_once_and_next_reconcile_replaces_it() {
    let (manager, backend, _) = fixture();
    manager.reconcile_watches(&desired(false)).unwrap();
    let callback = lock(&backend.requests)[0].callback.clone();
    callback(&[], Some(&fswatch::Error::WatchTerminated));
    assert_eq!(backend.closed.load(Ordering::Relaxed), 1);
    assert!(manager.drain_events().1);
    manager.reconcile_watches(&desired(false)).unwrap();
    assert_eq!(lock(&backend.requests).len(), 2);
    callback(&[], Some(&fswatch::Error::WatchTerminated));
    assert_eq!(backend.closed.load(Ordering::Relaxed), 1);
    manager.close_all_watches();
    assert_eq!(backend.closed.load(Ordering::Relaxed), 2);
}
#[test]
fn failed_batch_is_removed_and_can_be_retried() {
    let (manager, backend, _) = fixture();
    backend.fail.store(true, Ordering::Relaxed);
    assert!(manager.reconcile_watches(&desired(false)).is_err());
    backend.fail.store(false, Ordering::Relaxed);
    manager.reconcile_watches(&desired(false)).unwrap();
    assert_eq!(lock(&backend.requests).len(), 1);
}
#[test]
fn cancellation_wakes_idle_loop_and_closes_subscriptions() {
    let (manager, backend, _) = fixture();
    manager.reconcile_watches(&desired(false)).unwrap();
    let context = tsr_ipc::Context::background().with_cancel();
    let worker_context = context.clone();
    let worker = std::thread::spawn(move || {
        manager.run_loop(&worker_context, || panic!("no filesystem event"))
    });
    context.cancel();
    worker.join().unwrap();
    assert_eq!(backend.closed.load(Ordering::Relaxed), 1);
}
#[test]
fn ignored_paths_and_os_root_heuristics_match_pin() {
    for path in [
        b"/home/user/repo/.git".as_slice(),
        b"/home/user/repo/.git/config",
        b"C:\\repo\\node_modules\\.cache\\file",
        b"/repo/.#temp",
    ] {
        assert!(should_ignore_watch_path(path));
    }
    assert!(!should_ignore_watch_path(b"/repo/.github/file"));
    for path in [
        b"/".as_slice(),
        b"/home",
        b"/home/user",
        b"C:/",
        b"C:/Users/User",
        b"/home/user/repo",
        b"/workspaces/repo",
        b"C:/repo",
        b"C:/Users/User/repo",
        b"//server/C$/repo",
    ] {
        assert!(!can_watch_directory(path), "{:?}", path);
    }
    for path in [
        b"/home/user/repo/src".as_slice(),
        b"/workspaces/repo/src",
        b"C:/repo/src",
        b"C:/Users/User/repo/src",
        b"//server/C$/repo/src",
    ] {
        assert!(can_watch_directory(path), "{:?}", path);
    }
}
