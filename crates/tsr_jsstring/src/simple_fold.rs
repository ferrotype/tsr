//! The pinned Go simple-fold comparison shared by spelling and compiler paths.
use crate::wtf8::decode_utf8;

fn simple_fold(rune: i32) -> i32 {
    crate::go_fold_generated::SIMPLE_FOLD
        .binary_search_by_key(&(rune as u32), |row| row.0)
        .map_or(rune, |index| {
            crate::go_fold_generated::SIMPLE_FOLD[index].1 as i32
        })
}

/// Go `strings.EqualFold`, using the pinned toolchain's simple-fold cycles.
/// Malformed bytes each decode to RuneError; equality does not repair the input.
pub fn equal_fold(mut left: &[u8], mut right: &[u8]) -> bool {
    while !left.is_empty() && !right.is_empty() {
        let (a, width_a) = decode_utf8(left);
        let (b, width_b) = decode_utf8(right);
        left = &left[width_a..];
        right = &right[width_b..];
        if a == b {
            continue;
        }
        let (small, large) = if a < b { (a, b) } else { (b, a) };
        let mut folded = simple_fold(small);
        while folded != small && folded < large {
            folded = simple_fold(folded);
        }
        if folded != large {
            return false;
        }
    }
    left.is_empty() && right.is_empty()
}

/// Additional scalars in the pinned Go SimpleFold cycles intersecting a set of
/// sorted, disjoint inclusive ranges. Regex consumers must fold before negation.
pub fn simple_fold_additions(ranges: &[(u32, u32)]) -> Vec<u32> {
    let contains = |rune| {
        let n = ranges.partition_point(|&(lo, _)| lo <= rune);
        n != 0 && rune <= ranges[n - 1].1
    };
    let mut result = Vec::new();
    for &(rune, _) in crate::go_fold_generated::SIMPLE_FOLD {
        if !contains(rune) {
            continue;
        }
        let mut next = simple_fold(rune as i32) as u32;
        while next != rune {
            if !contains(next) {
                result.push(next);
            }
            next = simple_fold(next as i32) as u32;
        }
    }
    result
}
