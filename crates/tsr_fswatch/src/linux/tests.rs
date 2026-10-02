//! Linux handler cases and live kernel tests. These must run on Linux.
use super::*;
use crate::{Event, EventKind, WatchOptions};
use std::{
    os::unix::ffi::OsStrExt,
    sync::mpsc,
    time::{Duration, Instant},
};
fn key() -> HandleKey {
    HandleKey {
        fsid: [7, 9],
        handle_type: 2,
        handle: b"fid!".to_vec(),
    }
}
fn handler(mode: Mode) -> (State, Arc<DirWatch>) {
    let (fd, _) = pipe_with(PipeFlags::CLOEXEC).unwrap();
    let watch = DirWatch::for_test(b"/watch", b"/watch", true);
    let mut state = State {
        fd: Some(Arc::new(fd)),
        mode,
        mask: 0,
        no_rename: false,
        subscriptions: HashMap::new(),
        watches: vec![watch.clone()],
    };
    let key = if mode == Mode::Inotify {
        Key::Inotify(7)
    } else {
        Key::Fanotify(key())
    };
    state.subscriptions.insert(
        key,
        vec![Subscription {
            path: b"/watch".to_vec(),
            physical: b"/watch".to_vec(),
            watch: watch.clone(),
        }],
    );
    (state, watch)
}
fn drain(watch: &DirWatch) -> (Vec<Event>, Option<Error>) {
    let (mut batches, error) = watch.events.drain_for_sequences(&[0]);
    let mut events: Vec<_> = batches
        .remove(0)
        .into_iter()
        .map(|event| event.event)
        .collect();
    events.sort_by(|a, b| a.path.cmp(&b.path));
    (events, error)
}
fn inotify_record(wd: i32, mask: u32, name: &[u8]) -> Vec<u8> {
    let mut data = Vec::new();
    data.extend_from_slice(&wd.to_ne_bytes());
    data.extend_from_slice(&mask.to_ne_bytes());
    data.extend_from_slice(&0u32.to_ne_bytes());
    data.extend_from_slice(&(name.len() as u32).to_ne_bytes());
    data.extend_from_slice(name);
    data
}
#[test]
fn inotify_raw_create_delete_overflow_and_root_loss() {
    let (mut state, watch) = handler(Mode::Inotify);
    let mut touched = Vec::new();
    let mut data = inotify_record(7, libc::IN_CREATE, b"short\0\0");
    data.extend(inotify_record(7, libc::IN_DELETE, b"short\0\0"));
    data.extend(inotify_record(7, libc::IN_MODIFY, b"updated\0"));
    data.extend(inotify_record(-1, libc::IN_Q_OVERFLOW, b""));
    state.inotify_events(&data, &mut touched).unwrap();
    assert_eq!(touched.len(), 1);
    assert_eq!(
        drain(&watch),
        (
            vec![Event {
                path: b"/watch/updated".to_vec(),
                kind: EventKind::EventUpdate
            }],
            Some(Error::Overflow)
        )
    );
    state
        .inotify_events(&inotify_record(7, libc::IN_DELETE_SELF, b""), &mut touched)
        .unwrap();
    let (events, error) = drain(&watch);
    assert_eq!(
        events,
        [Event {
            path: b"/watch".to_vec(),
            kind: EventKind::EventDelete
        }]
    );
    assert!(error.unwrap().is_watch_terminated());
    assert!(state.subscriptions.is_empty());
}
#[test]
fn inotify_rejects_truncated_record_without_reading_outside_buffer() {
    let (mut state, _) = handler(Mode::Inotify);
    let mut data = inotify_record(7, libc::IN_CREATE, b"a");
    data[12..16].copy_from_slice(&u32::MAX.to_ne_bytes());
    assert!(state.inotify_events(&data, &mut Vec::new()).is_err());
}
fn fid_record(kind: u8, name: &[u8]) -> Vec<u8> {
    let mut data = vec![kind, 0];
    data.extend_from_slice(&((25 + name.len()) as u16).to_ne_bytes());
    data.extend_from_slice(&7i32.to_ne_bytes());
    data.extend_from_slice(&9i32.to_ne_bytes());
    data.extend_from_slice(&4u32.to_ne_bytes());
    data.extend_from_slice(&2i32.to_ne_bytes());
    data.extend_from_slice(b"fid!");
    data.extend_from_slice(name);
    data.push(0);
    data
}
fn fanotify_record(mask: u64, records: &[u8]) -> Vec<u8> {
    let mut data = Vec::new();
    data.extend_from_slice(&((24 + records.len()) as u32).to_ne_bytes());
    data.extend_from_slice(&[3, 0]);
    data.extend_from_slice(&24u16.to_ne_bytes());
    data.extend_from_slice(&mask.to_ne_bytes());
    data.extend_from_slice(&(-1i32).to_ne_bytes());
    data.extend_from_slice(&0i32.to_ne_bytes());
    data.extend_from_slice(records);
    data
}
#[test]
fn fanotify_paired_rename_delivers_old_delete_and_new_update() {
    let (mut state, watch) = handler(Mode::Fanotify);
    let mut records = fid_record(10, b"old");
    records.extend(fid_record(12, b"new"));
    state
        .fanotify_events(
            &fanotify_record(libc::FAN_RENAME, &records),
            &mut Vec::new(),
        )
        .unwrap();
    let (events, error) = drain(&watch);
    assert!(error.is_none());
    assert_eq!(
        events,
        [
            Event {
                path: b"/watch/new".to_vec(),
                kind: EventKind::EventUpdate
            },
            Event {
                path: b"/watch/old".to_vec(),
                kind: EventKind::EventDelete
            }
        ]
    );
}
#[test]
fn fanotify_overflow_and_fid_only_root_loss() {
    let (mut state, watch) = handler(Mode::Fanotify);
    state
        .fanotify_events(&fanotify_record(libc::FAN_Q_OVERFLOW, &[]), &mut Vec::new())
        .unwrap();
    assert_eq!(drain(&watch), (Vec::new(), Some(Error::Overflow)));
    state
        .fanotify_events(
            &fanotify_record(libc::FAN_DELETE_SELF, &fid_record(3, b"")),
            &mut Vec::new(),
        )
        .unwrap();
    let (events, error) = drain(&watch);
    assert_eq!(events[0].path, b"/watch");
    assert_eq!(events[0].kind, EventKind::EventDelete);
    assert!(error.unwrap().is_watch_terminated());
}
#[test]
fn fanotify_merged_create_delete_of_gone_file_cancels() {
    let (mut state, watch) = handler(Mode::Fanotify);
    state
        .fanotify_events(
            &fanotify_record(libc::FAN_CREATE | libc::FAN_DELETE, &fid_record(2, b"gone")),
            &mut Vec::new(),
        )
        .unwrap();
    assert_eq!(drain(&watch), (Vec::new(), None));
}
fn receive_path(rx: &mpsc::Receiver<(Vec<Event>, Option<Error>)>, path: &[u8], kind: EventKind) {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        let (events, error) = rx
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .expect("native watcher event before deadline");
        assert!(error.is_none(), "unexpected watcher error: {error:?}");
        if events.iter().any(|e| e.path == path && e.kind == kind) {
            return;
        }
    }
}
#[test]
fn native_inotify_retained_watch_observes_create_modify_delete() {
    let temp = crate::test_support::TempDir::new();
    let root = temp.0.as_os_str().as_bytes();
    let path = temp.0.join("file");
    let (tx, rx) = mpsc::channel();
    let watch = crate::inotify()
        .watch_directory(
            root,
            Arc::new(move |events, error| {
                let _ = tx.send((events.to_vec(), error.cloned()));
            }),
            WatchOptions::default(),
        )
        .unwrap();
    std::fs::write(&path, b"a").unwrap();
    receive_path(&rx, path.as_os_str().as_bytes(), EventKind::EventUpdate);
    std::fs::write(&path, b"b").unwrap();
    receive_path(&rx, path.as_os_str().as_bytes(), EventKind::EventUpdate);
    std::fs::remove_file(&path).unwrap();
    receive_path(&rx, path.as_os_str().as_bytes(), EventKind::EventDelete);
    watch.close().unwrap();
}
#[test]
fn native_inotify_recursive_move_subscribes_descendants_without_following_symlinks() {
    let temp = crate::test_support::TempDir::new();
    let outside = crate::test_support::TempDir::new();
    std::fs::create_dir_all(outside.0.join("incoming/deep")).unwrap();
    std::fs::write(outside.0.join("incoming/deep/file"), b"a").unwrap();
    std::os::unix::fs::symlink(&outside.0, temp.0.join("link")).unwrap();
    let (tx, rx) = mpsc::channel();
    let watch = crate::inotify()
        .watch_directory(
            temp.0.as_os_str().as_bytes(),
            Arc::new(move |events, error| {
                let _ = tx.send((events.to_vec(), error.cloned()));
            }),
            WatchOptions {
                recursive: true,
                ignore: None,
            },
        )
        .unwrap();
    std::fs::rename(outside.0.join("incoming"), temp.0.join("incoming")).unwrap();
    receive_path(
        &rx,
        temp.0.join("incoming").as_os_str().as_bytes(),
        EventKind::EventUpdate,
    );
    std::fs::write(temp.0.join("incoming/deep/file"), b"b").unwrap();
    receive_path(
        &rx,
        temp.0.join("incoming/deep/file").as_os_str().as_bytes(),
        EventKind::EventUpdate,
    );
    std::fs::write(outside.0.join("hidden"), b"x").unwrap();
    assert!(rx.recv_timeout(Duration::from_millis(150)).is_err());
    watch.close().unwrap();
}
#[test]
fn native_linux_backend_releases_worker_and_descriptor_after_shutdown() {
    let temp = crate::test_support::TempDir::new();
    for _ in 0..3 {
        let backend = LinuxBackend::new(Mode::Inotify, false).unwrap();
        let fd = Arc::downgrade(lock(&backend.state).fd.as_ref().unwrap());
        let watch = DirWatch::for_test(
            temp.0.as_os_str().as_bytes(),
            temp.0.as_os_str().as_bytes(),
            true,
        );
        backend.add_many(std::slice::from_ref(&watch)).unwrap();
        backend.remove(&watch).unwrap();
        backend.shutdown();
        assert!(lock(&backend.worker).is_none());
        assert!(lock(&backend.wake).is_none());
        drop(backend);
        assert!(fd.upgrade().is_none());
    }
}

fn unstarted_fanotify() -> LinuxBackend {
    // Owned placeholder descriptor models the state before a native worker or
    // wake pipe has started; shutdown must not touch either absent resource.
    let (fd, _) = pipe_with(PipeFlags::CLOEXEC).unwrap();
    LinuxBackend {
        state: Arc::new(Mutex::new(State {
            fd: Some(Arc::new(fd)),
            mode: Mode::Fanotify,
            mask: 0,
            no_rename: false,
            subscriptions: HashMap::new(),
            watches: Vec::new(),
        })),
        wake: Mutex::new(None),
        worker: Mutex::new(None),
    }
}

#[test]
// port: tsc/internal/fswatch/fanotify_linux_test.go:TestLinuxFanotifyShutdownBeforeStart
fn fanotify_shutdown_before_start() {
    let backend = unstarted_fanotify();
    backend.shutdown();
    backend.shutdown();
    assert!(lock(&backend.worker).is_none());
    assert!(lock(&backend.wake).is_none());
}

#[test]
// port: tsc/internal/fswatch/fanotify_linux_test.go:TestLinuxFanotifyBackendSelection
fn fanotify_backend_selection() {
    if !fanotify_available() {
        eprintln!("skip: fanotify not available");
        return;
    }
    assert_eq!(crate::fanotify().name(), "fanotify");
    let backend = fanotify().expect("fanotify factory");
    backend.shutdown();
    let direct = LinuxBackend::new(Mode::Fanotify, false).unwrap();
    assert!(lock(&direct.state).mode == Mode::Fanotify);
    assert!(!lock(&direct.state).no_rename);
    direct.shutdown();
}

#[test]
// port: tsc/internal/fswatch/fanotify_linux_test.go:TestLinuxFanotifySubscribeCleansUpAfterMarkFailure
fn fanotify_subscribe_cleans_up_after_mark_failure() {
    let directory = crate::test_support::TempDir::new();
    let path = directory.0.as_os_str().as_bytes();
    let watch = DirWatch::for_test(path, path, false);
    let backend = unstarted_fanotify();
    // The placeholder descriptor cannot accept a fanotify mark, as the native
    // test's not-started backend cannot. Exercise actual mark and cleanup.
    let error = backend.add_many(std::slice::from_ref(&watch)).unwrap_err();
    assert_eq!(error.watch_directory(), Some(watch.dir.as_slice()));
    assert!(error.raw_os_error().is_some());
    assert!(lock(&backend.state).subscriptions.is_empty());
    assert!(lock(&backend.state).watches.is_empty());
}

#[test]
// port: tsc/internal/fswatch/fanotify_linux_test.go:TestLinuxFanotifyParseDfidNameRoundTrip
fn fanotify_handle_key_round_trip() {
    let directory = crate::test_support::TempDir::new();
    let path = directory.0.as_os_str().as_bytes();
    let first = match ffi::handle_key(path) {
        Ok(key) => key,
        Err(error) => {
            eprintln!("skip: name_to_handle_at not supported: {error}");
            return;
        }
    };
    assert!(!first.handle.is_empty());
    let second = ffi::handle_key(path).unwrap();
    assert_eq!(first, second);
}

#[test]
// port: tsc/internal/fswatch/fanotify_linux_test.go:TestFanotifyCrossWatcherSameFs
fn fanotify_cross_watcher_same_filesystem() {
    if !fanotify_available() {
        eprintln!("skip: fanotify not available");
        return;
    }
    let a = crate::test_support::TempDir::new();
    let b = crate::test_support::TempDir::new();
    let path_a = a.0.join("child");
    let path_b = b.0.join("child");
    std::fs::write(&path_a, b"initial").unwrap();
    std::fs::write(&path_b, b"initial").unwrap();
    let (tx_a, rx_a) = mpsc::channel();
    let (tx_b, rx_b) = mpsc::channel();
    let watcher = crate::fanotify();
    let watch_a = watcher
        .watch_directory(
            a.0.as_os_str().as_bytes(),
            Arc::new(move |events, error| {
                let _ = tx_a.send((events.to_vec(), error.cloned()));
            }),
            WatchOptions::default(),
        )
        .unwrap();
    let watch_b = watcher
        .watch_directory(
            b.0.as_os_str().as_bytes(),
            Arc::new(move |events, error| {
                let _ = tx_b.send((events.to_vec(), error.cloned()));
            }),
            WatchOptions::default(),
        )
        .unwrap();
    std::fs::write(&path_a, b"changed").unwrap();
    let deadline = Instant::now() + Duration::from_secs(3);
    let mut got = Vec::new();
    loop {
        let quiet = if got.is_empty() {
            deadline.saturating_duration_since(Instant::now())
        } else {
            Duration::from_millis(200)
        };
        match rx_a.recv_timeout(quiet) {
            Ok((events, error)) => {
                assert!(error.is_none(), "{error:?}");
                got.extend(events);
            }
            Err(mpsc::RecvTimeoutError::Timeout) => break,
            Err(error) => panic!("watcher channel disconnected: {error}"),
        }
    }
    got.sort_by(|a, b| a.path.cmp(&b.path));
    got.dedup();
    assert_eq!(
        got,
        vec![Event {
            kind: EventKind::EventUpdate,
            path: path_a.as_os_str().as_bytes().to_vec()
        }]
    );
    assert!(
        rx_b.recv_timeout(Duration::from_millis(200)).is_err(),
        "watcher B got phantom events"
    );
    watch_a.close().unwrap();
    watch_b.close().unwrap();
}

#[test]
// port: tsc/internal/fswatch/fanotify_linux_test.go:TestLinuxFanotifyMaybeWrapUnsupportedFilesystem
fn fanotify_unsupported_filesystem_keeps_tag_and_errno() {
    for errno in [rustix::io::Errno::OPNOTSUPP, rustix::io::Errno::NODEV] {
        let wrapped = ffi::unsupported(errno).context_prefix("name_to_handle_at");
        assert!(wrapped.is_filesystem_unsupported());
        assert_eq!(wrapped.raw_os_error(), Some(errno.raw_os_error()));
    }
    let other = ffi::unsupported(rustix::io::Errno::ACCESS);
    assert!(!other.is_filesystem_unsupported());
    assert_eq!(
        other.raw_os_error(),
        Some(rustix::io::Errno::ACCESS.raw_os_error())
    );
}

#[test]
// port: tsc/internal/fswatch/fanotify_linux_test.go:TestLinuxFanotifyMarkENODEVTagged
fn fanotify_mark_enodev_is_tagged() {
    let error = ffi::unsupported(rustix::io::Errno::NODEV);
    assert!(error.is_filesystem_unsupported());
    assert_eq!(
        error.raw_os_error(),
        Some(rustix::io::Errno::NODEV.raw_os_error())
    );
}

#[test]
// port: tsc/internal/fswatch/fanotify_linux_test.go:TestLinuxFanotifyUnsupportedTagSurvivesDirWatchError
fn fanotify_unsupported_tag_survives_directory_watch_error() {
    let inner = ffi::unsupported(rustix::io::Errno::OPNOTSUPP).context_prefix("name_to_handle_at");
    let backend = unstarted_fanotify();
    let watch = DirWatch::for_test(b"/x", b"/x", false);
    let error = lock(&backend.state).subscription_error(&watch, b"/x", inner);
    assert!(error.is_filesystem_unsupported());
    assert_eq!(
        error.raw_os_error(),
        Some(rustix::io::Errno::OPNOTSUPP.raw_os_error())
    );
    assert_eq!(error.watch_directory(), Some(b"/x".as_slice()));
}

#[test]
fn overflow_does_not_revive_a_deleted_native_root() {
    for mode in [Mode::Inotify, Mode::Fanotify] {
        let (mut state, watch) = handler(mode);
        match mode {
            Mode::Inotify => state
                .inotify_events(
                    &inotify_record(7, libc::IN_DELETE_SELF, b""),
                    &mut Vec::new(),
                )
                .unwrap(),
            Mode::Fanotify => state
                .fanotify_events(
                    &fanotify_record(libc::FAN_DELETE_SELF, &fid_record(3, b"")),
                    &mut Vec::new(),
                )
                .unwrap(),
        }
        assert!(drain(&watch).1.unwrap().is_watch_terminated());
        assert!(state.subscriptions.is_empty());
        let mut touched = Vec::new();
        state.overflow(&mut touched);
        assert!(touched.is_empty());
        assert_eq!(drain(&watch), (Vec::new(), None));
    }
}

#[test]
fn inotify_subscription_failure_preserves_directory_and_cause() {
    let (state, watch) = handler(Mode::Inotify);
    let error = state.subscription_error(&watch, b"/watch/child", rustix::io::Errno::ACCESS.into());
    assert_eq!(error.watch_directory(), Some(b"/watch".as_slice()));
    assert_eq!(
        error.raw_os_error(),
        Some(rustix::io::Errno::ACCESS.raw_os_error())
    );
    assert!(error
        .to_string()
        .starts_with("inotify_add_watch on '/watch/child' failed: "));
    assert!(!error.is_filesystem_unsupported());
}

#[test]
fn failed_worker_retires_descriptor_before_accepting_more_subscriptions() {
    let (reader, writer) = pipe_with(PipeFlags::NONBLOCK | PipeFlags::CLOEXEC).unwrap();
    let fd = Arc::new(reader);
    let retained = Arc::downgrade(&fd);
    let (wake_reader, _wake_writer) = pipe_with(PipeFlags::NONBLOCK | PipeFlags::CLOEXEC).unwrap();
    let state = Arc::new(Mutex::new(State {
        fd: Some(fd.clone()),
        mode: Mode::Fanotify,
        mask: 0,
        no_rename: false,
        subscriptions: HashMap::new(),
        watches: Vec::new(),
    }));
    let mut record = fanotify_record(libc::FAN_MODIFY, &fid_record(2, b"file"));
    record[4] = 0; // The native reader refuses an unsupported kernel metadata version.
    write(&writer, &record).unwrap();
    run_worker(state.clone(), fd, wake_reader);
    assert!(
        retained.upgrade().is_none(),
        "failed worker must close its native descriptor"
    );
    let backend = LinuxBackend {
        state,
        wake: Mutex::new(None),
        worker: Mutex::new(None),
    };
    let watch = DirWatch::for_test(b"/watch", b"/watch", false);
    let error = backend.add_many(&[watch]).unwrap_err();
    assert_eq!(
        error.raw_os_error(),
        Some(rustix::io::Errno::BADF.raw_os_error())
    );
    assert_eq!(error.watch_directory(), Some(b"/watch".as_slice()));
    assert!(lock(&backend.state).subscriptions.is_empty());
}
