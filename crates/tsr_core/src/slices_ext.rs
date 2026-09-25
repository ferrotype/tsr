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
