//! The `vfs/vfstest` group: the pinned in-memory test filesystem.
//!
//! Almost all of it is a recorded gap. `MapFS` is a mutable filesystem keyed by
//! a canonical path, carrying a per-entry original spelling, a per-entry
//! modification time, an injectable clock, a symlink registry, file handles and
//! a directory cursor. `tsr_vfs` has none of that: `MemorySnapshot` is an
//! immutable published map whose `FileInfo` is `{ directory, size }`
//! (crates/tsr_vfs/src/lib.rs:64-67), there is no `Clock`, no handle type and no
//! `Open` on the `FileSystem` trait (lib.rs:70-95), and the mutating half of
//! the trait is four deliberate capability errors (lib.rs:83-94). Several
//! same-named Rust functions exist and were read before being rejected; each
//! `MISSING` row below says which one and why it is not a counterpart.
//!
//! Preparation records the gap. It never emulates a missing algorithm to make a
//! comparison run, and it never reads an expected value.
//!
//! The one part that does run is the paired snapshot. The plan asks for
//! snapshot A to stay alive while the live filesystem is mutated and B is
//! published, and for A's retention to be recorded as an assertion about the
//! Rust side rather than as an invented Go snapshot operation. `MemoryBuilder`
//! and `MemorySnapshot` are the real entry points for that, so the two
//! `MapFSSnapshot` cases call them and record what they answer.
//!
//! The live filesystem is modelled as the current `MemorySnapshot`, shared
//! behind an `Arc`, and `publish` hands the view that same object rather than a
//! copy of it. That matters: `MapFS` has no snapshot operation, and the pin's
//! published view is the live `*fstest.MapFile` `GetFileInfo` returns
//! (vfstest.go:671), so a faithful pairing has the Rust view hold the object
//! the later mutations address. A mutation that reached an already published
//! snapshot would then be read back at `read_view`. An earlier shape published
//! from a throwaway `live.clone().finish()` could not have observed that at
//! all, because nothing ever mutated the clone the view was finished from.
//!
//! Neither `mutate_` action emulates the pinned method. `mutate_chtimes` calls
//! `FileSystem::change_times` on the live filesystem and records its real
//! answer, a capability error. `mutate_write` records `FileSystem::write_file`'s
//! real answer too, and names the `MemoryBuilder` substitution that advances the
//! trace, so the row cannot be read as parity on `MapFS.WriteFile`.

use std::{collections::BTreeMap, sync::Arc};

use serde_json::{json, Value};
use tsr_vfs::{Error, FileSystem, MemoryBuilder, MemorySnapshot, SnapshotId};

use crate::api::{self, Outcome};

/// The case prefix this group owns. The schedule is shared with every other
/// filesystem group and a subject string could collide with a neighbour's, so
/// ownership is keyed on the case id this group was assigned; the subject then
/// only picks between the group's own two trace shapes.
const CASE_PREFIX: &str = "filesystem/vfstest/";

/// Every operation a `MapFS` case names, with the Rust home that does not have
/// it. Keyed by the request's own operation id so a recorded gap names the
/// operation the case was written for rather than the subject it shares.
const MISSING: &[(&str, &str, &str)] = &[
    (
        "tsc/internal/vfs/vfstest/vfstest.go:FromMap",
        "pub fn from_map(entries: &[(&[u8], MapFile)], case_sensitive: bool) -> MemorySnapshot, \
         panicking `non-rooted path %q`, `non-normalized path %q` and `mixed posix and windows \
         paths` before any entry is made, and running the same check over a symlink's target. The \
         pin's fourth rejection is not asked for: `invalid file type %T` (vfstest.go:118-119) \
         formats the offending value's dynamic Go type, and a typed MapFile parameter has no such \
         case to reject, so a port could only reproduce that sentence by hardcoding it. \
         MemoryBuilder::new + insert_physical + finish \
         (crates/tsr_vfs/src/lib.rs:114, :146, :162) is the nearest construct and is not a \
         counterpart: it keys on tsr_tspath::to_path = canonical(absolute(..)) (lib.rs:131), which \
         accepts a relative key against the builder's cwd and silently normalizes `/a/../b`",
        "crates/tsr_vfs/src/lib.rs (no rejecting constructor; MemoryBuilder accepts every path)",
    ),
    (
        "tsc/internal/vfs/vfstest/vfstest.go:FromMapWithClock",
        "pub trait Clock { fn now(&self) -> Instant; } plus a from_map_with_clock that stamps every \
         file from the injected clock in sorted order and every synthesized directory afterwards. \
         tsr_vfs models no time at all: FileInfo is { directory, size } (crates/tsr_vfs/src/lib.rs:64-67) \
         and no entry, builder or snapshot carries an instant to stamp",
        "crates/tsr_vfs/src/lib.rs (no Clock trait, no modification time, nothing to inject into)",
    ),
    (
        "tsc/internal/vfs/vfstest/vfstest.go:comparePathsByParts",
        "fn compare_paths_by_parts(a: &[u8], b: &[u8]) -> Ordering, comparing separator-delimited \
         segments and falling back to a whole-string compare once either side runs out of \
         separators. tsr_vfs orders entries by BTreeMap key order over canonical paths \
         (crates/tsr_vfs/src/lib.rs:175) and MemorySnapshot::entries sorts basenames with the \
         derived Ord (lib.rs:279-280), which is byte order and reverses the pin on `a/b` vs `a-x/c`",
        "crates/tsr_vfs/src/lib.rs and crates/tsr_tspath/src/lib.rs (absent in both)",
    ),
    (
        "tsc/internal/vfs/vfstest/vfstest.go:MapFS.AddSymlink",
        "pub fn add_symlink(&mut self, path: &[u8], target: &[u8]) taking an UNROOTED target, \
         creating no parent directories, stamping no modification time and registering the link \
         under its canonical key. MemoryBuilder::insert_symlink (crates/tsr_vfs/src/lib.rs:155-157) \
         was read and rejected: it routes through insert (lib.rs:128), which materialises every \
         missing ancestor, and its target is resolved relative to the link's own directory \
         (lib.rs:193) rather than stored verbatim",
        "crates/tsr_vfs/src/lib.rs:155 (present but not a counterpart; MapFS itself is absent)",
    ),
    (
        "tsc/internal/vfs/vfstest/vfstest.go:MapFS.AppendFile",
        "fn append_file(&self, path: &[u8], data: &[u8], perm: Mode) -> Result<(), Error> that \
         tolerates a broken symlink and writes through it under the link's own realpath, keeps an \
         existing file's mode and applies perm&^umask only when there is none. The trait's \
         append_file is the deliberate capability error Unsupported(\"immutable filesystem append\") \
         (crates/tsr_vfs/src/lib.rs:86-88) and MemoryBuilder has no append at all",
        "crates/tsr_vfs/src/lib.rs:86 (capability error) with no builder counterpart",
    ),
    (
        "tsc/internal/vfs/vfstest/vfstest.go:MapFS.Chtimes",
        "fn change_times(&self, path: &[u8], a_time: Instant, m_time: Instant) -> Result<(), Error>, \
         storing m_time and discarding a_time, resolving no symlink and reporting a miss for a path \
         that still carries its leading separator. The trait's change_times takes NO instants \
         (crates/tsr_vfs/src/lib.rs:92-94) and returns the deliberate capability error \
         Unsupported(\"immutable filesystem timestamps\"), so this is a signature gap as well as a \
         capability gap",
        "crates/tsr_vfs/src/lib.rs:92 (wrong signature, capability error)",
    ),
    (
        "tsc/internal/vfs/vfstest/vfstest.go:MapFS.GetFileInfo",
        "fn get_file_info(&self, path: &[u8]) -> Option<&Entry> answering for the entry stored at \
         the canonical key WITHOUT resolving symlinks, and carrying the stored mode, bytes, \
         original spelling and caller payload. MemorySnapshot::stat (crates/tsr_vfs/src/lib.rs:231) \
         is the same-named function and is not a counterpart: it resolves through self.get -> \
         self.resolve (lib.rs:208-213), so it always answers for the target",
        "crates/tsr_vfs/src/lib.rs:231 (resolves; no mode, no realpath, no payload)",
    ),
    (
        "tsc/internal/vfs/vfstest/vfstest.go:MapFS.Open",
        "fn open(&self, path: &[u8]) -> Result<Box<dyn File>, Error> returning a handle whose Stat \
         substitutes the stored realpath and the caller's payload, and whose directory form carries \
         a ReadDir cursor; plus the `.` exemption and the `unexpected synthesized dir: %q` panic \
         everywhere else. tsr_vfs has no handle type and the FileSystem trait has no open \
         (crates/tsr_vfs/src/lib.rs:70-95)",
        "crates/tsr_vfs/src/lib.rs:70-95 (no handle type, no open)",
    ),
    (
        "tsc/internal/vfs/vfstest/vfstest.go:MapFS.Realpath",
        "fn realpath(&self, path: &[u8]) -> Result<JsString, Error> returning the STORED original \
         spelling and an error when nothing is there. MemorySnapshot::realpath \
         (crates/tsr_vfs/src/lib.rs:283-288) was read and rejected: it falls back to the resolved \
         path instead of erroring on absence, and the name it returns is the caller's own absolute \
         spelling rather than a separately recorded original",
        "crates/tsr_vfs/src/lib.rs:283 (present but not a counterpart)",
    ),
    (
        "tsc/internal/vfs/vfstest/vfstest.go:MapFS.Remove",
        "fn remove(&mut self, path: &[u8]) -> Result<(), Error> removing a directory recursively, \
         guarding a sibling that merely shares the removed path's prefix, purging the symlink \
         registry for every key it drops, and returning Ok for a path that is not there. \
         MemoryBuilder::remove (crates/tsr_vfs/src/lib.rs:158-161) was read and rejected: it drops \
         one key with no recursion, no prefix scan and no registry, and the trait's remove is the \
         capability error Unsupported(\"immutable filesystem remove\") (lib.rs:89-91)",
        "crates/tsr_vfs/src/lib.rs:158 (one key) and :89 (capability error)",
    ),
    (
        "tsc/internal/vfs/vfstest/vfstest.go:MapFS.WriteFile",
        "fn write_file(&mut self, path: &[u8], data: &[u8], perm: Mode) -> Result<(), Error> that \
         checks the parent only when dirName is non-empty, distinguishes a missing parent from a \
         parent that is not a directory from a target that is not a regular file, and tolerates a \
         broken symlink. MemoryBuilder::insert_physical (crates/tsr_vfs/src/lib.rs:146-148) was read \
         and rejected: it is infallible and creates every missing ancestor; the trait's write_file is \
         the capability error Unsupported(\"immutable filesystem write\") (lib.rs:83-85)",
        "crates/tsr_vfs/src/lib.rs:146 (infallible) and :83 (capability error)",
    ),
    (
        "tsc/internal/vfs/vfstest/vfstest.go:MapFS.mkdirAll",
        "fn mkdir_all(&mut self, path: &[u8], perm: Mode) -> Result<(), Error> walking root-first \
         with a resumable offset, restarting from the target's realpath when a component turns out \
         to be a symlink, creating nothing until the whole path has been walked, and applying \
         perm&^umask. The nearest Rust behaviour is the implicit ancestor creation inside \
         MemoryBuilder::insert (crates/tsr_vfs/src/lib.rs:129-135), which is leaf-first over \
         tsr_tspath::ancestors, creates as it goes and cannot express the restart",
        "crates/tsr_vfs/src/lib.rs:129 (implicit, leaf-first, no restart, no modes)",
    ),
    (
        "tsc/internal/vfs/vfstest/vfstest.go:MapFS.getFollowingSymlinksWorker",
        "fn resolve_worker(&self, path: &[u8], from: &[u8], to: &[u8]) -> Result<(&Entry, JsString), Error> \
         carrying the last hop it traversed so a dangling link reports `broken symlink %q -> %q` \
         over CANONICAL spellings, and distinguishing that from a plain miss. \
         MemorySnapshot::resolve (crates/tsr_vfs/src/lib.rs:182-204) was read and rejected: it caps \
         at 40 rewrites and returns the unresolved name, and tsr_vfs::Error has no broken-link \
         variant to report (lib.rs:13-19), so both outcomes collapse into one miss",
        "crates/tsr_vfs/src/lib.rs:182 (no error taxonomy) and :13 (no variant)",
    ),
    (
        "tsc/internal/vfs/vfstest/vfstest.go:MapFS.setEntry",
        "fn set_entry(&mut self, realpath: &[u8], canonical: &[u8], entry: Entry) storing the \
         caller's payload nested inside the filesystem's own wrapper alongside the original \
         spelling, panicking `empty path` for either half, and registering a symlink under its \
         canonical target. MemoryBuilder::insert (crates/tsr_vfs/src/lib.rs:128-145) stores a \
         NamedEntry { name, value } under to_path(name) and is partial only: it derives the name \
         from the key's own input, so the two can never disagree, and there is no payload slot",
        "crates/tsr_vfs/src/lib.rs:128 (one name derived from the key, no payload, no panic)",
    ),
    (
        "tsc/internal/vfs/vfstest/vfstest.go:convertMapFS",
        "fn convert_map_fs(input: &[(&[u8], MapFile)], case_sensitive: bool, clock: &dyn Clock) -> MapFS \
         panicking `duplicate path: %q and %q have the same canonical path` in case-insensitive mode \
         only, and `failed to create intermediate directories for %q: %v` when a parent is already a \
         file. MemoryBuilder::finish (crates/tsr_vfs/src/lib.rs:162-170) is the only build step and \
         is a move of an already-populated map: it verifies nothing and cannot reject either input",
        "crates/tsr_vfs/src/lib.rs:162 (no verification pass)",
    ),
    (
        "tsc/internal/vfs/vfstest/vfstest.go:fileInfo.Name",
        "impl FileInfo { fn name(&self) -> &[u8] } returning the basename of the STORED original \
         spelling rather than of the canonical key, so a case-insensitive lookup preserves case. \
         tsr_vfs::FileInfo has no name field at all (crates/tsr_vfs/src/lib.rs:64-67); \
         MemorySnapshot::entries derives basenames separately with tsr_tspath::base_name \
         (lib.rs:267), from the stored name, with no handle to carry them",
        "crates/tsr_vfs/src/lib.rs:64 (no name field, no handle to hang one on)",
    ),
    (
        "tsc/internal/vfs/vfstest/vfstest.go:splitPath",
        "fn split_path(s: &[u8], offset: usize) -> (&[u8], &[u8]) splitting at the first separator \
         AT OR AFTER offset, so a caller can resume a root-first walk. tsr_tspath::ancestors \
         (crates/tsr_tspath/src/lib.rs:147-159) is the port's only path walker and goes leaf-first, \
         which cannot express mkdirAll's restart; there is no resumable root-first split",
        "crates/tsr_tspath/src/lib.rs:147 (leaf-first, not resumable)",
    ),
];

pub fn observe(request: &Value) -> Option<Outcome> {
    let case = request.get("case").and_then(Value::as_str).unwrap_or("");
    if !case.starts_with(CASE_PREFIX) {
        return None;
    }
    match api::subject(request) {
        "MapFS" => Some(missing_for(request)),
        "MapFSSnapshot" => Some(match snapshot_trace(request) {
            Ok(rows) => Outcome::Observed(api::ordered(rows)),
            Err(error) => Outcome::Failed(error),
        }),
        other => Some(Outcome::Failed(format!(
            "case {case:?} declares subject {other:?}, which the vfstest group does not serve"
        ))),
    }
}

fn missing_for(request: &Value) -> Outcome {
    let operation = request
        .get("operation")
        .and_then(Value::as_str)
        .unwrap_or_default();
    match MISSING.iter().find(|(name, _, _)| *name == operation) {
        Some((authority, signature, home)) => {
            Outcome::missing(*authority, authority, signature, home)
        }
        // A gap has to name a real operation. Inventing a record for an
        // operation nobody read would be exactly the over-attribution this
        // step exists to prevent.
        None => Outcome::Failed(format!(
            "no recorded Rust gap for operation {operation:?}; add it to vfstest::MISSING"
        )),
    }
}

// ---------------------------------------------------------------------------
// the paired snapshot

/// What `publish` captured, kept alive while the live filesystem moves on.
/// The snapshot is shared rather than copied, so the view holds the object a
/// later mutation would have to leak into for this pairing to detect it.
struct View {
    snapshot: Arc<MemorySnapshot>,
    id: Option<SnapshotId>,
    paths: Vec<String>,
    stamp: BTreeMap<String, Vec<Value>>,
}

/// Reduce a `tsr_vfs` failure to the same two-element vocabulary the native
/// probe uses, so a row that differs differs on the outcome and not on wording.
fn error_row(result: Result<(), Error>) -> Vec<Value> {
    match result {
        Ok(()) => vec![json!("none"), json!("")],
        Err(Error::Unsupported(reason)) => vec![json!("unsupported"), json!(reason)],
        Err(Error::OutsideScope) => vec![json!("outside_scope"), json!("")],
        Err(Error::InvalidPath) => vec![json!("invalid_path"), json!("")],
        Err(Error::SymlinkCycle) => vec![json!("symlink_cycle"), json!("")],
        Err(Error::Io(_)) => vec![json!("io"), json!("")],
    }
}

/// One published path's reading: present with its bytes, or absent.
fn reading(snapshot: &MemorySnapshot, path: &str) -> Result<Vec<Value>, String> {
    match snapshot.read_file(path.as_bytes()) {
        Ok(Some(content)) => Ok(vec![
            json!("present"),
            Value::String(String::from_utf8_lossy(&content.raw).into_owned()),
        ]),
        Ok(None) => Ok(vec![json!("absent"), json!("")]),
        Err(error) => Err(format!("read of {path:?} failed: {error}")),
    }
}

/// A published path's full stamp, which `view_unchanged` re-derives and
/// compares. Bytes and the directory flag are everything `MemorySnapshot`
/// models; the snapshot identity is compared alongside them.
fn stamp(snapshot: &MemorySnapshot, path: &str) -> Result<Vec<Value>, String> {
    let mut row = reading(snapshot, path)?;
    match snapshot.stat(path.as_bytes()) {
        Ok(Some(info)) => {
            row.push(json!(info.directory));
            row.push(json!(info.size));
        }
        Ok(None) => {
            row.push(json!(false));
            row.push(json!(0));
        }
        Err(error) => return Err(format!("stat of {path:?} failed: {error}")),
    }
    Ok(row)
}

fn need_str<'a>(action: &'a Value, op: &str, field: &str) -> Result<&'a str, String> {
    match action.get(field).and_then(Value::as_str) {
        Some(value) if !value.is_empty() => Ok(value),
        _ => Err(format!("action {op} needs a non-empty {field}")),
    }
}

fn need_bool(action: &Value, op: &str, field: &str) -> Result<bool, String> {
    action
        .get(field)
        .and_then(Value::as_bool)
        .ok_or_else(|| format!("action {op} needs {field}"))
}

fn need_paths(action: &Value, op: &str) -> Result<Vec<String>, String> {
    let list = action
        .get("paths")
        .and_then(Value::as_array)
        .ok_or_else(|| format!("action {op} needs the paths it publishes"))?;
    if list.is_empty() {
        return Err(format!("action {op} needs a non-empty paths list"));
    }
    list.iter()
        .map(|value| {
            value
                .as_str()
                .map(str::to_owned)
                .ok_or_else(|| format!("action {op} has a non-string path"))
        })
        .collect()
}

/// Populate a builder from a request's `[path, kind, data]` rows. A kind this
/// group did not write is a defect in the request, not an observation.
fn build(action: &Value, op: &str) -> Result<MemoryBuilder, String> {
    let files = action
        .get("files")
        .and_then(Value::as_array)
        .ok_or_else(|| format!("action {op} needs a files list"))?;
    if files.is_empty() {
        return Err(format!("action {op} needs a non-empty files list"));
    }
    let mut builder = MemoryBuilder::new(b"/", need_bool(action, op, "case_sensitive")?);
    for row in files {
        let parts: Vec<&str> = row
            .as_array()
            .map(|items| items.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default();
        let [path, kind, data] = parts.as_slice() else {
            return Err(format!("action {op} needs [path, kind, data] rows"));
        };
        match *kind {
            "string" | "bytes" | "mapfile" | "mapfile_sys" => {
                builder.insert_physical(path.as_bytes(), data.as_bytes().to_vec());
            }
            "symlink" => builder.insert_symlink(path.as_bytes(), data.as_bytes()),
            other => return Err(format!("action {op} has unknown file kind {other:?}")),
        }
    }
    Ok(builder)
}

fn snapshot_trace(request: &Value) -> Result<Vec<Value>, String> {
    let actions = api::actions(request);
    if actions.is_empty() {
        return Err("snapshot trace has no actions".into());
    }
    let mut live: Option<Arc<MemorySnapshot>> = None;
    let mut views: BTreeMap<String, View> = BTreeMap::new();
    let mut rows = Vec::with_capacity(actions.len());
    for action in actions {
        let op = api::action_op(action);
        let mut row = serde_json::Map::new();
        row.insert("op".into(), json!(op));
        match op {
            "build" => {
                live = Some(Arc::new(build(action, op)?.finish()));
                let count = action
                    .get("files")
                    .and_then(Value::as_array)
                    .map_or(0, Vec::len);
                row.insert("result".into(), json!(["built", count]));
            }
            "publish" => {
                let current = live
                    .as_ref()
                    .ok_or_else(|| format!("action {op} ran before build"))?;
                let name = need_str(action, op, "name")?.to_owned();
                let paths = need_paths(action, op)?;
                // The view holds the live filesystem itself, not a copy: a
                // mutation that reached an already published snapshot has to
                // be observable at read_view for this pairing to mean
                // anything.
                let snapshot = Arc::clone(current);
                let mut contents = Vec::with_capacity(paths.len());
                let mut stamps = BTreeMap::new();
                for path in &paths {
                    let mut entry = vec![json!(path)];
                    entry.extend(reading(&snapshot, path)?);
                    contents.push(Value::Array(entry));
                    stamps.insert(path.clone(), stamp(&snapshot, path)?);
                }
                row.insert("result".into(), json!(["published", name, contents]));
                row.insert("name".into(), json!(name));
                views.insert(
                    name,
                    View {
                        id: snapshot.snapshot_id(),
                        snapshot,
                        paths,
                        stamp: stamps,
                    },
                );
            }
            "mutate_write" => {
                let current = live
                    .as_ref()
                    .ok_or_else(|| format!("action {op} ran before build"))?;
                let path = need_str(action, op, "path")?.to_owned();
                let data = api::action_str(action, "data").as_bytes().to_vec();
                // The production entry point answers first. tsr_vfs has no
                // write: FileSystem::write_file is the deliberate capability
                // error (crates/tsr_vfs/src/lib.rs:83-85), and the recorded
                // gap for MapFS.WriteFile already says insert_physical is not
                // a counterpart. Recording a bare success here would have
                // claimed parity with the pin's WriteFile through the very
                // construct that gap rejects.
                let mut result = vec![
                    json!("substituted"),
                    json!("MemoryBuilder::insert_physical"),
                ];
                result.extend(error_row(current.write_file(path.as_bytes(), &data)));
                // The named substitution, which is what advances the trace. It
                // replaces the whole filesystem instead of assigning into the
                // published one; whether that leaves an earlier view's bytes
                // alone is what the retention rows go on to measure.
                let mut next = MemoryBuilder::from_snapshot(current);
                next.insert_physical(path.as_bytes(), data);
                live = Some(Arc::new(next.finish()));
                row.insert("result".into(), Value::Array(result));
                row.insert("path".into(), json!(path));
            }
            "mutate_chtimes" => {
                // The production entry point for "change this filesystem's
                // times" is FileSystem::change_times, called here on the live
                // filesystem, which is the same object any published view
                // holds. Its real answer is a deliberate capability error, and
                // that is what is recorded. The request carries the key
                // spelling, because the pin's Chtimes strips no separator of
                // its own, and the path is forwarded verbatim so both sides
                // address the same name.
                let current = live
                    .as_ref()
                    .ok_or_else(|| format!("action {op} ran before build"))?;
                let path = need_str(action, op, "path")?;
                // Both instants are required by the request even though the
                // Rust signature has nowhere to put either; a request that
                // omitted one would be describing a different action.
                need_str(action, op, "a_time")?;
                need_str(action, op, "m_time")?;
                row.insert(
                    "result".into(),
                    json!(error_row(current.change_times(path.as_bytes()))),
                );
                row.insert("path".into(), json!(path));
            }
            "read_view" => {
                let name = need_str(action, op, "name")?;
                let path = need_str(action, op, "path")?;
                let view = views
                    .get(name)
                    .ok_or_else(|| format!("action {op} names an unpublished view: {name}"))?;
                if !view.paths.iter().any(|held| held == path) {
                    return Err(format!(
                        "action {op} names a path view {name} never published"
                    ));
                }
                let mut result = match view.snapshot.read_file(path.as_bytes()) {
                    Ok(Some(content)) => vec![
                        json!("bytes"),
                        Value::String(String::from_utf8_lossy(&content.raw).into_owned()),
                    ],
                    Ok(None) => vec![json!("absent"), json!("")],
                    Err(error) => return Err(format!("read of {path:?} failed: {error}")),
                };
                result.truncate(2);
                row.insert("result".into(), Value::Array(result));
                row.insert("name".into(), json!(name));
                row.insert("path".into(), json!(path));
            }
            "read_live" => {
                let current = live
                    .as_ref()
                    .ok_or_else(|| format!("action {op} ran before build"))?;
                let path = need_str(action, op, "path")?;
                let result = match current.read_file(path.as_bytes()) {
                    Ok(Some(content)) => vec![
                        json!("bytes"),
                        Value::String(String::from_utf8_lossy(&content.raw).into_owned()),
                    ],
                    Ok(None) => vec![json!("absent"), json!("")],
                    Err(error) => return Err(format!("read of {path:?} failed: {error}")),
                };
                row.insert("result".into(), Value::Array(result));
                row.insert("path".into(), json!(path));
            }
            "view_unchanged" => {
                let name = need_str(action, op, "name")?;
                let view = views
                    .get(name)
                    .ok_or_else(|| format!("action {op} names an unpublished view: {name}"))?;
                // The identity check is the weaker half and is honest about
                // it: MemorySnapshot stores its id at finish and hands the
                // same value back (crates/tsr_vfs/src/lib.rs:172-177,
                // :219-221), so today this can only be true. It is kept
                // because a port that derived the id from content would move
                // it under a leak; the stamps below are what actually measure
                // one, and neither can see a timestamp change, because a
                // MemorySnapshot records no modification time at all.
                let mut unchanged = view.id == view.snapshot.snapshot_id();
                for path in &view.paths {
                    if view.stamp.get(path) != Some(&stamp(&view.snapshot, path)?) {
                        unchanged = false;
                    }
                }
                row.insert("result".into(), json!(["unchanged", unchanged]));
                row.insert("name".into(), json!(name));
            }
            other => {
                return Err(format!(
                    "unsupported snapshot action {other:?}; an unknown action is a harness failure"
                ))
            }
        }
        rows.push(Value::Object(row));
    }
    Ok(rows)
}
