use tsr_vfs::{Error, FileSystem, MemoryBuilder};

#[test]
fn unavailable_files_preserve_case_symlinks_metadata_and_directory_entries() {
    let mut builder = MemoryBuilder::new(b"/root", false);
    builder.insert_unavailable_file(b"/root/Dir/FILE.ts", u64::MAX);
    builder.insert_directory(b"/root/Dir/empty");
    builder.insert_symlink(b"/root/Alias", b"Dir");
    let snapshot = builder.finish();
    let alias = b"/ROOT/alias/file.ts";
    assert!(snapshot.file_exists(alias).unwrap());
    assert!(!snapshot.directory_exists(alias).unwrap());
    let info = snapshot.stat(alias).unwrap().unwrap();
    assert!(!info.directory);
    assert_eq!(info.size, u64::MAX);
    assert_eq!(
        snapshot.realpath(alias).unwrap().as_bytes(),
        b"/root/Dir/FILE.ts"
    );
    let entries = snapshot.entries(b"/ROOT/alias").unwrap();
    assert_eq!(entries.files.unwrap()[0].as_bytes(), b"FILE.ts");
    assert_eq!(entries.directories.unwrap()[0].as_bytes(), b"empty");
    let root = snapshot.entries(b"/root").unwrap();
    assert!(root
        .symlinks
        .unwrap()
        .iter()
        .any(|name| name.as_bytes() == b"Alias"));
    let Error::Detailed(error) = snapshot.read_file(alias).unwrap_err() else {
        panic!("unavailable content requires a structured error");
    };
    assert!(!error.is_not_exist());
    assert!(error.to_string().contains("/ROOT/alias/file.ts"));
    assert!(error
        .to_string()
        .contains("snapshot file content is unavailable"));
    assert!(snapshot.read_file(b"/root/missing.ts").unwrap().is_none());
}

#[test]
fn empty_files_are_readable_and_replacement_keeps_physical_decoding() {
    let mut builder = MemoryBuilder::new(b"/", true);
    builder.insert_unavailable_file(b"/empty", 0);
    builder.insert_unavailable_file(b"/source.ts", 4);
    let unavailable = builder.clone().finish();
    assert!(unavailable.read_file(b"/empty").is_err());
    builder.insert_physical(b"/empty", b"".as_slice());
    builder.insert_physical(b"/source.ts", b"\xef\xbb\xbfx".as_slice());
    let snapshot = builder.finish();
    assert!(snapshot
        .read_file(b"/empty")
        .unwrap()
        .unwrap()
        .raw
        .is_empty());
    let source = snapshot.read_file(b"/source.ts").unwrap().unwrap();
    assert_eq!(source.raw.as_ref(), b"\xef\xbb\xbfx");
    assert_eq!(source.text.as_bytes(), b"x");
    assert_eq!(snapshot.stat(b"/source.ts").unwrap().unwrap().size, 4);
    assert!(snapshot.read_file(b"/SOURCE.ts").unwrap().is_none());
    // Frozen snapshots retain their own state and cache identity.
    assert_ne!(snapshot.snapshot_id(), unavailable.snapshot_id());
    assert!(unavailable.read_file(b"/source.ts").is_err());
}

#[test]
fn directory_index_keeps_overwrite_removal_and_empty_directory_semantics() {
    let mut builder = MemoryBuilder::new(b"/", false);
    builder.insert_directory(b"/empty");
    builder.insert_physical(b"/DIR/old.ts", b"old".as_slice());
    builder.insert_physical(b"/Dir/keep.ts", b"old".as_slice());
    builder.insert_physical(b"/Dir/KEEP.ts", b"new".as_slice());
    builder.insert_physical(b"/other/deep/unrelated.ts", b"unrelated".as_slice());
    builder.remove(b"/dir/OLD.ts");
    builder.insert_symlink(b"/alias", b"Dir");
    let first = builder.finish();
    let entries = first.entries(b"/ALIAS").unwrap();
    assert_eq!(
        entries
            .files
            .unwrap()
            .iter()
            .map(tsr_jsstring::JsString::as_bytes)
            .collect::<Vec<_>>(),
        [b"KEEP.ts".as_slice()]
    );
    assert_eq!(entries.directories, None);
    let empty = first.entries(b"/empty").unwrap();
    assert_eq!(empty.files, None);
    assert_eq!(empty.directories, None);
    assert!(empty.symlinks.unwrap().is_empty());
    let mut updated = MemoryBuilder::from_snapshot(&first);
    updated.remove(b"/dir/keep.ts");
    let updated = updated.finish();
    assert_eq!(updated.entries(b"/dir").unwrap().files, None);
    assert!(first.file_exists(b"/DIR/keep.ts").unwrap());
}

#[test]
fn directory_index_defers_symlink_errors_until_queried() {
    let mut builder = MemoryBuilder::new(b"/", true);
    builder.insert_physical(b"/ok/value.ts", b"x".as_slice());
    builder.insert_symlink(b"/cycle/a", b"b");
    builder.insert_symlink(b"/cycle/b", b"a");
    builder.insert_symlink(b"/dangling", b"missing");
    let snapshot = builder.finish();
    assert_eq!(snapshot.entries(b"/ok").unwrap().files.unwrap().len(), 1);
    assert_eq!(snapshot.entries(b"/cycle"), Err(Error::SymlinkCycle));
    assert!(!snapshot
        .entries(b"/")
        .unwrap()
        .files
        .unwrap_or_default()
        .iter()
        .any(|name| name.as_bytes() == b"dangling"));
}
