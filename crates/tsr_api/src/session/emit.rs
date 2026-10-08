//! Emit through a checked program of the project's program: to the
//! session's file system (`emit`), to collected outputs (`emitToString`),
//! or for selected files (`getJavaScriptEmit`, `getDeclarationEmit`); and
//! the insertion formatter.
//! port: tsc/internal/api/session.go
use super::diagnostics::program_diagnostic_responses;
use super::responses::base64_decode;
use super::{client_error, ApiSession, SessionError, SessionResult};
use crate::proto::{
    EmitOutputFile, EmitOutputResponse, EmitParams, EmitResponse, FormatNodeForInsertionParams,
    SelectedFilesEmitParams,
};
use std::sync::{Arc, Mutex};
use tsr_arena::Counters;
use tsr_checker::{CheckerLifetime, CheckerRequest};
use tsr_compiler::{CheckedProgram, EmitOnly, EmitOptions, EmitResult, Program, ProgramFile};
use tsr_ipc::Context;
use tsr_project::Project;
use tsr_vfs::FileSystem;

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// port: tsc/internal/api/session.go:getEmitOnly
fn emit_only(value: Option<u32>) -> SessionResult<EmitOnly> {
    Ok(match value {
        None | Some(0) => EmitOnly::All,
        Some(1) => EmitOnly::Js,
        Some(2) => EmitOnly::Dts,
        Some(other) => return Err(client_error(format!("invalid emitOnly value: {other}"))),
    })
}

/// The pin's `program.Emit` takes each file's checker from the program's
/// pool, which is the project's, so an emit after a check reuses the checked
/// files; a private pool would recheck everything on every call. The request
/// is canceled with the connection's context, as the pin's `emitProgram`
/// honours the request context.
/// port: tsc/internal/api/session.go:emitProgram
fn emit_program(
    program: &Arc<Program>,
    project: &Project,
    ctx: &Context,
    options: &EmitOptions<'_>,
) -> SessionResult<EmitResult> {
    let checked = CheckedProgram::with_pool(program.clone(), Arc::new(project.clone()));
    let cancellation = tsr_core::CancellationToken::new();
    let _stop = ctx.after_func({
        let cancellation = cancellation.clone();
        move || cancellation.cancel()
    });
    let request = CheckerRequest {
        lifetime: CheckerLifetime::Temporary,
        cancellation: Some(cancellation),
    };
    checked
        .emit(&request, options)
        .map_err(|error| SessionError::Other(format!("{error}")))?
        .ok_or_else(|| SessionError::Other("compiler emit returned nil result".into()))
}

/// port: tsc/internal/api/session.go:emitToOutput
fn emit_to_output(
    program: &Arc<Program>,
    project: &Project,
    ctx: &Context,
    targets: Option<&[Arc<ProgramFile>]>,
    emit_only: EmitOnly,
    force_emit: bool,
) -> SessionResult<EmitOutputResponse> {
    let outputs: Mutex<Vec<EmitOutputFile>> = Mutex::new(Vec::new());
    let result = {
        let write_file =
            |file_name: &[u8], content: &[u8], data: &mut tsr_compiler::WriteFileData| {
                let source_file_name = data.source_file.and_then(|source| {
                    program
                        .files()
                        .iter()
                        .find(|file| file.source() == source)
                        .map(|file| text(&super::responses::file_name(file)))
                });
                outputs.lock().expect("emit outputs").push(EmitOutputFile {
                    file_name: text(file_name),
                    text: text(content),
                    source_file_name: source_file_name.map(Box::new),
                });
                Ok(())
            };
        let options = EmitOptions {
            target_source_files: targets,
            emit_only,
            force_emit,
            write_file: Some(&write_file),
        };
        emit_program(program, project, ctx, &options)?
    };
    let mut outputs = outputs.into_inner().expect("emit outputs");
    outputs.sort_by(|a, b| a.file_name.cmp(&b.file_name));
    Ok(EmitOutputResponse {
        emit_skipped: result.emit_skipped,
        diagnostics: program_diagnostic_responses(program, &result.diagnostics),
        output_files: outputs
            .into_iter()
            .map(|file| Some(Box::new(file)))
            .collect(),
    })
}

impl ApiSession {
    /// Writes through the session's file system, which is the client's
    /// callback file system when `writeFile` is enabled.
    /// port: tsc/internal/api/session.go:Session.handleEmit
    pub(super) fn handle_emit(
        &self,
        ctx: &Context,
        params: &EmitParams,
    ) -> SessionResult<EmitResponse> {
        let data = self.snapshot_data(params.snapshot)?;
        let project = data.project(&params.project)?;
        let program = data.program(&params.project)?;
        let emit_only = emit_only(params.emit_only.as_deref().copied())?;
        let fs: Arc<dyn FileSystem> = self.file_system().clone();
        let write_file = |file_name: &[u8], content: &[u8], _: &mut tsr_compiler::WriteFileData| {
            fs.write_file(file_name, content)
        };
        let result = emit_program(
            program,
            project,
            ctx,
            &EmitOptions {
                emit_only,
                write_file: Some(&write_file),
                ..EmitOptions::default()
            },
        )?;
        Ok(EmitResponse {
            emit_skipped: result.emit_skipped,
            diagnostics: program_diagnostic_responses(program, &result.diagnostics),
            emitted_files: result
                .emitted_files
                .iter()
                .map(|name| text(name.as_bytes()))
                .collect(),
        })
    }

    /// port: tsc/internal/api/session.go:Session.handleEmitToString
    pub(super) fn handle_emit_to_string(
        &self,
        ctx: &Context,
        params: &EmitParams,
    ) -> SessionResult<EmitOutputResponse> {
        let data = self.snapshot_data(params.snapshot)?;
        let project = data.project(&params.project)?;
        let program = data.program(&params.project)?;
        let emit_only = emit_only(params.emit_only.as_deref().copied())?;
        emit_to_output(program, project, ctx, None, emit_only, false)
    }

    /// port: tsc/internal/api/session.go:Session.handleSelectedFilesEmit
    pub(super) fn handle_selected_files_emit(
        &self,
        ctx: &Context,
        params: &SelectedFilesEmitParams,
        emit_only: EmitOnly,
        files_named: bool,
    ) -> SessionResult<EmitOutputResponse> {
        let data = self.snapshot_data(params.snapshot)?;
        let project = data.project(&params.project)?;
        let program = data.program(&params.project)?;
        // The pin refuses a nil list and emits nothing for an empty one.
        if !files_named {
            return Err(client_error("files is required"));
        }
        let mut targets: Vec<Arc<ProgramFile>> = Vec::with_capacity(params.files.len());
        for file in &params.files {
            let found = program.source_file(file.to_file_name().as_bytes());
            let target = found
                .and_then(|found| {
                    program
                        .files()
                        .iter()
                        .find(|candidate| std::ptr::eq(found, candidate.as_ref()))
                })
                .cloned()
                .ok_or_else(|| {
                    client_error(format!("source file not found: {}", file.display()))
                })?;
            targets.push(target);
        }
        emit_to_output(program, project, ctx, Some(&targets), emit_only, true)
    }

    /// port: tsc/internal/api/session.go:Session.handleFormatNodeForInsertion
    pub(super) fn handle_format_node_for_insertion(
        &self,
        params: &FormatNodeForInsertionParams,
    ) -> SessionResult<String> {
        let data = self.snapshot_data(params.snapshot)?;
        let program = data.program(&params.project)?;
        let file = program
            .source_file(params.file.to_file_name().as_bytes())
            .ok_or_else(|| {
                client_error(format!("source file not found: {}", params.file.display()))
            })?;
        let encoded = base64_decode(&params.data).map_err(|at| {
            client_error(format!(
                "invalid base64 data: illegal base64 data at input byte {at}"
            ))
        })?;
        let view = file.bound().view().ast();
        let source = view
            .source_file(file.source())
            .map_err(|error| SessionError::Other(format!("{error}")))?;
        let position = source
            .position_map()
            .utf16_to_utf8(isize::try_from(params.position).unwrap_or(isize::MAX));
        let mut provider = tsr_parser::ParserJsDocProvider::default();
        let mut target = tsr_format::FormatFile {
            view,
            source: file.source(),
            jsdoc: &mut provider,
        };
        // The default settings for every session: the pin reads the
        // snapshot's format settings, which only its LSP-hosted session has.
        let settings = tsr_format::FormatCodeSettings::default();
        crate::format_node_for_insertion(
            &encoded,
            &mut target,
            i64::try_from(position).unwrap_or(i64::MAX),
            &settings,
            &Counters::new(),
        )
        .map(|formatted| text(&formatted))
        .map_err(|error| match error {
            crate::FormatError::Decode(error) => {
                client_error(format!("failed to decode AST: {error}"))
            }
            other => SessionError::Other(format!("{other}")),
        })
    }
}
