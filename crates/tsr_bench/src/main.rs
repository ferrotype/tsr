//! Native parse-and-bind driver. Inputs and workers are provisioned before the
//! measured phase; every completed file remains owned through its endpoint.
#[path = "../../ts_binder/examples/support/graph.rs"]
mod binder_graph;
mod workload_graph;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{self, Write},
    path::Path,
    sync::{mpsc, Barrier},
    thread,
    time::Instant,
};
use ts_ast::{CompletedFile, ExternalModuleIndicatorOptions, JsString, SourceFileParseOptions};
use ts_jsstring::SourceText;

#[cfg(feature = "allocation")]
#[global_allocator]
static ALLOCATOR: cap::Cap<mimalloc::MiMalloc> = cap::Cap::new(mimalloc::MiMalloc, usize::MAX);
#[cfg(not(feature = "allocation"))]
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
    script_kind: ts_core::ScriptKind,
}
#[derive(Serialize)]
struct Report {
    version: u32,
    workers: usize,
    files: usize,
    loaded_bytes: usize,
    loaded_input_sha256: String,
    nodes: i64,
    symbols: isize,
    parse_diagnostics: usize,
    bind_diagnostics: usize,
    wall_time_ns: u128,
    allocated_bytes: Option<usize>,
    startup_ns: u128,
    preload_ns: u128,
    worker_setup_ns: u128,
    cpu_capacity: usize,
    goroutines_ready: Option<usize>,
}

// Length-framed identities and source hashes bind the actual decoded preload.
// This work finishes before the measured phase and its allocation counter.
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

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let entered = Instant::now();
    let args: Vec<_> = std::env::args_os().collect();
    if args.len() == 2 && args[1] == "--layout" {
        println!(
            "{}",
            serde_json::json!({
                "owned_construction_node": std::mem::size_of::<ts_ast::Node>(),
                "owned_construction_payload": std::mem::size_of::<ts_ast::NodeData>(),
                "owned_binding_snapshot": std::mem::size_of::<ts_ast::NodeBinding>(),
                "auxiliary": std::mem::size_of::<ts_ast::AstStorageData>(),
                "retained_core_total": "unavailable: use allocator and RSS captures; construction sizes do not describe compact headers and rows",
                "node_list": std::mem::size_of::<ts_ast::NodeList>(),
                "symbol": std::mem::size_of::<ts_ast::Symbol>(),
                "flow_node": std::mem::size_of::<ts_ast::FlowNode>(),
                "flow_list": std::mem::size_of::<ts_ast::FlowList>(),
                "js_string": std::mem::size_of::<JsString>(),
            })
        );
        return Ok(());
    }
    let graph_argument = args.get(3).and_then(|argument| argument.to_str());
    let graph_index = graph_argument
        .and_then(|argument| argument.strip_prefix("--graph-records="))
        .map(str::parse::<usize>)
        .transpose()?;
    let graph_mode =
        args.len() == 4 && (graph_argument == Some("--graphs") || graph_index.is_some());
    let local_binding_paths = args.len() == 4 && graph_argument == Some("--local-binding-paths");
    let binding_paths =
        local_binding_paths || args.len() == 4 && graph_argument == Some("--binding-paths");
    if args.len() != 3 && !graph_mode && !binding_paths {
        return Err(
            "usage: ts_bench INPUTS.json WORKERS (1 or 8) [--graphs | --graph-records=INDEX | --binding-paths | --local-binding-paths]"
                .into(),
        );
    }
    if (graph_mode || binding_paths) && cfg!(any(feature = "allocation", feature = "profile")) {
        return Err("graph reporting requires the uninstrumented binary".into());
    }
    let workers: usize = args[2].to_str().ok_or("invalid worker count")?.parse()?;
    if !matches!(workers, 1 | 8) {
        return Err("worker count must be 1 or 8".into());
    }
    let requests: Vec<Input> = serde_json::from_slice(&fs::read(Path::new(&args[1]))?)?;
    if requests.is_empty() {
        return Err("empty parse-and-bind workload".into());
    }
    if graph_index.is_some_and(|index| index >= requests.len()) {
        return Err("graph index outside workload".into());
    }
    let startup_ns = entered.elapsed().as_nanos();
    let preload_started = Instant::now();
    let inputs: Vec<_> = requests
        .into_iter()
        .map(|input| {
            Ok(Loaded {
                source: SourceText::from_bytes(fs::read(&input.local)?),
                options: SourceFileParseOptions {
                    file_name: JsString::from_bytes(input.filename.into_bytes()),
                    path: JsString::from_bytes(input.path.into_bytes()),
                    external_module_indicator_options: ExternalModuleIndicatorOptions {
                        jsx: input.jsx,
                        force: input.force,
                    },
                },
                script_kind: ts_core::ScriptKind(input.script_kind),
            })
        })
        .collect::<io::Result<_>>()?;
    let file_count = inputs.len();
    let loaded_bytes = inputs.iter().map(|input| input.source.len()).sum();
    let loaded_input_sha256 = loaded_digest(&inputs);
    let preload_ns = preload_started.elapsed().as_nanos();
    let worker_setup_started = Instant::now();
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
            let ready = &ready;
            let finished = &finished;
            let release = &release;
            handles.push(ts_parser::spawn_parser_worker(scope, move || {
                let mut retained: Vec<CompletedFile> =
                    Vec::with_capacity(file_count.div_ceil(workers));
                let mut failure = None;
                ready.wait();
                while let Ok(index) = receive.recv() {
                    if failure.is_some() {
                        continue;
                    }
                    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        let input = &inputs[index];
                        let parsed = ts_parser::parse_source_file(
                            input.source.clone(),
                            input.script_kind,
                            input.options.clone(),
                        );
                        ts_binder::bind_parsed_file(parsed).expect("workload binding must complete")
                    }));
                    match outcome {
                        Ok(file) => retained.push(file),
                        Err(panic) => failure = Some(panic),
                    }
                }
                finished.wait();
                release.wait();
                if let Some(panic) = failure {
                    std::panic::resume_unwind(panic);
                }
                retained
            })?);
        }
        ready.wait();
        let worker_setup_ns = worker_setup_started.elapsed().as_nanos();
        let cpu_capacity = thread::available_parallelism()?.get();
        #[cfg(feature = "allocation")]
        let allocated_before = ALLOCATOR.total_allocated();
        let start = Instant::now();
        for index in 0..file_count {
            senders[index % workers].send(index)?;
        }
        drop(senders);
        finished.wait();
        let wall_time_ns = start.elapsed().as_nanos();
        #[cfg(feature = "allocation")]
        let allocated_bytes = Some(
            ALLOCATOR
                .total_allocated()
                .checked_sub(allocated_before)
                .ok_or("allocation counter wrapped")?,
        );
        #[cfg(not(feature = "allocation"))]
        let allocated_bytes = None;
        release.wait();
        let files: Vec<_> = handles
            .into_iter()
            .map(|worker| {
                worker
                    .join()
                    .unwrap_or_else(|panic| std::panic::resume_unwind(panic))
            })
            .collect();
        let mut report = Report {
            version: 1,
            workers,
            files: 0,
            loaded_bytes,
            loaded_input_sha256,
            nodes: 0,
            symbols: 0,
            parse_diagnostics: 0,
            bind_diagnostics: 0,
            wall_time_ns,
            allocated_bytes,
            startup_ns,
            preload_ns,
            worker_setup_ns,
            cpu_capacity,
            goroutines_ready: None,
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
            return Err("workload file count mismatch".into());
        }
        if binding_paths {
            let local = files
                .iter()
                .flatten()
                .filter(|file| file.bound_with_local_scope())
                .count();
            let exclusive = files
                .iter()
                .flatten()
                .filter(|file| file.bound_in_place())
                .count();
            let mut paths = serde_json::json!({
                "version": 1, "workers": workers, "files": file_count,
                "bound_in_place_files": exclusive, "fallback_files": file_count - exclusive,
                "loaded_input_sha256": report.loaded_input_sha256,
            });
            if local_binding_paths {
                paths["local_scope_files"] = local.into();
                paths["checked_scope_files"] = (file_count - local).into();
            }
            println!("{paths}");
            std::hint::black_box(&files);
            return Ok(());
        }
        if graph_mode {
            let stdout = io::stdout();
            let mut stdout = io::BufWriter::new(stdout.lock());
            if let Some(index) = graph_index {
                workload_graph::write_records(
                    &mut stdout,
                    &files[index % workers][index / workers],
                    index,
                    workers,
                )?;
            } else {
                for index in 0..file_count {
                    workload_graph::write_report(
                        &mut stdout,
                        &files[index % workers][index / workers],
                        &inputs[index],
                        index,
                        workers,
                    )?;
                }
            }
            stdout.flush()?;
            std::hint::black_box(&files);
            return Ok(());
        }
        #[cfg(feature = "allocation")]
        eprintln!(
            "{}",
            serde_json::json!({"retained_requested_bytes":ALLOCATOR.allocated(),"peak_requested_bytes":ALLOCATOR.max_allocated()})
        );
        #[cfg(feature = "profile")]
        {
            let mut bindings = 0_usize;
            let mut empty = 0_usize;
            let mut flow_only = 0_usize;
            let mut node_kinds = std::collections::BTreeMap::<&str, usize>::new();
            let mut map_capacities = [0_usize; 3];
            for file in files.iter().flatten() {
                for (kind, count) in file.view().ast().layout_profile() {
                    *node_kinds.entry(kind).or_default() += count;
                }
                for (total, count) in map_capacities
                    .iter_mut()
                    .zip(file.view().result().layout_profile())
                {
                    *total += count;
                }
                for (_, binding) in file.view().result().bindings(file.view().ast()) {
                    bindings += 1;
                    if binding.symbol.is_none()
                        && binding.local_symbol.is_none()
                        && binding.locals.is_none()
                        && binding.next_container.is_none()
                        && binding.return_flow_node.is_none()
                        && binding.end_flow_node.is_none()
                        && binding.fallthrough_flow_node.is_none()
                    {
                        if binding.flow_node.is_some() {
                            flow_only += 1;
                        } else {
                            empty += 1;
                        }
                    }
                }
            }
            eprintln!(
                "{}",
                serde_json::json!({"bindings":bindings,"empty":empty,"flow_only":flow_only,"node_kinds":node_kinds,"map_capacities":map_capacities})
            );
        }
        let stdout = io::stdout();
        let mut stdout = stdout.lock();
        serde_json::to_writer(&mut stdout, &report)?;
        stdout.write_all(b"\n")?;
        stdout.flush()?;
        std::hint::black_box(&files);
        Ok(())
    })
}
