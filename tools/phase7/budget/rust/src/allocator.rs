//! Optional requested-allocation accounting. This is not allocator usable
//! size, resident memory, or Go runtime heap accounting.
use std::collections::BTreeMap;
use tsr_arena::Counts;

#[cfg(feature = "allocation-metrics")]
mod instrumented {
    use mimalloc::MiMalloc;
    use std::alloc::{GlobalAlloc, Layout};
    use std::sync::atomic::{AtomicU64, Ordering};

    pub struct Allocator;
    static LIVE: AtomicU64 = AtomicU64::new(0);
    static TOTAL: AtomicU64 = AtomicU64::new(0);
    static PEAK: AtomicU64 = AtomicU64::new(0);
    static ALLOCS: AtomicU64 = AtomicU64::new(0);
    static FREES: AtomicU64 = AtomicU64::new(0);

    fn allocated(bytes: usize) {
        let bytes = bytes as u64;
        let live = LIVE.fetch_add(bytes, Ordering::Relaxed) + bytes;
        TOTAL.fetch_add(bytes, Ordering::Relaxed);
        ALLOCS.fetch_add(1, Ordering::Relaxed);
        PEAK.fetch_max(live, Ordering::Relaxed);
    }
    fn freed(bytes: usize) {
        LIVE.fetch_sub(bytes as u64, Ordering::Relaxed);
        FREES.fetch_add(1, Ordering::Relaxed);
    }

    // SAFETY: each operation delegates the exact pointer/layout contract to
    // mimalloc. Accounting uses allocation-free atomics and never changes a
    // pointer, allocation size or failure result.
    unsafe impl GlobalAlloc for Allocator {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            // SAFETY: GlobalAlloc caller provides a valid layout.
            let pointer = unsafe { MiMalloc.alloc(layout) };
            if !pointer.is_null() {
                allocated(layout.size());
            }
            pointer
        }
        unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
            // SAFETY: GlobalAlloc caller provides a valid layout.
            let pointer = unsafe { MiMalloc.alloc_zeroed(layout) };
            if !pointer.is_null() {
                allocated(layout.size());
            }
            pointer
        }
        unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
            // SAFETY: caller supplies the allocation's matching layout.
            unsafe {
                MiMalloc.dealloc(pointer, layout);
            }
            freed(layout.size());
        }
        unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
            // SAFETY: caller supplies matching old layout and valid new size.
            let result = unsafe { MiMalloc.realloc(pointer, layout, new_size) };
            if !result.is_null() {
                freed(layout.size());
                allocated(new_size);
            }
            result
        }
    }
    pub fn values() -> [u64; 5] {
        [
            LIVE.load(Ordering::Relaxed),
            TOTAL.load(Ordering::Relaxed),
            PEAK.load(Ordering::Relaxed),
            ALLOCS.load(Ordering::Relaxed),
            FREES.load(Ordering::Relaxed),
        ]
    }
}

#[cfg(not(feature = "allocation-metrics"))]
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

#[cfg(feature = "allocation-metrics")]
#[global_allocator]
static GLOBAL: instrumented::Allocator = instrumented::Allocator;

pub fn snapshot(counts: Counts) -> BTreeMap<&'static str, String> {
    // Sample before allocating the metric response itself.
    #[cfg(feature = "allocation-metrics")]
    let values = instrumented::values();
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
