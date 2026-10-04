use super::*;
use tsr_vfs::{
    iovfs,
    vfstest::{self, InputFile, TestFs},
};
fn js(s: &str) -> JsString {
    JsString::from_bytes(s.as_bytes())
}
fn uri(s: &str) -> DocumentUri {
    DocumentUri::from_file_name(s.as_bytes())
}
fn setup(files: &[(&str, &str)]) -> (Arc<TestFs>, Arc<SnapshotFs>) {
    let files = files
        .iter()
        .map(|(name, text)| {
            (
                name.as_bytes().to_vec(),
                InputFile::Text(text.as_bytes().to_vec()),
            )
        })
        .collect();
    let fs = Arc::new(vfstest::from_map(&files, true));
    let snapshot = SnapshotFs::empty(Arc::new(iovfs::from(fs.clone(), true)), js("/"));
    (fs, snapshot)
}

// Pinned TestSnapshotFSBuilder: directory creation/deletion, disk identity and
// replacement. Mutate the live host between snapshots, never their handles.
#[test]
fn directories_and_old_handles_survive_edits_and_deletion() {
    let (fs, empty) = setup(&[
        ("/src/nested/deep/file.ts", "old"),
        ("/src/keep.ts", "keep"),
    ]);
    let builder = SnapshotFsBuilder::new(empty, Arc::default());
    let old = builder
        .get_file(b"/src/nested/deep/file.ts")
        .unwrap()
        .unwrap();
    let keep = builder.get_file(b"/src/keep.ts").unwrap().unwrap();
    let first = builder.finalize();
    assert_eq!(
        first.accessible_entries(b"/").directories.unwrap(),
        [js("src")]
    );
    assert_eq!(
        first.accessible_entries(b"/src/nested/deep").files.unwrap(),
        [js("file.ts")]
    );
    assert!(Arc::ptr_eq(
        &builder.get_file(b"/src/keep.ts").unwrap().unwrap(),
        &keep
    ));
    fs.write_file(b"src/nested/deep/file.ts", b"new", 0o644)
        .unwrap();
    let next = SnapshotFsBuilder::new(first.clone(), Arc::default());
    let mut change = FileChangeSummary {
        changed: BTreeSet::from([uri("/src/nested/deep/file.ts"), uri("/src/keep.ts")]),
        ..Default::default()
    };
    next.mark_dirty_files(&mut change).unwrap();
    assert_eq!(
        change.changed,
        BTreeSet::from([uri("/src/nested/deep/file.ts")])
    );
    let second = next.finalize();
    assert_eq!(old.content(), &js("old"));
    assert_eq!(
        second
            .get_file(b"/src/nested/deep/file.ts")
            .unwrap()
            .unwrap()
            .content(),
        &js("new")
    );
    assert!(Arc::ptr_eq(
        &second.get_file(b"/src/keep.ts").unwrap().unwrap(),
        &keep
    ));
    fs.remove(b"src/nested/deep/file.ts").unwrap();
    let next = SnapshotFsBuilder::new(second, Arc::default());
    let mut change = FileChangeSummary {
        deleted: BTreeSet::from([uri("/src/nested")]),
        ..Default::default()
    };
    next.filter_watch_events(&mut change, &[], &BTreeSet::new());
    assert_eq!(
        change.deleted,
        BTreeSet::from([uri("/src/nested/deep/file.ts")])
    );
    next.mark_dirty_files(&mut change).unwrap();
    let third = next.finalize();
    assert_eq!(third.cached_file_count(), 1);
    assert_eq!(third.accessible_entries(b"/src").directories, None);
    assert!(third
        .get_file(b"/src/nested/deep/file.ts")
        .unwrap()
        .is_none());
    assert_eq!(first.cached_file_count(), 2);
    assert_eq!(
        first
            .get_file(b"/src/nested/deep/file.ts")
            .unwrap()
            .unwrap()
            .content(),
        &js("old")
    );
}

#[test]
fn each_builder_refreshes_live_metadata_and_frozen_reads_memoize_misses() {
    let (fs, first) = setup(&[]);
    assert!(first.get_file(b"/later.ts").unwrap().is_none());
    let builder = SnapshotFsBuilder::new(first.clone(), Arc::default());
    assert!(!builder.file_exists(b"/later.ts").unwrap());
    let second = builder.finalize();
    fs.write_file(b"later.ts", b"created", 0o644).unwrap();
    assert!(first.get_file(b"/later.ts").unwrap().is_none());
    let next = SnapshotFsBuilder::new(second, Arc::default());
    assert!(next.file_exists(b"/later.ts").unwrap());
    assert_eq!(
        next.get_file(b"/later.ts").unwrap().unwrap().content(),
        &js("created")
    );
    let snapshot = next.finalize();
    std::thread::scope(|scope| {
        let reads: Vec<_> = (0..20)
            .map(|_| scope.spawn(|| snapshot.get_file(b"/later.ts").unwrap().unwrap()))
            .collect();
        let values: Vec<_> = reads.into_iter().map(|r| r.join().unwrap()).collect();
        assert!(values.iter().all(|value| Arc::ptr_eq(value, &values[0])));
    });
}

#[test]
fn open_and_close_compare_actual_disk_content_and_merge_overlay_directories() {
    let (fs, empty) = setup(&[("/src/a.ts", "disk")]);
    let builder = SnapshotFsBuilder::new(empty, Arc::default());
    builder.get_file(b"/src/a.ts").unwrap();
    let first = builder.finalize();
    let overlays = Arc::new(BTreeMap::from([(
        js("/src/a.ts"),
        Arc::new(FileHandle::overlay(
            js("/src/a.ts"),
            js("editor"),
            1,
            tsr_core::ScriptKind::TS,
        )),
    )]));
    let builder = SnapshotFsBuilder::new(first, overlays);
    let mut change = FileChangeSummary {
        opened: Some(uri("/src/a.ts")),
        ..Default::default()
    };
    builder.convert_open_and_close(&mut change).unwrap();
    assert_eq!(change.changed, BTreeSet::from([uri("/src/a.ts")]));
    // The pin appends overlay names; it does not deduplicate them against disk.
    assert_eq!(
        builder.entries(b"/src").unwrap().files.unwrap(),
        [js("a.ts"), js("a.ts")]
    );
    let open = builder.finalize();
    fs.write_file(b"src/a.ts", b"saved", 0o644).unwrap();
    let builder = SnapshotFsBuilder::new(open.clone(), Arc::default());
    let mut change = FileChangeSummary {
        closed: BTreeSet::from([uri("/src/a.ts")]),
        ..Default::default()
    };
    builder.convert_open_and_close(&mut change).unwrap();
    assert_eq!(change.changed, BTreeSet::from([uri("/src/a.ts")]));
    assert_eq!(
        builder.get_file(b"/src/a.ts").unwrap().unwrap().content(),
        &js("saved")
    );
    assert_eq!(
        open.get_file(b"/src/a.ts").unwrap().unwrap().content(),
        &js("editor")
    );
}

// Pinned TestRealpathAliasLifecycle and TestExpandAndFilterWatchEvents.
#[test]
fn symlink_watch_aliases_are_expanded_and_removed_with_the_last_file() {
    let (fs, empty) = setup(&[("/real/pkg/index.ts", "package")]);
    fs.add_symlink(b"node_modules/pkg", b"real/pkg");
    let builder = SnapshotFsBuilder::new(empty, Arc::default());
    assert!(builder
        .get_file(b"/node_modules/pkg/index.ts")
        .unwrap()
        .is_some());
    let first = builder.finalize();
    let mut changes = FileChangeSummary {
        changed: BTreeSet::from([uri("/real/pkg/index.ts")]),
        deleted: BTreeSet::from([uri("/real/pkg/index.ts")]),
        ..Default::default()
    };
    first.expand_realpath_aliases(&mut changes);
    assert!(changes.changed.contains(&uri("/node_modules/pkg/index.ts")));
    assert!(changes.deleted.contains(&uri("/node_modules/pkg/index.ts")));
    let builder = SnapshotFsBuilder::new(first.clone(), Arc::default());
    builder.mark_dirty_files(&mut changes).unwrap();
    let second = builder.finalize();
    assert!(second.aliases.is_empty());
    assert_eq!(first.aliases.len(), 1);
    let builder = SnapshotFsBuilder::new(second, Arc::default());
    let mut changes = FileChangeSummary {
        deleted: BTreeSet::from([uri("/node_modules/pkg"), uri("/untracked.txt")]),
        changed: BTreeSet::from([uri("/untracked.txt"), uri("/new.ts")]),
        created: BTreeSet::from([uri("/unknown")]),
        ..Default::default()
    };
    builder.filter_watch_events(&mut changes, &[], &BTreeSet::new());
    assert_eq!(changes.deleted, BTreeSet::from([uri("/node_modules/pkg")]));
    assert_eq!(changes.changed, BTreeSet::from([uri("/new.ts")]));
    assert_eq!(changes.created, BTreeSet::from([uri("/unknown")]));
}
