use super::*;
use std::sync::atomic::AtomicI64;
use std::time::Duration;
use tsr_contentmapper::{SpawnError, Spawner};
use tsr_ipc::Stream;
use tsr_vfs::{
    Entries, FileContent, FileInfo, FileSystem, MemoryBuilder, MemorySnapshot, SnapshotId,
};

type ReadObserver = Arc<dyn Fn(&[u8]) + Send + Sync>;

struct Fs {
    files: Mutex<MemoryBuilder>,
    times: Mutex<HashMap<Vec<u8>, Time>>,
    writes: Mutex<Vec<JsString>>,
    panic_read: Mutex<Option<Vec<u8>>>,
    read_observer: Mutex<Option<ReadObserver>>,
    touches: Mutex<Vec<JsString>>,
    clock: Arc<AtomicI64>,
}
impl Fs {
    fn new(clock: Arc<AtomicI64>, files: &[(&str, &str)]) -> Self {
        let mut builder = MemoryBuilder::new(b"/work", true);
        let mut times = HashMap::new();
        for (path, content) in files {
            builder.insert_loaded(path.as_bytes(), content.as_bytes());
            times.insert(path.as_bytes().to_vec(), Time::from_unix(100, 0));
        }
        Self {
            files: Mutex::new(builder),
            times: Mutex::new(times),
            writes: Mutex::default(),
            panic_read: Mutex::default(),
            read_observer: Mutex::default(),
            touches: Mutex::default(),
            clock,
        }
    }
    fn snapshot(&self) -> MemorySnapshot {
        lock(&self.files).clone().finish()
    }
    fn time(&self) -> Time {
        Time::from_unix(self.clock.fetch_add(1, Ordering::SeqCst), 0)
    }
}
impl FileSystem for Fs {
    fn use_case_sensitive_file_names(&self) -> bool {
        true
    }
    fn snapshot_id(&self) -> Option<SnapshotId> {
        None
    }
    fn read_file(&self, name: &[u8]) -> Result<Option<FileContent>, tsr_vfs::Error> {
        let panic = {
            let mut target = lock(&self.panic_read);
            if target.as_deref() == Some(name) {
                target.take();
                true
            } else {
                false
            }
        };
        assert!(!panic, "intentional project read failure");
        let observer = lock(&self.read_observer).clone();
        if let Some(observer) = observer {
            observer(name);
        }
        self.snapshot().read_file(name)
    }
    fn stat(&self, name: &[u8]) -> Result<Option<FileInfo>, tsr_vfs::Error> {
        Ok(self.snapshot().stat(name)?.map(|mut info| {
            info.mod_time = lock(&self.times).get(name).copied().unwrap_or_default();
            info
        }))
    }
    fn entries(&self, name: &[u8]) -> Result<Entries, tsr_vfs::Error> {
        self.snapshot().entries(name)
    }
    fn realpath(&self, name: &[u8]) -> Result<JsString, tsr_vfs::Error> {
        self.snapshot().realpath(name)
    }
    fn write_file(&self, name: &[u8], content: &[u8]) -> Result<(), tsr_vfs::Error> {
        lock(&self.files).insert_loaded(name, content);
        lock(&self.times).insert(name.to_vec(), self.time());
        lock(&self.writes).push(JsString::from_bytes(name));
        Ok(())
    }
    fn remove(&self, name: &[u8]) -> Result<(), tsr_vfs::Error> {
        lock(&self.files).remove(name);
        lock(&self.times).remove(name);
        Ok(())
    }
    fn change_times(&self, name: &[u8], _: Time, time: Time) -> Result<(), tsr_vfs::Error> {
        if !self.file_exists(name)? {
            return Err(tsr_vfs::Error::Unsupported("missing test timestamp target"));
        }
        lock(&self.times).insert(name.to_vec(), time);
        lock(&self.touches).push(JsString::from_bytes(name));
        Ok(())
    }
}
struct Sys {
    fs: Arc<Fs>,
    output: Arc<Buffer>,
    clock: Arc<AtomicI64>,
}
impl Sys {
    fn new(files: &[(&str, &str)]) -> Arc<Self> {
        let clock = Arc::new(AtomicI64::new(200));
        Arc::new(Self {
            fs: Arc::new(Fs::new(clock.clone(), files)),
            output: Arc::default(),
            clock,
        })
    }
    fn output(&self) -> String {
        String::from_utf8(lock(&self.output.0).clone()).unwrap()
    }
}
impl Spawner for Sys {
    fn spawn(
        &self,
        _: &[JsString],
        _: &[u8],
        _: Box<dyn std::io::Write + Send>,
    ) -> Result<Stream, SpawnError> {
        panic!("unconfigured content mapper")
    }
}
impl System for Sys {
    fn writer(&self) -> SharedWriter {
        self.output.clone()
    }
    fn error_writer(&self) -> SharedWriter {
        self.output.clone()
    }
    fn fs(&self) -> Arc<dyn FileSystem> {
        self.fs.clone()
    }
    fn default_library_path(&self) -> &[u8] {
        b"/lib"
    }
    fn get_current_directory(&self) -> &[u8] {
        b"/work"
    }
    fn write_output_is_tty(&self) -> bool {
        false
    }
    fn get_width_of_terminal(&self) -> i64 {
        0
    }
    fn get_environment_variable(&self, _: &str) -> Option<JsString> {
        None
    }
    fn now(&self) -> Time {
        Time::from_unix(self.clock.fetch_add(1, Ordering::SeqCst), 0)
    }
    fn since_start(&self) -> Duration {
        Duration::ZERO
    }
}
fn orchestrator(sys: Arc<Sys>, args: &[&str]) -> Orchestrator {
    let host = tsr_compiler::CompilerConfigHost::new_live(
        sys.fs(),
        JsString::from_bytes(b"/work".as_slice()),
    );
    let args: Vec<_> = args
        .iter()
        .map(|arg| JsString::from_bytes(arg.as_bytes()))
        .collect();
    let command = tsr_tsoptions::parse_build_command_line(&args, &host);
    assert!(command.errors.is_empty(), "{:?}", command.errors);
    Orchestrator::new(Options {
        sys,
        command,
        testing: None,
    })
}
fn names(paths: &[JsString]) -> Vec<String> {
    paths
        .iter()
        .map(|path| {
            String::from_utf8_lossy(path.as_bytes())
                .trim_start_matches("/work/")
                .trim_end_matches("/tsconfig.json")
                .to_owned()
        })
        .collect()
}

#[test]
// source: tsc/internal/execute/build/graph_test.go:TestBuildOrderGenerator
fn pinned_graph_order_diamonds_cycles_and_watch_edges() {
    // The nine graph orders in pinned build/graph_test.go.
    let dependencies: &[(&str, &[&str])] = &[
        ("A", &["B", "C"]),
        ("B", &["C", "D"]),
        ("C", &["D", "E"]),
        ("D", &[]),
        ("E", &[]),
        ("F", &["E"]),
        ("G", &[]),
        ("H", &["I"]),
        ("I", &["J"]),
        ("J", &["H", "E"]),
    ];
    let files: Vec<_> = dependencies
        .iter()
        .map(|(name, deps)| {
            let refs = deps
                .iter()
                .map(|dep| format!("{{\"path\":\"../{dep}\"}}"))
                .collect::<Vec<_>>()
                .join(",");
            (
                format!("/work/{name}/tsconfig.json"),
                format!("{{\"files\":[],\"references\":[{refs}]}}"),
            )
        })
        .collect();
    let sys = Sys::new(
        &files
            .iter()
            .map(|(name, text)| (name.as_str(), text.as_str()))
            .collect::<Vec<_>>(),
    );
    let cases: &[(&[&str], &[&str], bool)] = &[
        (&["A", "G"], &["D", "E", "C", "B", "A", "G"], false),
        (&["A"], &["D", "E", "C", "B", "A"], false),
        (&["A", "C", "D"], &["D", "E", "C", "B", "A"], false),
        (&["D", "C", "A"], &["D", "E", "C", "B", "A"], false),
        (&["F"], &["E", "F"], false),
        (&["E"], &["E"], false),
        (&["F", "C", "A"], &["E", "F", "D", "C", "B", "A"], false),
        (&["H"], &["E", "J", "I", "H"], true),
        (&["A", "H"], &["D", "E", "C", "B", "A", "J", "I", "H"], true),
    ];
    for &(roots, expected, cyclic) in cases {
        for watch in [false, true] {
            let mut args = vec!["--build", if watch { "--watch" } else { "--dry" }];
            args.extend_from_slice(roots);
            let mut o = orchestrator(sys.clone(), &args);
            o.generate_graph().unwrap();
            assert_eq!(names(o.order()), expected);
            assert_eq!(!o.errors().is_empty(), cyclic);
            for (position, config) in o.order().iter().enumerate() {
                for upstream in o.upstream(config.as_bytes()) {
                    assert!(o.order()[..position].contains(&upstream));
                    assert_eq!(o.downstream(upstream.as_bytes()).contains(config), watch);
                }
            }
            let before: Vec<_> = o.tasks.iter().map(|node| node.task.clone()).collect();
            o.generate_graph_reusing_old_tasks().unwrap();
            assert_eq!(names(o.order()), expected);
            assert!(o
                .tasks
                .iter()
                .all(|node| before.iter().any(|old| Arc::ptr_eq(old, &node.task))));
        }
    }
}

#[test]
// source: tsc/internal/execute/build/buildtask_contentmapper_test.go:TestIsContentMapperSupplementalBuildInfoPath
fn pinned_supplemental_paths_require_numeric_index_and_supported_extension() {
    for (path, expected) in [
        ("/src/app.vue.0.ts", true),
        ("/src/app.vue.12.mts", true),
        ("/src/app.vue.ts", false),
        ("/src/app.vue.0.txt", false),
        ("/src/other.vue.0.ts", false),
        ("/src/app.vue.+2.ts", true),
        ("/src/app.vue.-1.ts", true),
        ("/src/app.vue.9223372036854775808.ts", false),
    ] {
        assert_eq!(
            is_content_mapper_supplemental_build_info_path(
                path.as_bytes(),
                [b"/src/app.vue".as_slice(), b"/src/index.ts"]
            ),
            expected,
            "{path}"
        );
    }
}

#[test]
fn cycle_skips_every_build_and_canceled_tasks_release_dependencies() {
    let sys = Sys::new(&[
        (
            "/work/a/tsconfig.json",
            "{\"files\":[],\"references\":[{\"path\":\"../b\"}]}",
        ),
        (
            "/work/b/tsconfig.json",
            "{\"files\":[],\"references\":[{\"path\":\"../a\"}]}",
        ),
    ]);
    let mut o = orchestrator(sys.clone(), &["--build", "a"]);
    assert_eq!(
        o.start(&Context::background()).unwrap().status,
        ExitStatus::ProjectReferenceCycle_OutputsSkipped
    );
    assert!(lock(&sys.fs.writes).is_empty());
    assert!(sys.output().contains("Cycle detected"));
    let sys = Sys::new(&[
        (
            "/work/a/tsconfig.json",
            "{\"files\":[],\"references\":[{\"path\":\"../b\"}]}",
        ),
        ("/work/b/tsconfig.json", "{\"files\":[],\"references\":[]}"),
    ]);
    let mut o = orchestrator(sys.clone(), &["--build", "a"]);
    let ctx = Context::background().with_cancel();
    ctx.cancel();
    assert_eq!(
        o.start(&ctx).unwrap().status,
        ExitStatus::DiagnosticsPresent_OutputsSkipped
    );
    assert!(lock(&sys.fs.writes).is_empty());
    assert!(o
        .order()
        .iter()
        .all(|config| o.status(config.as_bytes()).unwrap().is_error()));
}

#[test]
fn real_incremental_build_reuses_build_info_and_touches_unchanged_text() {
    let sys = Sys::new(&[
        (
            "/work/tsconfig.json",
            "{\"compilerOptions\":{\"composite\":true,\"noLib\":true,\"noCheck\":true},\"files\":[\"a.ts\",\"globals.d.ts\"]}",
        ),
        ("/work/a.ts", "export const answer = 42;\n"),
        (
            "/work/globals.d.ts",
            "interface Array<T> { length: number; [n: number]: T; } interface Boolean {} interface CallableFunction {} interface Function {} interface IArguments {} interface NewableFunction {} interface Number {} interface Object {} interface RegExp {} interface String {}",
        ),
    ]);
    let mut first = orchestrator(sys.clone(), &["--build"]);
    let result = first.start(&Context::background()).unwrap();
    assert_eq!(result.status, ExitStatus::Success, "{}", sys.output());
    for file in [
        b"/work/a.js".as_slice(),
        b"/work/a.d.ts",
        b"/work/tsconfig.tsbuildinfo",
    ] {
        assert!(
            sys.fs.file_exists(file).unwrap(),
            "{}",
            String::from_utf8_lossy(file)
        );
    }
    lock(&sys.fs.writes).clear();
    let mut second = orchestrator(sys.clone(), &["--build"]);
    assert_eq!(
        second.start(&Context::background()).unwrap().status,
        ExitStatus::Success
    );
    assert!(lock(&sys.fs.writes).is_empty());
    let touched = sys.now();
    sys.fs
        .change_times(b"/work/a.ts", Time::ZERO, touched)
        .unwrap();
    lock(&sys.fs.touches).clear();
    let mut third = orchestrator(sys.clone(), &["--build", "--verbose"]);
    assert_eq!(
        third.start(&Context::background()).unwrap().status,
        ExitStatus::Success,
        "{}",
        sys.output()
    );
    assert!(lock(&sys.fs.writes).is_empty());
    assert_eq!(
        *lock(&sys.fs.touches),
        vec![JsString::from_bytes(
            b"/work/tsconfig.tsbuildinfo".as_slice()
        )]
    );
}

#[test]
fn clean_dry_lists_outputs_and_real_clean_preserves_inputs() {
    let sys = Sys::new(&[
        (
            "/work/tsconfig.json",
            "{\"compilerOptions\":{\"composite\":true},\"files\":[\"a.ts\"]}",
        ),
        ("/work/a.ts", "export {};"),
        ("/work/a.js", "export {};"),
        ("/work/a.d.ts", "export {};"),
        ("/work/tsconfig.tsbuildinfo", "{}"),
    ]);
    let mut dry = orchestrator(sys.clone(), &["--build", "--clean", "--dry"]);
    assert_eq!(
        dry.start(&Context::background()).unwrap().status,
        ExitStatus::Success
    );
    assert!(sys.output().contains("A non-dry build would delete"));
    assert!(sys.fs.file_exists(b"/work/a.js").unwrap());
    let mut clean = orchestrator(sys.clone(), &["--build", "--clean"]);
    assert_eq!(
        clean.start(&Context::background()).unwrap().status,
        ExitStatus::Success
    );
    assert!(!sys.fs.file_exists(b"/work/a.js").unwrap());
    assert!(!sys.fs.file_exists(b"/work/a.d.ts").unwrap());
    assert!(!sys.fs.file_exists(b"/work/tsconfig.tsbuildinfo").unwrap());
    assert!(sys.fs.file_exists(b"/work/a.ts").unwrap());
}

#[derive(Default)]
struct Backend {
    callbacks: Mutex<Vec<tsr_tsc::fswatch::WatchCallback>>,
}
struct Registration;
impl tsr_tsc::watchmanager::Closer for Registration {
    fn close(&self) -> Result<(), tsr_tsc::fswatch::Error> {
        Ok(())
    }
}
impl tsr_tsc::watchmanager::WatchBackend for Backend {
    fn watch_directory(
        &self,
        _: &[u8],
        callback: tsr_tsc::fswatch::WatchCallback,
        _: bool,
        _: Option<tsr_tsc::watchmanager::Ignore>,
    ) -> Result<Arc<dyn tsr_tsc::watchmanager::Closer>, tsr_tsc::fswatch::Error> {
        lock(&self.callbacks).push(callback);
        Ok(Arc::new(Registration))
    }
    fn watch_directories(
        &self,
        requests: Vec<tsr_tsc::watchmanager::WatchDirectoryRequest>,
    ) -> Result<Vec<Arc<dyn tsr_tsc::watchmanager::Closer>>, tsr_tsc::fswatch::Error> {
        requests
            .into_iter()
            .map(|request| {
                self.watch_directory(
                    &request.dir,
                    request.callback,
                    request.recursive,
                    request.ignore,
                )
            })
            .collect()
    }
}
impl Backend {
    fn notify(&self, file: &[u8]) {
        let callback = lock(&self.callbacks)
            .first()
            .cloned()
            .expect("registered watch");
        callback(
            &[tsr_tsc::fswatch::Event {
                kind: tsr_tsc::fswatch::EventKind::EventUpdate,
                path: file.to_vec(),
            }],
            None,
        );
    }
}
struct Hooks {
    backend: Arc<Backend>,
    programs: AtomicUsize,
    panic_first_program: bool,
}
impl tsr_tsc::watchmanager::CommandLineTestingWithWatchBackend for Hooks {
    fn watch_backend(&self) -> Arc<dyn tsr_tsc::watchmanager::WatchBackend> {
        self.backend.clone()
    }
}
impl CommandLineTesting for Hooks {
    fn on_emitted_files(
        &self,
        _: Option<&tsr_compiler::EmitResult>,
        _: Option<&tsr_core::collections::SyncMap<JsString, Time>>,
    ) {
    }
    fn on_list_files_start(&self, _: &SharedWriter) {}
    fn on_list_files_end(&self, _: &SharedWriter) {}
    fn on_statistics_start(&self, _: &SharedWriter) {}
    fn on_statistics_end(&self, _: &SharedWriter) {}
    fn on_build_status_report_start(&self, _: &SharedWriter) {}
    fn on_build_status_report_end(&self, _: &SharedWriter) {}
    fn on_watch_status_report_start(&self) {}
    fn on_watch_status_report_end(&self) {}
    fn get_trace(&self, _: SharedWriter, _: tsr_locale::Locale) -> tsr_tsc::Trace {
        Box::new(|_, _| {})
    }
    fn on_program(&self, _: &tsr_incremental::Program) {
        let count = self.programs.fetch_add(1, Ordering::SeqCst);
        assert!(
            !self.panic_first_program || count != 0,
            "intentional reporter failure"
        );
    }
    fn as_with_watch_backend(
        &self,
    ) -> Option<&dyn tsr_tsc::watchmanager::CommandLineTestingWithWatchBackend> {
        Some(self)
    }
}

#[test]
fn build_watch_retains_graph_and_rebuilds_only_after_an_event() {
    let sys = Sys::new(&[
        (
            "/work/project/source/code/tsconfig.json",
            "{\"compilerOptions\":{\"composite\":true,\"noLib\":true,\"noCheck\":true,\"target\":\"ESNext\",\"module\":\"ESNext\"},\"files\":[\"a.ts\",\"globals.d.ts\"]}",
        ),
        (
            "/work/project/source/code/a.ts",
            "export const answer = 42;\n",
        ),
        (
            "/work/project/source/code/globals.d.ts",
            "interface Array<T> { length: number; [n: number]: T; } interface Boolean {} interface CallableFunction {} interface Function {} interface IArguments {} interface NewableFunction {} interface Number {} interface Object {} interface RegExp {} interface String {}",
        ),
    ]);
    let prepared = orchestrator(sys.clone(), &["--build", "project/source/code", "--watch"]);
    let mut options = Options {
        sys: sys.clone(),
        command: prepared.opts.command.clone(),
        testing: None,
    };
    let backend = Arc::new(Backend::default());
    let hooks = Arc::new(Hooks {
        backend: backend.clone(),
        programs: AtomicUsize::new(0),
        panic_first_program: false,
    });
    options.testing = Some(hooks.clone());
    let result = start(&Context::background(), options).unwrap();
    assert_eq!(result.status, ExitStatus::Success, "{}", sys.output());
    let watcher = result.watcher.unwrap();
    let before = sys.output();
    let clock = sys.clock.load(Ordering::SeqCst);
    watcher.do_cycle().unwrap();
    assert_eq!(sys.output(), before);
    assert_eq!(sys.clock.load(Ordering::SeqCst), clock);
    sys.fs
        .write_file(
            b"/work/project/source/code/a.ts",
            b"export const answer = 43;\n",
        )
        .unwrap();
    lock(&sys.fs.writes).clear();
    backend.notify(b"/work/project/source/code/a.ts");
    watcher.do_cycle().unwrap();
    assert_eq!(hooks.programs.load(Ordering::SeqCst), 2);
    assert!(sys.output().contains("File change detected"));
    assert!(sys
        .fs
        .read_file(b"/work/project/source/code/a.d.ts")
        .unwrap()
        .unwrap()
        .text
        .as_bytes()
        .windows(2)
        .any(|part| part == b"43"));
}

#[test]
fn reporter_panic_releases_following_reports_and_tasks() {
    let sys = Sys::new(&[
        (
            "/work/a/tsconfig.json",
            "{\"compilerOptions\":{\"noLib\":true,\"noCheck\":true},\"files\":[\"a.ts\"]}",
        ),
        ("/work/a/a.ts", "export {};"),
        (
            "/work/b/tsconfig.json",
            "{\"compilerOptions\":{\"noLib\":true,\"noCheck\":true},\"files\":[\"b.ts\"]}",
        ),
        ("/work/b/b.ts", "export {};"),
    ]);
    let mut o = orchestrator(sys.clone(), &["--build", "a", "b", "--builders", "2"]);
    let hooks = Arc::new(Hooks {
        backend: Arc::default(),
        programs: AtomicUsize::new(0),
        panic_first_program: true,
    });
    o.opts.testing = Some(hooks.clone());
    let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        o.start(&Context::background())
    }));
    assert!(panic.is_err());
    assert_eq!(hooks.programs.load(Ordering::SeqCst), 2);
    assert!(o
        .tasks
        .iter()
        .all(|node| *lock(&node.task.done.done) && *lock(&node.task.report_done.done)));
}

#[test]
fn circular_build_keeps_root_config_identity_and_emits_both_projects() {
    let sys = Sys::new(&[
        (
            "/work/a/tsconfig.json",
            r#"{"compilerOptions":{"composite":true,"noLib":true,"noCheck":true},"files":["index.ts"],"references":[{"path":"../b","circular":true}]}"#,
        ),
        ("/work/a/index.ts", "export const a = 10;"),
        (
            "/work/b/tsconfig.json",
            r#"{"compilerOptions":{"composite":true,"noLib":true,"noCheck":true},"files":["index.ts"],"references":[{"path":"../a"}]}"#,
        ),
        ("/work/b/index.ts", "export const b = 10;"),
    ]);
    let mut o = orchestrator(sys.clone(), &["--build", "a", "b"]);
    o.start(&Context::background()).unwrap();
    assert!(o.errors().is_empty());
    for name in [b"/work/a/index.d.ts".as_slice(), b"/work/b/index.d.ts"] {
        assert!(
            sys.fs.file_exists(name).unwrap(),
            "missing {}",
            String::from_utf8_lossy(name)
        );
    }
    let a = &o.tasks[o.index(b"/work/a/tsconfig.json")]
        .task
        .resolved
        .as_ref()
        .unwrap();
    let from_provider =
        tsr_compiler::ResolvedProjectReferenceProvider::get_resolved_project_reference(
            &o,
            b"/work/a/tsconfig.json",
            b"/work/a/tsconfig.json",
        )
        .unwrap();
    assert!(Arc::ptr_eq(a, &from_provider));
}

#[test]
fn project_panic_releases_dependent_tasks_and_reporters() {
    let sys = Sys::new(&[
        (
            "/work/a/tsconfig.json",
            r#"{"compilerOptions":{"composite":true,"noLib":true,"noCheck":true},"files":["a.ts"]}"#,
        ),
        ("/work/a/a.ts", "export const a = 1;"),
        (
            "/work/b/tsconfig.json",
            r#"{"compilerOptions":{"composite":true,"noLib":true,"noCheck":true},"files":["b.ts"],"references":[{"path":"../a"}]}"#,
        ),
        ("/work/b/b.ts", "export const b = 2;"),
    ]);
    *lock(&sys.fs.panic_read) = Some(b"/work/a/a.ts".to_vec());
    let mut o = orchestrator(sys.clone(), &["--build", "b", "--stopBuildOnErrors"]);
    let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        o.start(&Context::background())
    }));
    assert!(panic.is_err());
    assert!(o
        .tasks
        .iter()
        .all(|node| *lock(&node.task.done.done) && *lock(&node.task.report_done.done)));
    assert_eq!(
        o.status(b"/work/a/tsconfig.json").unwrap().kind,
        StatusKind::BuildErrors
    );
    assert_eq!(
        o.status(b"/work/b/tsconfig.json").unwrap().kind,
        StatusKind::UpstreamErrors
    );
}

#[test]
fn active_project_tasks_respect_the_builder_limit() {
    for builders in [1, 2, 4] {
        let mut files = Vec::new();
        for index in 0..12 {
            files.push((
                format!("/work/p{index}/tsconfig.json"),
                r#"{"compilerOptions":{"noLib":true,"noCheck":true},"files":["a.ts"]}"#.to_owned(),
            ));
            files.push((format!("/work/p{index}/a.ts"), "export {};".to_owned()));
        }
        let files: Vec<_> = files
            .iter()
            .map(|(name, text)| (name.as_str(), text.as_str()))
            .collect();
        let sys = Sys::new(&files);
        let mut args = vec![
            "--build".to_owned(),
            "--builders".to_owned(),
            builders.to_string(),
        ];
        args.extend((0..12).map(|index| format!("p{index}")));
        let mut o = orchestrator(sys, &args.iter().map(String::as_str).collect::<Vec<_>>());
        let active = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let entered = Arc::new(std::sync::Barrier::new(builders));
        o.task_observer = Some({
            let active = active.clone();
            let peak = peak.clone();
            Arc::new(move |start| {
                if start {
                    let count = active.fetch_add(1, Ordering::SeqCst) + 1;
                    peak.fetch_max(count, Ordering::SeqCst);
                    entered.wait();
                } else {
                    active.fetch_sub(1, Ordering::SeqCst);
                }
            })
        });
        o.start(&Context::background()).unwrap();
        assert_eq!(active.load(Ordering::SeqCst), 0);
        assert_eq!(peak.load(Ordering::SeqCst), builders);
    }
}

#[test]
fn independent_project_loads_are_not_serialized_by_the_source_cache() {
    // Run both the ordinary-source path and two distinct shared-cache keys.
    // A blocked read must not stop the other project from entering its read.
    for extension in ["ts", "d.ts"] {
        let files: Vec<_> = (0..2).flat_map(|i| [
            (format!("/work/p{i}/tsconfig.json"), format!(r#"{{"compilerOptions":{{"noLib":true,"noCheck":true}},"files":["a.{extension}"]}}"#)),
            (format!("/work/p{i}/a.{extension}"), "export {};".to_owned()),
        ]).collect();
        let sys = Sys::new(
            &files
                .iter()
                .map(|(name, text)| (name.as_str(), text.as_str()))
                .collect::<Vec<_>>(),
        );
        let mut o = orchestrator(sys.clone(), &["--build", "--builders", "2", "p0", "p1"]);
        let release = Arc::new((Mutex::new(false), Condvar::new()));
        let (entered, observed) = std::sync::mpsc::channel();
        *lock(&sys.fs.read_observer) = Some({
            let release = release.clone();
            let suffix = format!("/a.{extension}");
            Arc::new(move |name| {
                if name.ends_with(suffix.as_bytes()) {
                    entered.send(name.to_vec()).unwrap();
                    let _ = release
                        .1
                        .wait_timeout_while(lock(&release.0), Duration::from_secs(5), |ready| {
                            !*ready
                        })
                        .unwrap();
                }
            })
        });
        std::thread::scope(|scope| {
            let worker = scope.spawn(|| o.start(&Context::background()));
            let first = observed.recv_timeout(Duration::from_secs(2));
            let second = observed.recv_timeout(Duration::from_secs(2));
            *lock(&release.0) = true;
            release.1.notify_all();
            worker.join().unwrap().unwrap();
            assert!(
                first.is_ok() && second.is_ok(),
                "project {extension} loads serialized: {first:?}, {second:?}"
            );
            assert_ne!(first.unwrap(), second.unwrap());
        });
    }
}
