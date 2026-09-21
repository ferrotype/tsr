//! Go's `testing/fstest.MapFS`: a filesystem that is a map from name to file.
//!
//! Directories need not be listed; they are synthesized from the names beneath
//! them with mode `dir | 0555`. Relative symbolic links are followed on open;
//! an absolute target does not resolve.
use super::{valid_path, FileMode, Fs, Handle, Info, IoError, Sys, Time};
use std::{collections::BTreeMap, sync::Arc};

/// Source type: testing/fstest.MapFile
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MapFile {
    /// File content, or a symbolic link's destination.
    pub data: Arc<[u8]>,
    pub mode: FileMode,
    pub mod_time: Time,
    pub sys: Sys,
}
impl MapFile {
    fn info(&self, name: &[u8]) -> Info {
        Info {
            name: base(name).to_vec(),
            size: self.data.len() as u64,
            mode: self.mode,
            mod_time: self.mod_time,
            sys: self.sys.clone(),
        }
    }
}
fn synthesized_directory() -> MapFile {
    MapFile {
        mode: FileMode::DIR | FileMode(0o555),
        ..MapFile::default()
    }
}

/// Source type: testing/fstest.MapFS
#[derive(Clone, Debug, Default)]
pub struct MapFs(pub BTreeMap<Vec<u8>, Arc<MapFile>>);
impl MapFs {
    pub fn get(&self, name: &[u8]) -> Option<&Arc<MapFile>> {
        self.0.get(name)
    }
    /// Source operation: testing/fstest.MapFS.resolveSymlinks
    fn resolve_symlinks(&self, name: &[u8]) -> Option<Vec<u8>> {
        let link_target = |at: &[u8]| {
            self.0
                .get(at)
                .filter(|file| file.mode.file_type() == FileMode::SYMLINK)
                .map(|file| file.data.clone())
        };
        if let Some(target) = link_target(name) {
            if target.starts_with(b"/") {
                return None;
            }
            return self.resolve_symlinks(&clean_join(dir(name), &target));
        }
        let mut index = 0;
        while index < name.len() {
            let (directory, next) = match name[index..].iter().position(|&b| b == b'/') {
                Some(offset) => (&name[..index + offset], index + offset),
                None => (name, name.len()),
            };
            if let Some(target) = link_target(directory) {
                if target.starts_with(b"/") {
                    return None;
                }
                let mut joined = clean_join(dir(directory), &target);
                joined.extend_from_slice(&name[next..]);
                return self.resolve_symlinks(&joined);
            }
            index = next + 1;
        }
        valid_path(name).then(|| name.to_vec())
    }
}
impl Fs for MapFs {
    /// Source operation: testing/fstest.MapFS.Open
    fn open(&self, name: &[u8]) -> Result<Handle, IoError> {
        let missing = || IoError::path("open", name, IoError::NotExist);
        if !valid_path(name) {
            return Err(missing());
        }
        let real = self.resolve_symlinks(name).ok_or_else(missing)?;
        let file = self.0.get(&real);
        if let Some(file) = file.filter(|file| !file.mode.is_dir()) {
            return Ok(Handle::file(name, file.info(name), file.data.clone()));
        }
        // A directory: listed explicitly, or implied by the names beneath it.
        let prefix = if real == b"." {
            Vec::new()
        } else {
            [real.as_slice(), b"/"].concat()
        };
        let mut listed: BTreeMap<Vec<u8>, Info> = BTreeMap::new();
        let mut implied: Vec<Vec<u8>> = Vec::new();
        for (full, entry) in &self.0 {
            let Some(rest) = full.strip_prefix(prefix.as_slice()) else {
                continue;
            };
            match rest.iter().position(|&b| b == b'/') {
                None if full.as_slice() != b"." && !rest.is_empty() => {
                    listed.insert(rest.to_vec(), entry.info(rest));
                }
                None => {}
                Some(end) => implied.push(rest[..end].to_vec()),
            }
        }
        if real != b"." && file.is_none() && listed.is_empty() && implied.is_empty() {
            return Err(missing());
        }
        for child in implied {
            listed
                .entry(child.clone())
                .or_insert_with(|| synthesized_directory().info(&child));
        }
        let own = file.map_or_else(synthesized_directory, |file| (**file).clone());
        let element: &[u8] = if name == b"." { b"." } else { base(name) };
        let mut info = own.info(element);
        info.name = element.to_vec();
        Ok(Handle::directory(
            name,
            info,
            listed.into_values().collect(),
        ))
    }
}
/// Go's `path.Base` for a clean, non-empty name.
pub(crate) fn base(name: &[u8]) -> &[u8] {
    name.rsplit(|&b| b == b'/').next().unwrap_or(name)
}
/// Go's `path.Dir` for a clean name: `.` when there is no directory part.
pub(crate) fn dir(name: &[u8]) -> &[u8] {
    match name.iter().rposition(|&b| b == b'/') {
        Some(0) => b"/",
        Some(index) => &name[..index],
        None => b".",
    }
}
/// Go's `path.Join(dir, target)`: joined, then lexically cleaned.
pub(crate) fn clean_join(directory: &[u8], target: &[u8]) -> Vec<u8> {
    let rooted = directory.starts_with(b"/");
    let mut parts: Vec<&[u8]> = Vec::new();
    for part in directory
        .split(|&b| b == b'/')
        .chain(target.split(|&b| b == b'/'))
    {
        match part {
            b"" | b"." => {}
            b".." => {
                if parts.last().is_some_and(|last| *last != b"..") {
                    parts.pop();
                } else if !rooted {
                    parts.push(part);
                }
            }
            _ => parts.push(part),
        }
    }
    let joined = parts.join(&b'/');
    match (rooted, joined.is_empty()) {
        (true, _) => [b"/".as_slice(), &joined].concat(),
        (false, true) => b".".to_vec(),
        (false, false) => joined,
    }
}
