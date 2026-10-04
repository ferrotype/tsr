//! Files read by a project belong to its snapshot. Builder I/O is outside the
//! map lock; publication rechecks the entry so concurrent readers share the
//! winning handle. Frozen snapshots never replace an observed file.
use crate::{
    dirty::Map,
    file_change::FileChangeSummary,
    overlay::{FileHandle, Overlays},
};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{Arc, Mutex, OnceLock},
};
use tsr_jsstring::JsString;
use tsr_lsproto::DocumentUri;
use tsr_vfs::{cached::CachedFs, Entries, Error, FileContent, FileInfo, FileSystem};

type Directory = BTreeMap<JsString, JsString>;
type Directories = BTreeMap<JsString, Arc<Directory>>;
type MemoizedFile = Arc<OnceLock<Result<Option<Arc<FileHandle>>, Error>>>;

#[derive(Clone)]
struct DiskEntry {
    name: JsString,
    file: Option<Arc<FileHandle>>,
    needs_reload: bool,
    realpath: Option<JsString>,
}
pub struct SnapshotFs {
    fs: Arc<dyn FileSystem>,
    origin: Arc<dyn FileSystem>,
    cwd: JsString,
    overlays: Overlays,
    overlay_directories: Arc<Directories>,
    disk_files: Arc<BTreeMap<JsString, DiskEntry>>,
    disk_directories: Arc<Directories>,
    aliases: Arc<BTreeMap<JsString, BTreeSet<JsString>>>,
    read_files: Mutex<BTreeMap<JsString, MemoizedFile>>,
}
impl SnapshotFs {
    pub fn empty(fs: Arc<dyn FileSystem>, cwd: JsString) -> Arc<Self> {
        Arc::new(Self {
            origin: fs.clone(),
            fs,
            cwd,
            overlays: Arc::default(),
            overlay_directories: Arc::default(),
            disk_files: Arc::default(),
            disk_directories: Arc::default(),
            aliases: Arc::default(),
            read_files: Mutex::default(),
        })
    }
    fn path(&self, name: &[u8]) -> JsString {
        tsr_tspath::to_path(
            name,
            self.cwd.as_bytes(),
            self.fs.use_case_sensitive_file_names(),
        )
    }
    pub fn overlays(&self) -> &Overlays {
        &self.overlays
    }
    pub fn cached_file_count(&self) -> usize {
        self.disk_files.len()
    }
    // port: tsc/internal/project/snapshotfs.go:SnapshotFS.GetFileByPath
    pub fn get_file(&self, name: &[u8]) -> Result<Option<Arc<FileHandle>>, Error> {
        let path = self.path(name);
        if let Some(file) = self.overlays.get(&path) {
            return Ok(Some(file.clone()));
        }
        if let Some(entry) = self.disk_files.get(&path) {
            return Ok(entry.file.clone());
        }
        let entry = self
            .read_files
            .lock()
            .unwrap()
            .entry(path)
            .or_default()
            .clone();
        entry.get_or_init(|| read_disk(&*self.fs, name)).clone()
    }
    // port: tsc/internal/project/snapshotfs.go:SnapshotFS.expandRealpathAliases
    pub fn expand_realpath_aliases(&self, change: &mut FileChangeSummary) {
        for set in [&mut change.changed, &mut change.deleted] {
            let additional: Vec<_> = set
                .iter()
                .flat_map(|uri| {
                    self.aliases
                        .get(&self.path(uri.file_name().as_bytes()))
                        .into_iter()
                        .flatten()
                })
                .map(|path| DocumentUri::from_file_name(path.as_bytes()))
                .collect();
            set.extend(additional);
        }
    }
    // port: tsc/internal/project/snapshotfs.go:SnapshotFS.GetAccessibleEntries
    pub fn accessible_entries(&self, name: &[u8]) -> Entries {
        let path = self.path(name);
        let mut entries = Entries::default();
        for directories in [&self.disk_directories, &self.overlay_directories] {
            if let Some(directory) = directories.get(&path) {
                append_directory(
                    directory,
                    |key| self.overlays.contains_key(key) || self.disk_files.contains_key(key),
                    &mut entries,
                );
            }
        }
        entries
    }
}
impl FileSystem for SnapshotFs {
    fn snapshot_id(&self) -> Option<tsr_vfs::SnapshotId> {
        None
    }
    fn use_case_sensitive_file_names(&self) -> bool {
        self.fs.use_case_sensitive_file_names()
    }
    fn read_file(&self, name: &[u8]) -> Result<Option<FileContent>, Error> {
        self.get_file(name).map(file_content)
    }
    fn file_exists(&self, name: &[u8]) -> Result<bool, Error> {
        let path = self.path(name);
        if self.overlays.contains_key(&path) || self.disk_files.contains_key(&path) {
            Ok(true)
        } else {
            self.fs.file_exists(name)
        }
    }
    fn directory_exists(&self, name: &[u8]) -> Result<bool, Error> {
        self.fs.directory_exists(name)
    }
    fn entries(&self, name: &[u8]) -> Result<Entries, Error> {
        Ok(self.accessible_entries(name))
    }
    fn stat(&self, name: &[u8]) -> Result<Option<FileInfo>, Error> {
        self.fs.stat(name)
    }
    fn realpath(&self, name: &[u8]) -> Result<JsString, Error> {
        self.fs.realpath(name)
    }
    fn walk_dir(&self, path: &[u8], visit: &mut tsr_vfs::WalkCallback<'_>) -> Result<(), Error> {
        self.fs.walk_dir(path, visit)
    }
}

struct BuildState {
    files: Map<JsString, DiskEntry>,
    deleted: BTreeSet<JsString>,
    sealed: bool,
}
pub struct SnapshotFsBuilder {
    base: Arc<SnapshotFs>,
    fs: Arc<CachedFs>,
    overlays: Overlays,
    overlay_directories: Arc<Directories>,
    state: Mutex<BuildState>,
    entries: Mutex<BTreeMap<JsString, Entries>>,
    frozen: OnceLock<Arc<SnapshotFs>>,
}
impl SnapshotFsBuilder {
    // port: tsc/internal/project/snapshotfs.go:newSnapshotFSBuilder
    pub fn new(base: Arc<SnapshotFs>, overlays: Overlays) -> Self {
        let fs = Arc::new(CachedFs::new(base.origin.clone()));
        let overlay_directories = Arc::new(directories(
            overlays.iter().map(|(path, file)| (path, file.file_name())),
        ));
        let state = Mutex::new(BuildState {
            files: Map::new(base.disk_files.clone()),
            deleted: BTreeSet::new(),
            sealed: false,
        });
        Self {
            base,
            fs,
            overlays,
            overlay_directories,
            state,
            entries: Mutex::default(),
            frozen: OnceLock::new(),
        }
    }
    pub fn base(&self) -> &Arc<SnapshotFs> {
        &self.base
    }
    // port: tsc/internal/project/snapshotfs.go:snapshotFSBuilder.GetFileByPath
    pub fn get_file(&self, name: &[u8]) -> Result<Option<Arc<FileHandle>>, Error> {
        if let Some(frozen) = self.frozen.get() {
            return frozen.get_file(name);
        }
        let path = self.base.path(name);
        if let Some(file) = self.overlays.get(&path) {
            return Ok(Some(file.clone()));
        }
        self.disk_file(name, &path, false)
    }
    // port: tsc/internal/project/snapshotfs.go:snapshotFSBuilder.getDiskFile
    fn disk_file(
        &self,
        name: &[u8],
        path: &JsString,
        force: bool,
    ) -> Result<Option<Arc<FileHandle>>, Error> {
        let entry = {
            let state = self.state.lock().unwrap();
            assert!(!state.sealed, "snapshot builder is finalized");
            if state.deleted.contains(path) {
                return Ok(None);
            }
            if let Some(entry) = state.files.get(path) {
                if !force && !entry.needs_reload {
                    return Ok(entry.file.clone());
                }
                Some(entry.clone())
            } else if state.files.original(path).is_some() {
                return Ok(None);
            } else {
                None
            }
        };
        let file = read_disk(&*self.fs, name)?;
        let realpath = if let Some(entry) = &entry {
            entry.realpath.clone()
        } else if name
            .windows(b"/node_modules/".len())
            .any(|w| w == b"/node_modules/")
        {
            let realpath = self.base.path(self.fs.realpath(name)?.as_bytes());
            (realpath != *path).then_some(realpath)
        } else {
            None
        };
        let mut state = self.state.lock().unwrap();
        assert!(!state.sealed, "snapshot builder finalized during a read");
        if state.deleted.contains(path) {
            return Ok(None);
        }
        if !force {
            if let Some(current) = state.files.get(path) {
                if !current.needs_reload {
                    return Ok(current.file.clone());
                }
            } else if state.files.original(path).is_some() {
                return Ok(None);
            }
        }
        if let Some(file) = &file {
            state.files.insert(
                path.clone(),
                DiskEntry {
                    name: JsString::from_bytes(name),
                    file: Some(file.clone()),
                    needs_reload: false,
                    realpath,
                },
            );
        } else {
            state.files.remove(path);
            state.deleted.insert(path.clone());
        }
        Ok(file)
    }
    // port: tsc/internal/project/snapshotfs.go:snapshotFSBuilder.Finalize
    pub fn finalize(&self) -> Arc<SnapshotFs> {
        self.frozen
            .get_or_init(|| {
                let mut state = self.state.lock().unwrap();
                assert!(!state.sealed, "snapshot builder is finalized");
                state.sealed = true;
                let (files, changed) =
                    std::mem::replace(&mut state.files, Map::new(Arc::default())).finalize();
                drop(state);
                let disk_directories = if changed {
                    update_directories(&self.base, &files)
                } else {
                    self.base.disk_directories.clone()
                };
                let aliases_changed = changed
                    && (files.len() != self.base.disk_files.len()
                        || files.iter().any(|(path, entry)| {
                            self.base
                                .disk_files
                                .get(path)
                                .is_none_or(|old| old.realpath != entry.realpath)
                        }));
                let aliases = if aliases_changed {
                    let mut aliases: BTreeMap<JsString, BTreeSet<JsString>> = BTreeMap::new();
                    for (path, entry) in &*files {
                        if let Some(realpath) = &entry.realpath {
                            aliases
                                .entry(realpath.clone())
                                .or_default()
                                .insert(path.clone());
                        }
                    }
                    Arc::new(aliases)
                } else {
                    self.base.aliases.clone()
                };
                Arc::new(SnapshotFs {
                    fs: self.fs.clone(),
                    origin: self.base.origin.clone(),
                    cwd: self.base.cwd.clone(),
                    overlays: self.overlays.clone(),
                    overlay_directories: self.overlay_directories.clone(),
                    disk_files: files,
                    disk_directories,
                    aliases,
                    read_files: Mutex::default(),
                })
            })
            .clone()
    }
    /// Remove files no project observed while constructing this snapshot.
    pub fn retain_files(&self, keep: impl Fn(&JsString) -> bool) {
        let mut state = self.state.lock().unwrap();
        assert!(!state.sealed, "snapshot builder is finalized");
        for key in state.files.keys() {
            if !keep(&key) {
                state.files.remove(&key);
                state.deleted.insert(key);
            }
        }
    }
    // port: tsc/internal/project/snapshotfs.go:snapshotFSBuilder.invalidateCache
    pub fn invalidate_cache(&self, node_modules_only: bool) {
        let mut state = self.state.lock().unwrap();
        assert!(!state.sealed, "snapshot builder is finalized");
        for key in state.files.keys() {
            if !node_modules_only
                || key
                    .as_bytes()
                    .windows(b"/node_modules/".len())
                    .any(|w| w == b"/node_modules/")
            {
                let mut entry = state.files.get(&key).unwrap().clone();
                entry.needs_reload = true;
                state.files.insert(key, entry);
            }
        }
    }
    // port: tsc/internal/project/snapshotfs.go:snapshotFSBuilder.watchChangesOverlapCache
    pub fn watch_changes_overlap_cache(&self, changes: &FileChangeSummary) -> bool {
        let state = self.state.lock().unwrap();
        changes.changed.iter().chain(&changes.deleted).any(|uri| {
            let path = self.base.path(uri.file_name().as_bytes());
            state.files.contains_key(&path) || self.base.aliases.contains_key(&path)
        })
    }
    // port: tsc/internal/project/snapshotfs.go:snapshotFSBuilder.markDirtyFiles
    pub fn mark_dirty_files(&self, change: &mut FileChangeSummary) -> Result<(), Error> {
        let mut filtered = BTreeSet::new();
        for uri in &change.changed {
            let name = uri.file_name();
            let path = self.base.path(name.as_bytes());
            let entry = self.state.lock().unwrap().files.get(&path).cloned();
            if self.overlays.contains_key(&path) || entry.is_none() {
                filtered.insert(uri.clone());
                continue;
            }
            let old = entry.unwrap();
            let file = read_disk(&*self.fs, old.name.as_bytes())?;
            let mut state = self.state.lock().unwrap();
            assert!(!state.sealed, "snapshot builder is finalized");
            let Some(old) = state.files.get(&path).cloned() else {
                filtered.insert(uri.clone());
                continue;
            };
            if let Some(file) = file {
                if old
                    .file
                    .as_ref()
                    .is_some_and(|old| old.content() == file.content())
                {
                    if old.needs_reload {
                        state.files.insert(
                            path,
                            DiskEntry {
                                needs_reload: false,
                                ..old
                            },
                        );
                    }
                    continue;
                }
                state.files.insert(
                    path,
                    DiskEntry {
                        file: Some(file),
                        needs_reload: false,
                        ..old
                    },
                );
            } else {
                state.files.remove(&path);
                state.deleted.insert(path);
            }
            filtered.insert(uri.clone());
        }
        change.changed = filtered;
        let mut state = self.state.lock().unwrap();
        assert!(!state.sealed, "snapshot builder is finalized");
        for uri in &change.deleted {
            let path = self.base.path(uri.file_name().as_bytes());
            if state.files.contains_key(&path) {
                state.files.remove(&path);
                state.deleted.insert(path);
            }
        }
        Ok(())
    }
    // port: tsc/internal/project/snapshotfs.go:snapshotFSBuilder.convertOpenAndCloseToChanges
    pub fn convert_open_and_close(&self, change: &mut FileChangeSummary) -> Result<(), Error> {
        if let Some(uri) = &change.opened {
            let name = uri.file_name();
            let path = self.base.path(name.as_bytes());
            if !tsr_tspath::is_dynamic_file_name(name.as_bytes()) {
                if let Some(old) = self
                    .base
                    .disk_files
                    .get(&path)
                    .and_then(|e| e.file.as_ref())
                {
                    if self
                        .overlays
                        .get(&path)
                        .is_some_and(|file| file.hash() != old.hash())
                    {
                        change.changed.insert(uri.clone());
                    }
                } else {
                    change.created.insert(uri.clone());
                }
            }
        }
        for uri in &change.closed {
            let name = uri.file_name();
            let path = self.base.path(name.as_bytes());
            if tsr_tspath::is_dynamic_file_name(name.as_bytes()) {
                continue;
            }
            if let Some(file) = self.disk_file(name.as_bytes(), &path, true)? {
                if file.hash() != self.base.overlays[&path].hash() {
                    change.changed.insert(uri.clone());
                }
            } else {
                change.deleted.insert(uri.clone());
            }
        }
        Ok(())
    }
    // port: tsc/internal/project/snapshotfs.go:snapshotFSBuilder.expandAndFilterWatchEvents
    pub fn filter_watch_events(
        &self,
        change: &mut FileChangeSummary,
        mapper_extensions: &[JsString],
        mapper_watched_files: &BTreeSet<JsString>,
    ) {
        let relevant = |uri: &DocumentUri| {
            let name = uri.file_name();
            let path = self.base.path(name.as_bytes());
            mapper_watched_files.contains(&path)
                || mapper_extensions
                    .iter()
                    .any(|ext| tsr_tspath::file_extension_is(name.as_bytes(), ext.as_bytes()))
                || tsr_tspath::is_dynamic_file_name(name.as_bytes())
                || self.overlays.contains_key(&path)
                || [
                    b".js".as_slice(),
                    b".jsx",
                    b".mjs",
                    b".cjs",
                    b".ts",
                    b".tsx",
                    b".mts",
                    b".cts",
                    b".json",
                ]
                .iter()
                .any(|ext| path.as_bytes().ends_with(ext))
        };
        let mut deleted = BTreeSet::new();
        for uri in &change.deleted {
            let path = self.base.path(uri.file_name().as_bytes());
            if self.base.disk_directories.contains_key(&path) {
                let state = self.state.lock().unwrap();
                let mut stack = vec![path];
                while let Some(dir) = stack.pop() {
                    if let Some(children) = self.base.disk_directories.get(&dir) {
                        for child in children.keys() {
                            if let Some(entry) = state.files.get(child) {
                                deleted.insert(DocumentUri::from_file_name(entry.name.as_bytes()));
                            }
                            if self.base.disk_directories.contains_key(child) {
                                stack.push(child.clone());
                            }
                        }
                    }
                }
            } else if relevant(uri)
                || path.as_bytes().ends_with(b"/node_modules")
                || path
                    .as_bytes()
                    .windows(b"/node_modules/".len())
                    .any(|w| w == b"/node_modules/")
            {
                deleted.insert(uri.clone());
            }
        }
        change.deleted = deleted;
        change.changed.retain(relevant);
    }
}
impl FileSystem for SnapshotFsBuilder {
    fn snapshot_id(&self) -> Option<tsr_vfs::SnapshotId> {
        None
    }
    fn use_case_sensitive_file_names(&self) -> bool {
        self.fs.use_case_sensitive_file_names()
    }
    fn read_file(&self, name: &[u8]) -> Result<Option<FileContent>, Error> {
        self.get_file(name).map(file_content)
    }
    fn file_exists(&self, name: &[u8]) -> Result<bool, Error> {
        if let Some(frozen) = self.frozen.get() {
            return frozen.file_exists(name);
        }
        let path = self.base.path(name);
        if self.overlays.contains_key(&path) {
            return Ok(true);
        }
        let cached = {
            let state = self.state.lock().unwrap();
            state.deleted.contains(&path)
                || state.files.contains_key(&path)
                || state.files.original(&path).is_some()
        };
        if cached {
            self.disk_file(name, &path, false)
                .map(|file| file.is_some())
        } else {
            self.fs.file_exists(name)
        }
    }
    fn directory_exists(&self, name: &[u8]) -> Result<bool, Error> {
        self.fs.directory_exists(name)
    }
    // port: tsc/internal/project/snapshotfs.go:snapshotFSBuilder.GetAccessibleEntries
    fn entries(&self, name: &[u8]) -> Result<Entries, Error> {
        if let Some(frozen) = self.frozen.get() {
            return frozen.entries(name);
        }
        let mut entries = self.fs.entries(name)?;
        let path = self.base.path(name);
        let Some(directory) = self.overlay_directories.get(&path) else {
            return Ok(entries);
        };
        if let Some(merged) = self.entries.lock().unwrap().get(&path) {
            return Ok(merged.clone());
        }
        append_directory(
            directory,
            |key| self.overlays.contains_key(key),
            &mut entries,
        );
        Ok(self
            .entries
            .lock()
            .unwrap()
            .entry(path)
            .or_insert(entries)
            .clone())
    }
    fn stat(&self, name: &[u8]) -> Result<Option<FileInfo>, Error> {
        self.fs.stat(name)
    }
    fn realpath(&self, name: &[u8]) -> Result<JsString, Error> {
        self.fs.realpath(name)
    }
    fn walk_dir(&self, path: &[u8], visit: &mut tsr_vfs::WalkCallback<'_>) -> Result<(), Error> {
        self.fs.walk_dir(path, visit)
    }
}

fn read_disk(fs: &dyn FileSystem, name: &[u8]) -> Result<Option<Arc<FileHandle>>, Error> {
    Ok(fs.read_file(name)?.map(|file| {
        Arc::new(FileHandle::disk(
            JsString::from_bytes(name),
            file.text
                .slice(0..file.text.len())
                .expect("whole source text"),
        ))
    }))
}
fn file_content(file: Option<Arc<FileHandle>>) -> Option<FileContent> {
    file.map(|file| FileContent::loaded(file.content().as_bytes()))
}
fn directories<'a>(files: impl Iterator<Item = (&'a JsString, &'a JsString)>) -> Directories {
    let mut directories: Directories = BTreeMap::new();
    for (path, name) in files {
        let mut child = path.clone();
        let mut name = name.clone();
        loop {
            let parent = JsString::from_bytes(tsr_tspath::directory(child.as_bytes()));
            if parent == child {
                break;
            }
            Arc::make_mut(directories.entry(parent.clone()).or_default()).insert(
                child,
                JsString::from_bytes(tsr_tspath::base_name(name.as_bytes())),
            );
            child = parent;
            name = JsString::from_bytes(tsr_tspath::directory(name.as_bytes()));
        }
    }
    directories
}
fn update_directories(
    base: &SnapshotFs,
    files: &BTreeMap<JsString, DiskEntry>,
) -> Arc<Directories> {
    let mut result = base.disk_directories.clone();
    for (path, file) in files {
        if base.disk_files.contains_key(path) {
            continue;
        }
        let mut child = path.clone();
        let mut name = file.name.clone();
        loop {
            let parent = JsString::from_bytes(tsr_tspath::directory(child.as_bytes()));
            if parent == child {
                break;
            }
            let existing = result.contains_key(&parent);
            Arc::make_mut(
                Arc::make_mut(&mut result)
                    .entry(parent.clone())
                    .or_default(),
            )
            .insert(
                child,
                JsString::from_bytes(tsr_tspath::base_name(name.as_bytes())),
            );
            if existing {
                break;
            }
            child = parent;
            name = JsString::from_bytes(tsr_tspath::directory(name.as_bytes()));
        }
    }
    for path in base.disk_files.keys() {
        if files.contains_key(path) {
            continue;
        }
        let mut child = path.clone();
        loop {
            let parent = JsString::from_bytes(tsr_tspath::directory(child.as_bytes()));
            if parent == child || !result.contains_key(&parent) {
                break;
            }
            let map = Arc::make_mut(&mut result);
            let directory = Arc::make_mut(map.get_mut(&parent).unwrap());
            directory.remove(&child);
            if !directory.is_empty() {
                break;
            }
            map.remove(&parent);
            child = parent;
        }
    }
    result
}
// port: tsc/internal/project/snapshotfs.go:readDirectoryIntoEntries
fn append_directory(
    directory: &Directory,
    is_file: impl Fn(&JsString) -> bool,
    entries: &mut Entries,
) {
    for (path, name) in directory {
        let target = if is_file(path) {
            &mut entries.files
        } else {
            &mut entries.directories
        };
        target.get_or_insert_default().push(name.clone());
    }
}

#[cfg(test)]
mod tests;
