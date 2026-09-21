//! The `internal/tspath` group: path algebra, roots, separators, components,
//! comparison and extension handling.
//!
//! Seventeen of this group's eighty operations have a public entry point in
//! `tsr_tspath` (directly or through its re-exports of `tsr_core::path`), so
//! those cases drive the production code and compare real values. The rest
//! have no entry point this harness can reach -- several exist as private
//! helpers inside consumer crates, which is not the same thing -- and each of
//! those cases is a recorded gap naming the pinned authority, the signature
//! the port is expected to carry and the file that does not have it. Nothing
//! here emulates a missing operation to make a comparison run, and nothing
//! here reads an expected value.
//!
//! Byte payloads travel as hex in both directions: an argument is UTF-8 text
//! under its own key or hex under `<key>_hex`, exactly one of the two, and a
//! byte result is always hex. A missing argument is a harness failure rather
//! than a defaulted observation, because a row both sides could agree on
//! without executing anything would be worse than no row at all.

use serde_json::{json, Value};

use crate::api::{action_op, actions, ordered, subject, Outcome};

/// Operations with no entry point the harness can reach, keyed by the primary
/// operation of the case that drives them. Each names the pinned Go authority,
/// the signature a port would have to carry, and where it is not.
const MISSING: &[(&str, &str, &str, &str)] = &[
    ("tsc/internal/tspath/path.go:PathIsAbsolute",
     "tsc/internal/tspath/path.go:PathIsAbsolute, :IsRootedDiskPath, :IsUrl, :IsDiskPathRoot and \
      :IsDynamicFileName",
     "five predicates over the encoded root length, each with its own rule: path_is_absolute is \
      `encoded_root_length(p) != 0`, is_rooted_disk_path is `> 0`, is_url is `< 0`, \
      is_disk_path_root is `> 0 && root_length(p) == p.len()`, and is_dynamic_file_name is the \
      literal prefix test `p.starts_with(b\"^/\")`",
     "crates/tsr_tspath/src/lib.rs re-exports encoded_root_length and root_length and stops \
      there; no crate names any of the five, and the one inlined use \
      (crates/tsr_tspath/src/comparison.rs:116 `encoded_root_length(&to[0]) > 0`) is a private \
      expression inside relative_to_directory_or_url"),
    ("tsc/internal/tspath/path.go:IsVolumeCharacter",
     "tsc/internal/tspath/path.go:IsVolumeCharacter and :isAnyDirectorySeparator",
     "two byte predicates: is_volume_character(byte) -> bool over the two ASCII letter ranges, \
      and is_any_directory_separator(byte) -> bool accepting b'/' and b'\\\\' and nothing else. \
      Both take a BYTE and not a decoded character",
     "crates/tsr_core/src/path.rs:6 has `fn separator(byte: u8)`, private and unexported, whose \
      own callers bypass it (crates/tsr_tspath/src/lib.rs:52 tests only b\"/\"); the volume \
      predicate exists only as the inlined `first.is_ascii_alphabetic()` at \
      crates/tsr_core/src/path.rs:25"),
    ("tsc/internal/tspath/path.go:getFileUrlVolumeSeparatorEnd",
     "tsc/internal/tspath/path.go:getFileUrlVolumeSeparatorEnd",
     "fn file_url_volume_separator_end(url: &[u8], start: usize) -> Option<usize>, answering None \
      when start is at or past the end, Some(start + 1) for a literal ':' and Some(start + 3) \
      for `%3a` or `%3A` with the length guard `url.len() > start + 2`",
     "crates/tsr_core/src/path.rs:47-56 inlines the scan inside encoded_root_length and exposes \
      nothing; the harness cannot call it independently of root detection"),
    ("tsc/internal/tspath/path.go:SplitVolumePath",
     "tsc/internal/tspath/path.go:SplitVolumePath",
     "fn split_volume_path(path: &[u8]) -> (Vec<u8>, &[u8], bool) returning the LOWERCASED \
      two-byte volume, the untouched remainder and whether a volume was found; it accepts `c:d`, \
      which has no root length at all",
     "absent: no crate lowercases a two-byte drive prefix, and a repository-wide search for \
      split_volume across crates/, tools/ and xtask/ finds nothing"),
    ("tsc/internal/tspath/path.go:HasTrailingDirectorySeparator",
     "tsc/internal/tspath/path.go:HasTrailingDirectorySeparator, :RemoveTrailingDirectorySeparator, \
      :RemoveTrailingDirectorySeparators, :EnsureTrailingDirectorySeparator, \
      :Path.RemoveTrailingDirectorySeparator and :Path.EnsureTrailingDirectorySeparator",
     "the separator quartet plus its two Path-typed wrappers, every one of them testing BOTH \
      separators through isAnyDirectorySeparator: has_trailing_separator, the singular strip, the \
      plural strip that loops, and ensure, which appends b'/' only when the last byte is neither \
      separator",
     "absent as entry points. crates/tsr_tspath/src/lib.rs:71-73 pops at most one byte and tests \
      only b\"/\", inside absolute(); crates/tsr_compiler/src/checker_module_specifiers.rs:13 \
      `fn trailing` is private to that crate and appends after testing only b'/', so it turns \
      `a\\` into `a\\/`"),
    ("tsc/internal/tspath/path.go:GetPathComponents",
     "tsc/internal/tspath/path.go:GetPathComponents and :pathComponents",
     "the UNREDUCED splitter: get_path_components(path, cwd) -> Vec<Vec<u8>>, keeping `.` and \
      interior empty components, plus path_components(path, root_length) taking an explicit root \
      so the split can be driven on its own. Exactly one trailing empty component is removed",
     "absent. crates/tsr_tspath/src/comparison.rs:45 normalized_components is the REDUCED walk \
      and a different operation (it is the one the gated go_observations witness covers); no \
      crate exposes the unreduced split"),
    ("tsc/internal/tspath/path.go:reducePathComponents",
     "tsc/internal/tspath/path.go:reducePathComponents and \
      :getNormalizedPathComponentsFromCombined",
     "reduce_path_components(components: &[Vec<u8>]) -> Vec<Vec<u8>> over a caller's list, and \
      normalized_components_from_combined(path: &[u8]) -> Vec<Vec<u8>> over an already-combined \
      path. Both keep `..` when the root component is empty and drop it when it is not",
     "absent as callable operations: the reduction is fused into \
      crates/tsr_tspath/src/comparison.rs:45-65, which always splits a path itself and so cannot \
      be handed a component list no splitter produces"),
    ("tsc/internal/tspath/path.go:simpleNormalizePath",
     "tsc/internal/tspath/path.go:simpleNormalizePath and :hasRelativePathSegment",
     "simple_normalize_path(path: &[u8]) -> Option<Cow<'_, [u8]>>, declining rather than \
      answering when the cheap cleanup would change the meaning of the path, and \
      has_relative_path_segment(path: &[u8]) -> bool, which counts dots per segment and requires \
      a segment of exactly one or two",
     "absent as callable operations. The fast path is inlined into \
      crates/tsr_core/src/path.rs:99-121 inside normalize, and the guard exists only as the \
      private crates/tsr_core/src/path.rs:82 `fn has_relative_segment`, which is a different \
      algorithm (a window scan plus a split) and carries no port marker"),
    ("tsc/internal/tspath/path.go:trimRuneCount",
     "tsc/internal/tspath/path.go:trimRuneCount",
     "fn trim_rune_count(s: &[u8], rune_count: usize) -> &[u8], skipping up to that many decoded \
      runes and CLAMPING to the end rather than failing when there are fewer",
     "absent as a callable operation: the clamp is inlined into \
      crates/tsr_tspath/src/comparison.rs:34-42 inside trim_file_path_prefix, so no rune count \
      the prefix path does not itself produce can be driven"),
    ("tsc/internal/tspath/path.go:ComparePathsCaseSensitive",
     "tsc/internal/tspath/path.go:ComparePathsCaseSensitive, :ComparePathsCaseInsensitive, \
      :ComparePathsOptions.GetComparer and :ComparePathsOptions.getEqualityComparer",
     "the two named wrappers -- compare_paths_case_sensitive(a, b, cwd) and its insensitive twin \
      -- and the two selectors the options type returns: an ordering comparer and an equality \
      comparer, each handed the COMPLEMENT of use_case_sensitive_file_names, with the equality \
      one being Go's simple EqualFold rather than full Unicode folding",
     "absent as named entry points. crates/tsr_tspath/src/comparison.rs:142 compare_paths takes \
      the flag as a parameter and has no wrapper pair (and is the gated go_observations \
      witness's operation, not these); the selectors are inlined at each call site, for example \
      crates/tsr_tspath/src/comparison.rs:206-211"),
    ("tsc/internal/tspath/path.go:CompareNumberOfDirectorySeparators",
     "tsc/internal/tspath/path.go:CompareNumberOfDirectorySeparators",
     "fn compare_number_of_directory_separators(left: &[u8], right: &[u8]) -> Ordering, counting \
      b'/' only and normalising NOTHING first",
     "absent: no crate counts separators to order two paths, and no neighbouring helper can be \
      reused, because every one of them normalises slashes first"),
    ("tsc/internal/tspath/path.go:GetPathComponentsRelativeTo",
     "tsc/internal/tspath/path.go:GetPathComponentsRelativeTo",
     "fn path_components_relative_to(from, to, cwd, case_sensitive) -> Vec<Vec<u8>>, comparing \
      component 0 case-insensitively ALWAYS and later components by the requested comparer, and \
      returning the `to` components untouched when nothing is shared",
     "absent as an entry point: the walk exists only inside \
      crates/tsr_tspath/src/comparison.rs:101-129 relative_to_directory_or_url, which returns a \
      joined path and never the component list"),
    ("tsc/internal/tspath/path.go:ConvertToRelativePath",
     "tsc/internal/tspath/path.go:ConvertToRelativePath",
     "fn convert_to_relative_path(path, cwd, use_case_sensitive_file_names) -> Vec<u8>, which \
      converts only a ROOTED DISK path (encoded root length > 0) and returns everything else, \
      URLs included, byte for byte",
     "absent: no crate gates a relative conversion on the sign of the encoded root length"),
    ("tsc/internal/tspath/path.go:EnsurePathIsNonModuleName",
     "tsc/internal/tspath/path.go:EnsurePathIsNonModuleName and :IsExternalModuleNameRelative",
     "ensure_path_is_non_module_name(path) -> Cow<'_, [u8]>, prefixing `./` unless the path is \
      absolute (encoded root length != 0) or dot-relative, and is_external_module_name_relative \
      (name) -> bool, which is dot-relative OR rooted disk path (> 0) and therefore rejects URLs",
     "the first exists only inlined at crates/tsr_tspath/src/comparison.rs:134-138 inside \
      relative_from_file; the second is crates/tsr_parser/src/references.rs:563, private to \
      tsr_parser and unreachable from any other crate"),
    ("tsc/internal/tspath/path.go:ResolveTripleslashReference",
     "tsc/internal/tspath/path.go:ResolveTripleslashReference",
     "fn resolve_tripleslash_reference(module_name: &[u8], containing_file: &[u8]) -> Vec<u8>, \
      normalising a rooted disk path directly and otherwise combining it onto the containing \
      file's directory before normalising",
     "absent: crates/tsr_parser/src/references.rs collects triple-slash directives but resolves \
      none of them, and no crate composes directory + combine + normalize under this name"),
    ("tsc/internal/tspath/path.go:StartsWithDirectory",
     "tsc/internal/tspath/path.go:StartsWithDirectory",
     "fn starts_with_directory(file, directory, case_sensitive) -> bool, canonicalising both, \
      trimming ONE trailing b'/' and then ONE trailing b'\\\\', and accepting either separator \
      after the prefix",
     "crates/tsr_compiler/src/checker_module_specifiers.rs:31 has the same name but is private \
      to tsr_compiler, so no harness and no other crate can call it"),
    ("tsc/internal/tspath/ignoredpaths.go:ContainsIgnoredPath",
     "tsc/internal/tspath/ignoredpaths.go:ContainsIgnoredPath",
     "fn contains_ignored_path(path: &[u8]) -> bool, a raw substring search for the three \
      patterns `/node_modules/.`, `/.git` and `.#` with no normalisation and no case folding",
     "crates/tsr_compiler/src/checker_module_specifiers.rs:24-28 `fn ignored` carries the same \
      three patterns but is private to tsr_compiler and has no public entry point in any crate"),
    ("tsc/internal/tspath/path.go:Path.ContainsPath",
     "tsc/internal/tspath/path.go:Path.ContainsPath",
     "the Path-TYPED containment test: a plain byte prefix check with a separator boundary, over \
      a Path newtype that is already rooted, reduced and case-folded, with no current directory \
      and no comparer",
     "absent: there is no Path newtype at all -- crates/tsr_tspath/src/lib.rs:112 to_path returns \
      a bare JsString -- so the typed adapter has nowhere to live, and \
      crates/tsr_tspath/src/comparison.rs:194 contains_path is the free function, a different \
      operation with a different answer"),
    ("tsc/internal/tspath/path.go:Path.GetDirectoryPath",
     "tsc/internal/tspath/path.go:Path.GetDirectoryPath",
     "the Path-TYPED directory accessor: the free operation applied to a Path and returning a \
      Path, adding no canonicalisation of its own",
     "absent for want of the Path newtype; crates/tsr_tspath/src/lib.rs:46 directory is the free \
      form and is compared in its own case"),
    ("tsc/internal/tspath/path.go:ForEachAncestorDirectoryStoppingAtGlobalCache",
     "tsc/internal/tspath/path.go:ForEachAncestorDirectoryStoppingAtGlobalCache and the \
      early-stop half of :ForEachAncestorDirectory",
     "a callback-driven walk: for_each_ancestor_directory(directory, callback) -> (T, bool) where \
      the callback answers (value, stop), the walk returns that value with true when it stops \
      and the ZERO value with false when it runs out; and the wrapper that also stops at a \
      global cache location, AFTER calling the callback there, returning only T",
     "crates/tsr_tspath/src/lib.rs:147 `ancestors` returns the visited sequence and nothing else: \
      no callback, no stop flag, no carried value and no terminating location. Its sequence is \
      compared in filesystem/tspath/ancestor-walk-sequence; this half has no entry point"),
    ("tsc/internal/tspath/path.go:ForEachAncestorDirectoryPath",
     "tsc/internal/tspath/path.go:ForEachAncestorDirectoryPath",
     "the Path-typed ancestor walk, which converts the Path to a string, walks, and hands each \
      ancestor back as a Path WITHOUT re-canonicalising it, so the separators of a non-canonical \
      input change mid-walk",
     "absent for want of both the Path newtype and the callback-shaped walk; \
      crates/tsr_tspath/src/lib.rs:147 ancestors is string-typed and unstoppable"),
    ("tsc/internal/tspath/extension.go:ExtensionIsTs",
     "tsc/internal/tspath/extension.go:ExtensionIsTs and :ExtensionIsOneOf",
     "two predicates over an EXTENSION rather than a path: extension_is_ts, which accepts the \
      seven literal extensions plus any `.d.<x>.ts` of at least seven bytes, and \
      extension_is_one_of, which is whole-string equality against a caller's list",
     "absent. crates/tsr_checker/src/external_resolution.rs:680 has the TS_EXTENSIONS list but no \
      predicate over an extension, and the only membership helpers in the tree are suffix tests \
      over a PATH, which is the other operation"),
    ("tsc/internal/tspath/path.go:FileExtensionIs",
     "tsc/internal/tspath/path.go:FileExtensionIs and \
      tsc/internal/tspath/extension.go:FileExtensionIsOneOf",
     "file_extension_is(path, extension) -> bool: a raw suffix test with the STRICT guard \
      `path.len() > extension.len()` and no dot boundary, plus the list form that loops over it \
      and answers false for an empty or absent list",
     "absent as named entry points. The faithful predicate exists only inlined in the private \
      crates/tsr_checker/src/module_specifiers_paths.rs:14-18, and the tree's other reading, \
      crates/tsr_checker/src/external_resolution.rs:1080, is a bare ends_with that loses the \
      strict guard"),
    ("tsc/internal/tspath/extension.go:HasTSFileExtension",
     "tsc/internal/tspath/extension.go:HasTSFileExtension, :HasJSFileExtension, \
      :HasJSONFileExtension and :HasImplementationTSFileExtension",
     "four list-membership predicates over a path, each FileExtensionIs against its own list, \
      with the implementation one additionally requiring NOT is_declaration_file_name so that \
      an arbitrary `.d.<x>.ts` is rejected",
     "absent as entry points: crates/tsr_checker/src/module_specifiers_packages.rs:114 \
      `fn implementation_ts` and crates/tsr_checker/src/external_resolution.rs:1080 \
      `fn has_ts_file_extension` are private to tsr_checker, and no crate names the JS or JSON \
      predicate at all"),
    ("tsc/internal/tspath/path.go:GetAnyExtensionFromPath",
     "tsc/internal/tspath/path.go:GetAnyExtensionFromPath, :GetLongestExtensionFromPath, \
      :getAnyExtensionFromPathWorker and :tryGetExtensionFromPath",
     "the four selectors that take a caller's extension list and a case rule: the FIRST-match \
      worker, the public wrapper that falls back to the base name when the list is empty, the \
      LONGEST-match selector, and the single-extension helper that requires the byte before the \
      extension to be a '.' and returns the PATH's bytes rather than the list's",
     "absent. crates/tsr_checker/src/external_resolution.rs:1087 `fn any_extension` is private, \
      takes no list and implements only the base-name arm; nothing in the workspace selects the \
      longest matching extension"),
    ("tsc/internal/tspath/extension.go:TryGetExtensionFromPath",
     "tsc/internal/tspath/extension.go:TryGetExtensionFromPath, :TryExtractTSExtension, \
      :GetDeclarationFileExtension, :GetDeclarationEmitExtensionForPath and \
      :GetPossibleOriginalInputExtensionForExtension",
     "five table-driven extractors: the flat removal list, the TypeScript-only list, the \
      declaration extractor that tries the three fixed suffixes BEFORE the first `.d.` index of \
      the base name, the emit extension built from the switch plus a `.d<ext>.ts` default, and \
      the ordered candidate list, whose ORDER is part of the contract",
     "absent. crates/tsr_checker/src/external_resolution.rs:963 try_get_extension_from_path is \
      private and answers Some for a path equal to its own extension; \
      crates/tsr_core/src/path.rs:159 is_declaration_file_name answers a bool and never the \
      extension; nothing returns a candidate list"),
    ("tsc/internal/tspath/extension.go:ChangeExtension",
     "tsc/internal/tspath/extension.go:ChangeExtension, :ChangeAnyExtension and \
      :ChangeFullExtension",
     "the three rewrites, each prepending the '.' when the new extension lacks one, slicing the \
      matched extension off the UNTRIMMED path, and deleting rather than truncating when the new \
      extension is empty; the full form replaces a declaration extension from its `.d` onwards",
     "crates/tsr_checker/src/module_specifiers_packages.rs:117 and :126 carry the first and the \
      third under the same names, private to tsr_checker and reachable from nowhere else; the \
      list-taking form has no counterpart at all"),
    ("tsc/internal/tspath/extension.go:RemoveExtension",
     "tsc/internal/tspath/extension.go:RemoveExtension and :RemoveAnyFileExtension",
     "the UNCHECKED slice remove_extension(path, extension), which does not verify that the \
      extension is a suffix and panics when it is longer than the path, and the two-stage \
      remove_any_file_extension built on it",
     "absent. crates/tsr_core/src/path.rs:187 remove_file_extension is the FIRST stage only and \
      chooses the extension itself; no crate exposes an extension-taking remover, and none \
      reproduces the fallback that measures on the trimmed path and slices the untrimmed one"),
    ("tsc/internal/tspath/path.go:GetCommonParents",
     "tsc/internal/tspath/path.go:GetCommonParents and :getCommonParentsWorker",
     "the minimal covering set of parent directories: get_common_parents(paths, min_components, \
      get_path_components, options) -> (Vec<Vec<u8>>, BTreeSet<Vec<u8>>) which PANICS when \
      min_components < 1 before it looks at the paths, and the recursive worker that fans out on \
      a divergence shallower than min_components, grouping by the canonicalised Path of the \
      diverging component and emitting the groups in SORTED key order",
     "absent: no crate computes a minimal covering set of parent directories, and no crate has \
      the recursive fan-out or its ordering rule"),
    ("tsc/internal/tspath/path.go:GetNormalizedAbsolutePathWithoutRoot",
     "tsc/internal/tspath/path.go:GetNormalizedAbsolutePathWithoutRoot",
     "fn normalized_absolute_path_without_root(file: &[u8], cwd: &[u8]) -> &[u8], the normalised \
      absolute path with GetRootLength (the COMPLEMENT FOLDED AWAY, so a URL loses its whole \
      scheme and authority) sliced off the front",
     "absent: the composition would be `&absolute(file, cwd)[root_length(..)..]` but no crate \
      performs it, and the operation is not a re-export of either half"),
];

pub fn observe(request: &Value) -> Option<Outcome> {
    if subject(request) != "tspath" {
        return None;
    }
    let operation = request
        .get("operation")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if let Some((_, authority, signature, home)) =
        MISSING.iter().find(|(name, ..)| *name == operation)
    {
        return Some(Outcome::missing(authority, signature, home));
    }
    Some(match replay(actions(request)) {
        Ok(rows) => Outcome::Observed(ordered(rows)),
        Err(problem) => Outcome::Failed(problem),
    })
}

fn replay(trace: &[Value]) -> Result<Vec<Value>, String> {
    trace.iter().map(row).collect()
}

fn row(action: &Value) -> Result<Value, String> {
    let op = action_op(action);
    let observed = match op {
        "encoded_root_length" => {
            let path = bytes(action, "path")?;
            json!({
                "op": op,
                "encoded": tsr_tspath::encoded_root_length(&path) as i64,
                "root": tsr_tspath::root_length(&path) as i64,
            })
        }
        "normalize_slashes" => text(op, &tsr_tspath::normalize_slashes(&bytes(action, "path")?)),
        "combine" => {
            let first = bytes(action, "first")?;
            let paths = list(action, "paths")?;
            text(op, &tsr_tspath::combine(&first, &borrow(&paths)))
        }
        "directory_path" => text(op, &tsr_tspath::directory(&bytes(action, "path")?)),
        "normalize_path" => text(op, &tsr_tspath::normalize(&bytes(action, "path")?)),
        "resolve_path" => {
            let path = bytes(action, "path")?;
            let paths = list(action, "paths")?;
            text(op, &tsr_tspath::resolve(&path, &borrow(&paths)))
        }
        "normalized_absolute_path" => {
            let (path, cwd) = (bytes(action, "path")?, bytes(action, "cwd")?);
            text(op, &tsr_tspath::absolute(&path, &cwd))
        }
        "to_file_name_lower_case" => text(
            op,
            &tsr_tspath::file_name_lower_case(&bytes(action, "path")?),
        ),
        "canonical_file_name" => {
            let path = bytes(action, "path")?;
            let sensitive = flag(action, "use_case_sensitive_file_names")?;
            text(op, &tsr_tspath::canonical(&path, sensitive))
        }
        "to_path" => {
            let (file, base) = (bytes(action, "file")?, bytes(action, "base")?);
            let sensitive = flag(action, "case_sensitive")?;
            text(op, tsr_tspath::to_path(&file, &base, sensitive).as_bytes())
        }
        "base_file_name" => text(op, tsr_tspath::base_name(&bytes(action, "path")?)),
        "has_extension" => {
            json!({ "op": op, "result": tsr_tspath::has_extension(&bytes(action, "path")?) })
        }
        "is_declaration_file_name" => {
            let path = bytes(action, "path")?;
            json!({ "op": op, "result": tsr_tspath::is_declaration_file_name(&path) })
        }
        "remove_file_extension" => text(
            op,
            tsr_tspath::remove_file_extension(&bytes(action, "path")?),
        ),
        "relative_to_directory_or_url" => {
            let (from, to) = (bytes(action, "from")?, bytes(action, "to")?);
            let cwd = bytes(action, "cwd")?;
            let as_url = flag(action, "absolute_path_as_url")?;
            let sensitive = flag(action, "case_sensitive")?;
            text(
                op,
                &tsr_tspath::relative_to_directory_or_url(&from, &to, as_url, &cwd, sensitive),
            )
        }
        "ancestor_walk" => {
            let visited: Vec<Value> = tsr_tspath::ancestors(&bytes(action, "path")?)
                .iter()
                .map(|directory| Value::String(hex(directory)))
                .collect();
            json!({ "op": op, "visited": visited })
        }
        _ => {
            return Err(format!(
                "unsupported action {op:?}: an unknown action is a harness failure, never an \
                 observation"
            ))
        }
    };
    Ok(observed)
}

/// One byte result, hex encoded under the same key both sides use.
fn text(op: &str, value: &[u8]) -> Value {
    json!({ "op": op, "result": hex(value) })
}

fn borrow(paths: &[Vec<u8>]) -> Vec<&[u8]> {
    paths.iter().map(Vec::as_slice).collect()
}

/// A byte argument: UTF-8 text under `key`, or hex under `key_hex`. Exactly
/// one of the two must be present, because a defaulted argument would produce
/// a row that ran nothing.
fn bytes(action: &Value, key: &str) -> Result<Vec<u8>, String> {
    let plain = action.get(key);
    let encoded = action.get(format!("{key}_hex"));
    match (plain, encoded) {
        (Some(_), Some(_)) | (None, None) => Err(format!(
            "an action must carry exactly one of {key:?} and {key}_hex"
        )),
        (Some(value), None) => value
            .as_str()
            .map(|value| value.as_bytes().to_vec())
            .ok_or_else(|| format!("action key {key:?} is not a string")),
        (None, Some(value)) => {
            let encoded = value
                .as_str()
                .ok_or_else(|| format!("action key {key}_hex is not a string"))?;
            unhex(encoded).ok_or_else(|| format!("action key {key}_hex is malformed: {encoded:?}"))
        }
    }
}

fn flag(action: &Value, key: &str) -> Result<bool, String> {
    action
        .get(key)
        .and_then(Value::as_bool)
        .ok_or_else(|| format!("action key {key:?} is missing or is not a boolean"))
}

fn list(action: &Value, key: &str) -> Result<Vec<Vec<u8>>, String> {
    let items = action
        .get(key)
        .ok_or_else(|| format!("action key {key:?} is missing"))?;
    if items.is_null() {
        return Ok(Vec::new());
    }
    items
        .as_array()
        .ok_or_else(|| format!("action key {key:?} is not an array"))?
        .iter()
        .map(|item| {
            item.as_str()
                .map(|item| item.as_bytes().to_vec())
                .ok_or_else(|| format!("action key {key:?} holds a non-string entry"))
        })
        .collect()
}

fn hex(value: &[u8]) -> String {
    use std::fmt::Write;
    let mut out = String::with_capacity(value.len() * 2);
    for byte in value {
        write!(out, "{byte:02x}").expect("writing to a String is infallible");
    }
    out
}

fn unhex(value: &str) -> Option<Vec<u8>> {
    if !value.len().is_multiple_of(2) {
        return None;
    }
    value
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let pair = std::str::from_utf8(pair).ok()?;
            u8::from_str_radix(pair, 16).ok()
        })
        .collect()
}
