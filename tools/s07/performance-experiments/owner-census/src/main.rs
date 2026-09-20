//! Untimed physical owner census over the complete frozen workload.
mod observe;
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
    if args.len() != 3 {
        return Err("usage: tsr_s07_bis_owner_census INPUTS.json OUTPUT.ndjson".into());
    }
    let inputs = preload(Path::new(&args[1]))?;
    let loaded_input_sha256 = loaded_digest(&inputs);
    let loaded_bytes: usize = inputs.iter().map(|input| input.source.len()).sum();
    let files = thread::scope(|scope| {
        tsr_parser::spawn_parser_worker(scope, || {
            inputs
                .iter()
                .map(|input| {
                    let parsed = tsr_parser::parse_source_file(
                        input.source.clone(),
                        input.script_kind,
                        input.options.clone(),
                    );
                    tsr_binder::bind_parsed_file(parsed)
                        .expect("frozen workload binding must complete")
                })
                .collect::<Vec<_>>()
        })
        .map(|worker| {
            worker
                .join()
                .unwrap_or_else(|panic| std::panic::resume_unwind(panic))
        })
    })?;
    let mut nodes = 0i64;
    let mut symbols = 0isize;
    let (mut parse_diagnostics, mut bind_diagnostics, mut bound_in_place_files) =
        (0usize, 0usize, 0usize);
    for file in &files {
        let view = file.view();
        let source = view.source_file()?;
        nodes += source.node_count;
        symbols += view.result().symbol_count();
        parse_diagnostics += source.diagnostics.len();
        bind_diagnostics += view.result().diagnostics().len();
        bound_in_place_files += usize::from(file.bound_in_place());
    }
    let mut output = io::BufWriter::new(
        fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&args[2])?,
    );
    for (index, file) in files.iter().enumerate() {
        serde_json::to_writer(
            &mut output,
            &observe::observe(index, file.view(), file.bound_in_place()),
        )?;
        writeln!(output)?;
    }
    output.flush()?;
    println!(
        "{}",
        serde_json::json!({"version":1,"diagnostic_only":true,"runtime":"rust","workers":1,
        "files":files.len(),"loaded_bytes":loaded_bytes,"loaded_input_sha256":loaded_input_sha256,
        "nodes":nodes,"symbols":symbols,"parse_diagnostics":parse_diagnostics,"bind_diagnostics":bind_diagnostics,
        "bound_in_place_files":bound_in_place_files,"fallback_files":files.len()-bound_in_place_files,
        "domain":"physical owned records after complete retained parse-bind endpoint; no timing or allocator measurement"})
    );
    io::stdout().flush()?;
    black_box(&files);
    Ok(())
}
