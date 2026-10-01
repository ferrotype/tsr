//! The transpilation's file system: the input file and, for declarations,
//! the barebones library, under their exact paths. Any other access is a
//! defect of the caller and panics, as the pin's `transpileFS` does by
//! embedding no file system for the operations it does not implement.
use std::collections::BTreeMap;
use tsr_jsstring::{go_quote, JsString};
use tsr_vfs::{
    iofs::Time, Entries, Error, FileContent, FileInfo, FileSystem, MemoryBuilder, MemorySnapshot,
    SnapshotId,
};

/// The pin's `transpileFS`.
pub(crate) struct TranspileFs {
    /// The files by their exact path; the texts are read as given, without
    /// the BOM decoding of a physical read.
    files: BTreeMap<Vec<u8>, FileContent>,
    /// The identity a program requires of an immutable host. Only memory
    /// snapshots issue identities, so an empty one, owned by this host and
    /// consulted for nothing else, supplies it: the files never change.
    identity: MemorySnapshot,
}

impl TranspileFs {
    pub(crate) fn new(files: BTreeMap<Vec<u8>, Vec<u8>>) -> Self {
        Self {
            files: files
                .into_iter()
                .map(|(path, text)| (path, FileContent::loaded(text)))
                .collect(),
            identity: MemoryBuilder::new(b"/", true).finish(),
        }
    }

    /// The panic of an operation the pin's `transpileFS` leaves to its
    /// embedded nil file system.
    #[cold]
    #[track_caller]
    fn unsupported(operation: &str, path: &[u8]) -> ! {
        panic!(
            "unexpected {operation} for {} (transpileFS embeds no file system)",
            go_quote(path)
        )
    }
}

impl FileSystem for TranspileFs {
    // port: tsc/internal/transpile/fs.go:transpileFS.UseCaseSensitiveFileNames
    fn use_case_sensitive_file_names(&self) -> bool {
        true
    }

    fn snapshot_id(&self) -> Option<SnapshotId> {
        self.identity.snapshot_id()
    }

    // port: tsc/internal/transpile/fs.go:transpileFS.FileExists
    fn file_exists(&self, path: &[u8]) -> Result<bool, Error> {
        let ok = self.files.contains_key(path);
        assert!(ok, "unexpected file existence check for {}", go_quote(path));
        Ok(ok)
    }

    // port: tsc/internal/transpile/fs.go:transpileFS.ReadFile
    fn read_file(&self, path: &[u8]) -> Result<Option<FileContent>, Error> {
        let Some(content) = self.files.get(path) else {
            panic!("unexpected file read for {}", go_quote(path));
        };
        Ok(Some(content.clone()))
    }

    // port: tsc/internal/transpile/fs.go:transpileFS.DirectoryExists
    fn directory_exists(&self, path: &[u8]) -> Result<bool, Error> {
        panic!(
            "unexpected directory existence check for {}",
            go_quote(path)
        );
    }

    // port: tsc/internal/transpile/fs.go:transpileFS.Realpath
    fn realpath(&self, path: &[u8]) -> Result<JsString, Error> {
        panic!("unexpected realpath request for {}", go_quote(path));
    }

    fn stat(&self, path: &[u8]) -> Result<Option<FileInfo>, Error> {
        Self::unsupported("stat", path)
    }

    fn entries(&self, path: &[u8]) -> Result<Entries, Error> {
        Self::unsupported("directory listing", path)
    }

    fn write_file(&self, path: &[u8], _data: &[u8]) -> Result<(), Error> {
        Self::unsupported("file write", path)
    }

    fn append_file(&self, path: &[u8], _data: &[u8]) -> Result<(), Error> {
        Self::unsupported("file append", path)
    }

    fn remove(&self, path: &[u8]) -> Result<(), Error> {
        Self::unsupported("removal", path)
    }

    fn change_times(&self, path: &[u8], _a_time: Time, _m_time: Time) -> Result<(), Error> {
        Self::unsupported("timestamp change", path)
    }
}

#[cfg(test)]
mod tests {
    use super::TranspileFs;
    use std::collections::BTreeMap;
    use std::panic::{catch_unwind, AssertUnwindSafe};
    use tsr_vfs::FileSystem;

    fn panic_message(run: impl FnOnce()) -> String {
        let payload = catch_unwind(AssertUnwindSafe(run)).expect_err("the access panics");
        payload
            .downcast_ref::<String>()
            .cloned()
            .or_else(|| {
                payload
                    .downcast_ref::<&str>()
                    .map(|text| (*text).to_owned())
            })
            .expect("a string payload")
    }

    fn module_fs() -> TranspileFs {
        TranspileFs::new(BTreeMap::from([(b"/src/module.ts".to_vec(), Vec::new())]))
    }

    /// The pin's `TestTranspileFSRejectsDirectoryAccess`.
    #[test]
    fn transpile_fs_rejects_directory_access() {
        let fs = module_fs();
        assert_eq!(
            panic_message(|| {
                let _ = fs.directory_exists(b"/src");
            }),
            r#"unexpected directory existence check for "/src""#
        );
        assert_eq!(
            panic_message(|| {
                let _ = fs.realpath(b"/src/module.ts");
            }),
            r#"unexpected realpath request for "/src/module.ts""#
        );
    }

    /// The files are found under their exact paths only, and read as given:
    /// a byte-order mark is kept, unlike a physical read's decoding.
    #[test]
    fn transpile_fs_reads_exact_paths_as_given() {
        let fs = TranspileFs::new(BTreeMap::from([(
            b"/module.ts".to_vec(),
            b"\xef\xbb\xbfexport {};".to_vec(),
        )]));
        assert!(fs.use_case_sensitive_file_names());
        assert!(fs.snapshot_id().is_some());
        assert!(fs.file_exists(b"/module.ts").expect("exists"));
        let content = fs.read_file(b"/module.ts").expect("read").expect("present");
        assert_eq!(content.text.as_bytes(), b"\xef\xbb\xbfexport {};");
        assert_eq!(
            panic_message(|| {
                let _ = fs.read_file(b"/Module.ts");
            }),
            r#"unexpected file read for "/Module.ts""#
        );
        assert_eq!(
            panic_message(|| {
                let _ = fs.file_exists(b"module.ts");
            }),
            r#"unexpected file existence check for "module.ts""#
        );
    }

    /// The operations the pin leaves to its embedded nil file system panic.
    #[test]
    fn transpile_fs_rejects_the_operations_it_does_not_implement() {
        let fs = module_fs();
        for message in [
            panic_message(|| {
                let _ = fs.stat(b"/src/module.ts");
            }),
            panic_message(|| {
                let _ = fs.entries(b"/src");
            }),
            panic_message(|| {
                let _ = fs.write_file(b"/src/module.js", b"");
            }),
        ] {
            assert!(
                message.ends_with("(transpileFS embeds no file system)"),
                "{message}"
            );
        }
    }
}
