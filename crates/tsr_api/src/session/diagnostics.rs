//! The nine diagnostic methods over one collector: syntactic, bind,
//! semantic, suggestion and declaration diagnostics per file or for the
//! whole program; the program's own, global and config-file parsing
//! diagnostics. Checker-backed collections acquire the project's
//! diagnostics checker through its scheduler, as the LSP server does.
//! port: tsc/internal/api/session.go
use super::responses::{config_resolver, diagnostic_responses};
use super::{client_error, ApiSession, SessionError, SessionResult};
use crate::proto::{DiagnosticResponse, GetDiagnosticsParams, GetProjectDiagnosticsParams};
use std::sync::Arc;
use tsr_ast::Diagnostic;
use tsr_checker::{CheckerLifetime, Operation};
use tsr_compiler::{Program, ProgramFile};
use tsr_ipc::Context;
use tsr_project::Project;

#[derive(Clone, Copy)]
pub(super) enum DiagnosticKind {
    Syntactic,
    Bind,
    Semantic,
    Suggestion,
    Declaration,
}

use super::checker_error;

/// Whether the payload names `files` with a non-null value: the pin's
/// `Files != nil`, which `[]` satisfies and an omitted or null field does not.
pub(super) fn names_files(payload: &[u8]) -> bool {
    struct Presence(bool);
    impl tsr_json::Decode for Presence {
        fn decode(&mut self, input: &mut tsr_json::Decoder<'_>) -> Result<(), tsr_json::Error> {
            input.object(|name, input| {
                if name == b"files" && input.peek_kind() != tsr_json::Kind::Null {
                    self.0 = true;
                }
                input.skip_value()
            })
        }
    }
    let mut presence = Presence(false);
    tsr_json::unmarshal(payload, &mut presence, tsr_json::Options::default()).is_ok() && presence.0
}

/// Responses for diagnostics of a program: each diagnostic's positions are
/// read through the view of the file it names.
/// port: tsc/internal/api/proto.go:NewDiagnosticResponses
pub(super) fn program_diagnostic_responses(
    program: &Program,
    diagnostics: &[Diagnostic],
) -> Vec<Option<Box<DiagnosticResponse>>> {
    // A program diagnostic names one of the program's files, or a config
    // source file the program holds outside its file list; related
    // information may name another file than the diagnostic.
    let config = config_resolver(program.config());
    let resolve = |node: tsr_ast::NodeId| {
        program
            .file_of_node(node)
            .map(|file| file.bound().view().ast())
            .or_else(|| config(node))
    };
    diagnostic_responses(diagnostics, &resolve)
}

/// Runs `collect` with the project's diagnostics checker for `file` (or any
/// file of the project when `None`).
fn with_diagnostics_checker<R>(
    project: &Project,
    file: Option<&ProgramFile>,
    ctx: &Context,
    collect: impl FnOnce(&mut Operation<'_>) -> SessionResult<R>,
) -> SessionResult<R> {
    let scheduler = project
        .scheduler()
        .ok_or_else(|| SessionError::Other("project has no checker scheduler".into()))?;
    let checker = scheduler
        .acquire(
            CheckerLifetime::Diagnostics,
            file.map(ProgramFile::source),
            ctx,
            "api",
        )
        .map_err(|error| match error {
            tsr_project::scheduler::AcquireError::Checker(error) => checker_error(error),
            tsr_project::scheduler::AcquireError::Canceled(error) => checker_error(error),
        })?;
    let mut operation = checker.operation().map_err(checker_error)?;
    collect(&mut operation)
}

/// One file's diagnostics of `kind`, or the whole program's, filtered and
/// sorted as the pin's program getters return them: the syntactic and bind
/// collectors sort their own result, the checker-backed ones are collected
/// per file through the file's diagnostics checker and sorted here.
/// port: tsc/internal/compiler/program.go:Program.collectDiagnostics
/// port: tsc/internal/compiler/program.go:Program.collectCheckerDiagnostics
fn diagnostics_of(
    project: &Project,
    program: &Arc<Program>,
    ctx: &Context,
    kind: DiagnosticKind,
    file: Option<&ProgramFile>,
) -> SessionResult<Vec<Diagnostic>> {
    match kind {
        DiagnosticKind::Syntactic => program.syntactic_diagnostics(file).map_err(checker_error),
        DiagnosticKind::Bind => program
            .bind_diagnostics(file.map(ProgramFile::source))
            .map_err(checker_error),
        DiagnosticKind::Semantic | DiagnosticKind::Suggestion | DiagnosticKind::Declaration => {
            let raw = if let Some(file) = file {
                checker_diagnostics_of(project, program, ctx, kind, file)?
            } else {
                let mut raw = Vec::new();
                for file in program.files() {
                    raw.extend(checker_diagnostics_of(project, program, ctx, kind, file)?);
                }
                raw
            };
            program
                .filter_and_sort_diagnostics(&raw)
                .map_err(checker_error)
        }
    }
}

/// One file's raw checker-backed diagnostics through its diagnostics checker.
fn checker_diagnostics_of(
    project: &Project,
    program: &Arc<Program>,
    ctx: &Context,
    kind: DiagnosticKind,
    file: &ProgramFile,
) -> SessionResult<Vec<Diagnostic>> {
    with_diagnostics_checker(project, Some(file), ctx, |operation| match kind {
        DiagnosticKind::Semantic => program
            .semantic_diagnostics_in(operation, file, None)
            .map_err(checker_error),
        DiagnosticKind::Suggestion => program
            .suggestion_diagnostics_in(operation, file, None)
            .map_err(checker_error),
        DiagnosticKind::Declaration => program
            .declaration_diagnostics_with_checker(operation, file)
            .map_err(checker_error),
        DiagnosticKind::Syntactic | DiagnosticKind::Bind => {
            unreachable!("not a checker-backed kind")
        }
    })
}

impl ApiSession {
    /// The files a request names (`[]` names none); the omitted list is the
    /// pin's nil list, which the caller answers for the whole program.
    fn requested_files<'a>(
        program: &'a Arc<Program>,
        files: &[crate::proto::DocumentIdentifier],
        files_named: bool,
    ) -> SessionResult<Vec<&'a ProgramFile>> {
        if !files_named {
            return Ok(program.files().iter().map(Arc::as_ref).collect());
        }
        files
            .iter()
            .map(|file| {
                program
                    .source_file(file.to_file_name().as_bytes())
                    .ok_or_else(|| {
                        client_error(format!("source file not found: {}", file.display()))
                    })
            })
            .collect()
    }

    /// The pin's getter runs once per named file, each result filtered and
    /// sorted on its own and the lists concatenated, or once for the whole
    /// program, sorted as one list.
    /// port: tsc/internal/api/session.go:Session.getDiagnostics
    pub(super) fn handle_get_diagnostics(
        &self,
        ctx: &Context,
        params: &GetDiagnosticsParams,
        kind: DiagnosticKind,
        files_named: bool,
    ) -> SessionResult<Vec<Option<Box<DiagnosticResponse>>>> {
        let data = self.snapshot_data(params.snapshot)?;
        let project = data.project(&params.project)?;
        let program = data.program(&params.project)?;
        let diagnostics = if files_named {
            let mut diagnostics = Vec::new();
            for file in Self::requested_files(program, &params.files, true)? {
                diagnostics.extend(diagnostics_of(project, program, ctx, kind, Some(file))?);
            }
            diagnostics
        } else {
            diagnostics_of(project, program, ctx, kind, None)?
        };
        Ok(program_diagnostic_responses(program, &diagnostics))
    }

    /// port: tsc/internal/api/session.go:Session.handleGetConfigFileParsingDiagnostics
    pub(super) fn handle_get_config_file_parsing_diagnostics(
        &self,
        params: &GetProjectDiagnosticsParams,
    ) -> SessionResult<Vec<Option<Box<DiagnosticResponse>>>> {
        let data = self.snapshot_data(params.snapshot)?;
        let program = data.program(&params.project)?;
        let diagnostics = program.config_file_parsing_diagnostics();
        Ok(program_diagnostic_responses(program, &diagnostics))
    }

    /// port: tsc/internal/api/session.go:Session.handleGetProgramDiagnostics
    pub(super) fn handle_get_program_diagnostics(
        &self,
        params: &GetProjectDiagnosticsParams,
    ) -> SessionResult<Vec<Option<Box<DiagnosticResponse>>>> {
        let data = self.snapshot_data(params.snapshot)?;
        let program = data.program(&params.project)?;
        let diagnostics = program.program_diagnostics().map_err(checker_error)?;
        Ok(program_diagnostic_responses(program, diagnostics))
    }

    /// The pin checks every file first, so the pool's accumulated global
    /// diagnostics are complete (an external pool reports them as its
    /// checkers are used), then keeps the entries of the project diagnostics
    /// without a file: the config-file parsing diagnostics, the program's
    /// own and the pool's globals, sorted and deduplicated as one list.
    /// port: tsc/internal/api/session.go:Session.handleGetGlobalDiagnostics
    /// port: tsc/internal/project/project.go:Project.GetProjectDiagnostics
    pub(super) fn handle_get_global_diagnostics(
        &self,
        ctx: &Context,
        params: &GetProjectDiagnosticsParams,
    ) -> SessionResult<Vec<Option<Box<DiagnosticResponse>>>> {
        let data = self.snapshot_data(params.snapshot)?;
        let project = data.project(&params.project)?;
        let program = data.program(&params.project)?;
        for file in program.files() {
            checker_diagnostics_of(project, program, ctx, DiagnosticKind::Semantic, file)?;
        }
        let scheduler = project
            .scheduler()
            .ok_or_else(|| SessionError::Other("project has no checker scheduler".into()))?;
        let mut diagnostics = program.config_file_parsing_diagnostics();
        diagnostics.extend_from_slice(program.program_diagnostics().map_err(checker_error)?);
        diagnostics.extend(scheduler.global_diagnostics().map_err(checker_error)?);
        let globals: Vec<Diagnostic> = program
            .sort_and_deduplicate_diagnostics(&diagnostics)
            .map_err(checker_error)?
            .into_iter()
            .filter(|diagnostic| diagnostic.file.is_none())
            .collect();
        Ok(program_diagnostic_responses(program, &globals))
    }
}
