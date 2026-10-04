use super::*;
use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicU64, Ordering},
        mpsc,
    },
    time::Instant,
};
type Active = Arc<Mutex<BTreeMap<Vec<u8>, (WatchCallback, bool)>>>;
#[derive(Default)]
struct Fake {
    active: Active,
    fail: Mutex<Option<Vec<u8>>>,
    race: Mutex<Option<PathBuf>>,
}
struct Guard {
    active: Active,
    path: Vec<u8>,
}
impl Subscription for Guard {}
impl Drop for Guard {
    fn drop(&mut self) {
        self.active.lock().unwrap().remove(&self.path);
    }
}
impl Backend for Fake {
    fn watch(
        &self,
        path: &[u8],
        callback: WatchCallback,
        recursive: bool,
    ) -> Result<Box<dyn Subscription>, tsr_fswatch::Error> {
        if self.fail.lock().unwrap().as_deref() == Some(path) {
            return Err(tsr_fswatch::Error::Message("injected failure".into()));
        }
        if let Some(target) = self.race.lock().unwrap().take() {
            std::fs::create_dir_all(target).unwrap();
        }
        self.active
            .lock()
            .unwrap()
            .insert(path.to_vec(), (callback, recursive));
        Ok(Box::new(Guard {
            active: self.active.clone(),
            path: path.to_vec(),
        }))
    }
}
impl Fake {
    fn emit(&self, root: &[u8], path: &[u8], kind: EventKind, error: Option<&tsr_fswatch::Error>) {
        let callback = self.active.lock().unwrap().get(root).unwrap().0.clone();
        callback(
            &[Event {
                path: path.to_vec(),
                kind,
            }],
            error,
        );
    }
    fn watching(&self, path: &[u8]) -> bool {
        self.active.lock().unwrap().contains_key(path)
    }
}
struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "tsr-lsp-watch-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).unwrap();
        Self(std::fs::canonicalize(path).unwrap())
    }
    fn bytes(&self) -> &[u8] {
        self.0.as_os_str().as_encoded_bytes()
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn logger() -> Arc<crate::logger::Logger> {
    let ctx = Context::background();
    let client = crate::rpc_client::RpcClient::new(ctx.clone(), Arc::new(|_| Ok(())));
    Arc::new(crate::logger::Logger::new(
        client,
        ctx,
        Box::new(std::io::sink()),
    ))
}
fn pattern(path: &[u8], recursive: bool, kind: u32) -> lsp::FileSystemWatcher {
    lsp::FileSystemWatcher {
        glob_pattern: lsp::PatternOrRelativePattern {
            pattern: Some(Box::new(format!(
                "{}{}",
                String::from_utf8_lossy(path),
                if recursive { "/**/*" } else { "/*" }
            ))),
            ..Default::default()
        },
        kind: Some(Box::new(lsp::WatchKind(kind))),
    }
}
fn wait(mut predicate: impl FnMut() -> bool) {
    let until = Instant::now() + Duration::from_secs(5);
    while !predicate() {
        assert!(Instant::now() < until, "watch transition timed out");
        std::thread::sleep(Duration::from_millis(5));
    }
}
fn fixture() -> (
    Temp,
    Arc<Fake>,
    Arc<Watcher>,
    mpsc::Receiver<Vec<lsp::FileEvent>>,
) {
    let temp = Temp::new();
    let backend = Arc::new(Fake::default());
    let (send, receive) = mpsc::channel();
    let watcher = Watcher::with_backend(
        tsr_vfs::os::shared_fs(),
        backend.clone(),
        Arc::new(move |v| {
            send.send(v).unwrap();
        }),
        logger(),
    );
    (temp, backend, watcher, receive)
}
// Source: lspwatcher_test.go missing/promote/multi-level/race/terminated cases.
#[test]
fn missing_tree_promotion_termination_and_recreation() {
    let (temp, backend, watcher, receive) = fixture();
    let target = temp.0.join("a/b");
    let path = target.as_os_str().as_encoded_bytes();
    let ctx = Context::background();
    watcher
        .watch_files(&ctx, "id".into(), vec![pattern(path, true, 7)])
        .unwrap();
    assert!(backend.watching(temp.bytes()));
    std::fs::create_dir_all(&target).unwrap();
    std::fs::write(target.join("file.ts"), b"x").unwrap();
    backend.emit(temp.bytes(), path, EventKind::EventUpdate, None);
    wait(|| backend.watching(path));
    let changes = receive.recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(changes
        .iter()
        .any(|e| e.uri.file_name().as_bytes() == path && e.r#type == lsp::FileChangeType::CREATED));
    assert!(changes.iter().any(|e| e.uri.0.ends_with("/file.ts")));
    std::fs::remove_dir_all(&target).unwrap();
    backend.emit(
        path,
        path,
        EventKind::EventDelete,
        Some(&tsr_fswatch::Error::WatchTerminated),
    );
    let parent = target.parent().unwrap().as_os_str().as_encoded_bytes();
    wait(|| backend.watching(parent));
    assert!(receive
        .recv_timeout(Duration::from_secs(5))
        .unwrap()
        .iter()
        .any(|e| e.r#type == lsp::FileChangeType::DELETED));
    std::fs::create_dir(&target).unwrap();
    backend.emit(parent, path, EventKind::EventUpdate, None);
    wait(|| backend.watching(path));
    watcher.unwatch_files(&ctx, "id".into()).unwrap();
    assert!(backend.active.lock().unwrap().is_empty());
    watcher.close();
}
#[test]
fn bookkeeping_kind_filter_rollback_and_atomic_creation_race() {
    let (temp, backend, watcher, receive) = fixture();
    let ctx = Context::background();
    let root = temp.bytes();
    *backend.fail.lock().unwrap() = Some(root.to_vec());
    assert!(watcher
        .watch_files(&ctx, "id".into(), vec![pattern(root, false, 4)])
        .is_err());
    *backend.fail.lock().unwrap() = None;
    watcher
        .watch_files(&ctx, "id".into(), vec![pattern(root, false, 4)])
        .unwrap();
    assert!(!backend.active.lock().unwrap().get(root).unwrap().1);
    assert!(watcher.watch_files(&ctx, "id".into(), vec![]).is_err());
    backend.emit(
        root,
        root,
        EventKind::EventUpdate,
        Some(&tsr_fswatch::Error::Overflow),
    );
    backend.emit(root, root, EventKind::EventDelete, None);
    let changes = receive.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].r#type, lsp::FileChangeType::DELETED);
    watcher.unwatch_files(&ctx, "id".into()).unwrap();
    assert!(watcher.unwatch_files(&ctx, "id".into()).is_err());
    let target = temp.0.join("race/child");
    *backend.race.lock().unwrap() = Some(target.clone());
    let path = target.as_os_str().as_encoded_bytes();
    watcher
        .watch_files(&ctx, "race".into(), vec![pattern(path, false, 1)])
        .unwrap();
    assert!(backend.watching(path));
    assert!(!backend.watching(root));
    watcher.close();
    assert!(backend.active.lock().unwrap().is_empty());
    assert!(watcher.watch_files(&ctx, "closed".into(), vec![]).is_err());
}
#[test]
fn roots_from_pinned_globs() {
    for pattern in [
        "/abs/path/**/*",
        "/abs/path/",
        "/abs/path/?.ts",
        "/abs/path/{a,b}/*",
    ] {
        assert_eq!(root_from_glob(pattern.as_bytes()), b"/abs/path");
    }
}

#[test]
fn close_during_registration_returns_before_backend_and_retires_late_watch() {
    struct Blocking {
        entered: mpsc::Sender<()>,
        release: Mutex<mpsc::Receiver<()>>,
        disposed: mpsc::Sender<()>,
    }
    struct Dispose(mpsc::Sender<()>);
    impl Subscription for Dispose {}
    impl Drop for Dispose {
        fn drop(&mut self) {
            let _ = self.0.send(());
        }
    }
    impl Backend for Blocking {
        fn watch(
            &self,
            _: &[u8],
            _: WatchCallback,
            _: bool,
        ) -> Result<Box<dyn Subscription>, tsr_fswatch::Error> {
            self.entered.send(()).unwrap();
            self.release.lock().unwrap().recv().unwrap();
            Ok(Box::new(Dispose(self.disposed.clone())))
        }
    }
    let temp = Temp::new();
    let (entered, entering) = mpsc::channel();
    let (release, releasing) = mpsc::channel();
    let (disposed, disposal) = mpsc::channel();
    let watcher = Watcher::with_backend(
        tsr_vfs::os::shared_fs(),
        Arc::new(Blocking {
            entered,
            release: Mutex::new(releasing),
            disposed,
        }),
        Arc::new(|_| panic!("closed watcher must not publish")),
        logger(),
    );
    let registering = watcher.clone();
    let root = temp.bytes().to_vec();
    let worker = std::thread::spawn(move || {
        registering.watch_files(
            &Context::background(),
            "id".into(),
            vec![pattern(&root, true, 7)],
        )
    });
    entering.recv_timeout(Duration::from_secs(2)).unwrap();
    let closing = watcher.clone();
    let (closed, closing_done) = mpsc::channel();
    let closer = std::thread::spawn(move || {
        closing.close();
        closed.send(()).unwrap();
    });
    let closed_before_release = closing_done.recv_timeout(Duration::from_secs(2));
    release.send(()).unwrap();
    closed_before_release.expect("Close waited for an in-flight registration");
    assert!(worker.join().unwrap().is_err());
    closer.join().unwrap();
    disposal
        .recv_timeout(Duration::from_secs(2))
        .expect("late backend subscription leaked");
}

#[test]
fn synthetic_create_depth_and_reused_registration_ignore_retired_callbacks() {
    for recursive in [false, true] {
        let (temp, backend, watcher, receive) = fixture();
        let ctx = Context::background();
        let target = temp.0.join("target");
        let path = target.as_os_str().as_encoded_bytes();
        watcher
            .watch_files(&ctx, "id".into(), vec![pattern(path, recursive, 7)])
            .unwrap();
        let retired = backend
            .active
            .lock()
            .unwrap()
            .get(temp.bytes())
            .unwrap()
            .0
            .clone();
        std::fs::create_dir_all(target.join("nested")).unwrap();
        std::fs::write(target.join("shallow.ts"), "").unwrap();
        std::fs::write(target.join("nested/deep.ts"), "").unwrap();
        backend.emit(temp.bytes(), path, EventKind::EventUpdate, None);
        let events = receive.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(events.iter().any(|e| e.uri.0.ends_with("/shallow.ts")));
        assert!(events.iter().any(|e| e.uri.0.ends_with("/nested")));
        assert_eq!(
            events.iter().any(|e| e.uri.0.ends_with("/deep.ts")),
            recursive
        );
        watcher.unwatch_files(&ctx, "id".into()).unwrap();
        watcher
            .watch_files(&ctx, "id".into(), vec![pattern(path, recursive, 7)])
            .unwrap();
        retired(
            &[Event {
                path: path.to_vec(),
                kind: EventKind::EventDelete,
            }],
            None,
        );
        // A following live event also acts as an actor-order barrier. A stale
        // delete on the target must not be mixed into its resulting flush.
        let live = target.join("live.ts");
        backend.emit(
            path,
            live.as_os_str().as_encoded_bytes(),
            EventKind::EventUpdate,
            None,
        );
        let events = receive.recv_timeout(Duration::from_secs(2)).unwrap();
        assert_eq!(events.len(), 1);
        assert!(events[0].uri.0.ends_with("/live.ts"));
        watcher.close();
    }
}
#[test]
fn native_missing_directory_then_create() {
    if !tsr_fswatch::default_watcher().has_fast_recursive_backend() {
        return;
    }
    let temp = Temp::new();
    let target = temp.0.join("native");
    let path = target.as_os_str().as_encoded_bytes();
    let (send, receive) = mpsc::channel();
    let watcher = Watcher::new(
        tsr_vfs::os::shared_fs(),
        Arc::new(move |v| {
            let _ = send.send(v);
        }),
        logger(),
    );
    watcher
        .watch_files(
            &Context::background(),
            "native".into(),
            vec![pattern(path, true, 7)],
        )
        .unwrap();
    std::fs::create_dir(&target).unwrap();
    std::fs::write(target.join("new.ts"), b"let x=1").unwrap();
    let deadline = Instant::now() + Duration::from_secs(8);
    let mut found = false;
    while !found {
        let changes = receive
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .expect("native FSEvents promotion");
        found = changes.iter().any(|e| e.uri.0.ends_with("/new.ts"));
    }
    watcher.close();
}
