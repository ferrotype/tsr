//! The `vfs/vfsmock` recording-wrapper group.
//!
//! The pinned subject is `FSMock`, the moq-generated recorder that `Wrap`
//! fills from a real `vfs.FS` (upstream/tsc/internal/vfs/vfsmock/wrapper.go).
//! What it contributes is a call log: each method appends the caller's
//! arguments to a private slice and then forwards, and each `XxxCalls` reader
//! hands that slice back. Every case in this group is about that log --
//! which calls reached the wrapper, in what order, with which arguments, and
//! whether an entry survives a call that failed.
//!
//! The port has no counterpart, and the gap is wider than a missing type.
//! `crates/tsr_vfs/src/lib.rs:70-95` declares `FileSystem` with twelve
//! members, and two types implement it: `MemorySnapshot` (:215), which wraps
//! nothing, and `tsr_bundled::BundledFs` (crates/tsr_bundled/src/lib.rs:447),
//! which *is* a delegating decorator -- it holds an `Arc<dyn FileSystem>` and
//! forwards every member it does not intercept (:457, :460, :468, :487, :493,
//! :515, :519). So the gap is not "no wrapper"; it is no *recording* wrapper.
//! Nothing keeps a call log, and the decorator that does exist shows two of
//! the three structural differences that would survive even once one did:
//!
//! * `file_exists` and `directory_exists` are trait *defaults* built on
//!   `stat` (:77-82), where the pin dispatches each of the three
//!   independently. A recorder over today's trait would log stat calls where
//!   the pin logs predicate calls. `BundledFs` is the live illustration: it
//!   overrides `directory_exists` (crates/tsr_bundled/src/lib.rs:489) but not
//!   `file_exists`, so a call to the latter arrives at the decorator's `stat`
//!   and the predicate itself is never a dispatch site at all.
//! * `change_times` takes no timestamps at all (:92) -- `fn change_times(&self,
//!   _path: &[u8])`. The pin's log keeps both the access and the modification
//!   time, including for a call that failed, so a port has nowhere to put the
//!   arguments the log is supposed to carry.
//! * there is no walk member of any kind, so `WalkDir` and the callback its
//!   log records have no Rust surface to be recorded on -- `BundledFs`, the
//!   one decorator in the tree, has no walk member to forward either.
//!
//! Preparation records the gap; it never emulates the recorder to make a
//! comparison run, and it never reads an expected result.

use crate::api::Outcome;
use serde_json::Value;

/// One record per case, because each case reaches a different part of the
/// missing surface and the gap it needs named is different. Each row names the
/// pinned Go authority, the signature the port is expected to carry and the
/// file that does not have it yet.
const MISSING: &[(&str, &str, &str, &str)] = &[
    ("filesystem/vfsmock/wrap-wires-every-interface-method",
     "tsc/internal/vfs/vfsmock/wrapper.go:Wrap, whose completeness the pinned \
      vfsmock/wrapper_test.go:TestWrap asserts field by field",
     "pub fn wrap(inner: Arc<dyn FileSystem>) -> RecordingFs, wiring one recorded delegate per \
      member of tsr_vfs::FileSystem and holding the delegate live (a later change to the wrapped \
      filesystem is visible through the wrapper) while the wrapper's own binding is fixed",
     "crates/tsr_vfs/src/recording.rs (absent). MemorySnapshot at crates/tsr_vfs/src/lib.rs:215 \
      and tsr_bundled::BundledFs at crates/tsr_bundled/src/lib.rs:455 are the only implementors \
      of tsr_vfs::FileSystem. BundledFs is already a delegating decorator -- it holds an \
      Arc<dyn FileSystem> (crates/tsr_bundled/src/lib.rs:447-453) and forwards every member it \
      does not intercept -- so what is missing is not a wrapper but a recording one: nothing in \
      the tree keeps a call log"),
    ("filesystem/vfsmock/append-file-records-before-delegating",
     "tsc/internal/vfs/vfsmock/mock_generated.go:FSMock.AppendFile and :FSMock.AppendFileCalls",
     "RecordingFs::append_file(&self, path, data) -> Result<(), Error> appending (path, data) to \
      the log before it delegates, so a delegate that fails or panics still leaves its entry, plus \
      append_file_calls(&self) -> Vec<(JsString, Vec<u8>)> returning the ordered log",
     "crates/tsr_vfs/src/recording.rs (absent; the trait's append_file at \
      crates/tsr_vfs/src/lib.rs:86 is the default Err(Error::Unsupported(\"immutable filesystem \
      append\")) and nothing overrides it)"),
    ("filesystem/vfsmock/chtimes-keeps-the-access-time-the-write-discards",
     "tsc/internal/vfs/vfsmock/mock_generated.go:FSMock.Chtimes and :FSMock.ChtimesCalls",
     "RecordingFs::change_times(&self, path, a_time, m_time) -> Result<(), Error> recording both \
      instants, plus change_times_calls(&self) -> Vec<(JsString, Time, Time)>. This first needs \
      the two arguments to exist: tsr_vfs::FileSystem::change_times is \
      fn change_times(&self, _path: &[u8]) (crates/tsr_vfs/src/lib.rs:92) and carries no \
      timestamps at all, so the log has nothing to record",
     "crates/tsr_vfs/src/recording.rs (absent) over a change_times that takes the two instants \
      (crates/tsr_vfs/src/lib.rs:92 takes neither)"),
    ("filesystem/vfsmock/predicate-dispatch-is-independent-of-stat",
     "tsc/internal/vfs/vfsmock/mock_generated.go:FSMock.FileExists, :FSMock.FileExistsCalls, \
      :FSMock.DirectoryExists, :FSMock.DirectoryExistsCalls, :FSMock.Stat and :FSMock.StatCalls",
     "RecordingFs::file_exists / directory_exists / stat, each an independent recorded dispatch \
      site with its own log, so three predicate logs read together give three independent counts. \
      tsr_vfs::FileSystem makes the first two defaults over stat \
      (crates/tsr_vfs/src/lib.rs:77-82), so a recorder over today's trait logs a stat where the \
      pin logs a predicate. Stat also needs a FileInfo carrying the members the pin's log exposes: \
      tsr_vfs::FileInfo is { directory, size } (crates/tsr_vfs/src/lib.rs:65-68) with no name, \
      mode or modification time",
     "crates/tsr_vfs/src/recording.rs (absent) over overridable file_exists/directory_exists and \
      a widened FileInfo (crates/tsr_vfs/src/lib.rs)"),
    ("filesystem/vfsmock/read-and-realpath-log-every-request",
     "tsc/internal/vfs/vfsmock/mock_generated.go:FSMock.ReadFile, :FSMock.ReadFileCalls, \
      :FSMock.Realpath and :FSMock.RealpathCalls",
     "RecordingFs::read_file and ::realpath logging every request verbatim -- including a repeat \
      of a path already asked for, and including the caller's unnormalised argument, since the \
      pin records what the caller passed and normalises inside the delegate",
     "crates/tsr_vfs/src/recording.rs (absent). The delegate also differs: \
      MemorySnapshot::realpath (crates/tsr_vfs/src/lib.rs:283-290) resolves and normalises, where \
      the pinned iovfs.Realpath returns the caller's own path unchanged when resolution fails \
      (upstream/tsc/internal/vfs/iovfs/iofs.go:188-196)"),
    ("filesystem/vfsmock/entries-listing-is-not-memoised",
     "tsc/internal/vfs/vfsmock/mock_generated.go:FSMock.GetAccessibleEntries and \
      :FSMock.GetAccessibleEntriesCalls",
     "RecordingFs::entries(&self, path) -> Result<Entries, Error> forwarding once per request with \
      no memoisation, plus entries_calls(&self) -> Vec<JsString>. The Entries the log exposes also \
      needs the pin's absent-member distinctions: the pinned Entries has nil-able Files, \
      Directories and Symlinks (upstream/tsc/internal/vfs/vfs.go:51-61) and tsr_vfs::Entries \
      (crates/tsr_vfs/src/lib.rs:59-64) can express absence on symlinks only",
     "crates/tsr_vfs/src/recording.rs (absent) over an Entries that can distinguish an absent \
      member from an empty one on all three fields (crates/tsr_vfs/src/lib.rs:59-64)"),
    ("filesystem/vfsmock/remove-is-one-call-per-request",
     "tsc/internal/vfs/vfsmock/mock_generated.go:FSMock.Remove and :FSMock.RemoveCalls",
     "RecordingFs::remove(&self, path) -> Result<(), Error> logging one entry per caller-level \
      request however many entries the delegate removes underneath, plus \
      remove_calls(&self) -> Vec<JsString>",
     "crates/tsr_vfs/src/recording.rs (absent; the trait's remove at \
      crates/tsr_vfs/src/lib.rs:89 is the default Err(Error::Unsupported(\"immutable filesystem \
      remove\")) and nothing overrides it)"),
    ("filesystem/vfsmock/write-file-logs-once-through-the-mkdirall-retry",
     "tsc/internal/vfs/vfsmock/mock_generated.go:FSMock.WriteFile and :FSMock.WriteFileCalls",
     "RecordingFs::write_file(&self, path, data) -> Result<(), Error> logging one entry per \
      caller-level write even when the delegate performs several inner writes -- the pinned \
      ioFS.writeFileEnsuringDir writes, and on failure calls mkdirAll and writes again \
      (upstream/tsc/internal/vfs/iovfs/iofs.go:201-213) -- plus \
      write_file_calls(&self) -> Vec<(JsString, Vec<u8>)>",
     "crates/tsr_vfs/src/recording.rs (absent; the trait's write_file at \
      crates/tsr_vfs/src/lib.rs:83 is the default Err(Error::Unsupported(\"immutable filesystem \
      write\")) and nothing overrides it)"),
    ("filesystem/vfsmock/walk-dir-records-the-supplied-callback",
     "tsc/internal/vfs/vfsmock/mock_generated.go:FSMock.WalkDir and :FSMock.WalkDirCalls",
     "RecordingFs::walk_dir(&self, root, &mut dyn FnMut(&[u8], Option<&DirEntry>, \
      Option<Error>) -> WalkAction) -> Result<(), Error>, logging the root together with a handle \
      to the callback the caller supplied -- one the reader can still invoke, so a recorded \
      callback that never reaches the caller's, that is handed rewritten arguments, or whose \
      answer is swallowed is detectable (a callback forwarded unchanged is not, and is not meant \
      to be) -- plus walk_dir_calls(&self). This first needs a walk member at all: \
      tsr_vfs::FileSystem (crates/tsr_vfs/src/lib.rs:70-95) declares none, and neither does any \
      implementor",
     "crates/tsr_vfs/src/recording.rs (absent) over a walk member of tsr_vfs::FileSystem \
      (crates/tsr_vfs/src/lib.rs:70-95, absent), with SkipDir and SkipAll as callback results"),
    ("filesystem/vfsmock/case-sensitivity-query-is-counted-not-cached",
     "tsc/internal/vfs/vfsmock/mock_generated.go:FSMock.UseCaseSensitiveFileNames and \
      :FSMock.UseCaseSensitiveFileNamesCalls",
     "RecordingFs::use_case_sensitive_file_names(&self) -> bool recording an entry per call even \
      though the call has no arguments, plus use_case_sensitive_file_names_calls(&self) -> usize: \
      for this member the count is the entire payload, so a wrapper that answered from a cached \
      value is visible only as a missing entry",
     "crates/tsr_vfs/src/recording.rs (absent; tsr_vfs::FileSystem declares \
      use_case_sensitive_file_names at crates/tsr_vfs/src/lib.rs:71 and MemorySnapshot answers it \
      from a field at :216-218, with no wrapper to count the asking)"),
];

pub fn observe(request: &Value) -> Option<Outcome> {
    if crate::api::subject(request) != "vfsmock.FSMock" {
        return None;
    }
    let case = request.get("case").and_then(Value::as_str).unwrap_or("");
    let Some((_, authority, signature, home)) = MISSING.iter().find(|(id, ..)| *id == case) else {
        // The request schedule and this module disagree. That is a harness
        // failure, not an observation: a generic gap record here would let a
        // case nobody described still report a tidy result.
        return Some(Outcome::Failed(format!(
            "case {case:?} declares subject vfsmock.FSMock but the vfsmock group has no record of \
             the gap it needs named"
        )));
    };
    Some(Outcome::missing(authority, signature, home))
}
