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
use tsr_compiler::{CheckedProgram, EmitOnly, EmitOptions, EmitResult, Program, ProgramFile};
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

/// port: tsc/internal/api/session.go:emitProgram
fn emit_program(program: &Arc<Program>, options: &EmitOptions<'_>) -> SessionResult<EmitResult> {
    let checked = CheckedProgram::new(program.clone(), &Counters::new(), None);
    checked
        .emit(&tsr_checker::CheckerRequest::default(), options)
        .map_err(|error| SessionError::Other(format!("{error}")))?
        .ok_or_else(|| SessionError::Other("compiler emit returned nil result".into()))
}

/// port: tsc/internal/api/session.go:emitToOutput
fn emit_to_output(
    program: &Arc<Program>,
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
        emit_program(program, &options)?
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
    pub(super) fn handle_emit(&self, params: &EmitParams) -> SessionResult<EmitResponse> {
        let data = self.snapshot_data(params.snapshot)?;
        let program = data.program(&params.project)?;
        let emit_only = emit_only(params.emit_only.as_deref().copied())?;
        let fs: Arc<dyn FileSystem> = self.file_system().clone();
        let write_file = |file_name: &[u8], content: &[u8], _: &mut tsr_compiler::WriteFileData| {
            fs.write_file(file_name, content)
        };
        let result = emit_program(
            program,
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
        params: &EmitParams,
    ) -> SessionResult<EmitOutputResponse> {
        let data = self.snapshot_data(params.snapshot)?;
        let program = data.program(&params.project)?;
        let emit_only = emit_only(params.emit_only.as_deref().copied())?;
        emit_to_output(program, None, emit_only, false)
    }

    /// port: tsc/internal/api/session.go:Session.handleSelectedFilesEmit
    pub(super) fn handle_selected_files_emit(
        &self,
        params: &SelectedFilesEmitParams,
        emit_only: EmitOnly,
        files_named: bool,
    ) -> SessionResult<EmitOutputResponse> {
        let data = self.snapshot_data(params.snapshot)?;
        let program = data.program(&params.project)?;
        // The pin refuses a nil list and emits nothing for an empty one.
        if !files_named {
            return Err(client_error("files is required"));
        }
        let mut targets: Vec<Arc<ProgramFile>> = Vec::with_capacity(params.files.len());
        for file in &params.files {
            let name = file.to_file_name();
            let target = program
                .files()
                .iter()
                .find(|candidate| {
                    program
                        .source_file(name.as_bytes())
                        .is_some_and(|found| std::ptr::eq(found, candidate.as_ref()))
                })
                .cloned()
                .ok_or_else(|| {
                    client_error(format!("source file not found: {}", file.display()))
                })?;
            targets.push(target);
        }
        emit_to_output(program, Some(&targets), emit_only, true)
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
        let encoded = base64_decode(&params.data)
            .ok_or_else(|| client_error("invalid base64 data: illegal base64 data"))?;
        let view = file.bound().view().ast();
        let source = view
            .source_file(file.source())
            .map_err(|error| SessionError::Other(format!("{error:?}")))?;
        let position = source
            .position_map()
            .utf16_to_utf8(isize::try_from(params.position).unwrap_or(isize::MAX));
        let mut provider = tsr_parser::ParserJsDocProvider::default();
        let mut target = tsr_format::FormatFile {
            view,
            source: file.source(),
            jsdoc: &mut provider,
        };
        // The standalone session has the default preferences; an LSP-hosted
        // session's format settings are the server's.
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
