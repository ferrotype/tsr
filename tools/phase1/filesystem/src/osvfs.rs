//! The live-OS adapter group: `internal/vfs/osvfs`, plus the two packages it
//! composes, `internal/nativepath` and `internal/osutil`.
//!
//! Every subject here is a recorded gap. There is no live OS `FileSystem` in
//! the port: `grep -rn "FileSystem for" crates/` returns exactly two impls,
//! `MemorySnapshot` at crates/tsr_vfs/src/lib.rs:215 and `BundledFs` at
//! crates/tsr_bundled/src/lib.rs:455, and both answer from memory. The only
//! OS-aware Rust construct, `ScopedOsFs` (crates/tsr_vfs/src/os.rs:8-57), is a
//! one-shot acquisition that walks a root into an immutable `MemorySnapshot`;
//! it implements nothing and it is the opposite of what this adapter is, so it
//! is named as the nearest construct rather than as a counterpart.
//!
//! PORTS.toml agrees and is the authority the record leans on:
//! `tsc/internal/vfs/osvfs/os.go` is `status = "planned"` with `rust = []`
//! (PORTS.toml:6939-6949), and so are all three `internal/nativepath` files
//! this group reaches and both `internal/osutil` files.
//!
//! Preparation records the gap; it never emulates the algorithm to make a
//! comparison run, and it never reads an expected value. Each row names the
//! pinned Go authority, the signature the port is expected to carry, and the
//! file that does not exist yet -- or, for the four subjects whose trait
//! signature DOES exist, the signature that is present and why it is not a
//! counterpart.

use serde_json::Value;

use crate::api::{subject, Outcome};

/// Subjects this group owns. A request is claimed only when its subject is one
/// of these AND its operation is in `MISSING`, so a neighbouring group's case
/// cannot be answered here by an operation id alone.
const SUBJECTS: &[&str] = &[
    "osvfs.swapCase",
    "osvfs.UseCaseSensitiveFileNames",
    "osutil.ProcessIdentity",
    "osvfs.FS",
    "osvfs.ReadFile",
    "osvfs.Stat",
    "osvfs.Exists",
    "osvfs.GetAccessibleEntries",
    "osvfs.Realpath",
    "nativepath.Realpath",
    "nativepath.RealpathLinux",
    "nativepath.IsSymlinkOrReparsePoint",
    "osvfs.WriteFile",
    "osvfs.WriteLayers",
    "osvfs.Remove",
    "osvfs.Chtimes",
    "osvfs.WalkDir",
    "osvfs.WalkBinding",
    "osvfs.GetGlobalTypingsCacheLocation",
];

/// Keyed by the request's `operation`, because three subjects in this group
/// carry more than one pinned operation: the two Realpath layers, the two
/// mutating writers and the three write layers each need their own authority,
/// intended signature and production home.
const MISSING: &[(&str, &str, &str)] = &[
    ("tsc/internal/vfs/osvfs/os.go:swapCase",
     "a private fn swap_case(&str) -> String (or over bytes) applying Go's SIMPLE case mappings \
      rune by rune: to_upper(r) if that differs from r, otherwise to_lower(r). It cannot be built \
      from char::to_uppercase/to_lowercase, which are full mappings and turn U+00DF into \"SS\", \
      and it must replace an invalid UTF-8 byte with U+FFFD the way strings.Map does",
     "crates/tsr_vfs (absent). The one case-mapping helper in the port is \
      crates/tsr_jsstring/src/helpers.rs, which is ASCII-only and serves the scanner, not a \
      filesystem case probe"),
    ("tsc/internal/vfs/osvfs/os.go:osFS.UseCaseSensitiveFileNames",
     "the DETECTION behind FileSystem::use_case_sensitive_file_names: a process-level OnceLock \
      computed at startup from the current executable's path spelled with its case swapped, \
      answering false on windows and true on wasm without probing",
     "crates/tsr_vfs/src/lib.rs:71 declares the getter and MemorySnapshot answers it from a \
      constructor argument (lib.rs:216-218); nothing in crates/ derives the value from the host, \
      and grep for current_exe across crates, xtask and tools returns nothing"),
    ("tsc/internal/osutil/osutil.go:Args",
     "pub fn args() -> Vec<OsString> returning the full argv including argv[0], with the \
      launcher-stripping confined to the android build",
     "crates/tsr_core (absent). std::env::args appears only in binaries and examples, never \
      behind an osutil-shaped accessor; PORTS.toml:4092-4102 records tsc/internal/osutil/osutil.go \
      as status = \"planned\" with rust = []"),
    ("tsc/internal/osutil/os_other.go:args",
     "the non-android build of args(): std::env::args_os().collect() verbatim, with no filtering \
      at all",
     "crates/tsr_core (absent). PORTS.toml:4079-4089 records tsc/internal/osutil/os_other.go as \
      status = \"planned\" with rust = []"),
    ("tsc/internal/osutil/osutil.go:Executable",
     "pub fn executable() -> std::io::Result<PathBuf>, fallible, whose failure the case-sensitivity \
      probe is not allowed to tolerate: the pin panics on it at os.go:62-65",
     "crates/tsr_core (absent). The nearest construct is std::env::current_exe(), which no \
      production Rust file calls"),
    ("tsc/internal/osutil/os_other.go:executable",
     "the non-android build of executable(): std::env::current_exe() verbatim, inheriting the \
      platform behavior including darwin's UNRESOLVED answer",
     "crates/tsr_core (absent)"),
    ("tsc/internal/vfs/osvfs/os.go:FS",
     "pub fn fs() -> &'static dyn FileSystem (or an Arc of one) handing back ONE process-level \
      live-OS host, so two acquisitions observe each other's writes",
     "crates/tsr_vfs (absent). There is no live OS FileSystem to hand back: the only OS-aware \
      construct, ScopedOsFs (crates/tsr_vfs/src/os.rs:8), produces an immutable MemorySnapshot \
      instead"),
    ("tsc/internal/vfs/osvfs/os.go:osFS.ReadFile",
     "an OS implementor of FileSystem::read_file that collapses every failure to the missing \
      answer, distinguishes an EMPTY file from a missing one, strips a UTF-8 BOM, decodes both \
      UTF-16 BOMs (dropping an odd trailing byte) and returns invalid UTF-8 unchanged",
     "crates/tsr_vfs/src/lib.rs:73 declares read_file and MemorySnapshot implements it \
      (lib.rs:222-230); no implementor reads the live OS"),
    ("tsc/internal/vfs/osvfs/os.go:osFS.Stat",
     "an OS implementor of FileSystem::stat carrying the fields the pin's io/fs.FileInfo does -- \
      basename, size, IsDir, mode and modification time -- and answering None for a denied path \
      as well as a missing one",
     "crates/tsr_vfs/src/lib.rs:74 declares stat, but FileInfo (lib.rs:64-67) has only `directory` \
      and `size`: no Name, Mode, ModTime or Sys. The contract is present and structurally \
      narrower, and no implementor reads the live OS"),
    ("tsc/internal/vfs/osvfs/os.go:osFS.FileExists",
     "an OS implementor whose file_exists is `stat is Some and not a directory` -- NOT \
      `metadata.is_file()`, which means regular file and answers differently for a FIFO",
     "crates/tsr_vfs/src/lib.rs:77-79 provides the default over stat, which is the right shape; \
      no implementor reads the live OS"),
    ("tsc/internal/vfs/osvfs/os.go:osFS.DirectoryExists",
     "an OS implementor whose directory_exists follows symlinks (stat, not symlink_metadata), so \
      a symlink to a directory is true and a dangling symlink is false",
     "crates/tsr_vfs/src/lib.rs:80-82 provides the default over stat; no implementor reads the \
      live OS, and the only OS code in the crate reaches for symlink_metadata \
      (crates/tsr_vfs/src/os.rs:31), which is the opposite choice"),
    ("tsc/internal/vfs/osvfs/os.go:osFS.GetAccessibleEntries",
     "an OS implementor of FileSystem::entries returning entries SORTED BY NAME (io/fs.ReadDir \
      sorts via os.ReadDir), dropping any entry that is neither a directory nor a regular file \
      after following symlinks, and always returning a non-nil symlink set",
     "crates/tsr_vfs/src/lib.rs:75 declares entries and MemorySnapshot implements it \
      (lib.rs:250-281), and its sort (lib.rs:279-280) AGREES with the pin, so ordering is not the \
      gap. The gap is that no implementor reads the live OS, and that MemorySnapshot admits every \
      stat-able entry (lib.rs:269) rather than dropping entries that are neither a directory nor \
      a regular file"),
    ("tsc/internal/vfs/osvfs/os.go:osFS.Realpath",
     "an OS implementor of FileSystem::realpath that is TOTAL on failure -- it returns the input \
      untouched rather than an error -- and that does not correct the case of a spelling on a \
      case-insensitive volume",
     "crates/tsr_vfs/src/lib.rs:76 declares realpath as Result<JsString, Error>, a fallible \
      signature the pin's total contract cannot use; no implementor reads the live OS"),
    ("tsc/internal/vfs/osvfs/os.go:osFSRealpath",
     "the private composition behind it: assert the path is rooted (panicking if not), translate \
      separators, resolve, absolutise, normalise to forward slashes, and return the ORIGINAL \
      string on either failure",
     "crates/tsr_vfs (absent). std::fs::canonicalize appears at crates/tsr_vfs/src/os.rs:12 and \
      :33 inside ScopedOsFs, where its error is mapped to Error::Io rather than swallowed"),
    ("tsc/internal/vfs/osvfs/os.go:osFS.WriteFile",
     "an OS implementor of FileSystem::write_file opening O_WRONLY|O_CREATE|O_TRUNC with mode \
      0o666 under the umask, creating missing parents on the retry",
     "crates/tsr_vfs/src/lib.rs:83-85 has only the default body, which returns \
      Err(Error::Unsupported(\"immutable filesystem write\")); neither MemorySnapshot nor \
      BundledFs overrides it"),
    ("tsc/internal/vfs/osvfs/os.go:osFS.AppendFile",
     "an OS implementor of FileSystem::append_file opening O_WRONLY|O_CREATE|O_APPEND, so an \
      empty append leaves the file unchanged and the offset is the kernel's, not the caller's",
     "crates/tsr_vfs/src/lib.rs:86-88 has only the default body, which returns \
      Err(Error::Unsupported(\"immutable filesystem append\"))"),
    ("tsc/internal/vfs/osvfs/os.go:osFS.writeFileWithFlag",
     "a private fn write_file_with_flag(&self, path, content, flags) -> io::Result<()> that does \
      NOT create parents and does NOT assert rootedness, returning the raw OS error from both the \
      open and the write",
     "crates/tsr_vfs (absent). write_file and append_file (lib.rs:83, :86) are two separate \
      methods with no shared flag-parameterised layer beneath them"),
    ("tsc/internal/vfs/osvfs/os.go:osFS.ensureDirectoryExists",
     "a private fn ensure_directory_exists(&self, path) -> io::Result<()> that is create_dir_all \
      with mode 0o777 under the umask: idempotent, and leaving an existing directory's \
      permissions alone",
     "crates/tsr_vfs (absent). Nothing in crates/ creates a directory; the OS calls in \
      crates/tsr_vfs/src/os.rs are canonicalize, symlink_metadata, read_dir and read only"),
    ("tsc/internal/vfs/osvfs/os.go:osFS.writeFileEnsuringDir",
     "a private fn write_file_ensuring_dir(&self, path, content, flags) -> io::Result<()> that \
      asserts rootedness, ATTEMPTS THE WRITE FIRST, and only on failure creates the parent and \
      retries -- returning the directory error, not the first write's, when the creation fails",
     "crates/tsr_vfs (absent). There is no write path of any kind, so there is no \
      try-then-create-then-retry layering to compare"),
    ("tsc/internal/vfs/osvfs/os.go:osFS.Remove",
     "an OS implementor of FileSystem::remove that is remove_dir_all semantics WITH the missing \
      path treated as success, and that removes a symlink rather than its target",
     "crates/tsr_vfs/src/lib.rs:89-91 has only the default body, which returns \
      Err(Error::Unsupported(\"immutable filesystem remove\")). MemoryBuilder::remove \
      (lib.rs:158) mutates a builder, not a filesystem, and is not an implementor"),
    ("tsc/internal/vfs/osvfs/os.go:osFS.Chtimes",
     "an OS implementor taking BOTH stamps -- change_times(&self, path, atime, mtime) -- where a \
      zero value means leave that stamp alone and the call follows symlinks",
     "crates/tsr_vfs/src/lib.rs:92-94 declares `fn change_times(&self, _path: &[u8])` with NO \
      time parameters at all, so the signature cannot express the operation, and its only body \
      returns Err(Error::Unsupported(\"immutable filesystem timestamps\"))"),
    ("tsc/internal/vfs/osvfs/os.go:osFS.WalkDir",
     "a walk method on FileSystem taking a callback of (path, entry, error) with a skip-directory \
      and a skip-all sentinel, preorder and lexical within a directory, reporting an unreadable \
      directory twice and not following a symlink entry",
     "crates/tsr_vfs/src/lib.rs:70-95 declares no walk method, no callback type and no skip \
      sentinel; a repo-wide search for a walk API across crates/ finds none"),
    ("tsc/internal/vfs/osvfs/os.go:getLimitedWalkDirFunc",
     "the per-walk callback binding: a pooled wrapper acquired for one walk and bound to that \
      walk's callback, so concurrent and nested walks never share a binding",
     "crates/tsr_vfs (absent). With no walk API there is no callback wrapper to bind"),
    ("tsc/internal/vfs/osvfs/os.go:putLimitedWalkDirFunc",
     "the release half: clear the callback binding and return the wrapper to the pool, WITHOUT \
      clearing the wrapper's own forwarding closure",
     "crates/tsr_vfs (absent)"),
    ("tsc/internal/vfs/osvfs/os.go:limitedWalkDirFunc.walker",
     "the wrapper's forwarding call: acquire a blocking-operation permit, then pass path, entry \
      and error through to the bound callback unchanged and return its result",
     "crates/tsr_vfs (absent). No walk callback exists to forward anything, and no concurrency \
      limiter guards filesystem calls"),
    ("tsc/internal/vfs/osvfs/os.go:isReparsePoint",
     "the adapter's own predicate: translate separators, then ask the platform whether the path \
      is a symlink or reparse point, answering false on any error",
     "crates/tsr_vfs (absent). Nothing in crates/ classifies a path as a symlink AS AN OPERATION; \
      crates/tsr_vfs/src/os.rs:31 calls symlink_metadata inline inside ScopedOsFs::capture"),
    ("tsc/internal/nativepath/symlink_other.go:IsSymlinkOrReparsePoint",
     "the posix arm: symlink_metadata(path).map(|m| m.file_type().is_symlink()).unwrap_or(false) \
      -- total, never fallible, and TRUE for a dangling link",
     "crates/tsr_nativepath (absent; PORTS.toml:4027-4037 records the file as status = \
      \"planned\" with rust = [])"),
    ("tsc/internal/nativepath/realpath_other.go:Realpath",
     "the !windows && !linux arm: a per-component symlink evaluation that leaves a relative input \
      relative, does NOT correct case on a case-insensitive volume, and reports a dangling link as \
      an lstat failure. std::fs::canonicalize is NOT this function: it corrects case and it \
      absolutises",
     "crates/tsr_nativepath (absent; PORTS.toml:4001-4011 records the file as status = \
      \"planned\" with rust = [])"),
    ("tsc/internal/nativepath/realpath_linux.go:Realpath",
     "the linux arm: open(O_PATH|O_CLOEXEC) then readlink of /proc/self/fd/N with a doubling \
      buffer, falling back to per-component evaluation when procfs is absent, and reporting \
      failures as open/readlink rather than lstat",
     "crates/tsr_nativepath (absent; PORTS.toml:3988-3998 records the file as status = \
      \"planned\" with rust = [])"),
    ("tsc/internal/vfs/osvfs/os.go:GetGlobalTypingsCacheLocation",
     "pub fn global_typings_cache_location() -> JsString combining the host user cache directory \
      (falling back to the temporary directory), a platform subdirectory, and the compiler's \
      MAJOR.MINOR version, joined with forward slashes",
     "crates/tsr_compiler/src/checker_host.rs:357-360 is NOT a port of this: it implements the \
      CheckerHost method declared at crates/tsr_checker/src/host.rs:120 by returning \
      JsString::default(), on the stated basis that ProgramOptions has no global typings-cache \
      input. It is an empty answer, not a computed location, and no Rust code reads a user cache \
      directory"),
];

pub fn observe(request: &Value) -> Option<Outcome> {
    if !SUBJECTS.contains(&subject(request)) {
        return None;
    }
    let operation = request.get("operation").and_then(Value::as_str)?;
    let (authority, signature, home) = MISSING
        .iter()
        .find(|(name, _, _)| *name == operation)
        .copied()?;
    Some(Outcome::missing(authority, authority, signature, home))
}
