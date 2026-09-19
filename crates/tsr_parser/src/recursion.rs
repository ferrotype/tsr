//! Native stack growth at recursive parser boundaries.

/// All grammar guards use the same production reservation. Test observations
/// measure actual remaining stack on each side of the guard, without changing
/// the guard threshold, segment size, or the parser's control flow.
#[inline]
pub(crate) fn guarded<T>(operation: impl FnOnce() -> T) -> T {
    #[cfg(test)]
    let before = stacker::remaining_stack();
    stacker::maybe_grow(crate::STACK_RED_ZONE, crate::STACK_SEGMENT, || {
        #[cfg(test)]
        OBSERVATIONS.with(|observations| {
            let mut value = observations.get();
            value.entries += 1;
            if before
                .zip(stacker::remaining_stack())
                .is_some_and(|(before, after)| after > before.saturating_add(crate::STACK_RED_ZONE))
            {
                value.growths += 1;
            }
            observations.set(value);
        });
        operation()
    })
}

#[cfg(test)]
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Observations {
    pub(crate) entries: usize,
    pub(crate) growths: usize,
}
#[cfg(test)]
thread_local! {
    static OBSERVATIONS: std::cell::Cell<Observations> =
        const { std::cell::Cell::new(Observations { entries: 0, growths: 0 }) };
}
#[cfg(test)]
pub(crate) fn take_observations() -> Observations {
    OBSERVATIONS.replace(Observations::default())
}
