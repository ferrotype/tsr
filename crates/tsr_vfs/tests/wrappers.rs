use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc, Mutex,
};
use tsr_vfs::{
    cached::CachedFs,
    recording::{Operation, RecordingFs},
    tracking::TrackingFs,
    wrapped::{Replacements, WrappedFs},
    FileContent, FileSystem, MemoryBuilder, ReadResult, WalkControl,
};
#[test]
fn callbacks_reenter_without_holding_record_or_cache_locks() {
    let calls = Arc::new(AtomicUsize::new(0));
    let slot = Arc::new(Mutex::new(None::<std::sync::Weak<CachedFs>>));
    let (calls2, slot2) = (calls.clone(), slot.clone());
    let replacements = Replacements {
        file_exists: Some(Arc::new(move |path| {
            calls2.fetch_add(1, Ordering::SeqCst);
            if path == b"outer" {
                let cache = slot2.lock().unwrap().as_ref().unwrap().upgrade().unwrap();
                cache.file_exists(b"inner")
            } else {
                Ok(false)
            }
        })),
        ..Replacements::default()
    };
    let base = Arc::new(MemoryBuilder::new(b"/", true).finish());
    let wrapped = Arc::new(WrappedFs::new(base, replacements));
    let cache = Arc::new(CachedFs::new(wrapped));
    *slot.lock().unwrap() = Some(Arc::downgrade(&cache));
    assert!(!cache.file_exists(b"outer").unwrap());
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert!(!cache.file_exists(b"outer").unwrap());
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    cache.disable_and_clear_cache();
    assert!(!cache.file_exists(b"outer").unwrap());
    assert_eq!(calls.load(Ordering::SeqCst), 4);
}
#[test]
fn wrappers_preserve_unsuccessful_read_bytes_and_retained_callbacks() {
    let mut base = MemoryBuilder::new(b"/", true);
    base.insert_loaded(b"/a", b"a".to_vec());
    let base = Arc::new(base.finish());
    let wrapped = WrappedFs::new(
        base,
        Replacements {
            read_file_result: Some(Arc::new(|_| {
                Ok(ReadResult {
                    content: FileContent::loaded(b"partial".to_vec()),
                    found: false,
                })
            })),
            ..Replacements::default()
        },
    );
    let recording = Arc::new(RecordingFs::new(Arc::new(wrapped)));
    let tracking = TrackingFs::new(recording.clone());
    let result = tracking.read_file_result(b"/missing").unwrap();
    assert!(!result.found);
    assert_eq!(result.content.raw.as_ref(), b"partial");
    assert_eq!(recording.calls(Operation::ReadFile).len(), 1);
    let calls = Arc::new(AtomicUsize::new(0));
    let counter = calls.clone();
    tracking
        .walk_dir_owned(
            b"/",
            Arc::new(move |_, _, _| {
                counter.fetch_add(1, Ordering::SeqCst);
                Ok(WalkControl::Continue)
            }),
        )
        .unwrap();
    let before = calls.load(Ordering::SeqCst);
    let retained = recording.retained_walks();
    assert_eq!(retained.len(), 1);
    (retained[0].1)(b"/later", None, None).unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), before + 1);
    assert!(tracking
        .seen_files
        .contains(&tsr_jsstring::JsString::from_bytes(b"/later".as_slice())));
}
