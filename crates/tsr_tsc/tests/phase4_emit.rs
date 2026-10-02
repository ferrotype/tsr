use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tsr_compiler::{CheckedProgram, FileCache, Program, ProgramOptions};
use tsr_core::{CompilerOptions, ModuleKind, Tristate};
use tsr_jsstring::JsString;
use tsr_tsc::{CompileTimes, EmitInput, SharedWriter, System, Writer};
use tsr_tsoptions::ParsedCommandLine;
use tsr_vfs::{
    iofs::Time, Entries, FileContent, FileInfo, FileSystem, MemoryBuilder, MemorySnapshot,
};

struct Files(Mutex<MemoryBuilder>);
impl Files {
    fn snapshot(&self) -> MemorySnapshot {
        self.0.lock().unwrap().clone().finish()
    }
}
impl FileSystem for Files {
    fn snapshot_id(&self) -> Option<tsr_vfs::SnapshotId> {
        None
    }
    fn use_case_sensitive_file_names(&self) -> bool {
        true
    }
    fn read_file(&self, name: &[u8]) -> Result<Option<FileContent>, tsr_vfs::Error> {
        self.snapshot().read_file(name)
    }
    fn stat(&self, name: &[u8]) -> Result<Option<FileInfo>, tsr_vfs::Error> {
        self.snapshot().stat(name)
    }
    fn entries(&self, name: &[u8]) -> Result<Entries, tsr_vfs::Error> {
        self.snapshot().entries(name)
    }
    fn realpath(&self, name: &[u8]) -> Result<JsString, tsr_vfs::Error> {
        self.snapshot().realpath(name)
    }
    fn write_file(&self, name: &[u8], text: &[u8]) -> Result<(), tsr_vfs::Error> {
        self.0.lock().unwrap().insert_loaded(name, text);
        Ok(())
    }
}
#[derive(Default)]
struct Clock {
    seconds: u64,
    nested_emit_in_progress: bool,
    nested_emit_calls: usize,
}
struct Discard;
impl Writer for Discard {
    fn write(&self, bytes: &[u8]) -> std::io::Result<usize> {
        Ok(bytes.len())
    }
}
struct TimingSystem {
    fs: Arc<Files>,
    clock: Arc<Mutex<Clock>>,
}
impl tsr_contentmapper::Spawner for TimingSystem {
    fn spawn(
        &self,
        _: &[JsString],
        _: &[u8],
        _: Box<dyn std::io::Write + Send>,
    ) -> Result<tsr_ipc::Stream, tsr_contentmapper::SpawnError> {
        unreachable!("no content mappers in this program")
    }
}
impl System for TimingSystem {
    fn writer(&self) -> SharedWriter {
        Arc::new(Discard)
    }
    fn error_writer(&self) -> SharedWriter {
        Arc::new(Discard)
    }
    fn fs(&self) -> Arc<dyn FileSystem> {
        self.fs.clone()
    }
    fn default_library_path(&self) -> &[u8] {
        b"/lib"
    }
    fn get_current_directory(&self) -> &[u8] {
        b"/project"
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
        Time::from_unix(self.clock.lock().unwrap().seconds as i64, 0)
    }
    fn since_start(&self) -> Duration {
        Duration::ZERO
    }
}
fn js(value: &str) -> JsString {
    JsString::from_bytes(value.as_bytes())
}

#[test]
// port: tsc/internal/execute/tsc/emit_test.go:TestIncrementalDeclarationEmitTimeIsExcludedFromCheckTime
fn incremental_declaration_emit_time_is_excluded_from_check_time() {
    let mut files = MemoryBuilder::new(b"/project", true);
    files.insert_loaded(b"/lib/lib.d.ts",b"interface Array<T> {} interface Boolean {} interface CallableFunction {} interface Function {} interface IArguments {} interface NewableFunction {} interface Number {} interface Object {} interface RegExp {} interface String {}".as_slice());
    let hub=b"export interface Box { value: string; } export const make = (): Box => ({ value: \"ok\" });";
    files.insert_loaded(b"/project/hub.ts", hub.as_slice());
    files.insert_loaded(
        b"/project/spoke.ts",
        b"import { make, type Box } from \"./hub\"; export const value: Box = make();".as_slice(),
    );
    let sys = TimingSystem {
        fs: Arc::new(Files(Mutex::new(files))),
        clock: Arc::default(),
    };
    let config = ParsedCommandLine::new(
        CompilerOptions {
            declaration: Tristate::TRUE,
            incremental: Tristate::TRUE,
            module: ModuleKind::ESNEXT,
            no_emit: Tristate::TRUE,
            ts_build_info_file: js("/project/tsconfig.tsbuildinfo"),
            ..Default::default()
        },
        vec![
            js("/lib/lib.d.ts"),
            js("/project/hub.ts"),
            js("/project/spoke.ts"),
        ],
    );
    let anchor = Instant::now();
    let nested_clock = {
        let clock = sys.clock.clone();
        Arc::new(move || {
            let mut clock = clock.lock().unwrap();
            clock.nested_emit_calls += 1;
            if clock.nested_emit_in_progress {
                clock.seconds += 1;
            }
            clock.nested_emit_in_progress = !clock.nested_emit_in_progress;
            anchor + Duration::from_secs(clock.seconds)
        })
    };
    let compile = |old: Option<&tsr_incremental::Program>| {
        let counters = tsr_arena::Counters::new();
        let loaded = Arc::new(
            Program::load_live(
                ProgramOptions {
                    config: config.clone(),
                    host: sys.fs(),
                    current_directory: js("/project"),
                    default_library_path: js("/lib"),
                    skip_module_resolution: false,
                    single_threaded: Tristate::UNKNOWN,
                },
                &mut FileCache::new(),
                &counters,
            )
            .unwrap(),
        );
        assert!(loaded.source_file(b"/lib/lib.d.ts").is_some());
        // The exact native fixture omits the effective default library. It is
        // retained as missing, without refusing the otherwise usable program.
        assert_eq!(loaded.missing_files(), &[js("/lib/lib.es2025.full.d.ts")]);
        let host = tsr_incremental::create_host(Arc::new(
            tsr_incremental::ProgramCompilerHost::new(loaded.clone()),
        ));
        let checked = Arc::new(CheckedProgram::new(loaded, &counters, None));
        let incremental = tsr_incremental::new_program(
            checked.clone(),
            old,
            host,
            Some(nested_clock.clone()),
            false,
        )
        .unwrap();
        let report = tsr_tsc::create_diagnostic_reporter(
            &sys,
            sys.writer(),
            tsr_locale::Locale::default(),
            &CompilerOptions {
                quiet: Tristate::TRUE,
                ..Default::default()
            },
        );
        let write_file = |file_name: &[u8], text: &[u8], _: &mut tsr_compiler::WriteFileData| {
            sys.fs.write_file(file_name, text)
        };
        let mut times = CompileTimes::default();
        tsr_tsc::emit_files_and_report_errors(EmitInput {
            sys: &sys,
            program_like: &incremental,
            program: &checked,
            incremental: Some(&incremental),
            config: &config,
            report_diagnostic: &report,
            times: &mut times,
            testing: None,
            writer: None,
            skip_error_summary: true,
            write_file: Some(&write_file),
            testing_m_times_cache: None,
        })
        .unwrap();
        (incremental, times)
    };
    let (old, _) = compile(None);
    sys.fs
        .write_file(
            b"/project/hub.ts",
            &[hub.as_slice(), b"\n// comment only change\n"].concat(),
        )
        .unwrap();
    let (_, times) = compile(Some(&old));
    assert_eq!(times.check_time, Duration::ZERO);
    assert_eq!(times.emit_time, Duration::from_secs(2));
    assert_eq!(sys.clock.lock().unwrap().nested_emit_calls, 4);
}
