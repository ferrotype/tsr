//! The command-line entry the runner calls, and the pinned interfaces it is
//! called through: `execute.CommandLine`, `tsc.System`, `tsc.ExitStatus`,
//! `tsc.CommandLineResult` with its `tsc.Watcher`, the testing hooks
//! `tsc.CommandLineTesting`, and the watch backend the hooks may supply
//! (`watchmanager.WatchBackend`, with the `fswatch` event types it carries).
//!
//! Phase 4 X1 defines these in `tsr_execute` (and X4/X5 the watch types in
//! `tsr_fswatch` and `tsr_execute::watchmanager`). They are shaped here as the
//! pin's are so those crates can replace this module without a change to the
//! harness: the runner and the fake system use only the names below.
//!
//! Until X1, [`command_line`] refuses with the named operation
//! [`COMMAND_LINE_OPERATION`]: it raises an [`Unsupported`] payload, which the
//! binary records as an `unsupported` row. The pin's `CommandLine` returns no
//! error, so a refusal cannot travel in its result; the payload is the one
//! channel that keeps the signature exact, and X1 can use the same
//! [`unsupported`] for any operation it has not ported yet.
use std::sync::Arc;
use tsr_ipc::Context;
use tsr_jsstring::JsString;

/// The operation [`command_line`] refuses until Phase 4 X1.
pub const COMMAND_LINE_OPERATION: &str = "execute.CommandLine is Phase 4 X1";

/// A named refusal: an operation the Rust command line does not implement
/// yet. Raised with [`unsupported`] and classified by the binary as an
/// `unsupported` row (never as a failure).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Unsupported {
    pub operation: String,
}

/// Refuses `operation` by unwinding with an [`Unsupported`] payload.
pub fn unsupported(operation: &str) -> ! {
    std::panic::panic_any(Unsupported {
        operation: operation.to_owned(),
    })
}

/// `execute.CommandLine`: a build flag is honored only as the first argument,
/// then `tscBuildCompilation` or `tscCompilation`. `testing` is the pin's
/// `tsc.CommandLineTesting`, nil outside tests.
///
/// Phase 4 X1 supplies the body (the pin's
/// `tsc/internal/execute/tsc.go:CommandLine`); until then every call refuses
/// with [`COMMAND_LINE_OPERATION`].
pub fn command_line(
    ctx: &Context,
    sys: Arc<dyn tsc::System>,
    command_line_args: &[JsString],
    testing: Option<Arc<dyn tsc::CommandLineTesting>>,
) -> tsc::CommandLineResult {
    let _ = (ctx, sys, command_line_args, testing);
    unsupported(COMMAND_LINE_OPERATION)
}

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
    pub trait System: tsr_contentmapper::Spawner {
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
    pub trait Watcher: Send + Sync {
        fn do_cycle(&self);
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

        /// The pin's type assertion
        /// `testing.(watchmanager.CommandLineTestingWithWatchBackend)`.
        fn as_with_watch_backend(
            &self,
        ) -> Option<&dyn watchmanager::CommandLineTestingWithWatchBackend> {
            None
        }
    }
}

/// The part of `fswatch` the watch backend carries: events, their kinds, the
/// callback and the overflow error. X4's `tsr_fswatch` replaces it.
pub mod fswatch {
    use std::sync::Arc;

    /// `fswatch.EventKind`.
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
    pub struct EventKind(pub i32);

    #[allow(non_upper_case_globals)] // the pin's constant names
    impl EventKind {
        pub const EventUpdate: Self = Self(1);
        pub const EventDelete: Self = Self(2);
    }

    /// The pin's `EventKind.String` (X4 ports it in `tsr_fswatch`).
    impl std::fmt::Display for EventKind {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str(match *self {
                Self::EventUpdate => "update",
                Self::EventDelete => "delete",
                _ => "unknown",
            })
        }
    }

    /// `fswatch.Event`: the exported fields (the pin's `includedWatchRoot` is
    /// the backend's own).
    #[derive(Clone, Debug, PartialEq, Eq)]
    pub struct Event {
        pub kind: EventKind,
        pub path: Vec<u8>,
    }

    /// The errors a watch callback or a watch request reports.
    #[derive(Clone, Debug, PartialEq, Eq)]
    pub enum Error {
        /// `fswatch.ErrOverflow`.
        Overflow,
        /// An error built from a sentence (`fmt.Errorf`).
        Message(String),
    }

    impl std::fmt::Display for Error {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            match self {
                Self::Overflow => f.write_str("fswatch: event overflow; some changes were missed"),
                Self::Message(text) => f.write_str(text),
            }
        }
    }

    impl std::error::Error for Error {}

    /// `fswatch.WatchCallback`: `func(events []Event, err error)`.
    pub type WatchCallback = Arc<dyn Fn(&[Event], Option<&Error>) + Send + Sync>;
}

/// The part of `execute/watchmanager` a test system supplies: the backend
/// abstraction and the optional testing extension that hands it over.
pub mod watchmanager {
    use super::fswatch;
    use std::sync::Arc;

    /// Go's `io.Closer` for a registered watch.
    pub trait Closer: Send + Sync {
        fn close(&self) -> Result<(), fswatch::Error>;
    }

    /// The `func(string) bool` ignore predicate of a watch.
    pub type Ignore = Arc<dyn Fn(&[u8]) -> bool + Send + Sync>;

    /// `watchmanager.WatchDirectoryRequest`.
    #[derive(Clone)]
    pub struct WatchDirectoryRequest {
        pub dir: Vec<u8>,
        pub callback: fswatch::WatchCallback,
        pub recursive: bool,
        pub ignore: Option<Ignore>,
    }

    /// `watchmanager.WatchBackend`: abstracts `fswatch.Watcher` for testing.
    pub trait WatchBackend: Send + Sync {
        fn watch_directory(
            &self,
            dir: &[u8],
            callback: fswatch::WatchCallback,
            recursive: bool,
            ignore: Option<Ignore>,
        ) -> Result<Arc<dyn Closer>, fswatch::Error>;
        fn watch_directories(
            &self,
            requests: Vec<WatchDirectoryRequest>,
        ) -> Result<Vec<Arc<dyn Closer>>, fswatch::Error>;
    }

    /// `watchmanager.CommandLineTestingWithWatchBackend`: an optional
    /// extension of `CommandLineTesting` that supplies a [`WatchBackend`] for
    /// test mode.
    pub trait CommandLineTestingWithWatchBackend: Send + Sync {
        fn watch_backend(&self) -> Arc<dyn WatchBackend>;
    }
}
