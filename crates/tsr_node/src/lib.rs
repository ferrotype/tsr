//! Node-API leaf: byte buffers in, owned protocol-8 buffer out.
use napi::bindgen_prelude::Buffer;
use napi_derive::napi;
use std::sync::mpsc::{self, Sender};
use std::thread::JoinHandle;
use ts_ast::SourceFileParseOptions;
use ts_core::ScriptKind;
use ts_jsstring::{JsString, SourceText};

struct Input {
    source: SourceText,
    kind: ScriptKind,
    options: SourceFileParseOptions,
}

impl Input {
    fn new(
        source: &Buffer,
        name: &Buffer,
        kind: i32,
        jsx: bool,
        force: bool,
    ) -> napi::Result<Self> {
        if source.len() > i32::MAX as usize || name.len() > i32::MAX as usize {
            return Err(napi::Error::from_reason(
                "input exceeds signed source positions",
            ));
        }
        Ok(Self {
            source: SourceText::from_loaded_bytes(source.as_ref()),
            kind: ScriptKind(kind),
            options: SourceFileParseOptions {
                file_name: JsString::from_bytes(name.as_ref()),
                path: JsString::from_bytes(name.as_ref()),
                external_module_indicator_options: ts_ast::ExternalModuleIndicatorOptions {
                    jsx,
                    force,
                },
            },
        })
    }

    fn execute(self) -> Result<Vec<u8>, String> {
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            ts_embed::parse_and_encode(self.source, self.kind, self.options)
        }))
        .map_err(|_| "parser panicked; request storage discarded".to_owned())?
        .map_err(|error| error.to_string())
    }
}

#[napi(js_name = "parseAndEncode")]
pub fn parse_and_encode(
    source: Buffer,
    name: Buffer,
    kind: i32,
    jsx: Option<bool>,
    force: Option<bool>,
) -> napi::Result<Buffer> {
    Input::new(
        &source,
        &name,
        kind,
        jsx.unwrap_or(false),
        force.unwrap_or(false),
    )?
    .execute()
    .map(Buffer::from)
    .map_err(napi::Error::from_reason)
}

struct Request {
    input: Input,
    reply: Sender<Response>,
    #[cfg(feature = "worker-probe")]
    measure: bool,
}

struct Response {
    result: Result<Vec<u8>, String>,
    #[cfg(feature = "worker-probe")]
    work: Option<(std::time::Instant, std::time::Instant)>,
}

#[cfg(feature = "worker-probe")]
#[napi(object)]
pub struct WorkerMeasurement {
    pub output: Buffer,
    pub prepare_ns: f64,
    pub dispatch_ns: f64,
    pub work_ns: f64,
    pub return_ns: f64,
}

/// A persistent reserved-stack parser worker. Calls remain synchronous from JS;
/// each request owns its source and AST, and returns only an owned byte buffer.
/// `close()` joins the worker; Node finalization also closes it if not explicit.
#[napi]
pub struct Parser {
    sender: Option<Sender<Request>>,
    worker: Option<JoinHandle<()>>,
}

#[napi]
impl Parser {
    #[napi(constructor)]
    pub fn new() -> napi::Result<Self> {
        let (sender, receiver) = mpsc::channel::<Request>();
        let worker = std::thread::Builder::new()
            .name("ts-node-parser-owner".into())
            .spawn(move || {
                // The outer owner joins the scoped production parser worker.
                // Its reserved stack and nested-parser contract stay unchanged.
                ts_parser::on_parser_worker(move || {
                    while let Ok(request) = receiver.recv() {
                        #[cfg(feature = "worker-probe")]
                        let started = request.measure.then(std::time::Instant::now);
                        let result = request.input.execute();
                        #[cfg(feature = "worker-probe")]
                        let work = started.map(|start| (start, std::time::Instant::now()));
                        let _ = request.reply.send(Response {
                            result,
                            #[cfg(feature = "worker-probe")]
                            work,
                        });
                    }
                });
            })
            .map_err(|error| napi::Error::from_reason(error.to_string()))?;
        Ok(Self {
            sender: Some(sender),
            worker: Some(worker),
        })
    }

    #[napi(js_name = "parseAndEncode")]
    pub fn parse_and_encode(
        &self,
        source: Buffer,
        name: Buffer,
        kind: i32,
        jsx: Option<bool>,
        force: Option<bool>,
    ) -> napi::Result<Buffer> {
        let sender = self
            .sender
            .as_ref()
            .ok_or_else(|| napi::Error::from_reason("parser is closed"))?;
        let (reply, receiver) = mpsc::channel();
        sender
            .send(Request {
                input: Input::new(
                    &source,
                    &name,
                    kind,
                    jsx.unwrap_or(false),
                    force.unwrap_or(false),
                )?,
                reply,
                #[cfg(feature = "worker-probe")]
                measure: false,
            })
            .map_err(|_| napi::Error::from_reason("parser worker exited"))?;
        receiver
            .recv()
            .map_err(|_| napi::Error::from_reason("parser worker exited"))?
            .result
            .map(Buffer::from)
            .map_err(napi::Error::from_reason)
    }

    #[napi]
    pub fn close(&mut self) {
        self.sender.take();
        if let Some(worker) = self.worker.take() {
            // No borrowed input or AST survives a callback; joining disposes
            // the worker even after it unwound. Drop must not itself panic.
            let _ = worker.join();
        }
    }
}

#[cfg(feature = "worker-probe")]
#[napi]
impl Parser {
    /// Attribute the existing request path without bypassing its reserved stack.
    /// This instrumented feature is a diagnostic, never E8 acceptance evidence.
    ///
    /// # Panics
    /// Panics if the worker violates the instrumented response protocol.
    #[napi(js_name = "measureParseAndEncode")]
    pub fn measure_parse_and_encode(
        &self,
        source: Buffer,
        name: Buffer,
        kind: i32,
    ) -> napi::Result<WorkerMeasurement> {
        use std::time::Instant;
        let start = Instant::now();
        let sender = self
            .sender
            .as_ref()
            .ok_or_else(|| napi::Error::from_reason("parser is closed"))?;
        let input = Input::new(&source, &name, kind, false, false)?;
        let prepared = Instant::now();
        let (reply, receiver) = mpsc::channel();
        sender
            .send(Request {
                input,
                reply,
                measure: true,
            })
            .map_err(|_| napi::Error::from_reason("parser worker exited"))?;
        let response = receiver
            .recv()
            .map_err(|_| napi::Error::from_reason("parser worker exited"))?;
        let completed = Instant::now();
        let (work_start, work_end) = response
            .work
            .expect("measured request has worker timestamps");
        let ns = |duration: std::time::Duration| duration.as_secs_f64() * 1e9;
        Ok(WorkerMeasurement {
            output: response
                .result
                .map(Buffer::from)
                .map_err(napi::Error::from_reason)?,
            prepare_ns: ns(prepared.duration_since(start)),
            dispatch_ns: ns(work_start.duration_since(prepared)),
            work_ns: ns(work_end.duration_since(work_start)),
            return_ns: ns(completed.duration_since(work_end)),
        })
    }
}

impl Drop for Parser {
    fn drop(&mut self) {
        self.close();
    }
}
