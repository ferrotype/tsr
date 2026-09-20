#![forbid(unsafe_code)]
use mimalloc::MiMalloc;
use std::{
    hint::black_box,
    sync::atomic::{AtomicU64, Ordering},
};
#[global_allocator]
static GLOBAL: cap::Cap<MiMalloc> = cap::Cap::new(MiMalloc, usize::MAX);
const SIZES: [usize; 4] = [100_003, 100_007, 31, 1_000_009];
#[inline(never)]
fn known_allocations() {
    let zeroed = black_box(vec![0u8; black_box(SIZES[0])]);
    assert!(zeroed.iter().all(|&byte| byte == 0));
    let filled = black_box(vec![7u8; black_box(SIZES[1])]);
    let mut grow = Vec::with_capacity(black_box(SIZES[2]));
    grow.extend_from_slice(&[9u8; 31]);
    grow.reserve_exact(black_box(SIZES[3] - grow.len()));
    assert_eq!(grow.capacity(), SIZES[3]);
    assert_eq!(&grow[..], &[9u8; 31]);
    black_box((&zeroed, &filled, &grow));
    drop((zeroed, filled, grow));
}
fn main() {
    let ready = AtomicU64::new(0);
    let phase = AtomicU64::new(0);
    let done = AtomicU64::new(0);
    let mut measured = None;
    std::thread::scope(|scope| {
        for _ in 0..8 {
            scope.spawn(|| {
                ready.fetch_add(1, Ordering::Release);
                while phase.load(Ordering::Acquire) == 0 {
                    std::hint::spin_loop();
                }
                known_allocations();
                done.fetch_add(1, Ordering::Release);
                while phase.load(Ordering::Acquire) == 1 {
                    std::hint::spin_loop();
                }
            });
        }
        while ready.load(Ordering::Acquire) != 8 {
            std::hint::spin_loop();
        }
        let live_before = GLOBAL.allocated();
        let total_before = GLOBAL.total_allocated();
        phase.store(1, Ordering::Release);
        while done.load(Ordering::Acquire) != 8 {
            std::hint::spin_loop();
        }
        let total_after = GLOBAL.total_allocated();
        let live_after = GLOBAL.allocated();
        measured = Some((
            total_after.checked_sub(total_before).unwrap(),
            live_before,
            live_after,
        ));
        phase.store(2, Ordering::Release);
    });
    let (allocated, before, after) = measured.unwrap();
    let expected = SIZES.iter().sum::<usize>() * 8;
    assert_eq!(allocated, expected);
    assert_eq!(before, after);
    println!(
        "{}",
        serde_json::json!({"version":1,"workers":8,"requested_bytes":allocated,"expected_bytes":expected,"live_before":before,"live_after":after})
    );
}
