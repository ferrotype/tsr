//! The file system and time stamps the incremental program writes through
//! (`host.go`), and the compiler host it reads build info and default
//! libraries with.
use std::sync::Arc;
use tsr_vfs::iofs::Time;
use tsr_vfs::FileSystem;

/// What the incremental package reads of the pin's `compiler.CompilerHost`:
/// its file system, current directory, default library directory and
/// content-mapper project.
pub trait CompilerHost: Send + Sync {
    fn fs(&self) -> &dyn FileSystem;
    fn default_library_path(&self) -> &[u8];
    fn get_current_directory(&self) -> &[u8];
    fn content_mapper_project(&self) -> Option<&Arc<dyn tsr_contentmapper::Project>>;
}

/// The [`CompilerHost`] a program was loaded with: its file system, current
/// directory, default library directory and content-mapper project.
pub struct ProgramCompilerHost {
    program: Arc<tsr_compiler::Program>,
}

impl ProgramCompilerHost {
    pub fn new(program: Arc<tsr_compiler::Program>) -> Self {
        Self { program }
    }
}

impl CompilerHost for ProgramCompilerHost {
    fn fs(&self) -> &dyn FileSystem {
        self.program.host()
    }
    fn default_library_path(&self) -> &[u8] {
        self.program.default_library_path()
    }
    fn get_current_directory(&self) -> &[u8] {
        self.program.current_directory()
    }
    fn content_mapper_project(&self) -> Option<&Arc<dyn tsr_contentmapper::Project>> {
        self.program.content_mapper_project()
    }
}

/// `Host`: the file system the incremental program writes outputs through
/// and the time stamps it keeps for `--build`.
pub trait Host: Send + Sync {
    fn fs(&self) -> &dyn FileSystem;
    fn get_mtime(&self, file_name: &[u8]) -> Time;
    fn set_mtime(&self, file_name: &[u8], m_time: Time) -> Result<(), tsr_vfs::Error>;
}

struct HostImpl {
    host: Arc<dyn CompilerHost>,
}

impl Host for HostImpl {
    // port: tsc/internal/execute/incremental/host.go:host.FS
    fn fs(&self) -> &dyn FileSystem {
        self.host.fs()
    }

    // port: tsc/internal/execute/incremental/host.go:host.GetMTime
    fn get_mtime(&self, file_name: &[u8]) -> Time {
        get_mtime(self.host.as_ref(), file_name)
    }

    // port: tsc/internal/execute/incremental/host.go:host.SetMTime
    fn set_mtime(&self, file_name: &[u8], m_time: Time) -> Result<(), tsr_vfs::Error> {
        self.host.fs().change_times(file_name, Time::ZERO, m_time)
    }
}

// port: tsc/internal/execute/incremental/host.go:CreateHost
pub fn create_host(compiler_host: Arc<dyn CompilerHost>) -> Arc<dyn Host> {
    Arc::new(HostImpl {
        host: compiler_host,
    })
}

/// The file's modification time, the zero time when it does not exist.
// port: tsc/internal/execute/incremental/host.go:GetMTime
pub fn get_mtime(host: &dyn CompilerHost, file_name: &[u8]) -> Time {
    let stat = host.fs().stat(file_name).ok().flatten();
    let mut m_time = Time::ZERO;
    if let Some(stat) = stat {
        m_time = stat.mod_time;
    }
    m_time
}
