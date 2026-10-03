use std::sync::Arc;
use std::time::Duration;
use tsr_contentmapper::{SpawnError, Spawner};
use tsr_ipc::{Context, Stream};
use tsr_jsstring::JsString;
use tsr_tsc::{SharedWriter, System, Writer};
use tsr_vfs::{iofs::Time, Entries, FileContent, FileInfo, FileSystem, SnapshotId};

struct RefusingFs;

impl FileSystem for RefusingFs {
    fn use_case_sensitive_file_names(&self) -> bool {
        true
    }
    fn snapshot_id(&self) -> Option<SnapshotId> {
        None
    }
    fn read_file(&self, _: &[u8]) -> Result<Option<FileContent>, tsr_vfs::Error> {
        panic!("a refused project must not be read")
    }
    fn stat(&self, path: &[u8]) -> Result<Option<FileInfo>, tsr_vfs::Error> {
        assert_eq!(path, b"/work/unsupported");
        Err(tsr_vfs::Error::Unsupported("project host stat"))
    }
    fn entries(&self, _: &[u8]) -> Result<Entries, tsr_vfs::Error> {
        panic!("a refused project must not be enumerated")
    }
    fn realpath(&self, _: &[u8]) -> Result<JsString, tsr_vfs::Error> {
        panic!("a refused project must not be resolved")
    }
}

struct RefusingSystem;

impl Writer for RefusingSystem {
    fn write(&self, _: &[u8]) -> std::io::Result<usize> {
        panic!("the command caller must report its typed refusal")
    }
}

impl Spawner for RefusingSystem {
    fn spawn(
        &self,
        _: &[JsString],
        _: &[u8],
        _: Box<dyn std::io::Write + Send>,
    ) -> Result<Stream, SpawnError> {
        panic!("a refused project must not spawn")
    }
}

impl System for RefusingSystem {
    fn writer(&self) -> SharedWriter {
        Arc::new(Self)
    }
    fn error_writer(&self) -> SharedWriter {
        Arc::new(Self)
    }
    fn fs(&self) -> Arc<dyn FileSystem> {
        Arc::new(RefusingFs)
    }
    fn default_library_path(&self) -> &[u8] {
        b"/lib"
    }
    fn get_current_directory(&self) -> &[u8] {
        b"/work"
    }
    fn write_output_is_tty(&self) -> bool {
        false
    }
    fn get_width_of_terminal(&self) -> i64 {
        80
    }
    fn get_environment_variable(&self, _: &str) -> Option<JsString> {
        None
    }
    fn now(&self) -> Time {
        Time::ZERO
    }
    fn since_start(&self) -> Duration {
        Duration::ZERO
    }
}

#[test]
fn command_line_returns_the_host_refusal_without_unwinding() {
    let args = [b"--project".as_slice(), b"/work/unsupported"].map(JsString::from_bytes);
    let result = crate::command_line(
        &Context::background(),
        Arc::new(RefusingSystem),
        &args,
        None,
    );
    assert!(matches!(
        result,
        Err(tsr_compiler::Error::Host(tsr_vfs::Error::Unsupported(
            "project host stat"
        )))
    ));
}
