//! io/fs.DirFS access used by Common's rooted dispatch.
use super::native::{bytes, failure, path};
use crate::iofs::{self, FileMode, Fs, Handle, Info, IoError, Sys, Time};
use std::{path::PathBuf, sync::Arc};
pub(super) struct Dir {
    pub root: PathBuf,
}
pub fn mode(metadata: &std::fs::Metadata) -> FileMode {
    #[cfg(unix)]
    let mut bits = {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o777
    };
    #[cfg(not(unix))]
    let mut bits = if metadata.permissions().readonly() {
        0o444
    } else {
        0o666
    };
    let ty = metadata.file_type();
    if ty.is_dir() {
        bits |= FileMode::DIR.0;
    } else if ty.is_symlink() {
        bits |= FileMode::SYMLINK.0;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::{FileTypeExt, MetadataExt};
        if ty.is_fifo() {
            bits |= 1 << 25;
        }
        if ty.is_socket() {
            bits |= 1 << 24;
        }
        if ty.is_block_device() || ty.is_char_device() {
            bits |= 1 << 26;
        }
        if ty.is_char_device() {
            bits |= 1 << 21;
        }
        let mode = metadata.mode();
        if mode & 0o4000 != 0 {
            bits |= 1 << 23;
        }
        if mode & 0o2000 != 0 {
            bits |= 1 << 22;
        }
        if mode & 0o1000 != 0 {
            bits |= 1 << 20;
        }
    }
    FileMode(bits)
}
pub fn info(name: &[u8], metadata: &std::fs::Metadata) -> Info {
    Info {
        name: name.to_vec(),
        size: metadata.len(),
        mode: mode(metadata),
        mod_time: metadata.modified().map_or(Time::ZERO, Time::from),
        sys: Sys::Native(Arc::new(metadata.clone())),
    }
}
impl Dir {
    fn resolve(&self, name: &[u8], op: &'static str) -> Result<PathBuf, IoError> {
        if !iofs::valid_path(name) {
            return Err(IoError::path(op, name, IoError::Invalid));
        }
        Ok(self.root.join(path(name)))
    }
}
impl Fs for Dir {
    fn stat(&self, name: &[u8]) -> Result<Arc<Info>, IoError> {
        let p = self.resolve(name, "stat")?;
        let m = std::fs::metadata(&p).map_err(|e| failure("stat", name, e))?;
        let label = p
            .file_name()
            .map_or_else(|| bytes(&p), |s| bytes(std::path::Path::new(s)));
        Ok(Arc::new(info(&label, &m)))
    }
    fn read_file(&self, name: &[u8]) -> Result<Arc<[u8]>, IoError> {
        std::fs::read(self.resolve(name, "readfile")?)
            .map(Arc::from)
            .map_err(|e| failure("read", name, e))
    }
    fn read_dir(&self, name: &[u8]) -> Result<Vec<Arc<Info>>, IoError> {
        let directory = std::fs::read_dir(self.resolve(name, "readdir")?)
            .map_err(|e| failure("open", name, e))?;
        let mut result = Vec::new();
        for entry in directory {
            let entry = entry.map_err(|e| failure("readdir", name, e))?;
            let m =
                std::fs::symlink_metadata(entry.path()).map_err(|e| failure("lstat", name, e))?;
            result.push(Arc::new(info(
                &bytes(std::path::Path::new(&entry.file_name())),
                &m,
            )));
        }
        result.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(result)
    }
    fn open(&self, name: &[u8]) -> Result<Handle, IoError> {
        let metadata = (*self.stat(name)?).clone();
        if metadata.is_dir() {
            Ok(Handle::directory(
                name,
                metadata,
                self.read_dir(name)?
                    .into_iter()
                    .map(|i| (*i).clone())
                    .collect(),
            ))
        } else {
            Ok(Handle::file(name, metadata, self.read_file(name)?))
        }
    }
}
