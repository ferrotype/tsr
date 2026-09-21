//! The known-symlink cache of `internal/symlinks`.
//!
//! Four concurrent indexes: a forward directory map to an optional link, a
//! reverse map from realpath to the symlink strings that reach it, and the same
//! pair for files. The current directory and case rule are captured once by the
//! constructor and read off the cache by every operation.
//!
//! Both reverse indexes grow only on a symlink key's first insertion, while the
//! forward maps are overwritten every time. A directory entry may hold no link:
//! it is present, it blocks the reverse index, and `has_directory` answers true.
use std::sync::Arc;
use tsr_core::collections::{SyncMap, SyncSet};
use tsr_tspath::{self as path, Path};

/// Source type: tsc/internal/symlinks/knownsymlinks.go:KnownDirectoryLink
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KnownDirectoryLink {
    /// The realpath in its original casing, with a trailing separator.
    pub real: Vec<u8>,
    /// The canonical form of `real`, with a trailing separator.
    pub real_path: Path,
}
/// The symlink strings a caller passed, not paths.
pub type SymlinkSet = Arc<SyncSet<Vec<u8>>>;

/// Source type: tsc/internal/symlinks/knownsymlinks.go:KnownSymlinks
#[derive(Debug, Default)]
pub struct KnownSymlinks {
    directories: SyncMap<Path, Option<KnownDirectoryLink>>,
    directories_by_realpath: SyncMap<Path, SymlinkSet>,
    files: SyncMap<Path, Vec<u8>>,
    files_by_realpath: SyncMap<Path, SymlinkSet>,
    cwd: Vec<u8>,
    case_sensitive: bool,
}
impl KnownSymlinks {
    /// port: tsc/internal/symlinks/knownsymlinks.go:NewKnownSymlink
    pub fn new(current_directory: &[u8], use_case_sensitive_file_names: bool) -> Self {
        Self {
            cwd: current_directory.to_vec(),
            case_sensitive: use_case_sensitive_file_names,
            ..Self::default()
        }
    }
    pub fn current_directory(&self) -> &[u8] {
        &self.cwd
    }
    pub fn use_case_sensitive_file_names(&self) -> bool {
        self.case_sensitive
    }
    /// port: tsc/internal/symlinks/knownsymlinks.go:KnownSymlinks.HasDirectory
    pub fn has_directory(&self, symlink_path: &Path) -> bool {
        self.directories
            .load(&symlink_path.ensure_trailing_directory_separator())
            .is_some()
    }
    /// The accessors hand back the cache's own storage, so a write made through
    /// one is visible to the cache's methods.
    /// port: tsc/internal/symlinks/knownsymlinks.go:KnownSymlinks.Directories
    pub fn directories(&self) -> &SyncMap<Path, Option<KnownDirectoryLink>> {
        &self.directories
    }
    /// port: tsc/internal/symlinks/knownsymlinks.go:KnownSymlinks.DirectoriesByRealpath
    pub fn directories_by_realpath(&self) -> &SyncMap<Path, SymlinkSet> {
        &self.directories_by_realpath
    }
    /// port: tsc/internal/symlinks/knownsymlinks.go:KnownSymlinks.Files
    pub fn files(&self) -> &SyncMap<Path, Vec<u8>> {
        &self.files
    }
    /// port: tsc/internal/symlinks/knownsymlinks.go:KnownSymlinks.FilesByRealpath
    pub fn files_by_realpath(&self) -> &SyncMap<Path, SymlinkSet> {
        &self.files_by_realpath
    }
    /// An absent link is stored without touching the reverse index.
    /// port: tsc/internal/symlinks/knownsymlinks.go:KnownSymlinks.SetDirectory
    pub fn set_directory(
        &self,
        symlink: &[u8],
        symlink_path: Path,
        real_directory: Option<KnownDirectoryLink>,
    ) {
        if let Some(link) = &real_directory {
            if self.directories.load(&symlink_path).is_none() {
                let (set, _) = self
                    .directories_by_realpath
                    .load_or_store(link.real_path.clone(), SymlinkSet::default());
                set.insert(symlink.to_vec());
            }
        }
        self.directories.store(symlink_path, real_directory);
    }
    /// port: tsc/internal/symlinks/knownsymlinks.go:KnownSymlinks.SetFile
    pub fn set_file(&self, symlink: &[u8], symlink_path: Path, realpath: &[u8]) {
        if self.files.load(&symlink_path).is_none() {
            let (set, _) = self
                .files_by_realpath
                .load_or_store(self.to_path(realpath), SymlinkSet::default());
            set.insert(symlink.to_vec());
        }
        self.files.store(symlink_path, realpath.to_vec());
    }
    /// The two drivers are the caller's. Each hands every resolution's original
    /// path and resolved file name to the callback; modules are driven first.
    /// port: tsc/internal/symlinks/knownsymlinks.go:KnownSymlinks.SetSymlinksFromResolutions
    pub fn set_symlinks_from_resolutions(
        &self,
        for_each_resolved_module: impl FnOnce(&mut dyn FnMut(&[u8], &[u8])),
        for_each_resolved_type_reference_directive: impl FnOnce(&mut dyn FnMut(&[u8], &[u8])),
    ) {
        for_each_resolved_module(&mut |original, resolved| {
            self.process_resolution(original, resolved);
        });
        for_each_resolved_type_reference_directive(&mut |original, resolved| {
            self.process_resolution(original, resolved);
        });
    }
    /// The file is recorded unconditionally; the directory only when the walk
    /// consumed a component and the symlink path is not an ignored one.
    /// port: tsc/internal/symlinks/knownsymlinks.go:KnownSymlinks.ProcessResolution
    pub fn process_resolution(&self, original_path: &[u8], resolved_file_name: &[u8]) {
        if original_path.is_empty() || resolved_file_name.is_empty() {
            return;
        }
        self.set_file(
            original_path,
            self.to_path(original_path),
            resolved_file_name,
        );
        let Some((common_resolved, common_original)) =
            self.guess_directory_symlink(resolved_file_name, original_path, &self.cwd)
        else {
            return;
        };
        if common_resolved.is_empty() || common_original.is_empty() {
            return;
        }
        let symlink_path = self.to_path(&common_original);
        if path::contains_ignored_path(symlink_path.as_bytes()) {
            return;
        }
        self.set_directory(
            &common_original,
            symlink_path.ensure_trailing_directory_separator(),
            Some(KnownDirectoryLink {
                real: path::ensure_trailing_directory_separator(&common_resolved).into_owned(),
                real_path: self
                    .to_path(&common_resolved)
                    .ensure_trailing_directory_separator(),
            }),
        );
    }
    /// Walks both paths upward while their last components agree under the case
    /// rule and neither parent is `node_modules` or a scoped package. `None`
    /// when no component was consumed. The current directory is this method's
    /// own argument, not the cache's.
    /// port: tsc/internal/symlinks/knownsymlinks.go:KnownSymlinks.guessDirectorySymlink
    pub fn guess_directory_symlink(
        &self,
        a: &[u8],
        b: &[u8],
        cwd: &[u8],
    ) -> Option<(Vec<u8>, Vec<u8>)> {
        let mut a_parts = path::path_components(&path::absolute(a, cwd), b"");
        let mut b_parts = path::path_components(&path::absolute(b, cwd), b"");
        let mut is_directory = false;
        while a_parts.len() >= 2
            && b_parts.len() >= 2
            && !self.is_node_modules_or_scoped_package_directory(&a_parts[a_parts.len() - 2])
            && !self.is_node_modules_or_scoped_package_directory(&b_parts[b_parts.len() - 2])
            && path::canonical(&a_parts[a_parts.len() - 1], self.case_sensitive)
                == path::canonical(&b_parts[b_parts.len() - 1], self.case_sensitive)
        {
            a_parts.pop();
            b_parts.pop();
            is_directory = true;
        }
        is_directory.then(|| {
            (
                path::path_from_components(&a_parts),
                path::path_from_components(&b_parts),
            )
        })
    }
    /// The scope test is on the raw name; only `node_modules` is case-folded.
    /// port: tsc/internal/symlinks/knownsymlinks.go:KnownSymlinks.isNodeModulesOrScopedPackageDirectory
    pub fn is_node_modules_or_scoped_package_directory(&self, name: &[u8]) -> bool {
        !name.is_empty()
            && (path::canonical(name, self.case_sensitive).as_ref() == b"node_modules"
                || name.starts_with(b"@"))
    }
    fn to_path(&self, file: &[u8]) -> Path {
        Path::from(path::to_path(file, &self.cwd, self.case_sensitive))
    }
}
