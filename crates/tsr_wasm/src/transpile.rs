//! Single-file production embedding operations with owned, byte-preserving
//! responses. This schema is independent of repository acceptance tooling.
use serde::Serialize;
use std::collections::HashMap;
use tsr_ast::{Diagnostic, NodeId};
use tsr_embed::{Program, TranspileOptions, TranspileOutput};
use tsr_jsstring::JsString;
use wasm_bindgen::prelude::*;

/// Transpile loaded source bytes to JavaScript through `tsr_embed`'s public
/// single-file operation. `compiler_options` is ordered numeric-enum JSON,
/// not tsconfig. Returns owned JSON bytes in the version-1 transpile response
/// format documented in this crate's README. No filesystem access or full
/// program compilation is substituted for this operation.
#[wasm_bindgen]
pub fn transpile_module(
    source: &[u8],
    filename: &[u8],
    compiler_options: &str,
    report_diagnostics: bool,
) -> Result<Vec<u8>, JsValue> {
    transpile(
        source,
        filename,
        compiler_options,
        report_diagnostics,
        false,
    )
    .map_err(crate::api_error)
}

/// Transpile loaded source bytes to declarations, using the public embedding
/// operation's isolated-declaration options and synthetic minimal library.
/// The response and option formats are the same as `transpile_module`.
#[wasm_bindgen]
pub fn transpile_declaration(
    source: &[u8],
    filename: &[u8],
    compiler_options: &str,
    report_diagnostics: bool,
) -> Result<Vec<u8>, JsValue> {
    transpile(source, filename, compiler_options, report_diagnostics, true)
        .map_err(crate::api_error)
}

fn transpile(
    source: &[u8],
    filename: &[u8],
    compiler_options: &str,
    report_diagnostics: bool,
    declaration: bool,
) -> Result<Vec<u8>, String> {
    if source.len() > i32::MAX as usize || filename.len() > i32::MAX as usize {
        return Err("source and filename must fit signed source positions".to_owned());
    }
    // serde's Value preserves order but replaces duplicate keys. Validate the
    // original serialized options before decoding them into that value.
    tsr_json::unmarshal(
        compiler_options.as_bytes(),
        &mut tsr_json::RawValue::default(),
        tsr_json::Options::default(),
    )
    .map_err(|error| format!("compiler options JSON: {error}"))?;
    let raw = serde_json::from_str(compiler_options)
        .map_err(|error| format!("compiler options JSON: {error}"))?;
    let options = tsr_tsoptions::raw::compiler_options(&raw)
        .map_err(|error| format!("compiler options: {error:?}"))?;
    let options = TranspileOptions {
        compiler_options: Some(&options),
        file_name: filename,
        report_diagnostics,
    };
    let request = tsr_embed::TranspileRequest::default();
    let output = if declaration {
        tsr_embed::transpile_declaration(&request, source, &options)
    } else {
        tsr_embed::transpile_module(&request, source, &options)
    }
    .map_err(|error| format!("transpilation failed: {error:?}"))?
    .ok_or_else(|| "transpilation was canceled".to_owned())?;
    encode(&output)
}

#[derive(Serialize)]
struct Response<'a> {
    version: u32,
    output: &'a [u8],
    source_map: &'a [u8],
    diagnostics: Vec<DiagnosticRecord>,
    sources: Vec<SourceRecord>,
}

#[derive(Serialize)]
struct SourceRecord {
    name: Vec<u8>,
    path: Vec<u8>,
    text: Vec<u8>,
    canonical_name: Option<Vec<u8>>,
}

#[derive(Serialize)]
struct DiagnosticRecord {
    file: Option<usize>,
    pos: i64,
    end: i64,
    code: i32,
    category: i32,
    source: Vec<u8>,
    key: Vec<u8>,
    text: Vec<u8>,
    args: Vec<Vec<u8>>,
    display_args: Vec<Vec<u8>>,
    has_message: bool,
    ad_hoc_message: Option<Vec<u8>>,
    chain: Vec<DiagnosticRecord>,
    related: Vec<DiagnosticRecord>,
    reports_unnecessary: bool,
    reports_deprecated: bool,
    skipped_on_no_emit: bool,
    repopulate: Option<RepopulateRecord>,
}

#[derive(Serialize)]
struct RepopulateRecord {
    kind: i32,
    module_reference: Vec<u8>,
    mode: i32,
    package_name: Vec<u8>,
}

fn encode(output: &TranspileOutput) -> Result<Vec<u8>, String> {
    let mut sources = Vec::with_capacity(output.program.files().len());
    let mut indices = HashMap::new();
    for file in output.program.files() {
        let view = file.bound().view();
        let source = view
            .source_file()
            .map_err(|error| format!("diagnostic source: {error:?}"))?;
        if source.content_mapper_info().is_some() {
            return Err(
                "transpile response does not support mapped or supplemental diagnostic sources"
                    .to_owned(),
            );
        }
        indices.insert(file.source(), sources.len());
        sources.push(SourceRecord {
            name: source.file_name().to_vec(),
            path: source.path().to_vec(),
            text: source.text().as_bytes().to_vec(),
            canonical_name: None,
        });
    }
    let diagnostics = output
        .diagnostics
        .iter()
        .map(|value| diagnostic(&output.program, &indices, value))
        .collect::<Result<_, _>>()?;
    // Output bytes serialize directly from their retained backing; avoid
    // constructing a serde_json::Value for each byte.
    serde_json::to_vec(&Response {
        version: 1,
        output: output.output_text.as_bytes(),
        source_map: output.source_map_text.as_bytes(),
        diagnostics,
        sources,
    })
    .map_err(|error| format!("transpile response: {error}"))
}

fn diagnostic(
    program: &Program,
    indices: &HashMap<NodeId, usize>,
    value: &Diagnostic,
) -> Result<DiagnosticRecord, String> {
    let file = value
        .file
        .map(|id| {
            indices.get(&id).copied().ok_or_else(|| {
                "diagnostic names a source outside the transpilation program".to_owned()
            })
        })
        .transpose()?;
    let display_args = if let Some(id) = value.file {
        let file = program
            .file_of_node(id)
            .ok_or_else(|| "diagnostic source owner is unavailable".to_owned())?;
        let view = file.bound().view();
        let source = view
            .source_file()
            .map_err(|error| format!("diagnostic source: {error:?}"))?;
        value.display_message_args_in(&source)
    } else {
        value.message_args.clone()
    };
    if let Some(message) = value.message {
        if tsr_diagnostics::by_key_bytes(value.message_key.as_bytes()) != Some(message) {
            return Err(
                "diagnostic message cannot be reconstructed from the pinned key".to_owned(),
            );
        }
    }
    Ok(DiagnosticRecord {
        file,
        pos: value.loc.pos(),
        end: value.loc.end(),
        code: value.code,
        category: value.category,
        source: value.source.as_bytes().to_vec(),
        key: value.message_key.as_bytes().to_vec(),
        text: value.message_text.as_bytes().to_vec(),
        args: bytes(&value.message_args),
        display_args: bytes(&display_args),
        has_message: value.message.is_some(),
        ad_hoc_message: value
            .ad_hoc_message
            .as_ref()
            .map(|value| value.text().to_vec()),
        chain: value
            .message_chain
            .iter()
            .map(|value| diagnostic(program, indices, value))
            .collect::<Result<_, _>>()?,
        related: value
            .related_information
            .iter()
            .map(|value| diagnostic(program, indices, value))
            .collect::<Result<_, _>>()?,
        reports_unnecessary: value.reports_unnecessary,
        reports_deprecated: value.reports_deprecated,
        skipped_on_no_emit: value.skipped_on_no_emit,
        repopulate: value
            .repopulate_info
            .as_ref()
            .map(|value| RepopulateRecord {
                kind: value.kind,
                module_reference: value.module_reference.as_bytes().to_vec(),
                mode: value.mode,
                package_name: value.package_name.as_bytes().to_vec(),
            }),
    })
}

fn bytes(values: &[JsString]) -> Vec<Vec<u8>> {
    values
        .iter()
        .map(|value| value.as_bytes().to_vec())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn malformed_options_are_recoverable_errors() {
        for options in [
            r#"{"target":1,"target":2}"#,
            r#"{"paths":{"x":["a"],"x":["b"]}}"#,
            r#"{"unknownPortableOption":true}"#,
            r#"{"target":"ESNext"}"#,
            "{} trailing",
        ] {
            assert!(transpile(b"const n = 1;", b"a.ts", options, true, false).is_err());
        }
    }
}
