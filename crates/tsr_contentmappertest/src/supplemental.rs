//! Mappers whose output comes with supplemental virtual files.
use crate::{identity_mapped_output, initialize_result, unexpected_method, MapperHandler};
use tsr_ast::span_map::{SpanMap, FEATURE_ALL, KIND_VERBATIM};
use tsr_ast::SpanSegment;
use tsr_contentmapper::{
    MappedOutput, TransformParams, TransformResultMessage, METHOD_INITIALIZE, METHOD_TRANSFORM,
};
use tsr_ipc::{Context, HandlerError, HandlerResult};
use tsr_json::RawValue;

fn output(text: &str) -> MappedOutput {
    MappedOutput {
        text: text.into(),
        extension: ".ts".into(),
        mappings: None,
        diagnostic_directives: None,
    }
}

fn params(bytes: &[u8]) -> Result<TransformParams, HandlerError> {
    let mut params = TransformParams::default();
    tsr_json::unmarshal(bytes, &mut params, tsr_json::Options::default())?;
    Ok(params)
}

fn result(canonical: &str, supplemental: MappedOutput) -> tsr_ipc::Response {
    tsr_ipc::Response::json(TransformResultMessage {
        output: output(canonical),
        diagnostics: Vec::new(),
        supplemental: vec![supplemental],
    })
}

/// The whole input as one supplemental file beside an empty module.
pub struct SupplementalHandler;

impl MapperHandler for SupplementalHandler {
    /// port: tsc/internal/testutil/contentmappertest/supplemental.go:supplementalHandler.HandleRequest
    fn handle_request(&self, _: &Context, method: &str, bytes: &[u8]) -> HandlerResult {
        match method {
            METHOD_INITIALIZE => Ok(Some(tsr_ipc::Response::json(initialize_result("mapper")))),
            METHOD_TRANSFORM => {
                let params = params(bytes)?;
                Ok(Some(result(
                    "export {};",
                    identity_mapped_output(&params.content)?,
                )))
            }
            _ => Err(unexpected_method(method)),
        }
    }
}

/// The input after a synthesized statement that fails to resolve.
pub struct SupplementalDiagnosticsHandler;

impl MapperHandler for SupplementalDiagnosticsHandler {
    /// port: tsc/internal/testutil/contentmappertest/supplemental_diagnostics.go:supplementalDiagnosticsHandler.HandleRequest
    fn handle_request(&self, _: &Context, method: &str, bytes: &[u8]) -> HandlerResult {
        match method {
            METHOD_INITIALIZE => Ok(Some(tsr_ipc::Response::json(initialize_result("mapper")))),
            METHOD_TRANSFORM => {
                let params = params(bytes)?;
                const PREFIX: &str = "missingSupplementalGlobal;\n";
                let (prefix, content) = (
                    i32::try_from(PREFIX.len())?,
                    i32::try_from(params.content.len())?,
                );
                let mappings = SpanMap::new(&[SpanSegment {
                    virtual_start: prefix,
                    virtual_end: prefix + content,
                    original_start: 0,
                    original_end: content,
                    kind: KIND_VERBATIM,
                    features: FEATURE_ALL,
                }])
                .marshal()?;
                Ok(Some(result(
                    "export {};",
                    MappedOutput {
                        mappings: Some(RawValue(mappings)),
                        ..output(&format!("{PREFIX}{}", params.content))
                    },
                )))
            }
            _ => Err(unexpected_method(method)),
        }
    }
}

/// Global declarations shared between two mapped files.
pub struct SupplementalGlobalsHandler;

impl MapperHandler for SupplementalGlobalsHandler {
    /// port: tsc/internal/testutil/contentmappertest/supplemental_globals.go:supplementalGlobalsHandler.HandleRequest
    fn handle_request(&self, _: &Context, method: &str, bytes: &[u8]) -> HandlerResult {
        match method {
            METHOD_INITIALIZE => Ok(Some(tsr_ipc::Response::json(initialize_result("mapper")))),
            METHOD_TRANSFORM => {
                let params = params(bytes)?;
                let supplemental = if params.file_name.ends_with("/a.vue") {
                    "/// <reference path=\"./extra.d.ts\" />\ninterface Shared extends Extra { value: string }"
                } else if params.file_name.ends_with("/b.vue") {
                    "declare const shared: Shared;"
                } else {
                    return Err(format!(
                        "contentmappertest: unexpected supplemental global input {}",
                        tsr_jsstring::go_quote(params.file_name.as_bytes())
                    )
                    .into());
                };
                Ok(Some(result(
                    "export default shared.value;",
                    output(supplemental),
                )))
            }
            _ => Err(unexpected_method(method)),
        }
    }
}

/// A supplemental module with a type error of its own.
pub struct SupplementalModuleHandler;

impl MapperHandler for SupplementalModuleHandler {
    /// port: tsc/internal/testutil/contentmappertest/supplemental_module.go:supplementalModuleHandler.HandleRequest
    fn handle_request(&self, _: &Context, method: &str, bytes: &[u8]) -> HandlerResult {
        match method {
            METHOD_INITIALIZE => Ok(Some(tsr_ipc::Response::json(initialize_result("mapper")))),
            METHOD_TRANSFORM => {
                params(bytes)?;
                Ok(Some(result(
                    "export default 1;",
                    output(r#"export const privateValue: number = "wrong";"#),
                )))
            }
            _ => Err(unexpected_method(method)),
        }
    }
}
