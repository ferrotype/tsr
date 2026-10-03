//! CLI allocator observations used by --diagnostics. These counters report
//! allocation requests/live requested bytes, not resident memory or Go arenas.
//!
//! mimalloc serves ordinary builds; `system-allocator` selects System for
//! instrumentation. Threads select one of 64 padded counter shards once;
//! later threads reuse shards, so collisions can still contend. Snapshots
//! sample cumulative releases before allocations, avoiding unsigned live-byte
//! underflow when another thread frees an allocation during the sample. Counts
//! are exact modulo u64 at quiescence; concurrent samples can overcount live
//! bytes and are not a consistent point-in-time view.
#![allow(unsafe_code)]
#[cfg(not(feature = "system-allocator"))]
use mimalloc::MiMalloc as InnerAllocator;
#[cfg(feature = "system-allocator")]
use std::alloc::System as InnerAllocator;
use std::alloc::{GlobalAlloc, Layout};
use std::cell::Cell;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

const SHARDS: usize = 64;

#[repr(align(128))]
struct Shard {
    allocated: AtomicU64,
    released: AtomicU64,
    count: AtomicU64,
}

impl Shard {
    const fn new() -> Self {
        Self {
            allocated: AtomicU64::new(0),
            released: AtomicU64::new(0),
            count: AtomicU64::new(0),
        }
    }

    fn add(&self, size: usize) {
        self.allocated.fetch_add(size as u64, Ordering::Relaxed);
        self.count.fetch_add(1, Ordering::Relaxed);
    }

    fn release(&self, size: usize) {
        // Pair with the snapshot's acquire before it samples allocations.
        // Synchronized cross-thread ownership transfer carries the original
        // allocation update here, even when it used a different shard.
        self.released.fetch_add(size as u64, Ordering::Release);
    }
}

static SHARDS_: [Shard; SHARDS] = [const { Shard::new() }; SHARDS];
static NEXT_SHARD: AtomicUsize = AtomicUsize::new(0);

thread_local! {
    // Native TLS on the supported Linux/macOS targets stores this const cell
    // directly, without allocation, lazy initialization or a destructor. It
    // remains readable during other TLS destructors at thread exit.
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

pub struct CountingAllocator;

// SAFETY: every pointer/layout pair is forwarded unchanged to InnerAllocator.
// Counter updates never allocate or panic on the supported native TLS targets,
// and failed allocations leave live bytes alone.
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: forwarded GlobalAlloc caller contract.
        let pointer = unsafe { InnerAllocator.alloc(layout) };
        if !pointer.is_null() {
            shard().add(layout.size());
        }
        pointer
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        // SAFETY: forwarded GlobalAlloc caller contract.
        let pointer = unsafe { InnerAllocator.alloc_zeroed(layout) };
        if !pointer.is_null() {
            shard().add(layout.size());
        }
        pointer
    }
    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        // SAFETY: forwarded GlobalAlloc caller contract.
        unsafe { InnerAllocator.dealloc(pointer, layout) };
        shard().release(layout.size());
    }
    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        // SAFETY: forwarded GlobalAlloc caller contract.
        let result = unsafe { InnerAllocator.realloc(pointer, layout, new_size) };
        if !result.is_null() {
            let shard = shard();
            shard.add(new_size);
            shard.release(layout.size());
        }
        result
    }
}

pub fn snapshot() -> tsr_tsc::MemoryStatistics {
    snapshot_after_releases(&SHARDS_, sample_released(&SHARDS_))
}

fn sample_released(shards: &[Shard]) -> u64 {
    shards.iter().fold(0u64, |total, shard| {
        total.wrapping_add(shard.released.load(Ordering::Acquire))
    })
}

fn snapshot_after_releases(shards: &[Shard], released: u64) -> tsr_tsc::MemoryStatistics {
    let mut statistics = tsr_tsc::MemoryStatistics {
        allocated_bytes: 0,
        allocation_count: 0,
        live_bytes: 0,
    };
    for shard in shards {
        statistics.allocated_bytes = statistics
            .allocated_bytes
            .wrapping_add(shard.allocated.load(Ordering::Relaxed));
        statistics.allocation_count = statistics
            .allocation_count
            .wrapping_add(shard.count.load(Ordering::Relaxed));
    }
    // All releases were sampled first. Their acquire loads make corresponding
    // allocations visible here; releases that race this pass are left for the
    // next sample. Taking a net live count from each shard could instead mix an
    // old allocation count with a new cross-thread free and wrap below zero.
    statistics.live_bytes = statistics.allocated_bytes.wrapping_sub(released);
    statistics
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cross_thread_release_is_counted_after_the_allocating_thread_exits() {
        // Private counters exclude unrelated allocations from parallel tests.
        let shards = [const { Shard::new() }; 2];
        std::thread::scope(|scope| {
            scope.spawn(|| shards[0].add(1024)).join().unwrap();
            scope.spawn(|| shards[1].release(1024)).join().unwrap();
        });
        let statistics = snapshot_after_releases(&shards, sample_released(&shards));
        assert_eq!(statistics.allocation_count, 1);
        assert_eq!(statistics.allocated_bytes, 1024);
        assert_eq!(statistics.live_bytes, 0);
    }

    #[test]
    fn cross_thread_free_during_snapshot_cannot_subtract_an_unseen_allocation() {
        let shards = [const { Shard::new() }; 2];
        // Freeze the first pass, then place a complete allocation/free between
        // the passes. The concurrent sample may overcount, but cannot underflow.
        let released = sample_released(&shards);
        std::thread::scope(|scope| {
            scope.spawn(|| shards[0].add(1024)).join().unwrap();
            scope.spawn(|| shards[1].release(1024)).join().unwrap();
        });
        let during = snapshot_after_releases(&shards, released);
        assert_eq!(during.live_bytes, 1024);
        let after = snapshot_after_releases(&shards, sample_released(&shards));
        assert_eq!(after.live_bytes, 0);
    }

    #[test]
    fn cumulative_counter_wrap_preserves_quiescent_live_bytes() {
        let shards = [Shard::new()];
        shards[0].allocated.store(u64::MAX - 7, Ordering::Relaxed);
        shards[0].released.store(u64::MAX - 15, Ordering::Relaxed);
        shards[0].add(16);
        let statistics = snapshot_after_releases(&shards, sample_released(&shards));
        assert_eq!(statistics.allocated_bytes, 8);
        assert_eq!(statistics.live_bytes, 24);
        shards[0].release(24);
        assert_eq!(
            snapshot_after_releases(&shards, sample_released(&shards)).live_bytes,
            0
        );
    }

    #[test]
    fn shard_remains_accessible_during_thread_local_destruction() {
        struct OnExit;
        impl Drop for OnExit {
            fn drop(&mut self) {
                shard().add(1);
                shard().release(1);
            }
        }
        thread_local! {
            static ON_EXIT: OnExit = const { OnExit };
        }
        std::thread::spawn(|| ON_EXIT.with(|_| {})).join().unwrap();
    }
}
