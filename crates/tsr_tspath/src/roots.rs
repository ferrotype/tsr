//! Root-shaped predicates, separators and the small path rewrites built on them.
use crate::{
    canonical, combine, directory, encoded_root_length, is_relative, normalize,
    relative_to_directory_or_url, root_length,
};
use std::{borrow::Cow, cmp::Ordering};

/// Takes a byte, not a decoded character.
/// port: tsc/internal/tspath/path.go:isAnyDirectorySeparator
pub fn is_any_directory_separator(byte: u8) -> bool {
    byte == b'/' || byte == b'\\'
}
/// port: tsc/internal/tspath/path.go:IsVolumeCharacter
pub fn is_volume_character(byte: u8) -> bool {
    byte.is_ascii_alphabetic()
}
/// port: tsc/internal/tspath/path.go:IsUrl
pub fn is_url(path: &[u8]) -> bool {
    encoded_root_length(path) < 0
}
/// port: tsc/internal/tspath/path.go:IsRootedDiskPath
pub fn is_rooted_disk_path(path: &[u8]) -> bool {
    encoded_root_length(path) > 0
}
/// port: tsc/internal/tspath/path.go:IsDiskPathRoot
pub fn is_disk_path_root(path: &[u8]) -> bool {
    let root = encoded_root_length(path);
    root > 0 && root.unsigned_abs() == path.len()
}
/// port: tsc/internal/tspath/path.go:IsDynamicFileName
pub fn is_dynamic_file_name(path: &[u8]) -> bool {
    path.starts_with(b"^/")
}
/// port: tsc/internal/tspath/path.go:PathIsAbsolute
pub fn path_is_absolute(path: &[u8]) -> bool {
    encoded_root_length(path) != 0
}
/// `None` is Go's -1.
/// port: tsc/internal/tspath/path.go:getFileUrlVolumeSeparatorEnd
pub fn file_url_volume_separator_end(url: &[u8], start: usize) -> Option<usize> {
    match *url.get(start)? {
        b':' => Some(start + 1),
        b'%' if url.len() > start + 2
            && url[start + 1] == b'3'
            && matches!(url[start + 2], b'a' | b'A') =>
        {
            Some(start + 3)
        }
        _ => None,
    }
}
/// The volume is lowercased; the remainder is untouched. `c:d` has a volume
/// even though it has no root length.
/// port: tsc/internal/tspath/path.go:SplitVolumePath
pub fn split_volume_path(path: &[u8]) -> (Vec<u8>, &[u8], bool) {
    if path.len() >= 2 && is_volume_character(path[0]) && path[1] == b':' {
        (path[..2].to_ascii_lowercase(), &path[2..], true)
    } else {
        (Vec::new(), path, false)
    }
}
/// port: tsc/internal/tspath/path.go:HasTrailingDirectorySeparator
pub fn has_trailing_directory_separator(path: &[u8]) -> bool {
    path.last().is_some_and(|&b| is_any_directory_separator(b))
}
/// port: tsc/internal/tspath/path.go:RemoveTrailingDirectorySeparator
pub fn remove_trailing_directory_separator(path: &[u8]) -> &[u8] {
    if has_trailing_directory_separator(path) {
        &path[..path.len() - 1]
    } else {
        path
    }
}
/// port: tsc/internal/tspath/path.go:RemoveTrailingDirectorySeparators
pub fn remove_trailing_directory_separators(mut path: &[u8]) -> &[u8] {
    while has_trailing_directory_separator(path) {
        path = &path[..path.len() - 1];
    }
    path
}
/// port: tsc/internal/tspath/path.go:EnsureTrailingDirectorySeparator
pub fn ensure_trailing_directory_separator(path: &[u8]) -> Cow<'_, [u8]> {
    if has_trailing_directory_separator(path) {
        Cow::Borrowed(path)
    } else {
        let mut owned = Vec::with_capacity(path.len() + 1);
        owned.extend_from_slice(path);
        owned.push(b'/');
        Cow::Owned(owned)
    }
}
/// port: tsc/internal/tspath/path.go:EnsurePathIsNonModuleName
pub fn ensure_path_is_non_module_name(path: &[u8]) -> Cow<'_, [u8]> {
    if !path_is_absolute(path) && !is_relative(path) {
        let mut owned = Vec::with_capacity(path.len() + 2);
        owned.extend_from_slice(b"./");
        owned.extend_from_slice(path);
        Cow::Owned(owned)
    } else {
        Cow::Borrowed(path)
    }
}
/// Dot-relative or a rooted disk path, so a URL is rejected.
/// port: tsc/internal/tspath/path.go:IsExternalModuleNameRelative
pub fn is_external_module_name_relative(name: &[u8]) -> bool {
    is_relative(name) || is_rooted_disk_path(name)
}
/// port: tsc/internal/tspath/path.go:ResolveTripleslashReference
pub fn resolve_tripleslash_reference(module_name: &[u8], containing_file: &[u8]) -> Vec<u8> {
    if is_rooted_disk_path(module_name) {
        return normalize(module_name).into_owned();
    }
    normalize(&combine(&directory(containing_file), &[module_name])).into_owned()
}
/// Only a rooted disk path is converted; everything else, URLs included, is
/// returned byte for byte.
/// port: tsc/internal/tspath/path.go:ConvertToRelativePath
pub fn convert_to_relative_path(path: &[u8], cwd: &[u8], case_sensitive: bool) -> Vec<u8> {
    if !is_rooted_disk_path(path) {
        return path.to_vec();
    }
    relative_to_directory_or_url(cwd, path, false, cwd, case_sensitive)
}
/// port: tsc/internal/tspath/path.go:GetNormalizedAbsolutePathWithoutRoot
pub fn normalized_absolute_path_without_root(file: &[u8], cwd: &[u8]) -> Vec<u8> {
    let absolute = crate::absolute(file, cwd);
    absolute[root_length(&absolute)..].to_vec()
}
/// A raw substring search: no normalisation and no case folding.
/// port: tsc/internal/tspath/ignoredpaths.go:ContainsIgnoredPath
pub fn contains_ignored_path(path: &[u8]) -> bool {
    [b"/node_modules/.".as_slice(), b"/.git", b".#"]
        .iter()
        .any(|pattern| path.windows(pattern.len()).any(|w| w == *pattern))
}
/// port: tsc/internal/tspath/path.go:StartsWithDirectory
pub fn starts_with_directory(file: &[u8], directory_name: &[u8], case_sensitive: bool) -> bool {
    if directory_name.is_empty() {
        return false;
    }
    let file = canonical(file, case_sensitive);
    let directory_name = canonical(directory_name, case_sensitive);
    let mut prefix: &[u8] = &directory_name;
    prefix = prefix.strip_suffix(b"/").unwrap_or(prefix);
    prefix = prefix.strip_suffix(b"\\").unwrap_or(prefix);
    file.len() > prefix.len()
        && file.starts_with(prefix)
        && is_any_directory_separator(file[prefix.len()])
}
/// Counts `/` only and normalises nothing first.
/// port: tsc/internal/tspath/path.go:CompareNumberOfDirectorySeparators
pub fn compare_number_of_directory_separators(left: &[u8], right: &[u8]) -> Ordering {
    // Paths are short; a dependency for counting one byte is not warranted.
    let count = |path: &[u8]| path.iter().fold(0usize, |n, &b| n + usize::from(b == b'/'));
    count(left).cmp(&count(right))
}
