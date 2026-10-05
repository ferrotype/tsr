//! Resolver-only view of unbuilt project declarations. It advertises output
//! paths whose source exists; parsing redirects to that source separately.
use crate::project_references::ReferenceFile;
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};
use tsr_jsstring::JsString;
use tsr_module::symlinks::{KnownDirectoryLink, KnownSymlinks};
use tsr_tspath::{self as path, Path};
use tsr_vfs::{
    Entries, Error, FileContent, FileInfo, FileSystem, ReadResult, SnapshotId, WalkCallback,
};

pub(crate) struct ProjectReferenceDtsFakingHost {
    host: Arc<dyn FileSystem>,
    cwd: JsString,
    outputs: Arc<BTreeMap<JsString, Arc<ReferenceFile>>>,
    dts_directories: Arc<BTreeSet<Path>>,
    known_symlinks: KnownSymlinks,
}

impl ProjectReferenceDtsFakingHost {
    /// port: tsc/internal/compiler/projectreferencedtsfakinghost.go:newProjectReferenceDtsFakingHost
    pub(crate) fn new(
        host: Arc<dyn FileSystem>,
        cwd: JsString,
        outputs: Arc<BTreeMap<JsString, Arc<ReferenceFile>>>,
        dts_directories: Arc<BTreeSet<Path>>,
    ) -> Self {
        let known_symlinks =
            KnownSymlinks::new(cwd.as_bytes(), host.use_case_sensitive_file_names());
        Self {
            host,
            cwd,
            outputs,
            dts_directories,
            known_symlinks,
        }
    }

    /// port: tsc/internal/compiler/projectreferencedtsfakinghost.go:projectReferenceDtsFakingVfs.toPath
    fn to_path(&self, name: &[u8]) -> Path {
        path::to_path(
            name,
            self.cwd.as_bytes(),
            self.use_case_sensitive_file_names(),
        )
        .into()
    }

    /// port: tsc/internal/compiler/projectreferencedtsfakinghost.go:projectReferenceDtsFakingVfs.handleDirectoryCouldBeSymlink
    fn handle_directory_could_be_symlink(&self, directory: &[u8]) -> Result<(), Error> {
        if path::contains_ignored_path(directory) || !in_node_modules(directory) {
            return Ok(());
        }
        let directory_path = self
            .to_path(directory)
            .ensure_trailing_directory_separator();
        if self
            .known_symlinks
            .directories()
            .load(&directory_path)
            .is_some()
        {
            return Ok(());
        }
        let real_directory = self.realpath(directory)?;
        if real_directory.as_bytes() == directory {
            return Ok(());
        }
        let real_path = self
            .to_path(real_directory.as_bytes())
            .ensure_trailing_directory_separator();
        if real_path != directory_path {
            self.known_symlinks.set_directory(
                directory,
                directory_path,
                Some(KnownDirectoryLink {
                    real: path::ensure_trailing_directory_separator(real_directory.as_bytes())
                        .into_owned(),
                    real_path,
                }),
            );
        }
        Ok(())
    }

    /// port: tsc/internal/compiler/projectreferencedtsfakinghost.go:projectReferenceDtsFakingVfs.fileExistsIfProjectReferenceDts
    fn file_exists_if_project_reference_dts(&self, file: &[u8]) -> Result<Option<bool>, Error> {
        self.outputs
            .get(self.to_path(file).as_bytes())
            .map(|source| self.host.file_exists(source.source.as_bytes()))
            .transpose()
    }

    /// port: tsc/internal/compiler/projectreferencedtsfakinghost.go:projectReferenceDtsFakingVfs.directoryExistsIfProjectReferenceDeclDir
    fn directory_exists_if_project_reference_decl_dir(&self, directory: &[u8]) -> Option<bool> {
        let directory = self.to_path(directory);
        self.dts_directories
            .iter()
            .any(|decl| directory.contains_path(decl) || decl.contains_path(&directory))
            .then_some(true)
    }

    fn exists_using_source(&self, name: &[u8], is_file: bool) -> Result<Option<bool>, Error> {
        if is_file {
            self.file_exists_if_project_reference_dts(name)
        } else {
            Ok(self.directory_exists_if_project_reference_decl_dir(name))
        }
    }

    /// port: tsc/internal/compiler/projectreferencedtsfakinghost.go:projectReferenceDtsFakingVfs.fileOrDirectoryExistsUsingSource
    fn file_or_directory_exists_using_source(
        &self,
        name: &[u8],
        is_file: bool,
    ) -> Result<bool, Error> {
        if let Some(exists) = self.exists_using_source(name, is_file)? {
            return Ok(exists);
        }
        let canonical = self.to_path(name);
        if !in_node_modules(canonical.as_bytes()) {
            return Ok(false);
        }
        let package_root = tsr_module::parse_node_module_from_path(name, true);
        if !package_root.is_empty() {
            self.handle_directory_could_be_symlink(&package_root)?;
        }
        if self.known_symlinks.directories().is_empty() {
            return Ok(false);
        }
        if is_file && self.known_symlinks.files().load(&canonical).is_some() {
            return Ok(true);
        }
        let mut result = Ok(false);
        // SyncMap::range releases its guards before invoking the callback;
        // source filesystem calls never run while a cache lock is held.
        self.known_symlinks.directories().range(|directory, link| {
            let Some(link) = link else {
                return true;
            };
            let Some(relative) = canonical.as_bytes().strip_prefix(directory.as_bytes()) else {
                return true;
            };
            let mut real = link.real_path.as_bytes().to_vec();
            real.extend_from_slice(relative);
            match self.exists_using_source(&real, is_file) {
                Ok(Some(true)) => {
                    if is_file {
                        let absolute = path::absolute(name, self.cwd.as_bytes());
                        let mut real_file = link.real;
                        real_file.extend_from_slice(&absolute[directory.as_bytes().len()..]);
                        self.known_symlinks
                            .set_file(&absolute, canonical.clone(), &real_file);
                    }
                    result = Ok(true);
                    false
                }
                Ok(_) => true,
                Err(error) => {
                    result = Err(error);
                    false
                }
            }
        });
        result
    }
}

fn in_node_modules(name: &[u8]) -> bool {
    name.windows(b"/node_modules/".len())
        .any(|part| part == b"/node_modules/")
}

impl FileSystem for ProjectReferenceDtsFakingHost {
    fn snapshot_id(&self) -> Option<SnapshotId> {
        self.host.snapshot_id()
    }

    /// port: tsc/internal/compiler/projectreferencedtsfakinghost.go:projectReferenceDtsFakingVfs.UseCaseSensitiveFileNames
    fn use_case_sensitive_file_names(&self) -> bool {
        self.host.use_case_sensitive_file_names()
    }

    /// port: tsc/internal/compiler/projectreferencedtsfakinghost.go:projectReferenceDtsFakingVfs.FileExists
    fn file_exists(&self, name: &[u8]) -> Result<bool, Error> {
        if self.host.file_exists(name)? {
            return Ok(true);
        }
        if !path::is_declaration_file_name(name) {
            return Ok(false);
        }
        self.file_or_directory_exists_using_source(name, true)
    }

    /// port: tsc/internal/compiler/projectreferencedtsfakinghost.go:projectReferenceDtsFakingVfs.DirectoryExists
    fn directory_exists(&self, name: &[u8]) -> Result<bool, Error> {
        if self.host.directory_exists(name)? {
            self.handle_directory_could_be_symlink(name)?;
            return Ok(true);
        }
        self.file_or_directory_exists_using_source(name, false)
    }

    /// port: tsc/internal/compiler/projectreferencedtsfakinghost.go:projectReferenceDtsFakingVfs.ReadFile
    fn read_file(&self, name: &[u8]) -> Result<Option<FileContent>, Error> {
        self.host.read_file(name)
    }
    fn read_file_result(&self, name: &[u8]) -> Result<ReadResult, Error> {
        self.host.read_file_result(name)
    }

    /// port: tsc/internal/compiler/projectreferencedtsfakinghost.go:projectReferenceDtsFakingVfs.Realpath
    fn realpath(&self, name: &[u8]) -> Result<JsString, Error> {
        if let Some(real) = self.known_symlinks.files().load(&self.to_path(name)) {
            return Ok(JsString::from_bytes(real));
        }
        self.host.realpath(name)
    }

    /// port: tsc/internal/compiler/projectreferencedtsfakinghost.go:projectReferenceDtsFakingVfs.Stat
    fn stat(&self, _name: &[u8]) -> Result<Option<FileInfo>, Error> {
        panic!("should not be called by resolver")
    }
    /// port: tsc/internal/compiler/projectreferencedtsfakinghost.go:projectReferenceDtsFakingVfs.GetAccessibleEntries
    fn entries(&self, _name: &[u8]) -> Result<Entries, Error> {
        panic!("should not be called by resolver")
    }
    /// port: tsc/internal/compiler/projectreferencedtsfakinghost.go:projectReferenceDtsFakingVfs.WalkDir
    fn walk_dir(&self, _root: &[u8], _visit: &mut WalkCallback<'_>) -> Result<(), Error> {
        panic!("should not be called by resolver")
    }
    /// port: tsc/internal/compiler/projectreferencedtsfakinghost.go:projectReferenceDtsFakingVfs.WriteFile
    fn write_file(&self, _name: &[u8], _data: &[u8]) -> Result<(), Error> {
        panic!("should not be called by resolver")
    }
    /// port: tsc/internal/compiler/projectreferencedtsfakinghost.go:projectReferenceDtsFakingVfs.AppendFile
    fn append_file(&self, _name: &[u8], _data: &[u8]) -> Result<(), Error> {
        panic!("should not be called by resolver")
    }
    /// port: tsc/internal/compiler/projectreferencedtsfakinghost.go:projectReferenceDtsFakingVfs.Remove
    fn remove(&self, _name: &[u8]) -> Result<(), Error> {
        panic!("should not be called by resolver")
    }
    /// port: tsc/internal/compiler/projectreferencedtsfakinghost.go:projectReferenceDtsFakingVfs.Chtimes
    fn change_times(
        &self,
        _name: &[u8],
        _access: tsr_vfs::iofs::Time,
        _modified: tsr_vfs::iofs::Time,
    ) -> Result<(), Error> {
        panic!("should not be called by resolver")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn host(case_sensitive: bool) -> ProjectReferenceDtsFakingHost {
        let mut fs = tsr_vfs::MemoryBuilder::new(b"/src", case_sensitive);
        fs.insert_loaded(b"/src/lib/input.ts", b"export const value = 1;".as_slice());
        fs.insert_loaded(b"/src/lib/package.json", b"{}".as_slice());
        fs.insert_symlink(b"/src/node_modules/linked", b"/src/lib");
        let key = path::to_path(b"/src/lib/types/input.d.ts", b"/src", case_sensitive);
        ProjectReferenceDtsFakingHost::new(
            Arc::new(fs.finish()),
            JsString::from_bytes("/src".as_bytes()),
            Arc::new(BTreeMap::from([(
                key,
                Arc::new(ReferenceFile {
                    source: JsString::from_bytes("/src/lib/input.ts".as_bytes()),
                    output_dts: JsString::from_bytes("/src/lib/types/input.d.ts".as_bytes()),
                    config: 0,
                }),
            )])),
            Arc::new(BTreeSet::from([path::to_path(
                b"/src/lib/types",
                b"/src",
                case_sensitive,
            )
            .into()])),
        )
    }

    #[test]
    fn declaration_existence_does_not_fabricate_contents_or_other_files() {
        let fs = host(true);
        assert!(fs.file_exists(b"/src/lib/types/input.d.ts").unwrap());
        assert!(fs
            .read_file(b"/src/lib/types/input.d.ts")
            .unwrap()
            .is_none());
        assert!(!fs.file_exists(b"/src/lib/types/input.js").unwrap());
        assert!(!fs.file_exists(b"/src/lib/types/other.d.ts").unwrap());
        assert!(fs.read_file(b"/src/lib/input.ts").unwrap().is_some());
    }

    #[test]
    fn declaration_directory_ancestors_and_descendants_obey_component_boundaries() {
        let fs = host(true);
        for path in [
            b"/src/lib/types".as_slice(),
            b"/src/lib/types/deep",
            b"/src/lib",
            b"/src",
        ] {
            assert!(fs.directory_exists(path).unwrap());
        }
        assert!(!fs.directory_exists(b"/src/lib/types-other").unwrap());
        assert!(!fs.directory_exists(b"/src/sibling").unwrap());
    }

    #[test]
    fn symlinked_missing_output_remembers_real_spelling() {
        let fs = host(false);
        assert!(fs
            .directory_exists(b"/src/node_modules/linked/types")
            .unwrap());
        assert!(fs
            .file_exists(b"/src/node_modules/linked/types/INPUT.d.ts")
            .unwrap());
        assert_eq!(
            fs.realpath(b"/src/node_modules/linked/types/INPUT.d.ts")
                .unwrap()
                .as_bytes(),
            b"/src/lib/types/INPUT.d.ts"
        );
        assert!(fs
            .read_file(b"/src/node_modules/linked/types/INPUT.d.ts")
            .unwrap()
            .is_none());
        assert!(!fs
            .file_exists(b"/src/node_modules/linked/types/missing.d.ts")
            .unwrap());
    }

    #[test]
    #[should_panic(expected = "should not be called by resolver")]
    fn resolver_only_host_rejects_entry_enumeration() {
        let _ = host(true).entries(b"/src");
    }
}
