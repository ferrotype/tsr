use phase4_tsctests::execute::{
    command_line,
    fswatch::{Event, EventKind},
    tsc::{CommandLineResult, ExitStatus, SharedWriter, System, Watcher},
};
use phase4_tsctests::runner::TscInput;
use phase4_tsctests::sys::{new_test_sys, TestSys};
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Arc, Condvar, Mutex,
};
use std::time::Duration;
use tsr_contentmapper::{SpawnError, Spawner};
use tsr_ipc::{Closer, Context, Stream};
use tsr_jsstring::JsString;
use tsr_vfs::{iofs::Time, vfstest::InputFile, FileSystem};

#[derive(Default)]
struct Counts {
    spawns: AtomicUsize,
    closes: AtomicUsize,
    completed_closes: Mutex<usize>,
    closed: Condvar,
}
struct RecordingClose {
    inner: Arc<dyn Closer>,
    counts: Arc<Counts>,
    closed: AtomicBool,
}
impl Closer for RecordingClose {
    fn close(&self) -> std::io::Result<()> {
        if !self.closed.swap(true, Ordering::AcqRel) {
            self.counts.closes.fetch_add(1, Ordering::AcqRel);
            let result = self.inner.close();
            *self.counts.completed_closes.lock().unwrap() += 1;
            self.counts.closed.notify_all();
            result
        } else {
            Ok(())
        }
    }
}
struct RecordingSystem {
    inner: Arc<TestSys>,
    counts: Arc<Counts>,
    lifecycle: Arc<tsr_contentmappertest::ProjectLifecycle>,
}
impl Spawner for RecordingSystem {
    fn spawn(
        &self,
        command: &[JsString],
        directory: &[u8],
        stderr: Box<dyn std::io::Write + Send>,
    ) -> Result<Stream, SpawnError> {
        let mut stream =
            tsr_contentmappertest::new_spawner_with_project_lifecycle(self.lifecycle.clone())
                .spawn(command, directory, stderr)?;
        self.counts.spawns.fetch_add(1, Ordering::AcqRel);
        stream.closer = Arc::new(RecordingClose {
            inner: stream.closer,
            counts: self.counts.clone(),
            closed: AtomicBool::new(false),
        });
        Ok(stream)
    }
}
impl System for RecordingSystem {
    fn writer(&self) -> SharedWriter {
        self.inner.writer()
    }
    fn error_writer(&self) -> SharedWriter {
        self.inner.error_writer()
    }
    fn fs(&self) -> Arc<dyn FileSystem> {
        self.inner.fs()
    }
    fn default_library_path(&self) -> &[u8] {
        self.inner.default_library_path()
    }
    fn get_current_directory(&self) -> &[u8] {
        self.inner.get_current_directory()
    }
    fn write_output_is_tty(&self) -> bool {
        self.inner.write_output_is_tty()
    }
    fn get_width_of_terminal(&self) -> i64 {
        self.inner.get_width_of_terminal()
    }
    fn get_environment_variable(&self, name: &str) -> Option<JsString> {
        self.inner.get_environment_variable(name)
    }
    fn now(&self) -> Time {
        self.inner.now()
    }
    fn since_start(&self) -> Duration {
        self.inner.since_start()
    }
}
fn manifest(mapper: &str) -> String {
    let dynamic = if mapper == tsr_contentmappertest::DYNAMIC_VERBATIM_MAPPER {
        ",\"dynamicConfig\":true"
    } else {
        ""
    };
    format!(
        r#"{{"name":"mapper","version":"1.0.0","typescript":{{"contentMapper":{{"exec":["{mapper}"]{dynamic}}}}}}}"#
    )
}
fn system(config: &str, name: &str, contents: &str, mapper: &str) -> Arc<RecordingSystem> {
    let files = [
        ("tsconfig.json".to_owned(), config.to_owned()),
        (name.to_owned(), contents.to_owned()),
        (
            "node_modules/mapper/package.json".to_owned(),
            manifest(mapper),
        ),
    ]
    .into_iter()
    .map(|(name, text)| {
        (
            format!("/home/src/workspaces/project/{name}").into_bytes(),
            InputFile::Text(text.into_bytes()),
        )
    })
    .collect();
    system_from_files(files)
}
fn system_from_files(
    files: std::collections::BTreeMap<Vec<u8>, InputFile>,
) -> Arc<RecordingSystem> {
    Arc::new(RecordingSystem {
        inner: new_test_sys(
            &TscInput {
                files,
                ..Default::default()
            },
            false,
        ),
        counts: Arc::default(),
        lifecycle: Arc::default(),
    })
}
fn run(sys: &Arc<RecordingSystem>, args: &[&str]) -> CommandLineResult {
    run_with_context(&Context::background(), sys, args)
}
fn run_with_context(ctx: &Context, sys: &Arc<RecordingSystem>, args: &[&str]) -> CommandLineResult {
    command_line(
        ctx,
        sys.clone(),
        &args
            .iter()
            .map(|v| JsString::from_bytes(v.as_bytes()))
            .collect::<Vec<_>>(),
        Some(sys.inner.clone()),
    )
    .expect("command completes")
}
fn output(sys: &RecordingSystem) -> String {
    String::from_utf8(sys.inner.current_write().string()).unwrap()
}

#[test]
// source: tsc/internal/execute/tsctests/contentmapper_watch_test.go:TestContentMapperBuildLifecycle
fn content_mapper_build_lifecycle() {
    let sys = system(
        r#"{"compilerOptions":{"composite":true},"contentMappers":[{"package":"mapper","extensions":[".vue"]}]}"#,
        "app.vue",
        "export const app = 1;",
        tsr_contentmappertest::VERBATIM_MAPPER,
    );
    let result = run(&sys, &["--build", "--runExternalCode"]);
    assert!(result.watcher.is_none());
    assert_eq!(sys.counts.spawns.load(Ordering::Acquire), 1);
    assert_eq!(sys.counts.closes.load(Ordering::Acquire), 1);
}

#[test]
// source: tsc/internal/execute/tsctests/contentmapper_watch_test.go:TestContentMapperSupplementalDiagnosticUsesOriginalFileName
fn supplemental_diagnostic_uses_original_file_name() {
    let sys = system(
        r#"{"compilerOptions":{"noEmit":true},"contentMappers":[{"package":"mapper","extensions":[".astro"]}]}"#,
        "app.astro",
        "const value: string = 1;",
        tsr_contentmappertest::SUPPLEMENTAL_DIAGNOSTICS_MAPPER,
    );
    let result = run(&sys, &["--pretty", "false", "--runExternalCode"]);
    assert_eq!(
        result.status,
        ExitStatus::DiagnosticsPresent_OutputsGenerated
    );
    let output = output(&sys);
    assert!(output.contains("app.astro(1,1): error TS2304"), "{output}");
    assert!(output.contains("app.astro(1,7): error TS2322"), "{output}");
    assert!(!output.contains("app.astro.0.ts"), "{output}");
}

#[test]
// source: tsc/internal/execute/tsctests/contentmapper_watch_test.go:TestContentMapperBuildDetectsNewPhysicalSupplementalFile
fn build_detects_new_physical_supplemental_file() {
    let sys = system(
        r#"{"compilerOptions":{"incremental":true},"files":["app.vue"],"contentMappers":[{"package":"mapper","extensions":[".vue"]}]}"#,
        "app.vue",
        "declare const value: number;",
        tsr_contentmappertest::SUPPLEMENTAL_MAPPER,
    );
    let args = ["--build", "--pretty", "false", "--runExternalCode"];
    assert_eq!(
        run(&sys, &args).status,
        ExitStatus::Success,
        "{}",
        output(&sys)
    );
    sys.inner.clear_output();
    sys.inner.write_file_no_error(
        b"/home/src/workspaces/project/app.vue.0.ts",
        b"export {};\n",
    );
    assert_eq!(
        run(&sys, &args).status,
        ExitStatus::DiagnosticsPresent_OutputsGenerated
    );
    let output = output(&sys);
    assert!(output.contains("TS100025"), "{output}");
    assert!(
        output.contains("conflicts with an existing file"),
        "{output}"
    );
}

#[test]
// source: tsc/internal/execute/tsctests/contentmapper_watch_test.go:TestContentMapperBuildIdentityFailureExitStatus
fn build_identity_failure_exit_status() {
    let sys = system(
        r#"{"compilerOptions":{"composite":true},"contentMappers":[{"package":"mapper","extensions":[".vue"]}]}"#,
        "app.vue",
        "export const app = 1;",
        tsr_contentmappertest::DYNAMIC_VERBATIM_MAPPER,
    );
    let args = ["--build", "--runExternalCode"];
    assert_eq!(
        run(&sys, &args).status,
        ExitStatus::Success,
        "{}",
        output(&sys)
    );
    sys.inner.write_file_no_error(b"/home/src/workspaces/project/node_modules/mapper/package.json",br#"{"name":"mapper","version":"1.0.0","typescript":{"contentMapper":{"exec":["missing-mapper"],"dynamicConfig":true}}}"#);
    assert_eq!(
        run(&sys, &args).status,
        ExitStatus::DiagnosticsPresent_OutputsSkipped
    );
}

fn read_config(files: &[(&str, &str)]) -> tsr_tsoptions::ParsedCommandLine {
    let mut fs = tsr_vfs::MemoryBuilder::new(b"/project", false);
    for (name, text) in files {
        fs.insert_loaded(name.as_bytes(), text.as_bytes());
    }
    let host = tsr_compiler::CompilerConfigHost::new(
        Arc::new(fs.finish()),
        JsString::from_bytes(b"/project".as_slice()),
    );
    let cache = tsr_tsoptions::ExtendedConfigCache::new(&host);
    cache
        .read_config_file(
            b"/project/tsconfig.json",
            &Default::default(),
            &tsr_tsoptions::ConfigValue::Null,
        )
        .unwrap()
        .command_line
        .unwrap()
}

#[test]
// source: tsc/internal/execute/tsc/extendedconfigcache_test.go:TestExtendedConfigCacheExtendsCircularity
fn extended_config_cache_extends_circularity() {
    for files in [
        vec![
            ("/project/tsconfig.json", r#"{"extends":"./base.json"}"#),
            ("/project/base.json", r#"{"extends":"./base.json"}"#),
        ],
        vec![
            ("/project/tsconfig.json", r#"{"extends":"./other.json"}"#),
            ("/project/other.json", r#"{"extends":"./tsconfig.json"}"#),
        ],
        vec![
            ("/project/tsconfig.json", r#"{"extends":"./Base.json"}"#),
            ("/project/base.json", r#"{"extends":"./base.json"}"#),
        ],
    ] {
        let config = read_config(&files);
        assert!(
            config.errors.iter().any(|error| error.code == 18000),
            "expected circularity diagnostic"
        );
    }
}

#[test]
// source: tsc/internal/execute/tsc/extendedconfigcache_test.go:TestExtendedConfigCacheNullExtendsDoesNotPanic
fn extended_config_cache_null_extends_does_not_panic() {
    assert!(!read_config(&[
        ("/project/tsconfig.json", r#"{"extends":null}"#),
        ("/project/main.ts", "// Hello World!")
    ])
    .errors
    .is_empty());
}

const PROJECT_ROOT: &str = "/home/src/workspaces/project/";
const MAPPER_CONFIG: &str = r#"{"compilerOptions":{"composite":true},"contentMappers":[{"package":"mapper","extensions":[".vue"]}]}"#;
struct CancelOnDrop(Context);
impl CancelOnDrop {
    fn new() -> Self {
        Self(Context::background().with_cancel())
    }
}
impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.cancel();
    }
}
fn write(sys: &RecordingSystem, name: &str, contents: &str) {
    sys.inner.write_file_no_error(
        format!("{PROJECT_ROOT}{name}").as_bytes(),
        contents.as_bytes(),
    );
}
fn notify(sys: &RecordingSystem, names: &[&str]) {
    sys.inner.mock_watch_backend.send_events(
        &names
            .iter()
            .map(|name| Event {
                kind: EventKind::EventUpdate,
                path: format!("{PROJECT_ROOT}{name}").into_bytes(),
            })
            .collect::<Vec<_>>(),
    );
}
fn compiler_watcher(watcher: &dyn Watcher) -> &tsr_execute::tsc::watcher::CompilerWatcher {
    (watcher as &dyn std::any::Any).downcast_ref().unwrap()
}
fn assert_process_counts(sys: &RecordingSystem, spawns: usize, closes: usize) {
    assert_eq!(
        sys.counts.spawns.load(Ordering::Acquire),
        spawns,
        "{}",
        output(sys)
    );
    assert_eq!(
        sys.counts.closes.load(Ordering::Acquire),
        closes,
        "{}",
        output(sys)
    );
}

#[test]
// source: tsc/internal/execute/tsctests/contentmapper_watch_test.go:TestContentMapperWatchLifecycle
fn content_mapper_watch_lifecycle() {
    for args in [
        vec!["--watch", "--runExternalCode"],
        vec!["--build", "--watch", "--runExternalCode"],
    ] {
        let config_a = MAPPER_CONFIG.replace("\"mapper\"", "\"mapper-a\"");
        let config_b = MAPPER_CONFIG.replace("\"mapper\"", "\"mapper-b\"");
        let sys = system(
            &config_a,
            "app.vue",
            "export const app = 1;",
            tsr_contentmappertest::VERBATIM_MAPPER,
        );
        let package = manifest(tsr_contentmappertest::VERBATIM_MAPPER);
        write(&sys, "node_modules/mapper-a/package.json", &package);
        write(
            &sys,
            "node_modules/mapper-b/package.json",
            &package.replace("1.0.0", "2.0.0"),
        );
        let ctx = CancelOnDrop::new();
        let result = run_with_context(&ctx.0, &sys, &args);
        let watcher = result.watcher.unwrap();
        assert_process_counts(&sys, 1, 0);
        write(&sys, "tsconfig.json", &config_b);
        notify(&sys, &["tsconfig.json"]);
        watcher.do_cycle().expect("watch cycle completes");
        assert_process_counts(&sys, 2, 1);
        write(
            &sys,
            "tsconfig.json",
            r#"{"compilerOptions":{"composite":true}}"#,
        );
        notify(&sys, &["tsconfig.json"]);
        watcher.do_cycle().expect("watch cycle completes");
        assert_process_counts(&sys, 2, 2);
        write(&sys, "tsconfig.json", &config_a);
        notify(&sys, &["tsconfig.json"]);
        watcher.do_cycle().expect("watch cycle completes");
        assert_process_counts(&sys, 3, 2);
        ctx.0.cancel();
        let (completed, _) = sys
            .counts
            .closed
            .wait_timeout_while(
                sys.counts.completed_closes.lock().unwrap(),
                Duration::from_secs(1),
                |count| *count != 3,
            )
            .unwrap();
        assert_eq!(
            *completed, 3,
            "content mapper process was not closed after cancellation"
        );
        assert_process_counts(&sys, 3, 3);
    }
}

#[test]
// source: tsc/internal/execute/tsctests/contentmapper_watch_test.go:TestContentMapperSupplementalCollisionWatch
fn content_mapper_supplemental_collision_watch() {
    let sys = system(
        r#"{"compilerOptions":{"noLib":true},"contentMappers":[{"package":"mapper","extensions":[".vue"]}]}"#,
        "app.vue",
        "declare const value: number;",
        tsr_contentmappertest::SUPPLEMENTAL_MAPPER,
    );
    let ctx = CancelOnDrop::new();
    let result = run_with_context(&ctx.0, &sys, &["--watch", "--runExternalCode"]);
    let watcher = result.watcher.unwrap();
    let full_builds = compiler_watcher(watcher.as_ref()).full_builds();
    write(&sys, "app.vue.0.ts", "export {};\n");
    notify(&sys, &["app.vue.0.ts"]);
    watcher.do_cycle().expect("watch cycle completes");
    assert_eq!(
        compiler_watcher(watcher.as_ref()).full_builds(),
        full_builds + 1
    );
    sys.inner
        .fs_from_file_map()
        .remove(format!("{PROJECT_ROOT}app.vue.0.ts").as_bytes())
        .unwrap();
    notify(&sys, &["app.vue.0.ts"]);
    watcher.do_cycle().expect("watch cycle completes");
    assert_eq!(
        compiler_watcher(watcher.as_ref()).full_builds(),
        full_builds + 2
    );
}

fn dynamic_mapper_watch_dependency(build: bool) {
    let sys = system(
        MAPPER_CONFIG,
        "app.vue",
        "export const app = 1;",
        tsr_contentmappertest::DYNAMIC_VERBATIM_MAPPER,
    );
    write(&sys, "mapper.config.json", r#"{"version":1}"#);
    let ctx = CancelOnDrop::new();
    let args = if build {
        vec!["--build", "--watch", "--runExternalCode"]
    } else {
        vec!["--watch", "--runExternalCode"]
    };
    let result = run_with_context(&ctx.0, &sys, &args);
    let watcher = result.watcher.unwrap();
    let full_builds = (!build).then(|| compiler_watcher(watcher.as_ref()).full_builds());
    assert_eq!(sys.lifecycle.opens.load(Ordering::Acquire), 1);
    write(&sys, "mapper.config.json", r#"{"version":2}"#);
    notify(&sys, &["mapper.config.json"]);
    watcher.do_cycle().expect("watch cycle completes");
    if let Some(full_builds) = full_builds {
        assert_eq!(
            compiler_watcher(watcher.as_ref()).full_builds(),
            full_builds + 1
        );
    }
    assert_eq!(sys.lifecycle.opens.load(Ordering::Acquire), 2);
    assert_eq!(sys.lifecycle.closes.load(Ordering::Acquire), 1);
    assert_process_counts(&sys, 1, 0);
}
#[test]
// source: tsc/internal/execute/tsctests/contentmapper_watch_test.go:TestDynamicContentMapperWatchDependency
fn dynamic_content_mapper_watch_dependency() {
    dynamic_mapper_watch_dependency(false);
}
#[test]
// source: tsc/internal/execute/tsctests/contentmapper_watch_test.go:TestDynamicContentMapperBuildWatchDependency
fn dynamic_content_mapper_build_watch_dependency() {
    dynamic_mapper_watch_dependency(true);
}

#[test]
// source: tsc/internal/execute/tsctests/contentmapper_watch_test.go:TestContentMapperMixedWatchBatchForcesFullRebuild
fn content_mapper_mixed_watch_batch_forces_full_rebuild() {
    let sys = system(
        r#"{"compilerOptions":{"noLib":true},"contentMappers":[{"package":"mapper","extensions":[".vue"]}]}"#,
        "app.vue",
        "export const marker = 1 as const;",
        tsr_contentmappertest::VERBATIM_MAPPER,
    );
    write(
        &sys,
        "main.ts",
        r#"import { marker } from "./app.vue"; const check: 1 = marker;"#,
    );
    let ctx = CancelOnDrop::new();
    let result = run_with_context(
        &ctx.0,
        &sys,
        &["--watch", "--pretty", "false", "--runExternalCode"],
    );
    let watcher = result.watcher.unwrap();
    let compiler = compiler_watcher(watcher.as_ref());
    let (fast_builds, full_builds) = (compiler.fast_path_builds(), compiler.full_builds());
    sys.inner.current_write().reset();
    write(&sys, "app.vue", "export const marker = 2 as const;");
    write(
        &sys,
        "main.ts",
        r#"import { marker } from "./app.vue"; const check: 2 = marker;"#,
    );
    notify(&sys, &["app.vue", "main.ts"]);
    watcher.do_cycle().expect("watch cycle completes");
    assert_eq!(compiler.full_builds(), full_builds + 1);
    assert_eq!(compiler.fast_path_builds(), fast_builds);
    let output = output(&sys);
    assert!(
        !output.contains("Type '1' is not assignable to type '2'"),
        "{output}"
    );
}

fn symlinked_mapper_system() -> Arc<RecordingSystem> {
    system_from_files(
        [
            (
                format!("{PROJECT_ROOT}tsconfig.json").into_bytes(),
                InputFile::Text(MAPPER_CONFIG.as_bytes().to_vec()),
            ),
            (
                format!("{PROJECT_ROOT}app.vue").into_bytes(),
                InputFile::Text(b"export const app = 1;".to_vec()),
            ),
            (
                format!("{PROJECT_ROOT}node_modules/mapper").into_bytes(),
                InputFile::File(tsr_vfs::vfstest::symlink(b"/home/src/workspaces/mapper")),
            ),
            (
                b"/home/src/workspaces/mapper/package.json".to_vec(),
                InputFile::Text(manifest(tsr_contentmappertest::VERBATIM_MAPPER).into_bytes()),
            ),
        ]
        .into_iter()
        .collect(),
    )
}
#[test]
// source: tsc/internal/execute/tsctests/contentmapper_watch_test.go:TestContentMapperBuildWatchSymlinkedManifestChange
fn content_mapper_build_watch_symlinked_manifest_change() {
    let sys = symlinked_mapper_system();
    let ctx = CancelOnDrop::new();
    let result = run_with_context(&ctx.0, &sys, &["--build", "--watch", "--runExternalCode"]);
    let watcher = result.watcher.unwrap();
    assert_process_counts(&sys, 1, 0);
    let target = b"/home/src/workspaces/mapper/package.json";
    sys.inner.write_file_no_error(
        target,
        manifest(tsr_contentmappertest::VERBATIM_MAPPER)
            .replace("1.0.0", "2.0.0")
            .as_bytes(),
    );
    sys.inner.mock_watch_backend.send_events(&[Event {
        kind: EventKind::EventUpdate,
        path: target.to_vec(),
    }]);
    watcher.do_cycle().expect("watch cycle completes");
    assert_process_counts(&sys, 2, 1);
}
#[test]
// source: tsc/internal/execute/tsctests/contentmapper_watch_test.go:TestContentMapperBuildWatchSymlinkedManifestDelete
fn content_mapper_build_watch_symlinked_manifest_delete() {
    let sys = symlinked_mapper_system();
    let ctx = CancelOnDrop::new();
    let result = run_with_context(&ctx.0, &sys, &["--build", "--watch", "--runExternalCode"]);
    let watcher = result.watcher.unwrap();
    assert_process_counts(&sys, 1, 0);
    sys.inner.clear_output();
    let target = b"/home/src/workspaces/mapper/package.json";
    sys.inner.fs_from_file_map().remove(target).unwrap();
    sys.inner.mock_watch_backend.send_events(&[Event {
        kind: EventKind::EventDelete,
        path: target.to_vec(),
    }]);
    watcher.do_cycle().expect("watch cycle completes");
    assert_process_counts(&sys, 1, 1);
    let output = output(&sys);
    assert!(
        output.contains("The content mapper package 'mapper' could not be resolved."),
        "{output}"
    );
}

#[test]
// source: tsc/internal/execute/tsctests/contentmapper_watch_test.go:TestContentMapperBuildWatchSharedLifecycle
fn content_mapper_build_watch_shared_lifecycle() {
    let sys = system(
        r#"{"files":[],"references":[{"path":"a"},{"path":"b"}]}"#,
        "a/app.vue",
        "export const a = 1;",
        tsr_contentmappertest::VERBATIM_MAPPER,
    );
    write(&sys, "a/tsconfig.json", MAPPER_CONFIG);
    write(&sys, "b/tsconfig.json", MAPPER_CONFIG);
    write(&sys, "b/app.vue", "export const b = 1;");
    let ctx = CancelOnDrop::new();
    let result = run_with_context(&ctx.0, &sys, &["--build", "--watch", "--runExternalCode"]);
    let watcher = result.watcher.unwrap();
    assert_process_counts(&sys, 1, 0);
    for (project, closes) in [("a", 0), ("b", 1)] {
        let config = format!("{project}/tsconfig.json");
        write(&sys, &config, r#"{"compilerOptions":{"composite":true}}"#);
        notify(&sys, &[&config]);
        watcher.do_cycle().expect("watch cycle completes");
        assert_process_counts(&sys, 1, closes);
    }
}
