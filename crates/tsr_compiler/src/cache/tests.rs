use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};
use tsr_arena::Counts;
use tsr_vfs::{
    Entries, FileContent, FileInfo, FileSystem, MemoryBuilder, MemorySnapshot, SnapshotId,
};

struct Fs {
    files: MemorySnapshot,
    reads: AtomicUsize,
}
impl Fs {
    fn new(files: &[(&str, &str)]) -> Self {
        let mut builder = MemoryBuilder::new(b"/src", true);
        for (name, text) in files {
            builder.insert_physical(name.as_bytes(), text.as_bytes());
        }
        Self {
            files: builder.finish(),
            reads: AtomicUsize::new(0),
        }
    }
}
impl FileSystem for Fs {
    fn use_case_sensitive_file_names(&self) -> bool {
        true
    }
    fn snapshot_id(&self) -> Option<SnapshotId> {
        self.files.snapshot_id()
    }
    fn read_file(&self, path: &[u8]) -> Result<Option<FileContent>, tsr_vfs::Error> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        self.files.read_file(path)
    }
    fn stat(&self, path: &[u8]) -> Result<Option<FileInfo>, tsr_vfs::Error> {
        self.files.stat(path)
    }
    fn entries(&self, path: &[u8]) -> Result<Entries, tsr_vfs::Error> {
        self.files.entries(path)
    }
    fn realpath(&self, path: &[u8]) -> Result<JsString, tsr_vfs::Error> {
        self.files.realpath(path)
    }
}
fn options(name: &str) -> SourceFileParseOptions {
    SourceFileParseOptions {
        file_name: JsString::from_bytes(name.as_bytes()),
        path: JsString::from_bytes(name.as_bytes()),
        ..Default::default()
    }
}
fn load(cache: &mut FileCache, fs: &Fs, name: &str, counters: &Counters) -> Arc<ProgramFile> {
    cache
        .load(
            fs,
            ScriptKind::ensure_from_file_name(name.as_bytes()),
            options(name),
            counters,
            None,
        )
        .unwrap()
        .unwrap()
}

#[test]
fn build_cache_shares_only_declarations_and_json_within_a_cycle() {
    let fs = Fs::new(&[
        ("/src/main.ts", "export const a = 1;"),
        ("/src/main.js", "export const a = 1;"),
        ("/src/lib.d.ts", "declare const a: number;"),
        ("/src/lib.d.mts", "declare const a: number;"),
        ("/src/lib.d.cts", "declare const a: number;"),
        ("/src/data.json", "{\"a\":1}"),
    ]);
    let shared = Arc::new(SharedSourceFileCache::new());
    let mut a = FileCache::for_build(shared.clone());
    let mut b = FileCache::for_build(shared.clone());
    let counters = Counters::new();
    for (name, expected_shared) in [
        ("/src/main.ts", false),
        ("/src/main.js", false),
        ("/src/lib.d.ts", true),
        ("/src/lib.d.mts", true),
        ("/src/lib.d.cts", true),
        ("/src/data.json", true),
    ] {
        let first = load(&mut a, &fs, name, &counters);
        let second = load(&mut b, &fs, name, &counters);
        assert_eq!(Arc::ptr_eq(&first, &second), expected_shared, "{name}");
    }
    assert_eq!(fs.reads.load(Ordering::SeqCst), 8);
    assert_eq!(counters.snapshot().owners, 4);
    let retained = load(&mut a, &fs, "/src/lib.d.ts", &counters);
    shared.reset();
    assert_eq!(counters.snapshot().owners, 1);
    let fresh = load(
        &mut FileCache::for_build(shared.clone()),
        &fs,
        "/src/lib.d.ts",
        &counters,
    );
    assert!(!Arc::ptr_eq(&retained, &fresh));
    assert_eq!(
        retained
            .bound()
            .view()
            .source_file()
            .unwrap()
            .text()
            .as_bytes(),
        b"declare const a: number;"
    );
    drop((a, b, fresh, retained, shared));
    assert_eq!(counters.snapshot(), Counts::default());
}

#[test]
fn build_cache_key_includes_all_parse_options_and_script_kind() {
    let fs = Fs::new(&[
        ("/src/lib.d.ts", "declare const a: number;"),
        ("/src/alias.d.ts", "declare const a: number;"),
    ]);
    let shared = Arc::new(SharedSourceFileCache::new());
    let counters = Counters::new();
    let base = options("/src/lib.d.ts");
    let mut variants = vec![(base.clone(), ScriptKind::TS)];
    let mut renamed = base.clone();
    renamed.file_name = JsString::from_bytes(b"/src/alias.d.ts".as_slice());
    variants.push((renamed, ScriptKind::TS));
    let mut different_path = base.clone();
    different_path.path = JsString::from_bytes(b"/src/other.d.ts".as_slice());
    variants.push((different_path, ScriptKind::TS));
    let mut jsx = base.clone();
    jsx.external_module_indicator_options.jsx = true;
    variants.push((jsx, ScriptKind::TS));
    let mut force = base.clone();
    force.external_module_indicator_options.force = true;
    variants.push((force, ScriptKind::TS));
    variants.push((base, ScriptKind::JS));
    let mut files = Vec::new();
    for (options, kind) in variants {
        let mut cache = FileCache::for_build(shared.clone());
        let file = cache
            .load(&fs, kind, options.clone(), &counters, None)
            .unwrap()
            .unwrap();
        assert!(files.iter().all(|old| !Arc::ptr_eq(old, &file)));
        let repeated = cache
            .load(&fs, kind, options, &counters, None)
            .unwrap()
            .unwrap();
        assert!(Arc::ptr_eq(&file, &repeated));
        files.push(file);
    }
    assert_eq!(fs.reads.load(Ordering::SeqCst), 6);
}

#[test]
fn build_cache_has_one_winner_for_concurrent_same_file_requests() {
    let fs = Fs::new(&[("/src/lib.d.ts", "declare const a: number;")]);
    let shared = Arc::new(SharedSourceFileCache::new());
    let counters = Counters::new();
    let barrier = std::sync::Barrier::new(8);
    std::thread::scope(|scope| {
        let jobs: Vec<_> = (0..8)
            .map(|_| {
                scope.spawn(|| {
                    barrier.wait();
                    load(
                        &mut FileCache::for_build(shared.clone()),
                        &fs,
                        "/src/lib.d.ts",
                        &counters,
                    )
                })
            })
            .collect();
        let files: Vec<_> = jobs.into_iter().map(|job| job.join().unwrap()).collect();
        assert!(files.iter().all(|file| Arc::ptr_eq(file, &files[0])));
    });
    assert_eq!(fs.reads.load(Ordering::SeqCst), 1);
    assert_eq!(counters.snapshot().owners, 1);
}

#[test]
fn build_cache_retries_missing_failed_and_panicking_loads() {
    let cache = SharedSourceFileCache::new();
    let key = || ParseKey::new(&options("/src/lib.d.ts"), ScriptKind::TS);
    assert!(cache.load_or_store(key(), || Ok(None)).unwrap().is_none());
    assert!(matches!(
        cache.load_or_store(key(), || Err(Error::Unsupported("read refused"))),
        Err(Error::Unsupported("read refused"))
    ));
    assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _ = cache.load_or_store(key(), || panic!("parse failed before publication"));
    }))
    .is_err());
    let fs = Fs::new(&[("/src/lib.d.ts", "declare const a: number;")]);
    let file = load(
        &mut FileCache::new(),
        &fs,
        "/src/lib.d.ts",
        &Counters::new(),
    );
    let published = cache
        .load_or_store(key(), || Ok(Some(file.clone())))
        .unwrap()
        .unwrap();
    assert!(Arc::ptr_eq(&file, &published));
    assert!(Arc::ptr_eq(
        &file,
        &cache
            .load_or_store(key(), || panic!("already cached"))
            .unwrap()
            .unwrap()
    ));
}
