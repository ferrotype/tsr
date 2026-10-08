//! Realistic content mappers served in process, as the pinned harness serves
//! `testutil/contentmappertest`: a spawner that connects each mapper command
//! to its handler over an in-memory pipe, the static project protocol most
//! mappers share, and the mappers the corpus's `runExternalCode` rows name.
mod failing;
mod lisp;
mod supplemental;
mod transforming;
mod verbatim;

use std::io::Write;
use std::sync::{atomic::AtomicI32, Arc};
use tsr_ast::span_map::{SpanMap, FEATURE_ALL, KIND_VERBATIM};
use tsr_ast::SpanSegment;
use tsr_contentmapper::{
    CloseProjectParams, InitializeResult, MappedOutput, OpenProjectParams, OpenProjectResult,
    OptionDiagnosticResult, PositionEncoding, SpawnError, Spawner, METHOD_CLOSE_PROJECT,
    METHOD_OPEN_PROJECT,
};
use tsr_ipc::{Context, HandlerError, HandlerResult};
use tsr_json::RawValue;
use tsr_jsstring::JsString;

pub use transforming::{Handler as TransformingHandler, DECLARED_OPTIONS};

/// The mapper commands of the pinned registry.
pub const TRANSFORMING_MAPPER: &str = "compiler-test-mapper";
pub const VERBATIM_MAPPER: &str = "verbatim-mapper";
pub const MODULE_VERBATIM_MAPPER: &str = "module-verbatim-mapper";
pub const DYNAMIC_VERBATIM_MAPPER: &str = "dynamic-verbatim-mapper";
pub const DIAGNOSTIC_CODE_COLLISION_MAPPER: &str = "diagnostic-code-collision-mapper";
pub const FAILING_MAPPER: &str = "failing-mapper";
pub const SYNTHESIZING_MAPPER: &str = "synthesizing-mapper";
pub const COMPONENT_MAPPER: &str = "component-mapper";
pub const DUPLICATE_MAPPER: &str = "duplicate-mapper";
pub const LISP_MAPPER: &str = "lisp-mapper";
pub const SUPPLEMENTAL_MAPPER: &str = "supplemental-mapper";
pub const SUPPLEMENTAL_DIAGNOSTICS_MAPPER: &str = "supplemental-diagnostics-mapper";
pub const SUPPLEMENTAL_GLOBALS_MAPPER: &str = "supplemental-globals-mapper";
pub const SUPPLEMENTAL_MODULE_MAPPER: &str = "supplemental-module-mapper";
pub const PREFIXED_SUPPLEMENTAL_MAPPER: &str = "prefixed-supplemental-mapper";
pub const UNMAPPED_FOLDING_MAPPER: &str = "unmapped-folding-mapper";
pub const HOISTING_MAPPER: &str = "hoisting-mapper";
pub const DUPLICATE_PROJECTION_MAPPER: &str = "duplicate-projection-mapper";

/// A mapper's own protocol, before the static project protocol wraps it.
pub trait MapperHandler: Send + Sync {
    fn handle_request(&self, ctx: &Context, method: &str, params: &[u8]) -> HandlerResult;
    /// The project lifecycle, for mappers that keep per-project state.
    fn lifecycle(&self) -> Option<&dyn ProjectLifecycleHandler> {
        None
    }
}

/// A mapper that keeps state per mapper project.
pub trait ProjectLifecycleHandler {
    fn open_project(&self, params: &OpenProjectParams) -> Result<(), HandlerError>;
    fn close_project(&self, params: &CloseProjectParams);
}

/// Records the dynamic mapper's real project protocol calls.
/// Source type: tsc/internal/testutil/contentmappertest/dynamic_verbatim.go:ProjectLifecycle
#[derive(Default)]
pub struct ProjectLifecycle {
    pub opens: AtomicI32,
    pub closes: AtomicI32,
}

/// Answers openProject and closeProject for a mapper without dynamic
/// configuration, and hands every other request to the mapper.
struct StaticProjectHandler(Arc<dyn MapperHandler>);

impl tsr_ipc::Handler for StaticProjectHandler {
    /// port: tsc/internal/testutil/contentmappertest/protocol.go:staticProjectHandler.HandleRequest
    fn handle_request(&self, ctx: &Context, method: &str, params: &[u8]) -> HandlerResult {
        match method {
            METHOD_OPEN_PROJECT => {
                let mut params_value = OpenProjectParams::default();
                tsr_json::unmarshal(params, &mut params_value, tsr_json::Options::default())?;
                if let Some(lifecycle) = self.0.lifecycle() {
                    lifecycle.open_project(&params_value)?;
                }
                let mut diagnostics = Vec::new();
                if params_value.options.as_ref().map(|raw| raw.0.as_slice())
                    == Some(br#"{"plugins":[{"name":1}]}"#.as_slice())
                {
                    diagnostics.push(OptionDiagnosticResult {
                        path: vec![
                            RawValue(br#""plugins""#.to_vec()),
                            RawValue(b"0".to_vec()),
                            RawValue(br#""name""#.to_vec()),
                        ],
                        message_text: "Option 'name' requires a string.".into(),
                        code: 123,
                    });
                }
                Ok(Some(tsr_ipc::Response::json(OpenProjectResult {
                    option_diagnostics: diagnostics,
                    ..OpenProjectResult::default()
                })))
            }
            METHOD_CLOSE_PROJECT => {
                let mut params_value = CloseProjectParams::default();
                tsr_json::unmarshal(params, &mut params_value, tsr_json::Options::default())?;
                if let Some(lifecycle) = self.0.lifecycle() {
                    lifecycle.close_project(&params_value);
                }
                Ok(None)
            }
            _ => self.0.handle_request(ctx, method, params),
        }
    }

    /// port: tsc/internal/testutil/contentmappertest/protocol.go:noNotifications.HandleNotification
    fn handle_notification(&self, _: &Context, _: &str, _: &[u8]) -> Result<(), HandlerError> {
        Ok(())
    }
}

/// port: tsc/internal/testutil/contentmappertest/protocol.go:initializeResult
fn initialize_result(source: &str) -> InitializeResult {
    InitializeResult {
        position_encoding: PositionEncoding::utf8(),
        diagnostic_source: source.into(),
    }
}

/// One verbatim segment over the whole content.
/// port: tsc/internal/testutil/contentmappertest/protocol.go:identityMappedOutput
fn identity_mapped_output(content: &str) -> Result<MappedOutput, HandlerError> {
    let length = i32::try_from(content.len())?;
    let mappings = SpanMap::new(&[SpanSegment {
        virtual_start: 0,
        virtual_end: length,
        original_start: 0,
        original_end: length,
        kind: KIND_VERBATIM,
        features: FEATURE_ALL,
    }])
    .marshal()?;
    Ok(MappedOutput {
        text: content.into(),
        extension: ".ts".into(),
        mappings: Some(RawValue(mappings)),
        diagnostic_directives: None,
    })
}

/// The pinned `%q` of a mapper's unexpected method.
fn unexpected_method(method: &str) -> HandlerError {
    format!(
        "contentmappertest: unexpected method {}",
        tsr_jsstring::go_quote(method.as_bytes())
    )
    .into()
}

/// The handler a mapper command names. Only the mappers the corpus rows
/// run are ported; the pin's other mappers are later work and refused.
/// port: tsc/internal/testutil/contentmappertest/registry.go:handlerForMapper
fn handler_for_mapper(
    command: &[JsString],
    lifecycle: Option<Arc<ProjectLifecycle>>,
) -> Result<Arc<dyn MapperHandler>, SpawnError> {
    let Some(name) = command.first() else {
        return Err("contentmappertest: empty mapper command".into());
    };
    let handler: Arc<dyn MapperHandler> = match name.as_bytes() {
        name if name == TRANSFORMING_MAPPER.as_bytes() => Arc::new(TransformingHandler::default()),
        name if name == VERBATIM_MAPPER.as_bytes() => {
            Arc::new(verbatim::Verbatim { module: false })
        }
        name if name == MODULE_VERBATIM_MAPPER.as_bytes() => {
            Arc::new(verbatim::Verbatim { module: true })
        }
        name if name == DYNAMIC_VERBATIM_MAPPER.as_bytes() => {
            Arc::new(verbatim::DynamicVerbatim { lifecycle })
        }
        name if name == SYNTHESIZING_MAPPER.as_bytes() => Arc::new(verbatim::Synthesizing),
        name if name == FAILING_MAPPER.as_bytes() => Arc::new(failing::FailingHandler),
        name if name == LISP_MAPPER.as_bytes() => Arc::new(lisp::LispHandler),
        name if name == SUPPLEMENTAL_MAPPER.as_bytes() => {
            Arc::new(supplemental::SupplementalHandler)
        }
        name if name == SUPPLEMENTAL_DIAGNOSTICS_MAPPER.as_bytes() => {
            Arc::new(supplemental::SupplementalDiagnosticsHandler)
        }
        name if name == SUPPLEMENTAL_GLOBALS_MAPPER.as_bytes() => {
            Arc::new(supplemental::SupplementalGlobalsHandler)
        }
        name if name == SUPPLEMENTAL_MODULE_MAPPER.as_bytes() => {
            Arc::new(supplemental::SupplementalModuleHandler)
        }
        _ => {
            let words: Vec<String> = command
                .iter()
                .map(|word| String::from_utf8_lossy(word.as_bytes()).into_owned())
                .collect();
            return Err(format!(
                "contentmappertest: unknown mapper command [{}]",
                words.join(" ")
            )
            .into());
        }
    };
    Ok(handler)
}

/// Serves the pinned test mappers in process.
#[derive(Clone, Copy, Debug, Default)]
pub struct InProcessSpawner;

/// port: tsc/internal/testutil/contentmappertest/spawner.go:NewSpawner
pub fn new_spawner() -> Arc<dyn Spawner> {
    Arc::new(InProcessSpawner)
}

/// port: tsc/internal/testutil/contentmappertest/spawner.go:NewSpawnerWithProjectLifecycle
pub fn new_spawner_with_project_lifecycle(lifecycle: Arc<ProjectLifecycle>) -> Arc<dyn Spawner> {
    Arc::new(LifecycleSpawner(lifecycle))
}

struct LifecycleSpawner(Arc<ProjectLifecycle>);
impl Spawner for LifecycleSpawner {
    fn spawn(
        &self,
        command: &[JsString],
        _: &[u8],
        _: Box<dyn Write + Send>,
    ) -> Result<tsr_ipc::Stream, SpawnError> {
        spawn_mapper(command, Some(self.0.clone()))
    }
}

impl Spawner for InProcessSpawner {
    /// A pipe whose far end a connection serves with the command's mapper.
    /// port: tsc/internal/testutil/contentmappertest/spawner.go:spawner.Spawn
    fn spawn(
        &self,
        command: &[JsString],
        _dir: &[u8],
        _stderr: Box<dyn Write + Send>,
    ) -> Result<tsr_ipc::Stream, SpawnError> {
        spawn_mapper(command, None)
    }
}

fn spawn_mapper(
    command: &[JsString],
    lifecycle: Option<Arc<ProjectLifecycle>>,
) -> Result<tsr_ipc::Stream, SpawnError> {
    let handler = handler_for_mapper(command, lifecycle)?;
    let (client, server) = tsr_ipc::pipe();
    let handler: Arc<dyn tsr_ipc::Handler> =
        if command[0].as_bytes() == DYNAMIC_VERBATIM_MAPPER.as_bytes() {
            Arc::new(DynamicProjectHandler(handler))
        } else {
            Arc::new(StaticProjectHandler(handler))
        };
    let conn = tsr_ipc::AsyncConn::new(server, handler);
    std::thread::spawn(move || {
        let _ = tsr_ipc::Conn::run(&conn, &Context::background());
    });
    Ok(client)
}

/// Drives the transforming mapper over `stream` until it closes or `ctx` ends.
/// port: tsc/internal/testutil/contentmappertest/spawner.go:Serve
pub fn serve(ctx: &Context, stream: tsr_ipc::Stream) -> Result<(), tsr_ipc::Error> {
    let conn = tsr_ipc::AsyncConn::new(
        stream,
        Arc::new(StaticProjectHandler(Arc::new(
            TransformingHandler::default(),
        ))),
    );
    tsr_ipc::Conn::run(&conn, ctx)
}

#[cfg(test)]
mod tests;

/// Dynamic mappers implement their own project protocol, as in the native spawner.
struct DynamicProjectHandler(Arc<dyn MapperHandler>);
impl tsr_ipc::Handler for DynamicProjectHandler {
    fn handle_request(&self, ctx: &Context, method: &str, params: &[u8]) -> HandlerResult {
        self.0.handle_request(ctx, method, params)
    }
    fn handle_notification(&self, _: &Context, _: &str, _: &[u8]) -> Result<(), HandlerError> {
        Ok(())
    }
}
