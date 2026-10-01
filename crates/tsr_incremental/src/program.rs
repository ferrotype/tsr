//! The incremental program (`program.go`): a compiler program with its
//! incremental state, which checks and emits only what changed since the old
//! program and writes its build info.
use crate::emit_files_handler::emit_files;
use crate::host::Host;
use crate::snapshot::{lock, Path, Snapshot};
use std::collections::BTreeMap;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tsr_ast::Diagnostic;
use tsr_checker::{CheckerRequest, TraceArgs, TracePhase};
use tsr_compiler::{
    filter_no_emit_semantic_diagnostics, handle_no_emit_options, CheckedProgram, EmitOnly,
    EmitOptions, EmitResult, Error, ProgramFile, ProgramLike, WriteFileData,
};
use tsr_core::{CompilerOptions, Tristate};
use tsr_jsstring::JsString;

/// How a file's signature was last updated (testing).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SignatureUpdateKind {
    ComputedDts,
    StoredAtEmit,
    UsedVersion,
}

/// The clock nested emits (signature computations) are timed with.
pub type NestedEmitNow = Arc<dyn Fn() -> Instant + Send + Sync>;

#[derive(Default)]
struct NestedEmit {
    depth: usize,
    start: Option<Instant>,
    time: Duration,
}

/// `Program`: the incremental program.
#[allow(clippy::struct_field_names)] // the pin's `program` field
pub struct Program {
    pub(crate) snapshot: Arc<Snapshot>,
    pub(crate) program: Option<Arc<CheckedProgram>>,
    pub(crate) host: Option<Arc<dyn Host>>,

    // Testing data
    pub(crate) testing_data: Option<TestingData>,

    nested_emit: Mutex<NestedEmit>,
    nested_emit_now: Option<NestedEmitNow>,
}

/// What a test observes of the incremental state: the program's and the old
/// program's cached semantic diagnostics, and how each file's signature was
/// updated.
pub struct TestingData {
    pub semantic_diagnostics_per_file: Arc<Snapshot>,
    pub old_program_semantic_diagnostics_per_file: Arc<Snapshot>,
    pub updated_signature_kinds: Mutex<BTreeMap<Path, SignatureUpdateKind>>,
}

/// `if p.program.Tracing() != nil { defer tr.Push(tracing.PhaseEmit, name, nil, true)() }`.
struct TraceSpan {
    sink: Arc<dyn tsr_checker::TraceSink>,
    token: u64,
}

impl Drop for TraceSpan {
    fn drop(&mut self) {
        self.sink.pop(self.token, &TraceArgs::new());
    }
}

fn push_emit_trace(program: &CheckedProgram, name: &str) -> Option<TraceSpan> {
    let sink = program.tracing()?;
    let token = sink.push(TracePhase::Emit, name, &TraceArgs::new(), true);
    Some(TraceSpan {
        sink: sink.clone(),
        token,
    })
}

/// Whether the request was canceled (the pin's `ctx.Err() != nil`).
pub(crate) fn canceled(request: &CheckerRequest) -> bool {
    request
        .cancellation
        .as_ref()
        .is_some_and(tsr_core::CancellationToken::is_canceled)
}

/// The checker host of a program, which the checker's diagnostic details
/// read the program through.
pub(crate) fn checker_host(
    program: &Arc<tsr_compiler::Program>,
) -> tsr_compiler::ProgramCheckerHost {
    tsr_compiler::ProgramCheckerHost::new(program.clone())
}

pub(crate) fn source_path(file: &ProgramFile) -> Result<Path, Error> {
    Ok(file
        .bound()
        .view()
        .source_file()?
        .parse_options()
        .path
        .clone())
}

// port: tsc/internal/execute/incremental/program.go:NewProgram
pub fn new_program(
    program: Arc<CheckedProgram>,
    old_program: Option<&Program>,
    host: Arc<dyn Host>,
    nested_emit_now: Option<NestedEmitNow>,
    testing: bool,
) -> Result<Program, Error> {
    let mut incremental_program = Program {
        snapshot: crate::program_to_snapshot::program_to_snapshot(&program, old_program, testing)?,
        program: Some(program),
        host: Some(host),
        testing_data: None,
        nested_emit: Mutex::new(NestedEmit::default()),
        nested_emit_now,
    };

    if testing {
        incremental_program.testing_data = Some(TestingData {
            semantic_diagnostics_per_file: incremental_program.snapshot.clone(),
            old_program_semantic_diagnostics_per_file: match old_program {
                Some(old_program) => old_program.snapshot.clone(),
                None => Arc::new(Snapshot::default()),
            },
            updated_signature_kinds: Mutex::new(BTreeMap::new()),
        });
    }
    Ok(incremental_program)
}

impl Program {
    /// The old program a build info describes: its state, without a program.
    pub(crate) fn from_snapshot(snapshot: Snapshot) -> Self {
        Self {
            snapshot: Arc::new(snapshot),
            program: None,
            host: None,
            testing_data: None,
            nested_emit: Mutex::new(NestedEmit::default()),
            nested_emit_now: None,
        }
    }

    // port: tsc/internal/execute/incremental/program.go:Program.GetTestingData
    pub fn get_testing_data(&self) -> Option<&TestingData> {
        self.testing_data.as_ref()
    }

    /// Starts timing a nested emit; dropping the guard ends it.
    // port: tsc/internal/execute/incremental/program.go:Program.beginNestedEmit
    pub(crate) fn begin_nested_emit(&self) -> NestedEmitGuard<'_> {
        let mut nested = lock(&self.nested_emit);
        let Some(now) = &self.nested_emit_now else {
            return NestedEmitGuard { program: None };
        };
        if nested.depth == 0 {
            nested.start = Some(now());
        }
        nested.depth += 1;
        NestedEmitGuard {
            program: Some(self),
        }
    }

    // port: tsc/internal/execute/incremental/program.go:Program.TakeNestedEmitTime
    pub fn take_nested_emit_time(&self) -> Duration {
        let mut nested = lock(&self.nested_emit);
        let nested_emit_time = nested.time;
        nested.time = Duration::ZERO;
        nested_emit_time
    }

    // port: tsc/internal/execute/incremental/program.go:Program.panicIfNoProgram
    fn panic_if_no_program(&self, method: &str) -> &Arc<CheckedProgram> {
        match &self.program {
            Some(program) => program,
            None => panic!("{method}: should not be called without program"),
        }
    }

    // port: tsc/internal/execute/incremental/program.go:Program.GetProgram
    pub fn get_program(&self) -> &Arc<CheckedProgram> {
        self.panic_if_no_program("GetProgram")
    }

    // port: tsc/internal/execute/incremental/program.go:Program.HasChangedDtsFile
    pub fn has_changed_dts_file(&self) -> bool {
        self.snapshot.state().has_changed_dts_file
    }

    /// Options implements compiler.AnyProgram interface.
    // port: tsc/internal/execute/incremental/program.go:Program.Options
    pub fn options(&self) -> &CompilerOptions {
        &self.snapshot.options
    }

    /// CommonSourceDirectory implements compiler.AnyProgram interface.
    // port: tsc/internal/execute/incremental/program.go:Program.CommonSourceDirectory
    pub fn common_source_directory(&self) -> Result<Vec<u8>, Error> {
        self.panic_if_no_program("CommonSourceDirectory")
            .common_source_directory()
    }

    /// Program implements compiler.AnyProgram interface.
    // port: tsc/internal/execute/incremental/program.go:Program.Program
    pub fn program(&self) -> &CheckedProgram {
        self.panic_if_no_program("Program")
    }

    /// IsSourceFileDefaultLibrary implements compiler.AnyProgram interface.
    // port: tsc/internal/execute/incremental/program.go:Program.IsSourceFileDefaultLibrary
    pub fn is_source_file_default_library(&self, path: &[u8]) -> bool {
        self.panic_if_no_program("IsSourceFileDefaultLibrary")
            .program()
            .is_lib(path)
    }

    /// GetSourceFiles implements compiler.AnyProgram interface.
    // port: tsc/internal/execute/incremental/program.go:Program.GetSourceFiles
    pub fn get_source_files(&self) -> &[Arc<ProgramFile>] {
        self.panic_if_no_program("GetSourceFiles").program().files()
    }

    /// GetSourceFile implements compiler.AnyProgram interface.
    // port: tsc/internal/execute/incremental/program.go:Program.GetSourceFile
    pub fn get_source_file(&self, path: &[u8]) -> Option<&ProgramFile> {
        self.panic_if_no_program("GetSourceFile")
            .program()
            .source_file(path)
    }

    /// GetConfigFileParsingDiagnostics implements compiler.AnyProgram interface.
    // port: tsc/internal/execute/incremental/program.go:Program.GetConfigFileParsingDiagnostics
    pub fn get_config_file_parsing_diagnostics(&self) -> Vec<Diagnostic> {
        self.panic_if_no_program("GetConfigFileParsingDiagnostics")
            .program()
            .config_file_parsing_diagnostics()
    }

    /// GetSyntacticDiagnostics implements compiler.AnyProgram interface.
    // port: tsc/internal/execute/incremental/program.go:Program.GetSyntacticDiagnostics
    pub fn get_syntactic_diagnostics(
        &self,
        request: &CheckerRequest,
        file: Option<&ProgramFile>,
    ) -> Result<Vec<Diagnostic>, Error> {
        ProgramLike::syntactic_diagnostics(
            self.panic_if_no_program("GetSyntacticDiagnostics").as_ref(),
            request,
            file,
        )
    }

    /// GetBindDiagnostics implements compiler.AnyProgram interface.
    // port: tsc/internal/execute/incremental/program.go:Program.GetBindDiagnostics
    pub fn get_bind_diagnostics(
        &self,
        request: &CheckerRequest,
        file: Option<&ProgramFile>,
    ) -> Result<Vec<Diagnostic>, Error> {
        ProgramLike::bind_diagnostics(
            self.panic_if_no_program("GetBindDiagnostics").as_ref(),
            request,
            file,
        )
    }

    // port: tsc/internal/execute/incremental/program.go:Program.GetProgramDiagnostics
    pub fn get_program_diagnostics(&self) -> Result<Vec<Diagnostic>, Error> {
        ProgramLike::program_diagnostics(self.panic_if_no_program("GetProgramDiagnostics").as_ref())
    }

    // port: tsc/internal/execute/incremental/program.go:Program.GetGlobalDiagnostics
    pub fn get_global_diagnostics(
        &self,
        request: &CheckerRequest,
    ) -> Result<Vec<Diagnostic>, Error> {
        ProgramLike::global_diagnostics(
            self.panic_if_no_program("GetGlobalDiagnostics").as_ref(),
            request,
        )
    }

    /// GetSemanticDiagnostics implements compiler.AnyProgram interface.
    // port: tsc/internal/execute/incremental/program.go:Program.GetSemanticDiagnostics
    pub fn get_semantic_diagnostics(
        &self,
        request: &CheckerRequest,
        file: Option<&ProgramFile>,
    ) -> Result<Vec<Diagnostic>, Error> {
        let program = self.panic_if_no_program("GetSemanticDiagnostics");
        if self.snapshot.options.no_check.is_true() {
            return Ok(Vec::new());
        }

        // Ensure all the diagnsotics are cached
        self.collect_semantic_diagnostics_of_affected_files(request, file)?;
        if canceled(request) {
            return Ok(Vec::new());
        }

        // Return result from cache
        if let Some(file) = file {
            return self.get_semantic_diagnostics_of_file(file);
        }

        let mut diagnostics = Vec::new();
        for file in program.program().files() {
            diagnostics.extend(self.get_semantic_diagnostics_of_file(file)?);
        }
        Ok(diagnostics)
    }

    // port: tsc/internal/execute/incremental/program.go:Program.getSemanticDiagnosticsOfFile
    fn get_semantic_diagnostics_of_file(
        &self,
        file: &ProgramFile,
    ) -> Result<Vec<Diagnostic>, Error> {
        let program = self.panic_if_no_program("GetSemanticDiagnostics");
        let path = source_path(file)?;
        let Some(cached_diagnostics) = lock(&self.snapshot.semantic_diagnostics_per_file)
            .get(&path)
            .cloned()
        else {
            panic!("After handling all the affected files, there shouldnt be more changes");
        };
        let mut diagnostics = filter_no_emit_semantic_diagnostics(
            cached_diagnostics.get_diagnostics(program.program(), Some(file))?,
            &self.snapshot.options,
        );
        diagnostics.extend(program.program().include_processor_diagnostics(file)?);
        Ok(diagnostics)
    }

    /// GetDeclarationDiagnostics implements compiler.AnyProgram interface.
    // port: tsc/internal/execute/incremental/program.go:Program.GetDeclarationDiagnostics
    pub fn get_declaration_diagnostics(
        &self,
        request: &CheckerRequest,
        file: Option<&ProgramFile>,
    ) -> Result<Vec<Diagnostic>, Error> {
        let program = self.panic_if_no_program("GetDeclarationDiagnostics");
        let target = file.map(|file| file_arc(program, file)).transpose()?;
        let result = emit_files(
            request,
            self,
            &EmitOptions {
                target_source_files: target.as_ref().map(std::slice::from_ref),
                ..EmitOptions::default()
            },
            true,
        )?;
        Ok(result.map(|result| result.diagnostics).unwrap_or_default())
    }

    /// GetSuggestionDiagnostics implements compiler.AnyProgram interface.
    // port: tsc/internal/execute/incremental/program.go:Program.GetSuggestionDiagnostics
    pub fn get_suggestion_diagnostics(
        &self,
        request: &CheckerRequest,
        file: Option<&ProgramFile>,
    ) -> Result<Vec<Diagnostic>, Error> {
        let program = self.panic_if_no_program("GetSuggestionDiagnostics");
        // TODO: incremental suggestion diagnostics (only relevant in editor incremental builder?)
        ProgramLike::suggestion_diagnostics(program.as_ref(), request, file)
    }

    // port: tsc/internal/execute/incremental/program.go:Program.Emit
    pub fn emit(
        &self,
        request: &CheckerRequest,
        options: &EmitOptions<'_>,
    ) -> Result<Option<EmitResult>, Error> {
        self.panic_if_no_program("Emit");

        let mut result = None;
        if !options.force_emit && options.emit_only != EmitOnly::BuilderSignature {
            let emit_build_info = || self.emit_build_info(request, options);
            let emit_build_info: Option<&dyn Fn() -> Result<Option<EmitResult>, Error>> =
                if self.options().no_emit.is_true() {
                    Some(&emit_build_info)
                } else {
                    None
                };
            result = handle_no_emit_options(
                self,
                request,
                options.target_source_files,
                emit_build_info,
            )?;
            if canceled(request) {
                return Ok(None);
            }
        }
        if let Some(mut result) = result {
            if options.target_source_files.is_some() || self.options().no_emit.is_true() {
                return Ok(Some(result));
            }

            // Emit buildInfo and combine result
            if let Some(build_info_result) = self.emit_build_info(request, options)? {
                result.diagnostics.extend(build_info_result.diagnostics);
                result.emitted_files.extend(build_info_result.emitted_files);
            }
            return Ok(Some(result));
        }
        emit_files(request, self, options, false)
    }

    /// Handle affected files and cache the semantic diagnostics for all of them or the file asked for
    // port: tsc/internal/execute/incremental/program.go:Program.collectSemanticDiagnosticsOfAffectedFiles
    fn collect_semantic_diagnostics_of_affected_files(
        &self,
        request: &CheckerRequest,
        file: Option<&ProgramFile>,
    ) -> Result<(), Error> {
        let program = self.panic_if_no_program("GetSemanticDiagnostics");
        let files = program.program().files();
        if self.snapshot.can_use_incremental_state() {
            // Get all affected files
            crate::affected_files_handler::collect_all_affected_files(request, self)?;
            if canceled(request) {
                return Ok(());
            }

            if lock(&self.snapshot.semantic_diagnostics_per_file).len() == files.len() {
                // If we have all the files,
                return Ok(());
            }
        }

        let mut affected_files = Vec::new();
        if let Some(file) = file {
            if lock(&self.snapshot.semantic_diagnostics_per_file).contains_key(&source_path(file)?)
            {
                return Ok(());
            }
            affected_files.push(file_arc(program, file)?);
        } else {
            for file in files {
                if !lock(&self.snapshot.semantic_diagnostics_per_file)
                    .contains_key(&source_path(file)?)
                {
                    affected_files.push(file.clone());
                }
            }
        }

        // Get their diagnostics and cache them
        let diagnostics_per_file =
            program.semantic_diagnostics_without_no_emit_filtering(request, &affected_files)?;
        // commit changes if no err
        if canceled(request) {
            return Ok(());
        }

        // Commit changes to snapshot
        for (file, diagnostics) in affected_files.iter().zip(diagnostics_per_file) {
            lock(&self.snapshot.semantic_diagnostics_per_file).insert(
                source_path(file)?,
                Arc::new(
                    crate::snapshot::DiagnosticsOrBuildInfoDiagnosticsWithFileName::from_diagnostics(
                        program.program(),
                        diagnostics,
                    ),
                ),
            );
        }
        if lock(&self.snapshot.semantic_diagnostics_per_file).len() == files.len() {
            let mut state = self.snapshot.state();
            if state.check_pending && !self.snapshot.options.no_check.is_true() {
                state.check_pending = false;
            }
        }
        self.snapshot
            .build_info_emit_pending
            .store(true, Ordering::SeqCst);
        Ok(())
    }

    // port: tsc/internal/execute/incremental/program.go:Program.emitBuildInfo
    pub(crate) fn emit_build_info(
        &self,
        request: &CheckerRequest,
        options: &EmitOptions<'_>,
    ) -> Result<Option<EmitResult>, Error> {
        let program = self.panic_if_no_program("Emit");
        let _span = push_emit_trace(program, "emitBuildInfo");
        let loaded = program.program();
        let build_info_file_name =
            JsString::from_bytes(tsr_tsoptions::output_paths::build_info_file(
                &self.snapshot.options,
                loaded.current_directory(),
                loaded.use_case_sensitive_file_names(),
            ));
        if build_info_file_name.is_empty()
            || loaded.is_emit_blocked(build_info_file_name.as_bytes())
        {
            return Ok(None);
        }
        let has_errors = self.snapshot.state().has_errors;
        if has_errors == Tristate::UNKNOWN {
            self.ensure_has_errors_for_state(request, program)?;
            let state = self.snapshot.state();
            if state.has_errors != state.has_errors_from_old_state
                || state.has_semantic_errors != state.has_semantic_errors_from_old_state
            {
                self.snapshot
                    .build_info_emit_pending
                    .store(true, Ordering::SeqCst);
            }
        }
        if self.snapshot.state().package_jsons.is_none() {
            self.ensure_package_jsons_for_state();
            let state = self.snapshot.state();
            if state.package_jsons.as_deref().unwrap_or_default()
                != state
                    .package_jsons_from_old_state
                    .as_deref()
                    .unwrap_or_default()
                || state.missing_package_jsons.as_deref().unwrap_or_default()
                    != state
                        .missing_package_jsons_from_old_state
                        .as_deref()
                        .unwrap_or_default()
            {
                self.snapshot
                    .build_info_emit_pending
                    .store(true, Ordering::SeqCst);
            }
        }
        if !self.snapshot.build_info_emit_pending.load(Ordering::SeqCst) {
            return Ok(None);
        }
        if canceled(request) {
            return Ok(None);
        }
        let build_info = match crate::snapshot_to_build_info::snapshot_to_build_info(
            &self.snapshot,
            program,
            &build_info_file_name,
        )? {
            Ok(build_info) => build_info,
            Err(error) => {
                return Ok(Some(EmitResult {
                    emit_skipped: true,
                    diagnostics: vec![tsr_compiler::content_mapper_project_diagnostic(&error)],
                    ..EmitResult::default()
                }));
            }
        };
        let text = match tsr_json::marshal(&build_info, tsr_json::Options::default()) {
            Ok(text) => text,
            Err(error) => panic!("Failed to marshal build info: {error}"),
        };
        let written = if let Some(write_file) = options.write_file {
            write_file(
                build_info_file_name.as_bytes(),
                &text,
                &mut WriteFileData {
                    build_info: Some(Arc::new(build_info)),
                    ..WriteFileData::default()
                },
            )
        } else {
            loaded
                .host()
                .write_file(build_info_file_name.as_bytes(), &text)
        };
        if let Err(error) = written {
            return Ok(Some(EmitResult {
                emit_skipped: true,
                diagnostics: vec![Diagnostic::compiler(
                    tsr_diagnostics::Could_not_write_file_0_Colon_1,
                    vec![
                        build_info_file_name.clone(),
                        JsString::from_bytes(error.to_string().into_bytes()),
                    ],
                )],
                ..EmitResult::default()
            }));
        }
        self.snapshot
            .build_info_emit_pending
            .store(false, Ordering::SeqCst);
        Ok(Some(EmitResult {
            emit_skipped: false,
            emitted_files: vec![build_info_file_name],
            ..EmitResult::default()
        }))
    }

    // port: tsc/internal/execute/incremental/program.go:Program.ensureHasErrorsForState
    fn ensure_has_errors_for_state(
        &self,
        request: &CheckerRequest,
        program: &CheckedProgram,
    ) -> Result<(), Error> {
        let loaded = program.program();
        let has_include_processing_diagnostics: Box<dyn Fn() -> Result<bool, Error> + '_>;
        let mut has_emit_diagnostics = false;
        if self.snapshot.can_use_incremental_state() {
            let mut include_processing = None;
            for file in loaded.files() {
                if lock(&self.snapshot.emit_diagnostics_per_file).contains_key(&source_path(file)?)
                {
                    // emit diagnostics will be encoded in buildInfo;
                    has_emit_diagnostics = true;
                    break;
                }
                if include_processing.is_none()
                    && !loaded.include_processor_diagnostics(file)?.is_empty()
                {
                    include_processing = Some(true);
                }
            }
            let include_processing = include_processing.unwrap_or(false);
            has_include_processing_diagnostics = Box::new(move || Ok(include_processing));
        } else {
            has_emit_diagnostics = self.snapshot.state().has_emit_diagnostics;
            has_include_processing_diagnostics = Box::new(move || {
                for file in loaded.files() {
                    if !loaded.include_processor_diagnostics(file)?.is_empty() {
                        return Ok(true);
                    }
                }
                Ok(false)
            });
        }

        if has_emit_diagnostics {
            let mut state = self.snapshot.state();
            // Record this for only non incremental build info
            state.has_errors = if self.snapshot.options.is_incremental() {
                Tristate::FALSE
            } else {
                Tristate::TRUE
            };
            // Dont need to encode semantic errors state since the emit diagnostics are encoded
            state.has_semantic_errors = false;
            return Ok(());
        }

        if has_include_processing_diagnostics()?
            || !ProgramLike::config_file_parsing_diagnostics(program).is_empty()
            || !ProgramLike::syntactic_diagnostics(program, request, None)?.is_empty()
            || !ProgramLike::program_diagnostics(program)?.is_empty()
            || !ProgramLike::global_diagnostics(program, request)?.is_empty()
        {
            let mut state = self.snapshot.state();
            state.has_errors = Tristate::TRUE;
            // Dont need to encode semantic errors state since the syntax and program diagnostics are encoded as present
            state.has_semantic_errors = false;
            return Ok(());
        }

        self.snapshot.state().has_errors = Tristate::FALSE;
        // Check semantic and emit diagnostics first as we dont need to ask program about it
        let mut found = false;
        for file in loaded.files() {
            let semantic_diagnostics = lock(&self.snapshot.semantic_diagnostics_per_file)
                .get(&source_path(file)?)
                .cloned();
            let Some(semantic_diagnostics) = semantic_diagnostics else {
                // Missing semantic diagnostics in cache will be encoded in incremental buildInfo
                if self.snapshot.options.is_incremental() {
                    found = true;
                    break;
                }
                continue;
            };
            if semantic_diagnostics.has_diagnostics() {
                // cached semantic diagnostics will be encoded in buildInfo
                found = true;
                break;
            }
        }
        if found {
            // Because semantic diagnostics are recorded in buildInfo, we dont need to encode hasErrors in incremental buildInfo
            // But encode as errors in non incremental buildInfo
            self.snapshot.state().has_semantic_errors = !self.snapshot.options.is_incremental();
        }
        Ok(())
    }

    // port: tsc/internal/execute/incremental/program.go:Program.ensurePackageJsonsForState
    fn ensure_package_jsons_for_state(&self) {
        let program = self.panic_if_no_program("Emit");
        let loaded = program.program();
        let config = tsr_tspath::directory(loaded.config().config_name().as_bytes());
        let mut package_jsons: Option<Vec<JsString>> = None;
        let mut missing_package_jsons: Option<Vec<JsString>> = None;
        if !config.is_empty() {
            loaded.package_json_cache_entries(|_key, value| {
                let mut package_json = JsString::from_bytes(tsr_tspath::combine(
                    value.package_directory.as_bytes(),
                    &[b"package.json"],
                ));
                if value.exists() || value.directory_exists {
                    package_json = loaded
                        .host()
                        .realpath(package_json.as_bytes())
                        .unwrap_or(package_json);
                }
                if value.exists() {
                    package_jsons
                        .get_or_insert_with(Vec::new)
                        .push(package_json);
                } else if contains(package_json.as_bytes(), b"/node_modules/") {
                    missing_package_jsons
                        .get_or_insert_with(Vec::new)
                        .push(package_json);
                }
                true
            });
        }
        let mut state = self.snapshot.state();
        state.package_jsons = Some(normalize_package_jsons(package_jsons));
        state.missing_package_jsons = Some(normalize_package_jsons(missing_package_jsons));
    }

    // port: tsc/internal/execute/incremental/program.go:Program.PackageJsonLookupPaths
    pub fn package_json_lookup_paths(&self) -> Option<Vec<JsString>> {
        let program = self.panic_if_no_program("PackageJsonLookupPaths");
        let loaded = program.program();
        let config = tsr_tspath::directory(loaded.config().config_name().as_bytes());
        if config.is_empty() {
            return None;
        }

        let mut package_jsons = Vec::new();
        loaded.package_json_cache_entries(|_key, value| {
            let mut package_json = JsString::from_bytes(tsr_tspath::combine(
                value.package_directory.as_bytes(),
                &[b"package.json"],
            ));
            if value.exists() || value.directory_exists {
                package_json = loaded
                    .host()
                    .realpath(package_json.as_bytes())
                    .unwrap_or(package_json);
            }
            package_jsons.push(package_json);
            true
        });
        package_jsons.sort();
        package_jsons.dedup();
        Some(package_jsons)
    }
}

/// The guard of a nested emit: dropping it ends the emit's timing.
pub(crate) struct NestedEmitGuard<'a> {
    program: Option<&'a Program>,
}

impl Drop for NestedEmitGuard<'_> {
    fn drop(&mut self) {
        let Some(program) = self.program else {
            return;
        };
        let now = program
            .nested_emit_now
            .as_ref()
            .expect("a timed nested emit has a clock");
        let mut nested = lock(&program.nested_emit);
        nested.depth -= 1;
        if nested.depth == 0 {
            let start = nested
                .start
                .expect("the outermost nested emit started the clock");
            nested.time += now().saturating_duration_since(start);
        }
    }
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

/// Sorted and deduplicated, empty for none.
// port: tsc/internal/execute/incremental/program.go:normalizePackageJsons
fn normalize_package_jsons(package_jsons: Option<Vec<JsString>>) -> Vec<JsString> {
    let Some(mut package_jsons) = package_jsons else {
        return Vec::new();
    };
    package_jsons.sort();
    package_jsons.dedup();
    package_jsons
}

/// The program's own handle of one of its files.
pub(crate) fn file_arc(
    program: &CheckedProgram,
    file: &ProgramFile,
) -> Result<Arc<ProgramFile>, Error> {
    program
        .program()
        .file_of_node(file.source())
        .cloned()
        .ok_or_else(|| tsr_arena::Error::WrongOwner.into())
}

impl ProgramLike for Program {
    fn options(&self) -> &CompilerOptions {
        Program::options(self)
    }
    fn source_file(&self, file_name: &[u8]) -> Option<&ProgramFile> {
        self.get_source_file(file_name)
    }
    fn source_files(&self) -> &[Arc<ProgramFile>] {
        self.get_source_files()
    }
    fn config_file_parsing_diagnostics(&self) -> Vec<Diagnostic> {
        self.get_config_file_parsing_diagnostics()
    }
    fn syntactic_diagnostics(
        &self,
        request: &CheckerRequest,
        file: Option<&ProgramFile>,
    ) -> Result<Vec<Diagnostic>, Error> {
        self.get_syntactic_diagnostics(request, file)
    }
    fn bind_diagnostics(
        &self,
        request: &CheckerRequest,
        file: Option<&ProgramFile>,
    ) -> Result<Vec<Diagnostic>, Error> {
        self.get_bind_diagnostics(request, file)
    }
    fn program_diagnostics(&self) -> Result<Vec<Diagnostic>, Error> {
        self.get_program_diagnostics()
    }
    fn global_diagnostics(&self, request: &CheckerRequest) -> Result<Vec<Diagnostic>, Error> {
        self.get_global_diagnostics(request)
    }
    fn semantic_diagnostics(
        &self,
        request: &CheckerRequest,
        file: Option<&ProgramFile>,
    ) -> Result<Vec<Diagnostic>, Error> {
        self.get_semantic_diagnostics(request, file)
    }
    fn declaration_diagnostics(
        &self,
        request: &CheckerRequest,
        file: Option<&ProgramFile>,
    ) -> Result<Vec<Diagnostic>, Error> {
        self.get_declaration_diagnostics(request, file)
    }
    fn suggestion_diagnostics(
        &self,
        request: &CheckerRequest,
        file: Option<&ProgramFile>,
    ) -> Result<Vec<Diagnostic>, Error> {
        self.get_suggestion_diagnostics(request, file)
    }
    fn emit(
        &self,
        request: &CheckerRequest,
        options: &EmitOptions<'_>,
    ) -> Result<Option<EmitResult>, Error> {
        Program::emit(self, request, options)
    }
    fn common_source_directory(&self) -> Result<Vec<u8>, Error> {
        Program::common_source_directory(self)
    }
    fn is_source_file_default_library(&self, path: &[u8]) -> bool {
        Program::is_source_file_default_library(self, path)
    }
    fn checked_program(&self) -> &CheckedProgram {
        self.program()
    }
}
