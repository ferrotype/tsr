//! The `Program.Emit` family of `compiler/program.go`: the emit options and
//! result, `noEmit` and `noEmitOnError` (`HandleNoEmitOptions` with
//! `GetDiagnosticsOfAnyProgram`), the emit-blocked outputs, and the per-file
//! emitters in the program's work group with their results combined in input
//! order. Build-info emit is Phase 4's: no caller passes one here.
use crate::emit_host::new_emit_host;
use crate::emitter::{self, Emitter, ScriptTransformers};
use crate::{CheckedProgram, Error, Program, ProgramCheckerHost, ProgramFile};
use std::sync::{Arc, Mutex, PoisonError};
use tsr_ast::{Diagnostic, NodeId};
use tsr_checker::{CheckerRequest, TraceArgs, TracePhase, TraceSink, TraceValue};
use tsr_core::workgroup::WorkGroup;
use tsr_jsstring::JsString;
use tsr_printer::{EmitTextWriter, TextWriter};
use tsr_sourcemap::RawSourceMap;
use tsr_tsoptions::output_paths::{get_output_paths_for, ForceEmitPaths};

/// `EmitOnly`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum EmitOnly {
    #[default]
    All,
    Js,
    Dts,
    BuilderSignature,
}

/// `WriteFileData`. `BuildInfo` is the build-info writer's (Phase 4).
#[derive(Clone, Debug, Default)]
pub struct WriteFileData {
    pub source_map_url_pos: isize,
    pub diagnostics: Vec<Diagnostic>,
    pub skipped_dts_write: bool,
    /// The emitted source file.
    pub source_file: Option<NodeId>,
}

/// `WriteFile`: called from the emit's work group, possibly concurrently.
pub type WriteFile<'a> =
    dyn Fn(&[u8], &[u8], &mut WriteFileData) -> Result<(), tsr_vfs::Error> + Sync + 'a;

/// `EmitOptions`.
#[derive(Clone, Copy, Default)]
pub struct EmitOptions<'a> {
    /// Source files to emit. If `None`, emits all files
    pub target_source_files: Option<&'a [Arc<ProgramFile>]>,
    pub emit_only: EmitOnly,
    pub force_emit: bool,
    pub write_file: Option<&'a WriteFile<'a>>,
}

/// `EmitResult`.
#[derive(Clone, Debug, Default)]
pub struct EmitResult {
    pub emit_skipped: bool,
    /// Contains declaration emit diagnostics
    pub diagnostics: Vec<Diagnostic>,
    /// Array of files the compiler wrote to disk
    pub emitted_files: Vec<JsString>,
    /// Array of sourceMapData if compiler emitted sourcemaps
    pub source_maps: Vec<SourceMapEmitResult>,
}

/// `SourceMapEmitResult`.
#[derive(Clone, Debug)]
pub struct SourceMapEmitResult {
    /// Input source file (which one can use on program to get the file), 1:1 mapping with the sourceMap.sources list
    pub input_source_file_names: Vec<JsString>,
    pub source_map: RawSourceMap,
    pub generated_file: JsString,
}

impl Program {
    // port: tsc/internal/compiler/program.go:Program.IsEmitBlocked
    pub fn is_emit_blocked(&self, emit_file_name: &[u8]) -> bool {
        self.option_verification()
            .blocked_output_paths
            .contains(&self.to_path(emit_file_name))
    }
}

/// An open emit trace event: dropping it ends the event, as the pin's
/// deferred `tr.Push(...)()` does on every return path.
#[must_use = "an unused span ends its event at once"]
pub(crate) struct EmitTraceSpan {
    sink: Arc<dyn TraceSink>,
    token: u64,
    args: TraceArgs,
}

impl Drop for EmitTraceSpan {
    fn drop(&mut self) {
        self.sink.pop(self.token, &self.args);
    }
}

/// `if tr != nil { defer tr.Push(tracing.PhaseEmit, name, args, separateBeginAndEnd)() }`,
/// with `args` given as one optional string argument (`nil` for `None`).
pub(crate) fn push_emit_trace(
    tracing: Option<&Arc<dyn TraceSink>>,
    name: &str,
    arg: Option<(&str, &[u8])>,
    separate_begin_and_end: bool,
) -> Option<EmitTraceSpan> {
    let sink = tracing?;
    let mut args = TraceArgs::new();
    if let Some((key, value)) = arg {
        args.insert(
            key.to_owned(),
            TraceValue::Str(String::from_utf8_lossy(value).into_owned()),
        );
    }
    let token = sink.push(TracePhase::Emit, name, &args, separate_begin_and_end);
    Some(EmitTraceSpan {
        sink: Arc::clone(sink),
        token,
        args,
    })
}

/// A per-file collection of [`get_diagnostics_of_any_program`]: one file, or
/// every file for `None`.
pub type FileDiagnostics<'a> = dyn Fn(Option<&ProgramFile>) -> Result<Vec<Diagnostic>, Error> + 'a;

impl CheckedProgram {
    /// `None` is the pin's nil result: the request was canceled before the
    /// emit began.
    /// Panics with the pinned refusal if `noEmitOnError` diagnostics try to
    /// reuse a checker that already observed cancellation.
    // port: tsc/internal/compiler/program.go:Program.Emit
    pub fn emit(
        &self,
        request: &CheckerRequest,
        options: &EmitOptions<'_>,
    ) -> Result<Option<EmitResult>, Error> {
        self.emit_with_script_transformers(request, options, &emitter::get_script_transformers)
    }

    /// [`CheckedProgram::emit`] with `script_transformers` in place of the
    /// emitter's `getScriptTransformers`, for tests that witness the emitter
    /// over a chain without the transformers the port does not have yet.
    #[doc(hidden)]
    pub fn emit_with_script_transformers(
        &self,
        request: &CheckerRequest,
        options: &EmitOptions<'_>,
        script_transformers: &ScriptTransformers,
    ) -> Result<Option<EmitResult>, Error> {
        let program = self.program();
        let _span = push_emit_trace(self.tracing(), "emit", None, true);

        if !options.force_emit && options.emit_only != EmitOnly::BuilderSignature {
            let result = handle_no_emit_options(self, request, options.target_source_files, None)?;
            if result.is_some()
                || request
                    .cancellation
                    .as_ref()
                    .is_some_and(tsr_core::CancellationToken::is_canceled)
            {
                return Ok(result);
            }
        }

        let new_line = program.options().new_line.as_str().as_bytes();
        let writer_pool: Mutex<Vec<TextWriter>> = Mutex::new(Vec::new());
        let force_dts_emit = options.emit_only == EmitOnly::BuilderSignature
            || options.force_emit && options.emit_only == EmitOnly::Dts;
        let force_js_emit = options.force_emit && options.emit_only == EmitOnly::Js;
        let source_files = crate::verify_options::source_files_to_emit(
            program,
            options.target_source_files,
            force_dts_emit,
            force_js_emit,
        )?;
        let checker_host = ProgramCheckerHost::new(program.clone());
        let dependencies = if source_files.is_empty() {
            tsr_ast::AstDependencies::default()
        } else {
            tsr_ast::AstDependencies::new(program.files().iter().map(|file| file.bound()))
        };

        let emitters: Vec<Mutex<Option<Result<EmitResult, Error>>>> =
            source_files.iter().map(|_| Mutex::new(None)).collect();
        let wg = WorkGroup::new(program.single_threaded());
        for (&source_file, emitter) in source_files.iter().zip(&emitters) {
            let (checker_host, writer_pool, dependencies) =
                (&checker_host, &writer_pool, &dependencies);
            wg.queue(move || {
                let result = new_emit_host(
                    self,
                    request,
                    checker_host,
                    dependencies,
                    source_file,
                    &mut |host| {
                        // take an unused writer
                        let mut writer = writer_pool
                            .lock()
                            .unwrap_or_else(PoisonError::into_inner)
                            .pop()
                            .unwrap_or_else(|| TextWriter::new(new_line, 0));
                        writer.clear();

                        // attach writer and perform emit
                        let result = file_emit(
                            host,
                            &mut writer,
                            source_file,
                            options,
                            ForceEmitPaths {
                                dts: force_dts_emit,
                                js: force_js_emit,
                                declaration_map: options.force_emit
                                    && options.emit_only == EmitOnly::Dts,
                            },
                            script_transformers,
                            self.tracing(),
                        );

                        // put the writer back in the pool
                        writer_pool
                            .lock()
                            .unwrap_or_else(PoisonError::into_inner)
                            .push(writer);
                        result
                    },
                );
                *emitter.lock().unwrap_or_else(PoisonError::into_inner) =
                    Some(result.and_then(|result| result));
            });
        }

        // wait for emit to complete
        wg.run_and_wait();
        drop(wg);

        // collect results from emit, preserving input order
        let mut results = Vec::with_capacity(emitters.len());
        for emitter in emitters {
            results.push(Some(
                emitter
                    .into_inner()
                    .unwrap_or_else(PoisonError::into_inner)
                    .expect("every queued emit ran")?,
            ));
        }
        Ok(Some(combine_emit_results(results)))
    }
}

/// The body of one queued emit: the file's output paths and its emitter.
fn file_emit(
    host: &crate::emit_host::EmitHost<'_>,
    writer: &mut TextWriter,
    source_file: &ProgramFile,
    options: &EmitOptions<'_>,
    force: ForceEmitPaths,
    script_transformers: &ScriptTransformers,
    tr: Option<&Arc<dyn TraceSink>>,
) -> Result<EmitResult, Error> {
    let source = source_file.bound().view().source_file()?;
    let paths = get_output_paths_for(
        &source,
        host.options(),
        &mut host.output_paths_host(),
        force,
    );
    let mut emitter = Emitter {
        host,
        emit_only: options.emit_only,
        emitter_diagnostics: emitter::EmitterDiagnostics::default(),
        writer,
        paths,
        source_file,
        emit_result: EmitResult::default(),
        force_emit: options.force_emit,
        write_file: options.write_file,
        tr,
        script_transformers,
    };
    emitter.emit()?;
    Ok(emitter.emit_result)
}

// port: tsc/internal/compiler/program.go:CombineEmitResults
pub fn combine_emit_results(results: Vec<Option<EmitResult>>) -> EmitResult {
    let mut result = EmitResult::default();
    for emit_result in results {
        let Some(emit_result) = emit_result else {
            continue; // Skip nil results
        };
        if emit_result.emit_skipped {
            result.emit_skipped = true;
        }
        result.diagnostics.extend(emit_result.diagnostics);
        result.emitted_files.extend(emit_result.emitted_files);
        result.source_maps.extend(emit_result.source_maps);
    }
    result
}

/// HandleNoEmitOptions mirrors tsc's handleNoEmitOptions. `None` is the
/// pin's nil result: the emit proceeds.
// port: tsc/internal/compiler/program.go:HandleNoEmitOptions
pub fn handle_no_emit_options(
    program: &CheckedProgram,
    request: &CheckerRequest,
    files: Option<&[Arc<ProgramFile>]>,
    emit_build_info: Option<&dyn Fn() -> Option<EmitResult>>,
) -> Result<Option<EmitResult>, Error> {
    let options = program.program().options();
    if !options.no_emit.is_true() {
        if !options.no_emit_on_error.is_true() {
            return Ok(None); // NoEmit is false and NoEmitOnError is also false, so we can proceed with normal emit
        }

        let diagnostics = get_diagnostics_of_any_program(
            program,
            request,
            files,
            true,
            &|file| {
                program
                    .program()
                    .bind_diagnostics(file.map(ProgramFile::source))
            },
            &|file| program.semantic_diagnostics(request, file),
        );
        // The checker API reports terminal cancellation as a typed error so
        // project pools can discard just that checker. Program.Emit's native
        // contract instead panics when its diagnostic collection reuses one.
        // Translate only that refusal, after all checker leases have returned;
        // cancellation must not retire the whole generation as an internal
        // panic during a checker operation would.
        let diagnostics = match diagnostics {
            Err(Error::Checker(tsr_checker::Error::PreviouslyCanceled)) => {
                panic!("Checker was previously cancelled")
            }
            result => result?,
        };
        if diagnostics.is_empty() {
            return Ok(None); // NoEmitOnError is enabled, but no diagnostics were found, so we can proceed with emitting
        }
        return Ok(Some(EmitResult {
            diagnostics,
            emit_skipped: true,
            ..EmitResult::default()
        }));
    }
    if files.is_some() {
        return Ok(Some(EmitResult {
            emit_skipped: true,
            ..EmitResult::default()
        }));
    }
    if let Some(emit_build_info) = emit_build_info {
        let result = emit_build_info();
        if result.is_some() {
            return Ok(result);
        }
    }
    Ok(Some(EmitResult::default()))
}

// port: tsc/internal/compiler/program.go:GetDiagnosticsOfAnyProgram
pub fn get_diagnostics_of_any_program(
    program: &CheckedProgram,
    request: &CheckerRequest,
    files: Option<&[Arc<ProgramFile>]>,
    skip_no_emit_check_for_dts_diagnostics: bool,
    get_bind_diagnostics: &FileDiagnostics<'_>,
    get_semantic_diagnostics: &FileDiagnostics<'_>,
) -> Result<Vec<Diagnostic>, Error> {
    let loaded = program.program();
    let mut all_diagnostics = loaded.config_file_parsing_diagnostics();
    let config_file_parsing_diagnostics_length = all_diagnostics.len();

    let append_diagnostics_for_all_files =
        |diagnostics: &mut Vec<Diagnostic>, get_diagnostics: &FileDiagnostics<'_>| {
            match files {
                None => diagnostics.extend(get_diagnostics(None)?),
                Some(files) => {
                    for file in files {
                        diagnostics.extend(get_diagnostics(Some(file))?);
                    }
                }
            }
            Ok::<(), Error>(())
        };

    let mut syntactic_diagnostics = Vec::new();
    append_diagnostics_for_all_files(&mut syntactic_diagnostics, &|file| {
        loaded.syntactic_diagnostics(file)
    })?;
    if !syntactic_diagnostics.is_empty() {
        // Per-file content mapper failures are syntactic diagnostics, but the locationless diagnostic
        // that disables a repeatedly failing mapper must still be reported.
        all_diagnostics.extend_from_slice(&loaded.content_mapper_diagnostics);
    }
    all_diagnostics.extend(syntactic_diagnostics);

    // If we didn't have any syntactic errors, then also try getting the program (options),
    // global and semantic errors.
    if all_diagnostics.len() == config_file_parsing_diagnostics_length {
        all_diagnostics.extend_from_slice(loaded.program_diagnostics()?);

        // Do binding early so we can track the time.
        append_diagnostics_for_all_files(&mut Vec::new(), get_bind_diagnostics)?;

        if !loaded.options().list_files_only.is_true() {
            all_diagnostics.extend(program.global_diagnostics()?);

            if all_diagnostics.len() == config_file_parsing_diagnostics_length {
                append_diagnostics_for_all_files(&mut all_diagnostics, get_semantic_diagnostics)?;
                // Ask for the global diagnostics again (they were empty above); we may have found new during checking, e.g. missing globals.
                all_diagnostics.extend(program.global_diagnostics()?);
            }

            if (skip_no_emit_check_for_dts_diagnostics || loaded.options().no_emit.is_true())
                && loaded.options().emit_declarations()
                && all_diagnostics.len() == config_file_parsing_diagnostics_length
            {
                append_diagnostics_for_all_files(&mut all_diagnostics, &|file| {
                    program.declaration_diagnostics(request, file)
                })?;
            }
        }
    }
    Ok(all_diagnostics)
}
