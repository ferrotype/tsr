use std::io::Read;

#[global_allocator]
static ALLOCATOR: cap::Cap<mimalloc::MiMalloc> = cap::Cap::new(mimalloc::MiMalloc, usize::MAX);

fn main() {
    let mut raw = Vec::new();
    std::io::stdin().read_to_end(&mut raw).unwrap();
    let request = serde_json::from_slice(&raw).unwrap();
    let prepared = ts_checker::storage_families::Prepared::new(&request).unwrap();
    let live_before = ALLOCATOR.allocated();
    let requested_before = ALLOCATOR.total_allocated();
    // NewChecker's prefix and the trace are sampled separately, like the Go driver.
    let started = prepared.start().unwrap();
    let prefix_requested = ALLOCATOR.total_allocated() - requested_before;
    let live_after_prefix = ALLOCATOR.allocated();
    let result = started.run().unwrap();
    let trace_requested = ALLOCATOR.total_allocated() - requested_before - prefix_requested;
    let live_after = ALLOCATOR.allocated();
    let mut output = result.observation().unwrap();
    output["allocator"] = serde_json::json!({
        "prefix": {"requested_bytes": prefix_requested, "retained_delta": live_after_prefix - live_before},
        "trace": {"requested_bytes": trace_requested, "retained_delta": live_after - live_after_prefix},
        "live_before": live_before, "live_after": live_after, "retained_delta": live_after - live_before});
    drop(result);
    // JSON output itself is live now; its allocations are deliberately outside the capture.
    println!("{output}");
}
