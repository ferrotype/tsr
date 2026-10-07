//! Configuration parsing and transpilation: the handlers that need no
//! snapshot.
//! port: tsc/internal/api/session.go
use super::responses::{config_file_response, diagnostic_response, diagnostic_responses};
use super::{ApiSession, SessionError, SessionResult};
use crate::proto::{
    ConfigFileResponse, ParseCommandLineParams, ParseConfigFileParams,
    ParseJsonConfigFileContentParams, ReadConfigFileParams, ReadConfigFileResponse,
    TranspileFromFileParams, TranspileOptions, TranspileOutputResponse, TranspileParams,
};
use std::sync::Arc;
use tsr_compiler::CompilerConfigHost;
use tsr_jsstring::{JsString, SourceText};
use tsr_tsoptions::{ConfigValue, TsConfigSourceFile};

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

impl ApiSession {
    /// The pin passes the snapshot host as the parse host; its resolver is
    /// the production module resolver over the session's file system.
    fn config_host(&self) -> CompilerConfigHost {
        CompilerConfigHost::new_live(self.file_system().clone(), self.current_directory().clone())
    }

    fn read_text(&self, file_name: &[u8]) -> Option<SourceText> {
        self.file_system()
            .read_file(file_name)
            .ok()
            .flatten()
            .map(|content| SourceText::from_loaded_bytes(content.raw))
    }

    /// port: tsc/internal/api/session.go:Session.handleParseCommandLine
    pub(super) fn handle_parse_command_line(
        &self,
        params: &ParseCommandLineParams,
    ) -> ConfigFileResponse {
        let args: Vec<JsString> = params
            .command_line
            .iter()
            .map(|arg| JsString::from_bytes(arg.as_bytes()))
            .collect();
        config_file_response(&tsr_tsoptions::parse_command_line(
            &args,
            &self.config_host(),
        ))
    }

    /// port: tsc/internal/api/session.go:Session.handleReadConfigFile
    pub(super) fn handle_read_config_file(
        &self,
        params: &ReadConfigFileParams,
    ) -> ReadConfigFileResponse {
        let config_file_name = params
            .file
            .to_absolute_file_name(self.current_directory().as_bytes());
        let Some(content) = self.read_text(config_file_name.as_bytes()) else {
            return ReadConfigFileResponse {
                config: Some(tsr_json::RawValue(b"{}".to_vec())),
                error: Some(Box::new(diagnostic_response(
                    &cannot_read_file(&config_file_name),
                    None,
                ))),
            };
        };
        let parsed = tsr_tsoptions::parse_config_file_text_to_json(
            config_file_name.clone(),
            self.to_path(config_file_name.as_bytes()),
            content,
        );
        ReadConfigFileResponse {
            config: tsr_json::marshal(&parsed.value, tsr_json::Options::default())
                .ok()
                .map(tsr_json::RawValue),
            error: parsed.diagnostics.first().map(|diagnostic| {
                Box::new(diagnostic_response(
                    diagnostic,
                    Some(parsed.source.file.view()),
                ))
            }),
        }
    }

    /// port: tsc/internal/api/session.go:Session.handleParseJsonConfigFileContent
    pub(super) fn handle_parse_json_config_file_content(
        &self,
        params: &ParseJsonConfigFileContentParams,
    ) -> SessionResult<ConfigFileResponse> {
        if params.config_directory.is_none() == params.config_file_name.is_none() {
            return Err(SessionError::Client(
                "exactly one of configDirectory or configFileName is required".into(),
            ));
        }
        let cwd = self.current_directory().clone();
        let (base_path, config_file_name) =
            match (&params.config_directory, &params.config_file_name) {
                (Some(directory), _) => (
                    tsr_tspath::absolute(directory.as_bytes(), cwd.as_bytes()),
                    Vec::new(),
                ),
                (None, Some(name)) => {
                    let name = name.to_absolute_file_name(cwd.as_bytes());
                    (
                        tsr_tspath::directory(name.as_bytes()),
                        name.as_bytes().to_vec(),
                    )
                }
                (None, None) => unreachable!("checked above"),
            };
        let mut value = JsonConfig::default();
        tsr_json::unmarshal(&params.json.0 .0, &mut value, tsr_json::Options::default())
            .map_err(|error| SessionError::InvalidRequest(format!("{error}")))?;
        let value = value.0;
        let command_line = tsr_tsoptions::parse_json_config_file_content(
            value,
            &self.config_host(),
            &base_path,
            &tsr_core::CompilerOptions::default(),
            &config_file_name,
            &[],
        )
        .map_err(|error| SessionError::Other(format!("{error:?}")))?;
        Ok(config_file_response(&command_line))
    }

    /// port: tsc/internal/api/session.go:Session.handleParseConfigFile
    pub(super) fn handle_parse_config_file(
        &self,
        params: &ParseConfigFileParams,
    ) -> SessionResult<ConfigFileResponse> {
        let config_file_name = params
            .file
            .to_absolute_file_name(self.current_directory().as_bytes());
        let Some(content) = self.read_text(config_file_name.as_bytes()) else {
            return Err(SessionError::Client(format!(
                "could not read file {}",
                tsr_jsstring::go_quote(config_file_name.as_bytes())
            )));
        };
        let config_dir = tsr_tspath::directory(config_file_name.as_bytes());
        let source = TsConfigSourceFile::parse(
            config_file_name.clone(),
            self.to_path(config_file_name.as_bytes()),
            content,
        );
        let command_line = tsr_tsoptions::parse_json_source_file_config_file_content(
            source,
            &self.config_host(),
            &config_dir,
            &tsr_core::CompilerOptions::default(),
            &ConfigValue::Null,
            config_file_name.as_bytes(),
        )
        .map_err(|error| SessionError::Other(format!("{error:?}")))?;
        Ok(config_file_response(&command_line))
    }

    /// port: tsc/internal/api/session.go:Session.handleTranspile
    pub(super) fn handle_transpile(
        params: &TranspileParams,
        declaration: bool,
    ) -> SessionResult<TranspileOutputResponse> {
        transpile_output(params.input.as_bytes(), &params.options, declaration)
    }

    /// port: tsc/internal/api/session.go:Session.handleTranspileFromFile
    pub(super) fn handle_transpile_from_file(
        &self,
        params: &TranspileFromFileParams,
        declaration: bool,
    ) -> SessionResult<TranspileOutputResponse> {
        let file_name = tsr_tspath::absolute(
            params.file_name.as_bytes(),
            self.current_directory().as_bytes(),
        );
        let Some(input) = self.read_text(&file_name) else {
            return Err(SessionError::Client(format!(
                "could not read file {}",
                tsr_jsstring::go_quote(&file_name)
            )));
        };
        let mut options = params.options.clone();
        options.file_name = text(&file_name);
        transpile_output(input.as_bytes(), &options, declaration)
    }
}

/// port: tsc/internal/api/session.go:transpileOutput
fn transpile_output(
    input: &[u8],
    options: &TranspileOptions,
    declaration: bool,
) -> SessionResult<TranspileOutputResponse> {
    let compiler_options = options.compiler_options.as_ref().map(|value| &value.0);
    let transpile_options = tsr_transpile::Options {
        compiler_options,
        file_name: options.file_name.as_bytes(),
        report_diagnostics: options.report_diagnostics,
    };
    let request = tsr_checker::CheckerRequest::default();
    let output = if declaration {
        tsr_transpile::transpile_declaration(&request, input, &transpile_options)
    } else {
        tsr_transpile::transpile_module(&request, input, &transpile_options)
    }
    .map_err(|error| SessionError::Other(format!("{error:?}")))?;
    let Some(output) = output else {
        return Err(SessionError::Other(
            "transpilation produced no output".into(),
        ));
    };
    // Diagnostics name files of the transpilation's own program.
    let program: &Arc<tsr_compiler::Program> = &output.program;
    let view = program
        .files()
        .first()
        .map(|file| file.bound().view().ast());
    Ok(TranspileOutputResponse {
        output_text: text(output.output_text.as_bytes()),
        diagnostics: diagnostic_responses(&output.diagnostics, view),
        source_map_text: text(output.source_map_text.as_bytes()),
    })
}

/// The pin's `ast.NewCompilerDiagnostic(diagnostics.Cannot_read_file_0, name)`.
fn cannot_read_file(file_name: &JsString) -> tsr_ast::Diagnostic {
    tsr_ast::Diagnostic::from_serialized(
        None,
        tsr_core::TextRange::new(0, 0),
        5083,
        tsr_diagnostics::Category::Error as i32,
        JsString::from_bytes(b"Cannot_read_file_0_5083".as_slice()),
        vec![file_name.clone()],
        Vec::new(),
        Vec::new(),
        false,
        false,
        false,
    )
}

/// The client's JSON value as a configuration value: nulls, booleans,
/// float64 numbers, strings, arrays with their null elements and objects in
/// key order, as the pin's `jsonValueToAny` produces them.
/// port: tsc/internal/api/proto.go:jsonValueToAny
struct JsonConfig(ConfigValue);
impl Default for JsonConfig {
    fn default() -> Self {
        Self(ConfigValue::Null)
    }
}
impl tsr_json::Decode for JsonConfig {
    fn decode(&mut self, input: &mut tsr_json::Decoder<'_>) -> Result<(), tsr_json::Error> {
        use tsr_json::Kind;
        self.0 = match input.peek_kind() {
            Kind::Null => {
                input.read_token()?;
                ConfigValue::Null
            }
            Kind::True | Kind::False => {
                let mut value = false;
                input.value(&mut value)?;
                ConfigValue::Boolean(value)
            }
            Kind::Number => {
                let mut value = 0.0f64;
                input.value(&mut value)?;
                ConfigValue::Number(value)
            }
            Kind::String => {
                let mut value = JsString::default();
                input.value(&mut value)?;
                ConfigValue::String(value)
            }
            Kind::BeginArray => {
                let mut items: Vec<Self> = Vec::new();
                input.value(&mut items)?;
                ConfigValue::Array(Some(items.into_iter().map(|item| item.0).collect()))
            }
            Kind::BeginObject => {
                let mut entries = tsr_core::collections::OrderedMap::default();
                input.object(|name, input| {
                    let mut child = Self::default();
                    input.value(&mut child)?;
                    entries.insert(JsString::from_bytes(name), child.0);
                    Ok(())
                })?;
                ConfigValue::Object(entries)
            }
            _ => return input.type_error("json value"),
        };
        Ok(())
    }
}
