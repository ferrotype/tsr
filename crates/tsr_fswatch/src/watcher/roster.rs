//! Pinned watcher_test.go contracts run against every supported native backend.
use super::*;
use crate::test_support::TempDir;
use std::{
    fs,
    io::Write,
    os::unix::fs::{symlink, PermissionsExt},
    sync::Condvar,
    time::{Duration, Instant},
};
const UPDATE: EventKind = EventKind::EventUpdate;
const DELETE: EventKind = EventKind::EventDelete;
#[derive(Default)]
struct Log {
    events: Vec<Event>,
    errors: Vec<Error>,
}
#[derive(Default)]
struct Recorder(Arc<(Mutex<Log>, Condvar)>);
impl Recorder {
    fn callback(&self) -> WatchCallback {
        let state = self.0.clone();
        Arc::new(move |events, error| {
            let mut log = lock(&state.0);
            log.events.extend_from_slice(events);
            if let Some(error) = error {
                log.errors.push(error.clone());
            }
            state.1.notify_all();
        })
    }
    fn take(&self) -> Log {
        std::mem::take(&mut *lock(&self.0 .0))
    }
    fn wait(&self, predicate: impl Fn(&Log) -> bool) -> Log {
        let deadline = Instant::now() + Duration::from_secs(3);
        let mut log = lock(&self.0 .0);
        while !predicate(&log) {
            let remaining = deadline.saturating_duration_since(Instant::now());
            assert!(
                !remaining.is_zero(),
                "event deadline; events={:?}, errors={:?}",
                log.events,
                log.errors
            );
            log = self.0 .1.wait_timeout(log, remaining).unwrap().0;
        }
        std::mem::take(&mut *log)
    }
    fn expect(&self, wanted: &[(EventKind, PathBuf)]) -> Vec<Event> {
        let log = self.wait(|log| {
            wanted.iter().all(|(kind, path)| {
                log.events
                    .iter()
                    .any(|e| e.kind == *kind && e.path == bytes(path))
            })
        });
        assert!(
            log.errors.is_empty(),
            "unexpected native error: {:?}",
            log.errors
        );
        log.events
    }
    fn quiet(&self, duration: Duration) -> Log {
        let mut log = lock(&self.0 .0);
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let (next, timed) = self
                .0
                 .1
                .wait_timeout(
                    log,
                    duration.min(deadline.saturating_duration_since(Instant::now())),
                )
                .unwrap();
            log = next;
            if timed.timed_out() || Instant::now() >= deadline {
                return std::mem::take(&mut *log);
            }
        }
    }
}
fn bytes(path: &Path) -> Vec<u8> {
    path.as_os_str().as_bytes().to_vec()
}
fn mkdir(path: &Path) {
    fs::create_dir_all(path).unwrap();
}
fn write(path: &Path) {
    fs::write(path, b"content").unwrap();
}
fn run(test: impl Fn(&Watcher, &TempDir)) {
    let mut count = 0;
    for watcher in all_watchers().into_iter().filter(Watcher::available) {
        let watcher = new(watcher.0.kind); // one native lifecycle per case
        test(&watcher, &TempDir::new());
        count += 1;
    }
    #[cfg(target_os = "linux")]
    if crate::linux::fanotify_available() {
        let watcher = new(Kind::FanotifyNoRename);
        test(&watcher, &TempDir::new());
        count += 1;
    }
    assert!(count > 0, "no supported native backend available");
}
fn subscribe(watcher: &Watcher, path: &Path, recursive: bool, file: bool) -> (Recorder, Watch) {
    if watcher.name() == "fsevents" {
        std::thread::sleep(Duration::from_millis(50));
    }
    let recorder = Recorder::default();
    let watch = if file {
        watcher.watch_file(&bytes(path), recorder.callback())
    } else {
        watcher.watch_directory(
            &bytes(path),
            recorder.callback(),
            WatchOptions {
                recursive,
                ignore: None,
            },
        )
    }
    .unwrap();
    std::thread::sleep(Duration::from_millis(if watcher.name() == "fsevents" {
        300
    } else {
        60
    }));
    recorder.take();
    (recorder, watch)
}
fn no_path(events: &[Event], path: &Path) {
    assert!(
        !events.iter().any(|e| e.path == bytes(path)),
        "unexpected event for {}: {events:?}",
        path.display()
    );
}
fn simple(operation: &str, nested: bool, recursive: bool, file: bool, directory: bool) {
    run(|watcher, temp| {
        let parent = if nested {
            temp.0.join("child")
        } else {
            temp.0.clone()
        };
        if nested {
            mkdir(&parent);
        }
        let target = parent.join("target");
        let renamed = parent.join("renamed");
        if !matches!(operation, "create" | "update") {
            if directory {
                mkdir(&target);
            } else {
                write(&target);
            }
        }
        let (r, _watch) = subscribe(
            watcher,
            if file { &target } else { &temp.0 },
            recursive,
            file,
        );
        if operation == "update" {
            write(&target);
            r.expect(&[(UPDATE, target.clone())]);
        }
        match operation {
            "create" => {
                if directory {
                    mkdir(&target);
                } else {
                    write(&target);
                }
            }
            "update" => fs::write(&target, b"longer changed contents").unwrap(),
            "delete" => {
                if directory {
                    fs::remove_dir_all(&target).unwrap();
                } else {
                    fs::remove_file(&target).unwrap();
                }
            }
            "rename" => fs::rename(&target, &renamed).unwrap(),
            "truncate" => fs::OpenOptions::new()
                .write(true)
                .open(&target)
                .unwrap()
                .set_len(0)
                .unwrap(),
            "append" => fs::OpenOptions::new()
                .append(true)
                .open(&target)
                .unwrap()
                .write_all(b" appended")
                .unwrap(),
            _ => unreachable!(),
        }
        let wanted = match operation {
            "delete" => vec![(DELETE, target)],
            "rename" => vec![(DELETE, target), (UPDATE, renamed)],
            _ => vec![(UPDATE, target)],
        };
        r.expect(&wanted);
    });
}

#[test]
// source: tsc/internal/fswatch/watcher_test.go:TestWatchFileCreate
fn watch_file_create() {
    simple("create", false, true, false, false);
}

#[test]
// source: tsc/internal/fswatch/watcher_test.go:TestWatchFileUpdate
fn watch_file_update() {
    simple("update", false, true, false, false);
}

#[test]
// source: tsc/internal/fswatch/watcher_test.go:TestWatchFileRename
fn watch_file_rename() {
    simple("rename", false, true, false, false);
}

#[test]
// source: tsc/internal/fswatch/watcher_test.go:TestWatchFileRenameExisting
fn watch_file_rename_existing() {
    simple("rename", false, true, false, false);
}

#[test]
// source: tsc/internal/fswatch/watcher_test.go:TestWatchFileDelete
fn watch_file_delete() {
    simple("delete", false, true, false, false);
}

#[test]
// source: tsc/internal/fswatch/watcher_test.go:TestSubscribeDirCreate
fn subscribe_dir_create() {
    simple("create", false, true, false, true);
}

#[test]
// source: tsc/internal/fswatch/watcher_test.go:TestSubscribeDirRename
fn subscribe_dir_rename() {
    simple("rename", false, true, false, true);
}

#[test]
// source: tsc/internal/fswatch/watcher_test.go:TestSubscribeDirDelete
fn subscribe_dir_delete() {
    simple("delete", false, true, false, true);
}

#[test]
// source: tsc/internal/fswatch/watcher_test.go:TestSubscribeSubfileUpdate
fn subscribe_subfile_update() {
    simple("update", true, true, false, false);
}

#[test]
// source: tsc/internal/fswatch/watcher_test.go:TestSubscribeSubfileRename
fn subscribe_subfile_rename() {
    simple("rename", true, true, false, false);
}

#[test]
// source: tsc/internal/fswatch/watcher_test.go:TestSubscribeSubfileDelete
fn subscribe_subfile_delete() {
    simple("delete", true, true, false, false);
}

#[test]
// source: tsc/internal/fswatch/watcher_test.go:TestSubscribeSubdirCreate
fn subscribe_subdir_create() {
    simple("create", true, true, false, true);
}

#[test]
// source: tsc/internal/fswatch/watcher_test.go:TestSubscribeTruncateFile
fn subscribe_truncate_file() {
    simple("truncate", false, true, false, false);
}

#[test]
// source: tsc/internal/fswatch/watcher_test.go:TestSubscribeAppendToFile
fn subscribe_append_to_file() {
    simple("append", false, true, false, false);
}

#[test]
// source: tsc/internal/fswatch/watcher_test.go:TestNonRecursiveFileCreate
fn non_recursive_file_create() {
    simple("create", false, false, false, false);
}

#[test]
// source: tsc/internal/fswatch/watcher_test.go:TestNonRecursiveFileUpdate
fn non_recursive_file_update() {
    simple("update", false, false, false, false);
}

#[test]
// source: tsc/internal/fswatch/watcher_test.go:TestNonRecursiveFileDelete
fn non_recursive_file_delete() {
    simple("delete", false, false, false, false);
}

#[test]
// source: tsc/internal/fswatch/watcher_test.go:TestNonRecursiveDirCreate
fn non_recursive_dir_create() {
    simple("create", false, false, false, true);
}

#[test]
// source: tsc/internal/fswatch/watcher_test.go:TestFileWatchCreate
fn file_watch_create() {
    simple("create", false, false, true, false);
}

#[test]
// source: tsc/internal/fswatch/watcher_test.go:TestFileWatchUpdate
fn file_watch_update() {
    simple("update", false, false, true, false);
}

#[test]
// source: tsc/internal/fswatch/watcher_test.go:TestFileWatchDelete
fn file_watch_delete() {
    simple("delete", false, false, true, false);
}

#[test]
// source: tsc/internal/fswatch/watcher_test.go:TestFileWatchNonExistentTarget
fn file_watch_non_existent_target() {
    simple("create", false, false, true, false);
}

#[test]
// source: tsc/internal/fswatch/watcher_test.go:TestSubscribeNonASCIIPath
fn subscribe_non_ascii_path() {
    run(|w, t| {
        let dir = t.0.join("café-dir");
        mkdir(&dir);
        let (r, _s) = subscribe(w, &dir, true, false);
        let p = dir.join("résumé.txt");
        write(&p);
        r.expect(&[(UPDATE, p)]);
    });
}
#[test]
// source: tsc/internal/fswatch/watcher_test.go:TestSubscribeSubfileCreate
fn subscribe_subfile_create() {
    run(|w, t| {
        let (r, _s) = subscribe(w, &t.0, true, false);
        let dir = t.0.join("child");
        mkdir(&dir);
        r.expect(&[(UPDATE, dir.clone())]);
        std::thread::sleep(Duration::from_millis(100));
        let p = dir.join("file");
        write(&p);
        r.expect(&[(UPDATE, p)]);
    });
}
#[test]
// source: tsc/internal/fswatch/watcher_test.go:TestSubscribeSubdirDeleteWithFiles
fn subscribe_subdir_delete_with_files() {
    run(|w, t| {
        let dir = t.0.join("child");
        mkdir(&dir);
        let p = dir.join("file");
        write(&p);
        let (r, _s) = subscribe(w, &t.0, true, false);
        fs::remove_dir_all(&dir).unwrap();
        r.expect(&[(DELETE, dir), (DELETE, p)]);
    });
}
#[test]
// source: tsc/internal/fswatch/watcher_test.go:TestSubscribeWatchedDirDeleted
fn subscribe_watched_dir_deleted() {
    run(|w, t| {
        let (r, _s) = subscribe(w, &t.0, true, false);
        fs::remove_dir_all(&t.0).unwrap();
        let log = r.wait(|l| {
            l.events
                .iter()
                .any(|e| e.path == bytes(&t.0) && e.kind == DELETE)
                && l.errors.iter().any(Error::is_watch_terminated)
        });
        assert!(log.errors.iter().any(Error::is_watch_terminated));
        mkdir(&t.0);
        assert!(r.quiet(Duration::from_millis(200)).events.is_empty());
    });
}
fn symlink_case(delete: bool) {
    run(|w, t| {
        let target = t.0.join("target");
        write(&target);
        let link = t.0.join("link");
        if delete {
            symlink(&target, &link).unwrap();
        }
        let (r, _s) = subscribe(w, &t.0, true, false);
        if delete {
            fs::remove_file(&link).unwrap();
        } else {
            symlink(&target, &link).unwrap();
        }
        r.expect(&[(if delete { DELETE } else { UPDATE }, link)]);
    });
}
#[test]
// source: tsc/internal/fswatch/watcher_test.go:TestSubscribeSymlinkCreate
fn subscribe_symlink_create() {
    symlink_case(false);
}
#[test]
// source: tsc/internal/fswatch/watcher_test.go:TestSubscribeSymlinkDelete
fn subscribe_symlink_delete() {
    symlink_case(true);
}
#[test]
// source: tsc/internal/fswatch/watcher_test.go:TestSubscribeSymlinkedDirectoryRebasesTargetEvents
fn symlinked_directory_rebases_target_events() {
    run(|w, t| {
        let target = t.0.join("target");
        mkdir(&target);
        let link = t.0.join("link");
        symlink(&target, &link).unwrap();
        let (r, _s) = subscribe(w, &link, true, false);
        write(&target.join("child"));
        r.expect(&[(UPDATE, link.join("child"))]);
    });
}
#[test]
// source: tsc/internal/fswatch/watcher_test.go:TestRecursiveSubscribeSymlinkedDirectoryDoesNotFollowDescendantSymlink
fn recursive_symlink_root_does_not_follow_descendant() {
    run(|w, t| {
        if w.has_fast_recursive_backend() {
            return;
        }
        let target = t.0.join("target");
        mkdir(&target);
        let link = t.0.join("link");
        symlink(&target, &link).unwrap();
        let outside = t.0.join("outside");
        mkdir(&outside);
        symlink(&outside, target.join("nested")).unwrap();
        let (r, _s) = subscribe(w, &link, true, false);
        write(&link.join("nested/file"));
        write(&target.join("marker"));
        let mut events = r.expect(&[(UPDATE, link.join("marker"))]);
        events.extend(r.quiet(Duration::from_secs(1)).events);
        no_path(&events, &link.join("nested/file"));
        no_path(&events, &outside.join("file"));
    });
}
fn multiple(same: bool, batch: bool, file: bool) {
    run(|w, t| {
        let other = TempDir::new();
        let d2 = if same { &t.0 } else { &other.0 };
        let p1 = t.0.join("first");
        let p2 = d2.join("second");
        let r1 = Recorder::default();
        let r2 = Recorder::default();
        let _subs = if batch {
            w.watch_directories(&[
                WatchDirectoryRequest {
                    dir: bytes(&t.0),
                    callback: r1.callback(),
                    options: WatchOptions {
                        recursive: true,
                        ignore: None,
                    },
                },
                WatchDirectoryRequest {
                    dir: bytes(d2),
                    callback: r2.callback(),
                    options: WatchOptions {
                        recursive: true,
                        ignore: None,
                    },
                },
            ])
            .unwrap()
        } else if file {
            vec![
                w.watch_file(&bytes(&p1), r1.callback()).unwrap(),
                w.watch_file(&bytes(&p2), r2.callback()).unwrap(),
            ]
        } else {
            vec![
                w.watch_directory(&bytes(&t.0), r1.callback(), WatchOptions::default())
                    .unwrap(),
                w.watch_directory(&bytes(d2), r2.callback(), WatchOptions::default())
                    .unwrap(),
            ]
        };
        std::thread::sleep(Duration::from_millis(100));
        r1.take();
        r2.take();
        write(&p1);
        write(&p2);
        let e1 = r1.expect(&[(UPDATE, p1.clone())]);
        let e2 = r2.expect(&[(UPDATE, p2.clone())]);
        if !same || file {
            no_path(&e1, &p2);
            no_path(&e2, &p1);
        } else {
            assert!(
                e1.iter().any(|e| e.path == bytes(&p2))
                    || !r1
                        .wait(|l| l.events.iter().any(|e| e.path == bytes(&p2)))
                        .events
                        .is_empty()
            );
            assert!(
                e2.iter().any(|e| e.path == bytes(&p1))
                    || !r2
                        .wait(|l| l.events.iter().any(|e| e.path == bytes(&p1)))
                        .events
                        .is_empty()
            );
        }
    });
}
#[test]
// source: tsc/internal/fswatch/watcher_test.go:TestSubscribeMultipleSameDir
fn multiple_same_dir() {
    multiple(true, false, false);
}
#[test]
// source: tsc/internal/fswatch/watcher_test.go:TestSubscribeMultipleDifferentDirs
fn multiple_different_dirs() {
    multiple(false, false, false);
}
#[test]
// source: tsc/internal/fswatch/watcher_test.go:TestWatchDirectoriesBatch
fn watch_directories_batch() {
    multiple(false, true, false);
}
#[test]
// source: tsc/internal/fswatch/watcher_test.go:TestFileWatchMultipleSameDir
fn file_watch_multiple_same_dir() {
    multiple(true, false, true);
}
#[test]
// source: tsc/internal/fswatch/watcher_test.go:TestSubscribeUnsubscribeIdempotent
fn unsubscribe_idempotent() {
    run(|w, t| {
        let (_, s) = subscribe(w, &t.0, false, false);
        s.close().unwrap();
        s.close().unwrap();
    });
}
#[test]
// source: tsc/internal/fswatch/watcher_test.go:TestSubscribeCloseThenReSubscribe
fn close_then_resubscribe() {
    run(|w, t| {
        let (r1, s1) = subscribe(w, &t.0, false, false);
        s1.close().unwrap();
        let (r2, _s2) = subscribe(w, &t.0, false, false);
        let p = t.0.join("file");
        write(&p);
        r2.expect(&[(UPDATE, p)]);
        assert!(r1.quiet(Duration::from_millis(50)).events.is_empty());
    });
}
#[test]
// source: tsc/internal/fswatch/watcher_test.go:TestSubscribeNoGoroutineLeak
fn final_subscription_releases_native_and_debounce_workers() {
    run(|w, t| {
        for _ in 0..9 {
            let (_, s) = subscribe(w, &t.0, false, false);
            let (backend, debounce) = {
                let state = lock(&w.0.state);
                (
                    Arc::downgrade(state.backend.as_ref().unwrap()),
                    Arc::downgrade(state.debounce.as_ref().unwrap()),
                )
            };
            s.close().unwrap();
            assert!(backend.upgrade().is_none(), "native worker retained");
            assert!(debounce.upgrade().is_none(), "debounce worker retained");
            assert!(lock(&w.0.state).dirs.is_empty());
        }
    });
}
#[test]
// source: tsc/internal/fswatch/watcher_test.go:TestSubscribeConcurrentSubscribeUnsubscribe
fn concurrent_subscribe_unsubscribe() {
    run(|w, t| {
        std::thread::scope(|s| {
            for _ in 0..8 {
                s.spawn(|| {
                    let subscription = w
                        .watch_directory(&bytes(&t.0), Arc::new(|_, _| {}), WatchOptions::default())
                        .unwrap();
                    subscription.close().unwrap();
                });
            }
        });
        assert!(lock(&w.0.state).dirs.is_empty());
    });
}
#[test]
// source: tsc/internal/fswatch/watcher_test.go:TestSubscribeNoEventsAfterUnsubscribe
fn no_events_after_unsubscribe() {
    run(|w, t| {
        let (r, s) = subscribe(w, &t.0, true, false);
        s.close().unwrap();
        write(&t.0.join("file"));
        assert!(r.quiet(Duration::from_millis(500)).events.is_empty());
    });
}
#[test]
// source: tsc/internal/fswatch/watcher_test.go:TestSubscribeDeepNestedCreate
fn deep_nested_create() {
    run(|w, t| {
        let (r, _s) = subscribe(w, &t.0, true, false);
        let a = t.0.join("a");
        let b = a.join("b");
        let c = b.join("c");
        for path in [&a, &b, &c] {
            mkdir(path);
            std::thread::sleep(Duration::from_millis(150));
        }
        let f = c.join("deep");
        write(&f);
        r.expect(&[(UPDATE, a), (UPDATE, f)]);
    });
}
#[test]
// source: tsc/internal/fswatch/watcher_test.go:TestSubscribeManyFilesAtOnce
fn many_files_at_once() {
    run(|w, t| {
        let (r, _s) = subscribe(w, &t.0, true, false);
        let wanted: Vec<_> = (0..50)
            .map(|i| {
                let p = t.0.join(format!("file{i}"));
                write(&p);
                (UPDATE, p)
            })
            .collect();
        r.expect(&wanted);
    });
}
#[test]
// source: tsc/internal/fswatch/watcher_test.go:TestSubscribeRenameDir
fn rename_populated_dir() {
    run(|w, t| {
        let before = t.0.join("before");
        mkdir(&before);
        write(&before.join("child"));
        let (r, _s) = subscribe(w, &t.0, true, false);
        let after = t.0.join("after");
        fs::rename(&before, &after).unwrap();
        r.expect(&[(DELETE, before), (UPDATE, after)]);
    });
}
fn replace_kind(was_dir: bool) {
    run(|w, t| {
        let p = t.0.join("target");
        if was_dir {
            mkdir(&p);
        } else {
            write(&p);
        }
        let (r, _s) = subscribe(w, &t.0, true, false);
        if was_dir {
            fs::remove_dir(&p).unwrap();
            write(&p);
        } else {
            fs::remove_file(&p).unwrap();
            mkdir(&p);
        }
        r.wait(|log| log.events.iter().any(|e| e.path == bytes(&p)));
    });
}
#[test]
// source: tsc/internal/fswatch/watcher_test.go:TestSubscribeReplaceFileWithDir
fn replace_file_with_dir() {
    replace_kind(false);
}
#[test]
// source: tsc/internal/fswatch/watcher_test.go:TestReplaceDirWithFile
fn replace_dir_with_file() {
    replace_kind(true);
}
#[test]
// source: tsc/internal/fswatch/watcher_test.go:TestSubscribeNestedDirDeletionCleansDescendants
fn nested_deletion_cleans_descendants() {
    run(|w, t| {
        let p = t.0.join("parent");
        mkdir(&p.join("child"));
        write(&p.join("child/file"));
        let (r, _s) = subscribe(w, &t.0, true, false);
        fs::remove_dir_all(&p).unwrap();
        r.expect(&[(DELETE, p)]);
    });
}

fn shallow(new_child: bool, both: bool) {
    run(|w, t| {
        let child = t.0.join("child");
        if !new_child {
            mkdir(&child);
        }
        let (r, _s) = subscribe(w, &t.0, false, false);
        let recursive = both.then(|| subscribe(w, &t.0, true, false));
        if new_child {
            mkdir(&child);
            r.expect(&[(UPDATE, child.clone())]);
        }
        let grandchild = child.join("file");
        write(&grandchild);
        let marker = t.0.join("marker");
        write(&marker);
        if let Some((r, _)) = &recursive {
            r.expect(&[(UPDATE, grandchild.clone())]);
        }
        let mut events = r.expect(&[(UPDATE, marker)]);
        events.extend(r.quiet(Duration::from_secs(1)).events);
        no_path(&events, &grandchild);
    });
}
#[test]
// source: tsc/internal/fswatch/watcher_test.go:TestNonRecursiveGrandchildIgnored
fn non_recursive_grandchild_ignored() {
    shallow(false, false);
}
#[test]
// source: tsc/internal/fswatch/watcher_test.go:TestNonRecursiveNewSubdirContentIgnored
fn non_recursive_new_subdir_content_ignored() {
    shallow(true, false);
}
#[test]
// source: tsc/internal/fswatch/watcher_test.go:TestNonRecursiveAndRecursiveSameDir
fn non_recursive_and_recursive_same_dir() {
    shallow(false, true);
}
fn denied(recursive: bool) {
    run(|w, t| {
        let denied = t.0.join("denied");
        mkdir(&denied);
        fs::set_permissions(&denied, fs::Permissions::from_mode(0o0)).unwrap();
        struct Restore(PathBuf);
        impl Drop for Restore {
            fn drop(&mut self) {
                let _ = fs::set_permissions(&self.0, fs::Permissions::from_mode(0o700));
            }
        }
        let _restore = Restore(denied);
        let accessible = if recursive {
            let p = t.0.join("ok");
            mkdir(&p);
            p
        } else {
            t.0.clone()
        };
        let (r, _s) = subscribe(w, &t.0, recursive, false);
        let file = accessible.join("test");
        write(&file);
        r.expect(&[(UPDATE, file)]);
    });
}
#[test]
// source: tsc/internal/fswatch/watcher_test.go:TestNonRecursiveWithDeniedSubdir
fn non_recursive_denied_subdir() {
    denied(false);
}
#[test]
// source: tsc/internal/fswatch/watcher_test.go:TestRecursiveWithDeniedSubdir
fn recursive_denied_subdir() {
    denied(true);
}
#[test]
// source: tsc/internal/fswatch/watcher_test.go:TestFileWatchIgnoresSiblings
fn file_watch_ignores_siblings() {
    run(|w, t| {
        let target = t.0.join("target");
        let (r, _s) = subscribe(w, &target, false, true);
        write(&t.0.join("sibling"));
        write(&target);
        let mut events = r.expect(&[(UPDATE, target)]);
        events.extend(r.quiet(Duration::from_millis(300)).events);
        no_path(&events, &t.0.join("sibling"));
    });
}
#[test]
// source: tsc/internal/fswatch/watcher_test.go:TestFileWatchDeleteAndRecreate
fn file_watch_delete_and_recreate() {
    run(|w, t| {
        let p = t.0.join("file");
        write(&p);
        let (r, _s) = subscribe(w, &p, false, true);
        fs::remove_file(&p).unwrap();
        r.expect(&[(DELETE, p.clone())]);
        write(&p);
        r.expect(&[(UPDATE, p)]);
    });
}
fn atomic_save(file_watch: bool) {
    run(|w, t| {
        let p = t.0.join("target");
        write(&p);
        let (r, _s) = subscribe(w, if file_watch { &p } else { &t.0 }, true, file_watch);
        let tmp = t.0.join("target.tmp");
        write(&tmp);
        fs::rename(&tmp, &p).unwrap();
        r.wait(|l| l.events.iter().any(|e| e.path == bytes(&p)));
    });
}
#[test]
// source: tsc/internal/fswatch/watcher_test.go:TestAtomicSave
fn atomic_save_directory_watch() {
    atomic_save(false);
}
#[test]
// source: tsc/internal/fswatch/watcher_test.go:TestAtomicSaveFileWatch
fn atomic_save_file_watch() {
    atomic_save(true);
}
fn nudge(r: &Recorder, dir: &Path) {
    let deadline = Instant::now() + Duration::from_secs(4);
    let mut i = 0;
    loop {
        let p = dir.join(format!("attempt{i}"));
        write(&p);
        let log = r.quiet(Duration::from_millis(250));
        if log
            .events
            .iter()
            .any(|e| e.kind == UPDATE && is_in_directory_or_self(&bytes(dir), &e.path))
        {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "recursive watch failed to rearm: {:?}",
            log.events
        );
        i += 1;
    }
}
#[test]
// source: tsc/internal/fswatch/watcher_test.go:TestRecursiveMoveInPrePopulated
fn recursive_move_in_prepopulated() {
    run(|w, t| {
        let outside = TempDir::new();
        mkdir(&outside.0.join("a/b/c"));
        let (r, _s) = subscribe(w, &t.0, true, false);
        let dest = t.0.join("tree");
        fs::rename(&outside.0, &dest).unwrap();
        r.quiet(Duration::from_millis(500));
        nudge(&r, &dest.join("a/b/c"));
    });
}
#[test]
// source: tsc/internal/fswatch/watcher_test.go:TestRecreateSubdirAndModify
fn recreate_subdir_and_modify() {
    run(|w, t| {
        let p = t.0.join("sub");
        mkdir(&p);
        write(&p.join("file"));
        let (r, _s) = subscribe(w, &t.0, true, false);
        fs::remove_dir_all(&p).unwrap();
        r.quiet(Duration::from_millis(500));
        mkdir(&p);
        std::thread::sleep(Duration::from_millis(150));
        nudge(&r, &p);
    });
}
#[test]
// source: tsc/internal/fswatch/watcher_test.go:TestReplaceParentDirWithDifferent
fn replace_parent_with_different_tree() {
    run(|w, t| {
        let p = t.0.join("pkg");
        mkdir(&p.join("old"));
        write(&p.join("old/a"));
        let (r, _s) = subscribe(w, &t.0, true, false);
        fs::remove_dir_all(&p).unwrap();
        mkdir(&p.join("new"));
        write(&p.join("new/b"));
        r.quiet(Duration::from_millis(500));
        nudge(&r, &p.join("new"));
    });
}
#[test]
// source: tsc/internal/fswatch/watcher_test.go:TestRenameDirOutOfTreeNoStaleEvents
fn rename_out_drops_descendants() {
    run(|w, t| {
        let outside = TempDir::new();
        let sub = t.0.join("sub");
        mkdir(&sub.join("inner"));
        write(&sub.join("inner/leaf"));
        let (r, _s) = subscribe(w, &t.0, true, false);
        let dest = outside.0.join("moved");
        fs::rename(&sub, &dest).unwrap();
        r.quiet(Duration::from_millis(500));
        fs::write(dest.join("inner/leaf"), b"changed").unwrap();
        let log = r.quiet(Duration::from_millis(800));
        assert!(
            log.events
                .iter()
                .all(|e| !is_in_directory_or_self(&bytes(&sub), &e.path)),
            "stale moved subtree: {:?}",
            log.events
        );
    });
}
#[test]
// source: tsc/internal/fswatch/watcher_test.go:TestRoundTripRename
fn round_trip_rename() {
    run(|w, t| {
        let p = t.0.join("original");
        write(&p);
        let (r, _s) = subscribe(w, &t.0, true, false);
        let tmp = t.0.join("renamed");
        fs::rename(&p, &tmp).unwrap();
        fs::rename(&tmp, &p).unwrap();
        let events = r.quiet(Duration::from_millis(500)).events;
        let deleted = events
            .iter()
            .any(|e| e.path == bytes(&p) && e.kind == DELETE);
        let updated = events
            .iter()
            .any(|e| e.path == bytes(&p) && e.kind == UPDATE);
        assert!(
            !deleted || updated,
            "stale delete after round trip: {events:?}"
        );
    });
}
fn coalesce(mode: u8) {
    run(|w, t| {
        let (r, _s) = subscribe(w, &t.0, true, false);
        let p = t.0.join("file");
        let extra = t.0.join("extra");
        if matches!(mode, 1 | 3 | 4) {
            write(&p);
            r.expect(&[(UPDATE, p.clone())]);
        }
        match mode {
            0 => {
                write(&p);
                fs::write(&p, b"v2").unwrap();
            }
            1 => {
                fs::remove_file(&p).unwrap();
                write(&p);
            }
            2 => {
                write(&p);
                write(&extra);
                fs::remove_file(&extra).unwrap();
            }
            3 => {
                for value in [b"v2", b"v3", b"v4"] {
                    fs::write(&p, value).unwrap();
                }
            }
            4 => {
                fs::write(&p, b"v2").unwrap();
                fs::remove_file(&p).unwrap();
            }
            _ => unreachable!(),
        };
        let events = r.quiet(Duration::from_millis(1500)).events;
        let replay = EventList::default();
        for event in events
            .iter()
            .filter(|e| e.path == bytes(&p) || e.path == bytes(&extra))
        {
            if event.kind == UPDATE {
                replay.update(&event.path);
            } else {
                replay.remove(&event.path);
            }
        }
        let (mut batches, error) = replay.drain_for_sequences(&[0]);
        assert!(error.is_none());
        // The pin's assertEventSet filters to the wanted paths. In this case
        // the transient extra file is outside the assertion's wanted set.
        let mut events: Vec<_> = batches
            .remove(0)
            .into_iter()
            .map(|p| p.event)
            .filter(|event| event.path == bytes(&p))
            .collect();
        events.sort_by(|a, b| a.path.cmp(&b.path));
        assert_eq!(
            events,
            [Event {
                kind: if mode == 4 { DELETE } else { UPDATE },
                path: bytes(&p)
            }]
        );
    });
}
#[test]
// source: tsc/internal/fswatch/watcher_test.go:TestSubscribeCoalesceCreateUpdate
fn coalesce_create_update() {
    coalesce(0);
}
#[test]
// source: tsc/internal/fswatch/watcher_test.go:TestSubscribeCoalesceDeleteCreateAsUpdate
fn coalesce_delete_create() {
    coalesce(1);
}
#[test]
// source: tsc/internal/fswatch/watcher_test.go:TestSubscribeCoalesceCreateThenDelete
fn coalesce_create_delete() {
    coalesce(2);
}
#[test]
// source: tsc/internal/fswatch/watcher_test.go:TestSubscribeCoalesceMultipleUpdates
fn coalesce_multiple_updates() {
    coalesce(3);
}
#[test]
// source: tsc/internal/fswatch/watcher_test.go:TestSubscribeCoalesceUpdateDelete
fn coalesce_update_delete() {
    coalesce(4);
}

#[test]
// source: tsc/internal/fswatch/watcher_test.go:TestRebasePath
fn rebase_paths() {
    for (p, from, to, want) in [
        ("/from", "/from", "/to", "/to"),
        ("/from/child", "/from", "/to", "/to/child"),
        ("/from-sibling/child", "/from", "/to", "/from-sibling/child"),
        ("/child", "/", "/to", "/to/child"),
        ("/from/child", "/from", "/", "/child"),
    ] {
        assert_eq!(
            rebase_path(p.as_bytes(), from.as_bytes(), to.as_bytes()),
            want.as_bytes()
        );
    }
}
#[test]
// source: tsc/internal/fswatch/watcher_test.go:TestPhysicalDirForResolvesSymlinkAncestor
fn physical_dir_resolves_symlink_ancestor() {
    let t = TempDir::new();
    let target = t.0.join("target");
    mkdir(&target.join("nested"));
    let link = t.0.join("link");
    symlink(&target, &link).unwrap();
    assert_eq!(
        physical_dir_for(&bytes(&link.join("nested"))),
        physical_dir_for(&bytes(&target.join("nested")))
    );
}
#[test]
// source: tsc/internal/fswatch/watcher_test.go:TestIsInDirectoryOrSelf
fn directory_containment() {
    for (dir, path, want) in [
        ("/parent", "/parent", true),
        ("/parent", "/parent/child", true),
        ("/parent", "/parent/child/nested", true),
        ("/parent", "/parent-sibling", false),
        ("/", "/", true),
        ("/", "/child", true),
        ("", "/parent/child", false),
    ] {
        assert_eq!(
            is_in_directory_or_self(dir.as_bytes(), path.as_bytes()),
            want
        );
    }
    assert!(is_direct_child(b"/", b"/child"));
    assert!(!is_direct_child(b"/", b"/child/nested"));
}
#[test]
// source: tsc/internal/fswatch/watcher_test.go:TestDefaultBackendMatchesPlatform
fn default_backend_matches_platform() {
    let w = default_watcher();
    assert!(w.available());
    #[cfg(target_os = "macos")]
    assert_eq!(w.name(), "fsevents");
    #[cfg(target_os = "linux")]
    assert_eq!(
        w.name(),
        if fanotify().available() {
            "fanotify"
        } else {
            "inotify"
        }
    );
}
#[test]
// source: tsc/internal/fswatch/watcher_test.go:TestUnavailableBackendReturnsError
fn unavailable_backend_returns_error() {
    let w = all_watchers().into_iter().find(|w| !w.available()).unwrap();
    let t = TempDir::new();
    assert!(matches!(
        w.watch_directory(&bytes(&t.0), Arc::new(|_, _| {}), WatchOptions::default()),
        Err(Error::Unavailable)
    ));
}
#[test]
// source: tsc/internal/fswatch/watcher_test.go:TestSubscribeMissingDirError
fn missing_directory_error() {
    run(|w, t| {
        assert!(w
            .watch_directory(
                &bytes(&t.0.join("missing")),
                Arc::new(|_, _| {}),
                WatchOptions::default()
            )
            .is_err());
    });
}
#[test]
// source: tsc/internal/fswatch/watcher_test.go:TestSubscribeNotADirError
fn not_directory_error() {
    run(|w, t| {
        let p = t.0.join("file");
        write(&p);
        assert!(w
            .watch_directory(&bytes(&p), Arc::new(|_, _| {}), WatchOptions::default())
            .is_err());
    });
}
#[test]
// source: tsc/internal/fswatch/watcher_test.go:TestSubscribeRejectsRelativePath
fn relative_paths_rejected() {
    run(|w, _| {
        assert!(w
            .watch_directory(
                b"relative/path",
                Arc::new(|_, _| {}),
                WatchOptions::default()
            )
            .is_err());
        assert!(w
            .watch_file(b"relative/path/file", Arc::new(|_, _| {}))
            .is_err());
    });
}
#[test]
// source: tsc/internal/fswatch/watcher_test.go:TestBackendRunReturnsStartError
fn backend_start_error_propagates() {
    let mut w = new(Kind::Fsevents);
    Arc::get_mut(&mut w.0).unwrap().startup_for_test =
        Some(|| Err(Error::Message("startup failed".into())));
    let t = TempDir::new();
    let error = w
        .watch_directory(&bytes(&t.0), Arc::new(|_, _| {}), WatchOptions::default())
        .err()
        .unwrap();
    assert_eq!(error, Error::Message("startup failed".into()));
    assert!(lock(&w.0.state).dirs.is_empty());
}
#[test]
// source: tsc/internal/fswatch/watcher_test.go:TestDirWatchErrorImplementsError
fn directory_error_implements_error() {
    let error: Box<dyn std::error::Error> = Box::new(Error::Message("boom".into()));
    assert_eq!(error.to_string(), "boom");
}
#[test]
// source: tsc/internal/fswatch/watcher_test.go:TestFileCallbackForwardsErrAlongsideEvents
fn file_callback_preserves_events_and_errors() {
    let r = Recorder::default();
    let p = b"/abs/dir/target".to_vec();
    let other = b"/abs/dir/sibling".to_vec();
    let cb = file_callback(p.clone(), r.callback());
    cb(
        &[
            Event {
                kind: UPDATE,
                path: p.clone(),
            },
            Event {
                kind: UPDATE,
                path: other.clone(),
            },
        ],
        None,
    );
    assert_eq!(
        r.take().events,
        [Event {
            kind: UPDATE,
            path: p.clone()
        }]
    );
    cb(
        &[Event {
            kind: UPDATE,
            path: other.clone(),
        }],
        Some(&Error::Overflow),
    );
    let log = r.take();
    assert!(log.events.is_empty());
    assert_eq!(log.errors, [Error::Overflow]);
    cb(
        &[
            Event {
                kind: DELETE,
                path: p.clone(),
            },
            Event {
                kind: UPDATE,
                path: other,
            },
        ],
        Some(&Error::Overflow),
    );
    let log = r.take();
    assert_eq!(
        log.events,
        [Event {
            kind: DELETE,
            path: p
        }]
    );
    assert_eq!(log.errors, [Error::Overflow]);
    cb(&[], None);
    let log = r.take();
    assert!(log.events.is_empty() && log.errors.is_empty());
}
#[derive(Default)]
struct Counting {
    added: Mutex<Vec<Arc<DirWatch>>>,
    closed: Mutex<Vec<Arc<DirWatch>>>,
}
impl Backend for Counting {
    fn add_many(&self, watches: &[Arc<DirWatch>]) -> Result<(), Error> {
        lock(&self.added).extend_from_slice(watches);
        Ok(())
    }
    fn remove(&self, watch: &Arc<DirWatch>) -> Result<(), Error> {
        lock(&self.closed).push(watch.clone());
        Ok(())
    }
    fn shutdown(&self) {}
}
fn consolidated(outside: bool) {
    let t = TempDir::new();
    let parent = t.0.join("node_modules/.bun");
    mkdir(&parent);
    let backend = Arc::new(Counting::default());
    let w = Watcher::with_backend_for_test(backend.clone());
    let mut subs = Vec::new();
    for i in 0..if outside { 10 } else { 12 } {
        let path = parent.join(format!("pkg{i}"));
        mkdir(&path);
        subs.extend(
            w.register_directories(&[WatchDirectoryRequest {
                dir: bytes(&path),
                callback: Arc::new(|_, _| {}),
                options: WatchOptions::default(),
            }])
            .unwrap(),
        );
    }
    let added = lock(&backend.added);
    assert_eq!(added.len(), 10);
    assert_eq!(added[9].dir, bytes(&parent));
    assert!(added[9].recursive);
    let consolidated = added[9].clone();
    drop(added);
    if outside {
        let target = t.0.join("outside");
        mkdir(&target);
        let link = parent.join("linked");
        symlink(&target, &link).unwrap();
        let extra = w
            .register_directories(&[WatchDirectoryRequest {
                dir: bytes(&link),
                callback: Arc::new(|_, _| {}),
                options: WatchOptions::default(),
            }])
            .unwrap();
        assert!(!Arc::ptr_eq(&extra[0].dir, &consolidated));
        assert_eq!(extra[0].dir.dir, bytes(&link));
        assert_eq!(extra[0].dir.physical_dir, physical_dir_for(&bytes(&link)));
    } else {
        assert!(Arc::ptr_eq(&subs[11].dir, &consolidated));
        assert!(!lock(&w.0.state)
            .dirs
            .iter()
            .any(|d| d.dir == bytes(&parent.join("pkg11"))));
    }
}
#[test]
// source: tsc/internal/fswatch/watcher_test.go:TestFastRecursiveWatcherConsolidatesSiblingDirectories
fn recursive_backend_consolidates_siblings() {
    consolidated(false);
}
#[test]
// source: tsc/internal/fswatch/watcher_test.go:TestFastRecursiveWatcherDoesNotConsolidateSymlinkOutsideRoot
fn recursive_backend_does_not_consolidate_outside_symlink() {
    consolidated(true);
}
fn direct(
    dir: &[u8],
    physical: &[u8],
    logical: &[u8],
    logical_physical: &[u8],
    recursive: bool,
) -> (Arc<DirWatch>, Recorder) {
    let watch = DirWatch::for_test(dir, physical, true);
    let r = Recorder::default();
    watch.watch(
        logical.to_vec(),
        logical_physical.to_vec(),
        r.callback(),
        WatchOptions {
            recursive,
            ignore: None,
        },
        None,
    );
    (watch, r)
}
#[test]
// source: tsc/internal/fswatch/watcher_test.go:TestConsolidatedSymlinkChildMapsSharedLogicalPath
fn consolidated_symlink_maps_shared_path() {
    let (w, r) = direct(
        b"/logical",
        b"/physical",
        b"/logical/link",
        b"/physical/target",
        true,
    );
    w.events.update(b"/logical/target/file.ts");
    w.trigger_callbacks();
    assert_eq!(
        r.take().events,
        [Event {
            kind: UPDATE,
            path: b"/logical/link/file.ts".to_vec()
        }]
    );
}
#[test]
// source: tsc/internal/fswatch/watcher_test.go:TestConsolidatedSymlinkChildTerminatesFromSharedLogicalPath
fn consolidated_symlink_terminates_shared_path() {
    let (w, r) = direct(
        b"/logical",
        b"/physical",
        b"/logical/link",
        b"/physical/target",
        true,
    );
    assert!(w.terminate_callbacks_for_deleted_root(b"/logical/target", 1, Error::WatchTerminated));
    w.trigger_callbacks();
    assert_eq!(r.take().errors, [Error::WatchTerminated]);
}
#[test]
// source: tsc/internal/fswatch/watcher_test.go:TestConsolidatedChildWatchFiltersAgainstRequestedDir
fn consolidated_child_filters_requested_dir() {
    let (w, r) = direct(
        b"/parent",
        b"/parent",
        b"/parent/child",
        b"/parent/child",
        false,
    );
    w.events.update_watch_root_at(b"/parent/child", 1);
    for p in [
        b"/parent/child/file.ts".as_slice(),
        b"/parent/child/nested/file.ts",
        b"/parent/sibling/file.ts",
    ] {
        w.events.update(p);
    }
    w.trigger_callbacks();
    let mut events = r.take().events;
    events.sort_by(|a, b| a.path.cmp(&b.path));
    assert_eq!(
        events,
        [
            Event {
                kind: UPDATE,
                path: b"/parent/child".to_vec()
            },
            Event {
                kind: UPDATE,
                path: b"/parent/child/file.ts".to_vec()
            }
        ]
    );
}
#[test]
// source: tsc/internal/fswatch/watcher_test.go:TestConsolidatedChildWatchIgnoresEventsBeforeSubscribe
fn consolidated_child_ignores_earlier_events() {
    let w = DirWatch::for_test(b"/parent", b"/parent", true);
    w.events.update(b"/parent/child");
    let r = Recorder::default();
    w.watch(
        b"/parent/child".to_vec(),
        b"/parent/child".to_vec(),
        r.callback(),
        WatchOptions {
            recursive: true,
            ignore: None,
        },
        None,
    );
    w.events.remove(b"/parent/child");
    w.trigger_callbacks();
    assert_eq!(
        r.take().events,
        [Event {
            kind: DELETE,
            path: b"/parent/child".to_vec()
        }]
    );
}
#[test]
// source: tsc/internal/fswatch/watcher_test.go:TestRecursiveWatchWithIgnoreDoesNotFilterByLogicalRoot
fn recursive_ignore_does_not_filter_logical_root() {
    let w = DirWatch::for_test(b"/root", b"/root", true);
    let r = Recorder::default();
    w.watch(
        b"/root".to_vec(),
        b"/root".to_vec(),
        r.callback(),
        WatchOptions {
            recursive: true,
            ignore: Some(Arc::new(|_| false)),
        },
        None,
    );
    w.events.update(b"/outside/pkg/index.ts");
    w.trigger_callbacks();
    assert_eq!(
        r.take().events,
        [Event {
            kind: UPDATE,
            path: b"/outside/pkg/index.ts".to_vec()
        }]
    );
}
#[test]
// source: tsc/internal/fswatch/fallback_test.go:TestFanotifyUsesInternalInotifyFallback
fn fanotify_uses_shared_internal_inotify() {
    let primary = fanotify();
    assert!(primary.0.kind == Kind::Fanotify);
    let secondary = primary.fallback_watcher().unwrap();
    assert!(Arc::ptr_eq(&secondary.0, &inotify().0));
    assert!(inotify().fallback_watcher().is_none());
}

#[test]
fn close_finishes_cleanup_and_returns_success_after_native_teardown_failure() {
    struct FailRemove;
    impl Backend for FailRemove {
        fn add_many(&self, _: &[Arc<DirWatch>]) -> Result<(), Error> {
            Ok(())
        }
        fn remove(&self, _: &Arc<DirWatch>) -> Result<(), Error> {
            Err(Error::Message("remove failed".into()))
        }
        fn shutdown(&self) {}
    }
    let watcher = Watcher::with_backend_for_test(Arc::new(FailRemove));
    let temp = TempDir::new();
    let watches = watcher
        .register_directories(&[WatchDirectoryRequest {
            dir: bytes(&temp.0),
            callback: Arc::new(|_, _| {}),
            options: WatchOptions::default(),
        }])
        .unwrap();
    assert_eq!(watches[0].close(), Ok(()));
    assert_eq!(watches[0].close(), Ok(()));
    let state = lock(&watcher.0.state);
    assert!(state.dirs.is_empty());
    assert!(state.backend.is_none());
    assert!(state.debounce.is_none());
}
