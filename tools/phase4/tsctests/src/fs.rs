//! `fs.go`: the file system the command line sees in a test. It remembers
//! which files the command wrote, forgets a default library once something
//! reads it, keeps build infos at the fake version on disk (and at the real
//! one when the command reads them back), and writes a readable rendering
//! beside every build info.
use crate::fsbaselineutil::{sanitize_internal_symbol_name, NilableSyncSet, SyncSet};
use crate::harnessutil::FAKE_TS_VERSION;
use crate::readablebuildinfo::to_readable_build_info;
use std::sync::Arc;
use tsr_incremental::BuildInfo;
use tsr_jsstring::JsString;
use tsr_vfs::iofs::{IoError, Time};
use tsr_vfs::iovfs::IoVfs;
use tsr_vfs::vfstest::TestFs as MapFs;
use tsr_vfs::{Entries, Error, FileContent, FileInfo, FileSystem, SnapshotId, WalkCallback};

const EXTENSION_TS_BUILD_INFO: &[u8] = b".tsbuildinfo";

/// `testFs`: the embedded `vfs.FS` with the harness's bookkeeping.
pub struct TestFs {
    /// The embedded `vfs.FS`.
    pub fs: Arc<IoVfs<MapFs>>,
    pub default_libs: Arc<NilableSyncSet>,
    pub written_files: Arc<SyncSet>,
}

fn message(text: String) -> Error {
    Error::from(IoError::Message { text, source: None })
}

fn unmarshal_build_info(data: &[u8]) -> Result<BuildInfo, tsr_json::Error> {
    let mut build_info = BuildInfo::default();
    tsr_json::unmarshal(data, &mut build_info, tsr_json::Options::default())?;
    Ok(build_info)
}

impl TestFs {
    pub fn new(fs: Arc<IoVfs<MapFs>>) -> Self {
        Self {
            fs,
            default_libs: Arc::new(NilableSyncSet::default()),
            written_files: Arc::new(SyncSet::default()),
        }
    }

    // port: tsc/internal/execute/tsctests/fs.go:testFs.removeIgnoreLibPath
    fn remove_ignore_lib_path(&self, path: &[u8]) {
        if self.default_libs.has(path) == Some(true) {
            self.default_libs.delete(path);
        }
    }

    // port: tsc/internal/execute/tsctests/fs.go:testFs.readFileHandlingBuildInfo
    fn read_file_handling_build_info(&self, path: &[u8]) -> Result<Option<FileContent>, Error> {
        let contents = FileSystem::read_file(&*self.fs, path)?;
        let Some(contents) = contents else {
            return Ok(None);
        };
        if tsr_tspath::file_extension_is(path, EXTENSION_TS_BUILD_INFO) {
            // read buildinfo and modify version
            if let Ok(mut build_info) = unmarshal_build_info(&contents.raw) {
                if build_info.version.as_bytes() == FAKE_TS_VERSION.as_bytes() {
                    build_info.version = JsString::from_bytes(tsr_core::version().as_bytes());
                    let new_contents =
                        tsr_json::marshal(&build_info, tsr_json::Options::default())
                            .unwrap_or_else(|error| {
                                panic!(
                                    "testFs.ReadFile: failed to marshal build info after fixing version: {error}"
                                )
                            });
                    return Ok(Some(FileContent::loaded(new_contents)));
                }
            }
        }
        Ok(Some(contents))
    }

    // port: tsc/internal/execute/tsctests/fs.go:testFs.writeFileHandlingBuildInfo
    fn write_file_handling_build_info(&self, path: &[u8], data: &[u8]) -> Result<(), Error> {
        let mut data = data.to_vec();
        if tsr_tspath::file_extension_is(path, EXTENSION_TS_BUILD_INFO) {
            match unmarshal_build_info(&data) {
                Ok(mut build_info) => {
                    if build_info.version.as_bytes() == tsr_core::version().as_bytes() {
                        // Change it to harnessutil.FakeTSVersion
                        build_info.version = JsString::from_bytes(FAKE_TS_VERSION.as_bytes());
                        data = tsr_json::marshal(&build_info, tsr_json::Options::default())
                            .map_err(|error| {
                                message(format!(
                                    "testFs.WriteFile: failed to marshal build info after fixing version: {error}"
                                ))
                            })?;
                    }
                    // Write readable build info version
                    let readable_path = [path, b".readable.baseline.txt"].concat();
                    let readable = to_readable_build_info(
                        &build_info,
                        &sanitize_internal_symbol_name(&data),
                    );
                    FileSystem::write_file(self, &readable_path, &readable).map_err(|error| {
                        message(format!(
                            "testFs.WriteFile: failed to write readable build info: {error}"
                        ))
                    })?;
                }
                Err(error) => panic!(
                    "testFs.WriteFile: failed to unmarshal build info: - use underlying FS's write method if this is intended use for testcase{error}"
                ),
            }
        }
        FileSystem::write_file(&*self.fs, path, &data)
    }
}

impl FileSystem for TestFs {
    fn use_case_sensitive_file_names(&self) -> bool {
        FileSystem::use_case_sensitive_file_names(&*self.fs)
    }
    fn snapshot_id(&self) -> Option<SnapshotId> {
        FileSystem::snapshot_id(&*self.fs)
    }
    /// ReadFile reads the file specified by path and returns the content.
    /// If the file fails to be read, ok will be false.
    // port: tsc/internal/execute/tsctests/fs.go:testFs.ReadFile
    fn read_file(&self, path: &[u8]) -> Result<Option<FileContent>, Error> {
        self.remove_ignore_lib_path(path);
        self.read_file_handling_build_info(path)
    }
    fn stat(&self, path: &[u8]) -> Result<Option<FileInfo>, Error> {
        FileSystem::stat(&*self.fs, path)
    }
    fn entries(&self, path: &[u8]) -> Result<Entries, Error> {
        FileSystem::entries(&*self.fs, path)
    }
    fn realpath(&self, path: &[u8]) -> Result<JsString, Error> {
        FileSystem::realpath(&*self.fs, path)
    }
    fn file_exists(&self, path: &[u8]) -> Result<bool, Error> {
        FileSystem::file_exists(&*self.fs, path)
    }
    fn directory_exists(&self, path: &[u8]) -> Result<bool, Error> {
        FileSystem::directory_exists(&*self.fs, path)
    }
    fn walk_dir(&self, root: &[u8], visit: &mut WalkCallback<'_>) -> Result<(), Error> {
        FileSystem::walk_dir(&*self.fs, root, visit)
    }
    // port: tsc/internal/execute/tsctests/fs.go:testFs.WriteFile
    fn write_file(&self, path: &[u8], data: &[u8]) -> Result<(), Error> {
        self.remove_ignore_lib_path(path);
        self.written_files.add(path);
        self.write_file_handling_build_info(path, data)
    }
    fn append_file(&self, path: &[u8], data: &[u8]) -> Result<(), Error> {
        FileSystem::append_file(&*self.fs, path, data)
    }
    /// Removes `path` and all its contents. Will return the first error it
    /// encounters.
    // port: tsc/internal/execute/tsctests/fs.go:testFs.Remove
    fn remove(&self, path: &[u8]) -> Result<(), Error> {
        self.remove_ignore_lib_path(path);
        FileSystem::remove(&*self.fs, path)
    }
    fn change_times(&self, path: &[u8], a_time: Time, m_time: Time) -> Result<(), Error> {
        FileSystem::change_times(&*self.fs, path, a_time, m_time)
    }
}
