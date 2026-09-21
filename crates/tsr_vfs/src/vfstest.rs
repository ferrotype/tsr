//! The in-memory test filesystem of `vfs/vfstest`: a `MapFs` with absolute
//! symbolic links, optional case-insensitivity, intermediate directories, a
//! clock for modification times, and mutation.
//!
//! Entries are keyed by canonical path and remember the spelling they were
//! created with. A stored entry is immutable: every mutation, `chtimes`
//! included, replaces it, so a handle obtained earlier never changes under its
//! holder. The pin assigns the modification time through its stored pointer
//! instead; that one aliasing leak is deliberately not reproduced.
use crate::iofs::{FileMode, Fs, Handle, Info, IoError, MapFile, MapFs, Sys, Time};
use std::{
    collections::BTreeMap,
    sync::{Arc, RwLock},
};
use tsr_tspath as path;

const UMASK: u32 = 0o022;

/// Source type: tsc/internal/vfs/vfstest/vfstest.go:Clock
pub trait Clock: Send + Sync {
    fn now(&self) -> Time;
}
struct SystemClock;
impl Clock for SystemClock {
    fn now(&self) -> Time {
        Time::now()
    }
}

/// What a caller may put in the input map.
#[derive(Clone, Debug)]
pub enum InputFile {
    Text(Vec<u8>),
    Bytes(Vec<u8>),
    /// Copied; its modification time is replaced by the clock's.
    File(MapFile),
}
/// port: tsc/internal/vfs/vfstest/vfstest.go:Symlink
pub fn symlink(target: &[u8]) -> MapFile {
    MapFile {
        data: target.into(),
        mode: FileMode::SYMLINK,
        ..MapFile::default()
    }
}

#[derive(Default)]
struct State {
    files: MapFs,
    symlinks: BTreeMap<Vec<u8>, Vec<u8>>,
}
/// Source type: tsc/internal/vfs/vfstest/vfstest.go:MapFS
pub struct TestFs {
    state: RwLock<State>,
    case_sensitive: bool,
    clock: Arc<dyn Clock>,
}

/// Rooted, normalised input paths, all POSIX or all Windows. Symbolic link
/// targets are checked the same way.
///
/// # Panics
/// With the pinned sentences, on a non-rooted or non-normalised path, on mixed
/// path styles, and on two paths with one canonical form.
/// port: tsc/internal/vfs/vfstest/vfstest.go:FromMapWithClock
pub fn from_map_with_clock(
    input: &BTreeMap<Vec<u8>, InputFile>,
    case_sensitive: bool,
    clock: Arc<dyn Clock>,
) -> TestFs {
    let (mut posix, mut windows) = (false, false);
    let mut check = |p: &[u8]| {
        assert!(path::is_rooted_disk_path(p), "non-rooted path {}", quote(p));
        let normal = path::normalize(p);
        assert!(
            path::remove_trailing_directory_separator(&normal) == p,
            "non-normalized path {}",
            quote(p)
        );
        if p.starts_with(b"/") {
            posix = true;
        } else {
            windows = true;
        }
    };
    let mut keys: Vec<&Vec<u8>> = input.keys().collect();
    keys.sort_by(|a, b| compare_paths_by_parts(a, b));
    let mut files = BTreeMap::new();
    for key in keys {
        check(key);
        let mut file = match &input[key] {
            InputFile::Text(data) | InputFile::Bytes(data) => MapFile {
                data: data.as_slice().into(),
                ..MapFile::default()
            },
            InputFile::File(file) => file.clone(),
        };
        file.mod_time = clock.now();
        if file.mode.is_symlink() {
            check(&file.data);
            file.data = strip_root(&file.data).into();
        }
        files.insert(strip_root(key).to_vec(), Arc::new(file));
    }
    assert!(!(posix && windows), "mixed posix and windows paths");
    convert_map_fs(&MapFs(files), case_sensitive, Some(clock))
}
/// port: tsc/internal/vfs/vfstest/vfstest.go:FromMap
pub fn from_map(input: &BTreeMap<Vec<u8>, InputFile>, case_sensitive: bool) -> TestFs {
    from_map_with_clock(input, case_sensitive, Arc::new(SystemClock))
}
/// # Panics
/// On two inputs with one canonical path, and when an intermediate directory
/// cannot be created.
/// port: tsc/internal/vfs/vfstest/vfstest.go:convertMapFS
pub fn convert_map_fs(
    input: &MapFs,
    case_sensitive: bool,
    clock: Option<Arc<dyn Clock>>,
) -> TestFs {
    let fs = TestFs {
        state: RwLock::default(),
        case_sensitive,
        clock: clock.unwrap_or_else(|| Arc::new(SystemClock)),
    };
    let mut seen: BTreeMap<Vec<u8>, &Vec<u8>> = BTreeMap::new();
    for name in input.0.keys() {
        if let Some(other) = seen.insert(fs.canonical(name), name) {
            let (first, second) = if name < other {
                (name, other)
            } else {
                (other, name)
            };
            panic!(
                "duplicate path: {} and {} have the same canonical path",
                quote(first),
                quote(second)
            );
        }
    }
    let mut keys: Vec<&Vec<u8>> = input.0.keys().collect();
    keys.sort_by(|a, b| compare_paths_by_parts(a, b));
    {
        let mut state = fs
            .state
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for key in keys {
            let parent = dir_name(key);
            if !parent.is_empty() {
                if let Err(error) = fs.mkdir_all_locked(&mut state, parent, 0o777) {
                    panic!(
                        "failed to create intermediate directories for {}: {error}",
                        quote(key)
                    );
                }
            }
            fs.set_entry(&mut state, key, &fs.canonical(key), (*input.0[key]).clone());
        }
    }
    fs
}
/// Compares element by element, so a directory's children sort right after it.
/// port: tsc/internal/vfs/vfstest/vfstest.go:comparePathsByParts
pub fn compare_paths_by_parts(mut a: &[u8], mut b: &[u8]) -> std::cmp::Ordering {
    loop {
        let (Some(a_end), Some(b_end)) = (
            a.iter().position(|&c| c == b'/'),
            b.iter().position(|&c| c == b'/'),
        ) else {
            return a.cmp(b);
        };
        let order = a[..a_end].cmp(&b[..b_end]);
        if order != std::cmp::Ordering::Equal {
            return order;
        }
        (a, b) = (&a[a_end + 1..], &b[b_end + 1..]);
    }
}
/// The part before the first separator at or after `offset`, and the rest.
///
/// # Panics
/// When `offset` is past the end, as the pinned slice does.
/// port: tsc/internal/vfs/vfstest/vfstest.go:splitPath
pub fn split_path(text: &[u8], offset: usize) -> (&[u8], &[u8]) {
    assert!(
        offset <= text.len(),
        "slice bounds out of range [{offset}:{}]",
        text.len()
    );
    match text[offset..].iter().position(|&c| c == b'/') {
        Some(index) => (&text[..index + offset], &text[index + 1 + offset..]),
        None => (text, b""),
    }
}
/// port: tsc/internal/vfs/vfstest/vfstest.go:dirName
pub fn dir_name(p: &[u8]) -> &[u8] {
    match p.iter().rposition(|&c| c == b'/') {
        Some(index) => p[..=index].strip_suffix(b"/").unwrap_or(&p[..=index]),
        None => b"",
    }
}
/// port: tsc/internal/vfs/vfstest/vfstest.go:baseName
pub fn base_name(p: &[u8]) -> &[u8] {
    match p.iter().rposition(|&c| c == b'/') {
        Some(index) => &p[index + 1..],
        None => p,
    }
}
fn strip_root(p: &[u8]) -> &[u8] {
    p.strip_prefix(b"/").unwrap_or(p)
}
fn quote(bytes: &[u8]) -> String {
    crate::iofs::go_quote(bytes)
}
/// The result of following symbolic links: the file and its canonical path, or
/// the canonical path reached and why it failed.
pub type Resolved = Result<(Arc<MapFile>, Vec<u8>), (Vec<u8>, IoError)>;

impl TestFs {
    pub fn use_case_sensitive_file_names(&self) -> bool {
        self.case_sensitive
    }
    /// port: tsc/internal/vfs/vfstest/vfstest.go:MapFS.getCanonicalPath
    pub fn canonical(&self, p: &[u8]) -> Vec<u8> {
        path::canonical(p, self.case_sensitive).into_owned()
    }
    fn read(&self) -> std::sync::RwLockReadGuard<'_, State> {
        self.state
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
    fn write(&self) -> std::sync::RwLockWriteGuard<'_, State> {
        self.state
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
    /// port: tsc/internal/vfs/vfstest/vfstest.go:MapFS.getFollowingSymlinks
    pub fn get_following_symlinks(&self, canonical: &[u8]) -> Resolved {
        Self::follow(&self.read(), canonical, b"", b"")
    }
    /// `from` and `to` name the link being followed; a miss while following one
    /// is a broken link and not a missing file.
    /// port: tsc/internal/vfs/vfstest/vfstest.go:MapFS.getFollowingSymlinksWorker
    pub fn get_following_symlinks_worker(
        &self,
        canonical: &[u8],
        from: &[u8],
        to: &[u8],
    ) -> Resolved {
        Self::follow(&self.read(), canonical, from, to)
    }
    fn follow(state: &State, canonical: &[u8], from: &[u8], to: &[u8]) -> Resolved {
        let (mut current, mut from, mut to) = (canonical.to_vec(), from.to_vec(), to.to_vec());
        // The pinned worker is tail recursive and a link cycle overflows its stack.
        for _ in 0..1_000_000 {
            if let Some(file) = state
                .files
                .get(&current)
                .filter(|file| !file.mode.is_symlink())
            {
                return Ok((file.clone(), current));
            }
            if let Some(target) = state.symlinks.get(&current) {
                let target = target.clone();
                (from, to) = (current, target.clone());
                current = target;
                continue;
            }
            let through = state.symlinks.iter().find(|(other, _)| {
                other.len() < current.len()
                    && current.starts_with(other)
                    && current[other.len()] == b'/'
            });
            let Some((other, target)) = through else {
                let error = if from.is_empty() {
                    IoError::NotExist
                } else {
                    IoError::BrokenSymlink { from, to }
                };
                return Err((current, error));
            };
            let next = [target.as_slice(), &current[other.len()..]].concat();
            (from, to) = (other.clone(), target.clone());
            current = next;
        }
        panic!("stack overflow: symbolic link cycle");
    }
    /// port: tsc/internal/vfs/vfstest/vfstest.go:MapFS.set
    pub fn set(&self, canonical: &[u8], file: MapFile) {
        self.write()
            .files
            .0
            .insert(canonical.to_vec(), Arc::new(file));
    }
    /// Wraps the entry's `sys` with its spelled path, and records a link.
    ///
    /// # Panics
    /// On an empty path.
    /// port: tsc/internal/vfs/vfstest/vfstest.go:MapFS.setEntry
    pub fn set_entry_public(&self, realpath: &[u8], canonical: &[u8], file: MapFile) {
        self.set_entry(&mut self.write(), realpath, canonical, file);
    }
    fn set_entry(&self, state: &mut State, realpath: &[u8], canonical: &[u8], mut file: MapFile) {
        assert!(!realpath.is_empty() && !canonical.is_empty(), "empty path");
        file.sys = Sys::Wrapper {
            original: Box::new(file.sys),
            realpath: realpath.to_vec(),
        };
        if file.mode.is_symlink() {
            state
                .symlinks
                .insert(canonical.to_vec(), self.canonical(&file.data));
        }
        state.files.0.insert(canonical.to_vec(), Arc::new(file));
    }
    /// port: tsc/internal/vfs/vfstest/vfstest.go:MapFS.open
    pub fn open_canonical(&self, canonical: &[u8]) -> Result<Handle, IoError> {
        self.read().files.open(canonical)
    }
    /// A missing path is not an error. A directory takes everything beneath it.
    /// port: tsc/internal/vfs/vfstest/vfstest.go:MapFS.remove
    pub fn remove_canonical(&self, p: &[u8]) {
        let mut state = self.write();
        let canonical = self.canonical(p);
        let Some(file) = state.files.0.remove(&canonical) else {
            return;
        };
        state.symlinks.remove(&canonical);
        if file.mode.is_dir() {
            let prefix = [canonical.as_slice(), b"/"].concat();
            state.files.0.retain(|name, _| !name.starts_with(&prefix));
            state.symlinks.retain(|name, _| !name.starts_with(&prefix));
        }
    }
    /// port: tsc/internal/vfs/vfstest/vfstest.go:MapFS.Remove
    pub fn remove(&self, p: &[u8]) -> Result<(), IoError> {
        self.remove_canonical(p);
        Ok(())
    }
    /// # Panics
    /// On an empty path.
    /// port: tsc/internal/vfs/vfstest/vfstest.go:MapFS.MkdirAll
    pub fn mkdir_all(&self, p: &[u8], perm: u32) -> Result<(), IoError> {
        self.mkdir_all_locked(&mut self.write(), p, perm)
    }
    /// port: tsc/internal/vfs/vfstest/vfstest.go:MapFS.mkdirAll
    fn mkdir_all_locked(&self, state: &mut State, p: &[u8], perm: u32) -> Result<(), IoError> {
        assert!(!p.is_empty(), "empty path");
        let not_directory = |at: &[u8]| {
            IoError::message(format!(
                "mkdir {}: path exists but is not a directory",
                quote(at)
            ))
        };
        if let Ok((other, _)) = Self::follow(state, &self.canonical(p), b"", b"") {
            return if other.mode.is_dir() {
                Ok(())
            } else {
                Err(not_directory(p))
            };
        }
        let mut p = p.to_vec();
        let mut to_create: Vec<Vec<u8>> = Vec::new();
        let mut offset = 0;
        loop {
            let (directory, rest) = {
                let (directory, rest) = split_path(&p, offset);
                (directory.to_vec(), rest.to_vec())
            };
            let canonical = self.canonical(&directory);
            match Self::follow(state, &canonical, b"", b"") {
                Err((_, error)) if error.is_not_exist() => to_create.push(directory.clone()),
                Err((_, error)) => return Err(error),
                Ok((other, other_path)) => {
                    if !other.mode.is_dir() {
                        return Err(not_directory(&other_path));
                    }
                    if canonical != other_path {
                        // Through a link: restart from the target's spelled path.
                        let Sys::Wrapper { realpath, .. } = &other.sys else {
                            panic!("interface conversion: stored entry has no wrapper");
                        };
                        p = [realpath.as_slice(), b"/", &rest].concat();
                        to_create.clear();
                        offset = 0;
                        continue;
                    }
                }
            }
            if rest.is_empty() {
                break;
            }
            offset = directory.len() + 1;
        }
        for directory in to_create {
            let file = MapFile {
                mode: FileMode::DIR | FileMode(perm & !UMASK),
                mod_time: self.clock.now(),
                ..MapFile::default()
            };
            self.set_entry(state, &directory, &self.canonical(&directory), file);
        }
        Ok(())
    }
    /// port: tsc/internal/vfs/vfstest/vfstest.go:MapFS.AddSymlink
    pub fn add_symlink(&self, p: &[u8], target: &[u8]) {
        self.set_entry(&mut self.write(), p, &self.canonical(p), symlink(target));
    }
    /// port: tsc/internal/vfs/vfstest/vfstest.go:MapFS.WriteFile
    pub fn write_file(&self, p: &[u8], data: &[u8], perm: u32) -> Result<(), IoError> {
        self.put(p, data, perm, "write", false)
    }
    /// port: tsc/internal/vfs/vfstest/vfstest.go:MapFS.AppendFile
    pub fn append_file(&self, p: &[u8], data: &[u8], perm: u32) -> Result<(), IoError> {
        self.put(p, data, perm, "append", true)
    }
    fn put(
        &self,
        p: &[u8],
        data: &[u8],
        perm: u32,
        verb: &str,
        append: bool,
    ) -> Result<(), IoError> {
        let mut state = self.write();
        let parent = dir_name(p);
        if !parent.is_empty() {
            match Self::follow(&state, &self.canonical(parent), b"", b"") {
                Err((_, error)) => {
                    return Err(IoError::Message {
                        text: format!("{verb} {}: {error}", quote(p)),
                        source: Some(Box::new(error)),
                    })
                }
                Ok((file, _)) if !file.mode.is_dir() => {
                    return Err(IoError::message(format!(
                        "{verb} {}: parent path exists but is not a directory",
                        quote(p)
                    )))
                }
                Ok(_) => {}
            }
        }
        let (existing, target) = match Self::follow(&state, &self.canonical(p), b"", b"") {
            Ok((file, target)) => {
                if !file.mode.is_regular() {
                    return Err(IoError::message(format!(
                        "{verb} {}: path exists but is not a regular file",
                        quote(p)
                    )));
                }
                (Some(file), target)
            }
            Err((target, error)) => {
                assert!(error.is_not_exist() || error.is_broken_symlink(), "{error}");
                (None, target)
            }
        };
        let (data, mode) = match existing.filter(|_| append) {
            Some(file) => {
                let mode = if file.mode.0 == 0 {
                    FileMode(perm & !UMASK)
                } else {
                    file.mode
                };
                ([&file.data[..], data].concat(), mode)
            }
            None => (data.to_vec(), FileMode(perm & !UMASK)),
        };
        let file = MapFile {
            data: data.into(),
            mode,
            mod_time: self.clock.now(),
            sys: Sys::Nil,
        };
        self.set_entry(&mut state, p, &target, file);
        Ok(())
    }
    /// The access time is accepted and ignored. The entry is replaced, not
    /// assigned through; see the module note.
    /// port: tsc/internal/vfs/vfstest/vfstest.go:MapFS.Chtimes
    pub fn chtimes(&self, p: &[u8], _a_time: Time, m_time: Time) -> Result<(), IoError> {
        let mut state = self.write();
        let canonical = self.canonical(p);
        let Some(file) = state.files.0.get(&canonical) else {
            return Err(IoError::NotExist);
        };
        let file = MapFile {
            mod_time: m_time,
            ..(**file).clone()
        };
        state.files.0.insert(canonical, Arc::new(file));
        Ok(())
    }
    /// port: tsc/internal/vfs/vfstest/vfstest.go:MapFS.Realpath
    pub fn realpath(&self, name: &[u8]) -> Result<Vec<u8>, IoError> {
        let (file, _) = self
            .get_following_symlinks(&self.canonical(name))
            .map_err(|(_, error)| error)?;
        match &file.sys {
            Sys::Wrapper { realpath, .. } => Ok(realpath.clone()),
            _ => panic!("interface conversion: stored entry has no wrapper"),
        }
    }
    /// port: tsc/internal/vfs/vfstest/vfstest.go:MapFS.GetTargetOfSymlink
    pub fn get_target_of_symlink(&self, p: &[u8]) -> Option<Vec<u8>> {
        let file = self.get_file_info(p)?;
        file.mode
            .is_symlink()
            .then(|| [b"/".as_slice(), &file.data].concat())
    }
    /// The zero time for a missing entry.
    /// port: tsc/internal/vfs/vfstest/vfstest.go:MapFS.GetModTime
    pub fn get_mod_time(&self, p: &[u8]) -> Time {
        self.get_file_info(p)
            .map_or(Time::ZERO, |file| file.mod_time)
    }
    /// The stored entry itself, without following links.
    /// port: tsc/internal/vfs/vfstest/vfstest.go:MapFS.GetFileInfo
    pub fn get_file_info(&self, p: &[u8]) -> Option<Arc<MapFile>> {
        self.read()
            .files
            .get(&self.canonical(strip_root(p)))
            .cloned()
    }
    /// Every entry with its spelled, rooted path, children right after parents.
    /// port: tsc/internal/vfs/vfstest/vfstest.go:MapFS.Entries
    pub fn entries(&self) -> Vec<(Vec<u8>, Arc<MapFile>)> {
        let state = self.read();
        let mut keys: Vec<&Vec<u8>> = state.files.0.keys().collect();
        keys.sort_by(|a, b| compare_paths_by_parts(a, b));
        keys.into_iter()
            .map(|key| {
                let file = state.files.0[key].clone();
                let Sys::Wrapper { realpath, .. } = &file.sys else {
                    panic!("interface conversion: stored entry has no wrapper");
                };
                let spelled = if path::path_is_absolute(realpath) {
                    realpath.clone()
                } else {
                    [b"/".as_slice(), realpath].concat()
                };
                (spelled, file)
            })
            .collect()
    }
}
/// The entry's metadata with the wrapper removed: the spelled base name and the
/// caller's original `sys`. `None` for an entry that has no wrapper, which is a
/// directory `MapFs` synthesized.
/// port: tsc/internal/vfs/vfstest/vfstest.go:convertInfo
pub fn convert_info(info: &Info) -> Option<Info> {
    let Sys::Wrapper { original, realpath } = &info.sys else {
        return None;
    };
    Some(Info {
        name: base_name(realpath).to_vec(),
        sys: (**original).clone(),
        ..info.clone()
    })
}
impl Fs for TestFs {
    /// Follows links, then reports spelled names instead of canonical ones.
    ///
    /// # Panics
    /// On a synthesized directory other than the root.
    /// port: tsc/internal/vfs/vfstest/vfstest.go:MapFS.Open
    fn open(&self, name: &[u8]) -> Result<Handle, IoError> {
        let state = self.read();
        let canonical = match Self::follow(&state, &self.canonical(name), b"", b"") {
            Ok((_, canonical)) | Err((canonical, _)) => canonical,
        };
        let handle = state.files.open(&canonical)?;
        let converted = convert_info(&handle.info);
        let own = converted.unwrap_or_else(|| {
            assert!(name == b".", "unexpected synthesized dir: {}", quote(name));
            Info {
                name: b".".to_vec(),
                ..(*handle.info).clone()
            }
        });
        Ok(handle.with_info(own, |entry| {
            convert_info(entry)
                .unwrap_or_else(|| panic!("unexpected synthesized dir: {}", quote(&entry.name)))
        }))
    }
}
