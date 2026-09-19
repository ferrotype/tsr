//! Distinct source path comparisons: simple folding, filename canonicalization,
//! and Go's lowercase ordering are deliberately separate operations.
use crate::{canonical, combine, directory, root_length};
pub use ts_jsstring::equal_fold;
use ts_jsstring::{helpers::to_lower_go, wtf8::decode_utf8};
/// port: tsc/internal/tspath/path.go:PathIsRelative
pub fn is_relative(path: &[u8]) -> bool {
    matches!(path, b"." | b"..")
        || path.starts_with(b"./")
        || path.starts_with(b".\\")
        || path.starts_with(b"../")
        || path.starts_with(b"..\\")
}
/// port: tsc/internal/tspath/path.go:TrimFilePathPrefix
pub fn trim_file_path_prefix<'a>(
    path: &'a [u8],
    prefix: &[u8],
    case_sensitive: bool,
) -> Option<&'a [u8]> {
    if case_sensitive {
        return path.strip_prefix(prefix);
    }
    let prefix = canonical(prefix, false);
    if !canonical(path, false).starts_with(&prefix) {
        return None;
    }
    let mut count = 0;
    let mut remaining = prefix.as_ref();
    while !remaining.is_empty() {
        let (_, width) = decode_utf8(remaining);
        remaining = &remaining[width..];
        count += 1;
    }
    let mut remaining = path;
    for _ in 0..count {
        if remaining.is_empty() {
            break;
        }
        let (_, width) = decode_utf8(remaining);
        remaining = &remaining[width..];
    }
    Some(remaining)
}
/// port: tsc/internal/tspath/path.go:GetNormalizedPathComponents
pub fn normalized_components(path: &[u8], cwd: &[u8]) -> Vec<Vec<u8>> {
    let path = combine(cwd, &[path]);
    let root = root_length(&path);
    let mut parts = vec![path[..root].to_vec()];
    for part in path[root..].split(|&byte| byte == b'/') {
        if part.is_empty() || part == b"." {
            continue;
        }
        if part == b".." {
            if parts.len() > 1 && parts.last().is_some_and(|last| last != b"..") {
                parts.pop();
                continue;
            }
            if root != 0 {
                continue;
            }
        }
        parts.push(part.to_vec());
    }
    parts
}
/// port: tsc/internal/tspath/path.go:GetPathFromPathComponents
pub fn path_from_components(parts: &[Vec<u8>]) -> Vec<u8> {
    if parts.is_empty() {
        return Vec::new();
    }
    let mut result = parts[0].clone();
    if !result.is_empty() && !matches!(result.last(), Some(b'/' | b'\\')) {
        result.push(b'/');
    }
    result.extend_from_slice(&parts[1..].join(&b'/'));
    result
}
/// port: tsc/internal/tspath/path.go:GetRelativePathFromDirectory
pub fn relative_from_directory(
    from: &[u8],
    to: &[u8],
    cwd: &[u8],
    case_sensitive: bool,
) -> Vec<u8> {
    assert!(
        (root_length(from) > 0) == (root_length(to) > 0),
        "paths must either both be absolute or both be relative"
    );
    relative_to_directory_or_url(from, to, false, cwd, case_sensitive)
}
/// Relative paths accept mixed absolute/relative inputs and URL roots here;
/// `relative_from_directory` retains its separate native precondition.
/// port: tsc/internal/tspath/path.go:GetRelativePathToDirectoryOrUrl
pub fn relative_to_directory_or_url(
    from: &[u8],
    to: &[u8],
    absolute_path_as_url: bool,
    cwd: &[u8],
    case_sensitive: bool,
) -> Vec<u8> {
    let from = normalized_components(from, cwd);
    let mut to = normalized_components(to, cwd);
    let common = from
        .iter()
        .zip(&to)
        .enumerate()
        .take_while(|(index, (left, right))| {
            if *index == 0 || !case_sensitive {
                equal_fold(left, right)
            } else {
                left == right
            }
        })
        .count();
    if common == 0 {
        if absolute_path_as_url && crate::encoded_root_length(&to[0]) > 0 {
            let prefix = if to[0].starts_with(b"/") {
                b"file://".as_slice()
            } else {
                b"file:///"
            };
            to[0] = [prefix, &to[0]].concat();
        }
        return path_from_components(&to);
    }
    let mut result = vec![Vec::new()];
    result.extend((common..from.len()).map(|_| b"..".to_vec()));
    result.extend_from_slice(&to[common..]);
    path_from_components(&result)
}
/// port: tsc/internal/tspath/path.go:GetRelativePathFromFile
pub fn relative_from_file(from: &[u8], to: &[u8], cwd: &[u8], case_sensitive: bool) -> Vec<u8> {
    let value = relative_from_directory(&directory(from), to, cwd, case_sensitive);
    if is_relative(&value) || root_length(&value) != 0 {
        value
    } else {
        [b"./".as_slice(), &value].concat()
    }
}
/// ComparePaths uses lowercased code points, not SimpleFold or filename keys.
/// port: tsc/internal/tspath/path.go:ComparePaths
pub fn compare_paths(
    left: &[u8],
    right: &[u8],
    cwd: &[u8],
    case_sensitive: bool,
) -> std::cmp::Ordering {
    let left = combine(cwd, &[left]);
    let right = combine(cwd, &[right]);
    if left == right {
        return std::cmp::Ordering::Equal;
    }
    if left.is_empty() {
        return std::cmp::Ordering::Less;
    }
    if right.is_empty() {
        return std::cmp::Ordering::Greater;
    }
    let left_root = root_length(&left);
    let right_root = root_length(&right);
    let order = to_lower_go(&left[..left_root]).cmp(&to_lower_go(&right[..right_root]));
    if order != std::cmp::Ordering::Equal {
        return order;
    }
    let (a, b) = (&left[left_root..], &right[right_root..]);
    let relative = |path: &[u8]| {
        path.windows(2).any(|pair| pair == b"//")
            || path
                .split(|&byte| byte == b'/')
                .any(|part| matches!(part, b"." | b".."))
    };
    if !relative(a) && !relative(b) {
        return if case_sensitive {
            a.cmp(b)
        } else {
            to_lower_go(a).cmp(&to_lower_go(b))
        };
    }
    let left = normalized_components(&left, b"");
    let right = normalized_components(&right, b"");
    for (index, (a, b)) in left.iter().zip(&right).enumerate() {
        let order = if index == 0 || !case_sensitive {
            to_lower_go(a).cmp(&to_lower_go(b))
        } else {
            a.cmp(b)
        };
        if order != std::cmp::Ordering::Equal {
            return order;
        }
    }
    left.len().cmp(&right.len())
}
/// port: tsc/internal/tspath/path.go:ContainsPath
pub fn contains_path(parent: &[u8], child: &[u8], cwd: &[u8], case_sensitive: bool) -> bool {
    let parent = combine(cwd, &[parent]);
    let child = combine(cwd, &[child]);
    if parent.is_empty() || child.is_empty() {
        return false;
    }
    if parent == child {
        return true;
    }
    let parent = normalized_components(&parent, b"");
    let child = normalized_components(&child, b"");
    parent.len() <= child.len()
        && parent.iter().zip(child).enumerate().all(|(index, (a, b))| {
            if index == 0 || !case_sensitive {
                equal_fold(a, &b)
            } else {
                a == &b
            }
        })
}
