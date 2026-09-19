//! Formatter descent follows ADR 0011's native growth policy. Entry points
//! also work on caller-owned threads, whose initial stack may be small.

const RED_ZONE: usize = 128 * 1024;
const SEGMENT: usize = 2 * 1024 * 1024;

#[inline]
pub(crate) fn guarded<T>(operation: impl FnOnce() -> T) -> T {
    #[cfg(test)]
    let before = stacker::remaining_stack();
    stacker::maybe_grow(RED_ZONE, SEGMENT, || {
        #[cfg(test)]
        if before
            .zip(stacker::remaining_stack())
            .is_some_and(|(before, after)| after > before.saturating_add(RED_ZONE))
        {
            GROWTHS.set(GROWTHS.get() + 1);
        }
        operation()
    })
}

#[cfg(test)]
thread_local! {
    static GROWTHS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
pub(crate) fn take_growths() -> usize {
    GROWTHS.replace(0)
}
