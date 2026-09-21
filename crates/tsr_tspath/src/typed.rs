//! The typed path and the callback-shaped ancestor walks.
use crate::{directory, ensure_trailing_directory_separator, remove_trailing_directory_separator};
use tsr_jsstring::JsString;

/// A path that is already rooted, reduced and case-folded by `to_path`. Its
/// operations add no canonicalisation of their own.
#[derive(Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Path(JsString);
impl Path {
    pub fn from_bytes(bytes: impl Into<Vec<u8>>) -> Self {
        Self(JsString::from_bytes(bytes.into()))
    }
    pub fn as_bytes(&self) -> &[u8] {
        self.0.as_bytes()
    }
    pub fn into_js_string(self) -> JsString {
        self.0
    }
    /// A byte prefix test with a separator boundary: no current directory and
    /// no comparer.
    /// port: tsc/internal/tspath/path.go:Path.ContainsPath
    pub fn contains_path(&self, child: &Self) -> bool {
        let (parent, child) = (self.as_bytes(), child.as_bytes());
        if parent.is_empty() {
            return false;
        }
        parent == child
            || child.len() > parent.len()
                && child.starts_with(parent)
                && (parent.ends_with(b"/") || child[parent.len()] == b'/')
    }
    /// port: tsc/internal/tspath/path.go:Path.GetDirectoryPath
    #[must_use]
    pub fn directory_path(&self) -> Self {
        Self::from_bytes(directory(self.as_bytes()))
    }
    /// port: tsc/internal/tspath/path.go:Path.RemoveTrailingDirectorySeparator
    #[must_use]
    pub fn remove_trailing_directory_separator(&self) -> Self {
        Self::from_bytes(remove_trailing_directory_separator(self.as_bytes()))
    }
    /// port: tsc/internal/tspath/path.go:Path.EnsureTrailingDirectorySeparator
    #[must_use]
    pub fn ensure_trailing_directory_separator(&self) -> Self {
        Self::from_bytes(ensure_trailing_directory_separator(self.as_bytes()).into_owned())
    }
}
impl From<JsString> for Path {
    fn from(value: JsString) -> Self {
        Self(value)
    }
}

/// The callback answers `(value, stop)`. The walk returns that value when it
/// stops and `None` when it runs out of ancestors.
/// port: tsc/internal/tspath/path.go:ForEachAncestorDirectory
pub fn for_each_ancestor_directory<T>(
    directory_name: &[u8],
    mut callback: impl FnMut(&[u8]) -> (T, bool),
) -> Option<T> {
    let mut current = directory_name.to_vec();
    loop {
        let (value, stop) = callback(&current);
        if stop {
            return Some(value);
        }
        let parent = directory(&current);
        if parent == current {
            return None;
        }
        current = parent;
    }
}
/// Also stops at the global cache location, after calling the callback there.
/// `None` means the walk ran out without stopping.
/// port: tsc/internal/tspath/path.go:ForEachAncestorDirectoryStoppingAtGlobalCache
pub fn for_each_ancestor_directory_stopping_at_global_cache<T>(
    global_cache_location: &[u8],
    directory_name: &[u8],
    mut callback: impl FnMut(&[u8]) -> (T, bool),
) -> Option<T> {
    for_each_ancestor_directory(directory_name, |ancestor| {
        let (value, stop) = callback(ancestor);
        (value, stop || ancestor == global_cache_location)
    })
}
/// Each ancestor is handed back as a `Path` without re-canonicalising it, so
/// the separators of a non-canonical input change mid-walk.
/// port: tsc/internal/tspath/path.go:ForEachAncestorDirectoryPath
pub fn for_each_ancestor_directory_path<T>(
    directory_name: &Path,
    mut callback: impl FnMut(&Path) -> (T, bool),
) -> Option<T> {
    for_each_ancestor_directory(directory_name.as_bytes(), |ancestor| {
        callback(&Path::from_bytes(ancestor))
    })
}
