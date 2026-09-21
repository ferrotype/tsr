//! The `internal/symlinks` known-symlink cache group.
//!
//! The pinned subject is `KnownSymlinks`
//! (upstream/tsc/internal/symlinks/knownsymlinks.go): four `SyncMap` indexes
//! -- a forward directory map to `*KnownDirectoryLink`, a reverse map from
//! realpath to a set of symlink strings, and the same pair for files -- plus
//! the cwd and case rule captured once by `NewKnownSymlink` (:74-79). What the
//! group is about is the bookkeeping: which index a single call writes, that
//! both reverse indexes are extended only on a symlink key's FIRST insertion
//! while the forward maps are overwritten every time (:55-72), and what the
//! walk in `guessDirectorySymlink` (:114-130) consumes before it stops.
//!
//! Every case here is a recorded gap. The scope's by-name rule put
//! `ProcessResolution` at `implemented_untested` and the other eleven
//! operations at `missing`; reading the tree says the whole surface is missing
//! as an entry point, and `ProcessResolution` is missing a half.
//!
//! * `PORTS.toml:5339-5350` maps `tsc/internal/symlinks/knownsymlinks.go` to
//!   `crate = "tsr_core"`, `status = "planned"`, `rust = []`, `verify = []`.
//!   There is no `symlinks` module in `crates/tsr_core`.
//! * The only Rust code in the workspace that does this work is a private
//!   re-inlining inside the checker host:
//!   `crates/tsr_compiler/src/checker_module_specifiers.rs:10-13` declares
//!   `pub(crate) struct KnownSymlinks { directories: BTreeSet<JsString>,
//!   by_realpath: BTreeMap<JsString, BTreeSet<JsString>> }`. It is
//!   `pub(crate)`, so nothing outside `tsr_compiler` can construct or observe
//!   it, and `phase1_filesystem` does not and cannot depend on it as a callable
//!   entry point.
//!
//! Four structural differences survive even once an entry point exists, and
//! they are named in the records below because they are what a port has to
//! decide rather than incidental:
//!
//! * There is no link VALUE. `directories` is a `BTreeSet<JsString>`, so
//!   neither `KnownDirectoryLink.Real` (the realpath's original casing) nor
//!   `.RealPath` (its canonical form) is stored, and the pin's nil link --
//!   present, reverse-index-blocking, and `HasDirectory`-true -- has no Rust
//!   spelling at all.
//! * There is no file half. The struct has no `files` or `files_by_realpath`
//!   member, and `process`
//!   (crates/tsr_compiler/src/checker_module_specifiers.rs:43-82) never records
//!   one: it returns at :65-67 whenever the walk consumed no component, where
//!   the pin has already called `SetFile` unconditionally
//!   (knownsymlinks.go:97). That is a live divergence, not only an absence.
//! * Configuration is per call, not per cache. `process` takes `cwd` and
//!   `case_sensitive` as arguments (:43) and `has_directory` takes them again
//!   (:83); the pin captures both in the constructor and every operation reads
//!   them off the cache.
//! * Nothing is an accessor. `guessDirectorySymlink` and
//!   `isNodeModulesOrScopedPackageDirectory` are inlined into the walk and the
//!   closure `is_package` (:49-52, :54-64) rather than named, and the four
//!   index accessors do not exist, so the pin's contract that each hands back
//!   the cache's own storage has nothing to be true or false of.
//!
//! Preparation records that. It never emulates the cache here to make a
//! comparison run, and it never reads an expected result: a hand-written
//! `KnownSymlinks` in this file would answer every case correctly and would
//! prove nothing about the port.

use crate::api::Outcome;
use serde_json::Value;

/// The production home every record points at. `tsr_core` is where the ledger
/// says this file lands; the private partial is named too, because a port that
/// grows the entry point has to reconcile with it rather than beside it.
const HOME: &str = "crates/tsr_core/src/symlinks.rs (absent). PORTS.toml:5339-5350 maps \
                    tsc/internal/symlinks/knownsymlinks.go to crate = \"tsr_core\", \
                    status = \"planned\", rust = [], and crates/tsr_core has no symlinks module. \
                    The only implementation in the workspace is the private re-inlining at \
                    crates/tsr_compiler/src/checker_module_specifiers.rs:10-88, a pub(crate) \
                    struct with two members and no accessors";

/// One record per case, because each case reaches a different part of the
/// missing surface and the gap it needs named is different. Each row names the
/// pinned Go authority, the signature the port is expected to carry and the
/// file that does not have it yet.
const MISSING: &[(&str, &str, &str, &str)] = &[
    ("filesystem/symlinks/the-constructor-captures-cwd-and-case-sensitivity-for-the-cache-s-lifetime",
     "tsc/internal/symlinks/knownsymlinks.go:NewKnownSymlink",
     "tsc/internal/symlinks/knownsymlinks.go:NewKnownSymlink (:74-79), whose captured cwd and \
      useCaseSensitiveFileNames (:27-28) are read by ProcessResolution (:97, :98, :100, :107) and \
      SetFile (:67)",
     "pub fn new(current_directory: &[u8], use_case_sensitive_file_names: bool) -> KnownSymlinks \
      with four empty indexes, storing both values for the cache's lifetime so that every later \
      operation reads them off the cache and no operation takes them again. \
      crates/tsr_compiler/src/checker_module_specifiers.rs:96 builds its cache with \
      KnownSymlinks::default() and passes cwd and the case flag per call instead (:43, :83), so \
      two caches cannot disagree about one input -- which is the whole observation here"),
    ("filesystem/symlinks/has-directory-appends-the-separator-and-set-directory-does-not",
     "tsc/internal/symlinks/knownsymlinks.go:KnownSymlinks.HasDirectory",
     "tsc/internal/symlinks/knownsymlinks.go:KnownSymlinks.HasDirectory (:31-34) beside \
      KnownSymlinks.SetDirectory (:55-63)",
     "pub fn has_directory(&self, symlink_path: &Path) -> bool taking an ALREADY canonical path, \
      appending the trailing directory separator itself, and testing key presence only. \
      crates/tsr_compiler/src/checker_module_specifiers.rs:83-87 is \
      fn has_directory(&self, directory: &[u8], cwd: &[u8], case_sensitive: bool) -> bool: it is \
      pub(crate), and it calls to_path itself (:84) where the pin canonicalises nothing, so a \
      caller handing it a raw relative path gets an answer the pin would not give"),
    ("filesystem/symlinks/the-reverse-directory-index-is-written-once-and-never-corrected",
     "tsc/internal/symlinks/knownsymlinks.go:KnownSymlinks.SetDirectory",
     "tsc/internal/symlinks/knownsymlinks.go:KnownSymlinks.SetDirectory (:55-63) and \
      KnownSymlinks.DirectoriesByRealpath (:41-43)",
     "pub fn set_directory(&self, symlink: &[u8], symlink_path: &Path, \
      real_directory: Option<&KnownDirectoryLink>) overwriting the forward entry on every call \
      while extending the reverse index only when the symlink key was absent, over a forward map \
      whose value is a KnownDirectoryLink { real: JsString, real_path: Path }. \
      crates/tsr_compiler/src/checker_module_specifiers.rs:75-81 has the same first-insertion \
      rule, but `directories` is a BTreeSet<JsString> (:11) with no value, so there is no forward \
      entry to overwrite and the divergence this case pins cannot arise"),
    ("filesystem/symlinks/a-nil-link-is-present-and-blocks-the-reverse-index-forever",
     "tsc/internal/symlinks/knownsymlinks.go:KnownSymlinks.SetDirectory",
     "tsc/internal/symlinks/knownsymlinks.go:KnownSymlinks.SetDirectory's nil branch (:56-62), \
      read back through KnownSymlinks.HasDirectory (:31-34) and KnownSymlinks.Directories (:37-39)",
     "the same set_directory over a forward map that can hold an ABSENT link -- \
      SyncMap<Path, Option<KnownDirectoryLink>>, not SyncMap<Path, KnownDirectoryLink> -- so that \
      a nil store makes the key present, has_directory answers true, the reverse index is not \
      touched, and a real link arriving later is still barred from the reverse index by the \
      key-presence guard. crates/tsr_compiler/src/checker_module_specifiers.rs:11 stores no value \
      at all, so absence is unrepresentable and the poisoned-key state has no Rust expression"),
    ("filesystem/symlinks/set-file-canonicalises-only-the-reverse-key",
     "tsc/internal/symlinks/knownsymlinks.go:KnownSymlinks.SetFile",
     "tsc/internal/symlinks/knownsymlinks.go:KnownSymlinks.SetFile (:65-72), \
      KnownSymlinks.Files (:46-48) and KnownSymlinks.FilesByRealpath (:51-53)",
     "pub fn set_file(&self, symlink: &[u8], symlink_path: &Path, realpath: &[u8]) storing the \
      realpath VERBATIM in the forward map while keying the reverse index by \
      to_path(realpath, self.cwd, self.use_case_sensitive_file_names) and holding the symlink \
      string verbatim, extending the reverse index only when the symlink key was absent; plus \
      files(&self) and files_by_realpath(&self) accessors. \
      crates/tsr_compiler/src/checker_module_specifiers.rs:10-13 has no file index of any kind \
      and nothing in crates/ records one"),
    ("filesystem/symlinks/process-resolution-records-the-file-even-when-it-guesses-no-directory",
     "tsc/internal/symlinks/knownsymlinks.go:KnownSymlinks.ProcessResolution",
     "tsc/internal/symlinks/knownsymlinks.go:KnownSymlinks.ProcessResolution (:93-112), whose \
      SetFile call at :97 precedes the guess at :98 and is not conditional on it",
     "pub fn process_resolution(&self, original_path: &[u8], resolved_file_name: &[u8]) that \
      returns early only when either argument is empty, then records the file, and only then \
      guesses a directory. crates/tsr_compiler/src/checker_module_specifiers.rs:43-82 is the \
      nearest code and it diverges here: it computes the walk first and returns at :65-67 when no \
      component was consumed, so the file the pin records in exactly that situation is never \
      recorded. It is also pub(crate) and takes cwd and the case flag as arguments"),
    ("filesystem/symlinks/the-common-ancestor-walk-stops-at-the-package-boundary",
     "tsc/internal/symlinks/knownsymlinks.go:KnownSymlinks.guessDirectorySymlink",
     "tsc/internal/symlinks/knownsymlinks.go:KnownSymlinks.guessDirectorySymlink (:114-130)",
     "fn guess_directory_symlink(&self, a: &[u8], b: &[u8], cwd: &[u8]) -> (JsString, JsString) \
      returning the two common ancestors, or two EMPTY strings when the walk consumed nothing, \
      and resolving both inputs against its own cwd argument rather than the cache's. \
      crates/tsr_compiler/src/checker_module_specifiers.rs:54-64 inlines the same walk inside \
      `process` and keeps only a bool (:53, :63), so nothing in the tree can answer the two paths \
      this operation returns"),
    ("filesystem/symlinks/the-package-boundary-test-folds-node-modules-but-not-the-scope-sigil",
     "tsc/internal/symlinks/knownsymlinks.go:KnownSymlinks.isNodeModulesOrScopedPackageDirectory",
     "tsc/internal/symlinks/knownsymlinks.go:KnownSymlinks.isNodeModulesOrScopedPackageDirectory \
      (:132-134)",
     "fn is_node_modules_or_scoped_package_directory(&self, s: &[u8]) -> bool, total, false on an \
      empty name, comparing against node_modules through the cache's case rule and testing the @ \
      sigil as a raw prefix. crates/tsr_compiler/src/checker_module_specifiers.rs:49-52 is the \
      closure `is_package` inside `process`, with the same two tests and the same asymmetry, but \
      it is a local binding rather than a callable member and it reads a per-call case flag"),
    ("filesystem/symlinks/resolutions-are-drained-modules-first-with-a-nil-source-file",
     "tsc/internal/symlinks/knownsymlinks.go:KnownSymlinks.SetSymlinksFromResolutions",
     "tsc/internal/symlinks/knownsymlinks.go:KnownSymlinks.SetSymlinksFromResolutions (:81-91)",
     "pub fn set_symlinks_from_resolutions(&self, for_each_resolved_module: &dyn Fn(&mut dyn \
      FnMut(&ResolvedModule, &[u8], ResolutionMode, &Path), Option<&SourceFile>), \
      for_each_resolved_type_reference_directive: &dyn Fn(&mut dyn \
      FnMut(&ResolvedTypeReferenceDirective, &[u8], ResolutionMode, &Path), Option<&SourceFile>)) \
      driving the module source first and the type-reference source second, entering each exactly \
      once and passing None for the source file. The nearest Rust is \
      crates/tsr_compiler/src/checker_module_specifiers.rs:92-107, which is marked a port of \
      program.go:Program.GetSymlinkCache rather than of this operation (:91): it walks \
      program.resolutions() then program.type_resolutions() in the same order, but it takes no \
      callbacks at all, so the seam this operation exists to expose is not present"),
    ("filesystem/symlinks/the-ignored-path-test-runs-on-the-canonical-path-and-spares-the-file-half",
     "tsc/internal/symlinks/knownsymlinks.go:KnownSymlinks.ProcessResolution",
     "tsc/internal/symlinks/knownsymlinks.go:KnownSymlinks.ProcessResolution's ignored-path guard \
      (:100-101), which tests tspath.ContainsIgnoredPath against the CANONICALISED symlink path \
      and guards only the SetDirectory call",
     "the same process_resolution, testing contains_ignored_path on \
      to_path(common_original, self.cwd, self.use_case_sensitive_file_names) -- so the answer \
      depends on the cache's case rule -- and skipping only the directory record, never the file \
      record made earlier. crates/tsr_compiler/src/checker_module_specifiers.rs:70-73 applies its \
      local `ignored` (:24-28, a re-inlining of ContainsIgnoredPath with the same three patterns) \
      to the same canonical key, but it reaches that line only after the early return at :65-67 \
      and it has no file record to spare"),
    ("filesystem/symlinks/the-four-accessors-hand-back-the-cache-s-own-storage",
     "tsc/internal/symlinks/knownsymlinks.go:KnownSymlinks.Directories",
     "tsc/internal/symlinks/knownsymlinks.go:KnownSymlinks.Directories (:37-39), \
      KnownSymlinks.DirectoriesByRealpath (:41-43), KnownSymlinks.Files (:46-48) and \
      KnownSymlinks.FilesByRealpath (:51-53), each of which returns &cache.<field>",
     "four accessors handing back the cache's own storage rather than a snapshot or a clone, so \
      that a write made through a returned handle is visible to has_directory and to \
      set_directory's own first-insertion guard: directories(&self) -> \
      &SyncMap<Path, Option<KnownDirectoryLink>>, directories_by_realpath(&self) -> \
      &SyncMap<Path, SyncSet<JsString>>, files(&self) -> &SyncMap<Path, JsString> and \
      files_by_realpath(&self) -> &SyncMap<Path, SyncSet<JsString>>. \
      crates/tsr_compiler/src/checker_module_specifiers.rs:10-13 has two private fields and no \
      accessor: by_realpath is reached only by its owning impl (:213-217), and two of the four \
      indexes do not exist"),
];

pub fn observe(request: &Value) -> Option<Outcome> {
    if crate::api::subject(request) != "symlinks.KnownSymlinks" {
        return None;
    }
    let case = request.get("case").and_then(Value::as_str).unwrap_or("");
    let Some((_, identity, authority, signature)) = MISSING.iter().find(|(id, ..)| *id == case)
    else {
        // The request schedule and this module disagree. That is a harness
        // failure, not an observation: a generic gap record here would let a
        // case nobody described still report a tidy result.
        return Some(Outcome::Failed(format!(
            "case {case:?} declares subject symlinks.KnownSymlinks but the symlinks group has no \
             record of the gap it needs named"
        )));
    };
    Some(Outcome::missing(*identity, authority, signature, HOME))
}
