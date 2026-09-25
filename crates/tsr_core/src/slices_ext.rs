//! Slice helpers of `tsc/internal/core/core.go` beyond `slices.rs`.
//!
//! Witnessed by the `core` group of the Phase 1 operation tables
//! (`docs/PHASE1-mutation-witnesses.md`, section 9).

/// Go `int` arguments, clamped as at the pin. The only callers
/// (`printer/emitcontext.go:314-333`) assign the result back, so the value is
/// the contract and an unchanged input is returned as a copy.
/// port: tsc/internal/core/core.go:Splice
pub fn splice<T: Clone>(s1: &[T], start: i64, delete_count: i64, items: &[T]) -> Vec<T> {
    let len = i64::try_from(s1.len()).expect("slice length fits in a Go int");
    let mut start = if start < 0 { len + start } else { start };
    if start < 0 {
        start = 0;
    }
    if start > len {
        start = len;
    }
    let mut delete_count = delete_count;
    if delete_count < 0 {
        delete_count = 0;
    }
    let end = (start + delete_count.max(0)).min(len);
    if start == end && items.is_empty() {
        return s1.to_vec();
    }
    let start = usize::try_from(start).expect("clamped to the slice");
    let end = usize::try_from(end).expect("clamped to the slice");
    let mut result = Vec::with_capacity(s1.len() - (end - start) + items.len());
    result.extend_from_slice(&s1[..start]);
    result.extend_from_slice(items);
    result.extend_from_slice(&s1[end..]);
    result
}

use std::borrow::Cow;
use std::collections::HashMap;
use std::hash::Hash;

/// Go's `[]*S` with nil elements as `None`; a nil element panics with `msg`.
/// port: tsc/internal/core/core.go:CheckEachDefined
pub fn check_each_defined<'a, S>(s: &'a [Option<S>], msg: &str) -> &'a [Option<S>] {
    for value in s {
        if value.is_none() {
            panic!("{msg}");
        }
    }
    s
}

/// Go's `CopyMapInto`: a nil `dst` is a clone of `src`, else `src` is copied
/// into `dst`. The port marker is on the nil test, a site the mutation splicer
/// can negate (a map has no replacement value).
pub fn copy_map_into<K: Clone + Eq + Hash, V: Clone>(
    dst: Option<HashMap<K, V>>,
    src: &HashMap<K, V>,
) -> HashMap<K, V> {
    // port: tsc/internal/core/core.go:CopyMapInto
    if dst.is_none() {
        return src.clone();
    }
    let mut dst = dst.unwrap_or_default();
    dst.extend(src.iter().map(|(key, value)| (key.clone(), value.clone())));
    dst
}

/// Go's `comparableValuesEqual`, the equality `DiffMaps` passes.
fn comparable_values_equal<V: PartialEq>(left: &V, right: &V) -> bool {
    left == right
}

/// port: tsc/internal/core/core.go:DiffMaps
pub fn diff_maps<K: Eq + Hash, V: PartialEq>(
    m1: &HashMap<K, V>,
    m2: &HashMap<K, V>,
    on_added: Option<&mut dyn FnMut(&K, &V)>,
    on_removed: Option<&mut dyn FnMut(&K, &V)>,
    on_changed: Option<&mut dyn FnMut(&K, &V, &V)>,
) {
    diff_maps_func(
        m1,
        m2,
        comparable_values_equal,
        on_added,
        on_removed,
        on_changed,
    );
}

/// Go ranges over maps in random order, so the callbacks' order is no
/// contract. A nil `onRemoved` panics when a key was removed and `onChanged`
/// is not nil.
/// port: tsc/internal/core/core.go:DiffMapsFunc
pub fn diff_maps_func<K: Eq + Hash, V1, V2>(
    m1: &HashMap<K, V1>,
    m2: &HashMap<K, V2>,
    equal_values: fn(&V1, &V2) -> bool,
    on_added: Option<&mut dyn FnMut(&K, &V2)>,
    on_removed: Option<&mut dyn FnMut(&K, &V1)>,
    on_changed: Option<&mut dyn FnMut(&K, &V1, &V2)>,
) {
    if let Some(on_added) = on_added {
        for (key, v2) in m2 {
            if !m1.contains_key(key) {
                on_added(key, v2);
            }
        }
    }
    if on_changed.is_none() && on_removed.is_none() {
        return;
    }
    let (mut on_removed, mut on_changed) = (on_removed, on_changed);
    for (key, v1) in m1 {
        if let Some(v2) = m2.get(key) {
            if let Some(on_changed) = on_changed.as_deref_mut() {
                if !equal_values(v1, v2) {
                    on_changed(key, v1, v2);
                }
            }
        } else {
            on_removed
                .as_deref_mut()
                .expect("runtime error: invalid memory address or nil pointer dereference")(
                key, v1,
            );
        }
    }
}

/// Go's `ElementOrNil` for a nonnegative index (a negative one panics).
pub fn element_or_nil<T: Clone + Default>(slice: &[T], index: i64) -> T {
    let index = usize::try_from(index).expect("runtime error: index out of range");
    // port: tsc/internal/core/core.go:ElementOrNil
    if index < slice.len() {
        return slice[index].clone();
    }
    T::default()
}

/// port: tsc/internal/core/core.go:FindLastIndex
pub fn find_last_index<T>(slice: &[T], f: impl Fn(&T) -> bool) -> i64 {
    for index in (0..slice.len()).rev() {
        if f(&slice[index]) {
            return i64::try_from(index).expect("slice index fits in a Go int");
        }
    }
    -1
}

/// Go's `FirstNonNil`: the first mapped value that is not the zero value.
pub fn first_non_nil<T, U: PartialEq + Default>(slice: &[T], f: impl Fn(&T) -> U) -> U {
    for value in slice {
        let mapped = f(value);
        // port: tsc/internal/core/core.go:FirstNonNil
        if mapped != U::default() {
            return mapped;
        }
    }
    U::default()
}

/// Go's `FirstNonZero`.
pub fn first_non_zero<T: PartialEq + Default + Clone>(values: &[T]) -> T {
    let zero = T::default();
    for value in values {
        // port: tsc/internal/core/core.go:FirstNonZero
        if *value != zero {
            return value.clone();
        }
    }
    zero
}

/// Go's `FirstOrNilSeq`: the first value of a (nil-able) sequence.
pub fn first_or_nil_seq<T: Default>(seq: Option<impl IntoIterator<Item = T>>) -> T {
    // port: tsc/internal/core/core.go:FirstOrNilSeq
    if seq.is_some() {
        if let Some(seq) = seq {
            if let Some(value) = seq.into_iter().next() {
                return value;
            }
        }
    }
    T::default()
}

/// Go slices `s[startIndex:]`, which panics past the end.
/// port: tsc/internal/core/core.go:IndexAfter
pub fn index_after(s: &[u8], pattern: &[u8], start_index: i64) -> i64 {
    let start = usize::try_from(start_index).expect("runtime error: slice bounds out of range");
    let rest = &s[start..];
    let matched = if pattern.is_empty() {
        Some(0)
    } else {
        rest.windows(pattern.len())
            .position(|window| window == pattern)
    };
    match matched {
        None => -1,
        Some(matched) => i64::try_from(matched).expect("fits") + start_index,
    }
}

/// port: tsc/internal/core/core.go:MapNonNil
pub fn map_non_nil<T, U: PartialEq + Default>(slice: &[T], f: impl Fn(&T) -> U) -> Vec<U> {
    let mut result = Vec::new();
    for value in slice {
        let mapped = f(value);
        if mapped != U::default() {
            result.push(mapped);
        }
    }
    result
}

/// port: tsc/internal/core/core.go:MinAllFunc
pub fn min_all_func<T: Clone>(xs: &[T], cmp: impl Fn(&T, &T) -> i64) -> Vec<T> {
    let Some(first) = xs.first() else {
        return Vec::new();
    };
    let mut minimum = first.clone();
    let mut mins = vec![minimum.clone()];
    for x in &xs[1..] {
        let c = cmp(x, &minimum);
        if c < 0 {
            minimum = x.clone();
            mins.clear();
            mins.push(x.clone());
        } else if c == 0 {
            mins.push(x.clone());
        }
    }
    mins
}

/// Go's `Or`: the returned predicate is true when any of `funcs` is.
pub struct Or<'a, T> {
    funcs: Vec<&'a dyn Fn(&T) -> bool>,
}

/// Go's `Or`, whose work is the returned closure ([`Or::call`]).
pub fn or<T>(funcs: Vec<&dyn Fn(&T) -> bool>) -> Or<'_, T> {
    Or { funcs }
}

impl<T> Or<'_, T> {
    /// The closure `Or` returns.
    /// port: tsc/internal/core/core.go:Or
    pub fn call(&self, input: &T) -> bool {
        for f in &self.funcs {
            if f(input) {
                return true;
            }
        }
        false
    }
}

/// Go indexes `result[i]`, which panics out of range.
/// port: tsc/internal/core/core.go:ReplaceElement
pub fn replace_element<T: Clone>(slice: &[T], i: i64, t: T) -> Vec<T> {
    let mut result = slice.to_vec();
    result[usize::try_from(i).expect("runtime error: index out of range")] = t;
    result
}

/// Go returns the input slice itself when `f` changes nothing.
/// port: tsc/internal/core/core.go:SameMapIndex
pub fn same_map_index<T: PartialEq + Clone>(
    slice: &[T],
    f: impl Fn(&T, usize) -> T,
) -> Cow<'_, [T]> {
    for (i, value) in slice.iter().enumerate() {
        let mapped = f(value, i);
        if mapped != *value {
            let mut result = Vec::with_capacity(slice.len());
            result.extend_from_slice(&slice[..i]);
            result.push(mapped);
            for (j, value) in slice.iter().enumerate().skip(i + 1) {
                result.push(f(value, j));
            }
            return Cow::Owned(result);
        }
    }
    Cow::Borrowed(slice)
}

/// port: tsc/internal/core/core.go:UnorderedEqual
pub fn unordered_equal<T: Eq + Hash>(s1: &[T], s2: &[T]) -> bool {
    if s1.len() != s2.len() {
        return false;
    }
    let mut counts: HashMap<&T, i64> = HashMap::new();
    for value in s1 {
        *counts.entry(value).or_insert(0) += 1;
    }
    for value in s2 {
        let count = counts.entry(value).or_insert(0);
        *count -= 1;
        if *count < 0 {
            return false;
        }
    }
    true
}
