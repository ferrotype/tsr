//! Optional requested-layout and allocator-usable allocation accounting.
//! Neither counter is resident memory or allocator page/metadata overhead.
use std::collections::BTreeMap;
use tsr_arena::Counts;

#[cfg(feature = "allocation-metrics")]
mod instrumented {
    use mimalloc::MiMalloc;
    use std::alloc::{GlobalAlloc, Layout};
    use std::sync::atomic::{AtomicU64, Ordering};

    pub struct Allocator {
        live: AtomicU64,
        total: AtomicU64,
        peak: AtomicU64,
        allocs: AtomicU64,
        frees: AtomicU64,
        live_usable: AtomicU64,
        total_usable: AtomicU64,
        peak_usable: AtomicU64,
    }
    impl Allocator {
        pub const fn new() -> Self {
            Self {
                live: AtomicU64::new(0),
                total: AtomicU64::new(0),
                peak: AtomicU64::new(0),
                allocs: AtomicU64::new(0),
                frees: AtomicU64::new(0),
                live_usable: AtomicU64::new(0),
                total_usable: AtomicU64::new(0),
                peak_usable: AtomicU64::new(0),
            }
        }
        fn allocated(&self, bytes: usize, usable: usize) {
            let bytes = bytes as u64;
            let usable = usable as u64;
            let live = self.live.fetch_add(bytes, Ordering::Relaxed) + bytes;
            self.total.fetch_add(bytes, Ordering::Relaxed);
            self.allocs.fetch_add(1, Ordering::Relaxed);
            self.peak.fetch_max(live, Ordering::Relaxed);
            let live_usable = self.live_usable.fetch_add(usable, Ordering::Relaxed) + usable;
            self.total_usable.fetch_add(usable, Ordering::Relaxed);
            self.peak_usable.fetch_max(live_usable, Ordering::Relaxed);
        }
        fn freed(&self, bytes: usize, usable: usize) {
            self.live.fetch_sub(bytes as u64, Ordering::Relaxed);
            self.live_usable.fetch_sub(usable as u64, Ordering::Relaxed);
            self.frees.fetch_add(1, Ordering::Relaxed);
        }
        pub fn values(&self) -> [u64; 8] {
            [
                self.live.load(Ordering::Relaxed),
                self.total.load(Ordering::Relaxed),
                self.peak.load(Ordering::Relaxed),
                self.allocs.load(Ordering::Relaxed),
                self.frees.load(Ordering::Relaxed),
                self.live_usable.load(Ordering::Relaxed),
                self.total_usable.load(Ordering::Relaxed),
                self.peak_usable.load(Ordering::Relaxed),
            ]
        }
    }

    // SAFETY: each operation delegates the exact pointer/layout contract to
    // mimalloc. Accounting uses allocation-free atomics and never changes a
    // pointer, allocation size or failure result.
    unsafe impl GlobalAlloc for Allocator {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            // SAFETY: GlobalAlloc caller provides a valid layout.
            let pointer = unsafe { MiMalloc.alloc(layout) };
            if !pointer.is_null() {
                // SAFETY: a successful mimalloc allocation is still live.
                self.allocated(layout.size(), unsafe { MiMalloc.usable_size(pointer) });
            }
            pointer
        }
        unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
            // SAFETY: GlobalAlloc caller provides a valid layout.
            let pointer = unsafe { MiMalloc.alloc_zeroed(layout) };
            if !pointer.is_null() {
                // SAFETY: a successful mimalloc allocation is still live.
                self.allocated(layout.size(), unsafe { MiMalloc.usable_size(pointer) });
            }
            pointer
        }
        unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
            // SAFETY: caller supplies a live mimalloc allocation. Query before free.
            let usable = unsafe { MiMalloc.usable_size(pointer) };
            // SAFETY: caller supplies the allocation's matching layout.
            unsafe {
                MiMalloc.dealloc(pointer, layout);
            }
            self.freed(layout.size(), usable);
        }
        unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
            // SAFETY: the old pointer is live until realloc succeeds.
            let usable = unsafe { MiMalloc.usable_size(pointer) };
            // SAFETY: caller supplies matching old layout and valid new size.
            let result = unsafe { MiMalloc.realloc(pointer, layout, new_size) };
            if !result.is_null() {
                self.freed(layout.size(), usable);
                // SAFETY: successful realloc returns a live mimalloc allocation.
                self.allocated(new_size, unsafe { MiMalloc.usable_size(result) });
            }
            result
        }
    }
    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn usable_bytes_follow_live_blocks_through_growth_shrink_and_free() {
            // A local counter excludes allocations made by other test threads.
            let allocator = Allocator::new();
            let mut layout = Layout::from_size_align(17, 64).unwrap();
            // SAFETY: every live pointer below uses its current matching layout.
            unsafe {
                let mut pointer = allocator.alloc_zeroed(layout);
                assert!(!pointer.is_null());
                let mut total_usable = MiMalloc.usable_size(pointer) as u64;
                assert!(total_usable >= 17);
                assert_eq!(allocator.values()[5], total_usable);
                assert_eq!(*pointer, 0);
                for new_size in [513, 7] {
                    pointer = allocator.realloc(pointer, layout, new_size);
                    assert!(!pointer.is_null());
                    layout = Layout::from_size_align(new_size, 64).unwrap();
                    let usable = MiMalloc.usable_size(pointer) as u64;
                    total_usable += usable;
                    let counts = allocator.values();
                    assert_eq!(counts[0], new_size as u64);
                    assert_eq!(counts[5], usable);
                    assert_eq!(counts[6], total_usable);
                    assert!(counts[7] >= usable);
                }
                allocator.dealloc(pointer, layout);
            }
            let counts = allocator.values();
            assert_eq!(counts[0], 0);
            assert_eq!(counts[5], 0);
            assert_eq!(counts[3], counts[4]);
        }
    }
}

#[cfg(not(feature = "allocation-metrics"))]
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

#[cfg(feature = "allocation-metrics")]
#[global_allocator]
static GLOBAL: instrumented::Allocator = instrumented::Allocator::new();

pub fn snapshot(counts: Counts) -> BTreeMap<&'static str, String> {
    // Sample before allocating the metric response itself.
    #[cfg(feature = "allocation-metrics")]
    let values = GLOBAL.values();
    let mut output = BTreeMap::from([
        ("authority", "rust_storage_owners_not_heap_bytes".into()),
        ("allocator", "mimalloc-0.1.48".into()),
        (
            "allocation_instrumented",
            cfg!(feature = "allocation-metrics").to_string(),
        ),
        ("tracked_owners", counts.owners.to_string()),
        ("tracked_allocations", counts.allocations.to_string()),
    ]);
    let metrics: Vec<(&'static str, String)> = {
        #[cfg(feature = "allocation-metrics")]
        {
            [
                "live_requested_bytes",
                "total_requested_bytes",
                "peak_requested_bytes",
                "allocation_events",
                "deallocation_events",
                "live_usable_bytes",
                "total_usable_bytes",
                "peak_usable_bytes",
            ]
            .into_iter()
            .zip(values)
            .map(|(name, value)| (name, value.to_string()))
            .collect()
        }
        #[cfg(not(feature = "allocation-metrics"))]
        {
            Vec::new()
        }
    };
    output.extend(metrics);
    output
}
