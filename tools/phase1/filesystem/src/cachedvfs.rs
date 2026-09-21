//! The `vfs/cachedvfs` group: the caching wrapper and the shared VFS
//! internals it sits on.
//!
//! Almost all of it is a recorded gap. There is no wrapper, cache or root
//! dispatcher anywhere in `crates/`: `crates/tsr_vfs/src/lib.rs:68-95` is the
//! whole `FileSystem` trait and it has no enable flag, no clear, no walk and
//! no per-root factory, while `MemorySnapshot` (lib.rs:215) and
//! `crates/tsr_bundled/src/lib.rs:455` are the only two implementations.
//! Preparation records that; it never emulates a cache here to make a
//! comparison run, and it never reads an expected result.
//!
//! One subject is answered rather than recorded, and deliberately. The
//! repository's own disposition for `vfs.RootLength` is `implemented_untested`
//! on the basis that `crates/tsr_core/src/path.rs:73-80` defines `root_length`
//! (re-exported through `crates/tsr_tspath/src/lib.rs:10`). That claim is
//! exactly what this step exists to test, so the driver calls the claimed home
//! and lets the comparison say whether it holds. It does not hold everywhere:
//! the Rust function is a port of `tspath.GetRootLength`, and the pinned
//! `vfs.RootLength` is that function plus a precondition that panics.

use serde_json::{json, Value};

use crate::api::{action_op, actions, ordered, subject, Outcome};

/// Cases this module owns. The schedule is shared with every other filesystem
/// group and a subject string could collide with a neighbour's, so ownership
/// is keyed on the assigned case id, exactly as the Go probe keys it.
const CASE_PREFIX: &str = "filesystem/cachedvfs/";

const CACHED_HOME: &str = "crates/tsr_vfs/src/lib.rs (no caching wrapper exists: a repo-wide \
    search across crates/ for cachedvfs, cached_fs, clear_cache, disable_and_clear and an \
    enabled flag finds nothing, and the FileSystem trait at lib.rs:68-95 declares no wrapper \
    form of any of these methods)";

const COMMON_HOME: &str = "crates/tsr_vfs/src/lib.rs (no vfs::Common equivalent exists: every \
    FileSystem method takes a whole path and resolves it itself, so there is no RootFor-style \
    per-root factory, no root/remainder split and no place for the empty-remainder rewrite)";

/// The wrapper's sixteen operations, each with the signature the port owes.
/// The intended forms are stated against `tsr_vfs::FileSystem` because that is
/// the trait a wrapper would have to implement to be usable.
const CACHED: &[(&str, &str)] = &[
    ("From",
     "pub fn from(inner: Arc<dyn FileSystem>) -> CachedFs, returning a wrapper whose cache is \
      already ENABLED (the pin stores true in the constructor) and whose five caches -- \
      directory_exists, file_exists, entries, realpath and stat -- are empty"),
    ("FS.Enable",
     "pub fn enable(&self), setting the flag unconditionally and clearing nothing"),
    ("FS.ClearCache",
     "pub fn clear_cache(&self), emptying all five caches and leaving the enable flag alone"),
    ("FS.DisableAndClearCache",
     "pub fn disable_and_clear_cache(&self), clearing all five caches only on the transition \
      from enabled to disabled (a compare-and-swap, not an unconditional store)"),
    ("FS.DirectoryExists",
     "the cached form of FileSystem::directory_exists, keyed on the caller's raw path with no \
      normalization, storing and serving false as readily as true, and both consulting and \
      populating the cache only while enabled"),
    ("FS.FileExists",
     "the cached form of FileSystem::file_exists, with the same raw key and the same \
      negative-caching and enable-gating rules"),
    ("FS.GetAccessibleEntries",
     "the cached form of FileSystem::entries, caching the zero-valued Entries an unreadable or \
      missing directory yields as a real answer rather than as a miss"),
    ("FS.Realpath",
     "the cached form of FileSystem::realpath, caching the total function's failure fallback -- \
      the echoed input path -- as a real answer, which an Option-shaped cache cannot represent"),
    ("FS.Stat",
     "the cached form of FileSystem::stat over a FileInfo that can answer name and modification \
      time as well as size and is-dir, and whose cache can hold a present-but-absent entry: the \
      pin stores the nil FileInfo and SyncMap.Load serves it without re-querying"),
    ("FS.ReadFile",
     "a bare forward of FileSystem::read_file with NO cache and no enable check; file contents \
      are deliberately not cached"),
    ("FS.UseCaseSensitiveFileNames",
     "a bare forward of FileSystem::use_case_sensitive_file_names with no memoization, asked of \
      the underlying on every call"),
    ("FS.WalkDir",
     "a bare forward of a walk operation the trait does not have: fn walk_dir(&self, root: \
      &[u8], f: &mut dyn FnMut(&[u8], Option<&DirEntry>, Option<Error>) -> WalkDecision) -> \
      Result<(), Error>, forwarding the caller's own decisions -- skip-dir, skip-all and a \
      caller error -- with nothing interposed"),
    ("FS.WriteFile",
     "a bare forward of FileSystem::write_file that invalidates NOTHING, so a cached miss \
      survives the creation unchanged"),
    ("FS.AppendFile",
     "a bare forward of FileSystem::append_file that invalidates NOTHING, so a cached stat \
      keeps the pre-append size while the uncached read sees the appended bytes at once"),
    ("FS.Remove",
     "a bare forward of FileSystem::remove that invalidates NOTHING, so a cached hit keeps \
      reporting a deleted file as present"),
    ("FS.Chtimes",
     "a bare forward carrying BOTH instants -- fn change_times(&self, path: &[u8], a_time: \
      SystemTime, m_time: SystemTime) -- and invalidating nothing. crates/tsr_vfs/src/lib.rs:92 \
      declares `fn change_times(&self, _path: &[u8])`, which drops both arguments: the name \
      matches and the operation does not"),
];

/// The shared internals. `Common` is a root dispatcher, and the port has no
/// type that plays that role.
const COMMON: &[(&str, &str)] = &[
    ("Common.RootAndPath",
     "fn root_and_path(&self, path: &[u8]) -> (Option<&dyn FileSystem>, JsString, JsString), \
      splitting the path, rewriting an empty remainder to \".\", and handing the root's exact \
      spelling to an injectable per-root factory"),
    ("Common.Stat",
     "the root-dispatched form of stat: nil for every failure -- missing, dangling link, \
      unreadable, unserved root -- and a FileInfo carrying name and modification time as well \
      as size and is-dir"),
    ("Common.FileExists",
     "the root-dispatched `stat != nil && !stat.is_dir()`. crates/tsr_vfs/src/lib.rs:77-79 has \
      the predicate but over a whole-path stat and returning Result, not over a root dispatch \
      and not total"),
    ("Common.DirectoryExists",
     "the root-dispatched `stat != nil && stat.is_dir()`, with the same difference from \
      crates/tsr_vfs/src/lib.rs:80-82"),
    ("Common.GetAccessibleEntries",
     "the root-dispatched classifier: ReadDir order preserved, symlinks resolved in place, an \
      optional is-reparse-point hook consulted for irregular entries only, non-regular \
      non-directory entries dropped, and a zero-valued Entries with a present-but-empty \
      symlink set for every failure"),
    ("Common.getEntries",
     "the unfiltered listing under it: fn raw_entries(&self, path: &[u8]) -> Vec<DirEntry> \
      returning every entry ReadDir gave, in order, and an empty vector rather than an error \
      for an unserved root or a failed read"),
    ("Common.ReadFile",
     "the root-dispatched read returning (contents, ok), distinguishing a zero-byte file \
      (empty, true) from every failure (empty, false), and decoding a UTF-16 or UTF-8 byte \
      order mark at read time rather than at insertion time"),
    ("Common.WalkDir",
     "fn walk_dir(&self, root: &[u8], f: ...) -> Result<(), Error> over the root dispatch, \
      rewriting the walk's \".\" to the root name, forwarding skip-dir and skip-all, calling \
      back twice for a directory whose listing fails, and walking nothing for an unserved root"),
    ("SplitPath",
     "pub fn split_path(path: &[u8]) -> (JsString, JsString), normalizing first, then splitting \
      at the root length, then removing a trailing separator from the REMAINDER only"),
];

/// The symbol an operation id names, after the last `:`.
fn symbol(operation: &str) -> &str {
    operation.rsplit(':').next().unwrap_or(operation)
}

fn missing(table: &'static [(&str, &str)], operation: &str, home: &'static str) -> Option<Outcome> {
    let wanted = symbol(operation);
    let (_, signature) = table.iter().find(|(name, _)| *name == wanted)?;
    // The Go authority is the request's own operation id; `missing` takes a
    // 'static authority, so the file is named and the row already carries the
    // symbol under `missing_operation.operation`.
    let authority = if home == CACHED_HOME {
        "tsc/internal/vfs/cachedvfs/cachedvfs.go"
    } else {
        "tsc/internal/vfs/internal/internal.go"
    };
    Some(Outcome::missing(authority, signature, home))
}

/// The action vocabulary each subject accepts. An unknown or malformed action
/// is a harness failure on BOTH sides, never a row the two could agree on
/// without executing anything: the Go probe panics on it, and a recorded gap
/// would otherwise swallow it here, because a gap is reported whatever the
/// trace says.
const VOCABULARY: &[(&str, &[&str])] = &[
    (
        "CachedFS",
        &[
            "from",
            "enable",
            "clear_cache",
            "disable_and_clear",
            "directory_exists",
            "file_exists",
            "read_file",
            "realpath",
            "stat",
            "entries",
            "use_case_sensitive_file_names",
            "write_file",
            "append_file",
            "remove",
            "chtimes",
            "walk",
        ],
    ),
    (
        "vfs.Common",
        &[
            "from_common",
            "root_and_path",
            "stat",
            "file_exists",
            "directory_exists",
            "entries",
            "read_file",
            "walk",
        ],
    ),
    ("vfs.RootLength", &["root_length"]),
    ("vfs.SplitPath", &["split_path"]),
];

/// The first action whose op this subject does not define, if there is one.
fn unknown_action(subject: &str, trace: &[Value]) -> Option<String> {
    let (_, vocabulary) = VOCABULARY.iter().find(|(name, _)| *name == subject)?;
    trace
        .iter()
        .map(action_op)
        .find(|op| !vocabulary.contains(op))
        .map(ToOwned::to_owned)
}

/// `vfs.RootLength` over the home the repository's disposition claims for it.
fn root_length(trace: &[Value]) -> Result<Vec<Value>, String> {
    trace
        .iter()
        .map(|action| {
            let path = action
                .get("path")
                .and_then(Value::as_str)
                .ok_or("a root_length action is missing required key path")?;
            Ok(json!({
                "op": "root_length",
                "path": path,
                "length": tsr_tspath::root_length(path.as_bytes()),
            }))
        })
        .collect()
}

pub fn observe(request: &Value) -> Option<Outcome> {
    let case = request.get("case").and_then(Value::as_str).unwrap_or("");
    if !case.starts_with(CASE_PREFIX) {
        return None;
    }
    let operation = request
        .get("operation")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let subject = subject(request);
    if let Some(op) = unknown_action(subject, actions(request)) {
        return Some(Outcome::Failed(format!(
            "case {case:?} carries action {op:?}, which subject {subject:?} does not define"
        )));
    }
    if subject == "vfs.RootLength" {
        return Some(match root_length(actions(request)) {
            Ok(rows) => Outcome::Observed(ordered(rows)),
            Err(problem) => Outcome::Failed(problem),
        });
    }
    let recorded = match subject {
        "CachedFS" => missing(CACHED, operation, CACHED_HOME),
        "vfs.Common" | "vfs.SplitPath" => missing(COMMON, operation, COMMON_HOME),
        other => {
            return Some(Outcome::Failed(format!(
                "case {case:?} declares subject {other:?}, which the cachedvfs group does not \
                 serve"
            )))
        }
    };
    // A claimed case with no entry in the group's table is a harness failure,
    // never an observation: it means the schedule and this module disagree
    // about which operations the group covers.
    Some(recorded.unwrap_or_else(|| {
        Outcome::Failed(format!(
            "case {case:?} names operation {operation:?}, for which the cachedvfs group has no \
             recorded Rust gap"
        ))
    }))
}
