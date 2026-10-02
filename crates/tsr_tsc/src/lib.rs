//! Shared command-line contracts and compilation helpers from Go's
//! `execute/tsc` package. Both the driver and build orchestrator depend on
//! this layer; keeping it separate avoids a driver/build dependency cycle.

mod colors;
pub mod diagnostics;
pub mod emit;
pub mod help;
pub mod init;
pub use diagnostics::*;
pub use emit::*;

/// `execute/tsc`: the system, the exit statuses, the result and the testing
/// hooks.
pub mod tsc {
    use super::watchmanager;
    use std::sync::Arc;
    use std::time::Duration;
    use tsr_compiler::EmitResult;
    use tsr_core::collections::SyncMap;
    use tsr_diagnostics::{Argument, Message};
    use tsr_jsstring::JsString;
    use tsr_locale::Locale;
    use tsr_vfs::iofs::Time;
    use tsr_vfs::FileSystem;

    /// Go's `io.Writer` as the command line shares it: one writer is handed
    /// to reporters, build tasks and hooks at once, so it is written through a
    /// shared reference, and the pin compares writers by identity
    /// (`w == sys.Writer()`), which [`same_writer`] does.
    pub trait Writer: Send + Sync {
        fn write(&self, bytes: &[u8]) -> std::io::Result<usize>;
    }

    /// A shared [`Writer`] (Go's `io.Writer` interface value).
    pub type SharedWriter = Arc<dyn Writer>;

    /// `w1 == w2` on two `io.Writer` interface values: the same writer.
    pub fn same_writer(a: &SharedWriter, b: &SharedWriter) -> bool {
        Arc::ptr_eq(a, b)
    }

    /// `fmt.Fprint(w, ...)`: the pin ignores the error of every console
    /// write.
    pub fn write_all(w: &dyn Writer, mut bytes: &[u8]) {
        while !bytes.is_empty() {
            match w.write(bytes) {
                Ok(0) | Err(_) => return,
                Ok(n) => bytes = &bytes[n..],
            }
        }
    }

    /// `tsc.System`: the command line's view of the host. ADR 0014: nothing
    /// below it reads the environment, the clock or the terminal directly.
    ///
    /// The pin's eleven methods: `Spawn` is the supertrait's
    /// [`tsr_contentmapper::Spawner::spawn`] (the pin hands the system itself
    /// to the content-mapper host as its spawner), the other ten are below.
    #[derive(Clone, Copy, Debug, Default)]
    pub struct MemoryStatistics {
        pub allocated_bytes: u64,
        pub allocation_count: u64,
        pub live_bytes: u64,
    }

    pub trait System: tsr_contentmapper::Spawner {
        /// Process allocator counters, when supplied by a native host. A fake
        /// clock/filesystem host need not pretend to own the global allocator.
        fn memory_statistics(&self) -> Option<MemoryStatistics> {
            None
        }

        fn writer(&self) -> SharedWriter;
        fn error_writer(&self) -> SharedWriter;
        /// `FS() vfs.FS`.
        fn fs(&self) -> Arc<dyn FileSystem>;
        fn default_library_path(&self) -> &[u8];
        fn get_current_directory(&self) -> &[u8];
        fn write_output_is_tty(&self) -> bool;
        /// Go's `int`.
        fn get_width_of_terminal(&self) -> i64;
        /// `(value, ok)`: `None` when the variable is not set.
        fn get_environment_variable(&self, name: &str) -> Option<JsString>;
        fn now(&self) -> Time;
        fn since_start(&self) -> Duration;
        /// Rust's filesystem timestamp retains an instant without a location.
        /// Fake systems use UTC; OS systems override this with local time to
        /// preserve the pin's `sys.Now().Format("03:04:05 PM")`.
        fn format_watch_time(&self, time: Time) -> Vec<u8> {
            crate::diagnostics::format_watch_time(time)
        }
    }

    /// `tsc.ExitStatus`: an open integer type, so a value outside the six
    /// constants stays representable (the runner reports it).
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
    pub struct ExitStatus(pub i32);

    #[allow(non_upper_case_globals)] // the pin's constant names
    impl ExitStatus {
        pub const Success: Self = Self(0);
        pub const DiagnosticsPresent_OutputsSkipped: Self = Self(1);
        pub const DiagnosticsPresent_OutputsGenerated: Self = Self(2);
        pub const InvalidProject_OutputsSkipped: Self = Self(3);
        pub const ProjectReferenceCycle_OutputsSkipped: Self = Self(4);
        pub const NotImplemented: Self = Self(5);
    }

    /// `tsc.Watcher`: a watch session the runner drives one cycle at a time.
    pub trait Watcher: Send + Sync + std::any::Any {
        /// Returns a compilation failure or named refusal without unwinding.
        /// Callers end the watch session after an error; a failed cycle does
        /// not promise a state that can be retried.
        fn do_cycle(&self) -> Result<(), tsr_compiler::Error>;
    }

    /// `tsc.CommandLineResult`.
    #[derive(Clone)]
    pub struct CommandLineResult {
        pub status: ExitStatus,
        pub watcher: Option<Arc<dyn Watcher>>,
    }

    /// The `func(msg *diagnostics.Message, args ...any)` trace callback.
    pub type Trace = Box<dyn Fn(&Message, &[Argument]) + Send + Sync>;

    /// `tsc.CommandLineTesting`: the eleven hooks the fake system implements.
    pub trait CommandLineTesting: Send + Sync {
        /// Ensure that all emitted files are timestamped in order to ensure
        /// they are deterministic for test baseline. `None` is a nil result or
        /// a nil cache.
        fn on_emitted_files(
            &self,
            result: Option<&EmitResult>,
            m_times_cache: Option<&SyncMap<JsString, Time>>,
        );
        fn on_list_files_start(&self, w: &SharedWriter);
        fn on_list_files_end(&self, w: &SharedWriter);
        fn on_statistics_start(&self, w: &SharedWriter);
        fn on_statistics_end(&self, w: &SharedWriter);
        fn on_build_status_report_start(&self, w: &SharedWriter);
        fn on_build_status_report_end(&self, w: &SharedWriter);
        fn on_watch_status_report_start(&self);
        fn on_watch_status_report_end(&self);
        fn get_trace(&self, w: SharedWriter, locale: Locale) -> Trace;
        fn on_program(&self, program: &tsr_incremental::Program);

        /// Passive ownership instrumentation shared by every compilation path.
        /// The default does not retain the program or change baseline output.
        fn on_compiler_program(&self, _program: &tsr_compiler::CheckedProgram) {}

        /// The pin's type assertion
        /// `testing.(watchmanager.CommandLineTestingWithWatchBackend)`.
        fn as_with_watch_backend(
            &self,
        ) -> Option<&dyn watchmanager::CommandLineTestingWithWatchBackend> {
            None
        }
    }
}

/// Native filesystem watcher contracts, shared with the test backend.
pub use tsr_fswatch as fswatch;
pub mod watcher;
pub mod watchmanager;

pub use tsc::*;

/// port: tsc/internal/execute/tsc/emit.go:GetTraceWithWriterFromSys
pub fn get_trace_with_writer_from_sys(
    writer: SharedWriter,
    locale: tsr_locale::Locale,
    testing: Option<&dyn CommandLineTesting>,
) -> Trace {
    if let Some(testing) = testing {
        return testing.get_trace(writer, locale);
    }
    Box::new(move |message, args| {
        write_all(writer.as_ref(), &message.localize(&locale, args));
        write_all(writer.as_ref(), b"\n");
    })
}

/// Flush the loader's ordered per-file traces through the CLI's testable writer.
pub fn report_resolution_trace(program: &tsr_compiler::Program, trace: &Trace) {
    for item in program.trace() {
        let args: Vec<_> = item
            .args
            .iter()
            .map(|arg| match arg {
                tsr_module::TraceArg::Text(text) => {
                    tsr_diagnostics::Argument::Bytes(text.as_bytes().to_vec())
                }
                tsr_module::TraceArg::Bool(value) => tsr_diagnostics::Argument::Bool(*value),
            })
            .collect();
        trace(item.message, &args);
    }
}

pub mod statistics;
pub use statistics::Statistics;

pub mod tracing;
pub use tracing::{start_tracing_if_needed, stop_tracing};
