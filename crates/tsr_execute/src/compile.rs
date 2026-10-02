use crate::tsc::{
    self, CommandLineResult, CommandLineTesting, CompileTimes, DiagnosticReporter, System,
};
use std::sync::{Arc, Mutex};
use tsr_arena::Counters;
use tsr_compiler::{CheckedProgram, Error, FileCache, Program, ProgramOptions};
use tsr_contentmapper::{Host, Project};
use tsr_ipc::Context;
use tsr_jsstring::JsString;
use tsr_tsoptions::ParsedCommandLine;
use tsr_vfs::FileSystem;

/// Every project is closed before the session host on all return/panic paths.
struct MapperSession {
    host: Option<tsr_contentmapper::HostImpl>,
    project: Option<Arc<dyn Project>>,
}
impl Drop for MapperSession {
    fn drop(&mut self) {
        if let Some(project) = &self.project {
            let _ = project.close();
        }
        if let Some(host) = &self.host {
            let _ = host.close();
        }
    }
}

/// port: tsc/internal/execute/tsc/compile.go:newContentMapperLogger
fn mapper_logger(sys: &dyn System) -> Option<tsr_contentmapper::Logger> {
    if sys
        .get_environment_variable("TS_CONTENT_MAPPER_DEBUG")
        .is_none_or(|value| value.is_empty())
    {
        return None;
    }
    let writer = sys.error_writer();
    let mutex = Mutex::new(());
    Some(Arc::new(move |text| {
        let _guard = mutex
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        tsc::write_all(writer.as_ref(), text.as_bytes());
        tsc::write_all(writer.as_ref(), b"\n");
    }))
}

struct CompilationHost {
    fs: Arc<dyn FileSystem>,
    cwd: JsString,
    library: JsString,
    mapper: Option<Arc<dyn Project>>,
}
impl tsr_incremental::CompilerHost for CompilationHost {
    fn fs(&self) -> &dyn FileSystem {
        self.fs.as_ref()
    }
    fn get_current_directory(&self) -> &[u8] {
        self.cwd.as_bytes()
    }
    fn default_library_path(&self) -> &[u8] {
        self.library.as_bytes()
    }
    fn content_mapper_project(&self) -> Option<&Arc<dyn Project>> {
        self.mapper.as_ref()
    }
}

/// port: tsc/internal/execute/tsc.go:performCompilation
/// port: tsc/internal/execute/tsc.go:performIncrementalCompilation
pub(crate) fn perform_compilation(
    ctx: &Context,
    sys: Arc<dyn System>,
    config: ParsedCommandLine,
    report: DiagnosticReporter,
    mut times: CompileTimes,
    testing: Option<Arc<dyn CommandLineTesting>>,
) -> Result<CommandLineResult, Error> {
    let incremental = config.options.is_incremental();
    let mapper_host = config.options.run_external_code.is_true().then(|| {
        tsr_contentmapper::new_host_with_options(
            ctx,
            sys.clone(),
            config.locale().clone(),
            tsr_contentmapper::HostOptions {
                logger: mapper_logger(sys.as_ref()),
            },
        )
    });
    let project = mapper_host
        .as_ref()
        .filter(|_| {
            config
                .content_mappers
                .as_ref()
                .is_some_and(|mappers| !mappers.is_empty())
        })
        .and_then(|host| {
            host.project(tsr_contentmapper::ProjectSpec {
                config_file_name: config.config_name(),
                mappers: config.content_mappers.clone().unwrap_or_default().into(),
                compiler_options: Arc::new(config.options.clone()),
            })
        });
    let session = MapperSession {
        host: mapper_host,
        project,
    };
    let cached = Arc::new(tsr_vfs::cached::CachedFs::new(sys.fs()));
    let host: Arc<dyn tsr_incremental::CompilerHost> = Arc::new(CompilationHost {
        fs: cached.clone(),
        cwd: JsString::from_bytes(sys.get_current_directory()),
        library: JsString::from_bytes(sys.default_library_path()),
        mapper: session.project.clone(),
    });
    let old_program = if incremental {
        let start = sys.now();
        let reader = tsr_incremental::new_build_info_reader(host.clone());
        let old = tsr_incremental::read_build_info_program(&config, reader.as_ref(), host.as_ref());
        times.build_info_read_time = tsc::elapsed(sys.now(), start);
        old
    } else {
        None
    };
    let tracing = tsc::start_tracing_if_needed(sys.as_ref(), &config, testing.as_deref());
    let start = sys.now();
    let counters = Counters::new();
    let mut cache = FileCache::new();
    let program = Arc::new(Program::load_live_with_content_mapper_project_and_tracing(
        ProgramOptions {
            config: config.clone(),
            host: cached,
            current_directory: JsString::from_bytes(sys.get_current_directory()),
            default_library_path: JsString::from_bytes(sys.default_library_path()),
            skip_module_resolution: false,
            single_threaded: tsr_core::Tristate::UNKNOWN,
        },
        session.project.clone(),
        &mut cache,
        &counters,
        tracing
            .clone()
            .map(|tracing| tracing as Arc<dyn tsr_checker::TraceSink>),
    )?);
    tsc::report_resolution_trace(
        &program,
        &tsc::get_trace_with_writer_from_sys(
            sys.writer(),
            config.locale().clone(),
            testing.as_deref(),
        ),
    );
    let program = Arc::new(CheckedProgram::new(program, &counters, None));
    times.parse_time = tsc::elapsed(sys.now(), start);
    if let Some(host) = &session.host {
        times.content_mapper_times = host.timings();
    }
    let incremental_program = if incremental {
        let start = sys.now();
        // An affine mapping keeps the existing nested-emit clock interface
        // while observing exactly one system-clock reading per invocation.
        let anchor = std::time::Instant::now();
        let clock_sys = sys.clone();
        let clock = Arc::new(move || {
            let (seconds, nanos) = clock_sys.now().unix();
            let whole = std::time::Duration::from_secs(seconds.unsigned_abs());
            let value = if seconds < 0 {
                anchor.checked_sub(whole)
            } else {
                anchor.checked_add(whole)
            };
            value
                .and_then(|value| {
                    value.checked_add(std::time::Duration::from_nanos(u64::from(nanos)))
                })
                .expect("system clock is representable by nested emit clock")
        });
        let program = tsr_incremental::new_program(
            program.clone(),
            old_program.as_ref(),
            tsr_incremental::create_host(host),
            Some(clock),
            testing.is_some(),
        )?;
        times.changes_compute_time = tsc::elapsed(sys.now(), start);
        Some(program)
    } else {
        None
    };
    let program_like: &dyn tsr_compiler::ProgramLike = incremental_program.as_ref().map_or(
        program.as_ref() as &dyn tsr_compiler::ProgramLike,
        |value| value,
    );
    let result = tsc::emit_and_report_statistics(tsc::EmitInput {
        sys: sys.as_ref(),
        program_like,
        program: &program,
        incremental: incremental_program.as_ref(),
        config: &config,
        report_diagnostic: &report,
        writer: None,
        skip_error_summary: false,
        write_file: None,
        testing_m_times_cache: None,
        times: &mut times,
        testing: testing.as_deref(),
    })?;
    tsc::stop_tracing(sys.as_ref(), tracing.as_deref(), &program);
    if let (Some(testing), Some(program)) = (&testing, &incremental_program) {
        testing.on_program(program);
    }
    Ok(CommandLineResult {
        status: result.status,
        watcher: None,
    })
}

#[cfg(test)]
mod logger_tests {
    use super::*;
    use crate::tsc::{SharedWriter, Writer};
    use std::time::Duration;
    use tsr_vfs::iofs::Time;

    #[derive(Default)]
    struct Buffer(Mutex<Vec<u8>>);
    impl Writer for Buffer {
        fn write(&self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }
    }
    struct LoggingSystem {
        enabled: bool,
        stderr: Arc<Buffer>,
    }
    impl tsr_contentmapper::Spawner for LoggingSystem {
        fn spawn(
            &self,
            _: &[JsString],
            _: &[u8],
            _: Box<dyn std::io::Write + Send>,
        ) -> Result<tsr_ipc::Stream, tsr_contentmapper::SpawnError> {
            unreachable!("logger does not spawn")
        }
    }
    impl System for LoggingSystem {
        fn writer(&self) -> SharedWriter {
            unreachable!("logger uses stderr")
        }
        fn error_writer(&self) -> SharedWriter {
            self.stderr.clone()
        }
        fn fs(&self) -> Arc<dyn FileSystem> {
            unreachable!("logger does not read files")
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
        fn get_environment_variable(&self, name: &str) -> Option<JsString> {
            (self.enabled && name == "TS_CONTENT_MAPPER_DEBUG")
                .then(|| JsString::from_bytes(b"1".as_slice()))
        }
        fn now(&self) -> Time {
            Time::ZERO
        }
        fn since_start(&self) -> Duration {
            Duration::ZERO
        }
    }

    #[test]
    // port: tsc/internal/execute/tsc/emit_test.go:TestContentMapperLoggerEnvironmentVariable
    fn content_mapper_logger_environment_variable() {
        let mut sys = LoggingSystem {
            enabled: false,
            stderr: Arc::default(),
        };
        assert!(mapper_logger(&sys).is_none());
        sys.enabled = true;
        let logger = mapper_logger(&sys).expect("debug logger");
        std::thread::scope(|scope| {
            for _ in 0..10 {
                let logger = &logger;
                scope.spawn(move || logger("mapper log".into()));
            }
        });
        assert_eq!(*sys.stderr.0.lock().unwrap(), b"mapper log\n".repeat(10));
    }
}
