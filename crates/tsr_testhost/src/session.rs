use std::{
    collections::BTreeMap,
    io,
    sync::atomic::{AtomicU64, Ordering},
};

use serde::Deserialize;
use serde_json::value::RawValue;

use crate::{
    configuration::{Configuration, Initialize, PluginState},
    filesystem::OPERATIONS,
    protocol::{
        decode, empty, failure, fields, fits, invalid, notify, progress, response_fits, success,
        Id, RequestResult,
    },
    streams::Streams,
    wire::{self, raw, wire, Json},
};

const MAX_PENDING: usize = 64;

/// Process-unique internal completion identity; never sent over the wire.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct OptionsToken(u64);
impl OptionsToken {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let value = NEXT
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
                value.checked_add(1)
            })
            .unwrap_or_else(|_| std::process::abort());
        Self(value)
    }
}
/// Borrowed hook input. The S11 executable commits immediately; an embedding
/// can keep pumping the Session and later complete the token. Phase 5 supplies
/// the actual project-system update and its atomic commit/cancellation boundary.
pub struct OptionsUpdate<'a> {
    pub token: OptionsToken,
    pub options: &'a RawValue,
    pub cancellation_requested: bool,
}
enum UpdateValue {
    Initialize(Box<Configuration>),
    Options(Json),
}
struct Update {
    token: OptionsToken,
    request: Id,
    value: UpdateValue,
    canceled: bool,
}
enum Action {
    File { operation: String, path: String },
    Spawn { index: usize },
}
struct Pending {
    request: Id,
    action: Action,
    canceled: bool,
}

/// Single connection state. Reverse calls are registered before their frames
/// are returned. The router never waits for host callbacks or stream credit.
#[derive(Default)]
pub struct Session {
    configuration: Option<Configuration>,
    update: Option<Update>,
    pending: BTreeMap<String, Pending>,
    streams: Streams,
    next_callback: u64,
    closed: bool,
}
impl Session {
    pub fn is_closed(&self) -> bool {
        self.closed
    }
    pub fn has_pending(&self) -> bool {
        self.update.is_some() || !self.pending.is_empty()
    }

    pub fn pending_options(&self) -> Option<OptionsUpdate<'_>> {
        self.update.as_ref().map(|update| OptionsUpdate {
            token: update.token,
            options: match &update.value {
                UpdateValue::Initialize(configuration) => &configuration.options,
                UpdateValue::Options(options) => options,
            },
            cancellation_requested: update.canceled,
        })
    }

    /// Publish the hook outcome. Ok means applied, even if cancellation raced;
    /// Err guarantees nothing remains applied. Cancellation is advisory until
    /// this completion, and cannot release the ordering barrier prematurely.
    /// A token from another session, a finished update or shutdown is rejected.
    pub fn complete_options(
        &mut self,
        token: OptionsToken,
        outcome: Result<(), String>,
    ) -> io::Result<Vec<Json>> {
        if self.closed || self.update.as_ref().is_none_or(|u| u.token != token) {
            return Err(invalid("unknown or retired options completion"));
        }
        let update = self.update.take().expect("validated update");
        if let Err(error) = outcome {
            return Ok(vec![failure(
                &update.request,
                if update.canceled { -32800 } else { -32001 },
                &error,
                None,
            )]);
        }
        let (value, initialized) = match update.value {
            UpdateValue::Initialize(configuration) => {
                let value = configuration.wire();
                self.configuration = Some(*configuration);
                (value, true)
            }
            UpdateValue::Options(options) => {
                let value = wire!({"options":options});
                self.configuration
                    .as_mut()
                    .expect("options require initialization")
                    .options = options;
                (value, false)
            }
        };
        let mut messages = vec![success(&update.request, &value)];
        if initialized {
            messages.push(notify("testhost/initialized", &wire!({"version":2})));
        }
        Ok(messages)
    }

    /// Invalid wire input is terminal, including for callers driving Session
    /// directly rather than through serve. Discard the connection on error.
    pub fn receive(&mut self, message: &crate::framing::Message<'_>) -> io::Result<Vec<Json>> {
        let outcome = self.receive_message(message);
        if outcome.is_err() {
            self.closed = true;
            self.pending.clear();
            self.update = None;
            self.configuration = None;
            self.streams = Streams::default();
        }
        outcome
    }

    fn receive_message(&mut self, message: &crate::framing::Message<'_>) -> io::Result<Vec<Json>> {
        if self.closed {
            return Err(invalid("message on closed test-host session"));
        }
        let envelope: BTreeMap<String, &RawValue> =
            serde_json::from_str(message.as_str()).map_err(invalid)?;
        let string = |name: &str| {
            envelope
                .get(name)
                .and_then(|v| serde_json::from_str::<String>(v.get()).ok())
        };
        if string("jsonrpc").as_deref() != Some("2.0") {
            return Err(invalid("expected jsonrpc version 2.0"));
        }
        if envelope.contains_key("method") {
            fields(&envelope, &["jsonrpc", "id", "method", "params"])?;
            let method = string("method").ok_or_else(|| invalid("method must be a string"))?;
            let default_params = wire!({});
            let params = envelope.get("params").copied().unwrap_or(&default_params);
            if let Some(id) = envelope.get("id") {
                let id: Id = serde_json::from_str(id.get()).map_err(invalid)?;
                if self.update.as_ref().is_some_and(|u| u.request == id)
                    || self
                        .pending
                        .values()
                        .any(|p| p.request == id && !p.canceled)
                {
                    return Err(invalid("duplicate in-flight client request ID"));
                }
                if !fits(&failure(&id, -32001, "", None)) {
                    return Err(invalid("request ID leaves no space for a response"));
                }
                Ok(self.request(&id, &method, params))
            } else {
                self.notification(&method, params)
            }
        } else {
            fields(&envelope, &["jsonrpc", "id", "result", "error"])?;
            let id = string("id").ok_or_else(|| invalid("callback response requires string ID"))?;
            if envelope.contains_key("result") == envelope.contains_key("error") {
                return Err(invalid("response requires exactly one result or error"));
            }
            if let Some(error) = envelope.get("error") {
                let error = wire::fields(error).map_err(invalid)?;
                fields(&error, &["code", "message", "data"])?;
                if error
                    .get("code")
                    .and_then(|v| serde_json::from_str::<i32>(v.get()).ok())
                    .is_none()
                    || error
                        .get("message")
                        .and_then(|v| serde_json::from_str::<String>(v.get()).ok())
                        .is_none()
                {
                    return Err(invalid("invalid callback error"));
                }
            }
            self.response(
                &id,
                envelope.get("result").map(|v| (*v).to_owned()),
                envelope.get("error").map(|v| (*v).to_owned()),
            )
        }
    }

    fn request(&mut self, id: &Id, method: &str, params: &RawValue) -> Vec<Json> {
        let result = match method {
            "test/initialize" => self.initialize(id, params),
            "test/shutdown" => self.shutdown(id, params),
            "test/state" | "test/setOptions" | "test/fs" | "test/openPlugin"
            | "test/streamWrite" | "test/streamRead" | "test/streamStatus" | "test/closeStream"
                if self.configuration.is_none() =>
            {
                Err((-32002, "test-host is not initialized".into()))
            }
            "test/fs" | "test/openPlugin" | "test/streamWrite" if self.update.is_some() => {
                Err((-32002, "configuration update is pending".into()))
            }
            "test/state" => empty(params)
                .map(|()| vec![success(id, &self.configuration.as_ref().unwrap().wire())]),
            "test/setOptions" => self.options(id, params),
            "test/fs" => self.file(id, params),
            "test/openPlugin" => self.spawn(id, params),
            "test/streamWrite" | "test/streamRead" | "test/streamStatus" => {
                self.streams.request(id, method, params)
            }
            "test/closeStream" => self.close_stream(id, params),
            _ => Err((-32601, "unknown test-host method".into())),
        };
        result
            .unwrap_or_else(|(code, message)| vec![failure(id, code, &message, None)])
            .into_iter()
            .map(|message| {
                if fits(&message) {
                    message
                } else {
                    failure(id, -32001, "response exceeds frame limit", None)
                }
            })
            .collect()
    }
    fn initialize(&mut self, id: &Id, params: &RawValue) -> RequestResult {
        if self.configuration.is_some() || self.has_pending() {
            return Err((-32002, "initialization already started or complete".into()));
        }
        let configuration =
            Configuration::from_wire(decode::<Initialize>(params)?).map_err(|e| (-32602, e))?;
        response_fits(id, &configuration.wire()).map_err(|e| (-32602, e))?;
        self.update = Some(Update {
            token: OptionsToken::new(),
            request: id.clone(),
            value: UpdateValue::Initialize(Box::new(configuration)),
            canceled: false,
        });
        Ok(vec![])
    }
    fn options(&mut self, id: &Id, params: &RawValue) -> RequestResult {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Options {
            options: Json,
        }
        let options: Options = decode(params)?;
        if !wire::object(&options.options) {
            return Err((-32602, "options must be an object".into()));
        }
        if self.has_pending() {
            return Err((-32002, "options require an idle callback queue".into()));
        }
        let state = self
            .configuration
            .as_ref()
            .unwrap()
            .wire_with_options(&options.options, true);
        // Reserve the longest state spelling. Longer future request IDs may
        // receive a bounded error, but cannot change stored state or kill IO.
        response_fits(&Id::Integer(i32::MIN), &state).map_err(|e| (-32602, e))?;
        response_fits(id, &wire!({"options":options.options})).map_err(|e| (-32602, e))?;
        self.update = Some(Update {
            token: OptionsToken::new(),
            request: id.clone(),
            value: UpdateValue::Options(options.options),
            canceled: false,
        });
        Ok(vec![])
    }
    fn file(&mut self, id: &Id, params: &RawValue) -> RequestResult {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct File {
            operation: String,
            path: String,
        }
        let File { operation, path } = decode(params)?;
        if !OPERATIONS.contains(&operation.as_str()) {
            return Err((-32602, "unknown filesystem operation".into()));
        }
        crate::filesystem::valid_base_path(&path).map_err(|e| (-32602, e))?;
        let host = &self.configuration.as_ref().unwrap().host;
        if host.enabled(&operation) {
            self.start(
                id,
                &operation.clone(),
                &raw(&path),
                Action::File { operation, path },
            )
        } else {
            host.complete(&operation, &path, serde_json::Value::Null)
                .map(|v| vec![success(id, &raw(&v))])
                .map_err(|e| (-32001, e))
        }
    }
    fn spawn(&mut self, id: &Id, params: &RawValue) -> RequestResult {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Spawn {
            name: String,
        }
        let input: Spawn = decode(params)?;
        let reserved = self
            .pending
            .values()
            .filter(|p| !p.canceled && matches!(p.action, Action::Spawn { .. }))
            .count();
        if !self.streams.can_reserve(reserved) {
            return Err((
                -32002,
                "stream identity limit reached; start a new session".into(),
            ));
        }
        let configuration = self.configuration.as_ref().unwrap();
        let index = configuration
            .plugins
            .iter()
            .position(|p| p.registration.name == input.name)
            .ok_or_else(|| (-32602, "unregistered plugin".into()))?;
        let plugin = &configuration.plugins[index];
        if plugin.state != PluginState::Registered {
            return Err((-32002, "plugin already opening, open or retired".into()));
        }
        let params = wire!({"name":input.name,"options":plugin.registration.options});
        let messages = self.start(id, "testhost/spawnPlugin", &params, Action::Spawn { index })?;
        self.configuration.as_mut().unwrap().plugins[index].state = PluginState::Opening;
        Ok(messages)
    }
    fn close_stream(&mut self, id: &Id, params: &RawValue) -> RequestResult {
        let index = self.streams.plugin_for(params)?;
        let messages = self.streams.request(id, "test/closeStream", params)?;
        self.stream_finished(index);
        Ok(messages)
    }
    fn stream_finished(&mut self, index: usize) {
        let plugin = &mut self.configuration.as_mut().unwrap().plugins[index];
        if plugin.state == PluginState::Open && !self.streams.has_active(index) {
            plugin.state = PluginState::Registered;
        }
    }

    fn start(
        &mut self,
        request: &Id,
        method: &str,
        params: &RawValue,
        action: Action,
    ) -> RequestResult {
        if self.pending.len() == MAX_PENDING {
            return Err((-32002, "pending callback limit reached".into()));
        }
        self.next_callback = self
            .next_callback
            .checked_add(1)
            .ok_or_else(|| (-32002, "callback IDs exhausted".into()))?;
        let callback = format!("callback:{}", self.next_callback);
        let message = wire!({"jsonrpc":"2.0","id":callback,"method":method,"params":params});
        let begin = progress(request, &callback, "begin");
        if !fits(&message) || !fits(&begin) {
            return Err((-32602, "outgoing callback exceeds frame limit".into()));
        }
        self.pending.insert(
            callback,
            Pending {
                request: request.clone(),
                action,
                canceled: false,
            },
        );
        Ok(vec![begin, message])
    }
    fn response(
        &mut self,
        callback: &str,
        result: Option<Json>,
        error: Option<Json>,
    ) -> io::Result<Vec<Json>> {
        let pending = self
            .pending
            .remove(callback)
            .ok_or_else(|| invalid("unknown or duplicate callback response"))?;
        if pending.canceled {
            return Ok(vec![]);
        }
        let mut messages = vec![progress(&pending.request, callback, "end")];
        if let Some(error) = error {
            if let Action::Spawn { index } = pending.action {
                self.configuration.as_mut().unwrap().plugins[index].state = PluginState::Registered;
            }
            messages.push(failure(
                &pending.request,
                -32001,
                "callback failed",
                Some(error),
            ));
            return Ok(messages);
        }
        let result = result.expect("validated response");
        match pending.action {
            Action::File { operation, path } => {
                let value = serde_json::from_str(result.get())
                    .map_err(|e| e.to_string())
                    .and_then(|value| {
                        self.configuration
                            .as_ref()
                            .unwrap()
                            .host
                            .complete(&operation, &path, value)
                    })
                    .map(|v| raw(&v));
                match value
                    .and_then(|value| response_fits(&pending.request, &value).map(|()| value))
                {
                    Ok(value) => messages.push(success(&pending.request, &value)),
                    Err(error) => messages.push(failure(&pending.request, -32001, &error, None)),
                }
            }
            Action::Spawn { index } => {
                #[derive(Deserialize)]
                #[serde(deny_unknown_fields)]
                struct Spawned {
                    stream: String,
                }
                let opened = serde_json::from_str::<Spawned>(result.get())
                    .map_err(|e| e.to_string())
                    .and_then(|spawned| {
                        response_fits(&pending.request, &result)?;
                        self.streams.open(&spawned.stream, index)
                    });
                let plugin = &mut self.configuration.as_mut().unwrap().plugins[index];
                match opened {
                    Ok(credits) => {
                        plugin.state = PluginState::Open;
                        messages.push(success(&pending.request, &result));
                        messages.extend(credits);
                    }
                    Err(error) => {
                        plugin.state = PluginState::Retired;
                        messages.push(notify(
                            "testhost/retirePlugin",
                            &wire!({"name":plugin.registration.name}),
                        ));
                        messages.push(failure(&pending.request, -32001, &error, None));
                    }
                }
            }
        }
        Ok(messages)
    }
    fn notification(&mut self, method: &str, params: &RawValue) -> io::Result<Vec<Json>> {
        match method {
            "$/cancelRequest" => {
                #[derive(Deserialize)]
                #[serde(deny_unknown_fields)]
                struct Cancel {
                    id: Id,
                }
                let cancel: Cancel = serde_json::from_str(params.get()).map_err(invalid)?;
                if let Some(update) = self.update.as_mut().filter(|u| u.request == cancel.id) {
                    update.canceled = true;
                    return Ok(vec![]);
                }
                Ok(self.interrupt(&cancel.id, -32800, "request canceled"))
            }
            "test/callbackProgress" => {
                #[derive(Deserialize)]
                #[serde(deny_unknown_fields)]
                struct Report {
                    callback: String,
                    value: Json,
                }
                if !wire::fields(params).map_err(invalid)?.contains_key("value") {
                    return Err(invalid("progress value is required"));
                }
                let report: Report = serde_json::from_str(params.get()).map_err(invalid)?;
                let pending = self
                    .pending
                    .get(&report.callback)
                    .ok_or_else(|| invalid("progress for unknown callback"))?;
                if pending.canceled {
                    return Ok(vec![]);
                }
                let request = pending.request.clone();
                let message = notify(
                    "testhost/progress",
                    &wire!({"id":request,"callback":report.callback,"phase":"report","value":report.value}),
                );
                if fits(&message) {
                    Ok(vec![message])
                } else {
                    Ok(self.interrupt(&request, -32001, "progress exceeds frame limit"))
                }
            }
            "test/streamData" | "test/streamCredit" | "test/streamEnd" | "test/streamExit"
            | "test/streamClose" => {
                if let Some(index) = self.streams.notification(method, params).map_err(invalid)? {
                    self.stream_finished(index);
                }
                Ok(vec![])
            }
            _ => Ok(vec![]),
        }
    }
    fn interrupt(&mut self, request: &Id, code: i64, message: &str) -> Vec<Json> {
        let Some((callback, pending)) = self
            .pending
            .iter_mut()
            .find(|(_, p)| p.request == *request && !p.canceled)
        else {
            return vec![];
        };
        pending.canceled = true;
        let mut messages = vec![notify("$/cancelRequest", &wire!({"id":callback}))];
        if let Action::Spawn { index } = pending.action {
            let plugin = &mut self.configuration.as_mut().unwrap().plugins[index];
            plugin.state = PluginState::Retired;
            messages.push(notify(
                "testhost/retirePlugin",
                &wire!({"name":plugin.registration.name}),
            ));
        }
        messages.push(progress(request, callback, "end"));
        messages.push(failure(request, code, message, None));
        messages
    }
    fn shutdown(&mut self, id: &Id, params: &RawValue) -> RequestResult {
        empty(params)?;
        let requests: Vec<_> = self
            .pending
            .values()
            .filter(|p| !p.canceled)
            .map(|p| p.request.clone())
            .collect();
        let mut messages: Vec<_> = requests
            .iter()
            .flat_map(|request| self.interrupt(request, -32800, "request canceled"))
            .collect();
        if let Some(update) = self.update.take() {
            messages.push(failure(&update.request, -32800, "session shut down", None));
        }
        messages.extend(self.streams.shutdown());
        self.pending.clear();
        self.closed = true;
        messages.push(success(id, &raw(&())));
        Ok(messages)
    }
}

#[cfg(test)]
#[path = "session_tests.rs"]
mod tests;
