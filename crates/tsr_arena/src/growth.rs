//! Growth policy for stores whose reserved capacity is charged as storage: a
//! quarter at a time instead of doubling, so a table of n records reserves at
//! most about n/4 unused slots where doubling reserved up to n.
/// Push with quarter-step growth (never fewer than eight slots).
#[inline]
pub fn push_frugal<T>(vec: &mut Vec<T>, value: T) {
    if vec.len() == vec.capacity() {
        let extra = (vec.capacity() / 4).max(8);
        vec.reserve_exact(extra);
    }
    vec.push(value);
}
