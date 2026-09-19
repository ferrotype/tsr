use crate::{ConfigValue, ParseConfigHost, ParsedCommandLine, TsConfigSourceFile};
use ts_ast::Diagnostic;
use ts_core::CompilerOptions;
use ts_jsstring::JsString;
use ts_vfs::Error;

/// Read errors are separate from recoverable configuration diagnostics, as in
/// the source API. Successful results retain all ASTs referenced by diagnostics.
#[derive(Debug)]
pub struct ReadConfigResult {
    pub command_line: Option<ParsedCommandLine>,
    pub read_errors: Vec<Diagnostic>,
}
/// port: tsc/internal/tsoptions/tsconfigparsing.go:GetParsedCommandLineOfConfigFile
pub fn get_parsed_command_line_of_config_file(
    name: &[u8],
    options: &CompilerOptions,
    raw: &ConfigValue,
    host: &dyn ParseConfigHost,
) -> Result<ReadConfigResult, Error> {
    let name = ts_tspath::absolute(name, host.current_directory());
    let path = ts_tspath::to_path(
        &name,
        host.current_directory(),
        host.fs().use_case_sensitive_file_names(),
    );
    get_parsed_command_line_of_config_file_path(&name, path, options, raw, host)
}
/// port: tsc/internal/tsoptions/tsconfigparsing.go:GetParsedCommandLineOfConfigFilePath
pub fn get_parsed_command_line_of_config_file_path(
    name: &[u8],
    path: JsString,
    options: &CompilerOptions,
    raw: &ConfigValue,
    host: &dyn ParseConfigHost,
) -> Result<ReadConfigResult, Error> {
    let Some(content) = host.fs().read_file(name)? else {
        return Ok(ReadConfigResult {
            command_line: None,
            read_errors: vec![Diagnostic::compiler(
                ts_diagnostics::Cannot_read_file_0,
                vec![JsString::from_bytes(name)],
            )],
        });
    };
    let source = TsConfigSourceFile::parse(JsString::from_bytes(name), path, content.text);
    let command_line = crate::parse_json_source_file_config_file_content(
        source,
        host,
        &ts_tspath::directory(name),
        options,
        raw,
        name,
    )?;
    Ok(ReadConfigResult {
        command_line: Some(command_line),
        read_errors: Vec::new(),
    })
}
