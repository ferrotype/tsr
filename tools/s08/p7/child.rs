//! The Rust checkerbench child: `p7_checkerbench REQUESTS ROWS` runs every
//! request serially, retains each variant's bound roots and escaping outputs
//! through its checkpoint, releases them before the next variant, writes one
//! NDJSON row per variant to ROWS and one totals report to stdout.
//!
//! Executables: normal timing (no feature), `s08-phase-timer` (nested
//! exclusive init/check/display clocks) and `s08-allocation` (counting
//! allocator wrapper, retained checkpoints, structural census).
use crate::corpus;
use crate::executor::Hooks;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::io::Write as _;
use std::sync::Arc;
use std::time::Instant;
use ts_checker::{Operation, TypeRef};
use ts_compiler::{FileCache, Program, ProgramFile};

// S07's allocator wrapper: requested bytes (layout sizes) and live requested
// bytes at the allocator boundary; allocator size-class slack excluded.
// `cap` counts no allocation calls; that field is reported as unavailable.
#[cfg(feature = "s08-allocation")]
#[global_allocator]
static ALLOCATOR: cap::Cap<mimalloc::MiMalloc> = cap::Cap::new(mimalloc::MiMalloc, usize::MAX);
#[cfg(not(feature = "s08-allocation"))]
#[global_allocator]
static ALLOCATOR: mimalloc::MiMalloc = mimalloc::MiMalloc;

pub const MODE: &str = if cfg!(feature = "s08-allocation") {
    "alloc"
} else if cfg!(feature = "s08-phase-timer") {
    "phase"
} else {
    "normal"
};

/// Per-variant measurement state. The interval clock accumulates wall time
/// between `interval_start`/`pause`/`resume` and the checkpoint.
#[derive(Default)]
struct Measure {
    /// Library files of the previous variant, kept alive until this variant
    /// has loaded so the shared cache reuses their parse and bind (the Go
    /// harness caches parsed files across tests the same way). Never checker
    /// state; live at both retained endpoints.
    libraries: Vec<Arc<ProgramFile>>,
    running: Option<Instant>,
    interval_ns: u64,
    roots: Vec<TypeRef>,
    checkpoint: Option<Value>,
    #[cfg(feature = "s08-allocation")]
    allocation: Allocation,
}
#[cfg(feature = "s08-allocation")]
#[derive(Default)]
struct Allocation {
    /// Total-allocated snapshot at interval start; deltas accumulate while running.
    running: Option<u64>,
    requested: u64,
    live_before: u64,
    live_after: Option<u64>,
}
#[cfg(feature = "s08-allocation")]
fn live() -> u64 {
    ALLOCATOR.allocated() as u64
}
#[cfg(feature = "s08-allocation")]
fn total() -> u64 {
    ALLOCATOR.total_allocated() as u64
}

impl Measure {
    fn start(&mut self) {
        self.running = Some(Instant::now());
        #[cfg(feature = "s08-allocation")]
        {
            self.allocation.running = Some(total());
        }
    }
    fn stop(&mut self) {
        if let Some(since) = self.running.take() {
            self.interval_ns += u64::try_from(since.elapsed().as_nanos()).unwrap_or(u64::MAX);
        }
        #[cfg(feature = "s08-allocation")]
        if let Some(before) = self.allocation.running.take() {
            self.allocation.requested += total() - before;
        }
    }
}
impl Hooks for Measure {
    fn loaded(&mut self, program: &Program) {
        self.libraries = program
            .files()
            .iter()
            .filter(|file| {
                file.bound()
                    .view()
                    .source_file()
                    .is_ok_and(|source| program.is_lib(source.parse_options().path.as_bytes()))
            })
            .cloned()
            .collect();
    }
    fn interval_start(&mut self) {
        #[cfg(feature = "s08-allocation")]
        {
            self.allocation.live_before = live();
        }
        #[cfg(feature = "s08-phase-timer")]
        {
            crate::baseline::instrument::reset();
            crate::baseline::instrument::push(crate::baseline::instrument::CHECK);
        }
        self.start();
    }
    fn init_start(&mut self) {
        #[cfg(feature = "s08-phase-timer")]
        crate::baseline::instrument::push(crate::baseline::instrument::INIT);
    }
    fn init_end(&mut self) {
        #[cfg(feature = "s08-phase-timer")]
        crate::baseline::instrument::pop();
    }
    fn pause(&mut self) {
        self.stop();
        #[cfg(feature = "s08-phase-timer")]
        crate::baseline::instrument::suspend();
    }
    fn resume(&mut self) {
        #[cfg(feature = "s08-phase-timer")]
        crate::baseline::instrument::resume();
        self.start();
    }
    fn roots(&mut self, types: &[TypeRef]) {
        self.roots = types.to_vec();
    }
    fn checkpoint(&mut self, op: &mut Operation<'_>) {
        self.stop();
        #[cfg(feature = "s08-phase-timer")]
        {
            crate::baseline::instrument::pop();
            debug_assert_eq!(crate::baseline::instrument::depth(), 0);
        }
        #[cfg(feature = "s08-allocation")]
        let census = {
            // Checker and every escaping result are live; temporaries are gone.
            self.allocation.live_after = Some(live());
            match op.census(&self.roots) {
                Ok(census) => census,
                Err(error) => json!({"state":"failed","reason":format!("{error:?}")}),
            }
        };
        #[cfg(not(feature = "s08-allocation"))]
        let census = Value::Null;
        self.checkpoint = Some(json!({
            "types_created": op.type_count(), "symbols_created": op.symbol_count(),
            "signatures_created": op.signature_count(), "roots": self.roots.len(),
            "census": census,
        }));
    }
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut text = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        write!(&mut text, "{byte:02x}").expect("formatting into String is infallible");
    }
    text
}

/// The produced result bytes: baseline texts, public type strings and the
/// ordered diagnostics (code, file, position) of every phase. Hashed after the
/// interval; both runtimes hash the same fields in the same order.
fn output_digest(row: &Value) -> String {
    let mut hasher = Sha256::new();
    let mut part = |label: &str, value: &Value| {
        hasher.update(label.as_bytes());
        hasher.update(b"\0");
        hasher.update(value.as_str().unwrap_or("").as_bytes());
        hasher.update(b"\n");
    };
    let baselines = &row["type_symbol_baselines"];
    part("types", &baselines["types"]["text_hex"]);
    part("symbols", &baselines["symbols"]["text_hex"]);
    part("errors", &row["error_baseline"]["baseline"]["text_hex"]);
    for query in baselines["public_type_strings"]["queries"]
        .as_array()
        .into_iter()
        .flatten()
    {
        part("tts", &query["text_hex"]);
    }
    // The sorted, deduplicated diagnostics both runtimes produce.
    let mut diagnostics = Vec::new();
    for d in row["error_baseline"]["diagnostics"]
        .as_array()
        .into_iter()
        .flatten()
    {
        diagnostics.push(format!(
            "{}:{}:{}:{}",
            d["code"],
            d["file_hex"].as_str().unwrap_or(""),
            d["pos"],
            d["end"]
        ));
    }
    for d in diagnostics {
        part("diagnostic", &Value::String(d));
    }
    hex(&hasher.finalize())
}

fn executed(row: &Value) -> Result<(), String> {
    if row["fatal"].is_object() {
        return Err(format!("panic: {}", row["fatal"]["reason"]));
    }
    if row["load"]["state"] != "executed" {
        return Err(format!("load: {}", row["load"]["reason"]));
    }
    for (name, phase) in row["phases"].as_object().into_iter().flatten() {
        if phase["state"] != "executed" {
            return Err(format!("phase {name}: {}", phase["state"]));
        }
    }
    let baselines = &row["type_symbol_baselines"];
    if baselines["state"] != "executed" && baselines["state"] != "not_requested" {
        return Err(format!("type_symbol_baselines: {}", baselines["reason"]));
    }
    if row["error_baseline"]["state"] != "executed" {
        return Err(format!(
            "error_baseline: {}",
            row["error_baseline"]["reason"]
        ));
    }
    Ok(())
}

fn variant(request: &Value, cache: &mut FileCache, libraries: &mut Vec<Arc<ProgramFile>>) -> Value {
    let mut measure = Measure {
        libraries: std::mem::take(libraries),
        ..Measure::default()
    };
    let observed = corpus::observe_with(request, cache, &mut measure, true);
    *libraries = std::mem::take(&mut measure.libraries);
    let mut row = json!({
        "id": request["id"], "acceptance_tier": request["acceptance_tier"],
        "interval_ns": measure.interval_ns, "actions": corpus::action_counts(&observed),
        "output_sha256": output_digest(&observed), "checkpoint": measure.checkpoint,
    });
    match executed(&observed) {
        Ok(()) => row["outcome"] = json!("executed"),
        Err(reason) => {
            row["outcome"] = json!("failed");
            row["failure"] = json!(reason);
        }
    }
    #[cfg(feature = "s08-phase-timer")]
    {
        let totals = crate::baseline::instrument::totals();
        row["phases_ns"] = json!(crate::baseline::instrument::NAMES
            .iter()
            .zip(totals)
            .map(|(name, ns)| (name.to_string(), json!(ns)))
            .collect::<serde_json::Map<_, _>>());
    }
    #[cfg(feature = "s08-allocation")]
    {
        let a = &measure.allocation;
        row["allocation"] = json!({
            "requested_bytes": a.requested, "allocation_calls": Value::Null,
            "live_before_interval": a.live_before, "live_at_checkpoint": a.live_after,
        });
    }
    // Checkpoint 3: everything of this variant released (leak diagnostic).
    drop(observed);
    #[cfg(feature = "s08-allocation")]
    {
        row["allocation"]["live_after_release"] = json!(live());
    }
    row
}

pub fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args_os().collect();
    if args.len() != 3 {
        return Err("usage: p7_checkerbench REQUESTS ROWS".into());
    }
    let requests: Vec<Value> = serde_json::from_slice(&std::fs::read(&args[1])?)?;
    let mut rows = std::io::BufWriter::new(std::fs::File::create(&args[2])?);
    let mut totals = json!({
        "version": 1, "mode": MODE, "variants": requests.len(), "executed": 0, "failed": 0,
        "interval_ns": 0u64, "phases_ns": {"init": 0u64, "check": 0u64, "display": 0u64},
        "allocation": {"requested_bytes": 0u64, "retained_bytes": 0u64},
        "census": {"type_storage_bytes": 0u64, "checker_bytes": 0u64, "types_reachable": 0u64, "types_created": 0u64, "unavailable": 0u64},
        "failures": [],
    });
    let mut digests = Sha256::new();
    let mut actions = Sha256::new();
    let started = Instant::now();
    let mut cache = FileCache::new();
    let mut libraries = Vec::new();
    for request in &requests {
        let row = variant(request, &mut cache, &mut libraries);
        cache.prune();
        digests.update(row["output_sha256"].as_str().unwrap_or("").as_bytes());
        digests.update(b"\n");
        actions.update(serde_json::to_vec(&row["actions"])?);
        actions.update(b"\n");
        let add = |totals: &mut Value, path: &[&str], value: u64| {
            let mut slot = totals;
            for key in path {
                slot = &mut slot[*key];
            }
            *slot = json!(slot.as_u64().unwrap_or(0) + value);
        };
        if row["outcome"] == "executed" {
            add(&mut totals, &["executed"], 1);
        } else {
            add(&mut totals, &["failed"], 1);
            totals["failures"]
                .as_array_mut()
                .expect("failures list")
                .push(json!({"id": row["id"], "failure": row["failure"]}));
        }
        add(
            &mut totals,
            &["interval_ns"],
            row["interval_ns"].as_u64().unwrap_or(0),
        );
        for phase in ["init", "check", "display"] {
            add(
                &mut totals,
                &["phases_ns", phase],
                row["phases_ns"][phase].as_u64().unwrap_or(0),
            );
        }
        let allocation = &row["allocation"];
        add(
            &mut totals,
            &["allocation", "requested_bytes"],
            allocation["requested_bytes"].as_u64().unwrap_or(0),
        );
        add(
            &mut totals,
            &["allocation", "retained_bytes"],
            allocation["live_at_checkpoint"]
                .as_u64()
                .unwrap_or(0)
                .saturating_sub(allocation["live_before_interval"].as_u64().unwrap_or(0)),
        );
        let census = &row["checkpoint"]["census"];
        if census.is_object() && census["state"].is_null() {
            add(
                &mut totals,
                &["census", "type_storage_bytes"],
                census["type_storage_bytes"].as_u64().unwrap_or(0),
            );
            add(
                &mut totals,
                &["census", "checker_bytes"],
                census["checker_bytes"].as_u64().unwrap_or(0),
            );
            add(
                &mut totals,
                &["census", "types_reachable"],
                census["types"]["reachable"].as_u64().unwrap_or(0),
            );
            add(
                &mut totals,
                &["census", "types_created"],
                census["types"]["created"].as_u64().unwrap_or(0),
            );
            add(
                &mut totals,
                &["census", "unavailable"],
                census["unavailable"].as_array().map_or(0, Vec::len) as u64,
            );
        }
        serde_json::to_writer(&mut rows, &row)?;
        rows.write_all(b"\n")?;
    }
    rows.flush()?;
    totals["process_ns"] = json!(u64::try_from(started.elapsed().as_nanos()).unwrap_or(u64::MAX));
    totals["outputs_sha256"] = json!(hex(&digests.finalize()));
    totals["actions_sha256"] = json!(hex(&actions.finalize()));
    println!("{}", serde_json::to_string(&totals)?);
    Ok(())
}
