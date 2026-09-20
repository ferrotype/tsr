//! Byte-preserving path operations required by parser source-file construction.
//! These use the pinned compiler's path grammar, independent of the host OS.

use std::borrow::Cow;

fn separator(byte: u8) -> bool {
    byte == b'/' || byte == b'\\'
}

/// URL roots are bitwise complemented; disk and untitled roots are positive.
/// port: tsc/internal/tspath/path.go:GetEncodedRootLength
pub fn encoded_root_length(path: &[u8]) -> isize {
    let Some(&first) = path.first() else {
        return 0;
    };
    if separator(first) {
        if path.len() == 1 || path[1] != first {
            return 1;
        }
        return path[2..]
            .iter()
            .position(|&c| c == first)
            .map_or(path.len(), |end| end + 3) as isize;
    }
    if first.is_ascii_alphabetic() && path.get(1) == Some(&b':') {
        if path.len() == 2 {
            return 2;
        }
        if separator(path[2]) {
            return 3;
        }
    }
    if first == b'^' && path.get(1) == Some(&b'/') {
        return 2;
    }
    if let Some(scheme_end) = path.windows(3).position(|bytes| bytes == b"://") {
        let authority_start = scheme_end + 3;
        if let Some(end) = path[authority_start..].iter().position(|&b| b == b'/') {
            let authority_end = authority_start + end;
            let authority = &path[authority_start..authority_end];
            if &path[..scheme_end] == b"file"
                && (authority.is_empty() || authority == b"localhost")
                && path.len() > authority_end + 2
                && path[authority_end + 1].is_ascii_alphabetic()
            {
                let start = authority_end + 2;
                let volume_end = if path[start] == b':' {
                    Some(start + 1)
                } else if path
                    .get(start..start + 3)
                    .is_some_and(|s| s == b"%3a" || s == b"%3A")
                {
                    Some(start + 3)
                } else {
                    None
                };
                if let Some(end) = volume_end {
                    if end == path.len() {
                        return !(end as isize);
                    }
                    if path[end] == b'/' {
                        return !((end + 1) as isize);
                    }
                }
            }
            return !((authority_end + 1) as isize);
        }
        return !(path.len() as isize);
    }
    0
}

pub fn root_length(path: &[u8]) -> usize {
    let encoded = encoded_root_length(path);
    if encoded < 0 {
        (!encoded) as usize
    } else {
        encoded as usize
    }
}

fn has_relative_segment(path: &[u8]) -> bool {
    path.windows(2).any(|s| s == b"//")
        || path.split(|&b| b == b'/').any(|s| s == b"." || s == b"..")
}

/// Preserve trailing separators, URL/UNC roots, malformed bytes and case.
/// port: tsc/internal/tspath/path.go:NormalizePath
pub fn normalize(path: &[u8]) -> Cow<'_, [u8]> {
    let slashes: Cow<'_, [u8]> = if path.contains(&b'\\') {
        Cow::Owned(
            path.iter()
                .map(|&b| if b == b'\\' { b'/' } else { b })
                .collect(),
        )
    } else {
        Cow::Borrowed(path)
    };
    if !has_relative_segment(&slashes) {
        return slashes;
    }

    // The fast source simplification is observable for bare drive/URL roots.
    let mut simplified = Vec::with_capacity(slashes.len());
    let mut index = 0;
    while index < slashes.len() {
        if slashes.get(index..index + 3) == Some(b"/./") {
            simplified.push(b'/');
            index += 3;
        } else {
            simplified.push(slashes[index]);
            index += 1;
        }
    }
    let trimmed = simplified.strip_prefix(b"./").unwrap_or(&simplified);
    if trimmed != slashes.as_ref()
        && !has_relative_segment(trimmed)
        && !(trimmed.len() != simplified.len() && trimmed.starts_with(b"/"))
    {
        return Cow::Owned(trimmed.to_vec());
    }

    let root_end = root_length(&slashes);
    let root = &slashes[..root_end];
    let mut components: Vec<&[u8]> = Vec::new();
    for component in slashes[root_end..].split(|&b| b == b'/') {
        if component.is_empty() || component == b"." {
            continue;
        }
        if component == b".." {
            if components.last().is_some_and(|last| *last != b"..") {
                components.pop();
                continue;
            }
            if components.is_empty() && !root.is_empty() {
                continue;
            }
        }
        components.push(component);
    }
    let mut result = root.to_vec();
    if !result.is_empty() && !result.ends_with(b"/") {
        result.push(b'/');
    }
    for (index, component) in components.iter().enumerate() {
        if index > 0 {
            result.push(b'/');
        }
        result.extend_from_slice(component);
    }
    if !result.is_empty() && slashes.ends_with(b"/") && !result.ends_with(b"/") {
        result.push(b'/');
    }
    Cow::Owned(result)
}

/// Includes arbitrary-extension declarations such as `style.d.css.ts`.
/// port: tsc/internal/tspath/extension.go:IsDeclarationFileName
pub fn is_declaration_file_name(path: &[u8]) -> bool {
    let normalized = if path.contains(&b'\\') {
        Cow::Owned(
            path.iter()
                .map(|&b| if b == b'\\' { b'/' } else { b })
                .collect::<Vec<_>>(),
        )
    } else {
        Cow::Borrowed(path)
    };
    let root_end = root_length(&normalized);
    if root_end == normalized.len() {
        return false;
    }
    let without_trailing = normalized.strip_suffix(b"/").unwrap_or(&normalized);
    let basename_start = without_trailing
        .iter()
        .rposition(|&b| b == b'/')
        .map_or(root_end, |i| root_end.max(i + 1));
    let base = &without_trailing[basename_start..];
    base.ends_with(b".d.ts")
        || base.ends_with(b".d.cts")
        || base.ends_with(b".d.mts")
        || (base.ends_with(b".ts") && base.windows(3).any(|s| s == b".d."))
}

/// Remove the first matching pinned compiler suffix, including declaration suffixes.
/// port: tsc/internal/tspath/extension.go:RemoveFileExtension
pub fn remove_file_extension(path: &[u8]) -> &[u8] {
    for suffix in [
        b".d.ts".as_slice(),
        b".d.mts",
        b".d.cts",
        b".mjs",
        b".mts",
        b".cjs",
        b".cts",
        b".ts",
        b".js",
        b".tsx",
        b".jsx",
        b".json",
    ] {
        if let Some(path) = path.strip_suffix(suffix) {
            return path;
        }
    }
    path
}
