//! The `iovfs` adapter group: the io/fs-backed filesystem.
//!
//! Every operation in this group is a recorded gap. `crates/` has exactly two
//! `impl FileSystem` -- `MemorySnapshot` (crates/tsr_vfs/src/lib.rs:215) and
//! `BundledFs` (crates/tsr_bundled/src/lib.rs:455) -- and neither is an adapter
//! over a foreign filesystem: `MemorySnapshot` owns its own `BTreeMap` of
//! entries and `BundledFs` delegates to another `FileSystem`. Nothing in
//! `crates/` takes a host filesystem abstraction and wraps it, nothing probes
//! it for optional capabilities, and nothing hands the wrapped value back.
//! `ScopedOsFs` (crates/tsr_vfs/src/os.rs:6) reads a real directory into a
//! `MemorySnapshot` once; it does not implement `FileSystem` and it is not an
//! adapter.
//!
//! A same-named trait method is not a counterpart. `FileSystem::file_exists`
//! and `directory_exists` (lib.rs:77-82) have the derived-from-stat shape this
//! group tests, but there is no way to obtain one over an `fs.FS`-shaped
//! backing, which is what every case here constructs; the four mutations
//! (lib.rs:83-94) are provided methods that return `Error::Unsupported`, which
//! is a different contract from the pin's five distinct panics; and
//! `change_times` takes no times at all, so the argument-order case has no
//! signature to call.
//!
//! Preparation records the gap; it never emulates the adapter to make a
//! comparison run, and it never reads an expected result. Each row names the
//! pinned Go authority, the signature the port is expected to carry and the
//! file that does not have it yet.

use crate::api::Outcome;
use serde_json::Value;

/// The action vocabulary, with the keys each action must carry.
///
/// An unknown action, or an action missing a key, is a harness failure on both
/// sides rather than an observation -- the Go probe panics, this module fails.
/// A defaulted key would let the two sides agree on a row neither of them
/// executed, which is exactly the agreement these cases exist to refuse.
const ACTIONS: &[(&str, &[(&str, Kind)])] = &[
    (
        "new_fs",
        &[
            ("backing", Kind::Str),
            ("case_sensitive", Kind::Bool),
            ("files", Kind::Array),
        ],
    ),
    ("use_case_sensitive_file_names", &[]),
    ("file_exists", &[("path", Kind::Str)]),
    ("directory_exists", &[("path", Kind::Str)]),
    ("stat", &[("path", Kind::Str)]),
    ("read_file", &[("path", Kind::Str)]),
    ("entries", &[("path", Kind::Str)]),
    ("realpath", &[("path", Kind::Str)]),
    ("walk", &[("root", Kind::Str)]),
    ("write_file", &[("path", Kind::Str), ("content", Kind::Str)]),
    (
        "append_file",
        &[("path", Kind::Str), ("content", Kind::Str)],
    ),
    ("remove", &[("path", Kind::Str)]),
    (
        "chtimes",
        &[
            ("path", Kind::Str),
            ("atime", Kind::Str),
            ("mtime", Kind::Str),
        ],
    ),
    ("fsys_kind", &[]),
    ("fsys_identity", &[("path", Kind::Str)]),
    ("fsys_read", &[("path", Kind::Str)]),
    ("backing_mod_time", &[("path", Kind::Str)]),
    ("spy_log", &[]),
];

#[derive(Clone, Copy)]
enum Kind {
    Str,
    Bool,
    Array,
}

impl Kind {
    fn accepts(self, value: &Value) -> bool {
        match self {
            Kind::Str => value.is_string(),
            Kind::Bool => value.is_boolean(),
            Kind::Array => value.is_array(),
        }
    }

    fn name(self) -> &'static str {
        match self {
            Kind::Str => "a string",
            Kind::Bool => "a boolean",
            Kind::Array => "an array",
        }
    }
}

/// The pinned authority, the signature the port is expected to carry and the
/// file that does not have it yet, one row per operation id.
const MISSING: &[(&str, &str, &str, &str)] = &[
    ("tsc/internal/vfs/iovfs/iofs.go:From",
     "tsc/internal/vfs/iovfs/iofs.go:From",
     "pub fn from_io_fs(fsys: Arc<dyn IoFs>, use_case_sensitive_file_names: bool) -> Arc<dyn FsWithSys>, \
      detecting two OPTIONAL capabilities on the backing separately -- a realpath capability and a \
      writable capability -- and binding six closures: a realpath that strips a leading `/`, calls the \
      backing and puts the `/` back (or the identity when the backing has none), and write/append/mkdir/\
      remove/chtimes that strip a leading `/` and pass perm 0o666, 0o666, 0o777 (or panic with the five \
      literal texts `writeFile not supported`, `appendFile not supported`, `mkdirAll not supported`, \
      `remove not supported`, `chtimes not supported`), plus a root_for that returns the backing for `/`, \
      a sub-filesystem for any other root, None for a URL root and a panic otherwise",
     "crates/tsr_vfs/src/lib.rs (absent: the two `impl FileSystem` at lib.rs:215 and \
      crates/tsr_bundled/src/lib.rs:455 own or delegate their storage; no adapter over a foreign \
      filesystem exists, and no trait models an optional backing capability)"),
    ("tsc/internal/vfs/iovfs/iofs.go:ioFS.AppendFile",
     "tsc/internal/vfs/iovfs/iofs.go:ioFS.AppendFile, through :writeFileEnsuringDir",
     "fn append_file(&self, path: &[u8], data: &[u8]) -> Result<(), Error> on the io/fs-backed \
      adapter, creating a missing file with content == data, extending an existing one, following a \
      directory symlink, and going through the shared ensure-directory body",
     "crates/tsr_vfs/src/lib.rs:86-88 has only the trait default `Err(Error::Unsupported(\"immutable \
      filesystem append\"))`; no implementor overrides it and no adapter exists to override it on"),
    ("tsc/internal/vfs/iovfs/iofs.go:ioFS.Chtimes",
     "tsc/internal/vfs/iovfs/iofs.go:ioFS.Chtimes",
     "fn change_times(&self, path: &[u8], atime: SystemTime, mtime: SystemTime) -> Result<(), Error>, \
      asserting the path is rooted before the backing sees it and forwarding BOTH times in order",
     "crates/tsr_vfs/src/lib.rs:92-94 declares `change_times(&self, _path: &[u8])`, which takes no \
      times at all, so the signature cannot carry this operation even once an adapter exists"),
    ("tsc/internal/vfs/iovfs/iofs.go:ioFS.DirectoryExists",
     "tsc/internal/vfs/iovfs/iofs.go:ioFS.DirectoryExists, through internal.Common.DirectoryExists",
     "fn directory_exists(&self, path: &[u8]) -> Result<bool, Error> on the io/fs-backed adapter, \
      derived from stat so it inherits absent-means-false and the rooted-path assertion",
     "crates/tsr_vfs/src/lib.rs:80-82 has the derived shape as a trait default, but no io/fs-backed \
      implementor exists to call it on"),
    ("tsc/internal/vfs/iovfs/iofs.go:ioFS.FSys",
     "tsc/internal/vfs/iovfs/iofs.go:ioFS.FSys",
     "fn fsys(&self) -> Arc<dyn IoFs> returning the EXACT backing handed to the constructor -- never a \
      copy, never a sub-filesystem, never the adapter itself -- so the adapter stays an alias of a live \
      backing rather than a snapshot of it",
     "crates/tsr_vfs/src/ (absent: nothing in crates/ hands back the source a filesystem was built over; \
      `BundledFs` stores `inner: Arc<dyn FileSystem>` at crates/tsr_bundled/src/lib.rs:451 and exposes \
      no accessor for it)"),
    ("tsc/internal/vfs/iovfs/iofs.go:ioFS.FileExists",
     "tsc/internal/vfs/iovfs/iofs.go:ioFS.FileExists, through internal.Common.FileExists",
     "fn file_exists(&self, path: &[u8]) -> Result<bool, Error> on the io/fs-backed adapter, derived \
      from stat so a directory answers FALSE and a dangling symlink answers false",
     "crates/tsr_vfs/src/lib.rs:77-79 has the derived shape as a trait default, but no io/fs-backed \
      implementor exists to call it on"),
    ("tsc/internal/vfs/iovfs/iofs.go:ioFS.GetAccessibleEntries",
     "tsc/internal/vfs/iovfs/iofs.go:ioFS.GetAccessibleEntries, through internal.Common.GetAccessibleEntries",
     "fn entries(&self, path: &[u8]) -> Result<Entries, Error> on the io/fs-backed adapter, listing the \
      backing's directory in the backing's own order, dropping an entry that is neither a directory nor a \
      regular file after following it, dropping a dangling link entirely, and always answering a \
      present-but-possibly-empty symlink set",
     "crates/tsr_vfs/src/lib.rs:250-282 implements it only for `MemorySnapshot`, over that snapshot's own \
      `BTreeMap`; there is no io/fs-backed implementor"),
    ("tsc/internal/vfs/iovfs/iofs.go:ioFS.ReadFile",
     "tsc/internal/vfs/iovfs/iofs.go:ioFS.ReadFile, through internal.Common.ReadFile and :decodeBytes",
     "fn read_file(&self, path: &[u8]) -> Result<Option<FileContent>, Error> on the io/fs-backed adapter, \
      answering present-with-empty for a zero-length file before any decoding, absent for a missing path, \
      a directory or an unreadable one, and otherwise decoding a UTF-16 BOM, stripping a UTF-8 BOM and \
      passing the remaining bytes through unvalidated",
     "crates/tsr_vfs/src/lib.rs:222-230 implements it only for `MemorySnapshot`, which returns a \
      pre-decoded `FileContent` from its own map; there is no io/fs-backed implementor and no adapter-\
      level byte decoder"),
    ("tsc/internal/vfs/iovfs/iofs.go:ioFS.Realpath",
     "tsc/internal/vfs/iovfs/iofs.go:ioFS.Realpath, with the closure bound at :44-61",
     "fn realpath(&self, path: &[u8]) -> Result<JsString, Error> on the io/fs-backed adapter, splitting \
      and normalizing the path first, then either resolving it through the backing's optional realpath \
      capability or returning the normalized path unchanged, and returning the RAW argument when the \
      backing errors",
     "crates/tsr_vfs/src/lib.rs:283-288 implements it only for `MemorySnapshot`; there is no io/fs-backed \
      implementor and no notion of a backing that may or may not resolve real paths"),
    ("tsc/internal/vfs/iovfs/iofs.go:ioFS.Remove",
     "tsc/internal/vfs/iovfs/iofs.go:ioFS.Remove",
     "fn remove(&self, path: &[u8]) -> Result<(), Error> on the io/fs-backed adapter, asserting the path \
      is rooted and then forwarding the raw path to the backing",
     "crates/tsr_vfs/src/lib.rs:89-91 has only the trait default `Err(Error::Unsupported(\"immutable \
      filesystem remove\"))`; `MemoryBuilder::remove` (lib.rs:158) mutates a builder, not a filesystem, \
      and is not reachable through the trait"),
    ("tsc/internal/vfs/iovfs/iofs.go:ioFS.Stat",
     "tsc/internal/vfs/iovfs/iofs.go:ioFS.Stat, with the rooted-path assertion at :168",
     "fn stat(&self, path: &[u8]) -> Result<Option<FileInfo>, Error> on the io/fs-backed adapter, \
      carrying the entry's name, size, directory flag, mode and modification time, asserting the RAW \
      path is rooted before normalization, and answering absent rather than an error for a missing path \
      or a URL root",
     "crates/tsr_vfs/src/lib.rs:231-249 implements it only for `MemorySnapshot`, and its `FileInfo` \
      (lib.rs:64-67) carries only `directory` and `size` -- no name, no mode and no modification time, \
      so three quarters of this case's observation has no field to land in"),
    ("tsc/internal/vfs/iovfs/iofs.go:ioFS.UseCaseSensitiveFileNames",
     "tsc/internal/vfs/iovfs/iofs.go:ioFS.UseCaseSensitiveFileNames",
     "fn use_case_sensitive_file_names(&self) -> bool on the io/fs-backed adapter, returning the stored \
      constructor argument verbatim and NEVER governing lookup inside the adapter: the backing decides \
      whether names fold",
     "crates/tsr_vfs/src/lib.rs:71 declares the method and lib.rs:216-218 returns `MemorySnapshot`'s \
      stored flag, but there the same flag also drives the snapshot's own canonicalisation, so no type \
      in crates/ separates the reported flag from the folding the way this adapter does"),
    ("tsc/internal/vfs/iovfs/iofs.go:ioFS.WalkDir",
     "tsc/internal/vfs/iovfs/iofs.go:ioFS.WalkDir, through internal.Common.WalkDir",
     "fn walk_dir(&self, root: &[u8], f: &mut dyn FnMut(&[u8], Option<&DirEntry>, Option<Error>) -> \
      WalkDecision) -> Result<(), Error>, visiting in the backing's own order, rewriting the root row's \
      path from `.` to the root name, passing an absent entry together with an error, and honouring \
      skip-directory, skip-all and a callback error",
     "crates/tsr_vfs/src/lib.rs:70-95 lists eleven trait methods and none of them walks; no crate in \
      crates/ carries a directory walk over a `FileSystem`"),
    ("tsc/internal/vfs/iovfs/iofs.go:ioFS.WriteFile",
     "tsc/internal/vfs/iovfs/iofs.go:ioFS.WriteFile, through :writeFileEnsuringDir",
     "fn write_file(&self, path: &[u8], data: &[u8]) -> Result<(), Error> on the io/fs-backed adapter, \
      replacing rather than extending, keeping a file that is written empty, following a directory \
      symlink, and going through the shared ensure-directory body",
     "crates/tsr_vfs/src/lib.rs:83-85 has only the trait default `Err(Error::Unsupported(\"immutable \
      filesystem write\"))`; no implementor overrides it and no adapter exists to override it on"),
    ("tsc/internal/vfs/iovfs/iofs.go:ioFS.writeFileEnsuringDir",
     "tsc/internal/vfs/iovfs/iofs.go:ioFS.writeFileEnsuringDir",
     "a private shared body behind write_file and append_file: assert the RAW path is rooted, call the \
      backing once with the RAW path, and only if that fails create the directory of the NORMALIZED path \
      and call the backing a second time with the RAW path again -- two different spellings of the same \
      path in one function, and exactly one retry",
     "crates/tsr_vfs/src/lib.rs (absent, and the behaviour has no analogue either: write_file and \
      append_file at lib.rs:83-88 are trait defaults that return an error without touching any storage, \
      so there is no directory-creating retry anywhere in crates/)"),
];

fn problem(request: &Value) -> Option<String> {
    for (index, action) in crate::api::actions(request).iter().enumerate() {
        let op = crate::api::action_op(action);
        let Some((_, keys)) = ACTIONS.iter().find(|(name, _)| *name == op) else {
            return Some(format!("action {index} is an unsupported action: {op:?}"));
        };
        for (key, kind) in *keys {
            match action.get(*key) {
                None => {
                    return Some(format!(
                        "action {index} ({op}) requires {key}; an absent key must fail rather than default"
                    ))
                }
                Some(value) if !kind.accepts(value) => {
                    return Some(format!(
                        "action {index} ({op}) needs {key} to be {}",
                        kind.name()
                    ))
                }
                Some(_) => {}
            }
        }
    }
    None
}

pub fn observe(request: &Value) -> Option<Outcome> {
    if !crate::api::subject(request).starts_with("iovfs.") {
        return None;
    }
    if let Some(reason) = problem(request) {
        return Some(Outcome::Failed(reason));
    }
    let operation = request
        .get("operation")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let Some((_, authority, signature, home)) =
        MISSING.iter().find(|(id, _, _, _)| *id == operation)
    else {
        return Some(Outcome::Failed(format!(
            "no recorded gap for operation {operation:?}; the iovfs group answers only the \
             fifteen operations of tsc/internal/vfs/iovfs/iofs.go"
        )));
    };
    Some(Outcome::missing(authority, signature, home))
}
