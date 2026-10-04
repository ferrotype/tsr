use super::*;
use tsr_compiler::{FileCache, Program, ProgramOptions};
use tsr_core::{CompilerOptions, Tristate};
use tsr_vfs::MemoryBuilder;
fn js(s: &str) -> JsString {
    JsString::from_bytes(s.as_bytes())
}
fn options() -> SourceFileParseOptions {
    SourceFileParseOptions {
        file_name: js("/a.js"),
        path: js("/a.js"),
        ..Default::default()
    }
}
fn acquire(
    cache: &ParseCache,
    source: &str,
    options: SourceFileParseOptions,
    kind: ScriptKind,
    counters: &Counters,
) -> CachedProgramFile {
    cache
        .acquire(
            SourceText::from_loaded_bytes(source.as_bytes()),
            kind,
            options,
            counters,
            None,
        )
        .unwrap()
}
// Pinned TestParseCacheBindsBeforePublishing, through the compiler hand-off.
#[test]
fn programs_release_cache_references_while_escaped_files_keep_syntax_alive() {
    let cache = Arc::new(ParseCache::new(RefCountCacheOptions::default()));
    let counters = Counters::new();
    let before = counters.snapshot();
    let mut fs = MemoryBuilder::new(b"/", true);
    fs.insert_loaded(b"/a.js", &b"module.exports = 0;"[..]);
    let fs = Arc::new(fs.finish());
    let load = || {
        Program::load(
            ProgramOptions {
                config: tsr_tsoptions::ParsedCommandLine::new(
                    CompilerOptions {
                        no_lib: Tristate::TRUE,
                        allow_js: Tristate::TRUE,
                        ..Default::default()
                    },
                    vec![js("/a.js")],
                ),
                host: fs.clone(),
                current_directory: js("/"),
                default_library_path: js("/"),
                skip_module_resolution: false,
                single_threaded: Tristate::TRUE,
            },
            &mut FileCache::for_project(cache.clone()),
            &counters,
        )
        .unwrap()
    };
    let first = load();
    let second = load();
    assert_eq!(cache.len(), 1);
    let file = first.files()[0].clone();
    assert!(Arc::ptr_eq(&file, &second.files()[0]));
    assert_eq!(file.source(), second.files()[0].source());
    let state = file.bound().view().source_file().unwrap();
    let key = ParseCacheKey::new(
        state.parse_options(),
        xxhash_rust::xxh3::xxh3_128(state.text().as_bytes()),
        state.script_kind,
    );
    assert_eq!(cache.reference_count(&key), Some(2));
    assert!(state.common_js_module_indicator().is_some());
    drop(first);
    assert_eq!(cache.len(), 1);
    assert_eq!(cache.reference_count(&key), Some(1));
    drop(second);
    assert!(cache.is_empty());
    assert_eq!(
        file.bound().view().source_file().unwrap().text().as_bytes(),
        b"module.exports = 0;"
    );
    drop(file);
    assert!(cache.is_empty());
    assert_eq!(counters.snapshot(), before);
}

#[test]
fn failed_and_panicking_program_loads_release_acquired_cache_references() {
    use std::sync::atomic::{AtomicBool, Ordering};
    struct Failing {
        cache: ParseCache,
        fail: AtomicBool,
        panic: bool,
    }
    impl SourceFileCache for Failing {
        fn retain(&self, file: &Arc<ProgramFile>) -> Result<Box<dyn Send + Sync>, Error> {
            self.cache.retain(file)
        }
        fn acquire(
            &self,
            source: SourceText,
            kind: ScriptKind,
            options: SourceFileParseOptions,
            counters: &Counters,
            tracing: Option<&Arc<dyn TraceSink>>,
        ) -> Result<CachedProgramFile, Error> {
            if self.cache.len() == 1 && self.fail.load(Ordering::Relaxed) {
                assert_eq!(
                    self.cache.len(),
                    1,
                    "failure must occur after one cache acquisition"
                );
                assert!(!self.panic, "injected failure during loading");
                return Err(Error::Unsupported("injected failure during loading"));
            }
            self.cache.acquire(source, kind, options, counters, tracing)
        }
    }
    for panic in [false, true] {
        let shared = Arc::new(Failing {
            cache: ParseCache::new(RefCountCacheOptions::default()),
            fail: AtomicBool::new(true),
            panic,
        });
        let counters = Counters::new();
        let baseline = counters.snapshot();
        let mut fs = MemoryBuilder::new(b"/", true);
        for name in [b"/a.ts", b"/b.ts"] {
            fs.insert_loaded(name, &b"export const x = 1"[..]);
        }
        let fs = Arc::new(fs.finish());
        let options = || ProgramOptions {
            config: tsr_tsoptions::ParsedCommandLine::new(
                CompilerOptions {
                    no_lib: Tristate::TRUE,
                    ..Default::default()
                },
                vec![js("/a.ts"), js("/b.ts")],
            ),
            host: fs.clone(),
            current_directory: js("/"),
            default_library_path: js("/"),
            skip_module_resolution: false,
            single_threaded: Tristate::TRUE,
        };
        let mut cache = FileCache::for_project(shared.clone());
        let failed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            Program::load(options(), &mut cache, &counters)
        }));
        assert!(if panic {
            failed.is_err()
        } else {
            failed.unwrap().is_err()
        });
        assert!(shared.cache.is_empty());
        assert_eq!(counters.snapshot(), baseline);
        shared.fail.store(false, Ordering::Relaxed);
        let next = Program::load(options(), &mut cache, &counters).unwrap();
        assert_eq!(shared.cache.len(), 2);
        drop(next);
        assert!(shared.cache.is_empty());
        assert_eq!(counters.snapshot(), baseline);
    }
}

#[test]
fn content_kind_path_and_module_options_invalidate_the_key() {
    let cache = ParseCache::new(RefCountCacheOptions::default());
    let counters = Counters::new();
    let first = acquire(
        &cache,
        "const x = 1;",
        options(),
        ScriptKind::UNKNOWN,
        &counters,
    );
    let same = acquire(&cache, "const x = 1;", options(), ScriptKind::JS, &counters);
    assert_eq!(first.file.source(), same.file.source());
    let mut changed_options = options();
    changed_options.external_module_indicator_options.force = true;
    let module = acquire(
        &cache,
        "const x = 1;",
        changed_options,
        ScriptKind::JS,
        &counters,
    );
    let changed = acquire(&cache, "const x = 2;", options(), ScriptKind::JS, &counters);
    let kind = acquire(&cache, "const x = 1;", options(), ScriptKind::TS, &counters);
    let mut changed_options = options();
    changed_options.path = js("/other/a.js");
    let path = acquire(
        &cache,
        "const x = 1;",
        changed_options,
        ScriptKind::JS,
        &counters,
    );
    for file in [&module, &changed, &kind, &path] {
        assert_ne!(file.file.source(), first.file.source());
    }
    assert_eq!(cache.len(), 5);
    drop((first, same, module, changed, kind, path));
    assert!(cache.is_empty());
}

#[test]
fn explicit_test_cache_retains_parse_identity_across_disposed_programs() {
    let cache = ParseCache::new(RefCountCacheOptions {
        disable_deletion: true,
    });
    let counters = Counters::new();
    let before = counters.snapshot();
    let source = acquire(&cache, "let x = 1", options(), ScriptKind::JS, &counters)
        .file
        .source();
    assert_eq!(cache.len(), 1);
    let next = acquire(&cache, "let x = 1", options(), ScriptKind::JS, &counters);
    assert_eq!(next.file.source(), source);
    let changed = acquire(&cache, "let x = 2", options(), ScriptKind::JS, &counters);
    assert_ne!(changed.file.source(), source);
    drop((next, changed, cache));
    assert_eq!(counters.snapshot(), before);
}

#[test]
fn mapped_key_separates_raw_text_transform_locale_and_parse_context() {
    let options = options();
    let make = |raw, mapper, locale| ContentMappedParseCacheKey::new(&options, raw, mapper, locale);
    let key = make(0x1020_3040_5060_7080_90a0_b0c0_d0e0_f000, 17, "en");
    assert_eq!(
        key,
        make(0x1020_3040_5060_7080_90a0_b0c0_d0e0_f000, 17, "en")
    );
    assert_ne!(key, make(18, 17, "en"));
    assert_ne!(
        key,
        make(0x1020_3040_5060_7080_90a0_b0c0_d0e0_f000, 18, "en")
    );
    assert_ne!(
        key,
        make(0x1020_3040_5060_7080_90a0_b0c0_d0e0_f000, 17, "de")
    );
    let mut changed = options.clone();
    changed.external_module_indicator_options.force = true;
    assert_ne!(
        key,
        ContentMappedParseCacheKey::new(
            &changed,
            0x1020_3040_5060_7080_90a0_b0c0_d0e0_f000,
            17,
            "en"
        )
    );
}

#[test]
fn program_reuse_retains_unchanged_files_and_speculative_fallback_separately() {
    let shared = Arc::new(ParseCache::new(RefCountCacheOptions::default()));
    let counters = Counters::new();
    let baseline = counters.snapshot();
    let host = |a: &str| {
        let mut fs = MemoryBuilder::new(b"/", true);
        fs.insert_loaded(b"/a.ts", a.as_bytes());
        fs.insert_loaded(b"/b.ts", &b"export const b = 1"[..]);
        Arc::new(fs.finish())
    };
    let options = |host: Arc<dyn tsr_vfs::FileSystem>| ProgramOptions {
        config: tsr_tsoptions::ParsedCommandLine::new(
            CompilerOptions {
                no_lib: Tristate::TRUE,
                ..Default::default()
            },
            vec![js("/a.ts"), js("/b.ts")],
        ),
        host,
        current_directory: js("/"),
        default_library_path: js("/"),
        skip_module_resolution: false,
        single_threaded: Tristate::TRUE,
    };
    let mut cache = FileCache::for_project(shared.clone());
    let first = Program::load(options(host("export const a = 1")), &mut cache, &counters).unwrap();
    let reuse = first
        .reuse_program(b"/a.ts", host("export const a = 2"), &mut cache, &counters)
        .unwrap();
    let second = reuse
        .program
        .expect("initializer-only change permits reuse");
    assert_eq!(shared.len(), 3);
    drop(reuse.file);
    drop(first);
    assert_eq!(
        shared.len(),
        2,
        "the replaced original must leave the cache"
    );
    let changed_host = host("import { b } from './b'; export const a = b;");
    let declined = second
        .reuse_program(b"/a.ts", changed_host.clone(), &mut cache, &counters)
        .unwrap();
    assert!(declined.program.is_none());
    let speculative = declined.file.as_ref().unwrap().source();
    let third = Program::load(options(changed_host), &mut cache, &counters).unwrap();
    assert!(
        third
            .files()
            .iter()
            .any(|file| file.source() == speculative),
        "fallback must reuse the speculative parse"
    );
    drop(declined);
    drop(second);
    assert_eq!(shared.len(), 2);
    let escaped = third.files()[0].clone();
    drop(third);
    assert!(shared.is_empty());
    assert!(escaped.bound().view().source_file().is_ok());
    drop(escaped);
    assert_eq!(counters.snapshot(), baseline);
}
