//! `harnessutil/recorderfs.go`: the harness's `OutputRecorderFS`, which
//! records every file the post-emit program writes. The Rust emitter hands
//! each write to `EmitOptions::write_file` instead of the file system, so the
//! recorder is that callback's target (`compile.rs`).
use std::collections::HashMap;
use std::sync::Mutex;

/// A recorded output: its real path and text.
pub type Output = (Vec<u8>, Vec<u8>);

/// Every write in written order, one entry per real path; a later write of
/// the same path replaces the text in place. The pin's write lands in the
/// file system before `Realpath`, so on a case-insensitive file system a
/// second spelling of a written file is the first one: the entries are keyed
/// by the host's path of the real name.
pub struct Recorder<'p> {
    host: &'p dyn tsr_vfs::FileSystem,
    outputs: Mutex<Recorded>,
}

/// The outputs in written order and each real path's position.
#[derive(Default)]
struct Recorded {
    outputs: Vec<Output>,
    index: HashMap<Vec<u8>, usize>,
}

impl<'p> Recorder<'p> {
    /// A recorder over the program's file system (`host`), which resolves
    /// the real names.
    // port: tsc/internal/testutil/harnessutil/recorderfs.go:NewOutputRecorderFS
    pub fn new(host: &'p dyn tsr_vfs::FileSystem) -> Self {
        Self {
            host,
            outputs: Mutex::new(Recorded::default()),
        }
    }

    // port: tsc/internal/testutil/harnessutil/recorderfs.go:OutputRecorderFS.WriteFile
    pub fn write_file(&self, name: &[u8], text: &[u8]) {
        let real = self
            .host
            .realpath(name)
            .map_or_else(|_| name.to_vec(), |path| path.as_bytes().to_vec());
        let key = tsr_tspath::to_path(&real, b"", self.host.use_case_sensitive_file_names());
        let mut guard = self
            .outputs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let recorded = &mut *guard;
        if let Some(&at) = recorded.index.get(key.as_bytes()) {
            recorded.outputs[at].1 = text.to_vec();
        } else {
            recorded
                .index
                .insert(key.as_bytes().to_vec(), recorded.outputs.len());
            recorded.outputs.push((real, text.to_vec()));
        }
    }

    /// The recorded outputs, in written order.
    // port: tsc/internal/testutil/harnessutil/recorderfs.go:OutputRecorderFS.Outputs
    pub fn outputs(self) -> Vec<Output> {
        self.outputs
            .into_inner()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .outputs
    }
}
