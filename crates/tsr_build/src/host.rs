use crate::*;
use tsr_core::collections::SyncMap;
use tsr_vfs::{cached::CachedFs, FileSystem};

pub(crate) struct BuildHost {
    pub fs: Arc<CachedFs>,
    pub cwd: JsString,
    pub library: JsString,
    pub case_sensitive: bool,
    pub m_times: SyncMap<JsString, Time>,
}
impl BuildHost {
    pub fn new(sys: &dyn System) -> Self {
        let fs = sys.fs();
        Self {
            case_sensitive: fs.use_case_sensitive_file_names(),
            fs: Arc::new(CachedFs::new(fs)),
            cwd: JsString::from_bytes(sys.get_current_directory()),
            library: JsString::from_bytes(sys.default_library_path()),
            m_times: SyncMap::default(),
        }
    }
    pub fn path(&self, name: &[u8]) -> JsString {
        tsr_tspath::to_path(name, self.cwd.as_bytes(), self.case_sensitive)
    }
    pub fn m_time(&self, name: &[u8]) -> Time {
        let path = self.path(name);
        if let Some(time) = self.m_times.load(&path) {
            return time;
        }
        let time = self
            .fs
            .stat(name)
            .ok()
            .flatten()
            .map_or(Time::ZERO, |info| info.mod_time);
        self.m_times.load_or_store(path, time).0
    }
    // port: tsc/internal/execute/build/host.go:host.storeMTime
    pub fn store_m_time(&self, name: &[u8], time: Time) {
        self.m_times.store(self.path(name), time);
    }
}
impl tsr_incremental::Host for BuildHost {
    // port: tsc/internal/execute/build/host.go:host.FS
    fn fs(&self) -> &dyn FileSystem {
        self.fs.as_ref()
    }
    // port: tsc/internal/execute/build/host.go:host.GetMTime
    fn get_mtime(&self, name: &[u8]) -> Time {
        self.m_time(name)
    }
    // port: tsc/internal/execute/build/host.go:host.SetMTime
    fn set_mtime(&self, name: &[u8], time: Time) -> Result<(), tsr_vfs::Error> {
        self.fs.change_times(name, Time::ZERO, time)
    }
}

pub(crate) struct CompilerHost {
    pub host: Arc<BuildHost>,
    pub project: Option<Arc<dyn tsr_contentmapper::Project>>,
}
impl tsr_incremental::CompilerHost for CompilerHost {
    // port: tsc/internal/execute/build/compilerHost.go:compilerHost.FS
    fn fs(&self) -> &dyn FileSystem {
        self.host.fs.as_ref()
    }
    // port: tsc/internal/execute/build/compilerHost.go:compilerHost.DefaultLibraryPath
    fn default_library_path(&self) -> &[u8] {
        self.host.library.as_bytes()
    }
    // port: tsc/internal/execute/build/compilerHost.go:compilerHost.GetCurrentDirectory
    fn get_current_directory(&self) -> &[u8] {
        self.host.cwd.as_bytes()
    }
    // port: tsc/internal/execute/build/compilerHost.go:compilerHost.ContentMapperProject
    fn content_mapper_project(&self) -> Option<&Arc<dyn tsr_contentmapper::Project>> {
        self.project.as_ref()
    }
}
