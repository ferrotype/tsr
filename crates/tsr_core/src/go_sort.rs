// Copyright 2022 The Go Authors. All rights reserved.
// SPDX-License-Identifier: BSD-3-Clause
// Derived from Go 1.27.1 src/slices/{sort.go,zsortanyfunc.go}.
// See licenses/GO-BSD-3-Clause.txt for the retained license.

//! Pinned Go SortFunc comparison order. Lazy node identities and diagnostics
//! that compare equal can expose comparison/swap order, so substituting a Rust
//! unstable sort changes those source observations.
use std::cmp::Ordering;

type Compare<'a, T> = &'a mut dyn FnMut(&T, &T) -> Ordering;

/// Sort in the pinned Go SortFunc comparison/swap order.
/// The comparator must meet the source ordering contract.
pub fn sort<T>(data: &mut [T], compare: &mut impl FnMut(&T, &T) -> Ordering) {
    let length = data.len();
    let limit = usize::BITS - length.leading_zeros();
    pdqsort(data, 0, length, limit, compare);
}

fn insertion_sort<T>(data: &mut [T], a: usize, b: usize, compare: Compare<'_, T>) {
    for i in a + 1..b {
        let mut j = i;
        while j > a && compare(&data[j], &data[j - 1]).is_lt() {
            data.swap(j, j - 1);
            j -= 1;
        }
    }
}
fn sift_down<T>(data: &mut [T], lo: usize, hi: usize, first: usize, compare: Compare<'_, T>) {
    let mut root = lo;
    loop {
        let mut child = 2 * root + 1;
        if child >= hi {
            break;
        }
        if child + 1 < hi && compare(&data[first + child], &data[first + child + 1]).is_lt() {
            child += 1;
        }
        if !compare(&data[first + root], &data[first + child]).is_lt() {
            return;
        }
        data.swap(first + root, first + child);
        root = child;
    }
}
fn heap_sort<T>(data: &mut [T], a: usize, b: usize, compare: Compare<'_, T>) {
    let length = b - a;
    // Go integer division makes (-1)/2 zero when length is zero.
    for i in (0..=length.saturating_sub(1) / 2).rev() {
        sift_down(data, i, length, a, compare);
    }
    for i in (0..length).rev() {
        data.swap(a, a + i);
        sift_down(data, 0, i, a, compare);
    }
}

fn pdqsort<T>(data: &mut [T], mut a: usize, mut b: usize, mut limit: u32, compare: Compare<'_, T>) {
    let mut was_balanced = true;
    let mut was_partitioned = true;
    loop {
        let length = b - a;
        if length <= 12 {
            insertion_sort(data, a, b, compare);
            return;
        }
        if limit == 0 {
            heap_sort(data, a, b, compare);
            return;
        }
        if !was_balanced {
            break_patterns(data, a, b);
            limit -= 1;
        }
        let (mut pivot, mut hint) = choose_pivot(data, a, b, compare);
        if hint == Hint::Decreasing {
            data[a..b].reverse();
            pivot = b - 1 - (pivot - a);
            hint = Hint::Increasing;
        }
        if was_balanced
            && was_partitioned
            && hint == Hint::Increasing
            && partial_insertion_sort(data, a, b, compare)
        {
            return;
        }
        if a > 0 && !compare(&data[a - 1], &data[pivot]).is_lt() {
            a = partition_equal(data, a, b, pivot, compare);
            continue;
        }
        let (mid, already_partitioned) = partition(data, a, b, pivot, compare);
        was_partitioned = already_partitioned;
        let (left_len, right_len) = (mid - a, b - mid);
        let balance_threshold = length / 8;
        // Only the smaller partition recurses; stack depth is bounded by log2(n).
        if left_len < right_len {
            was_balanced = left_len >= balance_threshold;
            pdqsort(data, a, mid, limit, compare);
            a = mid + 1;
        } else {
            was_balanced = right_len >= balance_threshold;
            pdqsort(data, mid + 1, b, limit, compare);
            b = mid;
        }
    }
}
fn partition<T>(
    data: &mut [T],
    a: usize,
    b: usize,
    pivot: usize,
    compare: Compare<'_, T>,
) -> (usize, bool) {
    data.swap(a, pivot);
    let (mut i, mut j) = (a + 1, b - 1);
    while i <= j && compare(&data[i], &data[a]).is_lt() {
        i += 1;
    }
    while i <= j && !compare(&data[j], &data[a]).is_lt() {
        j -= 1;
    }
    if i > j {
        data.swap(j, a);
        return (j, true);
    }
    data.swap(i, j);
    i += 1;
    j -= 1;
    loop {
        while i <= j && compare(&data[i], &data[a]).is_lt() {
            i += 1;
        }
        while i <= j && !compare(&data[j], &data[a]).is_lt() {
            j -= 1;
        }
        if i > j {
            break;
        }
        data.swap(i, j);
        i += 1;
        j -= 1;
    }
    data.swap(j, a);
    (j, false)
}
fn partition_equal<T>(
    data: &mut [T],
    a: usize,
    b: usize,
    pivot: usize,
    compare: Compare<'_, T>,
) -> usize {
    data.swap(a, pivot);
    let (mut i, mut j) = (a + 1, b - 1);
    loop {
        while i <= j && !compare(&data[a], &data[i]).is_lt() {
            i += 1;
        }
        while i <= j && compare(&data[a], &data[j]).is_lt() {
            j -= 1;
        }
        if i > j {
            break;
        }
        data.swap(i, j);
        i += 1;
        j -= 1;
    }
    i
}
fn partial_insertion_sort<T>(data: &mut [T], a: usize, b: usize, compare: Compare<'_, T>) -> bool {
    let mut i = a + 1;
    for _ in 0..5 {
        while i < b && !compare(&data[i], &data[i - 1]).is_lt() {
            i += 1;
        }
        if i == b {
            return true;
        }
        if b - a < 50 {
            return false;
        }
        data.swap(i, i - 1);
        if i - a >= 2 {
            // The pinned algorithm intentionally stops at index 1, not a + 1.
            for j in (1..i).rev() {
                if !compare(&data[j], &data[j - 1]).is_lt() {
                    break;
                }
                data.swap(j, j - 1);
            }
        }
        if b - i >= 2 {
            for j in i + 1..b {
                if !compare(&data[j], &data[j - 1]).is_lt() {
                    break;
                }
                data.swap(j, j - 1);
            }
        }
    }
    false
}
fn break_patterns<T>(data: &mut [T], a: usize, b: usize) {
    let length = b - a;
    if length >= 8 {
        let mut random = length as u64;
        let modulus = 1_usize
            .checked_shl(usize::BITS - length.leading_zeros())
            .unwrap_or(0);
        for index in a + length / 4 * 2 - 1..=a + length / 4 * 2 + 1 {
            random ^= random << 13;
            random ^= random >> 7;
            random ^= random << 17;
            let mut other = random as usize & modulus.wrapping_sub(1);
            if other >= length {
                other -= length;
            }
            data.swap(index, a + other);
        }
    }
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum Hint {
    Unknown,
    Increasing,
    Decreasing,
}
fn choose_pivot<T>(data: &[T], a: usize, b: usize, compare: Compare<'_, T>) -> (usize, Hint) {
    let length = b - a;
    let mut swaps = 0;
    let (mut i, mut j, mut k) = (a + length / 4, a + length / 4 * 2, a + length / 4 * 3);
    if length >= 8 {
        if length >= 50 {
            i = median(data, i - 1, i, i + 1, &mut swaps, compare);
            j = median(data, j - 1, j, j + 1, &mut swaps, compare);
            k = median(data, k - 1, k, k + 1, &mut swaps, compare);
        }
        j = median(data, i, j, k, &mut swaps, compare);
    }
    (
        j,
        match swaps {
            0 => Hint::Increasing,
            12 => Hint::Decreasing,
            _ => Hint::Unknown,
        },
    )
}
fn order2<T>(
    data: &[T],
    a: usize,
    b: usize,
    swaps: &mut usize,
    compare: Compare<'_, T>,
) -> (usize, usize) {
    if compare(&data[b], &data[a]).is_lt() {
        *swaps += 1;
        (b, a)
    } else {
        (a, b)
    }
}
fn median<T>(
    data: &[T],
    a: usize,
    b: usize,
    c: usize,
    swaps: &mut usize,
    compare: Compare<'_, T>,
) -> usize {
    let (a, b) = order2(data, a, b, swaps, compare);
    let (b, _) = order2(data, b, c, swaps, compare);
    let (_, b) = order2(data, a, b, swaps, compare);
    b
}

#[cfg(test)]
mod tests;
