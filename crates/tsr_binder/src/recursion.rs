//! Native binder recursion uses the accepted reserved-stack and growth policy.
const RED_ZONE: usize = 128 * 1024;
const SEGMENT: usize = 2 * 1024 * 1024;
#[inline]
pub(crate) fn guarded<T>(operation: impl FnOnce() -> T) -> T {
    #[cfg(test)]
    let before = stacker::remaining_stack();
    stacker::maybe_grow(RED_ZONE, SEGMENT, || {
        #[cfg(test)]
        OBSERVATIONS.with(|cell| {
            let mut value = cell.get();
            value.entries += 1;
            if before
                .zip(stacker::remaining_stack())
                .is_some_and(|(before, after)| after > before.saturating_add(RED_ZONE))
            {
                value.growths += 1;
            }
            cell.set(value);
            assert!(
                !(value.growths > 0 && PANIC_AFTER_GROWTH.replace(false)),
                "S07 injected binder failure after stack growth"
            );
        });
        operation()
    })
}
#[cfg(test)]
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Observations {
    pub entries: usize,
    pub growths: usize,
    pub binary_nodes: usize,
    pub binary_frames: usize,
}
#[cfg(test)]
thread_local! {static OBSERVATIONS:std::cell::Cell<Observations>=const {std::cell::Cell::new(Observations{entries:0,growths:0,binary_nodes:0,binary_frames:0})};}
#[cfg(test)]
thread_local! {static PANIC_AFTER_GROWTH:std::cell::Cell<bool>=const {std::cell::Cell::new(false)};}
#[cfg(test)]
pub(crate) fn panic_after_next_growth() {
    PANIC_AFTER_GROWTH.set(true);
}
#[cfg(test)]
pub(crate) fn take_observations() -> Observations {
    OBSERVATIONS.replace(Observations::default())
}
#[cfg(test)]
pub(crate) fn binary_frame(frames: usize) {
    OBSERVATIONS.with(|cell| {
        let mut value = cell.get();
        value.binary_nodes += 1;
        value.binary_frames = value.binary_frames.max(frames);
        cell.set(value);
    });
}
