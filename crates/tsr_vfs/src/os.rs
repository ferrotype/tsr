//! Explicit OS acquisition into a complete immutable directory snapshot.
//! There is no implicit disk fallback from an in-memory program host.
use crate::{Error, MemoryBuilder, MemorySnapshot};
use std::path::{Path, PathBuf};

pub struct ScopedOsFs {
    root: PathBuf,
    case_sensitive: bool,
}
impl ScopedOsFs {
    pub fn new(root: impl AsRef<Path>, case_sensitive: bool) -> Result<Self, Error> {
        let root = std::fs::canonicalize(root).map_err(|e| Error::Io(e.kind()))?;
        if !root.is_dir() {
            return Err(Error::InvalidPath);
        }
        Ok(Self {
            root,
            case_sensitive,
        })
    }
    /// Capture every entry in the declared root, retaining symlink spelling. Link
    /// targets outside the root fail explicitly instead of expanding the scope.
    pub fn snapshot(&self) -> Result<MemorySnapshot, Error> {
        let root = path_bytes(&self.root)?;
        let mut builder = MemoryBuilder::new(&root, self.case_sensitive);
        self.capture(&self.root, &mut builder)?;
        Ok(builder.finish())
    }
    fn capture(&self, path: &Path, builder: &mut MemoryBuilder) -> Result<(), Error> {
        let bytes = path_bytes(path)?;
        let metadata = std::fs::symlink_metadata(path).map_err(|e| Error::Io(e.kind()))?;
        if metadata.file_type().is_symlink() {
            let target = std::fs::canonicalize(path).map_err(|e| Error::Io(e.kind()))?;
            if !target.starts_with(&self.root) {
                return Err(Error::OutsideScope);
            }
            builder.insert_symlink(&bytes, &path_bytes(&target)?);
        } else if metadata.is_dir() {
            builder.insert_directory(&bytes);
            let mut entries = std::fs::read_dir(path)
                .map_err(|e| Error::Io(e.kind()))?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| Error::Io(e.kind()))?;
            entries.sort_by_key(std::fs::DirEntry::file_name);
            for entry in entries {
                self.capture(&entry.path(), builder)?;
            }
        } else if metadata.is_file() {
            builder.insert_physical(
                &bytes,
                std::fs::read(path).map_err(|e| Error::Io(e.kind()))?,
            );
        } else {
            return Err(Error::Unsupported("non-regular OS entry"));
        }
        Ok(())
    }
}
#[cfg(unix)]
// Keep one fallible interface: non-Unix paths can fail UTF-8 validation.
#[allow(clippy::unnecessary_wraps)]
fn path_bytes(path: &Path) -> Result<Vec<u8>, Error> {
    use std::os::unix::ffi::OsStrExt;
    Ok(path.as_os_str().as_bytes().to_vec())
}
#[cfg(not(unix))]
fn path_bytes(path: &Path) -> Result<Vec<u8>, Error> {
    path.to_str()
        .map(|p| p.replace('\\', "/").into_bytes())
        .ok_or(Error::InvalidPath)
}
