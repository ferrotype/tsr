use crate::{write_all, CommandLineTesting, System};
use std::sync::Arc;
use tsr_compiler::CheckedProgram;
use tsr_tracing::Tracing;
use tsr_tsoptions::ParsedCommandLine;

// port: tsc/internal/execute/tsc.go:startTracingIfNeeded
pub fn start_tracing_if_needed(
    sys: &dyn System,
    config: &ParsedCommandLine,
    testing: Option<&dyn CommandLineTesting>,
) -> Option<Arc<Tracing>> {
    if config.options.generate_trace.is_empty() {
        return None;
    }
    let config_file_path = config.config_file.as_ref().map_or_else(Vec::new, |file| {
        file.file
            .view()
            .source_file(file.root)
            .map(|source| source.file_name().to_vec())
            .unwrap_or_default()
    });
    match tsr_tracing::start_tracing(
        sys.fs(),
        config.options.generate_trace.as_bytes(),
        &config_file_path,
        testing.is_some(),
    ) {
        Ok(tracing) => Some(tracing),
        Err(error) => {
            write_all(
                sys.writer().as_ref(),
                format!("Warning: Failed to start tracing: {error}\n").as_bytes(),
            );
            None
        }
    }
}

// port: tsc/internal/execute/tsc.go:stopTracing
pub fn stop_tracing(sys: &dyn System, tracing: Option<&Tracing>, program: &CheckedProgram) {
    let Some(tracing) = tracing else {
        return;
    };
    if let Err(error) = tracing.stop_tracing(|checker_index, ids| {
        program
            .trace_type_records(checker_index, ids)
            .map_err(|error| error.to_string())
    }) {
        write_all(
            sys.writer().as_ref(),
            format!("Warning: Failed to stop tracing: {error}\n").as_bytes(),
        );
    }
}
