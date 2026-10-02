use crate::api_error as error;
use std::sync::Arc;
use tsr_arena::Counters;
use tsr_ast::Diagnostic;
use tsr_compiler::diagnostic_writer::DiagnosticWriter;
use tsr_embed::{EmitOnly, EmitOptions, FileCache, ProgramOptions, Session};
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
        Self {
            builder: tsr_vfs::MemoryBuilder::new(cwd, case_sensitive),
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

    pub fn add_symlink(&mut self, path: &[u8], target: &[u8]) {
        self.builder.insert_symlink(path, target);
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
                single_threaded: tsr_core::Tristate::UNKNOWN,
            },
            &mut FileCache::new(),
            &counters,
        )
        .map_err(error)?;
        Ok(WasmSession { session })
    }
}

/// An owned checker session. `.free()` releases it; `.retire()` cancels further
/// queries. Do not call either after a trap: discard the enclosing instance.
#[wasm_bindgen]
pub struct WasmSession {
    session: Session,
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
        let mut operation = self.session.operation().map_err(error)?;
        let mut diagnostics = program.config().config_file_parsing_diagnostics();
        diagnostics.extend_from_slice(program.program_diagnostics().map_err(error)?);
        diagnostics.extend(program.syntactic_diagnostics(None).map_err(error)?);
        diagnostics.extend(operation.global_diagnostics().map_err(error)?);
        for file in program.files() {
            diagnostics.extend(
                program
                    .semantic_diagnostics_with_checker(&mut operation, file)
                    .map_err(error)?,
            );
            if program.options().declaration.is_true() || program.options().composite.is_true() {
                diagnostics.extend(
                    program
                        .declaration_diagnostics_with_checker(&mut operation, file)
                        .map_err(error)?,
                );
            }
        }
        let values = program
            .sort_and_deduplicate_diagnostics(&diagnostics)
            .map_err(error)?;
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
        let output = self
            .session
            .emit(&EmitOptions {
                target_source_files: files.as_deref(),
                emit_only: request.emit_only,
                force_emit: request.force_emit,
            })
            .map_err(error)?;
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
