//! Phase attribution for the unchanged S07 parse/bind workload. This adapter
//! emits diagnostic elapsed timers; Time Profiler supplies separate CPU samples.
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

#[global_allocator]
static ALLOCATOR: mimalloc::MiMalloc = mimalloc::MiMalloc;

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
}

#[inline(never)]
fn profile_preload(path: &Path) -> Result<Vec<Loaded>, Box<dyn std::error::Error>> {
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
    black_box(tsr_parser::parse_source_file(
        input.source.clone(),
        input.script_kind,
        input.options.clone(),
    ))
}

#[inline(never)]
fn profile_publish(parsed: ParsedFile) -> (AstFile, NodeId) {
    let source = parsed.root();
    black_box((parsed.publish_unbound(), source))
}

#[inline(never)]
fn profile_bind(file: AstFile, source: NodeId) -> BoundFile {
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
    drop(black_box(files));
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args_os().collect();
    if args.len() != 3 {
        return Err("usage: tsr_cpu_profile INPUTS.json WORKERS (1 or 8)".into());
    }
    let workers: usize = args[2].to_str().ok_or("invalid workers")?.parse()?;
    if !matches!(workers, 1 | 8) || thread::available_parallelism()?.get() < workers {
        return Err("profiling requires 1 or 8 available workers".into());
    }
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
                ready.wait();
                while let Ok(index) = receive.recv() {
                    if failure.is_some() {
                        continue;
                    }
                    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        let start = Instant::now();
                        let parsed = profile_parse(&inputs[index]);
                        times.parse_ns += start.elapsed().as_nanos();
                        let start = Instant::now();
                        let (file, source) = profile_publish(parsed);
                        times.publish_ns += start.elapsed().as_nanos();
                        let start = Instant::now();
                        let bound = profile_bind(file, source);
                        times.bind_ns += start.elapsed().as_nanos();
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
        let pipeline_wall_ns = measure_pipeline(senders, file_count, &finished);
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
        println!("{}", serde_json::to_string(&report)?);
        io::stdout().flush()?;
        black_box(&files);
        profile_retirement(files);
        Ok(())
    })
}
