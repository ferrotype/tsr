//! Byte-preserving compiler paths, independent of the host operating system.
use std::borrow::Cow;
mod comparison;
pub use comparison::{
    compare_paths, contains_path, equal_fold, is_relative, normalized_components,
    path_from_components, relative_from_directory, relative_from_file,
    relative_to_directory_or_url, trim_file_path_prefix,
};
pub use ts_core::path::{
    encoded_root_length, is_declaration_file_name, normalize, remove_file_extension, root_length,
};
use ts_jsstring::JsString;

/// port: tsc/internal/tspath/path.go:NormalizeSlashes
pub fn normalize_slashes(path: &[u8]) -> Cow<'_, [u8]> {
    if path.contains(&b'\\') {
        Cow::Owned(
            path.iter()
                .map(|&b| if b == b'\\' { b'/' } else { b })
                .collect(),
        )
    } else {
        Cow::Borrowed(path)
    }
}
/// port: tsc/internal/tspath/path.go:CombinePaths
pub fn combine(first: &[u8], paths: &[&[u8]]) -> Vec<u8> {
    let mut result = normalize_slashes(first).into_owned();
    for path in paths {
        if path.is_empty() {
            continue;
        }
        let path = normalize_slashes(path);
        if result.is_empty() || root_length(&path) != 0 {
            result = path.into_owned();
        } else {
            if !result.ends_with(b"/") {
                result.push(b'/');
            }
            result.extend_from_slice(&path);
        }
    }
    result
}
/// port: tsc/internal/tspath/path.go:GetDirectoryPath
pub fn directory(path: &[u8]) -> Vec<u8> {
    let path = normalize_slashes(path);
    let root = root_length(&path);
    if root == path.len() {
        return path.into_owned();
    }
    let path = if path.ends_with(b"/") {
        &path[..path.len() - 1]
    } else {
        &path
    };
    path[..root.max(path.iter().rposition(|&b| b == b'/').unwrap_or(0))].to_vec()
}
/// port: tsc/internal/tspath/path.go:ResolvePath
pub fn resolve(path: &[u8], paths: &[&[u8]]) -> Vec<u8> {
    normalize(&combine(path, paths)).into_owned()
}
/// port: tsc/internal/tspath/path.go:GetNormalizedAbsolutePath
pub fn absolute(file: &[u8], cwd: &[u8]) -> Vec<u8> {
    let mut path = if root_length(file) == 0 && !cwd.is_empty() {
        normalize(&combine(cwd, &[file])).into_owned()
    } else {
        normalize(file).into_owned()
    };
    let root = root_length(&path);
    if path.len() > root && path.ends_with(b"/") {
        path.pop();
    } else if path.len() == root && root != 0 && !path.ends_with(b"/") {
        path.push(b'/');
    }
    path
}
/// port: tsc/internal/tspath/path.go:ToFileNameLowerCase
pub fn file_name_lower_case(path: &[u8]) -> Cow<'_, [u8]> {
    if path.is_ascii() {
        if path.iter().all(|b| !b.is_ascii_uppercase()) {
            return Cow::Borrowed(path);
        }
        return Cow::Owned(path.to_ascii_lowercase());
    }
    let mut result = Vec::with_capacity(path.len());
    let mut offset = 0;
    while offset < path.len() {
        if path[offset..].starts_with(&[0xc4, 0xb0]) {
            result.extend_from_slice(&[0xc4, 0xb0]);
            offset += 2;
            continue;
        }
        let end = path[offset..]
            .windows(2)
            .position(|pair| pair == [0xc4, 0xb0])
            .map_or(path.len(), |n| offset + n);
        result.extend(ts_jsstring::helpers::to_lower_go(&path[offset..end]));
        offset = end;
    }
    Cow::Owned(result)
}
/// port: tsc/internal/tspath/path.go:GetCanonicalFileName
pub fn canonical(path: &[u8], case_sensitive: bool) -> Cow<'_, [u8]> {
    if case_sensitive {
        Cow::Borrowed(path)
    } else {
        file_name_lower_case(path)
    }
}
/// port: tsc/internal/tspath/path.go:ToPath
pub fn to_path(file: &[u8], base: &[u8], case_sensitive: bool) -> JsString {
    JsString::from_bytes(canonical(&absolute(file, base), case_sensitive).into_owned())
}
/// port: tsc/internal/tspath/path.go:GetBaseFileName
pub fn base_name(path: &[u8]) -> &[u8] {
    let root = root_length(path);
    if root == path.len() {
        return b"";
    }
    let path = if path.ends_with(b"/") || path.ends_with(b"\\") {
        &path[..path.len() - 1]
    } else {
        path
    };
    &path[path
        .iter()
        .rposition(|&b| b == b'/' || b == b'\\')
        .map_or(root, |n| n + 1)..]
}
pub fn has_extension(path: &[u8]) -> bool {
    base_name(path).contains(&b'.')
}
pub fn ancestors(path: &[u8]) -> Vec<Vec<u8>> {
    let mut path = path.to_vec();
    let mut result = Vec::new();
    loop {
        result.push(path.clone());
        let parent = directory(&path);
        if parent == path {
            break;
        }
        path = parent;
        if path.is_empty() {
            break;
        }
    }
    result
}
