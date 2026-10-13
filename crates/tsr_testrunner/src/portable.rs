//! Compilation in an external public-API consumer; rendering stays in the
//! existing runner. Only source shells are reconstructed here, never parsed
//! or checked. Child failure or malformed output is a failed observation.
use crate::harness::transpile::{Output, Unit};
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};
use tsr_arena::Counters;
use tsr_ast::{AstBuilder, AstFile, Diagnostic, NodeId, SourceFileParseOptions, SourceFileRead};
use tsr_compiler::diagnostic_writer::DiagnosticSources;
use tsr_core::TextRange;
use tsr_jsstring::{JsString, SourceText};
use tsr_transpile::Options;

#[path = "../../../tools/phase7/portable/protocol.rs"]
pub mod protocol;

/// Execute exactly one requested unit. `parity.py` owns the process-group
/// timeout, including this child. Stdout must be exactly one response.
pub fn transpile(
    consumer: &Path,
    unit: &Unit,
    options: &Options<'_>,
    declaration: bool,
) -> Result<Output, String> {
    let defaults = tsr_core::CompilerOptions::default();
    let request = protocol::TranspileRequest {
        version: protocol::VERSION,
        declaration,
        source: unit.content.clone(),
        filename: options.file_name.to_vec(),
        compiler_options: tsr_tsoptions::options_json::marshal_compiler_options(
            options.compiler_options.unwrap_or(&defaults),
        )
        .map_err(|error| format!("portable compiler options: {error}"))?,
        report_diagnostics: options.report_diagnostics,
    };
    request.validate()?;
    let input = serde_json::to_vec(&request).map_err(|error| error.to_string())?;
    let mut child = Command::new(consumer)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("starting {}: {error}", consumer.display()))?;
    let mut stdin = child.stdin.take().ok_or("consumer stdin unavailable")?;
    // A separate writer lets wait_with_output drain the output while a large
    // request is being sent. All pipe ends close even when the child fails.
    let (output, written) = std::thread::scope(|scope| {
        let writer = scope.spawn(move || {
            stdin.write_all(&input)?;
            stdin.write_all(b"\n")
        });
        (child.wait_with_output(), writer.join())
    });
    let output = output.map_err(|error| format!("waiting for consumer: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "consumer {} failed ({}): {}",
            consumer.display(),
            output.status,
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    written
        .map_err(|_| "consumer input writer panicked")?
        .map_err(|error| format!("writing consumer input: {error}"))?;
    let response: protocol::TranspileResponse = serde_json::from_slice(&output.stdout)
        .map_err(|error| format!("invalid consumer response: {error}"))?;
    restore(response)
}

pub(crate) struct Sources {
    owner: Option<AstFile>,
}
impl DiagnosticSources for Sources {
    fn diagnostic_source(&self, id: NodeId) -> Result<SourceFileRead<'_>, tsr_compiler::Error> {
        let owner = self.owner.as_ref().ok_or(tsr_arena::Error::WrongOwner)?;
        Ok(owner.view().source_file(id)?)
    }
}

fn restore(response: protocol::TranspileResponse) -> Result<Output, String> {
    response.validate()?;
    let counters = Counters::new();
    let mut factory = AstBuilder::new(SourceText::default(), &counters);
    let mut ids = Vec::with_capacity(response.sources.len());
    for source in &response.sources {
        let id = source_shell(&mut factory, &source.name, &source.path, &source.text)?;
        ids.push(id);
    }
    let sources = publish_sources(factory, &ids)?;
    let diagnostics = response
        .diagnostics
        .into_iter()
        .map(|value| diagnostic(value, &ids))
        .collect::<Result<_, _>>()?;
    Ok(Output {
        output_text: JsString::from_bytes(response.output),
        source_map_text: JsString::from_bytes(response.source_map),
        diagnostics,
        sources: Box::new(sources),
    })
}

pub(crate) fn source_shell(
    factory: &mut AstBuilder,
    name: &[u8],
    path: &[u8],
    text: &[u8],
) -> Result<NodeId, String> {
    if tsr_core::path::encoded_root_length(name) == 0
        || name != tsr_core::path::normalize(name).as_ref()
    {
        return Err("portable source name must be normalized and absolute".into());
    }
    let id = factory.new_source_file(
        SourceFileParseOptions {
            file_name: JsString::from_bytes(name),
            path: JsString::from_bytes(path),
            ..SourceFileParseOptions::default()
        },
        SourceText::from_loaded_bytes(text),
        None,
        None,
    );
    factory
        .node_mut(id)
        .map_err(|error| format!("source shell: {error:?}"))?
        .set_range(TextRange::new(0, text.len() as i64));
    Ok(id)
}

pub(crate) fn publish_sources(factory: AstBuilder, ids: &[NodeId]) -> Result<Sources, String> {
    let owner = ids
        .first()
        .map(|&root| {
            factory
                .complete(root)
                .and_then(tsr_ast::ParsedFile::try_publish_unbound)
                .map_err(|error| format!("source snapshots: {error:?}"))
        })
        .transpose()?;
    Ok(Sources { owner })
}

fn diagnostic(value: protocol::DiagnosticRecord, ids: &[NodeId]) -> Result<Diagnostic, String> {
    fn unmapped(value: &protocol::DiagnosticRecord) -> Result<(), String> {
        if value.display_args != value.args {
            return Err("portable diagnostic needs mapped display arguments".into());
        }
        for value in value.chain.iter().chain(&value.related) {
            unmapped(value)?;
        }
        Ok(())
    }
    unmapped(&value)?;
    crate::compiler_codec::thaw_diagnostic(value, &|index| {
        ids.get(index)
            .copied()
            .ok_or("diagnostic file index outside source snapshots".into())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::{errors, transpile};

    fn response() -> protocol::TranspileResponse {
        protocol::TranspileResponse {
            version: protocol::VERSION,
            output: b"var x = ;\n".to_vec(),
            source_map: vec![],
            sources: vec![protocol::SourceRecord {
                name: b"/x.ts".to_vec(),
                path: b"/x.ts".to_vec(),
                text: b"const x: number = ;".to_vec(),
                canonical_name: None,
            }],
            diagnostics: vec![protocol::DiagnosticRecord {
                file: Some(0),
                pos: 18,
                end: 19,
                code: 1109,
                category: 1,
                source: vec![],
                key: tsr_diagnostics::by_code(1109)
                    .unwrap()
                    .key
                    .as_bytes()
                    .to_vec(),
                text: vec![],
                args: vec![],
                display_args: vec![],
                has_message: true,
                ad_hoc_message: None,
                chain: vec![],
                related: vec![],
                reports_unnecessary: false,
                reports_deprecated: false,
                skipped_on_no_emit: false,
                repopulate: None,
            }],
        }
    }

    #[test]
    fn restores_source_only_diagnostics_for_the_existing_writer() {
        let mut row = response();
        let related = row.diagnostics[0].clone();
        row.diagnostics[0].related.push(related);
        let output = restore(row).unwrap();
        let diagnostic = &output.diagnostics[0];
        assert_eq!(diagnostic.file, diagnostic.related_information[0].file);
        let source = output
            .sources
            .diagnostic_source(diagnostic.file.unwrap())
            .unwrap();
        assert_eq!(source.text().as_bytes(), b"const x: number = ;");
        // No host parse or checker is needed to retain and render a diagnostic.
        assert_eq!(source.node_count, 0);
        let rendered = errors::error_baseline(
            output.sources.as_ref(),
            &[errors::InputFile {
                name: b"/x.ts",
                content: b"const x: number = ;",
            }],
            &output.diagnostics,
            false,
        )
        .unwrap();
        assert!(rendered
            .text
            .windows(b"Expression expected".len())
            .any(|w| w == b"Expression expected"));
    }

    #[test]
    fn bad_source_links_and_unknown_messages_fail_before_rendering() {
        let mut row = response();
        row.diagnostics[0].file = Some(8);
        assert!(restore(row).is_err());
        let mut row = response();
        row.diagnostics[0].key = b"unknown".to_vec();
        assert!(restore(row).is_err());
        let mut row = response();
        row.diagnostics[0].display_args.push(b"mapped".to_vec());
        assert!(restore(row).is_err());
    }

    #[test]
    fn failed_backend_is_not_replaced_by_native_compilation() {
        let options = tsr_core::CompilerOptions::default();
        let unit = transpile::Unit {
            name: b"x.ts".to_vec(),
            content: b"const x = 1;".to_vec(),
        };
        let config = transpile::Configuration {
            file: b"transpile/x.ts",
            name: b"",
            units: &[unit],
            options: &options,
            report_diagnostics: true,
        };
        let mut calls = 0;
        let result = transpile::run_one_with(&config, false, &mut |_, _, _| {
            calls += 1;
            Err("injected consumer trap".to_owned())
        });
        assert!(matches!(result, Err(error) if error == "injected consumer trap"));
        assert_eq!(calls, 1);
    }
}
