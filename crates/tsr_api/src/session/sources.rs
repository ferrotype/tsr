//! Source files of a project: the encoded tree the client decodes into its
//! own AST, file names, metadata and the configuration source files.
//! port: tsc/internal/api/session.go
use super::responses::{base64_standard, file_name};
use super::{ApiSession, SessionError, SessionResult};
use crate::proto::{
    GetProjectDiagnosticsParams, GetSourceFileNamesParams, GetSourceFileParams, SourceFileMetadata,
    SourceFileResponse,
};
use tsr_ipc::Response;
use tsr_jsstring::JsString;
use tsr_vfs::FileSystem;

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

impl ApiSession {
    /// The encoded file as raw bytes on the msgpack protocol and base64 on
    /// JSON-RPC; `None` is an empty binary response or a JSON null.
    /// port: tsc/internal/api/session.go:Session.encodeSourceFileResponse
    fn encode_source_file_response(&self, encoded: Option<Vec<u8>>) -> Response {
        match (encoded, self.binary()) {
            (None, true) => Response::binary(Vec::new()),
            (None, false) => Response::json(None::<SourceFileResponse>),
            (Some(bytes), true) => Response::binary(bytes),
            (Some(bytes), false) => Response::json(SourceFileResponse {
                data: base64_standard(&bytes),
            }),
        }
    }

    fn encode_program_file(file: &tsr_compiler::ProgramFile) -> SessionResult<Vec<u8>> {
        let view = file.bound().view();
        tsr_encoder::encode_source_file(
            view.ast(),
            file.source(),
            &mut tsr_parser::ParserJsDocProvider::default(),
        )
        .map(|encoded| encoded.bytes)
        .map_err(|error| SessionError::Other(format!("failed to encode source file: {error:?}")))
    }

    /// port: tsc/internal/api/session.go:Session.handleGetSourceFile
    pub(super) fn handle_get_source_file(
        &self,
        params: &GetSourceFileParams,
    ) -> SessionResult<Response> {
        let data = self.snapshot_data(params.snapshot)?;
        let program = data.program(&params.project)?;
        let encoded = match program.source_file(params.file.to_file_name().as_bytes()) {
            Some(file) => Some(Self::encode_program_file(file)?),
            None => None,
        };
        Ok(self.encode_source_file_response(encoded))
    }

    /// port: tsc/internal/api/session.go:Session.handleGetSourceFileNames
    pub(super) fn handle_get_source_file_names(
        &self,
        params: &GetSourceFileNamesParams,
    ) -> SessionResult<Vec<String>> {
        let data = self.snapshot_data(params.snapshot)?;
        let program = data.program(&params.project)?;
        Ok(program
            .files()
            .iter()
            .map(|file| text(&file_name(file)))
            .collect())
    }

    /// port: tsc/internal/api/session.go:Session.handleGetSourceFileMetadata
    pub(super) fn handle_get_source_file_metadata(
        &self,
        params: &GetSourceFileParams,
    ) -> SessionResult<Option<SourceFileMetadata>> {
        let data = self.snapshot_data(params.snapshot)?;
        let program = data.program(&params.project)?;
        let Some(file) = program.source_file(params.file.to_file_name().as_bytes()) else {
            return Ok(None);
        };
        let path = super::responses::file_path(file);
        let metadata = program.metadata(&path);
        Ok(Some(SourceFileMetadata {
            is_default_library: program.is_lib(&path),
            is_from_external_library: program.is_external_library(&path),
            package_json_type: metadata
                .map_or_else(String::new, |m| text(m.package_json_type.as_bytes())),
            package_json_directory: metadata
                .map_or_else(String::new, |m| text(m.package_json_directory.as_bytes())),
            implied_node_format: metadata.map(|m| m.implied_node_format).unwrap_or_default(),
        }))
    }

    /// The config file of the project's command line and the files it
    /// extends; `None` for a project without a config source file.
    fn config_file_names(
        data: &super::SnapshotData,
        project: &crate::proto::ProjectId,
    ) -> SessionResult<Option<Vec<JsString>>> {
        let project = data.project(project)?;
        let command_line = &project.data().expect("loaded project").command_line;
        let Some(config) = &command_line.config_file else {
            return Ok(None);
        };
        let root = config
            .file
            .view()
            .source_file(config.root)
            .map(|source| JsString::from_bytes(source.file_name()))
            .map_err(|error| SessionError::Other(format!("{error:?}")))?;
        let mut names = vec![root];
        names.extend(config.extended_source_files.iter().cloned());
        Ok(Some(names))
    }

    /// port: tsc/internal/api/session.go:Session.handleGetConfigFileNames
    pub(super) fn handle_get_config_file_names(
        &self,
        params: &GetProjectDiagnosticsParams,
    ) -> SessionResult<Option<Vec<String>>> {
        let data = self.snapshot_data(params.snapshot)?;
        data.program(&params.project)?;
        Ok(Self::config_file_names(&data, &params.project)?
            .map(|names| names.iter().map(|name| text(name.as_bytes())).collect()))
    }

    /// port: tsc/internal/api/session.go:Session.handleGetConfigSourceFile
    pub(super) fn handle_get_config_source_file(
        &self,
        params: &GetSourceFileParams,
    ) -> SessionResult<Response> {
        let data = self.snapshot_data(params.snapshot)?;
        let program = data.program(&params.project)?;
        let project = data.project(&params.project)?;
        let command_line = &project.data().expect("loaded project").command_line;
        let Some(config) = &command_line.config_file else {
            return Ok(self.encode_source_file_response(None));
        };
        let requested = tsr_tspath::to_path(
            params.file.to_file_name().as_bytes(),
            program.current_directory(),
            program.use_case_sensitive_file_names(),
        );
        let root_view = config.file.view();
        let root = root_view
            .source_file(config.root)
            .map_err(|error| SessionError::Other(format!("{error:?}")))?;
        if root.path() == requested.as_bytes() {
            let encoded = tsr_encoder::encode_source_file(
                root_view,
                config.root,
                &mut tsr_parser::ParserJsDocProvider::default(),
            )
            .map(|encoded| encoded.bytes)
            .map_err(|error| {
                SessionError::Other(format!("failed to encode source file: {error:?}"))
            })?;
            return Ok(self.encode_source_file_response(Some(encoded)));
        }
        for config_file_name in &config.extended_source_files {
            let path = tsr_tspath::to_path(
                config_file_name.as_bytes(),
                program.current_directory(),
                program.use_case_sensitive_file_names(),
            );
            if path != requested {
                continue;
            }
            let Some(content) = data
                .snapshot
                .filesystem()
                .and_then(|fs| fs.read_file(config_file_name.as_bytes()).ok().flatten())
            else {
                return Ok(self.encode_source_file_response(None));
            };
            let source = tsr_tsoptions::TsConfigSourceFile::parse(
                config_file_name.clone(),
                requested,
                tsr_jsstring::SourceText::from_loaded_bytes(content.raw),
            );
            let encoded = tsr_encoder::encode_source_file(
                source.file.view(),
                source.root,
                &mut tsr_parser::ParserJsDocProvider::default(),
            )
            .map(|encoded| encoded.bytes)
            .map_err(|error| {
                SessionError::Other(format!("failed to encode source file: {error:?}"))
            })?;
            return Ok(self.encode_source_file_response(Some(encoded)));
        }
        Ok(self.encode_source_file_response(None))
    }
}
