//! Pinned test-only identity and synthetic projections used by CLI scenarios.
use crate::{identity_mapped_output, initialize_result, unexpected_method, MapperHandler};
use tsr_ast::span_map::SpanMap;
use tsr_contentmapper::{
    MappedOutput, OpenProjectParams, OpenProjectResult, OptionDiagnosticResult, TransformParams,
    TransformResultMessage, METHOD_CLOSE_PROJECT, METHOD_INITIALIZE, METHOD_OPEN_PROJECT,
    METHOD_TRANSFORM,
};
use tsr_ipc::{Context, HandlerResult};
use tsr_json::RawValue;

pub struct Verbatim {
    pub module: bool,
}
impl MapperHandler for Verbatim {
    // port: tsc/internal/testutil/contentmappertest/verbatim.go:verbatimHandler.HandleRequest
    // port: tsc/internal/testutil/contentmappertest/verbatim.go:moduleVerbatimHandler.HandleRequest
    fn handle_request(&self, _: &Context, method: &str, params: &[u8]) -> HandlerResult {
        match method {
            METHOD_INITIALIZE => Ok(Some(tsr_ipc::Response::json(initialize_result("mapper")))),
            METHOD_TRANSFORM => {
                let mut params_value = TransformParams::default();
                tsr_json::unmarshal(params, &mut params_value, tsr_json::Options::default())?;
                let mut mapped_output = identity_mapped_output(&params_value.content)?;
                if self.module {
                    mapped_output.extension = ".mts".into();
                }
                Ok(Some(tsr_ipc::Response::json(TransformResultMessage {
                    output: mapped_output,
                    ..TransformResultMessage::default()
                })))
            }
            _ => Err(unexpected_method(method)),
        }
    }
}

pub struct DynamicVerbatim {
    pub lifecycle: Option<std::sync::Arc<crate::ProjectLifecycle>>,
}
impl MapperHandler for DynamicVerbatim {
    // port: tsc/internal/testutil/contentmappertest/dynamic_verbatim.go:dynamicVerbatimHandler.HandleRequest
    fn handle_request(&self, ctx: &Context, method: &str, params: &[u8]) -> HandlerResult {
        match method {
            METHOD_OPEN_PROJECT => {
                if let Some(lifecycle) = &self.lifecycle {
                    lifecycle
                        .opens
                        .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
                }
                let mut p = OpenProjectParams::default();
                tsr_json::unmarshal(params, &mut p, tsr_json::Options::default())?;
                let mut identity = p.config_file_name.as_bytes().to_vec();
                identity.push(b':');
                if let Some(options) = &p.options {
                    identity.extend_from_slice(&options.0);
                }
                let mut directory = tsr_tspath::directory(p.config_file_name.as_bytes());
                if directory.is_empty() {
                    directory.push(b'/');
                }
                let diagnostics = if p.options.as_ref().map(|o| o.0.as_slice())
                    == Some(br#"{"plugins":[{"name":1}]}"#)
                {
                    vec![OptionDiagnosticResult {
                        path: vec![
                            RawValue(br#""plugins""#.to_vec()),
                            RawValue(b"0".to_vec()),
                            RawValue(br#""name""#.to_vec()),
                        ],
                        message_text: "Option 'name' requires a string.".into(),
                        code: 123,
                    }]
                } else {
                    Vec::new()
                };
                Ok(Some(tsr_ipc::Response::json(OpenProjectResult {
                    config_identity: String::from_utf8(identity)?,
                    watched_files: vec![String::from_utf8(tsr_tspath::combine(
                        &directory,
                        &[b"mapper.config.json"],
                    ))?],
                    option_diagnostics: diagnostics,
                })))
            }
            METHOD_CLOSE_PROJECT => {
                if let Some(lifecycle) = &self.lifecycle {
                    lifecycle
                        .closes
                        .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
                }
                Ok(None)
            }
            METHOD_TRANSFORM => {
                let mut p = TransformParams::default();
                tsr_json::unmarshal(params, &mut p, tsr_json::Options::default())?;
                if p.project_handle.is_empty() {
                    return Err("content mapper transform requires a project handle".into());
                }
                Verbatim { module: false }.handle_request(ctx, method, params)
            }
            _ => Verbatim { module: false }.handle_request(ctx, method, params),
        }
    }
}

pub struct Synthesizing;
impl MapperHandler for Synthesizing {
    // port: tsc/internal/testutil/contentmappertest/synthesizing.go:synthesizingHandler.HandleRequest
    fn handle_request(&self, _: &Context, method: &str, params: &[u8]) -> HandlerResult {
        match method {
            METHOD_INITIALIZE => Ok(Some(tsr_ipc::Response::json(initialize_result("mapper")))),
            METHOD_TRANSFORM => {
                let mut p = TransformParams::default();
                tsr_json::unmarshal(params, &mut p, tsr_json::Options::default())?;
                Ok(Some(tsr_ipc::Response::json(TransformResultMessage {
                    output: MappedOutput {
                        text: "export const el = jsxRuntime(Widget);\n".into(),
                        extension: ".ts".into(),
                        mappings: Some(RawValue(SpanMap::new(&[]).marshal()?)),
                        diagnostic_directives: None,
                    },
                    ..TransformResultMessage::default()
                })))
            }
            _ => Err(unexpected_method(method)),
        }
    }
}
