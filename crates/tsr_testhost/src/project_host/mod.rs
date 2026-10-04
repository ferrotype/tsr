//! Version 3 private endpoint: S11 host transport plus production projects.
//! The router never executes compiler work. Its single ordered worker performs
//! project updates while the connection pumps callback replies and cancellation.
mod state;
#[cfg(test)]
mod tests;
mod worker;

use crate::{
    bridge::{BridgeError, CallbackRouter, CancelCalls},
    framing,
    protocol::{self, Id},
    wire::{self, raw, wire, Json},
    Session,
};
use serde::Deserialize;
use serde_json::{value::RawValue, Value};
use std::{
    collections::BTreeMap,
    io,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc,
    },
    thread,
};
use tsr_project::session::SessionOptions;
use tsr_vfs::FileSystem;
use worker::{Action, Worker};

pub enum Input {
    Message(Vec<u8>),
    End(io::Result<()>),
}
enum Event {
    Input(Input),
    Completed(u64, Result<Json, String>),
}
struct Task {
    sequence: u64,
    action: Action,
    host: Arc<dyn FileSystem>,
    cancel: Arc<CancelCalls>,
    started: Arc<AtomicBool>,
}
enum Completion {
    Request(Id),
    Notification,
    Options(crate::OptionsToken),
    Reset(Id),
    Protocol {
        id: Option<Id>,
        exit: bool,
        initialized: bool,
    },
}
struct Pending {
    id: Option<Id>,
    cancel: Arc<CancelCalls>,
    completion: Completion,
    context: Option<tsr_ipc::Context>,
    started: Arc<AtomicBool>,
}

/// Send incoming frames here; the reader can block independently of the router.
#[derive(Clone)]
pub struct InputSender(mpsc::Sender<Event>);
impl InputSender {
    pub fn send(&self, input: Input) -> io::Result<()> {
        self.0
            .send(Event::Input(input))
            .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "project router stopped"))
    }
}

/// A connection owns one serialized test session. The injected parse cache is
/// deliberately retained by its worker across reset; no snapshot is retained.
pub struct Connection {
    session: Session,
    bridge: Option<CallbackRouter>,
    output: mpsc::Sender<Json>,
    events: mpsc::Receiver<Event>,
    tasks: Option<mpsc::SyncSender<Task>>,
    worker: Option<thread::JoinHandle<()>>,
    pending: BTreeMap<u64, Pending>,
    sequence: u64,
    initialized: bool,
    resetting: bool,
    shutdown: bool,
    closed: bool,
    lsp_client: Arc<tsr_lsp::rpc_client::RpcClient>,
    context: tsr_ipc::Context,
    lsp_started: bool,
}
impl Connection {
    pub fn new(output: mpsc::Sender<Json>) -> (Self, InputSender) {
        let (sender, events) = mpsc::channel();
        let input = InputSender(sender.clone());
        let (tasks, receive) = mpsc::sync_channel::<Task>(65);
        let context = tsr_ipc::Context::background().with_cancel();
        let output_clone = output.clone();
        let lsp_client = tsr_lsp::rpc_client::RpcClient::new(
            context.clone(),
            Arc::new(move |value| {
                let value = serde_json::value::RawValue::from_string(
                    String::from_utf8(value.0).map_err(|e| tsr_lsproto::ResponseError {
                        code: -32603,
                        message: e.to_string(),
                        data: None,
                    })?,
                )
                .map_err(|e| tsr_lsproto::ResponseError {
                    code: -32603,
                    message: e.to_string(),
                    data: None,
                })?;
                if !protocol::fits(&value) {
                    return Err(tsr_lsproto::ResponseError {
                        code: -32001,
                        message: "LSP response exceeds frame limit".into(),
                        data: None,
                    });
                }
                output_clone
                    .send(value)
                    .map_err(|e| tsr_lsproto::ResponseError {
                        code: -32603,
                        message: e.to_string(),
                        data: None,
                    })
            }),
        );
        let worker_client = lsp_client.clone();
        let worker_context = context.clone();
        let worker = thread::spawn(move || {
            let mut worker = Worker::with_client(worker_client, worker_context);
            while let Ok(task) = receive.recv() {
                task.started.store(true, Ordering::Release);
                let resetting = matches!(task.action, Action::Reset);
                let outcome = if task.cancel.is_canceled() && !resetting {
                    Err("request canceled".into())
                } else {
                    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        worker.run(task.action, task.host)
                    }))
                    .unwrap_or_else(|_| {
                        worker.poison();
                        Err("project operation panicked; reset the test session".into())
                    })
                };
                if sender
                    .send(Event::Completed(task.sequence, outcome))
                    .is_err()
                {
                    break;
                }
            }
        });
        (
            Self {
                session: Session::default(),
                bridge: None,
                output,
                events,
                tasks: Some(tasks),
                worker: Some(worker),
                pending: BTreeMap::new(),
                sequence: 0,
                initialized: false,
                resetting: false,
                shutdown: false,
                closed: false,
                lsp_client,
                context,
                lsp_started: false,
            },
            input,
        )
    }
    pub fn run(mut self) -> io::Result<()> {
        loop {
            match self.events.recv().map_err(protocol::invalid)? {
                Event::Input(Input::Message(bytes)) => self.receive(&bytes)?,
                Event::Input(Input::End(result)) => {
                    result?;
                    return Ok(());
                }
                Event::Completed(sequence, outcome) => self.complete(sequence, outcome)?,
            }
            if self.closed {
                return Ok(());
            }
        }
    }
    fn send(&self, message: Json) -> io::Result<()> {
        if !protocol::fits(&message) {
            return Err(protocol::invalid("project response exceeds frame limit"));
        }
        self.output.send(message).map_err(protocol::invalid)
    }
    fn receive(&mut self, bytes: &[u8]) -> io::Result<()> {
        let message = framing::parse_json(bytes)?;
        let envelope: BTreeMap<String, &RawValue> =
            serde_json::from_str(message.as_str()).map_err(protocol::invalid)?;
        let method: Option<String> = envelope
            .get("method")
            .map(|v| serde_json::from_str(v.get()))
            .transpose()
            .map_err(protocol::invalid)?;
        let id: Option<Id> = envelope
            .get("id")
            .map(|v| serde_json::from_str(v.get()))
            .transpose()
            .map_err(protocol::invalid)?;
        let empty = wire!({});
        let params = envelope.get("params").copied().unwrap_or(&empty);
        if method.is_some() {
            protocol::fields(&envelope, &["jsonrpc", "id", "method", "params"])?;
            if envelope.get("jsonrpc").is_none_or(|v| {
                serde_json::from_str::<String>(v.get()).ok().as_deref() != Some("2.0")
            }) {
                return Err(protocol::invalid("expected jsonrpc version 2.0"));
            }
            if let Some(id) = &id {
                if self.pending.values().any(|p| p.id.as_ref() == Some(id))
                    || self.session.request_pending(id)
                {
                    return Err(protocol::invalid("duplicate in-flight client request ID"));
                }
                if !protocol::fits(&protocol::failure(id, -32001, "", None)) {
                    return Err(protocol::invalid("oversized request ID"));
                }
            }
        }
        let method = method.as_deref();
        if method.is_none() {
            if matches!(&id, Some(Id::String(value)) if value.starts_with("ts")) {
                let mut response = tsr_lsproto::Message::default();
                tsr_json::unmarshal(bytes, &mut response, tsr_json::Options::default())
                    .map_err(protocol::invalid)?;
                self.lsp_client.receive(&response);
                return Ok(());
            }
            // Validate the response with the existing S11 decoder, even when it
            // belongs to a project worker or is a late, retired response.
            let owned = matches!(&id,Some(Id::String(id)) if self.session.has_callback(id));
            if owned {
                for response in self.session.receive(&message)? {
                    self.send(response)?;
                }
            } else {
                let reply = protocol::callback_response(&envelope)?;
                if let Some(bridge) = &self.bridge {
                    let result = if let Some(value) = reply.result {
                        serde_json::from_str::<Value>(value.get())
                            .map_err(|_| BridgeError::InvalidReply)
                    } else {
                        Err(BridgeError::InvalidReply)
                    };
                    bridge.complete(&reply.id, result);
                }
            }
            return Ok(());
        }
        if method == Some("$/cancelRequest") && id.is_none() {
            #[derive(Deserialize)]
            #[serde(deny_unknown_fields)]
            struct Cancel {
                id: Id,
            }
            let cancel: Cancel = serde_json::from_str(params.get()).map_err(protocol::invalid)?;
            for pending in self
                .pending
                .values()
                .filter(|p| p.id.as_ref() == Some(&cancel.id))
            {
                // LSP cancellation starts at dispatch. Private test/ requests
                // keep S11's existing admission-time cancellation contract.
                if pending.context.is_some() && !pending.started.load(Ordering::Acquire) {
                    continue;
                }
                pending.cancel.cancel();
                if let Some(context) = &pending.context {
                    context.cancel();
                }
            }
            for response in self.session.receive(&message)? {
                self.send(response)?;
            }
            return Ok(());
        }
        if method == Some("test/callbackProgress") && id.is_none() {
            #[derive(Deserialize)]
            #[serde(deny_unknown_fields)]
            struct Progress {
                callback: String,
                value: Json,
            }
            let progress: Progress =
                serde_json::from_str(params.get()).map_err(protocol::invalid)?;
            if let Some(pending) = self
                .pending
                .values()
                .find(|p| p.cancel.owns(&progress.callback))
            {
                let response = protocol::notify(
                    "testhost/progress",
                    &wire!({"id":pending.id,"callback":progress.callback,"phase":"report","value":progress.value}),
                );
                if protocol::fits(&response) {
                    return self.send(response);
                }
                if let Some(bridge) = &self.bridge {
                    bridge.complete(&progress.callback, Err(BridgeError::InvalidReply));
                }
                return Ok(());
            }
            if !self.session.has_callback(&progress.callback) {
                return Ok(());
            }
        }
        if self.resetting {
            if let Some(id) = id {
                self.send(protocol::failure(&id, -32002, "reset is pending", None))?;
            }
            return Ok(());
        }
        if method == Some("test/reset") || method == Some("test/shutdown") {
            let Some(id) = id else {
                return Err(protocol::invalid("reset requires a request ID"));
            };
            if protocol::empty(params).is_err() {
                return self.send(protocol::failure(
                    &id,
                    -32602,
                    "expected empty reset parameters",
                    None,
                ));
            }
            self.shutdown = method == Some("test/shutdown");
            for frame in self.session.cancel_host_work() {
                self.send(frame)?;
            }
            let Some(bridge) = &self.bridge else {
                self.session.reset();
                self.initialized = false;
                self.closed = self.shutdown;
                return self.send(protocol::success(&id, &wire!({"reset":true})));
            };
            for pending in self.pending.values() {
                pending.cancel.cancel();
                if let Some(context) = &pending.context {
                    context.cancel();
                }
            }
            bridge.retire();
            let (fs, cancel) = bridge.filesystem();
            self.resetting = true;
            return self.enqueue(
                Action::Reset,
                Arc::new(fs),
                Arc::new(cancel),
                Some(id.clone()),
                Completion::Reset(id),
            );
        }
        if method == Some("test/initialize") {
            let Some(id) = id else {
                return Err(protocol::invalid("initialize requires a request ID"));
            };
            if self.initialized || self.session.has_pending() || !self.pending.is_empty() {
                return self.send(protocol::failure(
                    &id,
                    -32002,
                    "initialization already started or complete",
                    None,
                ));
            }
            let mut fields = wire::fields(params).map_err(protocol::invalid)?;
            let project = fields.remove("project").ok_or_else(|| {
                protocol::invalid("version 3 initialization requires project options")
            })?;
            let (options, progress_delay) = match ProjectOptions::parse(project) {
                Ok(value) => value,
                Err(message) => return self.send(protocol::failure(&id, -32602, &message, None)),
            };
            if fields.get("version").is_none_or(|v| v.get() != "3") {
                return self.send(protocol::failure(
                    &id,
                    -32602,
                    "expected private test-host version 3",
                    None,
                ));
            }
            let two = raw(&2);
            fields.insert("version".into(), &two);
            let rewritten =
                wire!({"jsonrpc":"2.0","id":id,"method":"test/initialize","params":fields});
            let parsed = framing::parse_json(rewritten.get().as_bytes())?;
            for response in self.session.receive(&parsed)? {
                self.send(response)?;
            }
            if let Some(update) = self.session.pending_options() {
                let compiler = match compiler_options(update.options) {
                    Ok(value) => value,
                    Err(error) => {
                        let token = update.token;
                        for response in self.session.complete_options(token, Err(error))? {
                            self.send(response)?;
                        }
                        return Ok(());
                    }
                };
                let token = update.token;
                self.bridge = Some(
                    self.session
                        .options_callback_router(token, self.output.clone())?,
                );
                let bridge = self.bridge.as_ref().unwrap();
                let (base, _) = bridge.filesystem();
                let (fs, cancel) = bridge.filesystem();
                return self.enqueue(
                    Action::Initialize {
                        options,
                        progress_delay,
                        compiler,
                        host: Arc::new(base),
                    },
                    Arc::new(fs),
                    Arc::new(cancel),
                    Some(id),
                    Completion::Options(token),
                );
            }
            return Ok(());
        }
        if method == Some("test/setOptions") {
            if !self.pending.is_empty() {
                if let Some(id) = id {
                    self.send(protocol::failure(
                        &id,
                        -32002,
                        "project work is pending",
                        None,
                    ))?;
                }
                return Ok(());
            }
            for response in self.session.receive(&message)? {
                self.send(response)?;
            }
            if let Some(update) = self.session.pending_options() {
                let token = update.token;
                let compiler = match compiler_options(update.options) {
                    Ok(value) => value,
                    Err(error) => {
                        for response in self.session.complete_options(token, Err(error))? {
                            self.send(response)?;
                        }
                        return Ok(());
                    }
                };
                let (fs, cancel) = self
                    .bridge
                    .as_ref()
                    .ok_or_else(|| protocol::invalid("missing project callbacks"))?
                    .filesystem();
                return self.enqueue(
                    Action::Options(compiler),
                    Arc::new(fs),
                    Arc::new(cancel),
                    id,
                    Completion::Options(token),
                );
            }
            return Ok(());
        }
        if method.is_some_and(|method| {
            method == "initialize"
                || self.lsp_started
                    && !method.starts_with("test/")
                    && !method.starts_with("testhost/")
        }) {
            if !self.initialized {
                return Err(protocol::invalid(
                    "LSP requires the private initialization barrier",
                ));
            }
            let mut request = tsr_lsproto::Message::default();
            tsr_json::unmarshal(bytes, &mut request, tsr_json::Options::default())
                .map_err(protocol::invalid)?;
            if request.method == "initialize" {
                self.lsp_started = true;
            }
            let context = self.context.with_cancel();
            let (fs, cancel) = self.bridge.as_ref().unwrap().filesystem();
            let completion = Completion::Protocol {
                id: id.clone(),
                exit: request.method == "exit",
                initialized: request.method == "initialized",
            };
            return self.enqueue(
                Action::Protocol {
                    message: request,
                    context,
                },
                Arc::new(fs),
                Arc::new(cancel),
                id,
                completion,
            );
        }
        if method == Some("test/projectState")
            || method.is_some_and(|m| {
                m.starts_with("textDocument/") || m == "workspace/didChangeWatchedFiles"
            })
        {
            if !self.initialized || self.session.pending_options().is_some() {
                if let Some(id) = id {
                    self.send(protocol::failure(
                        &id,
                        -32002,
                        "project session is not ready",
                        None,
                    ))?;
                } else {
                    return Err(protocol::invalid(
                        "document action before initialization barrier",
                    ));
                }
                return Ok(());
            }
            let action = if method == Some("test/projectState") {
                if id.is_none() || protocol::empty(params).is_err() {
                    return Err(protocol::invalid(
                        "projectState requires a request with empty params",
                    ));
                }
                Action::State
            } else {
                if let Some(id) = id {
                    return self.send(protocol::failure(
                        &id,
                        -32600,
                        "document actions are notifications",
                        None,
                    ));
                }
                Action::Notification {
                    method: method.unwrap().into(),
                    params: Some(tsr_json::RawValue(params.get().as_bytes().to_vec())),
                }
            };
            let completion = id
                .clone()
                .map_or(Completion::Notification, Completion::Request);
            let (fs, cancel) = self.bridge.as_ref().unwrap().filesystem();
            return self.enqueue(action, Arc::new(fs), Arc::new(cancel), id, completion);
        }
        // S11's plugin streams and host methods continue on this connection.
        for response in self.session.receive(&message)? {
            self.send(response)?;
        }
        Ok(())
    }
    fn enqueue(
        &mut self,
        action: Action,
        host: Arc<dyn FileSystem>,
        cancel: Arc<CancelCalls>,
        id: Option<Id>,
        completion: Completion,
    ) -> io::Result<()> {
        if self.pending.len() >= 64 && !matches!(action, Action::Reset) {
            return Err(protocol::invalid("too many project operations"));
        }
        self.sequence = self
            .sequence
            .checked_add(1)
            .ok_or_else(|| protocol::invalid("project request identity exhausted"))?;
        let context = match &action {
            Action::Protocol { context, .. } => Some(context.clone()),
            _ => None,
        };
        let started = Arc::new(AtomicBool::new(false));
        self.tasks
            .as_ref()
            .unwrap()
            .try_send(Task {
                sequence: self.sequence,
                action,
                host,
                cancel: cancel.clone(),
                started: started.clone(),
            })
            .map_err(protocol::invalid)?;
        self.pending.insert(
            self.sequence,
            Pending {
                id,
                cancel,
                completion,
                context,
                started,
            },
        );
        Ok(())
    }
    fn complete(&mut self, sequence: u64, outcome: Result<Json, String>) -> io::Result<()> {
        let pending = self
            .pending
            .remove(&sequence)
            .ok_or_else(|| protocol::invalid("unknown project completion"))?;
        let resetting = matches!(pending.completion, Completion::Reset(_));
        if let Some(context) = &pending.context {
            context.cancel();
        }
        match pending.completion {
            Completion::Protocol {
                id,
                exit,
                initialized,
            } => {
                if exit && outcome.as_ref().is_ok_and(|value| value.get() == "null") {
                    self.closed = true;
                    return Ok(());
                }
                if let Some(id) = id {
                    let response = match outcome {
                        Ok(response) => response,
                        Err(error) => protocol::failure(
                            &id,
                            if pending.cancel.is_canceled() {
                                -32800
                            } else {
                                -32603
                            },
                            if pending.cancel.is_canceled() {
                                "RequestCancelled"
                            } else {
                                &error
                            },
                            None,
                        ),
                    };
                    self.send(response)?;
                } else if initialized && outcome.is_ok() {
                    // Publish InitComplete only after retiring this action,
                    // so an immediate test/setOptions sees an idle router.
                    self.send(
                        wire!({"jsonrpc":"2.0","method":"testhost/lspInitialized","params":{}}),
                    )?;
                }
            }
            Completion::Options(token) => {
                let success = outcome.is_ok();
                for message in self.session.complete_options(token, outcome.map(|_| ()))? {
                    let mut fields: BTreeMap<String, Json> =
                        serde_json::from_str(message.get()).map_err(protocol::invalid)?;
                    if fields
                        .get("method")
                        .is_some_and(|v| v.get() == "\"testhost/initialized\"")
                    {
                        fields.insert("params".into(), wire!({"version":3}));
                        self.initialized = true;
                    }
                    if let Some(result) = fields.get_mut("result") {
                        let mut value: BTreeMap<String, Json> =
                            serde_json::from_str(result.get()).map_err(protocol::invalid)?;
                        if value.contains_key("version") {
                            value.insert("version".into(), raw(&3));
                            *result = raw(&value);
                        }
                    }
                    self.send(raw(&fields))?;
                }
                if !success && !self.initialized {
                    self.bridge.take();
                }
            }
            Completion::Request(id) | Completion::Reset(id) => {
                if resetting {
                    if outcome.is_err() {
                        return Err(protocol::invalid(
                            "project reset failed; discard the worker",
                        ));
                    }
                    self.session.reset();
                    self.bridge.take();
                    self.initialized = false;
                    self.lsp_started = false;
                    self.resetting = false;
                    self.closed = self.shutdown;
                }
                let response = match outcome {
                    Ok(value) if protocol::response_fits(&id, &value).is_ok() => {
                        protocol::success(&id, &value)
                    }
                    Ok(_) => {
                        protocol::failure(&id, -32001, "project state exceeds frame limit", None)
                    }
                    Err(message) => protocol::failure(
                        &id,
                        if pending.cancel.is_canceled() {
                            -32800
                        } else {
                            -32603
                        },
                        &message,
                        None,
                    ),
                };
                self.send(response)?;
            }
            Completion::Notification => {
                if let Err(error) = outcome {
                    self.send(protocol::notify(
                        "testhost/failure",
                        &wire!({"message":error}),
                    ))?;
                }
            }
        }
        Ok(())
    }
}
impl Drop for Connection {
    fn drop(&mut self) {
        self.context.cancel();
        if let Some(bridge) = &self.bridge {
            bridge.retire();
        }
        for pending in self.pending.values() {
            pending.cancel.cancel();
        }
        self.tasks.take();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct ProjectOptions {
    current_directory: String,
    default_library_path: String,
    position_encoding: String,
    #[serde(default)]
    run_external_code: bool,
    #[serde(default)]
    progress_delay_nanos: i64,
}
impl ProjectOptions {
    fn parse(raw: &RawValue) -> Result<(SessionOptions, std::time::Duration), String> {
        let value: Self = serde_json::from_str(raw.get()).map_err(|e| e.to_string())?;
        let encoding = match value.position_encoding.as_str() {
            "utf-8" => tsr_jsstring::PositionEncoding::Utf8,
            "utf-16" => tsr_jsstring::PositionEncoding::Utf16,
            _ => return Err("unsupported project position encoding".into()),
        };
        Ok((
            SessionOptions {
                current_directory: tsr_jsstring::JsString::from_bytes(
                    value.current_directory.as_bytes(),
                ),
                default_library_path: tsr_jsstring::JsString::from_bytes(
                    value.default_library_path.as_bytes(),
                ),
                position_encoding: encoding,
                run_external_code: value.run_external_code,
                ..Default::default()
            },
            std::time::Duration::from_nanos(value.progress_delay_nanos.max(0) as u64),
        ))
    }
}
fn compiler_options(value: &RawValue) -> Result<tsr_core::CompilerOptions, String> {
    tsr_tsoptions::raw::compiler_options(
        &serde_json::from_str(value.get()).map_err(|e| format!("{e}"))?,
    )
    .map_err(|e| format!("invalid inferred compiler options: {e:?}"))
}
