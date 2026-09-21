//! The unreduced splitter, the reducer, the cheap normaliser and the helpers
//! built on component lists.
use crate::{combine, path_from_components, root_length, to_path};
use std::collections::BTreeSet;
use tsr_jsstring::wtf8::decode_utf8;
use tsr_jsstring::{compare::equality_comparer, equal_fold as equate_case_insensitive};

/// Keeps `.` and interior empty components. Exactly one trailing empty
/// component is removed.
/// port: tsc/internal/tspath/path.go:GetPathComponents
pub fn path_components(path: &[u8], cwd: &[u8]) -> Vec<Vec<u8>> {
    let path = combine(cwd, &[path]);
    split_path_components(&path, root_length(&path))
}
/// Panics, as the pinned slice does, when the root is longer than the path.
/// port: tsc/internal/tspath/path.go:pathComponents
pub fn split_path_components(path: &[u8], root: usize) -> Vec<Vec<u8>> {
    assert!(
        root <= path.len(),
        "slice bounds out of range [:{root}] with length {}",
        path.len()
    );
    let mut rest: Vec<&[u8]> = path[root..].split(|&b| b == b'/').collect();
    if rest.last().is_some_and(|last| last.is_empty()) {
        rest.pop();
    }
    std::iter::once(&path[..root])
        .chain(rest)
        .map(<[u8]>::to_vec)
        .collect()
}
/// `..` survives when the root component is empty and is dropped when it is not.
/// port: tsc/internal/tspath/path.go:reducePathComponents
pub fn reduce_path_components<T: AsRef<[u8]>>(components: &[T]) -> Vec<Vec<u8>> {
    let Some(first) = components.first() else {
        return Vec::new();
    };
    let mut reduced = vec![first.as_ref().to_vec()];
    for component in &components[1..] {
        let component = component.as_ref();
        if component.is_empty() || component == b"." {
            continue;
        }
        if component == b".." {
            if reduced.len() > 1 {
                if reduced.last().is_some_and(|last| last != b"..") {
                    reduced.pop();
                    continue;
                }
            } else if !reduced[0].is_empty() {
                continue;
            }
        }
        reduced.push(component.to_vec());
    }
    reduced
}
/// port: tsc/internal/tspath/path.go:getNormalizedPathComponentsFromCombined
pub fn normalized_components_from_combined(path: &[u8]) -> Vec<Vec<u8>> {
    let root = root_length(path);
    let mut components = vec![path[..root].to_vec()];
    for component in path[root..].split(|&b| b == b'/') {
        if component.is_empty() || component == b"." {
            continue;
        }
        if component == b".." {
            if components.len() > 1 {
                if components.last().is_some_and(|last| last != b"..") {
                    components.pop();
                    continue;
                }
            } else if !components[0].is_empty() {
                continue;
            }
        }
        components.push(component.to_vec());
    }
    components
}
/// Counts dots per segment: a segment of exactly one or two dots, or an empty
/// interior segment, is relative.
/// port: tsc/internal/tspath/path.go:hasRelativePathSegment
pub fn has_relative_path_segment(path: &[u8]) -> bool {
    let mut previous_slash = false;
    let mut segment = 0usize;
    // Consecutive dots at the start of the segment; None once another byte appears.
    let mut dots = Some(0usize);
    for &byte in path {
        if byte == b'/' {
            if previous_slash || (dots == Some(segment) && matches!(segment, 1 | 2)) {
                return true;
            }
            previous_slash = true;
            segment = 0;
            dots = Some(0);
            continue;
        }
        dots = if byte == b'.' {
            dots.map(|n| n + 1)
        } else {
            None
        };
        segment += 1;
        previous_slash = false;
    }
    dots == Some(segment) && matches!(segment, 1 | 2)
}
/// Declines, with `None`, when the cheap cleanup would change the path's meaning.
/// port: tsc/internal/tspath/path.go:simpleNormalizePath
pub fn simple_normalize_path(path: &[u8]) -> Option<Vec<u8>> {
    if !has_relative_path_segment(path) {
        return Some(path.to_vec());
    }
    let mut simplified = Vec::with_capacity(path.len());
    let mut index = 0;
    while index < path.len() {
        if path[index..].starts_with(b"/./") {
            simplified.push(b'/');
            index += 3;
        } else {
            simplified.push(path[index]);
            index += 1;
        }
    }
    let trimmed = simplified.strip_prefix(b"./").unwrap_or(&simplified);
    let stripped = trimmed.len() != simplified.len();
    (trimmed != path
        && !has_relative_path_segment(trimmed)
        && !(stripped && trimmed.starts_with(b"/")))
    .then(|| trimmed.to_vec())
}
/// Skips up to that many decoded runes and clamps at the end. A negative count
/// skips nothing, as Go's `for range n` does.
/// port: tsc/internal/tspath/path.go:trimRuneCount
pub fn trim_rune_count(text: &[u8], rune_count: isize) -> &[u8] {
    let mut index = 0;
    for _ in 0..rune_count.max(0) {
        if index >= text.len() {
            break;
        }
        index += decode_utf8(&text[index..]).1;
    }
    &text[index..]
}
/// Component 0 is always compared case-insensitively; later components use the
/// requested rule. With nothing shared the `to` components come back untouched.
/// port: tsc/internal/tspath/path.go:GetPathComponentsRelativeTo
pub fn path_components_relative_to(
    from: &[u8],
    to: &[u8],
    cwd: &[u8],
    case_sensitive: bool,
) -> Vec<Vec<u8>> {
    let from = reduce_path_components(&path_components(from, cwd));
    let to = reduce_path_components(&path_components(to, cwd));
    let equal = equality_comparer(!case_sensitive);
    let mut start = 0;
    while start < from.len().min(to.len()) {
        let same = if start == 0 {
            equate_case_insensitive(&from[start], &to[start])
        } else {
            equal(&from[start], &to[start])
        };
        if !same {
            break;
        }
        start += 1;
    }
    if start == 0 {
        return to;
    }
    let mut result = vec![Vec::new()];
    result.extend(std::iter::repeat_n(b"..".to_vec(), from.len() - start));
    result.extend_from_slice(&to[start..]);
    result
}

/// The minimal covering set of parent directories, and the inputs too shallow
/// to take part. Panics before looking at the paths when `min_components < 1`.
/// port: tsc/internal/tspath/path.go:GetCommonParents
pub fn common_parents(
    paths: &[&[u8]],
    min_components: isize,
    get_components: impl Fn(&[u8], &[u8]) -> Vec<Vec<u8>>,
    cwd: &[u8],
    case_sensitive: bool,
) -> (Vec<Vec<u8>>, BTreeSet<Vec<u8>>) {
    assert!(min_components >= 1, "minComponents must be at least 1");
    let mut ignored = BTreeSet::new();
    let deep_enough = |components: &[Vec<u8>]| {
        isize::try_from(components.len()).unwrap_or(isize::MAX) >= min_components
    };
    if paths.len() == 1 {
        let components = reduce_path_components(&get_components(paths[0], cwd));
        if !deep_enough(&components) {
            ignored.insert(paths[0].to_vec());
            return (Vec::new(), ignored);
        }
        return (vec![paths[0].to_vec()], ignored);
    }
    let mut groups = Vec::with_capacity(paths.len());
    for path in paths {
        let components = reduce_path_components(&get_components(path, cwd));
        if deep_enough(&components) {
            groups.push(components);
        } else {
            ignored.insert(path.to_vec());
        }
    }
    let parents = common_parents_worker(&groups, min_components, cwd, case_sensitive)
        .iter()
        .map(|components| path_from_components(components))
        .collect();
    (parents, ignored)
}
/// One fan-out group: its canonical key, the head of its last member, its tails.
type Group<'a> = (Vec<u8>, &'a [Vec<u8>], Vec<Vec<Vec<u8>>>);

/// Fans out on a divergence shallower than `min_components`, grouping by the
/// canonical path of the diverging component and emitting groups in sorted key
/// order. Each group keeps the head of its last member, as the pinned map does.
/// port: tsc/internal/tspath/path.go:getCommonParentsWorker
pub fn common_parents_worker(
    groups: &[Vec<Vec<u8>>],
    min_components: isize,
    cwd: &[u8],
    case_sensitive: bool,
) -> Vec<Vec<Vec<u8>>> {
    let Some(first) = groups.first() else {
        return Vec::new();
    };
    let max_depth = groups.iter().map(Vec::len).min().unwrap_or(0);
    let equal = equality_comparer(!case_sensitive);
    for index in 0..max_depth {
        if groups[1..]
            .iter()
            .all(|group| equal(&first[index], &group[index]))
        {
            continue;
        }
        if isize::try_from(index).unwrap_or(isize::MAX) >= min_components {
            return vec![first[..index].to_vec()];
        }
        let mut order: Vec<Vec<u8>> = Vec::new();
        let mut members: Vec<Group<'_>> = Vec::new();
        for group in groups {
            let key = to_path(&group[index], cwd, case_sensitive)
                .as_bytes()
                .to_vec();
            let tail = group[index + 1..].to_vec();
            if let Some(entry) = members.iter_mut().find(|(k, ..)| *k == key) {
                entry.1 = &group[..=index];
                entry.2.push(tail);
            } else {
                order.push(key.clone());
                members.push((key, &group[..=index], vec![tail]));
            }
        }
        order.sort();
        let remaining = min_components - isize::try_from(index + 1).unwrap_or(isize::MAX);
        let mut result = Vec::new();
        for key in order {
            let (_, head, tails) = members
                .iter()
                .find(|(k, ..)| *k == key)
                .expect("every ordered key has a group");
            for tail in common_parents_worker(tails, remaining, cwd, case_sensitive) {
                let mut parent = head.to_vec();
                parent.extend(tail);
                result.push(parent);
            }
        }
        return result;
    }
    vec![first[..max_depth].to_vec()]
}
