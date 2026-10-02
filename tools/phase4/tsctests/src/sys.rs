//! `sys.go`: the fake system the scenarios run on, its clock, the testing
//! hooks the command line calls, and the output sanitizer that makes the
//! console output deterministic.
use crate::execute::tsc::{
    self, write_all, CommandLineTesting, SharedWriter, System, Trace, Writer,
};
use crate::execute::watchmanager::{CommandLineTestingWithWatchBackend, WatchBackend};
use crate::fs::TestFs;
use crate::fsbaselineutil::{sanitize_internal_symbol_name, FsDiffer};
use crate::goutil::{replace_all, StringBuilder};
use crate::harnessutil::{ComparePathsOptions, TracerForBaselining, FAKE_TS_VERSION};
use crate::mock_watch_backend::{new_mock_watch_backend, MockWatchBackend};
use crate::program_view;
use crate::runner::TscInput;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::time::Duration;
use tsr_compiler::EmitResult;
use tsr_contentmapper::{SpawnError, Spawner};
use tsr_core::collections::SyncMap;
use tsr_diagnostics::{Argument, Message};
use tsr_incremental::SignatureUpdateKind;
use tsr_ipc::Stream;
use tsr_jsstring::JsString;
use tsr_locale::Locale;
use tsr_vfs::iofs::Time;
use tsr_vfs::iovfs::IoVfs;
use tsr_vfs::vfstest::{self, InputFile, TestFs as MapFs};
use tsr_vfs::FileSystem;

/// The input file map: Go's `FileMap` (`map[string]any`), a text, bytes or a
/// symbolic link per rooted path.
pub type FileMap = BTreeMap<Vec<u8>, InputFile>;

pub const TSC_LIB_PATH: &str = "/home/src/tslibs/TS/Lib";

/// The one library text the fake system writes for every default library.
pub const TSC_DEFAULT_LIB_CONTENT: &str = r#"/// <reference no-default-lib="true"/>
interface Boolean {}
interface Function {}
interface CallableFunction {}
interface NewableFunction {}
interface IArguments {}
interface Number { toExponential: any; }
interface Object {}
interface RegExp {}
interface String { charAt: any; }
interface Array<T> { length: number; [n: number]: T; }
interface ReadonlyArray<T> {}
interface SymbolConstructor {
    (desc?: string | number): symbol;
    for(name: string): symbol;
    readonly toStringTag: symbol;
}
declare var Symbol: SymbolConstructor;
interface Symbol {
    readonly [Symbol.toStringTag]: string;
}
declare const console: { log(msg: any): void; };"#;

/// The library file of a `--lib` name, under the fake system's library path.
// port: tsc/internal/execute/tsctests/sys.go:getTestLibPathFor
pub fn get_test_lib_path_for(lib_name: &str) -> String {
    let lib_file = match tsr_tsoptions::LIB_MAP
        .iter()
        .find(|(name, _)| *name == lib_name)
    {
        Some((_, file)) => (*file).to_owned(),
        None => format!("lib.{lib_name}.d.ts"),
    };
    format!("{TSC_LIB_PATH}/{lib_file}")
}

/// `tsoptions.LibFilesSet`: every library file name `--lib` maps to.
fn lib_files_set() -> BTreeSet<&'static str> {
    tsr_tsoptions::LIB_MAP
        .iter()
        .map(|(_, file)| *file)
        .collect()
}

/// `TestClock`: every reading advances the clock by one second from its
/// start.
#[derive(Debug)]
pub struct TestClock {
    start: Time,
    now: Mutex<Time>,
}

impl TestClock {
    pub fn new(start: Time) -> Self {
        Self {
            start,
            now: Mutex::new(Time::ZERO),
        }
    }

    pub fn start(&self) -> Time {
        self.start
    }

    // port: tsc/internal/execute/tsctests/sys.go:TestClock.Now
    pub fn now(&self) -> Time {
        let mut now = self.now.lock().unwrap_or_else(PoisonError::into_inner);
        if now.is_zero() {
            *now = self.start;
        }
        *now = now.add_seconds(1); // Simulate some time passing
        *now
    }

    /// The readings made so far: whole seconds since the start (harness
    /// instrumentation, as the recorder counts them).
    ///
    /// # Panics
    /// When the clock moved by a fraction of a second.
    pub fn readings(&self) -> u64 {
        let now = *self.now.lock().unwrap_or_else(PoisonError::into_inner);
        if now.is_zero() {
            return 0;
        }
        let elapsed = duration_between(self.start, now);
        assert!(
            elapsed.subsec_nanos() == 0,
            "the test clock moved by {elapsed:?}, not whole seconds"
        );
        elapsed.as_secs()
    }

    // port: tsc/internal/execute/tsctests/sys.go:TestClock.SinceStart
    pub fn since_start(&self) -> Duration {
        duration_between(self.start, self.now())
    }
}

/// `later.Sub(earlier)` for `later` not before `earlier`.
fn duration_between(earlier: Time, later: Time) -> Duration {
    let (later_seconds, later_nanos) = later.unix();
    let (earlier_seconds, earlier_nanos) = earlier.unix();
    let nanos = (i128::from(later_seconds) - i128::from(earlier_seconds)) * 1_000_000_000
        + i128::from(later_nanos)
        - i128::from(earlier_nanos);
    Duration::from_nanos(u64::try_from(nanos.max(0)).unwrap_or(u64::MAX))
}

impl vfstest::Clock for TestClock {
    fn now(&self) -> Time {
        TestClock::now(self)
    }
}

/// What `NewTscSystem` sets of a `TestSys`; `new_test_sys` completes it.
pub struct TscSystem {
    pub fs: Arc<TestFs>,
    pub cwd: Vec<u8>,
    pub output_is_tty: bool,
    pub clock: Arc<TestClock>,
}

// port: tsc/internal/execute/tsctests/sys.go:NewTscSystem
pub fn new_tsc_system(
    files: &FileMap,
    use_case_sensitive_file_names: bool,
    cwd: &[u8],
) -> TscSystem {
    let clock = Arc::new(TestClock::new(Time::now()));
    let map_fs = vfstest::from_map_with_clock(files, use_case_sensitive_file_names, clock.clone());
    TscSystem {
        fs: Arc::new(TestFs::new(Arc::new(map_fs.into_vfs()))),
        cwd: cwd.to_vec(),
        output_is_tty: true,
        clock,
    }
}

/// The files the command line wrote, added to `files`.
// port: tsc/internal/execute/tsctests/sys.go:GetFileMapWithBuild
pub fn get_file_map_with_build(mut files: FileMap, command_line_args: &[JsString]) -> FileMap {
    let sys = new_test_sys(
        &TscInput {
            files: files.clone(),
            ..TscInput::default()
        },
        false,
    );
    crate::execute::command_line(
        &tsr_ipc::Context::background(),
        sys.clone(),
        command_line_args,
        Some(sys.clone()),
    );
    for key in sys.fs.written_files.to_slice() {
        if let Some(text) = sys.fs_from_file_map().read_file(&key) {
            files.insert(key, InputFile::Text(text));
        }
    }
    files
}

// port: tsc/internal/execute/tsctests/sys.go:newTestSys
pub fn new_test_sys(tsc_input: &TscInput, for_incremental_correctness: bool) -> Arc<TestSys> {
    let cwd: &[u8] = if tsc_input.cwd.is_empty() {
        b"/home/src/workspaces/project"
    } else {
        &tsc_input.cwd
    };
    let mut lib_path = TSC_LIB_PATH.as_bytes().to_vec();
    if !tsc_input.windows_style_root.is_empty() {
        lib_path = [&tsc_input.windows_style_root[..], &lib_path[1..]].concat();
    }
    let current_write = Arc::new(StringBuilder::new());
    let system = new_tsc_system(&tsc_input.files, !tsc_input.ignore_case, cwd);
    let output_is_tty = tsc_input.output_is_tty.unwrap_or(system.output_is_tty);
    let tracer = Arc::new(TracerForBaselining::new(
        ComparePathsOptions {
            use_case_sensitive_file_names: !tsc_input.ignore_case,
            current_directory: cwd.to_vec(),
        },
        current_write.clone(),
    ));
    let raw = system.fs.fs.clone();
    let mock_watch_backend = Arc::new(new_mock_watch_backend(
        Some(Arc::new(move |dir: &[u8]| raw.directory_exists(dir))),
        !tsc_input.ignore_case,
    ));
    let fs_differ = FsDiffer::new(
        system.fs.fs.clone(),
        Some(system.fs.default_libs.clone()),
        system.fs.written_files.clone(),
    );
    let sys = TestSys {
        current_write,
        program_baselines: StringBuilder::new(),
        program_include_baselines: StringBuilder::new(),
        tracer,
        fs_differ,
        for_incremental_correctness,
        mock_watch_backend,
        fs: system.fs,
        default_library_path: lib_path,
        cwd: system.cwd,
        env: tsc_input.env.clone(),
        output_is_tty,
        clock: system.clock,
    };

    // Ensure the default library file is present
    sys.ensure_lib_path_exists(b"lib.d.ts");
    for (_, lib_file) in tsr_tsoptions::enum_maps::target_to_lib_map() {
        sys.ensure_lib_path_exists(lib_file.as_bytes());
    }
    for lib_file in lib_files_set() {
        sys.ensure_lib_path_exists(lib_file.as_bytes());
    }
    Arc::new(sys)
}

/// `TestSys`.
pub struct TestSys {
    current_write: Arc<StringBuilder>,
    program_baselines: StringBuilder,
    program_include_baselines: StringBuilder,
    tracer: Arc<TracerForBaselining>,
    pub fs_differ: FsDiffer,
    pub for_incremental_correctness: bool,
    pub mock_watch_backend: Arc<MockWatchBackend>,

    pub fs: Arc<TestFs>,
    default_library_path: Vec<u8>,
    cwd: Vec<u8>,
    env: BTreeMap<String, String>,
    output_is_tty: bool,
    pub clock: Arc<TestClock>,
}

impl TestSys {
    // port: tsc/internal/execute/tsctests/sys.go:TestSys.fsFromFileMap
    pub fn fs_from_file_map(&self) -> &IoVfs<MapFs> {
        &self.fs_differ.fs
    }

    // port: tsc/internal/execute/tsctests/sys.go:TestSys.mapFs
    pub fn map_fs(&self) -> &MapFs {
        self.fs_differ.map_fs()
    }

    // port: tsc/internal/execute/tsctests/sys.go:TestSys.ensureLibPathExists
    fn ensure_lib_path_exists(&self, path: &[u8]) {
        let path = [&self.default_library_path[..], b"/", path].concat();
        if self.fs_from_file_map().read_file(&path).is_none() {
            self.fs.default_libs.add(&path);
            if let Err(error) = self
                .fs_from_file_map()
                .write_file(&path, TSC_DEFAULT_LIB_CONTENT.as_bytes())
            {
                panic!("Failed to write default library file: {error}");
            }
        }
    }

    /// The writer every console write of the fake system goes to.
    pub fn current_write(&self) -> &Arc<StringBuilder> {
        &self.current_write
    }

    // port: tsc/internal/execute/tsctests/sys.go:TestSys.writeHeaderToBaseline
    fn write_header_to_baseline(
        &self,
        builder: &StringBuilder,
        program: &tsr_incremental::Program,
    ) {
        if !builder.is_empty() {
            builder.write_string(b"\n");
        }

        let config_file_path = &program.options().config_file_path;
        if !config_file_path.is_empty() {
            builder.write_string(&tsr_tspath::relative_from_directory(
                &self.cwd,
                config_file_path.as_bytes(),
                self.get_current_directory(),
                self.fs().use_case_sensitive_file_names(),
            ));
            builder.write_string(b"::\n");
        }
    }

    // port: tsc/internal/execute/tsctests/sys.go:TestSys.baselinePrograms
    pub fn baseline_programs(&self, baseline: &mut Vec<u8>, header: &str) -> Vec<u8> {
        baseline.extend_from_slice(&self.program_baselines.string());
        self.program_baselines.reset();
        let mut result = Vec::new();
        if !self.program_include_baselines.is_empty() {
            result.extend_from_slice(
                format!(
                    "\n\n{header}\n!!! Include reasons expectations don't match pls review!!!\n"
                )
                .as_bytes(),
            );
            result.extend_from_slice(&self.program_include_baselines.string());
            self.program_include_baselines.reset();
            baseline.extend_from_slice(&result);
        }
        result
    }

    // port: tsc/internal/execute/tsctests/sys.go:TestSys.serializeState
    pub fn serialize_state(&self, baseline: &mut Vec<u8>) {
        self.baseline_output(baseline);
        self.baseline_fs_with_diff(baseline);
        // todo watch
        // this.serializeWatches(baseline);
        // this.timeoutCallbacks.serialize(baseline);
        // this.immediateCallbacks.serialize(baseline);
        // this.pendingInstalls.serialize(baseline);
        // this.service?.baseline();
    }

    // port: tsc/internal/execute/tsctests/sys.go:TestSys.baselineOutput
    fn baseline_output(&self, baseline: &mut Vec<u8>) {
        baseline.extend_from_slice(b"\nOutput::\n");
        let output = self.get_output(false);
        baseline.extend_from_slice(&output);
    }

    // port: tsc/internal/execute/tsctests/sys.go:TestSys.getOutput
    pub fn get_output(&self, for_comparing: bool) -> Vec<u8> {
        sanitize_output(&self.current_write.string(), for_comparing)
    }

    // port: tsc/internal/execute/tsctests/sys.go:TestSys.clearOutput
    pub fn clear_output(&self) {
        self.current_write.reset();
        self.tracer.reset();
    }

    // port: tsc/internal/execute/tsctests/sys.go:TestSys.baselineFSwithDiff
    pub fn baseline_fs_with_diff(&self, baseline: &mut Vec<u8>) {
        self.fs_differ.baseline_fs_with_diff(baseline);
    }

    // port: tsc/internal/execute/tsctests/sys.go:TestSys.writeFileNoError
    pub fn write_file_no_error(&self, path: &[u8], content: &[u8]) {
        if let Err(error) = self.fs_from_file_map().write_file(path, content) {
            panic!("{error}");
        }
    }

    // port: tsc/internal/execute/tsctests/sys.go:TestSys.removeNoError
    pub fn remove_no_error(&self, path: &[u8]) {
        if let Err(error) = self.fs_from_file_map().remove(path) {
            panic!("{error}");
        }
    }

    // port: tsc/internal/execute/tsctests/sys.go:TestSys.readFileNoError
    pub fn read_file_no_error(&self, path: &[u8]) -> Vec<u8> {
        match self.fs_from_file_map().read_file(path) {
            Some(content) => content,
            None => panic!("File not found: {}", String::from_utf8_lossy(path)),
        }
    }

    // port: tsc/internal/execute/tsctests/sys.go:TestSys.renameFileNoError
    pub fn rename_file_no_error(&self, old_path: &[u8], new_path: &[u8]) {
        self.write_file_no_error(new_path, &self.read_file_no_error(old_path));
        self.remove_no_error(old_path);
    }

    // port: tsc/internal/execute/tsctests/sys.go:TestSys.replaceFileText
    pub fn replace_file_text(&self, path: &[u8], old_text: &[u8], new_text: &[u8]) {
        let content = self.read_file_no_error(path);
        let content = crate::goutil::replace(&content, old_text, new_text, 1);
        self.write_file_no_error(path, &content);
    }

    // port: tsc/internal/execute/tsctests/sys.go:TestSys.replaceFileTextAll
    pub fn replace_file_text_all(&self, path: &[u8], old_text: &[u8], new_text: &[u8]) {
        let content = self.read_file_no_error(path);
        let content = replace_all(&content, old_text, new_text);
        self.write_file_no_error(path, &content);
    }

    // port: tsc/internal/execute/tsctests/sys.go:TestSys.appendFile
    pub fn append_file(&self, path: &[u8], text: &[u8]) {
        let content = self.read_file_no_error(path);
        self.write_file_no_error(path, &[&content[..], text].concat());
    }

    // port: tsc/internal/execute/tsctests/sys.go:TestSys.prependFile
    pub fn prepend_file(&self, path: &[u8], text: &[u8]) {
        let content = self.read_file_no_error(path);
        self.write_file_no_error(path, &[text, &content[..]].concat());
    }

    /// `sys.FS().Chtimes(path, time.Time{}, sys.Now())`, the one modification
    /// time an edit sets.
    pub fn touch(&self, path: &[u8]) {
        let now = System::now(self);
        if let Err(error) = self.fs().change_times(path, Time::ZERO, now) {
            panic!("{error}");
        }
    }
}

impl System for TestSys {
    // port: tsc/internal/execute/tsctests/sys.go:TestSys.Now
    fn now(&self) -> Time {
        self.clock.now()
    }

    // port: tsc/internal/execute/tsctests/sys.go:TestSys.SinceStart
    fn since_start(&self) -> Duration {
        self.clock.since_start()
    }

    // port: tsc/internal/execute/tsctests/sys.go:TestSys.FS
    fn fs(&self) -> Arc<dyn FileSystem> {
        self.fs.clone()
    }

    // port: tsc/internal/execute/tsctests/sys.go:TestSys.DefaultLibraryPath
    fn default_library_path(&self) -> &[u8] {
        &self.default_library_path
    }

    // port: tsc/internal/execute/tsctests/sys.go:TestSys.GetCurrentDirectory
    fn get_current_directory(&self) -> &[u8] {
        &self.cwd
    }

    // port: tsc/internal/execute/tsctests/sys.go:TestSys.Writer
    fn writer(&self) -> SharedWriter {
        self.current_write.clone()
    }

    // port: tsc/internal/execute/tsctests/sys.go:TestSys.ErrorWriter
    fn error_writer(&self) -> SharedWriter {
        self.current_write.clone()
    }

    // port: tsc/internal/execute/tsctests/sys.go:TestSys.WriteOutputIsTTY
    fn write_output_is_tty(&self) -> bool {
        self.output_is_tty
    }

    /// # Panics
    /// When `TS_TEST_TERMINAL_WIDTH` is set and is not an integer
    /// (`core.Must(strconv.Atoi(...))`).
    // port: tsc/internal/execute/tsctests/sys.go:TestSys.GetWidthOfTerminal
    fn get_width_of_terminal(&self) -> i64 {
        if let Some(width_str) = self.get_environment_variable("TS_TEST_TERMINAL_WIDTH") {
            if !width_str.is_empty() {
                return atoi(width_str.as_bytes());
            }
        }
        0
    }

    // port: tsc/internal/execute/tsctests/sys.go:TestSys.GetEnvironmentVariable
    fn get_environment_variable(&self, name: &str) -> Option<JsString> {
        self.env
            .get(name)
            .map(|value| JsString::from_bytes(value.as_bytes()))
    }
}

/// `core.Must(strconv.Atoi(text))`: a decimal integer with an optional sign.
fn atoi(text: &[u8]) -> i64 {
    let invalid = || -> ! {
        panic!(
            "strconv.Atoi: parsing {}: invalid syntax",
            tsr_vfs::iofs::go_quote(text)
        )
    };
    let (negative, digits) = match text {
        [b'-', rest @ ..] => (true, rest),
        [b'+', rest @ ..] => (false, rest),
        _ => (false, text),
    };
    if digits.is_empty() || !digits.iter().all(u8::is_ascii_digit) {
        invalid();
    }
    let mut value: i64 = 0;
    for &digit in digits {
        value = value
            .checked_mul(10)
            .and_then(|value| {
                let digit = i64::from(digit - b'0');
                if negative {
                    value.checked_sub(digit)
                } else {
                    value.checked_add(digit)
                }
            })
            .unwrap_or_else(|| {
                panic!(
                    "strconv.Atoi: parsing {}: value out of range",
                    tsr_vfs::iofs::go_quote(text)
                )
            });
    }
    value
}

impl Spawner for TestSys {
    /// Spawn serves the fake content mappers in-process, selecting the
    /// implementation by the exec command the mapper package declares (see
    /// `tsr_contentmappertest`), so tests exercise the full IPC stack without
    /// spawning a subprocess.
    // port: tsc/internal/execute/tsctests/sys.go:TestSys.Spawn
    fn spawn(
        &self,
        command: &[JsString],
        dir: &[u8],
        stderr: Box<dyn std::io::Write + Send>,
    ) -> Result<Stream, SpawnError> {
        tsr_contentmappertest::new_spawner().spawn(command, dir, stderr)
    }
}

const FAKE_TIME_STAMP: &str = "HH:MM:SS AM";
const FAKE_DURATION: &str = "d.ddds";

const BUILD_STARTING_AT: &str = "build starting at ";
const BUILD_FINISHED_IN: &str = "build finished in ";
pub const LIST_FILE_START: &str = "!!! List files start";
pub const LIST_FILE_END: &str = "!!! List files end";
pub const STATISTICS_START: &str = "!!! Statistics start";
pub const STATISTICS_END: &str = "!!! Statistics end";
pub const BUILD_STATUS_REPORT_START: &str = "!!! Build Status Report Start";
pub const BUILD_STATUS_REPORT_END: &str = "!!! Build Status Report End";
pub const WATCH_STATUS_REPORT_START: &str = "!!! Watch Status Report Start";
pub const WATCH_STATUS_REPORT_END: &str = "!!! Watch Status Report End";
pub const TRACE_START: &str = "!!! Trace start";
pub const TRACE_END: &str = "!!! Trace end";

/// `fmt.Fprintln(w, line)`.
fn println(w: &dyn Writer, line: &str) {
    write_all(w, format!("{line}\n").as_bytes());
}

impl CommandLineTesting for TestSys {
    // port: tsc/internal/execute/tsctests/sys.go:TestSys.OnEmittedFiles
    fn on_emitted_files(
        &self,
        result: Option<&EmitResult>,
        m_times_cache: Option<&SyncMap<JsString, Time>>,
    ) {
        let Some(result) = result else {
            return;
        };
        for file in &result.emitted_files {
            let mod_time = self.map_fs().get_mod_time(file.as_bytes());
            if let Some(serialized_diff) = self.fs_differ.serialized_diff() {
                if serialized_diff
                    .snap
                    .get(file.as_bytes())
                    .is_some_and(|diff| diff.m_time == mod_time)
                {
                    // Even though written, timestamp was reverted
                    continue;
                }
            }

            // Ensure that the timestamp for emitted files is in the order
            let now = System::now(self);
            if let Err(error) = self
                .fs_from_file_map()
                .chtimes(file.as_bytes(), Time::ZERO, now)
            {
                panic!(
                    "Failed to change time for emitted file: {}: {error}",
                    String::from_utf8_lossy(file.as_bytes())
                );
            }
            // Update the mTime cache in --b mode to store the updated timestamp so tests will behave deteministically when finding newest output
            if let Some(m_times_cache) = m_times_cache {
                let path = tsr_tspath::to_path(
                    file.as_bytes(),
                    self.get_current_directory(),
                    self.fs().use_case_sensitive_file_names(),
                );
                if m_times_cache.load(&path).is_some() {
                    m_times_cache.store(path, now);
                }
            }
        }
    }

    // port: tsc/internal/execute/tsctests/sys.go:TestSys.OnListFilesStart
    fn on_list_files_start(&self, w: &SharedWriter) {
        println(&**w, LIST_FILE_START);
    }

    // port: tsc/internal/execute/tsctests/sys.go:TestSys.OnListFilesEnd
    fn on_list_files_end(&self, w: &SharedWriter) {
        println(&**w, LIST_FILE_END);
    }

    // port: tsc/internal/execute/tsctests/sys.go:TestSys.OnStatisticsStart
    fn on_statistics_start(&self, w: &SharedWriter) {
        println(&**w, STATISTICS_START);
    }

    // port: tsc/internal/execute/tsctests/sys.go:TestSys.OnStatisticsEnd
    fn on_statistics_end(&self, w: &SharedWriter) {
        println(&**w, STATISTICS_END);
    }

    // port: tsc/internal/execute/tsctests/sys.go:TestSys.OnBuildStatusReportStart
    fn on_build_status_report_start(&self, w: &SharedWriter) {
        println(&**w, BUILD_STATUS_REPORT_START);
    }

    // port: tsc/internal/execute/tsctests/sys.go:TestSys.OnBuildStatusReportEnd
    fn on_build_status_report_end(&self, w: &SharedWriter) {
        println(&**w, BUILD_STATUS_REPORT_END);
    }

    // port: tsc/internal/execute/tsctests/sys.go:TestSys.OnWatchStatusReportStart
    fn on_watch_status_report_start(&self) {
        println(&*self.writer(), WATCH_STATUS_REPORT_START);
    }

    // port: tsc/internal/execute/tsctests/sys.go:TestSys.OnWatchStatusReportEnd
    fn on_watch_status_report_end(&self) {
        println(&*self.writer(), WATCH_STATUS_REPORT_END);
    }

    // port: tsc/internal/execute/tsctests/sys.go:TestSys.GetTrace
    fn get_trace(&self, w: SharedWriter, locale: Locale) -> Trace {
        // The pin's callback captures the system; this one the tracer.
        let tracer = self.tracer.clone();
        let sys_writer: SharedWriter = self.current_write.clone();
        Box::new(move |msg: &Message, args: &[Argument]| {
            println(&*w, TRACE_START);
            // With tsc -b building projects in parallel we cannot serialize the package.json lookup trace
            // so trace as if it wasnt cached
            let text = msg.localize(&locale, args);
            tracer.trace_with_writer(&*w, &text, tsc::same_writer(&w, &sys_writer));
            println(&*w, TRACE_END);
        })
    }

    // port: tsc/internal/execute/tsctests/sys.go:TestSys.OnProgram
    fn on_program(&self, program: &tsr_incremental::Program) {
        self.write_header_to_baseline(&self.program_baselines, program);

        let testing_data = program
            .get_testing_data()
            .expect("OnProgram is called with a testing program");
        let checked = program.get_program();
        let compiler_program = checked.program();
        self.program_baselines
            .write_string(b"SemanticDiagnostics::\n");
        for file in compiler_program.files() {
            let (path, file_name) = program_view::path_and_file_name(file);
            if let Some(diagnostics) = program_view::semantic_diagnostics_identity(
                &testing_data.semantic_diagnostics_per_file,
                &path,
            ) {
                let old_diagnostics = program_view::semantic_diagnostics_identity(
                    &testing_data.old_program_semantic_diagnostics_per_file,
                    &path,
                );
                if old_diagnostics != Some(diagnostics) {
                    self.program_baselines.write_string(b"*refresh*    ");
                    self.program_baselines.write_string(file_name.as_bytes());
                    self.program_baselines.write_string(b"\n");
                }
            } else {
                self.program_baselines.write_string(b"*not cached* ");
                self.program_baselines.write_string(file_name.as_bytes());
                self.program_baselines.write_string(b"\n");
            }
        }

        // Write signature updates
        self.program_baselines.write_string(b"Signatures::\n");
        let updated_signature_kinds = testing_data
            .updated_signature_kinds
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        for file in compiler_program.files() {
            let (path, file_name) = program_view::path_and_file_name(file);
            if let Some(kind) = updated_signature_kinds.get(&path) {
                let label: &[u8] = match kind {
                    SignatureUpdateKind::ComputedDts => b"(computed .d.ts) ",
                    SignatureUpdateKind::StoredAtEmit => b"(stored at emit) ",
                    SignatureUpdateKind::UsedVersion => b"(used version)   ",
                };
                self.program_baselines.write_string(label);
                self.program_baselines.write_string(file_name.as_bytes());
                self.program_baselines.write_string(b"\n");
            }
        }

        let mut files_without_include_reason = Vec::new();
        let mut file_not_in_program_with_include_reason = Vec::new();
        let include_reasons = program_view::include_reason_paths(compiler_program);
        for file in compiler_program.files() {
            let (path, _) = program_view::path_and_file_name(file);
            if !include_reasons.contains(&path) {
                files_without_include_reason.push(path);
            }
        }
        for path in &include_reasons {
            if compiler_program.file(path.as_bytes()).is_none()
                && !program_view::is_missing_path(compiler_program, path)
            {
                file_not_in_program_with_include_reason.push(path.clone());
            }
        }
        if !files_without_include_reason.is_empty()
            || !file_not_in_program_with_include_reason.is_empty()
        {
            self.write_header_to_baseline(&self.program_include_baselines, program);
            self.program_include_baselines.write_string(
                b"!!! Expected all files to have include reasons\nfilesWithoutIncludeReason::\n",
            );
            for file in &files_without_include_reason {
                self.program_include_baselines.write_string(b"  ");
                self.program_include_baselines.write_string(file.as_bytes());
                self.program_include_baselines.write_string(b"\n");
            }
            self.program_include_baselines
                .write_string(b"filesNotInProgramWithIncludeReason::\n");
            for file in &file_not_in_program_with_include_reason {
                self.program_include_baselines.write_string(b"  ");
                self.program_include_baselines.write_string(file.as_bytes());
                self.program_include_baselines.write_string(b"\n");
            }
        }
    }

    fn as_with_watch_backend(&self) -> Option<&dyn CommandLineTestingWithWatchBackend> {
        Some(self)
    }
}

impl CommandLineTestingWithWatchBackend for TestSys {
    // port: tsc/internal/execute/tsctests/sys.go:TestSys.WatchBackend
    fn watch_backend(&self) -> Arc<dyn WatchBackend> {
        self.mock_watch_backend.clone()
    }
}

/// The version strings the sanitizer replaces.
struct VersionStrings {
    quoted: Vec<u8>,
    quoted_fake: Vec<u8>,
    english: Vec<u8>,
    fake_english: Vec<u8>,
    czech_text: Vec<u8>,
    fake_czech: Vec<u8>,
}

fn version_strings() -> &'static VersionStrings {
    static STRINGS: OnceLock<VersionStrings> = OnceLock::new();
    STRINGS.get_or_init(|| {
        let version = Argument::Bytes(tsr_core::version().as_bytes().to_vec());
        let fake = Argument::Bytes(FAKE_TS_VERSION.as_bytes().to_vec());
        let message = tsr_diagnostics::Version_0;
        let czech = Locale::parse("cs").0;
        VersionStrings {
            quoted: format!("'{}'", tsr_core::version()).into_bytes(),
            quoted_fake: format!("'{FAKE_TS_VERSION}'").into_bytes(),
            english: message.localize(&tsr_locale::DEFAULT, std::slice::from_ref(&version)),
            fake_english: message.localize(&tsr_locale::DEFAULT, std::slice::from_ref(&fake)),
            czech_text: message.localize(&czech, std::slice::from_ref(&version)),
            fake_czech: message.localize(&czech, std::slice::from_ref(&fake)),
        }
    })
}

/// `outputSanitizer`.
struct OutputSanitizer<'l> {
    for_comparing: bool,
    lines: Vec<&'l [u8]>,
    index: usize,
    output_lines: Vec<Vec<u8>>,
}

/// The sanitized console output: `getOutput` on `text`.
pub fn sanitize_output(text: &[u8], for_comparing: bool) -> Vec<u8> {
    let lines: Vec<&[u8]> = text.split(|&b| b == b'\n').collect();
    let mut transformer = OutputSanitizer {
        for_comparing,
        output_lines: Vec::with_capacity(lines.len()),
        lines,
        index: 0,
    };
    transformer.transform_lines()
}

impl OutputSanitizer<'_> {
    // port: tsc/internal/execute/tsctests/sys.go:outputSanitizer.addOutputLine
    fn add_output_line(&mut self, s: &[u8]) {
        let strings = version_strings();
        let s = replace_all(s, &strings.quoted, &strings.quoted_fake);
        let s = replace_all(&s, &strings.english, &strings.fake_english);
        let s = replace_all(&s, &strings.czech_text, &strings.fake_czech);
        let s = sanitize_internal_symbol_name(&s).into_owned();
        self.output_lines.push(s);
    }

    /// # Panics
    /// When the line has no `:` at or after its third byte, or is too short
    /// for the timestamp, as the pin's slicing does.
    // port: tsc/internal/execute/tsctests/sys.go:outputSanitizer.sanitizeBuildStatusTimeStamp
    fn sanitize_build_status_time_stamp(&self) -> Vec<u8> {
        let status_line = self.lines[self.index];
        let hh_separator = status_line.iter().position(|&b| b == b':');
        let hh_separator = match hh_separator {
            Some(at) if at >= 2 => at,
            _ => panic!("Expected timestamp"),
        };
        let rest = hh_separator + FAKE_TIME_STAMP.len() - 2;
        assert!(
            rest <= status_line.len(),
            "runtime error: slice bounds out of range [{rest}:{}]",
            status_line.len()
        );
        [
            &status_line[..hh_separator - 2],
            FAKE_TIME_STAMP.as_bytes(),
            &status_line[rest..],
        ]
        .concat()
    }

    // port: tsc/internal/execute/tsctests/sys.go:outputSanitizer.transformLines
    fn transform_lines(&mut self) -> Vec<u8> {
        while self.index < self.lines.len() {
            let line = self.lines[self.index];
            if line.starts_with(BUILD_STARTING_AT.as_bytes()) {
                if !self.for_comparing {
                    self.add_output_line(
                        format!("{BUILD_STARTING_AT}{FAKE_TIME_STAMP}").as_bytes(),
                    );
                }
                self.index += 1;
                continue;
            }
            if line.starts_with(BUILD_FINISHED_IN.as_bytes()) {
                if !self.for_comparing {
                    self.add_output_line(format!("{BUILD_FINISHED_IN}{FAKE_DURATION}").as_bytes());
                }
                self.index += 1;
                continue;
            }
            if !self.add_or_skip_lines_for_comparing(LIST_FILE_START, LIST_FILE_END, false, false)
                && !self.add_or_skip_lines_for_comparing(
                    STATISTICS_START,
                    STATISTICS_END,
                    true,
                    false,
                )
                && !self.add_or_skip_lines_for_comparing(TRACE_START, TRACE_END, false, false)
                && !self.add_or_skip_lines_for_comparing(
                    BUILD_STATUS_REPORT_START,
                    BUILD_STATUS_REPORT_END,
                    false,
                    true,
                )
                && !self.add_or_skip_lines_for_comparing(
                    WATCH_STATUS_REPORT_START,
                    WATCH_STATUS_REPORT_END,
                    false,
                    true,
                )
            {
                self.add_output_line(line);
            }
            self.index += 1;
        }
        self.output_lines.join(&b'\n')
    }

    /// `sanitize_first_line` is the pin's `sanitizeBuildStatusTimeStamp`
    /// argument (the only function it passes).
    ///
    /// # Panics
    /// When the block has no end line.
    // port: tsc/internal/execute/tsctests/sys.go:outputSanitizer.addOrSkipLinesForComparing
    fn add_or_skip_lines_for_comparing(
        &mut self,
        line_start: &str,
        line_end: &str,
        skip_even_if_not_comparing: bool,
        sanitize_first_line: bool,
    ) -> bool {
        if self.lines[self.index] != line_start.as_bytes() {
            return false;
        }
        self.index += 1;
        let mut is_first_line = true;
        while self.index < self.lines.len() {
            if self.lines[self.index] == line_end.as_bytes() {
                return true;
            }
            if !self.for_comparing && !skip_even_if_not_comparing {
                let mut line = self.lines[self.index].to_vec();
                if is_first_line && sanitize_first_line {
                    line = self.sanitize_build_status_time_stamp();
                    is_first_line = false;
                }
                self.add_output_line(&line);
            }
            self.index += 1;
        }
        panic!("Expected lineEnd{line_end} not found after {line_start}");
    }
}
