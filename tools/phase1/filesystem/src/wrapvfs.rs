//! The wrapping and tracking adapter group: `vfs/wrapvfs` and `vfs/trackingvfs`.
//!
//! Both subjects are recorded gaps, and the gap is structural rather than a
//! missing function. `crates/tsr_vfs/src/lib.rs:70-95` declares the whole
//! `FileSystem` trait, and a repository-wide search finds exactly two
//! implementors -- `MemorySnapshot` (crates/tsr_vfs/src/lib.rs:215) and
//! `BundledFs` (crates/tsr_bundled/src/lib.rs:455). Neither records accesses
//! and neither takes a replacement table, so there is no Rust counterpart to
//! either adapter: no `Replacements`, no `SeenFiles`, and no `walk_dir` method
//! on the trait at all.
//!
//! Preparation records that. It never emulates a wrapper here to make a
//! comparison run, and it never reads an expected result: a hand-written
//! forwarding shim in this file would answer every forwarding case correctly
//! and would prove nothing about the port.
//!
//! Three details of the trait are named in the records below because they are
//! what a port has to decide, not incidental:
//!
//! * `file_exists` and `directory_exists` are *default* methods over `stat`
//!   (:77-82). A wrapper written by overriding `stat` alone silently inherits
//!   both defaults, so a replacement table keyed on either predicate would
//!   never be consulted. `BundledFs` overrides `directory_exists` and leaves
//!   `file_exists` defaulted, which is exactly the asymmetry at issue.
//! * `read_file` returns `Result<Option<FileContent>, Error>` (:73), which has
//!   no room for Go's independent `(contents string, ok bool)` pair: the pinned
//!   `("X", false)` has no Rust spelling.
//! * `change_times` takes only a path (:92-94). The two instants Go's
//!   `Chtimes(path, aTime, mTime)` carries cannot be passed at all, so the
//!   lost-timestamp-arguments control has no Rust expression until the
//!   signature grows.

use crate::api::Outcome;
use serde_json::Value;

/// The two subjects, each with the Go authority, the signature the port is
/// expected to carry and the Rust file that does not have it yet.
const MISSING: &[(&str, &str, &str, &str)] = &[
    ("trackingvfs.FS",
     "tsc/internal/vfs/trackingvfs/trackingvfs.go:FS.ReadFile, FS.FileExists, \
      FS.UseCaseSensitiveFileNames, FS.WriteFile, FS.AppendFile, FS.Remove, FS.Chtimes, \
      FS.DirectoryExists, FS.GetAccessibleEntries, FS.Stat, FS.WalkDir and FS.Realpath \
      (trackingvfs.go:24-76)",
     "a pub struct TrackingFs { inner: Arc<dyn FileSystem>, seen: <a concurrent set of JsString> } \
      implementing tsr_vfs::FileSystem by forwarding every method to `inner` and recording the \
      path argument -- verbatim, before delegating, and whatever the answer -- for exactly the \
      seven read-like methods read_file, file_exists, directory_exists, entries, stat, realpath \
      and walk_dir, while use_case_sensitive_file_names and the four write-class methods \
      (write_file, append_file, remove, change_times) forward untracked because they are outputs \
      rather than dependencies. Recording must survive a miss, so a failed resolution's \
      non-existent path is in the set. Two pieces have to exist first: a walk_dir(&self, root: \
      &[u8], walk_fn: &mut dyn FnMut(&[u8], &DirEntry, Option<Error>) -> WalkControl) -> \
      Result<(), Error> on the trait, with SkipDir/SkipAll/stop-with-error sentinels, since the \
      tracker must substitute its own recording closure around the caller's (trackingvfs.go:65-71) \
      and cannot do so through a method that does not exist; and file_exists/directory_exists \
      overrides on the wrapper, because inheriting the trait's stat-derived defaults \
      (crates/tsr_vfs/src/lib.rs:77-82) would record the stat path instead of the requested one \
      -- or nothing at all. Neither read_file nor stat can carry the pinned observation as it \
      stands: read_file returns Result<Option<FileContent>, Error> (:73) rather than an \
      independent (contents, ok) pair, and FileInfo is {directory, size} (:62-66) with no Name, \
      Mode, ModTime or Sys",
     "crates/tsr_vfs/src/ (absent). The trait has no walk_dir and the workspace has exactly two \
      FileSystem implementors, MemorySnapshot at crates/tsr_vfs/src/lib.rs:215 and BundledFs at \
      crates/tsr_bundled/src/lib.rs:455; neither records accesses, and a repository-wide search \
      across crates/ for trackingvfs, SeenFiles and seen_files finds nothing"),
    ("wrapvfs.Wrap",
     "tsc/internal/vfs/wrapvfs/wrapvfs.go:Wrap (:24-29), the Replacements table (:9-22) and the \
      twelve wrappedFS methods (:37-130)",
     "a pub struct Replacements holding one optional callback per vfs.FS method -- including the \
      zero-argument use_case_sensitive_file_names, which is the only entry a path-keyed dispatch \
      table would drop -- and a pub fn wrap(inner: Arc<dyn FileSystem>, replacements: \
      Replacements) -> Arc<dyn FileSystem> whose every method consults its own replacement when one \
      is installed and forwards to `inner` otherwise. The rule is per method and unconditional: \
      an installed replacement answers, the inner is not consulted at all, and a false, an empty \
      string, a nil FileInfo or an error from the replacement is the answer rather than a reason \
      to fall back. Wrap itself neither validates nor fails, and it takes the table by value, so \
      later edits to the caller's own table do not reach the wrapper. Three entries have no Rust \
      spelling yet: read_file cannot express the independent (contents, ok) pair \
      (crates/tsr_vfs/src/lib.rs:73), change_times carries no instants (:92-94), and walk_dir \
      does not exist, so there is no callback to pass through unchanged. file_exists and \
      directory_exists must be overridden rather than left to their stat-derived defaults \
      (:77-82), or their replacements can never fire",
     "crates/tsr_vfs/src/ (absent). The only wrapping FileSystem implementor in the workspace is \
      BundledFs (crates/tsr_bundled/src/lib.rs:447-519), which intercepts a fixed bundled:/// \
      prefix rather than consulting a caller-supplied table, and a repository-wide search across \
      crates/ for wrapvfs and Replacements finds nothing"),
];

pub fn observe(request: &Value) -> Option<Outcome> {
    let subject = crate::api::subject(request);
    MISSING
        .iter()
        .find(|(name, ..)| *name == subject)
        .map(|(_, authority, signature, home)| Outcome::missing(authority, signature, home))
}
