//! `testutil/fsbaselineutil`: the file-system differ that renders the fake
//! system's files into a baseline, each step showing only what changed since
//! the step before, and the internal-symbol-name sanitizer.
use crate::goutil::{contains, decode_rune};
use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use tsr_vfs::iofs::Time;
use tsr_vfs::iovfs::IoVfs;
use tsr_vfs::vfstest::TestFs;

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// `collections.SyncSet[string]` of paths. Iteration is in path order where
/// the pin's is random; every pinned use sorts or only collects.
#[derive(Debug, Default)]
pub struct SyncSet(Mutex<BTreeSet<Vec<u8>>>);

impl SyncSet {
    pub fn has(&self, path: &[u8]) -> bool {
        lock(&self.0).contains(path)
    }
    pub fn add(&self, path: &[u8]) {
        lock(&self.0).insert(path.to_vec());
    }
    pub fn delete(&self, path: &[u8]) {
        lock(&self.0).remove(path);
    }
    /// `*set = collections.SyncSet[string]{}`.
    pub fn clear(&self) {
        lock(&self.0).clear();
    }
    /// `ToSlice()`.
    pub fn to_slice(&self) -> Vec<Vec<u8>> {
        lock(&self.0).iter().cloned().collect()
    }
}

/// A `*collections.SyncSet[string]` that starts nil: `testFs.defaultLibs`,
/// created by the first library file the fake system adds.
#[derive(Debug, Default)]
pub struct NilableSyncSet(Mutex<Option<BTreeSet<Vec<u8>>>>);

impl NilableSyncSet {
    /// `set == nil`.
    pub fn is_nil(&self) -> bool {
        lock(&self.0).is_none()
    }
    /// `set.Has(path)`; `None` for a nil set (the pin would dereference nil).
    pub fn has(&self, path: &[u8]) -> Option<bool> {
        lock(&self.0).as_ref().map(|set| set.contains(path))
    }
    /// `if set == nil { set = &SyncSet{} }; set.Add(path)`.
    pub fn add(&self, path: &[u8]) {
        lock(&self.0)
            .get_or_insert_with(BTreeSet::new)
            .insert(path.to_vec());
    }
    /// `set.Delete(path)` on a non-nil set.
    pub fn delete(&self, path: &[u8]) {
        if let Some(set) = lock(&self.0).as_mut() {
            set.remove(path);
        }
    }
    /// The members, or `None` for a nil set.
    pub fn snapshot(&self) -> Option<BTreeSet<Vec<u8>>> {
        lock(&self.0).clone()
    }
}

/// `fsbaselineutil.DiffEntry`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DiffEntry {
    pub content: Vec<u8>,
    pub m_time: Time,
    pub is_written: bool,
    pub symlink_target: Vec<u8>,
}

/// `fsbaselineutil.Snapshot`: the files as the last baseline rendered them.
#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    pub snap: BTreeMap<Vec<u8>, DiffEntry>,
    pub default_libs: BTreeSet<Vec<u8>>,
}

/// `fsbaselineutil.FileChange`: a file-system change between snapshots.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileChange {
    pub path: Vec<u8>,
    pub deleted: bool,
}

/// `fsbaselineutil.FSDiffer`.
pub struct FsDiffer {
    /// `FS iovfs.FsWithSys`: the test file system without the harness's
    /// `testFs` layer.
    pub fs: Arc<IoVfs<TestFs>>,
    /// `DefaultLibs func() *collections.SyncSet[string]`: `None` is a nil
    /// function; the set itself may be nil.
    pub default_libs: Option<Arc<NilableSyncSet>>,
    pub written_files: Arc<SyncSet>,
    serialized_diff: Mutex<Option<Arc<Snapshot>>>,
}

impl FsDiffer {
    pub fn new(
        fs: Arc<IoVfs<TestFs>>,
        default_libs: Option<Arc<NilableSyncSet>>,
        written_files: Arc<SyncSet>,
    ) -> Self {
        Self {
            fs,
            default_libs,
            written_files,
            serialized_diff: Mutex::new(None),
        }
    }

    // port: tsc/internal/testutil/fsbaselineutil/differ.go:FSDiffer.MapFs
    pub fn map_fs(&self) -> &TestFs {
        self.fs.fsys()
    }

    // port: tsc/internal/testutil/fsbaselineutil/differ.go:FSDiffer.SerializedDiff
    pub fn serialized_diff(&self) -> Option<Arc<Snapshot>> {
        lock(&self.serialized_diff).clone()
    }

    /// `d.DefaultLibs().Has(path)`, `None` when the function or the set is
    /// nil.
    fn default_libs_has(&self, path: &[u8]) -> Option<bool> {
        self.default_libs.as_ref()?.has(path)
    }

    // port: tsc/internal/testutil/fsbaselineutil/differ.go:FSDiffer.BaselineFSwithDiff
    pub fn baseline_fs_with_diff(&self, baseline: &mut Vec<u8>) {
        // todo: baselines the entire fs, possibly doesn't correctly diff all cases of emitted files, since emit isn't fully implemented and doesn't always emit the same way as strada
        let mut snap = BTreeMap::new();

        let mut diffs: BTreeMap<Vec<u8>, Vec<u8>> = BTreeMap::new();

        let previous = self.serialized_diff();
        for (path, file) in self.map_fs().entries() {
            if file.mode.is_symlink() {
                let Some(target) = self.map_fs().get_target_of_symlink(&path) else {
                    panic!(
                        "Failed to resolve symlink target: {}",
                        String::from_utf8_lossy(&path)
                    );
                };
                let new_entry = DiffEntry {
                    symlink_target: target,
                    ..DiffEntry::default()
                };
                self.add_fs_entry_diff(previous.as_deref(), &mut diffs, Some(&new_entry), &path);
                snap.insert(path, new_entry);
                // continue
            } else if file.mode.is_regular() {
                let content = sanitize_internal_symbol_name(&file.data).into_owned();
                let new_entry = DiffEntry {
                    content,
                    m_time: file.mod_time,
                    is_written: self.written_files.has(&path),
                    symlink_target: Vec::new(),
                };
                self.add_fs_entry_diff(previous.as_deref(), &mut diffs, Some(&new_entry), &path);
                snap.insert(path, new_entry);
            }
        }
        if let Some(previous) = previous.as_deref() {
            for path in previous.snap.keys() {
                if self.map_fs().get_file_info(path).is_none() {
                    // report deleted
                    self.add_fs_entry_diff(Some(previous), &mut diffs, None, path);
                }
            }
        }
        let default_libs = self
            .default_libs
            .as_ref()
            .and_then(|libs| libs.snapshot())
            .unwrap_or_default();
        *lock(&self.serialized_diff) = Some(Arc::new(Snapshot { snap, default_libs }));
        for (path, diff) in &diffs {
            baseline.extend_from_slice(b"//// [");
            baseline.extend_from_slice(path);
            baseline.extend_from_slice(b"] ");
            baseline.extend_from_slice(diff);
            baseline.push(b'\n');
        }
        baseline.push(b'\n');
        self.written_files.clear(); // Reset written files after baseline
    }

    // port: tsc/internal/testutil/fsbaselineutil/differ.go:FSDiffer.addFsEntryDiff
    fn add_fs_entry_diff(
        &self,
        previous: Option<&Snapshot>,
        diffs: &mut BTreeMap<Vec<u8>, Vec<u8>>,
        new_dir_content: Option<&DiffEntry>,
        path: &[u8],
    ) {
        let old_dir_content = previous.and_then(|snapshot| snapshot.snap.get(path));
        let default_libs = previous.map(|snapshot| &snapshot.default_libs);
        // todo handle more cases of fs changes
        let Some(old_dir_content) = old_dir_content else {
            let new_dir_content =
                new_dir_content.expect("a path is new only when it is in the file system");
            if self.default_libs_has(path) != Some(true) {
                let diff = if new_dir_content.symlink_target.is_empty() {
                    [b"*new* \n".as_slice(), &new_dir_content.content].concat()
                } else {
                    [b"-> ", new_dir_content.symlink_target.as_slice(), b" *new*"].concat()
                };
                diffs.insert(path.to_vec(), diff);
            }
            return;
        };
        let Some(new_dir_content) = new_dir_content else {
            diffs.insert(path.to_vec(), b"*deleted*".to_vec());
            return;
        };
        if new_dir_content.content != old_dir_content.content {
            diffs.insert(
                path.to_vec(),
                [b"*modified* \n".as_slice(), &new_dir_content.content].concat(),
            );
        } else if new_dir_content.is_written {
            diffs.insert(path.to_vec(), b"*rewrite with same content*".to_vec());
        } else if new_dir_content.m_time != old_dir_content.m_time {
            diffs.insert(path.to_vec(), b"*mTime changed*".to_vec());
        } else if default_libs.is_some_and(|libs| libs.contains(path))
            && self.default_libs_has(path) == Some(false)
        {
            // Lib file that was read
            diffs.insert(
                path.to_vec(),
                [b"*Lib*\n".as_slice(), &new_dir_content.content].concat(),
            );
        }
    }

    // port: tsc/internal/testutil/fsbaselineutil/differ.go:FSDiffer.ChangedPaths
    pub fn changed_paths(&self) -> Vec<FileChange> {
        let Some(old_snap) = self.serialized_diff() else {
            return Vec::new();
        };

        let mut changes = Vec::new();

        // Check current files against previous snapshot.
        for (path, file) in self.map_fs().entries() {
            if file.mode.is_symlink() || !file.mode.is_regular() {
                continue;
            }
            match old_snap.snap.get(&path) {
                // New file.
                None => changes.push(FileChange {
                    path,
                    deleted: false,
                }),
                // Modified or touched file.
                Some(old) if *file.data != *old.content || file.mod_time != old.m_time => {
                    changes.push(FileChange {
                        path,
                        deleted: false,
                    });
                }
                Some(_) => {}
            }
        }

        // Check for deleted files.
        for path in old_snap.snap.keys() {
            if self.map_fs().get_file_info(path).is_none() {
                changes.push(FileChange {
                    path: path.clone(),
                    deleted: true,
                });
            }
        }

        changes
    }
}

/// Replaces internal symbol names of shape `�@symbolName@123` with
/// `�@symbolName@<symbolId>` to avoid baselining differences in symbol
/// ids, which can change between runs.
///
/// The pin's `ReplaceAllStringFunc` over `\x{FFFD}@[^@]+@[0-9]+`: matches are
/// leftmost and do not overlap, the input is read as Go's regexp reads it
/// (an invalid UTF-8 byte is one `U+FFFD`), and `[^@]` matches newlines.
// port: tsc/internal/testutil/fsbaselineutil/differ.go:SanitizeInternalSymbolName
pub fn sanitize_internal_symbol_name(s: &[u8]) -> Cow<'_, [u8]> {
    if !contains(s, "\u{FFFD}@".as_bytes()) {
        return Cow::Borrowed(s);
    }
    let mut out = Vec::with_capacity(s.len());
    let (mut copied, mut at) = (0, 0);
    while at < s.len() {
        if let Some(end) = internal_symbol_match_at(s, at) {
            let matched = &s[at..end];
            let id_start = matched
                .iter()
                .rposition(|&b| b == b'@')
                .expect("a match ends in @ and digits");
            out.extend_from_slice(&s[copied..at]);
            out.extend_from_slice(&matched[..id_start]);
            out.extend_from_slice(b"@<symbolId>");
            (copied, at) = (end, end);
        } else {
            at += decode_rune(&s[at..]).1;
        }
    }
    out.extend_from_slice(&s[copied..]);
    Cow::Owned(out)
}

/// The end of a match of `\x{FFFD}@[^@]+@[0-9]+` starting at `at`. `[^@]+`
/// cannot cross an `@`, so the first `@` after the name is the separator.
fn internal_symbol_match_at(s: &[u8], at: usize) -> Option<usize> {
    let (rune, width) = decode_rune(&s[at..]);
    if rune != char::REPLACEMENT_CHARACTER {
        return None;
    }
    let mut i = at + width;
    if s.get(i) != Some(&b'@') {
        return None;
    }
    i += 1;
    let name = i;
    while i < s.len() && s[i] != b'@' {
        i += decode_rune(&s[i..]).1;
    }
    if i == name || i == s.len() {
        return None;
    }
    i += 1;
    let digits = i;
    while i < s.len() && s[i].is_ascii_digit() {
        i += 1;
    }
    (i > digits).then_some(i)
}
