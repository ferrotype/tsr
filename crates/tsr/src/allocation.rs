//! CLI allocator observations used by --diagnostics. These counters report
//! allocation requests/live requested bytes, not resident memory or Go arenas.
//!
//! mimalloc serves the allocations, as the benchmark binary's does. The
//! counters are sharded: each thread adds to its own cache-line-sized shard,
//! chosen once per thread, so counting never contends between threads; a
//! snapshot sums the shards. Live bytes freed on another thread than they
//! were allocated on leave one shard's count below zero and another's above;
//! the wrapping sum is still exact.
#![allow(unsafe_code)]
use std::alloc::{GlobalAlloc, Layout};
use std::cell::Cell;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

const SHARDS: usize = 64;

#[repr(align(128))]
struct Shard {
    allocated: AtomicU64,
    live: AtomicU64,
    count: AtomicU64,
}

#[allow(
    clippy::declare_interior_mutable_const,
    reason = "the array initializer"
)]
const EMPTY: Shard = Shard {
    allocated: AtomicU64::new(0),
    live: AtomicU64::new(0),
    count: AtomicU64::new(0),
};
static SHARDS_: [Shard; SHARDS] = [EMPTY; SHARDS];
static NEXT_SHARD: AtomicUsize = AtomicUsize::new(0);

thread_local! {
    // A const-initialized cell without a destructor: the allocator can read it
    // at any point of a thread's life, including during thread exit.
    static SHARD: Cell<usize> = const { Cell::new(usize::MAX) };
}

fn shard() -> &'static Shard {
    let index = SHARD.with(|cell| {
        let index = cell.get();
        if index != usize::MAX {
            return index;
        }
        let index = NEXT_SHARD.fetch_add(1, Ordering::Relaxed) % SHARDS;
        cell.set(index);
        index
    });
    &SHARDS_[index]
}

fn add(size: usize) {
    let shard = shard();
    shard.allocated.fetch_add(size as u64, Ordering::Relaxed);
    shard.live.fetch_add(size as u64, Ordering::Relaxed);
    shard.count.fetch_add(1, Ordering::Relaxed);
}

fn release(size: usize) {
    shard().live.fetch_sub(size as u64, Ordering::Relaxed);
}

pub struct CountingAllocator;

// SAFETY: every pointer/layout pair is forwarded unchanged to mimalloc. Counter
// updates never allocate or panic, and failed allocations leave live bytes alone.
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: forwarded GlobalAlloc caller contract.
        let pointer = unsafe { mimalloc::MiMalloc.alloc(layout) };
        if !pointer.is_null() {
            add(layout.size());
        }
        pointer
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        // SAFETY: forwarded GlobalAlloc caller contract.
        let pointer = unsafe { mimalloc::MiMalloc.alloc_zeroed(layout) };
        if !pointer.is_null() {
            add(layout.size());
        }
        pointer
    }
    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        // SAFETY: forwarded GlobalAlloc caller contract.
        unsafe { mimalloc::MiMalloc.dealloc(pointer, layout) };
        release(layout.size());
    }
    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        // SAFETY: forwarded GlobalAlloc caller contract.
        let result = unsafe { mimalloc::MiMalloc.realloc(pointer, layout, new_size) };
        if !result.is_null() {
            add(new_size);
            release(layout.size());
        }
        result
    }
}

pub fn snapshot() -> tsr_tsc::MemoryStatistics {
    let mut statistics = tsr_tsc::MemoryStatistics {
        allocated_bytes: 0,
        allocation_count: 0,
        live_bytes: 0,
    };
    for shard in &SHARDS_ {
        statistics.allocated_bytes = statistics
            .allocated_bytes
            .wrapping_add(shard.allocated.load(Ordering::Relaxed));
        statistics.allocation_count = statistics
            .allocation_count
            .wrapping_add(shard.count.load(Ordering::Relaxed));
        statistics.live_bytes = statistics
            .live_bytes
            .wrapping_add(shard.live.load(Ordering::Relaxed));
    }
    statistics
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn threads_count_on_their_own_shards_and_the_snapshot_sums_them() {
        let before = snapshot();
        let handles: Vec<_> = (0..4)
            .map(|_| {
                std::thread::spawn(|| {
                    let kept: Vec<Vec<u8>> = (0..64).map(|_| vec![7u8; 1024]).collect();
                    kept.len()
                })
            })
            .collect();
        for handle in handles {
            handle.join().unwrap();
        }
        let after = snapshot();
        assert!(after.allocation_count - before.allocation_count >= 4 * 64);
        assert!(after.allocated_bytes - before.allocated_bytes >= 4 * 64 * 1024);
        let retained = vec![1u8; 1 << 20];
        let holding = snapshot();
        assert!(holding.live_bytes.wrapping_sub(after.live_bytes) >= 1 << 20);
        drop(retained);
        assert!(snapshot().live_bytes.wrapping_sub(after.live_bytes) < 1 << 20);
    }
}
