//! The driver's diagnostic collection, emit and console reporting sequence.
use crate::{CommandLineTesting, DiagnosticReporter, ExitStatus, System};
use std::cell::Cell;
use std::time::Duration;
use tsr_ast::Diagnostic;
use tsr_checker::CheckerRequest;
use tsr_compiler::{CheckedProgram, EmitOptions, EmitResult, Error, ProgramLike};
use tsr_tsoptions::ParsedCommandLine;
use tsr_vfs::iofs::Time;

#[derive(Clone, Default)]
pub struct CompileTimes {
    pub config_time: Duration,
    pub parse_time: Duration,
    pub bind_time: Duration,
    pub check_time: Duration,
    pub emit_time: Duration,
    pub total_time: Duration,
    pub build_info_read_time: Duration,
    pub changes_compute_time: Duration,
    pub content_mapper_times: tsr_contentmapper::Timings,
}

pub fn elapsed(end: Time, start: Time) -> Duration {
    let (end_s, end_ns) = end.unix();
    let (start_s, start_ns) = start.unix();
    let nanos = (i128::from(end_s) - i128::from(start_s)) * 1_000_000_000 + i128::from(end_ns)
        - i128::from(start_ns);
    let nanos = u128::try_from(nanos).unwrap_or_default();
    Duration::new(
        u64::try_from(nanos / 1_000_000_000).unwrap_or(u64::MAX),
        (nanos % 1_000_000_000) as u32,
    )
}

pub struct CompileAndEmitResult {
    pub diagnostics: Vec<Diagnostic>,
    pub emit_result: EmitResult,
    pub status: ExitStatus,
    pub statistics: Option<crate::Statistics>,
}

pub struct EmitInput<'a> {
    pub sys: &'a dyn System,
    pub program_like: &'a dyn ProgramLike,
    pub program: &'a CheckedProgram,
    pub incremental: Option<&'a tsr_incremental::Program>,
    pub config: &'a ParsedCommandLine,
    pub report_diagnostic: &'a DiagnosticReporter,
    pub times: &'a mut CompileTimes,
    pub testing: Option<&'a dyn CommandLineTesting>,
    /// Build tasks write their whole transcript into a task-local buffer.
    pub writer: Option<&'a crate::SharedWriter>,
    pub skip_error_summary: bool,
    pub write_file: Option<&'a tsr_compiler::WriteFile<'a>>,
    pub testing_m_times_cache:
        Option<&'a tsr_core::collections::SyncMap<tsr_jsstring::JsString, Time>>,
}

/// port: tsc/internal/execute/tsc/emit.go:EmitFilesAndReportErrors
pub fn emit_files_and_report_errors(input: EmitInput<'_>) -> Result<CompileAndEmitResult, Error> {
    if let Some(testing) = input.testing {
        testing.on_compiler_program(input.program);
    }
    // Deliberately background: the pinned driver chooses Background here even
    // when its outer command has a cancellation context.
    let request = CheckerRequest::default();
    let bind_time = Cell::new(Duration::ZERO);
    let check_time = Cell::new(Duration::ZERO);
    let nested_time = Cell::new(Duration::ZERO);
    let mut diagnostics = tsr_compiler::get_diagnostics_of_any_program(
        input.program_like,
        &request,
        None,
        false,
        &|file| {
            let _trace = tsr_checker::TraceScope::new(
                input.program.tracing(),
                tsr_checker::TracePhase::Bind,
                "bindSourceFiles",
                Default::default,
                true,
            );
            let start = input.sys.now();
            let diagnostics = input.program_like.bind_diagnostics(&request, file)?;
            bind_time.set(elapsed(input.sys.now(), start));
            Ok(diagnostics)
        },
        &|file| {
            let _trace = tsr_checker::TraceScope::new(
                input.program.tracing(),
                tsr_checker::TracePhase::Check,
                "checkSourceFiles",
                Default::default,
                true,
            );
            let start = input.sys.now();
            let diagnostics = input.program_like.semantic_diagnostics(&request, file)?;
            let duration = elapsed(input.sys.now(), start);
            let nested = input.incremental.map_or(
                Duration::ZERO,
                tsr_incremental::Program::take_nested_emit_time,
            );
            nested_time.set(nested_time.get() + nested);
            check_time.set(duration.saturating_sub(nested));
            Ok(diagnostics)
        },
    )?;
    input.times.bind_time = bind_time.get();
    input.times.check_time = check_time.get();
    input.times.emit_time += nested_time.get();
    let result = if input.program_like.options().list_files_only.is_true() {
        EmitResult {
            emit_skipped: true,
            ..EmitResult::default()
        }
    } else {
        let start = input.sys.now();
        let result = input.program_like.emit(
            &request,
            &EmitOptions {
                write_file: input.write_file,
                ..EmitOptions::default()
            },
        )?;
        input.times.emit_time += elapsed(input.sys.now(), start);
        result.expect("background emit cannot be canceled")
    };
    diagnostics.extend_from_slice(&result.diagnostics);
    if let Some(testing) = input.testing {
        testing.on_emitted_files(Some(&result), input.testing_m_times_cache);
    }
    let program = input.program.program();
    let diagnostics = program.sort_and_deduplicate_diagnostics(&diagnostics)?;
    for diagnostic in &diagnostics {
        input
            .report_diagnostic
            .report(program.as_ref(), diagnostic)?;
    }
    let writer = input.writer.cloned().unwrap_or_else(|| input.sys.writer());
    list_files(&writer, input.program, &result, input.config, input.testing)?;
    if !input.skip_error_summary {
        crate::create_report_error_summary(
            input.sys,
            input.config.locale().clone(),
            &input.config.options,
        )
        .report(program.as_ref(), &diagnostics)?;
    }
    Ok(CompileAndEmitResult {
        diagnostics,
        emit_result: result,
        status: ExitStatus::Success,
        statistics: None,
    })
}

/// port: tsc/internal/execute/tsc/emit.go:EmitAndReportStatistics
pub fn emit_and_report_statistics(input: EmitInput<'_>) -> Result<CompileAndEmitResult, Error> {
    let sys = input.sys;
    let program = input.program;
    let report_statistics = input.config.options.diagnostics.is_true()
        || input.config.options.extended_diagnostics.is_true();
    let writer = input.writer.cloned().unwrap_or_else(|| sys.writer());
    let testing = input.testing;
    let times = &mut *input.times;
    let input = EmitInput {
        times: &mut *times,
        ..input
    };
    let mut result = emit_files_and_report_errors(input)?;
    times.total_time = sys.since_start();
    if report_statistics {
        let statistics = crate::Statistics::from_program(program, times, sys.memory_statistics())?;
        statistics.report(&writer, testing);
        result.statistics = Some(statistics);
    }
    if !result.diagnostics.is_empty() {
        result.status = if result.emit_result.emit_skipped {
            ExitStatus::DiagnosticsPresent_OutputsSkipped
        } else {
            ExitStatus::DiagnosticsPresent_OutputsGenerated
        };
    }
    Ok(result)
}

/// port: tsc/internal/execute/tsc/emit.go:listFiles
fn list_files(
    writer: &crate::SharedWriter,
    checked: &CheckedProgram,
    result: &EmitResult,
    config: &ParsedCommandLine,
    testing: Option<&dyn CommandLineTesting>,
) -> Result<(), Error> {
    struct End<'a>(Option<&'a dyn CommandLineTesting>, crate::SharedWriter);
    impl Drop for End<'_> {
        fn drop(&mut self) {
            if let Some(testing) = self.0 {
                testing.on_list_files_end(&self.1);
            }
        }
    }
    if let Some(testing) = testing {
        testing.on_list_files_start(&writer);
    }
    let _end = End(testing, writer.clone());
    let program = checked.program();
    let options = program.options();
    if options.list_emitted_files.is_true() {
        for file in &result.emitted_files {
            crate::write_all(writer.as_ref(), b"TSFILE: ");
            crate::write_all(
                writer.as_ref(),
                &tsr_tspath::absolute(file.as_bytes(), program.current_directory()),
            );
            crate::write_all(writer.as_ref(), b"\n");
        }
    }
    if options.explain_files.is_true() {
        program.explain_files(config.locale(), &mut |text| {
            crate::write_all(writer.as_ref(), text)
        })?;
    } else if options.list_files.is_true() || options.list_files_only.is_true() {
        for file in program.files() {
            let view = file.bound().view();
            crate::write_all(writer.as_ref(), view.source_file()?.file_name());
            crate::write_all(writer.as_ref(), b"\n");
        }
    }
    Ok(())
}
