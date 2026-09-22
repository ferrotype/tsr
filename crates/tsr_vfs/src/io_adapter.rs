//! Bridge the source-compatible I/O adapter to the compiler's host boundary.
//! Structured I/O errors retain their path and wrapping chain.
use crate::{
    iofs::{Info, Time, WalkError},
    iovfs::{Backing, IoVfs},
    Entries, Error, FileContent, FileInfo, FileSystem, SnapshotId, WalkCallback, WalkControl,
    WalkEntry,
};
use tsr_jsstring::JsString;

impl From<&Info> for FileInfo {
    fn from(info: &Info) -> Self {
        Self {
            directory: info.is_dir(),
            size: info.size,
            name: JsString::from_bytes(info.name.as_slice()),
            mod_time: info.mod_time,
            mode: info.mode,
            sys: info.sys.clone(),
        }
    }
}
impl<B: Backing + 'static> FileSystem for IoVfs<B> {
    fn snapshot_id(&self) -> Option<SnapshotId> {
        None
    }
    fn use_case_sensitive_file_names(&self) -> bool {
        Self::use_case_sensitive_file_names(self)
    }
    fn file_exists(&self, path: &[u8]) -> Result<bool, Error> {
        Ok(Self::file_exists(self, path))
    }
    fn directory_exists(&self, path: &[u8]) -> Result<bool, Error> {
        Ok(Self::directory_exists(self, path))
    }
    fn read_file(&self, path: &[u8]) -> Result<Option<FileContent>, Error> {
        Ok(Self::read_file(self, path).map(FileContent::loaded))
    }
    fn stat(&self, path: &[u8]) -> Result<Option<FileInfo>, Error> {
        Ok(Self::stat(self, path).map(|info| FileInfo::from(&*info)))
    }
    fn entries(&self, path: &[u8]) -> Result<Entries, Error> {
        Ok(self.get_accessible_entries(path))
    }
    fn realpath(&self, path: &[u8]) -> Result<JsString, Error> {
        Ok(JsString::from_bytes(Self::realpath(self, path)))
    }
    fn write_file(&self, path: &[u8], data: &[u8]) -> Result<(), Error> {
        Self::write_file(self, path, data).map_err(Error::from)
    }
    fn append_file(&self, path: &[u8], data: &[u8]) -> Result<(), Error> {
        Self::append_file(self, path, data).map_err(Error::from)
    }
    fn remove(&self, path: &[u8]) -> Result<(), Error> {
        Self::remove(self, path).map_err(Error::from)
    }
    fn change_times(&self, path: &[u8], a_time: Time, m_time: Time) -> Result<(), Error> {
        self.chtimes(path, a_time, m_time).map_err(Error::from)
    }
    fn walk_dir(&self, path: &[u8], visit: &mut WalkCallback<'_>) -> Result<(), Error> {
        // Preserve an arbitrary compiler callback error across the lower-level
        // walk boundary, without converting it to an unrelated I/O category.
        let mut callback_error = None;
        let result = Self::walk_dir(self, path, &mut |name, info, error| {
            let entry = info.map(|info| WalkEntry {
                name: JsString::from_bytes(info.name.as_slice()),
                info: FileInfo::from(info),
                symlink: info.mode.is_symlink(),
            });
            match visit(name, entry.as_ref(), error.map(Error::from)) {
                Ok(WalkControl::Continue) => Ok(()),
                Ok(WalkControl::SkipDir) => Err(WalkError::SkipDir),
                Ok(WalkControl::SkipAll) => Err(WalkError::SkipAll),
                Err(error) => {
                    callback_error = Some(error);
                    Err(WalkError::SkipAll)
                }
            }
        });
        if let Some(error) = callback_error {
            return Err(error);
        }
        result.map_err(Error::from)
    }
}
