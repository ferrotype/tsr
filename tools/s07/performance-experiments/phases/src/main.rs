//! Same-revision, one-worker attribution of A0's ownership transition.
//! Both backends group binding and publication, including final validation.
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
    BoundFile, BoundView, CompletedFile, ExternalModuleIndicatorOptions, JsString, ParsedFile,
    SourceFileParseOptions,
};
use tsr_jsstring::SourceText;

#[global_allocator]
static ALLOCATOR: mimalloc::MiMalloc = mimalloc::MiMalloc;

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
enum Backend {
    Published,
    Consuming,
}

enum RetainedFile {
    Published(BoundFile),
    Consuming(CompletedFile),
}
impl RetainedFile {
    fn view(&self) -> BoundView<'_> {
        match self {
            Self::Published(file) => file.view(),
            Self::Consuming(file) => file.view(),
        }
    }
    fn bound_in_place(&self) -> bool {
        match self {
            Self::Published(_) => false,
            Self::Consuming(file) => file.bound_in_place(),
        }
    }
}

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
struct PhaseTimes {
    parse_ns: u128,
    bind_and_publication_ns: u128,
}
#[derive(Serialize)]
struct Report {
    version: u32,
    runtime: &'static str,
    diagnostic_only: bool,
    backend: Backend,
    workers: usize,
    files: usize,
    loaded_bytes: usize,
    loaded_input_sha256: String,
    nodes: i64,
    symbols: isize,
    parse_diagnostics: usize,
    bind_diagnostics: usize,
    bound_in_place_files: usize,
    published_files: usize,
    shapes_emitted: bool,
    pipeline_wall_ns: u128,
    elapsed_worker_totals: PhaseTimes,
    timer_domain: &'static str,
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

#[inline(never)]
fn profile_parse(input: &Loaded) -> ParsedFile {
    black_box(tsr_parser::parse_source_file(
        input.source.clone(),
        input.script_kind,
        input.options.clone(),
    ))
}

#[inline(never)]
fn profile_binding_and_publication(parsed: ParsedFile, backend: Backend) -> RetainedFile {
    let file = match backend {
        Backend::Published => {
            let source = parsed.root();
            let file = parsed.publish_unbound();
            let bound = tsr_binder::bind_source_file(&file, source)
                .expect("diagnostic workload binding must complete");
            drop(file);
            RetainedFile::Published(bound)
        }
        Backend::Consuming => RetainedFile::Consuming(
            tsr_binder::bind_parsed_file(parsed).expect("diagnostic workload binding must complete"),
        ),
    };
    black_box(file)
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args_os().collect();
    if args.len() != 3 && !(args.len() == 5 && args[3] == "--shapes") {
        return Err("usage: tsr_s07_bis_phases INPUTS.json published|consuming [--shapes OUTPUT.ndjson] (one worker)".into());
    }
    let backend = match args[2].to_str() {
        Some("published") => Backend::Published,
        Some("consuming") => Backend::Consuming,
        _ => return Err("backend must be published or consuming".into()),
    };
    let inputs = preload(Path::new(&args[1]))?;
    let file_count = inputs.len();
    let loaded_bytes = inputs.iter().map(|input| input.source.len()).sum();
    let loaded_input_sha256 = loaded_digest(&inputs);
    let shape_path = args.get(4);
    let ready = Barrier::new(2);
    let finished = Barrier::new(2);
    let release = Barrier::new(2);
    thread::scope(|scope| -> Result<(), Box<dyn std::error::Error>> {
        let (send, receive) = mpsc::sync_channel::<usize>(1);
        let inputs = &inputs;
        let (ready, finished, release) = (&ready, &finished, &release);
        let worker = tsr_parser::spawn_parser_worker(scope, move || {
            let mut roots = Vec::with_capacity(file_count);
            let mut times = PhaseTimes::default();
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
                    let completed = profile_binding_and_publication(parsed, backend);
                    times.bind_and_publication_ns += start.elapsed().as_nanos();
                    completed
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
        })?;
        ready.wait();
        let start = Instant::now();
        for index in 0..file_count {
            send.send(index).expect("worker drains every input");
        }
        drop(send);
        finished.wait();
        let pipeline_wall_ns = start.elapsed().as_nanos();
        release.wait();
        let (files, times) = worker
            .join()
            .unwrap_or_else(|panic| std::panic::resume_unwind(panic));
        let mut report = Report {
            version: 1, runtime: "rust", diagnostic_only: true, backend, workers: 1,
            files: 0, loaded_bytes, loaded_input_sha256, nodes: 0, symbols: 0,
            parse_diagnostics: 0, bind_diagnostics: 0, bound_in_place_files: 0,
            published_files: 0, shapes_emitted: shape_path.is_some(),
            pipeline_wall_ns, elapsed_worker_totals: times,
            timer_domain: "one-worker per-file elapsed; bind_and_publication includes the complete transition and final validation; not sampled CPU or acceptance wall time",
        };
        for file in &files {
            let view = file.view();
            let source = view.source_file()?;
            report.files += 1;
            report.nodes += source.node_count;
            report.symbols += view.result().symbol_count();
            report.parse_diagnostics += source.diagnostics.len();
            report.bind_diagnostics += view.result().diagnostics().len();
            if file.bound_in_place() {
                report.bound_in_place_files += 1;
            } else {
                report.published_files += 1;
            }
        }
        if report.files != file_count {
            return Err("incomplete workload".into());
        }
        if let Some(path) = shape_path {
            // This traversal/allocation is after every measured endpoint. Its
            // output is an owned core census, never a phase or RSS measurement.
            let mut output = io::BufWriter::new(
                fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(path)?,
            );
            for (index, file) in files.iter().enumerate() {
                let counts = file.view().ast().layout_profile();
                serde_json::to_writer(
                    &mut output,
                    &serde_json::json!({
                        "index": index, "core_shapes": counts,
                        "bound_in_place": file.bound_in_place(),
                    }),
                )?;
                writeln!(output)?;
            }
            output.flush()?;
        }
        println!("{}", serde_json::to_string(&report)?);
        io::stdout().flush()?;
        // All roots are held through both phase and pipeline endpoints.
        black_box(&files);
        drop(files);
        Ok(())
    })
}
