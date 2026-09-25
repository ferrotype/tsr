//! Pinned embedded standard library bytes; these bypass BOM decoding.
use std::sync::Arc;
use tsr_jsstring::JsString;
use tsr_vfs::{Entries, Error, FileContent, FileInfo, FileSystem, SnapshotId};
pub const LIB_PATH: &[u8] = b"bundled:///libs";
pub const COPYRIGHT: &str = include_str!("../bundled/CopyrightNotice.txt");
pub static LIBRARIES: &[(&str, &[u8])] = &[
    ("lib.d.ts", include_bytes!("../bundled/libs/lib.d.ts")),
    (
        "lib.decorators.d.ts",
        include_bytes!("../bundled/libs/lib.decorators.d.ts"),
    ),
    (
        "lib.decorators.legacy.d.ts",
        include_bytes!("../bundled/libs/lib.decorators.legacy.d.ts"),
    ),
    (
        "lib.dom.asynciterable.d.ts",
        include_bytes!("../bundled/libs/lib.dom.asynciterable.d.ts"),
    ),
    (
        "lib.dom.d.ts",
        include_bytes!("../bundled/libs/lib.dom.d.ts"),
    ),
    (
        "lib.dom.iterable.d.ts",
        include_bytes!("../bundled/libs/lib.dom.iterable.d.ts"),
    ),
    (
        "lib.es2015.collection.d.ts",
        include_bytes!("../bundled/libs/lib.es2015.collection.d.ts"),
    ),
    (
        "lib.es2015.core.d.ts",
        include_bytes!("../bundled/libs/lib.es2015.core.d.ts"),
    ),
    (
        "lib.es2015.d.ts",
        include_bytes!("../bundled/libs/lib.es2015.d.ts"),
    ),
    (
        "lib.es2015.generator.d.ts",
        include_bytes!("../bundled/libs/lib.es2015.generator.d.ts"),
    ),
    (
        "lib.es2015.iterable.d.ts",
        include_bytes!("../bundled/libs/lib.es2015.iterable.d.ts"),
    ),
    (
        "lib.es2015.promise.d.ts",
        include_bytes!("../bundled/libs/lib.es2015.promise.d.ts"),
    ),
    (
        "lib.es2015.proxy.d.ts",
        include_bytes!("../bundled/libs/lib.es2015.proxy.d.ts"),
    ),
    (
        "lib.es2015.reflect.d.ts",
        include_bytes!("../bundled/libs/lib.es2015.reflect.d.ts"),
    ),
    (
        "lib.es2015.symbol.d.ts",
        include_bytes!("../bundled/libs/lib.es2015.symbol.d.ts"),
    ),
    (
        "lib.es2015.symbol.wellknown.d.ts",
        include_bytes!("../bundled/libs/lib.es2015.symbol.wellknown.d.ts"),
    ),
    (
        "lib.es2016.array.include.d.ts",
        include_bytes!("../bundled/libs/lib.es2016.array.include.d.ts"),
    ),
    (
        "lib.es2016.d.ts",
        include_bytes!("../bundled/libs/lib.es2016.d.ts"),
    ),
    (
        "lib.es2016.full.d.ts",
        include_bytes!("../bundled/libs/lib.es2016.full.d.ts"),
    ),
    (
        "lib.es2016.intl.d.ts",
        include_bytes!("../bundled/libs/lib.es2016.intl.d.ts"),
    ),
    (
        "lib.es2017.arraybuffer.d.ts",
        include_bytes!("../bundled/libs/lib.es2017.arraybuffer.d.ts"),
    ),
    (
        "lib.es2017.d.ts",
        include_bytes!("../bundled/libs/lib.es2017.d.ts"),
    ),
    (
        "lib.es2017.date.d.ts",
        include_bytes!("../bundled/libs/lib.es2017.date.d.ts"),
    ),
    (
        "lib.es2017.full.d.ts",
        include_bytes!("../bundled/libs/lib.es2017.full.d.ts"),
    ),
    (
        "lib.es2017.intl.d.ts",
        include_bytes!("../bundled/libs/lib.es2017.intl.d.ts"),
    ),
    (
        "lib.es2017.object.d.ts",
        include_bytes!("../bundled/libs/lib.es2017.object.d.ts"),
    ),
    (
        "lib.es2017.sharedmemory.d.ts",
        include_bytes!("../bundled/libs/lib.es2017.sharedmemory.d.ts"),
    ),
    (
        "lib.es2017.string.d.ts",
        include_bytes!("../bundled/libs/lib.es2017.string.d.ts"),
    ),
    (
        "lib.es2017.typedarrays.d.ts",
        include_bytes!("../bundled/libs/lib.es2017.typedarrays.d.ts"),
    ),
    (
        "lib.es2018.asyncgenerator.d.ts",
        include_bytes!("../bundled/libs/lib.es2018.asyncgenerator.d.ts"),
    ),
    (
        "lib.es2018.asynciterable.d.ts",
        include_bytes!("../bundled/libs/lib.es2018.asynciterable.d.ts"),
    ),
    (
        "lib.es2018.d.ts",
        include_bytes!("../bundled/libs/lib.es2018.d.ts"),
    ),
    (
        "lib.es2018.full.d.ts",
        include_bytes!("../bundled/libs/lib.es2018.full.d.ts"),
    ),
    (
        "lib.es2018.intl.d.ts",
        include_bytes!("../bundled/libs/lib.es2018.intl.d.ts"),
    ),
    (
        "lib.es2018.promise.d.ts",
        include_bytes!("../bundled/libs/lib.es2018.promise.d.ts"),
    ),
    (
        "lib.es2018.regexp.d.ts",
        include_bytes!("../bundled/libs/lib.es2018.regexp.d.ts"),
    ),
    (
        "lib.es2019.array.d.ts",
        include_bytes!("../bundled/libs/lib.es2019.array.d.ts"),
    ),
    (
        "lib.es2019.d.ts",
        include_bytes!("../bundled/libs/lib.es2019.d.ts"),
    ),
    (
        "lib.es2019.full.d.ts",
        include_bytes!("../bundled/libs/lib.es2019.full.d.ts"),
    ),
    (
        "lib.es2019.intl.d.ts",
        include_bytes!("../bundled/libs/lib.es2019.intl.d.ts"),
    ),
    (
        "lib.es2019.object.d.ts",
        include_bytes!("../bundled/libs/lib.es2019.object.d.ts"),
    ),
    (
        "lib.es2019.string.d.ts",
        include_bytes!("../bundled/libs/lib.es2019.string.d.ts"),
    ),
    (
        "lib.es2019.symbol.d.ts",
        include_bytes!("../bundled/libs/lib.es2019.symbol.d.ts"),
    ),
    (
        "lib.es2020.bigint.d.ts",
        include_bytes!("../bundled/libs/lib.es2020.bigint.d.ts"),
    ),
    (
        "lib.es2020.d.ts",
        include_bytes!("../bundled/libs/lib.es2020.d.ts"),
    ),
    (
        "lib.es2020.date.d.ts",
        include_bytes!("../bundled/libs/lib.es2020.date.d.ts"),
    ),
    (
        "lib.es2020.full.d.ts",
        include_bytes!("../bundled/libs/lib.es2020.full.d.ts"),
    ),
    (
        "lib.es2020.intl.d.ts",
        include_bytes!("../bundled/libs/lib.es2020.intl.d.ts"),
    ),
    (
        "lib.es2020.number.d.ts",
        include_bytes!("../bundled/libs/lib.es2020.number.d.ts"),
    ),
    (
        "lib.es2020.promise.d.ts",
        include_bytes!("../bundled/libs/lib.es2020.promise.d.ts"),
    ),
    (
        "lib.es2020.sharedmemory.d.ts",
        include_bytes!("../bundled/libs/lib.es2020.sharedmemory.d.ts"),
    ),
    (
        "lib.es2020.string.d.ts",
        include_bytes!("../bundled/libs/lib.es2020.string.d.ts"),
    ),
    (
        "lib.es2020.symbol.wellknown.d.ts",
        include_bytes!("../bundled/libs/lib.es2020.symbol.wellknown.d.ts"),
    ),
    (
        "lib.es2021.d.ts",
        include_bytes!("../bundled/libs/lib.es2021.d.ts"),
    ),
    (
        "lib.es2021.full.d.ts",
        include_bytes!("../bundled/libs/lib.es2021.full.d.ts"),
    ),
    (
        "lib.es2021.intl.d.ts",
        include_bytes!("../bundled/libs/lib.es2021.intl.d.ts"),
    ),
    (
        "lib.es2021.promise.d.ts",
        include_bytes!("../bundled/libs/lib.es2021.promise.d.ts"),
    ),
    (
        "lib.es2021.string.d.ts",
        include_bytes!("../bundled/libs/lib.es2021.string.d.ts"),
    ),
    (
        "lib.es2021.weakref.d.ts",
        include_bytes!("../bundled/libs/lib.es2021.weakref.d.ts"),
    ),
    (
        "lib.es2022.array.d.ts",
        include_bytes!("../bundled/libs/lib.es2022.array.d.ts"),
    ),
    (
        "lib.es2022.d.ts",
        include_bytes!("../bundled/libs/lib.es2022.d.ts"),
    ),
    (
        "lib.es2022.error.d.ts",
        include_bytes!("../bundled/libs/lib.es2022.error.d.ts"),
    ),
    (
        "lib.es2022.full.d.ts",
        include_bytes!("../bundled/libs/lib.es2022.full.d.ts"),
    ),
    (
        "lib.es2022.intl.d.ts",
        include_bytes!("../bundled/libs/lib.es2022.intl.d.ts"),
    ),
    (
        "lib.es2022.object.d.ts",
        include_bytes!("../bundled/libs/lib.es2022.object.d.ts"),
    ),
    (
        "lib.es2022.regexp.d.ts",
        include_bytes!("../bundled/libs/lib.es2022.regexp.d.ts"),
    ),
    (
        "lib.es2022.string.d.ts",
        include_bytes!("../bundled/libs/lib.es2022.string.d.ts"),
    ),
    (
        "lib.es2023.array.d.ts",
        include_bytes!("../bundled/libs/lib.es2023.array.d.ts"),
    ),
    (
        "lib.es2023.collection.d.ts",
        include_bytes!("../bundled/libs/lib.es2023.collection.d.ts"),
    ),
    (
        "lib.es2023.d.ts",
        include_bytes!("../bundled/libs/lib.es2023.d.ts"),
    ),
    (
        "lib.es2023.full.d.ts",
        include_bytes!("../bundled/libs/lib.es2023.full.d.ts"),
    ),
    (
        "lib.es2023.intl.d.ts",
        include_bytes!("../bundled/libs/lib.es2023.intl.d.ts"),
    ),
    (
        "lib.es2024.arraybuffer.d.ts",
        include_bytes!("../bundled/libs/lib.es2024.arraybuffer.d.ts"),
    ),
    (
        "lib.es2024.collection.d.ts",
        include_bytes!("../bundled/libs/lib.es2024.collection.d.ts"),
    ),
    (
        "lib.es2024.d.ts",
        include_bytes!("../bundled/libs/lib.es2024.d.ts"),
    ),
    (
        "lib.es2024.full.d.ts",
        include_bytes!("../bundled/libs/lib.es2024.full.d.ts"),
    ),
    (
        "lib.es2024.object.d.ts",
        include_bytes!("../bundled/libs/lib.es2024.object.d.ts"),
    ),
    (
        "lib.es2024.promise.d.ts",
        include_bytes!("../bundled/libs/lib.es2024.promise.d.ts"),
    ),
    (
        "lib.es2024.regexp.d.ts",
        include_bytes!("../bundled/libs/lib.es2024.regexp.d.ts"),
    ),
    (
        "lib.es2024.sharedmemory.d.ts",
        include_bytes!("../bundled/libs/lib.es2024.sharedmemory.d.ts"),
    ),
    (
        "lib.es2024.string.d.ts",
        include_bytes!("../bundled/libs/lib.es2024.string.d.ts"),
    ),
    (
        "lib.es2025.collection.d.ts",
        include_bytes!("../bundled/libs/lib.es2025.collection.d.ts"),
    ),
    (
        "lib.es2025.d.ts",
        include_bytes!("../bundled/libs/lib.es2025.d.ts"),
    ),
    (
        "lib.es2025.float16.d.ts",
        include_bytes!("../bundled/libs/lib.es2025.float16.d.ts"),
    ),
    (
        "lib.es2025.full.d.ts",
        include_bytes!("../bundled/libs/lib.es2025.full.d.ts"),
    ),
    (
        "lib.es2025.intl.d.ts",
        include_bytes!("../bundled/libs/lib.es2025.intl.d.ts"),
    ),
    (
        "lib.es2025.iterator.d.ts",
        include_bytes!("../bundled/libs/lib.es2025.iterator.d.ts"),
    ),
    (
        "lib.es2025.promise.d.ts",
        include_bytes!("../bundled/libs/lib.es2025.promise.d.ts"),
    ),
    (
        "lib.es2025.regexp.d.ts",
        include_bytes!("../bundled/libs/lib.es2025.regexp.d.ts"),
    ),
    (
        "lib.es5.d.ts",
        include_bytes!("../bundled/libs/lib.es5.d.ts"),
    ),
    (
        "lib.es6.d.ts",
        include_bytes!("../bundled/libs/lib.es6.d.ts"),
    ),
    (
        "lib.esnext.array.d.ts",
        include_bytes!("../bundled/libs/lib.esnext.array.d.ts"),
    ),
    (
        "lib.esnext.collection.d.ts",
        include_bytes!("../bundled/libs/lib.esnext.collection.d.ts"),
    ),
    (
        "lib.esnext.d.ts",
        include_bytes!("../bundled/libs/lib.esnext.d.ts"),
    ),
    (
        "lib.esnext.date.d.ts",
        include_bytes!("../bundled/libs/lib.esnext.date.d.ts"),
    ),
    (
        "lib.esnext.decorators.d.ts",
        include_bytes!("../bundled/libs/lib.esnext.decorators.d.ts"),
    ),
    (
        "lib.esnext.disposable.d.ts",
        include_bytes!("../bundled/libs/lib.esnext.disposable.d.ts"),
    ),
    (
        "lib.esnext.error.d.ts",
        include_bytes!("../bundled/libs/lib.esnext.error.d.ts"),
    ),
    (
        "lib.esnext.full.d.ts",
        include_bytes!("../bundled/libs/lib.esnext.full.d.ts"),
    ),
    (
        "lib.esnext.intl.d.ts",
        include_bytes!("../bundled/libs/lib.esnext.intl.d.ts"),
    ),
    (
        "lib.esnext.sharedmemory.d.ts",
        include_bytes!("../bundled/libs/lib.esnext.sharedmemory.d.ts"),
    ),
    (
        "lib.esnext.temporal.d.ts",
        include_bytes!("../bundled/libs/lib.esnext.temporal.d.ts"),
    ),
    (
        "lib.esnext.typedarrays.d.ts",
        include_bytes!("../bundled/libs/lib.esnext.typedarrays.d.ts"),
    ),
    (
        "lib.scripthost.d.ts",
        include_bytes!("../bundled/libs/lib.scripthost.d.ts"),
    ),
    (
        "lib.webworker.asynciterable.d.ts",
        include_bytes!("../bundled/libs/lib.webworker.asynciterable.d.ts"),
    ),
    (
        "lib.webworker.d.ts",
        include_bytes!("../bundled/libs/lib.webworker.d.ts"),
    ),
    (
        "lib.webworker.importscripts.d.ts",
        include_bytes!("../bundled/libs/lib.webworker.importscripts.d.ts"),
    ),
    (
        "lib.webworker.iterable.d.ts",
        include_bytes!("../bundled/libs/lib.webworker.iterable.d.ts"),
    ),
];
pub fn library(name: &[u8]) -> Option<&'static [u8]> {
    LIBRARIES
        .binary_search_by(|(key, _)| key.as_bytes().cmp(name))
        .ok()
        .map(|index| LIBRARIES[index].1)
}
pub fn is_bundled(path: &[u8]) -> bool {
    path.starts_with(b"bundled:///")
}
pub struct BundledFs {
    inner: Arc<dyn FileSystem>,
}
impl BundledFs {
    pub fn new(inner: Arc<dyn FileSystem>) -> Self {
        Self { inner }
    }
}
// Available only to test consumers that have a source checkout. Cargo's
// remapped source filenames do not change CARGO_MANIFEST_DIR.
#[cfg(feature = "test-support")]
/// port: tsc/internal/bundled/bundled.go:TestingLibPath
pub fn testing_lib_path() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("bundled/libs")
}

/// The pinned `fileInfo` a bundled entry reports (embed.go:85, :91, :95 and
/// the generated `libsEntries`): the name is `""` for the scheme root, `libs`
/// for the library directory and the bare library name for an asset.
fn file_info(name: &[u8], directory: bool, size: u64) -> FileInfo {
    FileInfo {
        name: JsString::from_bytes(name),
        ..FileInfo::basic(directory, size)
    }
}

impl BundledFs {
    /// port: tsc/internal/bundled/embed.go:wrappedFS.walkDir
    fn walk_bundled(
        rest: &[u8],
        visit: &mut tsr_vfs::WalkCallback<'_>,
    ) -> Result<tsr_vfs::WalkControl, Error> {
        use tsr_vfs::{WalkControl as C, WalkEntry};
        if rest.is_empty() {
            let entry = WalkEntry {
                name: JsString::from_bytes(b"libs".as_slice()),
                info: file_info(b"libs", true, 0),
                symlink: false,
            };
            match visit(b"bundled:////libs", Some(&entry), None)? {
                C::SkipAll => return Ok(C::SkipAll),
                C::SkipDir => return Ok(C::Continue),
                C::Continue => return Self::walk_bundled(b"libs", visit),
            }
        }
        if rest == b"libs" {
            for &(name, bytes) in LIBRARIES {
                let mut path = b"bundled:///libs/".to_vec();
                path.extend_from_slice(name.as_bytes());
                let entry = WalkEntry {
                    name: JsString::from_bytes(name.as_bytes()),
                    info: file_info(name.as_bytes(), false, bytes.len() as u64),
                    symlink: false,
                };
                // The embedded implementation treats SkipDir on a file as
                // continue, unlike io/fs.WalkDir. Preserve that pinned quirk.
                if visit(&path, Some(&entry), None)? == C::SkipAll {
                    return Ok(C::SkipAll);
                }
            }
        }
        Ok(C::Continue)
    }
}
impl FileSystem for BundledFs {
    fn use_case_sensitive_file_names(&self) -> bool {
        self.inner.use_case_sensitive_file_names()
    }
    fn snapshot_id(&self) -> Option<SnapshotId> {
        self.inner.snapshot_id()
    }
    fn read_file(&self, path: &[u8]) -> Result<Option<FileContent>, Error> {
        if let Some(rest) = path.strip_prefix(b"bundled:///") {
            return Ok(rest
                .strip_prefix(b"libs/")
                .and_then(library)
                .map(FileContent::loaded));
        }
        self.inner.read_file(path)
    }
    fn stat(&self, path: &[u8]) -> Result<Option<FileInfo>, Error> {
        if let Some(rest) = path.strip_prefix(b"bundled:///") {
            return Ok(if rest == b"libs" || rest.is_empty() {
                Some(file_info(rest, true, 0))
            } else {
                rest.strip_prefix(b"libs/").and_then(|name| {
                    library(name).map(|bytes| file_info(name, false, bytes.len() as u64))
                })
            });
        }
        self.inner.stat(path)
    }
    fn directory_exists(&self, path: &[u8]) -> Result<bool, Error> {
        if let Some(rest) = path.strip_prefix(b"bundled:///") {
            return Ok(rest == b"libs");
        }
        self.inner.directory_exists(path)
    }
    fn entries(&self, path: &[u8]) -> Result<Entries, Error> {
        if let Some(rest) = path.strip_prefix(b"bundled:///") {
            let mut entries = Entries::default();
            if rest.is_empty() {
                entries
                    .directories
                    .get_or_insert_with(Vec::new)
                    .push(JsString::from_bytes(b"libs".as_slice()));
            } else if rest == b"libs" {
                entries.files = Some(
                    LIBRARIES
                        .iter()
                        .map(|(name, _)| JsString::from_bytes(name.as_bytes()))
                        .collect(),
                );
            }
            return Ok(entries);
        }
        self.inner.entries(path)
    }
    /// port: tsc/internal/bundled/embed.go:wrappedFS.WalkDir
    fn walk_dir(&self, root: &[u8], visit: &mut tsr_vfs::WalkCallback<'_>) -> Result<(), Error> {
        if let Some(rest) = root.strip_prefix(b"bundled:///") {
            Self::walk_bundled(rest, visit).map(|_| ())
        } else {
            self.inner.walk_dir(root, visit)
        }
    }
    /// port: tsc/internal/bundled/embed.go:wrappedFS.WriteFile
    fn write_file(&self, path: &[u8], data: &[u8]) -> Result<(), Error> {
        assert!(!is_bundled(path), "cannot write to embedded file system");
        self.inner.write_file(path, data)
    }
    /// port: tsc/internal/bundled/embed.go:wrappedFS.AppendFile
    fn append_file(&self, path: &[u8], data: &[u8]) -> Result<(), Error> {
        assert!(!is_bundled(path), "cannot write to embedded file system");
        self.inner.append_file(path, data)
    }
    /// port: tsc/internal/bundled/embed.go:wrappedFS.Remove
    fn remove(&self, path: &[u8]) -> Result<(), Error> {
        assert!(!is_bundled(path), "cannot remove from embedded file system");
        self.inner.remove(path)
    }
    /// port: tsc/internal/bundled/embed.go:wrappedFS.Chtimes
    fn change_times(
        &self,
        path: &[u8],
        a_time: tsr_vfs::iofs::Time,
        m_time: tsr_vfs::iofs::Time,
    ) -> Result<(), Error> {
        assert!(
            !is_bundled(path),
            "cannot change times on embedded file system"
        );
        self.inner.change_times(path, a_time, m_time)
    }
    fn realpath(&self, path: &[u8]) -> Result<JsString, Error> {
        if is_bundled(path) {
            Ok(JsString::from_bytes(path))
        } else {
            self.inner.realpath(path)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tsr_vfs::{MemoryBuilder, WalkControl};

    fn filesystem() -> BundledFs {
        BundledFs::new(Arc::new(MemoryBuilder::new(b"/", true).finish()))
    }

    fn name(info: &FileInfo) -> &[u8] {
        info.name.as_bytes()
    }

    #[test]
    fn stat_reports_the_pinned_entry_names() {
        let fs = filesystem();
        let root = fs.stat(b"bundled:///").unwrap().unwrap();
        assert_eq!((name(&root), root.directory), (b"".as_slice(), true));
        let libs = fs.stat(b"bundled:///libs").unwrap().unwrap();
        assert_eq!((name(&libs), libs.directory), (b"libs".as_slice(), true));
        let lib = fs.stat(b"bundled:///libs/lib.es5.d.ts").unwrap().unwrap();
        assert_eq!(name(&lib), b"lib.es5.d.ts");
        assert_eq!(lib.size, library(b"lib.es5.d.ts").unwrap().len() as u64);
        assert!(!lib.directory);
    }

    #[test]
    fn walk_entry_info_carries_the_entry_name() {
        let fs = filesystem();
        let mut seen = Vec::new();
        fs.walk_dir(b"bundled:///", &mut |path, entry, _| {
            let entry = entry.unwrap();
            assert_eq!(entry.name.as_bytes(), name(&entry.info), "{path:?}");
            seen.push(entry.info.name.clone());
            Ok(WalkControl::Continue)
        })
        .unwrap();
        assert_eq!(seen.len(), LIBRARIES.len() + 1);
        assert_eq!(seen[0].as_bytes(), b"libs");
        assert_eq!(seen[1].as_bytes(), LIBRARIES[0].0.as_bytes());
    }
}
