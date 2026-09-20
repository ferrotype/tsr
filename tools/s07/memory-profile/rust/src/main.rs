//! Staged-only retained memory and allocation diagnostics for frozen S07 work.
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs,
    hint::black_box,
    io::{self, Write},
    path::Path,
    sync::{mpsc, Barrier},
    thread,
    time::Instant,
};
use tsr_ast::{
    AstFile, BoundFile, ExternalModuleIndicatorOptions, JsString, NodeId, ParsedFile,
    SourceFileParseOptions,
};
use tsr_jsstring::SourceText;

#[cfg(not(feature = "sites"))]
#[global_allocator]
static ALLOCATOR: cap::Cap<mimalloc::MiMalloc> = cap::Cap::new(mimalloc::MiMalloc, usize::MAX);
#[cfg(feature = "sites")]
#[global_allocator]
static ALLOCATOR: cap::Cap<alloc_tracker::Allocator<mimalloc::MiMalloc>> = cap::Cap::new(
    alloc_tracker::Allocator::new(mimalloc::MiMalloc),
    usize::MAX,
);
#[cfg(feature = "sites")]
use tsr_jsstring::memory_sites::{self, Phase};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Input {
    filename: String,
    path: String,
    local: String,
    script_kind: i32,
    jsx: bool,
    force: bool,
}
struct Loaded {
    source: SourceText,
    options: SourceFileParseOptions,
    script_kind: tsr_core::ScriptKind,
}
#[derive(Default, Serialize)]
struct WorkerTimes {
    parse_ns: u128,
    publish_ns: u128,
    bind_ns: u128,
    parse_allocated_bytes: usize,
    publish_allocated_bytes: usize,
    bind_allocated_bytes: usize,
    parse_live_growth_bytes: isize,
    publish_live_growth_bytes: isize,
    bind_live_growth_bytes: isize,
}
#[derive(Serialize)]
struct Report {
    version: u32,
    runtime: &'static str,
    diagnostic_only: bool,
    workers: usize,
    files: usize,
    loaded_bytes: usize,
    loaded_input_sha256: String,
    nodes: i64,
    symbols: isize,
    parse_diagnostics: usize,
    bind_diagnostics: usize,
    pipeline_wall_ns: u128,
    worker_times: Vec<WorkerTimes>,
    elapsed_worker_totals: WorkerTimes,
    timer_domain: &'static str,
    pre_pipeline: Memory,
    retained_endpoint: Memory,
    pipeline_allocated_bytes: usize,
    pipeline_live_growth_bytes: isize,
    pipeline_superseded_or_freed_requested_bytes: isize,
    allocation_domain: &'static str,
}

#[derive(Clone, Copy, Serialize)]
struct Memory {
    live_requested_bytes: usize,
    total_requested_bytes: usize,
    peak_requested_bytes: usize,
}
fn memory() -> Memory {
    Memory {
        live_requested_bytes: ALLOCATOR.allocated(),
        total_requested_bytes: ALLOCATOR.total_allocated(),
        peak_requested_bytes: ALLOCATOR.max_allocated(),
    }
}
fn checkpoint(name: &str, snapshot: Memory) -> io::Result<()> {
    println!(
        "{}",
        serde_json::json!({"checkpoint": name, "memory": snapshot})
    );
    io::stdout().flush()?;
    let mut input = String::new();
    if io::stdin().read_line(&mut input)? == 0 || input != "\n" {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "checkpoint requires one newline",
        ));
    }
    Ok(())
}

#[inline(never)]
fn profile_preload(path: &Path) -> Result<Vec<Loaded>, Box<dyn std::error::Error>> {
    #[cfg(feature = "sites")]
    let _phase = memory_sites::phase(Phase::Preload);
    let inputs: Vec<Input> = serde_json::from_slice(&fs::read(path)?)?;
    if inputs.is_empty() {
        return Err("empty workload".into());
    }
    inputs
        .into_iter()
        .map(|input| {
            Ok(Loaded {
                source: SourceText::from_bytes(fs::read(input.local)?),
                options: SourceFileParseOptions {
                    file_name: JsString::from_bytes(input.filename.into_bytes()),
                    path: JsString::from_bytes(input.path.into_bytes()),
                    external_module_indicator_options: ExternalModuleIndicatorOptions {
                        jsx: input.jsx,
                        force: input.force,
                    },
                },
                script_kind: tsr_core::ScriptKind(input.script_kind),
            })
        })
        .collect()
}

fn loaded_digest(inputs: &[Loaded]) -> String {
    let mut digest = Sha256::new();
    digest.update(b"S07-loaded-inputs-v1\0");
    digest.update((inputs.len() as u64).to_be_bytes());
    for input in inputs {
        for value in [
            input.options.file_name.as_bytes(),
            input.options.path.as_bytes(),
        ] {
            digest.update((value.len() as u64).to_be_bytes());
            digest.update(value);
        }
        digest.update(input.script_kind.0.to_be_bytes());
        digest.update([
            u8::from(input.options.external_module_indicator_options.jsx),
            u8::from(input.options.external_module_indicator_options.force),
        ]);
        digest.update((input.source.len() as u64).to_be_bytes());
        digest.update(Sha256::digest(input.source.as_bytes()));
    }
    format!("{:x}", digest.finalize())
}

// Named, non-inlined frames preserve the original interleaved operation order.
// black_box prevents a tail call from erasing the phase frame in sampled stacks.
#[inline(never)]
fn profile_parse(input: &Loaded) -> ParsedFile {
    #[cfg(feature = "sites")]
    let _phase = memory_sites::phase(Phase::Parse);
    black_box(tsr_parser::parse_source_file(
        input.source.clone(),
        input.script_kind,
        input.options.clone(),
    ))
}

#[inline(never)]
fn profile_publish(parsed: ParsedFile) -> (AstFile, NodeId) {
    #[cfg(feature = "sites")]
    let _phase = memory_sites::phase(Phase::Publish);
    let source = parsed.root();
    black_box((parsed.publish_unbound(), source))
}

#[inline(never)]
fn profile_bind(file: AstFile, source: NodeId) -> BoundFile {
    #[cfg(feature = "sites")]
    let _phase = memory_sites::phase(Phase::Bind);
    let bound = tsr_binder::bind_source_file(&file, source).expect("workload binding must complete");
    drop(file);
    black_box(bound)
}

#[inline(never)]
fn measure_pipeline(
    senders: Vec<mpsc::SyncSender<usize>>,
    count: usize,
    finished: &Barrier,
) -> u128 {
    let start = Instant::now();
    for index in 0..count {
        senders[index % senders.len()]
            .send(index)
            .expect("worker drains every input");
    }
    drop(senders);
    finished.wait();
    black_box(start.elapsed().as_nanos())
}

#[inline(never)]
fn profile_retirement(files: Vec<Vec<BoundFile>>) {
    #[cfg(feature = "sites")]
    let _phase = memory_sites::phase(Phase::Retire);
    drop(black_box(files));
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args_os().collect();
    if args.len() != 4 {
        return Err("usage: tsr_memory_profile INPUTS.json WORKERS OUTPUT_PREFIX".into());
    }
    let workers: usize = args[2].to_str().ok_or("invalid workers")?.parse()?;
    if !matches!(workers, 1 | 8) || thread::available_parallelism()?.get() < workers {
        return Err("profiling requires 1 or 8 available workers".into());
    }
    #[cfg(feature = "sites")]
    memory_sites::initialize();
    let prefix = Path::new(&args[3]);
    let inputs = profile_preload(Path::new(&args[1]))?;
    let file_count = inputs.len();
    let loaded_bytes = inputs.iter().map(|input| input.source.len()).sum();
    let loaded_input_sha256 = loaded_digest(&inputs);
    let ready = Barrier::new(workers + 1);
    let finished = Barrier::new(workers + 1);
    let release = Barrier::new(workers + 1);
    thread::scope(|scope| -> Result<(), Box<dyn std::error::Error>> {
        let mut senders = Vec::with_capacity(workers);
        let mut handles = Vec::with_capacity(workers);
        for _ in 0..workers {
            let (send, receive) = mpsc::sync_channel::<usize>(workers);
            senders.push(send);
            let inputs = &inputs;
            let (ready, finished, release) = (&ready, &finished, &release);
            handles.push(tsr_parser::spawn_parser_worker(scope, move || {
                let mut roots = Vec::with_capacity(file_count.div_ceil(workers));
                let mut times = WorkerTimes::default();
                let mut failure = None;
                #[cfg(feature = "sites")]
                memory_sites::initialize_thread();
                ready.wait();
                while let Ok(index) = receive.recv() {
                    if failure.is_some() {
                        continue;
                    }
                    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        let live_before = ALLOCATOR.allocated();
                        let allocated_before = ALLOCATOR.total_allocated();
                        let start = Instant::now();
                        let parsed = profile_parse(&inputs[index]);
                        times.parse_ns += start.elapsed().as_nanos();
                        let after_parse = ALLOCATOR.total_allocated();
                        let live_after_parse = ALLOCATOR.allocated();
                        if workers == 1 {
                            times.parse_live_growth_bytes +=
                                live_after_parse as isize - live_before as isize;
                        }
                        if workers == 1 {
                            times.parse_allocated_bytes += after_parse - allocated_before;
                        }
                        let start = Instant::now();
                        let (file, source) = profile_publish(parsed);
                        times.publish_ns += start.elapsed().as_nanos();
                        let after_publish = ALLOCATOR.total_allocated();
                        let live_after_publish = ALLOCATOR.allocated();
                        if workers == 1 {
                            times.publish_live_growth_bytes +=
                                live_after_publish as isize - live_after_parse as isize;
                        }
                        if workers == 1 {
                            times.publish_allocated_bytes += after_publish - after_parse;
                        }
                        let start = Instant::now();
                        let bound = profile_bind(file, source);
                        times.bind_ns += start.elapsed().as_nanos();
                        if workers == 1 {
                            times.bind_allocated_bytes +=
                                ALLOCATOR.total_allocated() - after_publish;
                            times.bind_live_growth_bytes +=
                                ALLOCATOR.allocated() as isize - live_after_publish as isize;
                        }
                        bound
                    }));
                    match outcome {
                        Ok(file) => roots.push(file),
                        Err(panic) => failure = Some(panic),
                    }
                }
                finished.wait();
                release.wait();
                if let Some(panic) = failure {
                    std::panic::resume_unwind(panic);
                }
                (roots, times)
            })?);
        }
        ready.wait();
        checkpoint("pre_pipeline", memory())?;
        let pre_pipeline = memory();
        let pipeline_wall_ns = measure_pipeline(senders, file_count, &finished);
        let retained_endpoint = memory();
        checkpoint("retained_endpoint", retained_endpoint)?;
        release.wait();
        let mut files = Vec::with_capacity(workers);
        let mut worker_times = Vec::with_capacity(workers);
        let mut totals = WorkerTimes::default();
        for handle in handles {
            let (roots, times) = handle
                .join()
                .unwrap_or_else(|panic| std::panic::resume_unwind(panic));
            totals.parse_ns += times.parse_ns;
            totals.publish_ns += times.publish_ns;
            totals.bind_ns += times.bind_ns;
            totals.parse_allocated_bytes += times.parse_allocated_bytes;
            totals.publish_allocated_bytes += times.publish_allocated_bytes;
            totals.bind_allocated_bytes += times.bind_allocated_bytes;
            totals.parse_live_growth_bytes += times.parse_live_growth_bytes;
            totals.publish_live_growth_bytes += times.publish_live_growth_bytes;
            totals.bind_live_growth_bytes += times.bind_live_growth_bytes;
            worker_times.push(times);
            files.push(roots);
        }
        let mut report = Report {
            version: 1,
            runtime: "rust",
            diagnostic_only: true,
            workers,
            files: 0,
            loaded_bytes,
            loaded_input_sha256,
            nodes: 0,
            symbols: 0,
            parse_diagnostics: 0,
            bind_diagnostics: 0,
            pipeline_wall_ns,
            worker_times,
            elapsed_worker_totals: totals,
            pre_pipeline,
            retained_endpoint,
            pipeline_allocated_bytes: retained_endpoint.total_requested_bytes - pre_pipeline.total_requested_bytes,
            pipeline_live_growth_bytes: retained_endpoint.live_requested_bytes as isize - pre_pipeline.live_requested_bytes as isize,
            pipeline_superseded_or_freed_requested_bytes: (retained_endpoint.total_requested_bytes - pre_pipeline.total_requested_bytes) as isize - (retained_endpoint.live_requested_bytes as isize - pre_pipeline.live_requested_bytes as isize),
            allocation_domain: "cap requested-byte counters, native mimalloc alloc/zero/realloc forwarding; successful realloc charges full new requested size, including in-place/shrink; churn subtraction counts freed OR superseded requests, not necessarily physical copies; phase counters are global request deltas measured only for one worker (zeros otherwise), so can include main-thread/channel allocations; separate sites build provides thread-local scopes",
            timer_domain:
                "sum of per-file elapsed worker timers; includes descheduling; not CPU time",
        };
        for file in files.iter().flatten() {
            let view = file.view();
            let source = view.source_file()?;
            report.files += 1;
            report.nodes += source.node_count;
            report.symbols += view.result().symbol_count();
            report.parse_diagnostics += source.diagnostics.len();
            report.bind_diagnostics += view.result().diagnostics().len();
        }
        if report.files != file_count {
            return Err("incomplete workload".into());
        }
        let census_before = memory();
        let census_start = Instant::now();
        let mut census = tsr_jsstring::census::Collector::default();
        for file in files.iter().flatten() {
            tsr_ast::add_retained_file(file, &mut census);
        }
        use tsr_jsstring::census::Walk;
        for input in &inputs {
            input.source.walk(&mut census, "preload.source");
            input
                .options
                .file_name
                .walk(&mut census, "preload.file_name");
            input.options.path.walk(&mut census, "preload.path");
        }
        census.vector(&inputs, "preload.records");
        for roots in &files {
            census.vector(roots, "driver.root_vectors");
        }
        census.vector(&files, "driver.worker_roots");
        let census_after = memory();
        let census_wall_ns = census_start.elapsed().as_nanos();
        fs::write(format!("{}-census.json", prefix.display()), census.json())?;
        drop(census);
        let mut output = serde_json::to_value(&report)?;
        output["census_cost"] = serde_json::json!({"before": census_before, "after": census_after, "wall_ns": census_wall_ns});
        fs::write(
            format!("{}-report.json", prefix.display()),
            serde_json::to_vec_pretty(&output)?,
        )?;
        black_box(&files);
        profile_retirement(files);
        #[cfg(feature = "sites")]
        {
            let rows: Vec<_> = memory_sites::report().into_iter().map(|r| serde_json::json!({
                "name":r.name,"phase":r.phase,"source_file":r.source_file,"source_line":r.source_line,
                "scope_kind":r.scope_kind,"requested_bytes":r.requested_bytes,
                "allocation_calls":r.allocation_calls,"observations":r.observations
            })).collect();
            fs::write(
                format!("{}-sites.json", prefix.display()),
                serde_json::to_vec_pretty(&rows)?,
            )?;
        }
        checkpoint("post_roots_retirement", memory())?;
        Ok(())
    })?;
    drop(inputs);
    checkpoint("post_retirement", memory())?;
    println!("{}", serde_json::json!({"complete": true}));
    Ok(())
}
