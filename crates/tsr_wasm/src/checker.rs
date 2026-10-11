use crate::api_error as error;
use std::sync::Arc;
use tsr_arena::Counters;
use tsr_ast::Diagnostic;
use tsr_compiler::diagnostic_writer::DiagnosticWriter;
use tsr_embed::{
    DiagnosticKind, EmitOnly, EmitOptions, EmitOutput, FileCache, ProgramOptions, Session,
    SessionRequest,
};
use tsr_jsstring::JsString;
use wasm_bindgen::prelude::*;

/// Explicit input snapshot. No method can fall back to the native filesystem.
#[wasm_bindgen]
pub struct MemoryHost {
    builder: tsr_vfs::MemoryBuilder,
    cwd: JsString,
    roots: Vec<JsString>,
}

#[wasm_bindgen]
impl MemoryHost {
    #[wasm_bindgen(constructor)]
    pub fn new(cwd: &[u8], case_sensitive: bool) -> Self {
        let mut builder = tsr_vfs::MemoryBuilder::new(cwd, case_sensitive);
        builder.insert_directory(cwd);
        Self {
            builder,
            cwd: JsString::from_bytes(cwd),
            roots: Vec::new(),
        }
    }

    /// Physical source bytes are decoded by the same loader as native input.
    pub fn add_file(&mut self, path: &[u8], contents: &[u8], root: bool) {
        self.builder.insert_physical(path, contents);
        if root {
            self.roots.push(JsString::from_bytes(path));
        }
    }

    pub fn add_directory(&mut self, path: &[u8]) {
        self.builder.insert_directory(path);
    }

    /// Declare an existing physical file whose content has not been supplied.
    /// Metadata and directory enumeration see it; a read fails explicitly.
    pub fn add_unavailable_file(&mut self, path: &[u8], raw_size: u64) {
        self.builder.insert_unavailable_file(path, raw_size);
    }

    pub fn add_symlink(&mut self, path: &[u8], target: &[u8]) {
        self.builder.insert_symlink(path, target);
    }

    /// Freeze the input snapshot without parsing configuration or loading a
    /// program. The returned host has no native filesystem fallback.
    pub fn prepare(self) -> PreparedHost {
        PreparedHost {
            fs: Arc::new(tsr_bundled::BundledFs::new(Arc::new(self.builder.finish()))),
            cwd: self.cwd,
        }
    }

    /// Consume this snapshot. `options` uses the compiler's numeric-enum JSON
    /// wire format (`tsr_tsoptions::raw`), not tsconfig string-valued enums.
    pub fn compile(self, options: &str) -> Result<WasmSession, JsValue> {
        let options = serde_json::from_str(options).map_err(error)?;
        let options = tsr_tsoptions::raw::compiler_options(&options)
            .map_err(|value| error(format!("compiler options: {value:?}")))?;
        let counters = Counters::new();
        let session = Session::load(
            ProgramOptions {
                config: tsr_tsoptions::ParsedCommandLine::new(options, self.roots),
                host: Arc::new(tsr_bundled::BundledFs::new(Arc::new(self.builder.finish()))),
                current_directory: self.cwd,
                default_library_path: JsString::from_bytes(tsr_bundled::LIB_PATH),
                skip_module_resolution: false,
                single_threaded: tsr_core::Tristate::TRUE,
            },
            &mut FileCache::new(),
            &counters,
        )
        .map_err(error)?;
        Ok(WasmSession::new(session))
    }
}

/// Frozen production host, suitable for loading a configured program in a
/// separately delimited operation. Builtin libraries use the production
/// bundled host; every other read stays in the supplied snapshot.
#[wasm_bindgen]
pub struct PreparedHost {
    fs: Arc<dyn tsr_vfs::FileSystem>,
    cwd: JsString,
}

#[wasm_bindgen]
impl PreparedHost {
    /// Parse a real tsconfig and its dependencies, then load one checker
    /// program. `arguments` is JSON byte arrays of ordered compiler override
    /// arguments, not pre-resolved options. A missing main config is an API
    /// error; recoverable config diagnostics stay with the loaded program.
    /// This narrow route rejects incremental/watch/external mapper execution.
    pub fn compile_config(self, path: &[u8], arguments: &str) -> Result<WasmSession, JsValue> {
        let arguments: Vec<Vec<u8>> = serde_json::from_str(arguments).map_err(error)?;
        let arguments: Vec<_> = arguments.into_iter().map(JsString::from_bytes).collect();
        let host = tsr_embed::CompilerConfigHost::new(self.fs.clone(), self.cwd.clone());
        let cli = tsr_tsoptions::parse_command_line(&arguments, &host);
        if !cli.root_file_names.is_empty()
            || !cli.options.project.is_empty()
            || cli.options.watch.is_true()
        {
            return Err(error("arguments must contain compiler overrides only"));
        }
        let parsed = tsr_tsoptions::get_parsed_command_line_of_config_file(
            path,
            &cli.options,
            &cli.raw,
            &host,
        )
        .map_err(error)?;
        let Some(mut config) = parsed.command_line else {
            return Err(error("main configuration could not be read"));
        };
        let mut diagnostics = cli.errors;
        diagnostics.append(&mut config.errors);
        config.errors = diagnostics;
        if config.options.incremental.is_true()
            || config.options.watch.is_true()
            || config.options.run_external_code.is_true()
            || config
                .content_mappers
                .as_ref()
                .is_some_and(|mappers| !mappers.is_empty())
        {
            return Err(error(
                "incremental/watch/external mapper execution unsupported",
            ));
        }
        let session = Session::load(
            ProgramOptions {
                config,
                host: self.fs,
                current_directory: self.cwd,
                default_library_path: JsString::from_bytes(tsr_bundled::LIB_PATH),
                skip_module_resolution: false,
                single_threaded: tsr_core::Tristate::TRUE,
            },
            &mut FileCache::new(),
            &Counters::new(),
        )
        .map_err(error)?;
        Ok(WasmSession::new(session))
    }
}

/// An owned checker session. `.free()` releases it; `.retire()` cancels further
/// queries. Do not call either after a trap: discard the enclosing instance.
#[wasm_bindgen]
pub struct WasmSession {
    session: Session,
    retained_diagnostics: Option<Vec<Diagnostic>>,
    retained_emit: Option<EmitOutput>,
    retained_observation: Option<crate::snapshot::Snapshot>,
}

impl WasmSession {
    fn new(session: Session) -> Self {
        Self {
            session,
            retained_diagnostics: None,
            retained_emit: None,
            retained_observation: None,
        }
    }
    fn freeze_observation(&mut self) -> Result<&crate::snapshot::Snapshot, JsValue> {
        if self.retained_observation.is_none() {
            let diagnostics = self
                .retained_diagnostics
                .as_ref()
                .ok_or_else(|| error("check has not completed"))?;
            self.retained_observation = Some(
                crate::snapshot::collect(&self.session, diagnostics, self.retained_emit.as_ref())
                    .map_err(error)?,
            );
        }
        Ok(self
            .retained_observation
            .as_ref()
            .expect("observation was initialized"))
    }
    fn collect_diagnostics(&self) -> Result<Vec<Diagnostic>, tsr_embed::SessionError> {
        let program = self.session.program();
        let request = SessionRequest::default();
        let mut diagnostics = Vec::new();
        for kind in [
            DiagnosticKind::Config,
            DiagnosticKind::Program,
            DiagnosticKind::Syntactic,
            DiagnosticKind::Global,
        ] {
            diagnostics.extend(self.session.diagnostics(&request, kind, None)?);
        }
        for file in program.files() {
            diagnostics.extend(self.session.diagnostics(
                &request,
                DiagnosticKind::Semantic,
                Some(file),
            )?);
            if program.options().declaration.is_true() || program.options().composite.is_true() {
                diagnostics.extend(self.session.diagnostics(
                    &request,
                    DiagnosticKind::Declaration,
                    Some(file),
                )?);
            }
        }
        program.sort_and_deduplicate_diagnostics(&diagnostics)
    }
}

#[wasm_bindgen]
impl WasmSession {
    pub fn retire(&self) {
        self.session.retire();
    }

    /// Return full diagnostic records with byte-array strings and UTF-8 byte
    /// ranges. This preserves malformed source bytes and nested message chains.
    pub fn diagnostics(&self) -> Result<Vec<u8>, JsValue> {
        let program = self.session.program();
        let values = self.collect_diagnostics().map_err(error)?;
        let sources = DiagnosticWriter::new(
            program,
            tsr_compiler::diagnostic_writer::FormattingOptions::default(),
        );
        let rows: Result<Vec<_>, _> = values
            .iter()
            .map(|value| diagnostic(&sources, value))
            .collect();
        serde_json::to_vec(&rows?).map_err(error)
    }

    /// Check all loaded files and retain the sorted diagnostic objects. No
    /// diagnostic JSON or output digest is constructed during this operation.
    /// Checks and emit share the same compiler pool and checker identities.
    pub fn check(&mut self) -> Result<(), JsValue> {
        self.retained_diagnostics = Some(self.collect_diagnostics().map_err(error)?);
        self.retained_observation = None;
        Ok(())
    }

    /// Retain a production emit and its callback writes without serialization.
    /// `request` has the same format as `emit`. A later call replaces the prior
    /// retained emit; `.free()` drops all retained source and output owners.
    pub fn emit_retained(&mut self, request: &str) -> Result<(), JsValue> {
        self.retained_emit = Some(self.emit_output(request)?);
        self.retained_observation = None;
        Ok(())
    }

    /// Snapshot owned sources, work counts, retained diagnostics and callback
    /// writes after checking. This method runs no diagnostic or emit phases.
    pub fn retained_snapshot(&self) -> Result<Vec<u8>, JsValue> {
        let diagnostics = self
            .retained_diagnostics
            .as_ref()
            .ok_or_else(|| error("check has not completed"))?;
        crate::snapshot::encode(&self.session, diagnostics, self.retained_emit.as_ref())
            .map_err(error)
    }

    /// Freeze retained metadata and diagnostic source indices without copying
    /// all source texts or serializing output rows. Counts are decimal strings.
    /// Subsequent row methods share this snapshot until another check or emit.
    pub fn retained_snapshot_header(&mut self) -> Result<Vec<u8>, JsValue> {
        crate::snapshot::header(self.freeze_observation()?).map_err(error)
    }

    /// Serialize one retained source, including its exact name, path and text.
    /// The zero-based index uses the order frozen by retained_snapshot_header.
    pub fn retained_source(&mut self, index: u32) -> Result<Vec<u8>, JsValue> {
        self.freeze_observation()?;
        crate::snapshot::source_row(
            self.retained_observation
                .as_ref()
                .expect("observation was initialized"),
            self.session.program(),
            index as usize,
        )
        .map_err(error)
    }

    /// Serialize one retained callback write in callback order.
    pub fn retained_write(&mut self, index: u32) -> Result<Vec<u8>, JsValue> {
        self.freeze_observation()?;
        crate::snapshot::write_row(
            self.retained_observation
                .as_ref()
                .expect("observation was initialized"),
            self.retained_emit.as_ref(),
            index as usize,
        )
        .map_err(error)
    }

    /// Return the loaded program's module-resolution trace in recorded order.
    /// Each row has a byte-array `message_key` and `args`, tagged `text` byte
    /// arrays or `bool` values. Rendering belongs to the caller; no queries or
    /// diagnostic scheduling are performed by this snapshot operation.
    pub fn resolution_trace(&self) -> Result<Vec<u8>, JsValue> {
        let rows: Vec<_> = self
            .session
            .program()
            .trace()
            .iter()
            .map(|entry| ResolutionTrace {
                message_key: entry.message.key.as_bytes(),
                args: entry
                    .args
                    .iter()
                    .map(|arg| match arg {
                        tsr_embed::TraceArg::Text(text) => TraceArgument::Text(text.as_bytes()),
                        tsr_embed::TraceArg::Bool(value) => TraceArgument::Bool(*value),
                    })
                    .collect(),
            })
            .collect();
        serde_json::to_vec(&rows).map_err(error)
    }

    /// Emit through `tsr_embed::Session::emit`, whose write callback keeps
    /// every output in memory: nothing reaches a file system. `request` is a
    /// JSON object whose keys are all optional: `files`, byte-array file
    /// names resolved as `Program.GetSourceFile` resolves them (absent or
    /// null for every file); `emitOnly`, the pin's API values (0 everything,
    /// 1 JavaScript, 2 declarations); `forceEmit`, a boolean. The result is
    /// JSON bytes: `emit_skipped`, `diagnostics` (rows as `diagnostics`
    /// returns them, in the emit's order) and `files`, each `name` and
    /// `text` a byte array, in `EmittedFiles` order. No acceptance claim:
    /// Phase 7 grades this entry point.
    pub fn emit(&self, request: &str) -> Result<Vec<u8>, JsValue> {
        let output = self.emit_output(request)?;
        let program = self.session.program();
        encode_emit(program, &output)
    }

    /// Query at a UTF-16 source offset, matching JavaScript-facing positions.
    /// The returned display is owned WTF-8 bytes, not a borrowed wasm view.
    pub fn type_at_position(&self, path: &[u8], position: u32) -> Result<Vec<u8>, JsValue> {
        let file = self
            .session
            .program()
            .file(path)
            .ok_or_else(|| error("file not in program"))?;
        let source = file.bound().view().source_file().map_err(error)?;
        let end = source
            .position_map()
            .utf8_to_utf16(source.text().as_bytes().len() as isize);
        if i64::from(position) > end as i64 {
            return Err(error("position outside source"));
        }
        let byte = source.position_map().utf16_to_utf8(position as isize);
        let mut provider = tsr_parser::ParserJsDocProvider::default();
        let mut navigator =
            tsr_astnav::Navigator::new(file.bound().view().ast(), file.source(), &mut provider);
        let node = navigator
            .get_token_at_position(byte as i64)
            .map_err(error)?;
        let mut operation = self.session.operation().map_err(error)?;
        let typ = operation.get_type_at_location(node).map_err(error)?;
        operation
            .type_to_string(
                typ,
                tsr_checker::type_format_flags::ALLOW_UNIQUE_ES_SYMBOL_TYPE
                    | tsr_checker::type_format_flags::USE_ALIAS_DEFINED_OUTSIDE_CURRENT_SCOPE,
            )
            .map(|text| text.as_bytes().to_vec())
            .map_err(error)
    }
}

impl WasmSession {
    fn emit_output(&self, request: &str) -> Result<EmitOutput, JsValue> {
        let request = EmitRequest::parse(request).map_err(error)?;
        let program = self.session.program();
        let files = request
            .files
            .as_ref()
            .map(|names| {
                names
                    .iter()
                    .map(|name| {
                        program
                            .source_file(name)
                            .ok_or_else(|| error("file not in program"))
                    })
                    .collect::<Result<Vec<_>, _>>()
            })
            .transpose()?;
        self.session
            .emit(&EmitOptions {
                target_source_files: files.as_deref(),
                emit_only: request.emit_only,
                force_emit: request.force_emit,
            })
            .map_err(error)
    }
}

fn encode_emit(program: &tsr_embed::Program, output: &EmitOutput) -> Result<Vec<u8>, JsValue> {
    let sources = DiagnosticWriter::new(
        program,
        tsr_compiler::diagnostic_writer::FormattingOptions::default(),
    );
    let rows: Result<Vec<_>, _> = output
        .diagnostics
        .iter()
        .map(|value| diagnostic(&sources, value))
        .collect();
    // Written field by field so output bytes go straight to the response
    // instead of through one JSON value per byte.
    let mut response = br#"{"emit_skipped":"#.to_vec();
    serde_json::to_writer(&mut response, &output.emit_skipped).map_err(error)?;
    response.extend_from_slice(br#","diagnostics":"#);
    serde_json::to_writer(&mut response, &rows?).map_err(error)?;
    response.extend_from_slice(br#","files":["#);
    for (index, file) in output.files.iter().enumerate() {
        if index > 0 {
            response.push(b',');
        }
        response.extend_from_slice(br#"{"name":"#);
        serde_json::to_writer(&mut response, file.name.as_bytes()).map_err(error)?;
        response.extend_from_slice(br#","text":"#);
        serde_json::to_writer(&mut response, file.text.as_slice()).map_err(error)?;
        response.push(b'}');
    }
    response.extend_from_slice(b"]}");
    Ok(response)
}

#[derive(serde::Serialize)]
struct ResolutionTrace<'a> {
    message_key: &'a [u8],
    args: Vec<TraceArgument<'a>>,
}

#[derive(serde::Serialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
enum TraceArgument<'a> {
    Text(&'a [u8]),
    Bool(bool),
}

/// A decoded `WasmSession::emit` request.
struct EmitRequest {
    files: Option<Vec<Vec<u8>>>,
    emit_only: EmitOnly,
    force_emit: bool,
}

impl EmitRequest {
    /// Absent and null fields take their defaults; an unknown field, a value
    /// of the wrong type and an `emitOnly` outside the pin's API range
    /// (`getEmitOnly`) are errors.
    fn parse(request: &str) -> Result<Self, String> {
        let request: serde_json::Value =
            serde_json::from_str(request).map_err(|value| value.to_string())?;
        let serde_json::Value::Object(fields) = request else {
            return Err("emit request must be a JSON object".to_owned());
        };
        let mut parsed = Self {
            files: None,
            emit_only: EmitOnly::All,
            force_emit: false,
        };
        for (key, value) in fields {
            match key.as_str() {
                "files" => {
                    parsed.files = serde_json::from_value(value)
                        .map_err(|value| format!("files must be byte arrays: {value}"))?;
                }
                "emitOnly" => {
                    parsed.emit_only = match value.as_u64() {
                        None if value.is_null() => EmitOnly::All,
                        Some(0) => EmitOnly::All,
                        Some(1) => EmitOnly::Js,
                        Some(2) => EmitOnly::Dts,
                        _ => return Err(format!("invalid emitOnly value: {value}")),
                    };
                }
                "forceEmit" => {
                    parsed.force_emit = match value {
                        serde_json::Value::Null => false,
                        serde_json::Value::Bool(value) => value,
                        value => return Err(format!("forceEmit must be a boolean: {value}")),
                    };
                }
                _ => return Err(format!("unknown emit request field: {key}")),
            }
        }
        Ok(parsed)
    }
}

fn diagnostic(
    sources: &DiagnosticWriter<'_>,
    value: &Diagnostic,
) -> Result<serde_json::Value, JsValue> {
    let file = if let Some(id) = value.file {
        // Reuse the program's owner index, also handling config sources. No
        // scan of all program files for each diagnostic or related record.
        let source = sources.source(id).map_err(error)?;
        Some(source.parse_options().file_name.as_bytes().to_vec())
    } else {
        None
    };
    let chain: Result<Vec<_>, _> = value
        .message_chain
        .iter()
        .map(|v| diagnostic(sources, v))
        .collect();
    let related: Result<Vec<_>, _> = value
        .related_information
        .iter()
        .map(|v| diagnostic(sources, v))
        .collect();
    Ok(serde_json::json!({
        "file": file, "start": value.loc.pos(), "end": value.loc.end(),
        "code": value.code, "category": value.category,
        "key": value.message_key.as_bytes(), "text": value.message_text.as_bytes(),
        "arguments": value.message_args.iter().map(JsString::as_bytes).collect::<Vec<_>>(),
        "chain": chain?, "related": related?,
        "reports_unnecessary": value.reports_unnecessary,
        "reports_deprecated": value.reports_deprecated,
    }))
}

#[cfg(test)]
mod trace_tests {
    use super::TraceArgument;

    #[test]
    fn resolution_trace_arguments_preserve_arbitrary_bytes_and_boolean_values() {
        let text = serde_json::to_value(TraceArgument::Text(&[0, 0xff, 0xed, 0xa0, 0x80])).unwrap();
        assert_eq!(
            text,
            serde_json::json!({"kind":"text","value":[0,255,237,160,128]})
        );
        let boolean = serde_json::to_value(TraceArgument::Bool(false)).unwrap();
        assert_eq!(boolean, serde_json::json!({"kind":"bool","value":false}));
    }
}

#[cfg(test)]
mod retained_tests {
    use super::MemoryHost;
    use std::sync::Arc;

    #[test]
    fn check_and_emit_share_checker_identity_and_retirement_gate() {
        let mut host = MemoryHost::new(b"/p", true);
        host.add_file(b"/p/main.ts", b"export const answer = 42;", true);
        let mut session = host.compile(r#"{"noLib":true}"#).unwrap();
        session.check().unwrap();
        let identity = |session: &super::WasmSession| {
            session
                .session
                .with_compiler_operations(|operations| {
                    assert_eq!(operations.len(), 1);
                    Ok(Arc::as_ptr(operations.iter().next().unwrap().owner().identity()) as usize)
                })
                .unwrap()
        };
        let before = identity(&session);
        session.emit_retained("{}").unwrap();
        assert_eq!(before, identity(&session));
        let owned = session.retained_snapshot().unwrap();
        session.retire();
        assert!(session.collect_diagnostics().is_err());
        drop(session);
        assert!(!owned.is_empty());
    }
    #[test]
    fn retained_rows_equal_complete_snapshot_and_invalidate_after_emit() {
        let mut host = MemoryHost::new(b"/p", true);
        host.add_file(b"/p/main.ts", b"export const answer: string = 42;", true);
        let mut session = host.compile(r#"{"noLib":true,"sourceMap":true}"#).unwrap();
        session.check().unwrap();
        session.emit_retained("{}").unwrap();
        let whole: serde_json::Value =
            serde_json::from_slice(&session.retained_snapshot().unwrap()).unwrap();
        let mut assembled: serde_json::Value =
            serde_json::from_slice(&session.retained_snapshot_header().unwrap()).unwrap();
        let sources = assembled["source_count"]
            .as_str()
            .unwrap()
            .parse::<u32>()
            .unwrap();
        let writes = assembled["write_count"]
            .as_str()
            .unwrap()
            .parse::<u32>()
            .unwrap();
        assembled.as_object_mut().unwrap().remove("source_count");
        assembled.as_object_mut().unwrap().remove("write_count");
        assembled["sources"] = serde_json::Value::Array(
            (0..sources)
                .map(|index| {
                    serde_json::from_slice(&session.retained_source(index).unwrap()).unwrap()
                })
                .collect(),
        );
        assembled["writes"] = serde_json::Value::Array(
            (0..writes)
                .map(|index| {
                    serde_json::from_slice(&session.retained_write(index).unwrap()).unwrap()
                })
                .collect(),
        );
        assert_eq!(assembled, whole);
        assert!(crate::snapshot::source_row(
            session.retained_observation.as_ref().unwrap(),
            session.session.program(),
            sources as usize
        )
        .is_err());
        assert!(crate::snapshot::write_row(
            session.retained_observation.as_ref().unwrap(),
            session.retained_emit.as_ref(),
            writes as usize
        )
        .is_err());
        let first = session.retained_snapshot_header().unwrap();
        assert_eq!(first, session.retained_snapshot_header().unwrap());
        session.emit_retained("{}").unwrap();
        assert!(session.retained_observation.is_none());
        session.retained_snapshot_header().unwrap();
        session.check().unwrap();
        assert!(session.retained_observation.is_none());
    }
}
