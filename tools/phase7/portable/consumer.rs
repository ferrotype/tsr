//! External execution boundary: only the shipped embedding transpile API runs
//! compiler work. Freeze every diagnostic's source before dropping its owner.
use crate::protocol::{
    DiagnosticRecord, RepopulateRecord, SourceRecord, TranspileRequest, TranspileResponse, VERSION,
};
use std::collections::HashMap;
use tsr_ast::{Diagnostic, NodeId};
use tsr_embed::{Program, TranspileOptions, TranspileOutput};
use tsr_jsstring::JsString;

pub fn execute(request: &TranspileRequest) -> Result<TranspileResponse, String> {
    request.validate()?;
    // Validate duplicate keys as well as JSON syntax before serde's ordered
    // Value decoder. It would otherwise silently replace duplicate entries.
    tsr_json::unmarshal(
        &request.compiler_options,
        &mut tsr_json::RawValue::default(),
        tsr_json::Options::default(),
    )
    .map_err(|error| format!("compiler options JSON: {error}"))?;
    let raw = serde_json::from_slice(&request.compiler_options)
        .map_err(|error| format!("compiler options JSON: {error}"))?;
    let options = tsr_tsoptions::raw::compiler_options(&raw)
        .map_err(|error| format!("compiler options: {error:?}"))?;
    let options = TranspileOptions {
        compiler_options: Some(&options),
        file_name: &request.filename,
        report_diagnostics: request.report_diagnostics,
    };
    let operation = tsr_embed::TranspileRequest::default();
    let output = if request.declaration {
        tsr_embed::transpile_declaration(&operation, &request.source, &options)
    } else {
        tsr_embed::transpile_module(&operation, &request.source, &options)
    }
    .map_err(|error| format!("transpilation failed: {error:?}"))?
    .ok_or_else(|| "transpilation was canceled".to_owned())?;
    freeze(&output)
}

fn freeze(output: &TranspileOutput) -> Result<TranspileResponse, String> {
    let mut sources = Vec::with_capacity(output.program.files().len());
    let mut indices = HashMap::new();
    for file in output.program.files() {
        let view = file.bound().view();
        let source = view
            .source_file()
            .map_err(|error| format!("diagnostic source: {error:?}"))?;
        if source.content_mapper_info().is_some() {
            return Err(
                "portable transpile does not support mapped or supplemental diagnostic sources"
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
        .map(|diagnostic| freeze_diagnostic(&output.program, &indices, diagnostic))
        .collect::<Result<_, _>>()?;
    let response = TranspileResponse {
        version: VERSION,
        output: output.output_text.as_bytes().to_vec(),
        source_map: output.source_map_text.as_bytes().to_vec(),
        diagnostics,
        sources,
    };
    response.validate()?;
    Ok(response)
}

fn freeze_diagnostic(
    program: &Program,
    indices: &HashMap<NodeId, usize>,
    diagnostic: &Diagnostic,
) -> Result<DiagnosticRecord, String> {
    let file = diagnostic
        .file
        .map(|id| {
            indices.get(&id).copied().ok_or_else(|| {
                "diagnostic names a source outside the transpilation program".to_owned()
            })
        })
        .transpose()?;
    let display_args = if let Some(id) = diagnostic.file {
        let file = program
            .file_of_node(id)
            .ok_or_else(|| "diagnostic source owner is unavailable".to_owned())?;
        let view = file.bound().view();
        let source = view
            .source_file()
            .map_err(|error| format!("diagnostic source: {error:?}"))?;
        diagnostic.display_message_args_in(&source)
    } else {
        diagnostic.message_args.clone()
    };
    if let Some(message) = diagnostic.message {
        if tsr_diagnostics::by_key_bytes(diagnostic.message_key.as_bytes()) != Some(message) {
            return Err(
                "diagnostic message cannot be reconstructed from the pinned key".to_owned(),
            );
        }
    }
    Ok(DiagnosticRecord {
        file,
        pos: diagnostic.loc.pos(),
        end: diagnostic.loc.end(),
        code: diagnostic.code,
        category: diagnostic.category,
        source: diagnostic.source.as_bytes().to_vec(),
        key: diagnostic.message_key.as_bytes().to_vec(),
        text: diagnostic.message_text.as_bytes().to_vec(),
        args: bytes(&diagnostic.message_args),
        display_args: bytes(&display_args),
        has_message: diagnostic.message.is_some(),
        ad_hoc_message: diagnostic
            .ad_hoc_message
            .as_ref()
            .map(|value| value.text().to_vec()),
        chain: diagnostic
            .message_chain
            .iter()
            .map(|value| freeze_diagnostic(program, indices, value))
            .collect::<Result<_, _>>()?,
        related: diagnostic
            .related_information
            .iter()
            .map(|value| freeze_diagnostic(program, indices, value))
            .collect::<Result<_, _>>()?,
        reports_unnecessary: diagnostic.reports_unnecessary,
        reports_deprecated: diagnostic.reports_deprecated,
        skipped_on_no_emit: diagnostic.skipped_on_no_emit,
        repopulate: diagnostic
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

    fn request() -> TranspileRequest {
        TranspileRequest {
            version: VERSION,
            declaration: false,
            source: b"export const n: number = ;".to_vec(),
            filename: b"syntax.ts".to_vec(),
            compiler_options: b"{}".to_vec(),
            report_diagnostics: true,
        }
    }

    #[test]
    fn returned_diagnostics_keep_source_bytes_after_the_program_is_dropped() {
        let request = request();
        let response = execute(&request).unwrap();
        let diagnostic = response
            .diagnostics
            .iter()
            .find(|diagnostic| diagnostic.code == 1109)
            .expect("expression expected");
        let source = &response.sources[diagnostic.file.expect("syntax error file")];
        assert_eq!(source.text, request.source);
        assert!(source.name.ends_with(b"syntax.ts"));
        assert_eq!(diagnostic.pos, 25);
        assert_eq!(diagnostic.args, diagnostic.display_args);
        assert!(diagnostic.has_message);
    }

    #[test]
    fn invalid_options_fail_before_transpilation() {
        for options in [
            br#"{"target":1,"target":2}"#.as_slice(),
            br#"{"paths":{"x":["a"],"x":["b"]}}"#,
            br#"{"unknownPortableOption":true}"#,
            br#"{"target":"ESNext"}"#,
            b"{} trailing",
        ] {
            let mut request = request();
            request.compiler_options = options.to_vec();
            assert!(execute(&request).is_err(), "{options:?}");
        }
    }
}
