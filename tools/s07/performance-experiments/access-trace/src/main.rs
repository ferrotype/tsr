//! Untimed access capture over the exact retained parse/bind workload.
#[path = "../../../../../crates/tsr_binder/examples/support/graph.rs"]
mod binder_graph;
#[allow(dead_code)]
#[path = "../../../../../crates/tsr_bench/src/workload_graph.rs"]
mod workload_graph;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{
    fs,
    hint::black_box,
    io::{self, Write},
    path::Path,
    thread,
};
use tsr_ast::{ExternalModuleIndicatorOptions, JsString, SourceFileParseOptions};
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
fn preload(path: &Path) -> Result<Vec<Loaded>, Box<dyn std::error::Error>> {
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

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args_os().collect();
    if args.len() != 5 {
        return Err("usage: tsr_s07_bis_access_trace INPUTS.json SUMMARY.json GRAPHS.ndjson PAYLOAD_LIMIT (binary trace on stdout)".into());
    }
    let inputs = preload(Path::new(&args[1]))?;
    let payload_limit: u64 = args[4]
        .to_str()
        .ok_or("payload limit is not text")?
        .parse()?;
    let loaded_input_sha256 = loaded_digest(&inputs);
    let loaded_bytes: usize = inputs.iter().map(|input| input.source.len()).sum();
    let files = thread::scope(|scope| {
        tsr_parser::spawn_parser_worker(scope, || {
            use tsr_ast::access_trace as trace;
            trace::start(Box::new(io::stdout()), payload_limit).expect("start trace");
            let mut files = Vec::with_capacity(inputs.len());
            let (mut node_total, mut symbol_total) = (0u64, 0u64);
            for (index, input) in inputs.iter().enumerate() {
                let ordinal = u32::try_from(index + 1).expect("file ordinal fits");
                trace::context(ordinal, 0);
                trace::event(1, 0, index as u64, input.source.len() as u64, 0, 0);
                trace::event(2, 0, 0, 0, 0, 0);
                let parsed = tsr_parser::parse_source_file(
                    input.source.clone(),
                    input.script_kind,
                    input.options.clone(),
                );
                trace::event(
                    3,
                    0,
                    parsed.root().bits(),
                    parsed
                        .view()
                        .source_file(parsed.root())
                        .expect("source")
                        .node_count as u64,
                    0,
                    0,
                );
                trace::event(4, 0, 0, 0, 0, 0);
                trace::context(ordinal, 1);
                tsr_ast::access_trace_state::export(parsed.view());
                trace::context(ordinal, 0);
                trace::event(5, 0, 0, 0, 0, 0);
                trace::event(6, 0, 0, 0, 0, 0);
                trace::context(ordinal, 2);
                let file = tsr_binder::bind_parsed_file(parsed)
                    .expect("frozen workload binding must complete");
                trace::context(ordinal, 0);
                let view = file.view();
                let source = view.source_file().expect("bound source");
                let nodes = u64::try_from(source.node_count).expect("nonnegative nodes");
                let symbols =
                    u64::try_from(view.result().symbol_count()).expect("nonnegative symbols");
                trace::event(
                    7,
                    0,
                    nodes,
                    symbols,
                    source.diagnostics.len() as u64,
                    view.result().diagnostics().len() as u64,
                );
                trace::event(8, 0, u64::from(file.bound_in_place()), 0, 0, 0);
                node_total += nodes;
                symbol_total += symbols;
                files.push(file);
            }
            trace::context(0, 0);
            trace::event(
                9,
                0,
                files.len() as u64,
                loaded_bytes as u64,
                node_total,
                symbol_total,
            );
            trace::finish().expect("complete trace");
            files
        })
        .map(|worker| {
            worker
                .join()
                .unwrap_or_else(|panic| std::panic::resume_unwind(panic))
        })
    })?;
    // The recorder is inactive here. Graph encoding cannot become binder events
    // or alter the parse/bind runtime-ID observations in the captured stream.
    let mut graphs = io::BufWriter::new(
        fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&args[3])?,
    );
    for (index, file) in files.iter().enumerate() {
        workload_graph::write_report(&mut graphs, file, &inputs[index], index, 1)?;
    }
    graphs.flush()?;
    let (mut nodes, mut symbols, mut parse_diagnostics, mut bind_diagnostics, mut exclusive) =
        (0i64, 0isize, 0usize, 0usize, 0usize);
    for file in &files {
        let view = file.view();
        let source = view.source_file()?;
        nodes += source.node_count;
        symbols += view.result().symbol_count();
        parse_diagnostics += source.diagnostics.len();
        bind_diagnostics += view.result().diagnostics().len();
        exclusive += usize::from(file.bound_in_place());
    }
    let summary = serde_json::json!({"version":1,"diagnostic_only":true,"runtime":"rust","workers":1,
        "files":files.len(),"loaded_bytes":loaded_bytes,"loaded_input_sha256":loaded_input_sha256,
        "nodes":nodes,"symbols":symbols,"parse_diagnostics":parse_diagnostics,"bind_diagnostics":bind_diagnostics,
        "bound_in_place_files":exclusive,"fallback_files":files.len()-exclusive,
        "payload_limit":payload_limit,"timing_claim":false,
        "domain":"serial retained parse-bind, pre-bind state observer and scoped binder operations; graph export after recorder completion"});
    let mut output = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&args[2])?;
    serde_json::to_writer_pretty(&mut output, &summary)?;
    writeln!(output)?;
    output.flush()?;
    black_box(&files);
    Ok(())
}
