//! Safe, fixed-request accounting counterexamples. Run in its own process.
#![forbid(unsafe_code)]
use std::alloc::System;
use std::hint::black_box;
use tsr_jsstring::memory_sites::{self as sites, Phase, Site};

#[global_allocator]
static ALLOCATOR: alloc_tracker::Allocator<System> = alloc_tracker::Allocator::new(System);

fn main() {
    sites::initialize();
    {
        let _phase = sites::phase(Phase::Parse);
        let _parent = sites::site(Site::ArenaPush);
        black_box(vec![1_u8; 101]);
        {
            let _child = sites::site(Site::CoreNodeSlots);
            black_box(vec![0_u8; 103]);
        }
        black_box(vec![3_u8; 107]);
    }
    // A different phase, outside every selected site, is the explicit residual.
    {
        let _phase = sites::phase(Phase::Bind);
        let mut bytes = Vec::with_capacity(109);
        bytes.resize(109, 1_u8);
        bytes.reserve_exact(127 - bytes.len());
        bytes.shrink_to(113);
        black_box(bytes);
    }
    // Origin groups do not exist in this traffic profiler: cross-thread drops
    // contribute zero allocation traffic, while each thread owns its scopes.
    let handles: Vec<_> = (0..8)
        .map(|_| {
            std::thread::spawn(|| {
                sites::initialize_thread();
                let _phase = sites::phase(Phase::Publish);
                let _site = sites::site(Site::FullBindings);
                black_box(vec![4_u8; 131])
            })
        })
        .collect();
    for handle in handles {
        drop(handle.join().unwrap());
    }
    let rows = sites::report();
    let row = |phase, kind, name| {
        rows.iter()
            .find(|r| r.phase == phase && r.scope_kind == kind && r.name == name)
            .unwrap()
    };
    assert_eq!(row("parse", "phase", "phase").requested_bytes, 311);
    assert_eq!(
        row("parse", "inclusive_site", "arena.page_and_directory_growth").requested_bytes,
        311
    );
    assert_eq!(
        row("parse", "inclusive_site", "arena.core_node_slots").requested_bytes,
        103
    );
    assert_eq!(
        row("parse", "selected_union", "selected source regions").requested_bytes,
        311
    );
    assert_eq!(
        row("parse", "selected_union", "selected source regions").allocation_calls,
        3
    );
    assert_eq!(
        row("bind", "phase", "phase").requested_bytes,
        109 + 127 + 113
    );
    assert_eq!(row("bind", "phase", "phase").allocation_calls, 3);
    assert_eq!(
        row("bind", "selected_union", "selected source regions").requested_bytes,
        0
    );
    assert_eq!(
        row("publish", "inclusive_site", "binding.full_record_map").requested_bytes,
        8 * 131
    );
    assert_eq!(
        row("publish", "selected_union", "selected source regions").requested_bytes,
        8 * 131
    );
    println!("allocation source-scope accounting passed");
}
