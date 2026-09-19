use std::io::Read;

#[global_allocator]
static ALLOCATOR: cap::Cap<mimalloc::MiMalloc> = cap::Cap::new(mimalloc::MiMalloc, usize::MAX);

fn main() {
    let mut raw = Vec::new();
    std::io::stdin().read_to_end(&mut raw).unwrap();
    let requests = serde_json::from_slice(&raw).unwrap();
    let prepared = ts_checker::storage_pilot::Prepared::new(&requests).unwrap();
    let live_before = ALLOCATOR.allocated();
    let requested_before = ALLOCATOR.total_allocated();
    let result = prepared.execute().unwrap();
    let requested_bytes = ALLOCATOR.total_allocated() - requested_before;
    let live_after = ALLOCATOR.allocated();
    let mut output = result.observation();
    output["allocator"] = serde_json::json!({"requested_bytes": requested_bytes,
        "live_before": live_before, "live_after": live_after, "retained_delta": live_after - live_before});
    drop(result);
    // JSON output itself is live now; its allocations are deliberately outside the capture.
    println!("{output}");
}
