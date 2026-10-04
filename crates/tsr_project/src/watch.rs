//! Snapshot watch identities and reference-counted client registrations.
//! Paths remain bytes until the strict-Unicode protocol boundary.
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex, OnceLock,
    },
};
use tsr_jsstring::JsString;
use tsr_lsproto::{
    DocumentUri, FileSystemWatcher, PatternOrRelativePattern, RelativePattern, WatchKind,
    WorkspaceFolderOrURI, URI,
};

mod manager;
pub use manager::{WatchClient, WatchManager, WatchSet};

pub const ALL_CHANGES: u32 = 7;
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PatternsAndIgnored {
    pub directories_outside_workspace: Vec<JsString>,
    pub patterns_inside_workspace: Vec<JsString>,
    pub ignored: BTreeSet<JsString>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Watcher {
    pub pattern: JsString,
    pub base_uri: Option<DocumentUri>,
    pub kind: u32,
}
impl Watcher {
    // port: tsc/internal/project/watch.go:fileSystemWatcherGlobString
    pub fn glob_string(&self) -> JsString {
        if let Some(uri) = &self.base_uri {
            JsString::from_bytes([uri.0.as_bytes(), b"/", self.pattern.as_bytes()].concat())
        } else {
            self.pattern.clone()
        }
    }
    /// The wire contract cannot carry invalid Unicode (ADR 0019). Keep that
    /// failure at serialization instead of replacing arbitrary path bytes.
    pub fn to_protocol(&self) -> Result<FileSystemWatcher, std::str::Utf8Error> {
        let pattern = std::str::from_utf8(self.pattern.as_bytes())?.to_owned();
        let glob_pattern = if let Some(uri) = &self.base_uri {
            PatternOrRelativePattern {
                relative_pattern: Some(Box::new(RelativePattern {
                    base_uri: WorkspaceFolderOrURI {
                        uri: Some(Box::new(URI(uri.0.clone()))),
                        ..Default::default()
                    },
                    pattern,
                })),
                ..Default::default()
            }
        } else {
            PatternOrRelativePattern {
                pattern: Some(Box::new(pattern)),
                ..Default::default()
            }
        };
        Ok(FileSystemWatcher {
            glob_pattern,
            kind: Some(Box::new(WatchKind(self.kind))),
        })
    }
}
#[derive(Clone, Debug)]
pub struct Watchers {
    pub id: JsString,
    pub workspace: Arc<[Watcher]>,
    pub outside_workspace: Arc<[Watcher]>,
    pub ignored: BTreeSet<JsString>,
}
impl Watchers {
    pub fn iter(&self) -> impl Iterator<Item = &Watcher> {
        self.workspace.iter().chain(self.outside_workspace.iter())
    }
}
static NEXT_WATCHER: AtomicU64 = AtomicU64::new(1);
fn next_id() -> u64 {
    NEXT_WATCHER
        .try_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_add(1))
        .expect("watcher identity exhausted")
}

pub struct WatchedFiles {
    name: JsString,
    kind: u32,
    relative: bool,
    input: PatternsAndIgnored,
    previous: (Arc<[Watcher]>, Arc<[Watcher]>),
    initial_id: u64,
    computed: OnceLock<Watchers>,
}
impl WatchedFiles {
    // port: tsc/internal/project/watch.go:NewWatchedFiles
    pub fn new(name: JsString, kind: u32, relative: bool) -> Arc<Self> {
        Arc::new(Self {
            name,
            kind,
            relative,
            input: PatternsAndIgnored::default(),
            previous: (Arc::from([]), Arc::from([])),
            initial_id: next_id(),
            computed: OnceLock::new(),
        })
    }
    // port: tsc/internal/project/watch.go:WatchedFiles.Clone
    pub fn with_input(&self, input: PatternsAndIgnored) -> Arc<Self> {
        let previous = self.computed.get().map_or_else(
            || self.previous.clone(),
            |w| (w.workspace.clone(), w.outside_workspace.clone()),
        );
        // Clone at the pin deliberately leaves its numeric id at zero; a new
        // id is assigned lazily only if the computed globs differ.
        Arc::new(Self {
            name: self.name.clone(),
            kind: self.kind,
            relative: self.relative,
            input,
            previous,
            initial_id: 0,
            computed: OnceLock::new(),
        })
    }
    // port: tsc/internal/project/watch.go:WatchedFiles.Watchers
    pub fn watchers(&self) -> &Watchers {
        self.computed.get_or_init(|| {
            let mut globs = self.input.patterns_inside_workspace.clone();
            globs.sort();
            globs.dedup();
            let mut directories = self.input.directories_outside_workspace.clone();
            directories.sort();
            directories.dedup();
            let mut changed = false;
            let workspace = if self.previous.0.iter().map(|w| &w.pattern).eq(globs.iter()) {
                self.previous.0.clone()
            } else {
                changed = true;
                globs
                    .into_iter()
                    .map(|pattern| Watcher {
                        pattern,
                        base_uri: None,
                        kind: self.kind,
                    })
                    .collect()
            };
            let outside: Arc<[Watcher]> = directories
                .into_iter()
                .map(|dir| recursive_directory_watcher(&dir, self.kind, self.relative))
                .collect();
            let outside_workspace = if self
                .previous
                .1
                .iter()
                .map(Watcher::glob_string)
                .eq(outside.iter().map(Watcher::glob_string))
            {
                self.previous.1.clone()
            } else {
                changed = true;
                outside
            };
            Watchers {
                id: JsString::from_bytes(
                    [
                        self.name.as_bytes(),
                        b" watcher ",
                        (if changed { next_id() } else { self.initial_id })
                            .to_string()
                            .as_bytes(),
                    ]
                    .concat(),
                ),
                workspace,
                outside_workspace,
                ignored: self.input.ignored.clone(),
            }
        })
    }
    // port: tsc/internal/project/watch.go:WatchedFiles.ID
    pub fn id(&self) -> &JsString {
        &self.watchers().id
    }
}
#[derive(Default)]
struct RegistryState {
    entries: BTreeMap<(JsString, u32), (JsString, usize)>,
    pending: BTreeSet<JsString>,
}
#[derive(Default)]
pub struct WatchRegistry(Mutex<RegistryState>);
impl WatchRegistry {
    // port: tsc/internal/project/watch.go:watchRegistry.Acquire
    pub fn acquire(&self, watcher: &Watcher, id: JsString) -> bool {
        let mut state = self.0.lock().unwrap();
        let value = state
            .entries
            .entry((watcher.glob_string(), watcher.kind))
            .or_insert((id, 0));
        value.1 = value.1.checked_add(1).expect("watch reference overflow");
        value.1 == 1
    }
    // port: tsc/internal/project/watch.go:watchRegistry.Release
    pub fn release(&self, watcher: &Watcher) -> Option<JsString> {
        let key = (watcher.glob_string(), watcher.kind);
        let mut state = self.0.lock().unwrap();
        let value = state.entries.get_mut(&key)?;
        if value.1 > 1 {
            value.1 -= 1;
            return None;
        }
        state.entries.remove(&key).map(|value| value.0)
    }
    // port: tsc/internal/project/watch.go:watchRegistry.MarkPending
    pub fn mark_pending(&self, id: &JsString) {
        self.0.lock().unwrap().pending.insert(id.clone());
    }
    // port: tsc/internal/project/watch.go:watchRegistry.ClearPending
    pub fn clear_pending(&self, id: &JsString) {
        self.0.lock().unwrap().pending.remove(id);
    }
    // port: tsc/internal/project/watch.go:watchRegistry.IsPending
    pub fn is_pending(&self, id: &JsString) -> bool {
        self.0.lock().unwrap().pending.contains(id)
    }
}

// port: tsc/internal/project/watch.go:newRecursiveDirectoryWatcher
fn recursive_directory_watcher(directory: &JsString, kind: u32, relative: bool) -> Watcher {
    if relative {
        Watcher {
            pattern: JsString::from_bytes(b"**/*".as_slice()),
            base_uri: Some(DocumentUri::from_file_name(directory.as_bytes())),
            kind,
        }
    } else {
        Watcher {
            pattern: recursive_glob(directory.as_bytes()),
            base_uri: None,
            kind,
        }
    }
}
// port: tsc/internal/project/watch.go:getRecursiveGlobPattern
pub(crate) fn recursive_glob(directory: &[u8]) -> JsString {
    JsString::from_bytes(
        [
            tsr_tspath::remove_trailing_directory_separator(directory),
            b"/**/*",
        ]
        .concat(),
    )
}
// port: tsc/internal/project/watch.go:getPathComponentsForWatching
pub fn components_for_watching(path: &[u8], cwd: &[u8]) -> Vec<Vec<u8>> {
    let components = tsr_tspath::path_components(path, cwd);
    let n = perceived_root(&components);
    if n <= 1 {
        return components;
    }
    let rest: Vec<_> = components[1..n].iter().map(Vec::as_slice).collect();
    std::iter::once(tsr_tspath::combine(&components[0], &rest))
        .chain(components[n..].iter().cloned())
        .collect()
}
// port: tsc/internal/project/watch.go:perceivedOsRootLengthForWatching
fn perceived_root(components: &[Vec<u8>]) -> usize {
    if components.len() <= 1 {
        return components.len();
    }
    if components[0].starts_with(b"//") {
        return 2;
    }
    let volume = &components[0];
    if volume.len() == 3 && volume[0].is_ascii_alphabetic() && &volume[1..] == b":/" {
        return if components[1].eq_ignore_ascii_case(b"users") {
            3.min(components.len())
        } else {
            1
        };
    }
    if components[1] == b"home" {
        3.min(components.len())
    } else {
        1
    }
}
fn common_directories(
    directories: &BTreeSet<JsString>,
    cwd: &[u8],
    sensitive: bool,
) -> (Vec<JsString>, BTreeSet<JsString>) {
    let names: Vec<_> = directories.iter().map(JsString::as_bytes).collect();
    let (mut common, ignored) =
        tsr_tspath::common_parents(&names, 2, components_for_watching, cwd, sensitive);
    common.sort();
    (
        common.into_iter().map(JsString::from_bytes).collect(),
        ignored.into_iter().map(JsString::from_bytes).collect(),
    )
}
// port: tsc/internal/project/watch.go:createResolutionLookupGlobMapper
pub fn resolution_patterns(
    files: &BTreeSet<JsString>,
    workspace: &[u8],
    library: &[u8],
    cwd: &[u8],
    sensitive: bool,
) -> PatternsAndIgnored {
    let workspace = tsr_tspath::to_path(workspace, cwd, sensitive);
    let library = tsr_tspath::to_path(library, cwd, sensitive);
    let current = tsr_tspath::to_path(cwd, cwd, sensitive);
    let mut seen = BTreeSet::new();
    let mut roots = BTreeSet::new();
    let mut external = BTreeSet::new();
    for file in files {
        let file = file.as_bytes();
        if tsr_tspath::is_dynamic_file_name(file) || !seen.insert(tsr_tspath::directory(file)) {
            continue;
        }
        let root = [workspace.as_bytes(), current.as_bytes(), library.as_bytes()]
            .into_iter()
            .find(|root| tsr_tspath::contains_path(root, file, b"", true));
        if let Some(root) = root {
            roots.insert(JsString::from_bytes(root));
        } else if let Some(index) = file
            .windows(b"/node_modules/".len())
            .position(|w| w == b"/node_modules/")
        {
            roots.insert(JsString::from_bytes(
                &file[..index + b"/node_modules".len()],
            ));
        } else {
            external.insert(JsString::from_bytes(tsr_tspath::directory(file)));
        }
    }
    let (directories_outside_workspace, ignored) = common_directories(&external, b"", true);
    PatternsAndIgnored {
        directories_outside_workspace,
        patterns_inside_workspace: roots
            .into_iter()
            .map(|root| recursive_glob(root.as_bytes()))
            .collect(),
        ignored,
    }
}
// port: tsc/internal/project/configfileregistrybuilder.go:configFileRegistryBuilder.updateRootFilesWatch
pub fn config_patterns(
    command: &tsr_tsoptions::ParsedCommandLine,
    name: &[u8],
    cwd: &[u8],
    sensitive: bool,
) -> PatternsAndIgnored {
    let directory = tsr_tspath::directory(name);
    let mut workspace = false;
    let mut config_dir = false;
    let mut external = BTreeSet::new();
    let mut classify = |dir: &[u8]| {
        if tsr_tspath::contains_path(cwd, dir, cwd, sensitive) {
            workspace = true;
        } else if tsr_tspath::contains_path(&directory, dir, cwd, sensitive) {
            config_dir = true;
        } else {
            external.insert(JsString::from_bytes(dir));
        }
    };
    if let Some(directories) = command.wildcard_directories() {
        for (dir, _) in directories.entries() {
            classify(dir.as_bytes());
        }
    }
    for file in command.literal_file_names().unwrap_or_default() {
        classify(&tsr_tspath::directory(file.as_bytes()));
    }
    let mut globs = Vec::new();
    if workspace {
        globs.push(recursive_glob(cwd));
    }
    if config_dir {
        globs.push(recursive_glob(&directory));
    }
    for file in command.extended_source_files() {
        if !(workspace && tsr_tspath::contains_path(cwd, file.as_bytes(), cwd, sensitive)) {
            globs.push(file.clone());
        }
    }
    let (common, ignored) = common_directories(&external, cwd, sensitive);
    globs.extend(common.into_iter().map(|dir| recursive_glob(dir.as_bytes())));
    globs.sort();
    PatternsAndIgnored {
        patterns_inside_workspace: globs,
        ignored,
        ..Default::default()
    }
}

#[cfg(test)]
mod tests;
