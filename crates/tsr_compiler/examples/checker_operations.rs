//! Independent development intervals around production checker operations.
//! Each invocation constructs a fresh program/checker. Output comparison and
//! source hashing occur after the interval; run both readers in separate
//! processes with tools/phase7/bench/operations.py.

use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{path::Path, sync::Arc, time::Instant};
use tsr_arena::{CheckerIdentity, Counters, Generation};
use tsr_ast::Diagnostic;
use tsr_checker::{
    operation_experiments::{ArgumentBackend, ArgumentReport, ObservationMode},
    CheckerOwner,
};
use tsr_compiler::{CompilerConfigHost, FileCache, Program, ProgramCheckerHost, ProgramOptions};
use tsr_core::{CompilerOptions, Tristate};
use tsr_jsstring::JsString;
use tsr_vfs::FileSystem;

#[global_allocator]
static ALLOCATOR: mimalloc::MiMalloc = mimalloc::MiMalloc;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

fn sha(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn diagnostic(program: &Program, value: &Diagnostic) -> Result<Value> {
    let file = value
        .file
        .map(|id| {
            let source = if let Some(file) = program.file_of_node(id) {
                file.bound().view().source_file()?
            } else {
                let config = program
                    .config_source(id)
                    .ok_or("diagnostic file not retained")?;
                config.file.view().source_file(config.root)?
            };
            Ok::<_, Box<dyn std::error::Error>>(source.file_name().to_vec())
        })
        .transpose()?;
    Ok(json!({
        "file": file, "start": value.loc.pos(), "end": value.loc.end(),
        "code": value.code, "category": value.category,
        "key": value.message_key.as_bytes(), "text": value.message_text.as_bytes(),
        "source": value.source.as_bytes(),
        "args": value.message_args.iter().map(JsString::as_bytes).collect::<Vec<_>>(),
        "chain": value.message_chain.iter().map(|d| diagnostic(program, d)).collect::<Result<Vec<_>>>()?,
        "related": value.related_information.iter().map(|d| diagnostic(program, d)).collect::<Result<Vec<_>>>()?,
        "unnecessary": value.reports_unnecessary, "deprecated": value.reports_deprecated,
        "skipped_on_no_emit": value.skipped_on_no_emit,
    }))
}

fn argument_report(report: &ArgumentReport, mode: ObservationMode) -> Value {
    let mut output = json!({
        "calls": report.calls, "outermost_calls": report.outermost_calls,
        "errors": report.errors, "location_errors": report.location_errors,
        "cleanup_errors": report.cleanup_errors,
        "elapsed_ns": report.elapsed.as_nanos(),
    });
    if mode == ObservationMode::Observe {
        output["trace_checksum"] = json!(report.trace_checksum);
        output["kinds"] = json!(report.kinds);
        output["modes"] = json!(report.modes);
        output["depths"] = json!(report.depths);
        output["inferential_calls"] = json!(report.inferential_calls);
        output["samples"] = json!(report.samples.iter().map(|sample| json!({
            "ordinal": sample.ordinal, "depth": sample.depth, "file": sample.file,
            "start": sample.pos, "end": sample.end, "kind": sample.kind,
            "contextual_type": sample.contextual_type, "inferential": sample.inferential,
            "mode": sample.mode, "result_type": sample.result_type, "error": sample.error,
            "types_before": sample.types_before, "types_after": sample.types_after,
            "node_slot": sample.node_slot, "checker_owned": sample.checker_owned,
            "location_error": sample.location_error, "inference_context": sample.inference_context,
            "context_depth_before": sample.context_depth_before, "context_depth_after": sample.context_depth_after,
            "inference_depth_before": sample.inference_depth_before, "inference_depth_after": sample.inference_depth_after,
            "expression_mode_before": sample.expression_mode_before, "expression_mode_after": sample.expression_mode_after,
            "current_node_restored": sample.current_node_restored, "read_scope_restored": sample.read_scope_restored,
        })).collect::<Vec<_>>());
    }
    output
}

fn run() -> Result<Value> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    if args.len() != 4 {
        return Err(
            "usage: checker_operations ABSOLUTE_TSCONFIG argument|flow routed|scoped|typed|state|combined observe|time"
                .into(),
        );
    }
    let [config_path, experiment, backend, mode] = &args[..] else {
        unreachable!();
    };
    if !Path::new(config_path).is_absolute() || !matches!(experiment.as_str(), "argument" | "flow")
    {
        return Err("expected an absolute tsconfig path and argument|flow experiment".into());
    }
    let backend = match backend.as_str() {
        "routed" => ArgumentBackend::Routed,
        "scoped" => ArgumentBackend::ScopedCore,
        "typed" => ArgumentBackend::TypedPayload,
        "state" => ArgumentBackend::ExecutionState,
        "combined" => ArgumentBackend::Combined,
        _ => return Err("unknown operation backend".into()),
    };
    if experiment == "flow"
        && !matches!(
            backend,
            ArgumentBackend::Routed | ArgumentBackend::ScopedCore
        )
    {
        return Err("flow backend must be routed or scoped".into());
    }
    let mode = match mode.as_str() {
        "observe" => ObservationMode::Observe,
        "time" => ObservationMode::Time,
        _ => return Err("mode must be observe or time".into()),
    };
    let start = Instant::now();
    let cwd = std::env::current_dir()?;
    let cwd = cwd
        .to_str()
        .ok_or("experiment current directory is not UTF-8")?
        .as_bytes();
    let fs: Arc<dyn FileSystem> = Arc::new(tsr_bundled::BundledFs::new(tsr_vfs::os::shared_fs()));
    let host = CompilerConfigHost::new_live(fs.clone(), JsString::from_bytes(cwd));
    let overrides = CompilerOptions {
        no_emit: Tristate::TRUE,
        incremental: Tristate::FALSE,
        composite: Tristate::FALSE,
        ..CompilerOptions::default()
    };
    let parsed = tsr_tsoptions::get_parsed_command_line_of_config_file(
        config_path.as_bytes(),
        &overrides,
        &tsr_tsoptions::ConfigValue::Null,
        &host,
    )?;
    let config = parsed
        .command_line
        .ok_or("could not read project configuration")?;
    if config.options.watch.is_true()
        || config.options.run_external_code.is_true()
        || config
            .content_mappers
            .as_ref()
            .is_some_and(|mappers| !mappers.is_empty())
    {
        return Err("operation experiments do not execute watch or content mappers".into());
    }
    let counters = Counters::default();
    let program = Arc::new(Program::load_live(
        ProgramOptions {
            config,
            host: fs,
            current_directory: JsString::from_bytes(cwd),
            default_library_path: JsString::from_bytes(b"bundled:///libs".as_slice()),
            skip_module_resolution: false,
            single_threaded: Tristate::TRUE,
        },
        &mut FileCache::new(),
        &counters,
    )?);
    let checker = Arc::new(CheckerOwner::for_program(
        CheckerIdentity::new(Generation::new(&counters), &counters),
        &counters,
        Arc::new(ProgramCheckerHost::new(program.clone())),
    )?);
    let mut op = checker.operation()?;
    let setup_ns = start.elapsed().as_nanos();
    let mut diagnostics = program.config().config_file_parsing_diagnostics();
    diagnostics.extend_from_slice(program.program_diagnostics()?);
    diagnostics.extend(program.syntactic_diagnostics(None)?);
    diagnostics.extend(op.global_diagnostics()?);
    if experiment == "argument" {
        op.begin_argument_experiment(backend, mode)?;
    } else {
        op.begin_flow_capture(100_000, 1)?;
    }
    let semantic_start = Instant::now();
    for file in program.files() {
        diagnostics.extend(
            program
                .semantic_diagnostics_with_checker(&mut op, file)
                .map_err(|error| {
                    format!(
                        "checking {}: {error}",
                        String::from_utf8_lossy(
                            file.bound()
                                .view()
                                .source_file()
                                .map_or(b"?".as_slice(), |source| source.file_name())
                        )
                    )
                })?,
        );
    }
    let semantic_ns = semantic_start.elapsed().as_nanos();
    let (frame_capacity, frame_retained_bytes) = op.argument_frame_storage();
    let types_after_check = op.type_count();
    let operations = if experiment == "argument" {
        argument_report(&op.finish_argument_experiment(), mode)
    } else {
        let capture = op.finish_flow_capture();
        // Normalize only process-global arena identities; source and local flow
        // slots preserve the exact ordered request schedule across processes.
        let mut request_hash = Sha256::new();
        for request in &capture.requests {
            let file = program
                .file_of_node(request.owner)
                .ok_or("captured flow owner not retained")?;
            let source = file.bound().view().source_file()?;
            let path = source.file_name();
            request_hash.update(u64::try_from(path.len())?.to_le_bytes());
            request_hash.update(path);
            request_hash.update(request.owner.slot().to_le_bytes());
            request_hash.update(request.flow.slot().to_le_bytes());
        }
        let request_sha256 = format!("{:x}", request_hash.finalize());
        let replay_start = Instant::now();
        let replay =
            op.replay_flow_reachability(&capture.requests, backend == ArgumentBackend::ScopedCore);
        let elapsed_ns = replay_start.elapsed().as_nanos();
        json!({
            "requests": capture.requests.len(), "total_seen": capture.total_seen,
            "truncated": capture.truncated, "request_sha256": request_sha256,
            "result_sha256": sha(&serde_json::to_vec(&replay.results)?),
            "errors": replay.results.iter().filter(|result| result.is_err()).count(),
            "local_requests": replay.local_requests, "routed_requests": replay.routed_requests,
            "node_reads": replay.node_reads, "list_reads": replay.list_reads,
            "semantic_calls": replay.semantic_calls, "elapsed_ns": elapsed_ns,
            "types_after_check": types_after_check,
        })
    };
    let diagnostics = program.sort_and_deduplicate_diagnostics(&diagnostics)?;
    let diagnostics_json = diagnostics
        .iter()
        .map(|d| diagnostic(&program, d))
        .collect::<Result<Vec<_>>>()?;
    let options_sha256 = sha(&tsr_tsoptions::options_json::marshal_compiler_options(
        program.options(),
    )?);
    let mut configurations = Vec::new();
    for config in program
        .config()
        .config_file
        .iter()
        .chain(&program.config().config_dependencies)
    {
        let source = config.file.view().source_file(config.root)?;
        configurations.push(json!({"path": std::str::from_utf8(source.file_name())?, "sha256": sha(source.text().as_bytes())}));
    }
    let mut files = Vec::with_capacity(program.files().len());
    for file in program.files() {
        let source = file.bound().view().source_file()?;
        files.push(json!({"path": std::str::from_utf8(source.file_name())?, "sha256": sha(source.text().as_bytes()),
            "options_sha256": sha(&tsr_tsoptions::options_json::marshal_compiler_options(
                program.options_for_file(source.parse_options().path.as_bytes(), source.file_name()))?)}));
    }
    Ok(json!({
        "format": 1, "experiment": experiment, "backend": args[2], "mode": args[3],
        "complete": true, "setup_ns": setup_ns, "semantic_ns": semantic_ns,
        "workload": {"files": files, "options_sha256": options_sha256, "configurations": configurations,
            "diagnostics_sha256": sha(&serde_json::to_vec(&diagnostics_json)?),
            "diagnostics_count": diagnostics.len(), "types": op.type_count(),
            "signatures": op.signature_count(), "instantiations": op.instantiation_count()},
        "operations": operations,
        "experiment_storage": {"frame_capacity": frame_capacity, "frame_retained_bytes": frame_retained_bytes},
    }))
}

fn main() {
    match run() {
        Ok(report) => {
            // Counting runs are separate from uninstrumented compiler timings.
            // Count the extra payload retains separately from existing page guards.
            #[cfg(feature = "access-stats")]
            eprintln!("node-read-counts {}", tsr_ast::access_stats::report());
            #[cfg(all(feature = "access-stats", feature = "compact-node-read"))]
            eprintln!(
                "lazy-read-node-retains {}",
                tsr_ast::access_stats::lazy_read_node_retains()
            );
            println!("{report}");
        }
        Err(error) => {
            eprintln!("checker operation experiment failed: {error}");
            std::process::exit(1);
        }
    }
}
