//! CLI allocator observations used by --diagnostics. These counters report
//! allocation requests/live requested bytes, not resident memory or Go arenas.
#![allow(unsafe_code)]
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicU64, Ordering};
static ALLOCATED: AtomicU64 = AtomicU64::new(0);
static LIVE: AtomicU64 = AtomicU64::new(0);
static COUNT: AtomicU64 = AtomicU64::new(0);
pub struct CountingAllocator;
fn add(size: usize) {
    ALLOCATED.fetch_add(size as u64, Ordering::Relaxed);
    LIVE.fetch_add(size as u64, Ordering::Relaxed);
    COUNT.fetch_add(1, Ordering::Relaxed);
}
// SAFETY: every pointer/layout pair is forwarded unchanged to System. Counter
// updates never allocate or panic, and failed allocations leave live bytes alone.
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: forwarded GlobalAlloc caller contract.
        let pointer = unsafe { System.alloc(layout) };
        if !pointer.is_null() {
            add(layout.size());
        }
        pointer
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        // SAFETY: forwarded GlobalAlloc caller contract.
        let pointer = unsafe { System.alloc_zeroed(layout) };
        if !pointer.is_null() {
            add(layout.size());
        }
        pointer
    }
    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        // SAFETY: forwarded GlobalAlloc caller contract.
        unsafe { System.dealloc(pointer, layout) };
        LIVE.fetch_sub(layout.size() as u64, Ordering::Relaxed);
    }
    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        // SAFETY: forwarded GlobalAlloc caller contract.
        let result = unsafe { System.realloc(pointer, layout, new_size) };
        if !result.is_null() {
            add(new_size);
            LIVE.fetch_sub(layout.size() as u64, Ordering::Relaxed);
        }
        result
    }
}
pub fn snapshot() -> tsr_tsc::MemoryStatistics {
    tsr_tsc::MemoryStatistics {
        allocated_bytes: ALLOCATED.load(Ordering::Relaxed),
        allocation_count: COUNT.load(Ordering::Relaxed),
        live_bytes: LIVE.load(Ordering::Relaxed),
    }
}
