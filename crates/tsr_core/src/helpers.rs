//! Slice-header compatibility helpers; see docs/PHASE1-aliasing-audit.md.
//! Identity-returning operations intentionally retain mutable backing. Ordinary
//! production transforms should use borrows/owned results and an explicit
//! changed flag where those preserve their caller contract.
use crate::slices::{GoSliceElement, SharedSlice};

/// port: tsc/internal/core/core.go:Filter
pub fn filter<T: GoSliceElement>(
    slice: &SharedSlice<T>,
    mut f: impl FnMut(&T) -> bool,
) -> SharedSlice<T> {
    for (i, value) in slice.iter().enumerate() {
        if !f(&value) {
            let mut result = SharedSlice::copied(&slice.read()[..i]);
            for value in slice.iter().skip(i + 1) {
                if f(&value) {
                    result.push(value);
                }
            }
            return result;
        }
    }
    slice.clone()
}
/// port: tsc/internal/core/core.go:Map
pub fn map<T: Clone, U>(slice: &SharedSlice<T>, mut f: impl FnMut(&T) -> U) -> SharedSlice<U> {
    if slice.is_nil() {
        SharedSlice::default()
    } else {
        SharedSlice::from_vec(slice.iter().map(|value| f(&value)).collect())
    }
}
/// port: tsc/internal/core/core.go:MapIndex
pub fn map_index<T: Clone, U>(
    slice: &SharedSlice<T>,
    mut f: impl FnMut(&T, usize) -> U,
) -> SharedSlice<U> {
    if slice.is_nil() {
        SharedSlice::default()
    } else {
        SharedSlice::from_vec(slice.iter().enumerate().map(|(i, t)| f(&t, i)).collect())
    }
}
/// port: tsc/internal/core/core.go:MapFiltered
pub fn map_filtered<T: Clone, U: GoSliceElement>(
    slice: &SharedSlice<T>,
    mut f: impl FnMut(&T) -> Option<U>,
) -> SharedSlice<U> {
    let mut result = SharedSlice::default();
    for value in slice {
        if let Some(value) = f(&value) {
            result.push(value);
        }
    }
    result
}
/// port: tsc/internal/core/core.go:FlatMap
pub fn flat_map<T: Clone, U: GoSliceElement>(
    slice: &SharedSlice<T>,
    mut f: impl FnMut(&T) -> Vec<U>,
) -> SharedSlice<U> {
    let mut result = SharedSlice::default();
    for value in slice {
        result.extend_from_slice(&f(&value));
    }
    result
}
/// port: tsc/internal/core/core.go:Flatten
pub fn flatten<T: GoSliceElement>(slices: &[SharedSlice<T>]) -> SharedSlice<T> {
    let mut result = SharedSlice::default();
    for slice in slices {
        result.extend_from_slice(&slice.read());
    }
    result
}
/// port: tsc/internal/core/core.go:Concatenate
pub fn concatenate<T: GoSliceElement>(
    left: &SharedSlice<T>,
    right: &SharedSlice<T>,
) -> SharedSlice<T> {
    if right.is_empty() {
        left.clone()
    } else if left.is_empty() {
        right.clone()
    } else {
        let mut values = Vec::with_capacity(left.len() + right.len());
        values.extend_from_slice(&left.read());
        values.extend_from_slice(&right.read());
        SharedSlice::copied(&values)
    }
}
/// port: tsc/internal/core/core.go:Deduplicate
pub fn deduplicate<T: GoSliceElement + Eq>(slice: &SharedSlice<T>) -> SharedSlice<T> {
    let values = slice.read();
    for (i, value) in values.iter().enumerate() {
        if values[..i].contains(value) {
            let mut result = SharedSlice::copied(&values[..i]);
            for value in &values[i + 1..] {
                if !result.read().contains(value) {
                    result.push(value.clone());
                }
            }
            return result;
        }
    }
    slice.clone()
}
/// port: tsc/internal/core/core.go:AppendIfUnique
pub fn append_if_unique<T: GoSliceElement + Eq>(
    slice: &SharedSlice<T>,
    value: T,
) -> SharedSlice<T> {
    let present = slice.read().contains(&value);
    let mut result = slice.clone();
    if !present {
        result.push(value);
    }
    result
}
/// port: tsc/internal/core/core.go:SameMap
pub fn same_map<T: Clone + Eq>(
    slice: &SharedSlice<T>,
    mut f: impl FnMut(&T) -> T,
) -> SharedSlice<T> {
    for (i, value) in slice.iter().enumerate() {
        let mapped = f(&value);
        if mapped != value {
            let mut result = Vec::with_capacity(slice.len());
            result.extend_from_slice(&slice.read()[..i]);
            result.push(mapped);
            for value in slice.iter().skip(i + 1) {
                result.push(f(&value));
            }
            return SharedSlice::from_vec(result);
        }
    }
    slice.clone()
}
/// port: tsc/internal/core/core.go:Some
pub fn some<T>(slice: &[T], f: impl FnMut(&T) -> bool) -> bool {
    slice.iter().any(f)
}
/// port: tsc/internal/core/core.go:Find
pub fn find<T: Clone + Default>(slice: &[T], mut f: impl FnMut(&T) -> bool) -> T {
    slice.iter().find(|v| f(v)).cloned().unwrap_or_default()
}
/// port: tsc/internal/core/core.go:FindLast
pub fn find_last<T: Clone + Default>(slice: &[T], mut f: impl FnMut(&T) -> bool) -> T {
    slice
        .iter()
        .rev()
        .find(|v| f(v))
        .cloned()
        .unwrap_or_default()
}
/// port: tsc/internal/core/core.go:FindIndex
pub fn find_index<T>(slice: &[T], f: impl FnMut(&T) -> bool) -> isize {
    slice.iter().position(f).map_or(-1, |i| i as isize)
}
/// port: tsc/internal/core/core.go:FirstOrNil
pub fn first_or_zero<T: Clone + Default>(slice: &[T]) -> T {
    slice.first().cloned().unwrap_or_default()
}
/// port: tsc/internal/core/core.go:LastOrNil
pub fn last_or_zero<T: Clone + Default>(slice: &[T]) -> T {
    slice.last().cloned().unwrap_or_default()
}
/// T is the caller's pointer/retained handle; it is moved, never deep-copied.
/// port: tsc/internal/core/core.go:SingleElementSlice
pub fn single_element_slice<T>(element: Option<T>) -> Option<Vec<T>> {
    element.map(|e| vec![e])
}
/// A failed initializer stays installed and is retried. The returned function
/// is deliberately FnMut: this memoizer does not promise concurrent calls.
/// port: tsc/internal/core/core.go:Memoize
pub fn memoize<T: Clone>(create: impl FnMut() -> T) -> impl FnMut() -> T {
    let mut create = Some(create);
    let mut value = None;
    move || {
        if let Some(create_fn) = &mut create {
            value = Some(create_fn());
            create = None;
        }
        value.as_ref().unwrap().clone()
    }
}
/// Panic with the original error value rather than its Debug/Display text.
/// port: tsc/internal/core/core.go:Must
pub fn must<T, E: Send + 'static>(result: Result<T, E>) -> T {
    match result {
        Ok(value) => value,
        Err(error) => std::panic::panic_any(error),
    }
}
/// port: tsc/internal/core/core.go:OrElse
pub fn or_else<T: Eq + Default>(value: T, fallback: T) -> T {
    if value == T::default() {
        fallback
    } else {
        value
    }
}
/// port: tsc/internal/core/core.go:IfElse
pub fn if_else<T>(condition: bool, when_true: T, when_false: T) -> T {
    if condition {
        when_true
    } else {
        when_false
    }
}
/// Nil sequences are skipped; laziness propagates the caller's early stop.
/// port: tsc/internal/core/core.go:ConcatenateSeq
pub fn concatenate_seq<T, I: IntoIterator<Item = T>>(
    seqs: impl IntoIterator<Item = Option<I>>,
) -> impl Iterator<Item = T> {
    seqs.into_iter().flatten().flat_map(IntoIterator::into_iter)
}
/// Pointer-bearing values must implement address equality, as ordinary Rust
/// raw pointers do. This helper does not replace that with pointee equality.
/// port: tsc/internal/core/core.go:comparableValuesEqual
pub fn comparable_values_equal<T: PartialEq>(a: &T, b: &T) -> bool {
    a == b
}
