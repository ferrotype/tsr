use std::io::{self, IsTerminal, Write};
use std::os::unix::ffi::OsStrExt;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tsr_contentmapper::{SpawnError, Spawner};
use tsr_ipc::Stream;
use tsr_jsstring::JsString;
use tsr_tsc::{SharedWriter, System, Writer};
use tsr_vfs::{iofs::Time, FileSystem};

struct Stdout;
struct Stderr;
impl Writer for Stdout {
    fn write(&self, bytes: &[u8]) -> io::Result<usize> {
        io::stdout().lock().write(bytes)
    }
}
impl Writer for Stderr {
    fn write(&self, bytes: &[u8]) -> io::Result<usize> {
        io::stderr().lock().write(bytes)
    }
}

/// source: tsc/cmd/tsc/sys.go:osSys
pub struct OsSystem {
    fs: Arc<dyn FileSystem>,
    cwd: Vec<u8>,
    writer: SharedWriter,
    errors: SharedWriter,
    start: Instant,
}
impl OsSystem {
    /// port: tsc/cmd/tsc/sys.go:newSystem
    pub fn new() -> io::Result<Self> {
        let cwd =
            tsr_tspath::normalize(std::env::current_dir()?.as_os_str().as_bytes()).into_owned();
        Ok(Self {
            fs: Arc::new(tsr_bundled::BundledFs::new(tsr_vfs::os::shared_fs())),
            cwd,
            writer: Arc::new(Stdout),
            errors: Arc::new(Stderr),
            start: Instant::now(),
        })
    }
}
impl Spawner for OsSystem {
    /// port: tsc/cmd/tsc/sys.go:osSys.Spawn
    fn spawn(
        &self,
        command: &[JsString],
        dir: &[u8],
        stderr: Box<dyn Write + Send>,
    ) -> Result<Stream, SpawnError> {
        Ok(crate::process::spawn(command, dir, stderr)?)
    }
}
impl System for OsSystem {
    fn memory_statistics(&self) -> Option<tsr_tsc::MemoryStatistics> {
        Some(crate::allocation::snapshot())
    }
    /// port: tsc/cmd/tsc/sys.go:osSys.Writer
    fn writer(&self) -> SharedWriter {
        self.writer.clone()
    }
    /// port: tsc/cmd/tsc/sys.go:osSys.ErrorWriter
    fn error_writer(&self) -> SharedWriter {
        self.errors.clone()
    }
    /// port: tsc/cmd/tsc/sys.go:osSys.FS
    fn fs(&self) -> Arc<dyn FileSystem> {
        self.fs.clone()
    }
    /// port: tsc/cmd/tsc/sys.go:osSys.DefaultLibraryPath
    fn default_library_path(&self) -> &[u8] {
        tsr_bundled::LIB_PATH
    }
    /// port: tsc/cmd/tsc/sys.go:osSys.GetCurrentDirectory
    fn get_current_directory(&self) -> &[u8] {
        &self.cwd
    }
    /// port: tsc/cmd/tsc/sys.go:osSys.WriteOutputIsTTY
    fn write_output_is_tty(&self) -> bool {
        io::stdout().is_terminal()
    }
    /// port: tsc/cmd/tsc/sys.go:osSys.GetWidthOfTerminal
    fn get_width_of_terminal(&self) -> i64 {
        rustix::termios::tcgetwinsize(io::stdout()).map_or(0, |size| i64::from(size.ws_col))
    }
    /// port: tsc/cmd/tsc/sys.go:osSys.GetEnvironmentVariable
    fn get_environment_variable(&self, name: &str) -> Option<JsString> {
        std::env::var_os(name).map(|value| JsString::from_bytes(value.as_bytes()))
    }
    /// port: tsc/cmd/tsc/sys.go:osSys.Now
    fn now(&self) -> Time {
        Time::now()
    }
    /// port: tsc/cmd/tsc/sys.go:osSys.SinceStart
    fn since_start(&self) -> Duration {
        self.start.elapsed()
    }
    fn format_watch_time(&self, time: Time) -> Vec<u8> {
        crate::signals::local_time(time)
    }
}
