//! Typed per-operation call recording. Unwired methods fail before recording;
//! configured delegates are recorded before invocation, including on unwind.
use crate::iofs::Time;
use crate::wrapped::Replacements;
use crate::{
    Entries, Error, FileContent, FileInfo, FileSystem, ReadResult, SnapshotId, WalkCallback,
};
use std::sync::Arc;
use std::sync::Mutex;
use tsr_jsstring::JsString;
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Call {
    pub path: Option<JsString>,
    pub data: Option<JsString>,
    pub times: Option<(Time, Time)>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Operation {
    UseCaseSensitiveFileNames,
    FileExists,
    ReadFile,
    WriteFile,
    AppendFile,
    Remove,
    Chtimes,
    DirectoryExists,
    GetAccessibleEntries,
    Stat,
    WalkDir,
    Realpath,
}
impl Operation {
    pub const ALL: &[Self] = &[
        Self::UseCaseSensitiveFileNames,
        Self::FileExists,
        Self::ReadFile,
        Self::WriteFile,
        Self::AppendFile,
        Self::Remove,
        Self::Chtimes,
        Self::DirectoryExists,
        Self::GetAccessibleEntries,
        Self::Stat,
        Self::WalkDir,
        Self::Realpath,
    ];
}
#[derive(Default)]
pub struct RecordingFs {
    pub delegates: Replacements,
    calls: Mutex<Vec<(Operation, Call)>>,
    retained_walks: Mutex<Vec<(JsString, crate::OwnedWalkCallback)>>,
}
impl RecordingFs {
    /// Mirrors the pinned Go test harness: tsc/internal/vfs/vfsmock/wrapper.go:Wrap
    pub fn new(inner: Arc<dyn FileSystem>) -> Self {
        Self {
            delegates: Replacements::forwarding(inner),
            calls: Mutex::default(),
            retained_walks: Mutex::default(),
        }
    }
    /// Returns owned callbacks; invoking them never holds the record lock.
    pub fn retained_walks(&self) -> Vec<(JsString, crate::OwnedWalkCallback)> {
        self.retained_walks
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
    pub fn calls(&self, operation: Operation) -> Vec<Call> {
        self.calls
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .filter(|(op, _)| *op == operation)
            .map(|(_, call)| call.clone())
            .collect()
    }
    pub fn all_calls(&self) -> Vec<(Operation, Call)> {
        self.calls
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
    fn record(
        &self,
        operation: Operation,
        path: Option<&[u8]>,
        data: Option<&[u8]>,
        times: Option<(Time, Time)>,
    ) {
        let call = Call {
            path: path.map(JsString::from_bytes),
            data: data.map(JsString::from_bytes),
            times,
        };
        self.calls
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push((operation, call));
    }
}
impl FileSystem for RecordingFs {
    fn walk_dir_owned(&self, path: &[u8], visit: crate::OwnedWalkCallback) -> Result<(), Error> {
        let borrowed = self.delegates.walk_dir.as_ref();
        let owned = self.delegates.walk_dir_owned.as_ref();
        assert!(
            borrowed.is_some() || owned.is_some(),
            "FSMock.WalkDirFunc: method is nil but FS.WalkDir was just called"
        );
        self.record(Operation::WalkDir, Some(path), None, None);
        self.retained_walks
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push((JsString::from_bytes(path), visit.clone()));
        if let Some(delegate) = owned {
            delegate(path, visit)
        } else {
            borrowed.expect("checked delegate")(path, &mut |path, entry, error| {
                visit(path, entry, error)
            })
        }
    }
    // Public delegates can be replaced or backed by mutable state.
    fn snapshot_id(&self) -> Option<SnapshotId> {
        None
    }
    fn read_file(&self, path: &[u8]) -> Result<Option<FileContent>, Error> {
        self.read_file_result(path).map(ReadResult::into_file)
    }
    /// Mirrors the pinned Go test harness: tsc/internal/vfs/vfsmock/mock_generated.go:FSMock.UseCaseSensitiveFileNames
    fn use_case_sensitive_file_names(&self) -> bool {
        let delegate = self.delegates.use_case_sensitive_file_names.as_ref().expect("FSMock.UseCaseSensitiveFileNamesFunc: method is nil but FS.UseCaseSensitiveFileNames was just called");
        self.record(Operation::UseCaseSensitiveFileNames, None, None, None);
        delegate()
    }
    /// Mirrors the pinned Go test harness: tsc/internal/vfs/vfsmock/mock_generated.go:FSMock.FileExists
    fn file_exists(&self, path: &[u8]) -> Result<bool, Error> {
        let delegate = self
            .delegates
            .file_exists
            .as_ref()
            .expect("FSMock.FileExistsFunc: method is nil but FS.FileExists was just called");
        self.record(Operation::FileExists, Some(path), None, None);
        delegate(path)
    }
    /// Mirrors the pinned Go test harness: tsc/internal/vfs/vfsmock/mock_generated.go:FSMock.ReadFile
    fn read_file_result(&self, path: &[u8]) -> Result<ReadResult, Error> {
        let delegate = self
            .delegates
            .read_file_result
            .as_ref()
            .expect("FSMock.ReadFileFunc: method is nil but FS.ReadFile was just called");
        self.record(Operation::ReadFile, Some(path), None, None);
        delegate(path)
    }
    /// Mirrors the pinned Go test harness: tsc/internal/vfs/vfsmock/mock_generated.go:FSMock.WriteFile
    fn write_file(&self, path: &[u8], data: &[u8]) -> Result<(), Error> {
        let delegate = self
            .delegates
            .write_file
            .as_ref()
            .expect("FSMock.WriteFileFunc: method is nil but FS.WriteFile was just called");
        self.record(Operation::WriteFile, Some(path), Some(data), None);
        delegate(path, data)
    }
    /// Mirrors the pinned Go test harness: tsc/internal/vfs/vfsmock/mock_generated.go:FSMock.AppendFile
    fn append_file(&self, path: &[u8], data: &[u8]) -> Result<(), Error> {
        let delegate = self
            .delegates
            .append_file
            .as_ref()
            .expect("FSMock.AppendFileFunc: method is nil but FS.AppendFile was just called");
        self.record(Operation::AppendFile, Some(path), Some(data), None);
        delegate(path, data)
    }
    /// Mirrors the pinned Go test harness: tsc/internal/vfs/vfsmock/mock_generated.go:FSMock.Remove
    fn remove(&self, path: &[u8]) -> Result<(), Error> {
        let delegate = self
            .delegates
            .remove
            .as_ref()
            .expect("FSMock.RemoveFunc: method is nil but FS.Remove was just called");
        self.record(Operation::Remove, Some(path), None, None);
        delegate(path)
    }
    /// Mirrors the pinned Go test harness: tsc/internal/vfs/vfsmock/mock_generated.go:FSMock.Chtimes
    fn change_times(&self, path: &[u8], a_time: Time, m_time: Time) -> Result<(), Error> {
        let delegate = self
            .delegates
            .change_times
            .as_ref()
            .expect("FSMock.ChtimesFunc: method is nil but FS.Chtimes was just called");
        self.record(Operation::Chtimes, Some(path), None, Some((a_time, m_time)));
        delegate(path, a_time, m_time)
    }
    /// Mirrors the pinned Go test harness: tsc/internal/vfs/vfsmock/mock_generated.go:FSMock.DirectoryExists
    fn directory_exists(&self, path: &[u8]) -> Result<bool, Error> {
        let delegate = self.delegates.directory_exists.as_ref().expect(
            "FSMock.DirectoryExistsFunc: method is nil but FS.DirectoryExists was just called",
        );
        self.record(Operation::DirectoryExists, Some(path), None, None);
        delegate(path)
    }
    /// Mirrors the pinned Go test harness: tsc/internal/vfs/vfsmock/mock_generated.go:FSMock.GetAccessibleEntries
    fn entries(&self, path: &[u8]) -> Result<Entries, Error> {
        let delegate = self.delegates.entries.as_ref().expect("FSMock.GetAccessibleEntriesFunc: method is nil but FS.GetAccessibleEntries was just called");
        self.record(Operation::GetAccessibleEntries, Some(path), None, None);
        delegate(path)
    }
    /// Mirrors the pinned Go test harness: tsc/internal/vfs/vfsmock/mock_generated.go:FSMock.Stat
    fn stat(&self, path: &[u8]) -> Result<Option<FileInfo>, Error> {
        let delegate = self
            .delegates
            .stat
            .as_ref()
            .expect("FSMock.StatFunc: method is nil but FS.Stat was just called");
        self.record(Operation::Stat, Some(path), None, None);
        delegate(path)
    }
    /// Mirrors the pinned Go test harness: tsc/internal/vfs/vfsmock/mock_generated.go:FSMock.WalkDir
    fn walk_dir(&self, path: &[u8], visit: &mut WalkCallback<'_>) -> Result<(), Error> {
        let delegate = self
            .delegates
            .walk_dir
            .as_ref()
            .expect("FSMock.WalkDirFunc: method is nil but FS.WalkDir was just called");
        self.record(Operation::WalkDir, Some(path), None, None);
        delegate(path, visit)
    }
    /// Mirrors the pinned Go test harness: tsc/internal/vfs/vfsmock/mock_generated.go:FSMock.Realpath
    fn realpath(&self, path: &[u8]) -> Result<JsString, Error> {
        let delegate = self
            .delegates
            .realpath
            .as_ref()
            .expect("FSMock.RealpathFunc: method is nil but FS.Realpath was just called");
        self.record(Operation::Realpath, Some(path), None, None);
        delegate(path)
    }
}
