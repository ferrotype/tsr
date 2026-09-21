//! Extension tables and the predicates, extractors and rewrites over them.
use crate::{
    base_name, is_declaration_file_name, remove_file_extension, remove_trailing_directory_separator,
};
use tsr_jsstring::compare::equality_comparer;

pub const EXTENSION_TS: &[u8] = b".ts";
pub const EXTENSION_TSX: &[u8] = b".tsx";
pub const EXTENSION_DTS: &[u8] = b".d.ts";
pub const EXTENSION_JS: &[u8] = b".js";
pub const EXTENSION_JSX: &[u8] = b".jsx";
pub const EXTENSION_JSON: &[u8] = b".json";
pub const EXTENSION_TS_BUILD_INFO: &[u8] = b".tsbuildinfo";
pub const EXTENSION_MJS: &[u8] = b".mjs";
pub const EXTENSION_MTS: &[u8] = b".mts";
pub const EXTENSION_DMTS: &[u8] = b".d.mts";
pub const EXTENSION_CJS: &[u8] = b".cjs";
pub const EXTENSION_CTS: &[u8] = b".cts";
pub const EXTENSION_DCTS: &[u8] = b".d.cts";

pub const SUPPORTED_DECLARATION_EXTENSIONS: &[&[u8]] =
    &[EXTENSION_DTS, EXTENSION_DCTS, EXTENSION_DMTS];
pub const SUPPORTED_TS_IMPLEMENTATION_EXTENSIONS: &[&[u8]] =
    &[EXTENSION_TS, EXTENSION_TSX, EXTENSION_MTS, EXTENSION_CTS];
const SUPPORTED_TS_EXTENSIONS_FOR_EXTRACT: &[&[u8]] = &[
    EXTENSION_DTS,
    EXTENSION_DCTS,
    EXTENSION_DMTS,
    EXTENSION_TS,
    EXTENSION_TSX,
    EXTENSION_MTS,
    EXTENSION_CTS,
];
pub const SUPPORTED_TS_EXTENSIONS_FLAT: &[&[u8]] = &[
    EXTENSION_TS,
    EXTENSION_TSX,
    EXTENSION_DTS,
    EXTENSION_CTS,
    EXTENSION_DCTS,
    EXTENSION_MTS,
    EXTENSION_DMTS,
];
pub const SUPPORTED_JS_EXTENSIONS_FLAT: &[&[u8]] =
    &[EXTENSION_JS, EXTENSION_JSX, EXTENSION_MJS, EXTENSION_CJS];
/// The order is part of the contract: the first matching suffix wins.
pub const EXTENSIONS_TO_REMOVE: &[&[u8]] = &[
    EXTENSION_DTS,
    EXTENSION_DMTS,
    EXTENSION_DCTS,
    EXTENSION_MJS,
    EXTENSION_MTS,
    EXTENSION_CJS,
    EXTENSION_CTS,
    EXTENSION_TS,
    EXTENSION_JS,
    EXTENSION_TSX,
    EXTENSION_JSX,
    EXTENSION_JSON,
];

/// A predicate over an extension, not a path.
/// port: tsc/internal/tspath/extension.go:ExtensionIsTs
pub fn extension_is_ts(ext: &[u8]) -> bool {
    SUPPORTED_TS_EXTENSIONS_FLAT.contains(&ext)
        || ext.len() >= 7 && ext.starts_with(b".d.") && ext.ends_with(b".ts")
}
/// Whole-string equality against the caller's list.
/// port: tsc/internal/tspath/extension.go:ExtensionIsOneOf
pub fn extension_is_one_of<T: AsRef<[u8]>>(ext: &[u8], extensions: &[T]) -> bool {
    extensions.iter().any(|candidate| candidate.as_ref() == ext)
}
/// A raw suffix test with a strict length guard and no dot boundary.
/// port: tsc/internal/tspath/path.go:FileExtensionIs
pub fn file_extension_is(path: &[u8], extension: &[u8]) -> bool {
    path.len() > extension.len() && path.ends_with(extension)
}
/// port: tsc/internal/tspath/extension.go:FileExtensionIsOneOf
pub fn file_extension_is_one_of<T: AsRef<[u8]>>(path: &[u8], extensions: &[T]) -> bool {
    extensions
        .iter()
        .any(|extension| file_extension_is(path, extension.as_ref()))
}
/// port: tsc/internal/tspath/extension.go:HasTSFileExtension
pub fn has_ts_file_extension(path: &[u8]) -> bool {
    file_extension_is_one_of(path, SUPPORTED_TS_EXTENSIONS_FLAT)
}
/// port: tsc/internal/tspath/extension.go:HasImplementationTSFileExtension
pub fn has_implementation_ts_file_extension(path: &[u8]) -> bool {
    file_extension_is_one_of(path, SUPPORTED_TS_IMPLEMENTATION_EXTENSIONS)
        && !is_declaration_file_name(path)
}
/// port: tsc/internal/tspath/extension.go:HasJSFileExtension
pub fn has_js_file_extension(path: &[u8]) -> bool {
    file_extension_is_one_of(path, SUPPORTED_JS_EXTENSIONS_FLAT)
}
/// port: tsc/internal/tspath/extension.go:HasJSONFileExtension
pub fn has_json_file_extension(path: &[u8]) -> bool {
    file_extension_is(path, EXTENSION_JSON)
}
/// Requires the byte before the extension to be a dot, and returns the path's
/// own bytes rather than the list's.
/// port: tsc/internal/tspath/path.go:tryGetExtensionFromPath
pub fn try_get_extension_from_path_with<'a>(
    path: &'a [u8],
    extension: &[u8],
    equal: fn(&[u8], &[u8]) -> bool,
) -> &'a [u8] {
    let dotted;
    let extension = if extension.starts_with(b".") {
        extension
    } else {
        dotted = [b".".as_slice(), extension].concat();
        &dotted
    };
    if path.len() >= extension.len() && path[path.len() - extension.len()] == b'.' {
        let candidate = &path[path.len() - extension.len()..];
        if equal(candidate, extension) {
            return candidate;
        }
    }
    b""
}
/// First match wins.
/// port: tsc/internal/tspath/path.go:getAnyExtensionFromPathWorker
pub fn any_extension_from_path_worker<'a, T: AsRef<[u8]>>(
    path: &'a [u8],
    extensions: &[T],
    equal: fn(&[u8], &[u8]) -> bool,
) -> &'a [u8] {
    extensions
        .iter()
        .map(|extension| try_get_extension_from_path_with(path, extension.as_ref(), equal))
        .find(|found| !found.is_empty())
        .unwrap_or(b"")
}
/// With an empty list this falls back to the base name's last dot.
/// port: tsc/internal/tspath/path.go:GetAnyExtensionFromPath
pub fn any_extension_from_path<'a, T: AsRef<[u8]>>(
    path: &'a [u8],
    extensions: &[T],
    ignore_case: bool,
) -> &'a [u8] {
    if !extensions.is_empty() {
        return any_extension_from_path_worker(
            remove_trailing_directory_separator(path),
            extensions,
            equality_comparer(ignore_case),
        );
    }
    let base = base_name(path);
    base.iter()
        .rposition(|&b| b == b'.')
        .map_or(b"", |index| &base[index..])
}
/// A longer list entry is tried only while it can still beat the current match.
/// port: tsc/internal/tspath/path.go:GetLongestExtensionFromPath
pub fn longest_extension_from_path<'a, T: AsRef<[u8]>>(
    path: &'a [u8],
    extensions: &[T],
    ignore_case: bool,
) -> &'a [u8] {
    let path = remove_trailing_directory_separator(path);
    let equal = equality_comparer(ignore_case);
    let mut longest: &[u8] = b"";
    for extension in extensions {
        let extension = extension.as_ref();
        if extension.len() > longest.len() {
            let matched = try_get_extension_from_path_with(path, extension, equal);
            if !matched.is_empty() {
                longest = matched;
            }
        }
    }
    longest
}
/// port: tsc/internal/tspath/extension.go:TryGetExtensionFromPath
pub fn try_get_extension_from_path(path: &[u8]) -> &'static [u8] {
    first_file_extension(path, EXTENSIONS_TO_REMOVE)
}
/// port: tsc/internal/tspath/extension.go:TryExtractTSExtension
pub fn try_extract_ts_extension(path: &[u8]) -> &'static [u8] {
    first_file_extension(path, SUPPORTED_TS_EXTENSIONS_FOR_EXTRACT)
}
fn first_file_extension(path: &[u8], extensions: &'static [&'static [u8]]) -> &'static [u8] {
    extensions
        .iter()
        .copied()
        .find(|ext| file_extension_is(path, ext))
        .unwrap_or(b"")
}
/// The three fixed suffixes are tried before the first `.d.` of the base name.
/// port: tsc/internal/tspath/extension.go:GetDeclarationFileExtension
pub fn declaration_file_extension(path: &[u8]) -> &[u8] {
    let base = base_name(path);
    if let Some(ext) = SUPPORTED_DECLARATION_EXTENSIONS
        .iter()
        .find(|ext| base.ends_with(ext))
    {
        return &base[base.len() - ext.len()..];
    }
    if base.ends_with(EXTENSION_TS) {
        if let Some(index) = base.windows(3).position(|w| w == b".d.") {
            return &base[index..];
        }
    }
    b""
}
/// port: tsc/internal/tspath/extension.go:GetDeclarationEmitExtensionForPath
pub fn declaration_emit_extension_for_path(path: &[u8]) -> Vec<u8> {
    if file_extension_is_one_of(path, &[EXTENSION_MJS, EXTENSION_MTS]) {
        EXTENSION_DMTS.to_vec()
    } else if file_extension_is_one_of(path, &[EXTENSION_CJS, EXTENSION_CTS]) {
        EXTENSION_DCTS.to_vec()
    } else if file_extension_is_one_of(
        path,
        &[EXTENSION_TS, EXTENSION_TSX, EXTENSION_JS, EXTENSION_JSX],
    ) {
        EXTENSION_DTS.to_vec()
    } else {
        let ext = any_extension_from_path::<&[u8]>(path, &[], false);
        if ext.is_empty() {
            EXTENSION_DTS.to_vec()
        } else {
            [b".d".as_slice(), ext, b".ts"].concat()
        }
    }
}
/// The candidate order is part of the contract.
/// port: tsc/internal/tspath/extension.go:GetPossibleOriginalInputExtensionForExtension
pub fn possible_original_input_extensions(path: &[u8]) -> Vec<Vec<u8>> {
    let list = |items: &[&[u8]]| items.iter().map(|item| item.to_vec()).collect();
    if file_extension_is_one_of(path, &[EXTENSION_DMTS, EXTENSION_MJS, EXTENSION_MTS]) {
        return list(&[EXTENSION_MTS, EXTENSION_MJS]);
    }
    if file_extension_is_one_of(path, &[EXTENSION_DCTS, EXTENSION_CJS, EXTENSION_CTS]) {
        return list(&[EXTENSION_CTS, EXTENSION_CJS]);
    }
    let ext = declaration_file_extension(path);
    if !ext.is_empty() && ext != EXTENSION_DTS {
        return vec![[b".".as_slice(), &ext[3..ext.len() - 3]].concat()];
    }
    list(&[EXTENSION_TSX, EXTENSION_TS, EXTENSION_JSX, EXTENSION_JS])
}
/// Slices the matched extension off the untrimmed path. An empty new extension
/// deletes; otherwise a missing dot is supplied.
/// port: tsc/internal/tspath/extension.go:ChangeAnyExtension
pub fn change_any_extension<T: AsRef<[u8]>>(
    path: &[u8],
    ext: &[u8],
    extensions: &[T],
    ignore_case: bool,
) -> Vec<u8> {
    let matched = any_extension_from_path(path, extensions, ignore_case).len();
    if matched == 0 {
        return path.to_vec();
    }
    with_extension(&path[..path.len() - matched], ext, true)
}
fn with_extension(stem: &[u8], ext: &[u8], empty_deletes: bool) -> Vec<u8> {
    let mut result = stem.to_vec();
    if ext.is_empty() && empty_deletes {
        return result;
    }
    if !ext.starts_with(b".") {
        result.push(b'.');
    }
    result.extend_from_slice(ext);
    result
}
/// port: tsc/internal/tspath/extension.go:ChangeExtension
pub fn change_extension(path: &[u8], ext: &[u8]) -> Vec<u8> {
    change_any_extension(path, ext, EXTENSIONS_TO_REMOVE, false)
}
/// A declaration extension is replaced from its `.d` onwards.
/// port: tsc/internal/tspath/extension.go:ChangeFullExtension
pub fn change_full_extension(path: &[u8], ext: &[u8]) -> Vec<u8> {
    let declaration = declaration_file_extension(path).len();
    if declaration == 0 {
        return change_extension(path, ext);
    }
    with_extension(&path[..path.len() - declaration], ext, false)
}
/// Unchecked, like the pinned slice: it does not verify the suffix and panics
/// when the extension is longer than the path.
/// port: tsc/internal/tspath/extension.go:RemoveExtension
pub fn remove_extension<'a>(path: &'a [u8], extension: &[u8]) -> &'a [u8] {
    let end = path.len().checked_sub(extension.len()).unwrap_or_else(|| {
        panic!(
            "slice bounds out of range [:{}-{}]",
            path.len(),
            extension.len()
        )
    });
    &path[..end]
}
/// port: tsc/internal/tspath/extension.go:RemoveAnyFileExtension
pub fn remove_any_file_extension(path: &[u8]) -> &[u8] {
    let removed = remove_file_extension(path);
    if removed.len() != path.len() {
        return removed;
    }
    let extension = any_extension_from_path::<&[u8]>(path, &[], false);
    if extension.is_empty() {
        path
    } else {
        remove_extension(path, extension)
    }
}
