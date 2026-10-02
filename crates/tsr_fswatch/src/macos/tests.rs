use super::*;
use crate::EventKind;
use std::sync::atomic::AtomicUsize;

fn snapshot(dir: &[u8], physical: &[u8]) -> WatchSnapshot {
    WatchSnapshot {
        watch: DirWatch::for_test(dir, physical, true),
        terminated: Arc::new(AtomicBool::new(false)),
    }
}

fn recorded(entry: &WatchSnapshot) -> (Vec<crate::event::PendingEvent>, Option<Error>) {
    let (mut events, error) = entry.watch.events.drain_for_sequences(&[0]);
    let mut events = events.remove(0);
    events.sort_by(|left, right| left.event.path.cmp(&right.event.path));
    (events, error)
}

#[test]
// source: tsc/internal/fswatch/fsevents_darwin_nfd_test.go:TestNormalizeNFC
fn normalization_preserves_pinned_byte_boundaries() {
    for (input, expected) in [
        ("", ""),
        ("/var/folders/abc/hello.txt", "/var/folders/abc/hello.txt"),
        ("/\u{7f}/path", "/\u{7f}/path"),
        ("caf\u{e9}", "caf\u{e9}"),
        ("cafe\u{301}", "caf\u{e9}"),
        ("\u{d55c}", "\u{d55c}"),
        ("\u{1112}\u{1161}\u{11ab}", "\u{d55c}"),
        ("\u{1ec7}", "\u{1ec7}"),
        ("e\u{323}\u{302}", "\u{1ec7}"),
        ("/tmp/cafe\u{301}/file.txt", "/tmp/caf\u{e9}/file.txt"),
        ("/tmp/\u{1f600}.txt", "/tmp/\u{1f600}.txt"),
        ("/ascii/path\u{7f}", "/ascii/path\u{7f}"),
        ("/cafe\u{301}/file", "/caf\u{e9}/file"),
        ("/caf\u{e9}", "/caf\u{e9}"),
        ("/A\u{30a}", "/\u{c5}"),
        ("/e\u{302}\u{301}", "/\u{1ebf}"),
        ("/\u{1100}\u{1161}\u{11a8}", "/\u{ac01}"),
        ("/\u{212b}", "/\u{c5}"),
        ("/ascii\0tail", "/ascii\0tail"),
        ("/cafe\u{301}\0tail", "/caf\u{e9}"),
    ] {
        assert_eq!(
            canonicalize(input.as_bytes()),
            expected.as_bytes(),
            "{input:?}"
        );
    }
    for invalid in [b"/invalid\xff".as_slice(), b"\xff\xfe", b"/\xc3\x28"] {
        assert_eq!(canonicalize(invalid), invalid);
    }
}

#[test]
// source: tsc/internal/fswatch/fsevents_darwin_nfd_test.go:TestNormalizeNFCASCIIFastPath
fn normalize_ascii_fast_path_preserves_input() {
    let path = b"/var/folders/abc/def/hello.txt";
    assert_eq!(canonicalize(path), path);
}

#[test]
// source: tsc/internal/fswatch/fsevents_darwin_nfd_test.go:TestIsASCII
fn ascii_predicate_matches_pinned_byte_cases() {
    // Rust's byte-slice predicate replaces the pin's byte loop directly.
    for (input, expected) in [
        (b"".as_slice(), true),
        (b"hello", true),
        (b"/tmp/file.txt", true),
        (b"\x7f", true),
        (b"\x80", false),
        ("caf\u{e9}".as_bytes(), false),
        ("cafe\u{301}".as_bytes(), false),
        (b"a\xc2\xa9", false),
    ] {
        assert_eq!(input.is_ascii(), expected, "{input:?}");
    }
}

#[test]
// source: tsc/internal/fswatch/fsevents_darwin_shared_test.go:TestFSEventsOverflowMatchesWatch
fn overflow_and_display_use_component_boundaries_and_both_roots() {
    let entry = snapshot(b"/logical/root", b"/physical/root");
    for path in [
        b"/physical/root".as_slice(),
        b"/physical/root/sub",
        b"/physical",
        b"/logical/root",
        b"/logical/root/sub",
        b"/logical",
        b"/",
    ] {
        assert!(overflow_matches(&entry.watch, path), "{path:?}");
    }
    for path in [b"/other/root".as_slice(), b"/physical/root2", b""] {
        assert!(!overflow_matches(&entry.watch, path), "{path:?}");
    }
    assert_eq!(
        display_path(&entry.watch, b"/physical/root/sub"),
        Some(b"/logical/root/sub".to_vec())
    );
    assert_eq!(
        display_path(&entry.watch, b"/logical/root/sub"),
        Some(b"/logical/root/sub".to_vec())
    );
    assert_eq!(display_path(&entry.watch, b"/physical/root2"), None);
}

#[test]
fn overflow_precedes_history_done_and_keeps_error_category_and_text() {
    for (flags, prefix) in [
        (
            USER_DROPPED | KERNEL_DROPPED,
            "events were dropped by the FSEvents client",
        ),
        (KERNEL_DROPPED, "events were dropped by the kernel"),
        (0, "too many events"),
    ] {
        let entry = snapshot(b"/root/a", b"/root/a");
        process_events(
            std::slice::from_ref(&entry),
            [
                (
                    b"/root".to_vec(),
                    MUST_SCAN_SUB_DIRS | HISTORY_DONE | flags,
                    1,
                ),
                (b"/root/a/ignored".to_vec(), ITEM_CREATED, 2),
            ],
            |_| panic!("overflow must not stat paths"),
        );
        let (events, error) = recorded(&entry);
        assert!(events.is_empty());
        let error = error.unwrap();
        assert!(error.is_overflow());
        assert_eq!(error.to_string(), format!("{prefix}: {}", Error::Overflow));
    }
}

#[test]
fn ordinary_events_skip_metadata_churn_and_never_probe_existence() {
    let left = snapshot(b"/root/a", b"/root/a");
    let right = snapshot(b"/root/b", b"/root/b");
    process_events(
        &[left.clone(), right.clone()],
        [
            (b"/root/a".to_vec(), ITEM_CREATED, 1),
            (b"/root/a/ignored".to_vec(), IGNORED_FLAGS, 2),
            (b"/root/a/new.ts".to_vec(), ITEM_CREATED, 3),
            (
                b"/root/a/deleted.ts".to_vec(),
                ITEM_REMOVED | ITEM_RENAMED,
                4,
            ),
            (b"/root/b/file.ts".to_vec(), 0x1000, 5),
        ],
        |_| panic!("pure remove and update must not stat"),
    );
    let (events, error) = recorded(&left);
    assert!(error.is_none());
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].event.kind, EventKind::EventDelete);
    assert_eq!(events[0].event.path, b"/root/a/deleted.ts");
    assert_eq!(events[1].event.path, b"/root/a/new.ts");
    assert_eq!(events[1].event.kind, EventKind::EventUpdate);
    assert!(events.iter().all(|event| !event.included_watch_root));
    assert_eq!(recorded(&right).0[0].event.path, b"/root/b/file.ts");
}

#[test]
fn rename_checks_once_for_aliases_and_terminates_only_deleted_roots() {
    let left = snapshot(b"/logical/a", b"/physical/root");
    let right = snapshot(b"/logical/b", b"/physical/root");
    let mut probes = 0;
    process_events(
        &[left.clone(), right.clone()],
        [
            (
                b"/physical/root/file.ts".to_vec(),
                ITEM_CREATED | ITEM_REMOVED,
                1,
            ),
            (b"/physical/root".to_vec(), ITEM_RENAMED, 2),
        ],
        |_| {
            probes += 1;
            true
        },
    );
    assert_eq!(probes, 2);
    let (events, error) = recorded(&left);
    assert!(error.is_none());
    assert_eq!(events.len(), 2);
    assert!(events[0].included_watch_root);
    assert_eq!(events[0].event.path, b"/logical/a");
    assert!(events
        .iter()
        .all(|event| event.event.kind == EventKind::EventUpdate));
    assert!(!left.terminated.load(Ordering::Acquire));
    recorded(&right);

    process_events(
        &[left.clone(), right.clone()],
        [
            (b"/physical/root".to_vec(), ITEM_RENAMED, 3),
            (b"/physical/root/late.ts".to_vec(), ITEM_CREATED, 4),
        ],
        |_| {
            probes += 1;
            false
        },
    );
    assert_eq!(probes, 3);
    for entry in [&left, &right] {
        let (events, error) = recorded(entry);
        assert_eq!(events.len(), 1);
        assert!(events[0].included_watch_root);
        assert_eq!(events[0].event.kind, EventKind::EventDelete);
        assert!(entry.terminated.load(Ordering::Acquire));
        let error = error.unwrap();
        assert!(error.is_watch_terminated());
        assert_eq!(
            error.to_string(),
            "fswatch: watch terminated: watched directory removed"
        );
    }
}

#[test]
// source: tsc/internal/fswatch/fsevents_darwin_shared_test.go:TestFSEventsSharedStreamFallsBackToChunks
fn shared_stream_first_then_chunk_fallback_and_failure_rollback() {
    let watches: Vec<_> = (0..1025)
        .rev()
        .map(|index| {
            let path = format!("/root/dir{index:04}");
            snapshot(path.as_bytes(), path.as_bytes())
        })
        .collect();
    let mut calls = Vec::new();
    let streams = start_streams(&watches, |paths, watches| {
        calls.push((paths.len(), watches.len()));
        assert!(paths.windows(2).all(|pair| pair[0] < pair[1]));
        assert!(watches
            .iter()
            .all(|entry| paths.binary_search(&entry.watch.physical_dir).is_ok()));
        if calls.len() == 1 {
            Err(Error::Unavailable)
        } else {
            Ok(())
        }
    })
    .unwrap();
    assert_eq!(streams.len(), 3);
    assert_eq!(calls, [(1025, 1025), (512, 512), (512, 512), (1, 1)]);

    struct Started(Arc<AtomicUsize>);
    impl Drop for Started {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }
    let dropped = Arc::new(AtomicUsize::new(0));
    let mut call = 0;
    let result = start_streams(&watches, |_, _| {
        call += 1;
        if call == 1 || call == 4 {
            Err(Error::Unavailable)
        } else {
            Ok(Started(dropped.clone()))
        }
    });
    assert!(result.is_err());
    assert_eq!(dropped.load(Ordering::SeqCst), 2);
}

#[test]
// source: tsc/internal/fswatch/fsevents_darwin_shared_test.go:TestWatchesForFSEventsPaths
fn watches_for_paths_selects_only_exact_physical_roots() {
    let watches: Vec<_> = [b"/watch/a".as_slice(), b"/watch/b", b"/watch/c"]
        .into_iter()
        .map(|path| snapshot(path, path))
        .collect();
    let selected = watches_for_paths(&watches, &[b"/watch/a".to_vec(), b"/watch/c".to_vec()]);
    assert_eq!(
        selected
            .iter()
            .map(|entry| entry.watch.physical_dir.as_slice())
            .collect::<Vec<_>>(),
        [b"/watch/a".as_slice(), b"/watch/c"]
    );
    assert!(watches_for_paths(&watches, &[]).is_empty());
}

#[test]
fn duplicate_physical_paths_share_stream_and_small_failure_retries() {
    let watches = [
        snapshot(b"/logical/a", b"/physical"),
        snapshot(b"/logical/b", b"/physical"),
    ];
    let mut calls = 0;
    let streams = start_streams(&watches, |paths, watches| {
        calls += 1;
        assert_eq!(paths, [b"/physical".to_vec()]);
        assert_eq!(watches.len(), 2);
        if calls == 1 {
            Err(Error::Unavailable)
        } else {
            Ok(())
        }
    })
    .unwrap();
    assert_eq!(streams.len(), 1);
    assert_eq!(calls, 2);
    let empty: Vec<()> =
        start_streams(&[], |_, _| panic!("empty set cannot start a stream")).unwrap();
    assert!(empty.is_empty());
}

pub(super) struct TempDir(pub(super) std::path::PathBuf);
impl TempDir {
    pub(super) fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "tsr-fsevents-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).unwrap();
        Self(std::fs::canonicalize(path).unwrap())
    }
    pub(super) fn bytes(&self) -> &[u8] {
        use std::os::unix::ffi::OsStrExt;
        self.0.as_os_str().as_bytes()
    }
}
impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn native_watcher() -> (crate::Watcher, Arc<FsEventsBackend>) {
    let backend = Arc::new(FsEventsBackend::default());
    (
        crate::Watcher::with_backend_for_test(backend.clone()),
        backend,
    )
}

type NativeReceiver = std::sync::mpsc::Receiver<(Vec<crate::Event>, Option<Error>)>;
fn subscribe(
    watcher: &crate::Watcher,
    path: &std::path::Path,
    recursive: bool,
) -> (crate::Watch, NativeReceiver) {
    use std::os::unix::ffi::OsStrExt;
    let (sender, receiver) = std::sync::mpsc::channel();
    let watch = watcher
        .watch_directory(
            path.as_os_str().as_bytes(),
            Arc::new(move |events, error| {
                let _ = sender.send((events.to_vec(), error.cloned()));
            }),
            crate::WatchOptions {
                recursive,
                ignore: None,
            },
        )
        .unwrap();
    (watch, receiver)
}

fn expect_native_update(receiver: &NativeReceiver, path: &[u8]) {
    use std::time::{Duration, Instant};
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let (events, error) = receiver
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .unwrap();
        assert!(error.is_none(), "{error:?}");
        if events
            .iter()
            .any(|event| event.path == path && event.kind == EventKind::EventUpdate)
        {
            return;
        }
    }
}

fn assert_no_native_path(receiver: &NativeReceiver, path: &[u8]) {
    use std::time::{Duration, Instant};
    let deadline = Instant::now() + Duration::from_millis(500);
    while let Ok((events, error)) =
        receiver.recv_timeout(deadline.saturating_duration_since(Instant::now()))
    {
        assert!(error.is_none(), "{error:?}");
        assert!(
            events.iter().all(|event| event.path != path),
            "sibling watch received {path:?}"
        );
        if Instant::now() >= deadline {
            break;
        }
    }
}

#[test]
// source: tsc/internal/fswatch/fsevents_darwin_shared_test.go:TestFSEventsSharedStreamAcrossWatches
fn native_five_directory_subscriptions_share_one_stream() {
    let root = TempDir::new();
    let (watcher, backend) = native_watcher();
    let mut watches = Vec::new();
    for index in 0..5 {
        let dir = root.0.join(format!("dir{index}"));
        std::fs::create_dir(&dir).unwrap();
        watches.push(subscribe(&watcher, &dir, false).0);
    }
    let state = lock(&backend.0);
    assert_eq!(state.streams.len(), 1);
    assert_eq!(state.watches.len(), watches.len());
}

#[test]
// source: tsc/internal/fswatch/fsevents_darwin_shared_test.go:TestFSEventsSharedStreamRoutesEvents
fn native_shared_stream_routes_both_siblings_without_leaking_events() {
    use std::os::unix::ffi::OsStrExt;
    let root = TempDir::new();
    let (watcher, _) = native_watcher();
    let a = root.0.join("a");
    let b = root.0.join("b");
    std::fs::create_dir(&a).unwrap();
    std::fs::create_dir(&b).unwrap();
    let (_a, left) = subscribe(&watcher, &a, false);
    let (_b, right) = subscribe(&watcher, &b, false);
    let file_a = a.join("file.ts");
    let file_b = b.join("file.ts");
    std::fs::write(&file_a, b"export {}").unwrap();
    expect_native_update(&left, file_a.as_os_str().as_bytes());
    assert_no_native_path(&right, file_a.as_os_str().as_bytes());
    std::fs::write(&file_b, b"export {}").unwrap();
    expect_native_update(&right, file_b.as_os_str().as_bytes());
    assert_no_native_path(&left, file_b.as_os_str().as_bytes());
}

fn consolidated_parent(
    watcher: &crate::Watcher,
    backend: &FsEventsBackend,
    root: &TempDir,
) -> (std::path::PathBuf, Vec<crate::Watch>) {
    use std::os::unix::ffi::OsStrExt;
    let parent = root.0.join("parent");
    let mut watches = Vec::new();
    // The pinned recursiveConsolidateThreshold is ten distinct roots.
    for index in 0..10 {
        let dir = parent.join(format!("pkg{index}"));
        std::fs::create_dir_all(&dir).unwrap();
        watches.push(subscribe(watcher, &dir, false).0);
    }
    assert!(lock(&backend.0)
        .watches
        .iter()
        .any(|entry| entry.watch.recursive && entry.watch.dir == parent.as_os_str().as_bytes()));
    (parent, watches)
}

#[test]
// source: tsc/internal/fswatch/fsevents_darwin_shared_test.go:TestFSEventsConsolidatedWatchValidatesLogicalRoot
fn native_consolidated_watch_validates_missing_and_file_roots() {
    use std::os::unix::ffi::OsStrExt;
    let root = TempDir::new();
    let (watcher, backend) = native_watcher();
    let (parent, _watches) = consolidated_parent(&watcher, &backend, &root);
    let callback: crate::WatchCallback = Arc::new(|_, _| {});
    assert!(watcher
        .watch_directory(
            parent.join("missing").as_os_str().as_bytes(),
            callback.clone(),
            crate::WatchOptions::default()
        )
        .is_err());
    let file = parent.join("file");
    std::fs::write(&file, b"x").unwrap();
    assert!(watcher
        .watch_directory(
            file.as_os_str().as_bytes(),
            callback,
            crate::WatchOptions::default()
        )
        .is_err());
}

#[test]
// source: tsc/internal/fswatch/fsevents_darwin_shared_test.go:TestFSEventsConsolidatedWatchTerminatesLogicalRoot
fn native_consolidated_watch_delivers_delete_and_terminal_error_for_logical_root() {
    use std::os::unix::ffi::OsStrExt;
    use std::time::{Duration, Instant};
    let root = TempDir::new();
    let (watcher, backend) = native_watcher();
    let (parent, _watches) = consolidated_parent(&watcher, &backend, &root);
    let watched = parent.join("watched");
    std::fs::create_dir(&watched).unwrap();
    let (_watch, receiver) = subscribe(&watcher, &watched, true);
    std::thread::sleep(Duration::from_millis(100));
    while receiver.try_recv().is_ok() {}
    std::fs::remove_dir(&watched).unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut got_events = Vec::new();
    let mut terminated = false;
    while got_events.is_empty() || !terminated {
        let (events, error) = receiver
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .unwrap();
        got_events.extend(events);
        if let Some(error) = error {
            assert!(error.is_watch_terminated(), "{error:?}");
            terminated = true;
        }
    }
    assert_eq!(
        got_events,
        [crate::Event {
            kind: EventKind::EventDelete,
            path: watched.as_os_str().as_bytes().to_vec()
        }]
    );
}

#[test]
// source: tsc/internal/fswatch/fsevents_darwin_nfd_test.go:TestFSEventsNFDOnDiskNFCSubscribe
fn native_nfd_directory_subscription_reports_nfc_child_paths() {
    use std::os::unix::ffi::OsStrExt;
    use std::time::{Duration, Instant};
    let root = TempDir::new();
    let nfd = root.0.join("cafe\u{301}-dir");
    let nfc = root.0.join("caf\u{e9}-dir");
    std::fs::create_dir(&nfd).unwrap();
    let (watcher, _) = native_watcher();
    let (_watch, receiver) = subscribe(&watcher, &nfc, false);
    let child = nfc.join("hello.txt");
    std::fs::write(&child, b"hi").unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let (events, error) = receiver
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .unwrap();
        assert!(error.is_none(), "{error:?}");
        if events.is_empty() {
            continue;
        }
        assert!(
            events
                .iter()
                .all(|event| event.path == child.as_os_str().as_bytes()),
            "{events:?}"
        );
        break;
    }
}

#[test]
fn native_shared_stream_routes_to_siblings_and_rolls_back_invalid_batch() {
    use std::os::unix::ffi::OsStrExt;
    let root = TempDir::new();
    let a = root.0.join("a");
    let b = root.0.join("b");
    std::fs::create_dir(&a).unwrap();
    std::fs::create_dir(&b).unwrap();
    let left = snapshot(a.as_os_str().as_bytes(), a.as_os_str().as_bytes());
    let right = snapshot(b.as_os_str().as_bytes(), b.as_os_str().as_bytes());
    let backend = FsEventsBackend::default();
    backend
        .add_many(&[left.watch.clone(), right.watch.clone()])
        .unwrap();
    assert_eq!(lock(&backend.0).streams.len(), 1);
    assert_eq!(lock(&backend.0).watches.len(), 2);
    let missing = snapshot(b"/missing-tsr-fsevents-root", b"/missing-tsr-fsevents-root");
    assert!(backend
        .add_many(&[left.watch.clone(), missing.watch])
        .is_err());
    assert_eq!(lock(&backend.0).streams.len(), 1);
    assert_eq!(lock(&backend.0).watches.len(), 2);
    let file = a.join("file.ts");
    std::fs::write(&file, b"export {}").unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        let (events, error) = recorded(&left);
        assert!(error.is_none());
        if events
            .iter()
            .any(|event| event.event.path == file.as_os_str().as_bytes())
        {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "native FSEvents did not report file creation"
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(recorded(&right).0.is_empty());
    backend.remove(&left.watch).unwrap();
    assert_eq!(lock(&backend.0).streams.len(), 1);
    backend.remove(&right.watch).unwrap();
    assert!(lock(&backend.0).streams.is_empty());
    backend.shutdown();
    backend.shutdown();
}

#[test]
// source: tsc/internal/fswatch/fsevents_darwin_nfd_test.go:TestFSEventsNFDOnDiskNFCWatchFile
fn native_nfd_file_reaches_nfc_filter_and_close_releases_watch() {
    use std::os::unix::ffi::OsStrExt;
    use std::sync::mpsc;
    use std::time::{Duration, Instant};
    let root = TempDir::new();
    let nfd_dir = root.0.join("cafe\u{301}");
    std::fs::create_dir(&nfd_dir).unwrap();
    let nfd_file = nfd_dir.join("re\u{301}sume\u{301}.ts");
    let nfc_file = canonicalize(nfd_file.as_os_str().as_bytes());
    let (sender, receiver) = mpsc::channel();
    let watcher = native_watcher().0;
    let watch = watcher
        .watch_file(
            &nfc_file,
            Arc::new(move |events, error| {
                sender.send((events.to_vec(), error.cloned())).unwrap();
            }),
        )
        .unwrap();
    std::fs::write(&nfd_file, b"export {}").unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let (events, error) = receiver
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .unwrap();
        assert!(error.is_none(), "{error:?}");
        assert!(events.iter().all(|event| event.path == nfc_file));
        if events
            .iter()
            .any(|event| event.path == nfc_file && event.kind == EventKind::EventUpdate)
        {
            break;
        }
    }
    watch.close().unwrap();
    while receiver.try_recv().is_ok() {}
    std::fs::write(&nfd_file, b"export const changed = 1").unwrap();
    assert!(receiver.recv_timeout(Duration::from_millis(200)).is_err());
    watcher.close();
}

#[test]
fn native_deleted_root_delivers_terminal_error_with_delete() {
    use std::os::unix::ffi::OsStrExt;
    use std::sync::mpsc;
    use std::time::{Duration, Instant};
    let root = TempDir::new();
    let dir = root.0.join("watched");
    std::fs::create_dir(&dir).unwrap();
    let (sender, receiver) = mpsc::channel();
    let watcher = native_watcher().0;
    let watch = watcher
        .watch_directory(
            dir.as_os_str().as_bytes(),
            Arc::new(move |events, error| {
                sender.send((events.to_vec(), error.cloned())).unwrap();
            }),
            crate::WatchOptions {
                recursive: true,
                ignore: None,
            },
        )
        .unwrap();
    std::fs::remove_dir(&dir).unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let (events, error) = receiver
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .unwrap();
        if error.as_ref().is_some_and(Error::is_watch_terminated) {
            assert!(events
                .iter()
                .any(|event| event.path == dir.as_os_str().as_bytes()
                    && event.kind == EventKind::EventDelete));
            break;
        }
    }
    watch.close().unwrap();
}

#[test]
fn native_callback_can_close_its_own_final_subscription() {
    use std::sync::mpsc;
    use std::time::Duration;
    let root = TempDir::new();
    let slot = Arc::new(Mutex::new(None::<crate::Watch>));
    let callback_slot = slot.clone();
    let (sender, receiver) = mpsc::channel();
    let watcher = native_watcher().0;
    let watch = watcher
        .watch_directory(
            root.bytes(),
            Arc::new(move |events, error| {
                assert!(error.is_none(), "{error:?}");
                if events
                    .iter()
                    .any(|event| event.path.ends_with(b"/trigger.ts"))
                {
                    let watch = lock(&callback_slot).take().unwrap();
                    sender.send(watch.close()).unwrap();
                }
            }),
            crate::WatchOptions::default(),
        )
        .unwrap();
    *lock(&slot) = Some(watch);
    std::fs::write(root.0.join("trigger.ts"), b"export {}").unwrap();
    receiver
        .recv_timeout(Duration::from_secs(10))
        .unwrap()
        .unwrap();
    assert!(lock(&slot).is_none());
    watcher.close();
}
