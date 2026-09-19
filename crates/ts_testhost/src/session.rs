use std::{collections::BTreeMap, io};

use serde::{de::DeserializeOwned, Deserialize};
use serde_json::{json, Value};

use crate::filesystem::{Host, OPERATIONS};

const MAX_PENDING: usize = 64;
const MAX_PLUGINS: usize = 32;
const MAX_CLIENT_ID: u64 = (1_u64 << 53) - 1;

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct Initialize {
    version: u32,
    case_sensitive: bool,
    base: BTreeMap<String, String>,
    symlinks: BTreeMap<String, String>,
    callbacks: Vec<String>,
    options: Value,
    plugins: Vec<Registration>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Registration {
    name: String,
    options: Value,
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum PluginState {
    Registered,
    Spawned,
    Ready,
    Retired,
}
impl PluginState {
    fn text(self) -> &'static str {
        match self {
            Self::Registered => "registered",
            Self::Spawned => "spawned",
            Self::Ready => "ready",
            Self::Retired => "retired",
        }
    }
}
struct Plugin {
    registration: Registration,
    state: PluginState,
}
struct Configuration {
    host: Host,
    options: Value,
    plugins: Vec<Plugin>,
}
impl Configuration {
    fn from_wire(wire: Initialize) -> Result<Self, String> {
        if wire.version != 1 {
            return Err("unsupported test-host version".into());
        }
        if !wire.options.is_object() {
            return Err("options must be an object".into());
        }
        if wire.plugins.len() > MAX_PLUGINS {
            return Err("too many registered plugins".into());
        }
        let mut names = std::collections::BTreeSet::new();
        for plugin in &wire.plugins {
            if plugin.name.is_empty() || !names.insert(&plugin.name) || !plugin.options.is_object()
            {
                return Err("plugins require unique nonempty names and object options".into());
            }
        }
        let host = Host::new(
            wire.case_sensitive,
            &wire.base,
            wire.symlinks,
            &wire.callbacks,
        )?;
        let plugins = wire
            .plugins
            .into_iter()
            .map(|registration| Plugin {
                registration,
                state: PluginState::Registered,
            })
            .collect();
        Ok(Self {
            host,
            options: wire.options,
            plugins,
        })
    }
    fn wire(&self) -> Value {
        let plugins: Vec<_> = self
            .plugins
            .iter()
            .map(|plugin| {
                json!({
                    "name":plugin.registration.name,"options":plugin.registration.options,
                    "state":plugin.state.text(),
                })
            })
            .collect();
        json!({"version":1,"caseSensitive":self.host.case_sensitive,"options":self.options,"plugins":plugins})
    }
}

enum Action {
    Initialize(Box<Configuration>),
    Options(Value),
    File { operation: String, path: String },
    Plugin { index: usize, method: String },
}
struct Pending {
    request: u64,
    action: Action,
    canceled: bool,
}

/// Single connection state. Reverse calls are registered before their frames
/// are returned to the writer. No reference to this state escapes a receive call.
#[derive(Default)]
pub struct Session {
    configuration: Option<Configuration>,
    pending: BTreeMap<String, Pending>,
    last_request: u64,
    next_callback: u64,
    closed: bool,
}
impl Session {
    pub fn is_closed(&self) -> bool {
        self.closed
    }
    pub fn has_pending(&self) -> bool {
        !self.pending.is_empty()
    }

    pub fn receive(&mut self, message: &Value) -> io::Result<Vec<Value>> {
        if self.closed {
            return Err(invalid("message on closed test-host session"));
        }
        let envelope = message
            .as_object()
            .ok_or_else(|| invalid("expected JSON-RPC object"))?;
        if envelope.get("jsonrpc") != Some(&json!("2.0")) {
            return Err(invalid("expected jsonrpc version 2.0"));
        }
        if let Some(method) = envelope.get("method") {
            fields(message, &["jsonrpc", "id", "method", "params"])?;
            let method = method
                .as_str()
                .ok_or_else(|| invalid("method must be a string"))?;
            let params = envelope.get("params").cloned().unwrap_or_else(|| json!({}));
            if let Some(id) = envelope.get("id") {
                let id = id
                    .as_u64()
                    .filter(|id| *id > self.last_request && *id <= MAX_CLIENT_ID)
                    .ok_or_else(|| {
                        invalid("client request IDs must be increasing positive safe integers")
                    })?;
                self.last_request = id;
                Ok(self.request(id, method, params))
            } else {
                self.notification(method, params)
            }
        } else {
            fields(message, &["jsonrpc", "id", "result", "error"])?;
            let id = envelope
                .get("id")
                .and_then(Value::as_str)
                .ok_or_else(|| invalid("callback response requires string ID"))?;
            if envelope.contains_key("result") == envelope.contains_key("error") {
                return Err(invalid("response requires exactly one result or error"));
            }
            if let Some(error) = envelope.get("error") {
                fields(error, &["code", "message", "data"])?;
                if error.get("code").and_then(Value::as_i64).is_none()
                    || error.get("message").and_then(Value::as_str).is_none()
                {
                    return Err(invalid("invalid callback error"));
                }
            }
            self.response(
                id,
                envelope.get("result").cloned(),
                envelope.get("error").cloned(),
            )
        }
    }

    fn request(&mut self, id: u64, method: &str, params: Value) -> Vec<Value> {
        let result = match method {
            "test/initialize" => self.initialize(id, params),
            "test/shutdown" => self.shutdown(id, &params),
            "test/state" | "test/setOptions" | "test/fs" | "test/plugin"
                if self.configuration.is_none() =>
            {
                Err((-32002, "test-host is not initialized".into()))
            }
            "test/fs" | "test/plugin"
                if self
                    .pending
                    .values()
                    .any(|p| matches!(p.action, Action::Options(_))) =>
            {
                Err((-32002, "configuration update is pending".into()))
            }
            "test/state" => empty(&params)
                .map(|()| vec![success(id, self.configuration.as_ref().unwrap().wire())]),
            "test/setOptions" => self.options(id, params),
            "test/fs" => self.file(id, params),
            "test/plugin" => self.plugin(id, params),
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

    fn initialize(&mut self, id: u64, params: Value) -> RequestResult {
        // Cancellation completes the client request, but the host may still be
        // processing configuration. Keep its ordering barrier until the late
        // callback response is consumed (or the connection is discarded).
        if self.configuration.is_some() || !self.pending.is_empty() {
            return Err((-32002, "initialization already started or complete".into()));
        }
        let wire: Initialize = decode(params)?;
        let configuration = Configuration::from_wire(wire).map_err(|error| (-32602, error))?;
        let params = json!({"options":configuration.options});
        self.start(
            id,
            "testhost/configuration",
            params,
            Action::Initialize(Box::new(configuration)),
        )
    }

    fn options(&mut self, id: u64, params: Value) -> RequestResult {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Options {
            options: Value,
        }
        let options: Options = decode(params)?;
        if !options.options.is_object() {
            return Err((-32602, "options must be an object".into()));
        }
        if !self.pending.is_empty() {
            return Err((-32002, "options require an idle session".into()));
        }
        self.start(
            id,
            "testhost/configuration",
            json!({"options":options.options}),
            Action::Options(options.options),
        )
    }

    fn file(&mut self, id: u64, params: Value) -> RequestResult {
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
        crate::filesystem::valid_base_path(&path).map_err(|error| (-32602, error))?;
        let host = &self.configuration.as_ref().unwrap().host;
        if host.enabled(&operation) {
            self.start(
                id,
                &operation.clone(),
                json!(path),
                Action::File { operation, path },
            )
        } else {
            host.complete(&operation, &path, Value::Null)
                .map(|value| vec![success(id, value)])
                .map_err(|error| (-32001, error))
        }
    }

    fn plugin(&mut self, id: u64, params: Value) -> RequestResult {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Call {
            name: String,
            method: String,
            params: Value,
        }
        let call: Call = decode(params)?;
        if !call.params.is_object() {
            return Err((-32602, "plugin params must be an object".into()));
        }
        let configuration = self.configuration.as_ref().unwrap();
        let index = configuration
            .plugins
            .iter()
            .position(|p| p.registration.name == call.name)
            .ok_or_else(|| (-32602, "unregistered plugin".into()))?;
        let plugin = &configuration.plugins[index];
        if self
            .pending
            .values()
            .any(|p| matches!(p.action, Action::Plugin { index: i, .. } if i == index))
        {
            return Err((-32002, "plugin has a pending request".into()));
        }
        let allowed = match call.method.as_str() {
            "spawn" => plugin.state == PluginState::Registered,
            "initialize" => plugin.state == PluginState::Spawned,
            "openProject" | "transform" | "closeProject" => plugin.state == PluginState::Ready,
            "dispose" => matches!(plugin.state, PluginState::Spawned | PluginState::Ready),
            _ => return Err((-32602, "unknown plugin method".into())),
        };
        if !allowed {
            return Err((-32002, "invalid plugin lifecycle".into()));
        }
        let params = json!({"name":call.name,"method":call.method,"params":call.params,"options":plugin.registration.options});
        self.start(
            id,
            "testhost/plugin",
            params,
            Action::Plugin {
                index,
                method: call.method,
            },
        )
    }

    fn start(
        &mut self,
        request: u64,
        method: &str,
        params: Value,
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
        let mut message = json!({"jsonrpc":"2.0","id":callback,"method":method});
        message["params"] = params;
        if !fits(&message) {
            return Err((-32602, "outgoing callback exceeds frame limit".into()));
        }
        self.pending.insert(
            callback.clone(),
            Pending {
                request,
                action,
                canceled: false,
            },
        );
        Ok(vec![progress(request, &callback, "begin"), message])
    }

    fn response(
        &mut self,
        callback: &str,
        result: Option<Value>,
        error: Option<Value>,
    ) -> io::Result<Vec<Value>> {
        let pending = self
            .pending
            .remove(callback)
            .ok_or_else(|| invalid("unknown or duplicate callback response"))?;
        if pending.canceled {
            return Ok(vec![]);
        }
        let mut messages = vec![progress(pending.request, callback, "end")];
        if let Some(error) = error {
            let error = failure(pending.request, -32001, "callback failed", Some(error));
            messages.push(if fits(&error) {
                error
            } else {
                failure(
                    pending.request,
                    -32001,
                    "callback error exceeds frame limit",
                    None,
                )
            });
            return Ok(messages);
        }
        let result = result.expect("validated response has result or error");
        let initialize = matches!(pending.action, Action::Initialize(_));
        let value = self.complete(pending.request, pending.action, result);
        match value {
            Ok(value) => {
                messages.push(success(pending.request, value));
                if initialize {
                    messages.push(notify("testhost/initialized", json!({"version":1})));
                }
            }
            Err(error) => messages.push(failure(pending.request, -32001, &error, None)),
        }
        Ok(messages)
    }

    fn complete(&mut self, request: u64, action: Action, result: Value) -> Result<Value, String> {
        match action {
            Action::Initialize(configuration) => {
                ready(&result)?;
                let value = configuration.wire();
                response_fits(MAX_CLIENT_ID, &value)?;
                self.configuration = Some(*configuration);
                Ok(value)
            }
            Action::Options(options) => {
                ready(&result)?;
                let value = json!({"options":options});
                response_fits(request, &value)?;
                let mut state = self.configuration.as_ref().unwrap().wire();
                state["options"] = options.clone();
                // A later dispose returns a plugin to "registered", the
                // longest state name. Reserve that space for every plugin now
                // so a successful lifecycle call cannot strand test/state.
                for plugin in state["plugins"].as_array_mut().expect("wire plugin array") {
                    plugin["state"] = json!(PluginState::Registered.text());
                }
                response_fits(MAX_CLIENT_ID, &state)?;
                self.configuration.as_mut().unwrap().options = options;
                Ok(value)
            }
            Action::File { operation, path } => {
                let value = self
                    .configuration
                    .as_ref()
                    .unwrap()
                    .host
                    .complete(&operation, &path, result)?;
                response_fits(request, &value)?;
                Ok(value)
            }
            Action::Plugin { index, method } => {
                response_fits(request, &result)?;
                let plugin = &mut self.configuration.as_mut().unwrap().plugins[index];
                match method.as_str() {
                    "spawn" | "dispose" => {
                        if !result.is_null() {
                            return Err("spawn/dispose result must be null".into());
                        }
                        plugin.state = if method == "spawn" {
                            PluginState::Spawned
                        } else {
                            PluginState::Registered
                        };
                    }
                    "initialize" => {
                        if !result.is_object() {
                            return Err("mapper initialize result must be an object".into());
                        }
                        plugin.state = PluginState::Ready;
                    }
                    _ => {}
                }
                Ok(result)
            }
        }
    }

    fn notification(&mut self, method: &str, params: Value) -> io::Result<Vec<Value>> {
        match method {
            "$/cancelRequest" => {
                #[derive(Deserialize)]
                #[serde(deny_unknown_fields)]
                struct Cancel {
                    id: u64,
                }
                let cancel: Cancel = serde_json::from_value(params).map_err(invalid)?;
                Ok(self.cancel(cancel.id))
            }
            "test/callbackProgress" => {
                #[derive(Deserialize)]
                #[serde(deny_unknown_fields)]
                struct Report {
                    callback: String,
                    value: Value,
                }
                if params.get("value").is_none() {
                    return Err(invalid("progress value is required"));
                }
                let report: Report = serde_json::from_value(params).map_err(invalid)?;
                let pending = self
                    .pending
                    .get(&report.callback)
                    .ok_or_else(|| invalid("progress for unknown callback"))?;
                if pending.canceled {
                    return Ok(vec![]);
                }
                Ok(vec![notify(
                    "testhost/progress",
                    json!({
                        "id":pending.request,"callback":report.callback,"phase":"report","value":report.value,
                    }),
                )])
            }
            _ => Ok(vec![]),
        }
    }

    fn cancel(&mut self, request: u64) -> Vec<Value> {
        let Some((callback, pending)) = self
            .pending
            .iter_mut()
            .find(|(_, p)| p.request == request && !p.canceled)
        else {
            return vec![];
        };
        pending.canceled = true;
        let mut messages = vec![notify("$/cancelRequest", json!({"id":callback}))];
        if let Action::Plugin { index, .. } = pending.action {
            let plugin = &mut self.configuration.as_mut().unwrap().plugins[index];
            plugin.state = PluginState::Retired;
            messages.push(notify(
                "testhost/retirePlugin",
                json!({"name":plugin.registration.name}),
            ));
        }
        messages.push(progress(request, callback, "end"));
        messages.push(failure(request, -32800, "request canceled", None));
        messages
    }

    fn shutdown(&mut self, id: u64, params: &Value) -> RequestResult {
        empty(params)?;
        let requests: Vec<_> = self.pending.values().map(|p| p.request).collect();
        let mut messages: Vec<_> = requests
            .into_iter()
            .flat_map(|request| self.cancel(request))
            .collect();
        if let Some(configuration) = &mut self.configuration {
            for plugin in &mut configuration.plugins {
                if matches!(plugin.state, PluginState::Spawned | PluginState::Ready) {
                    messages.push(notify(
                        "testhost/retirePlugin",
                        json!({"name":plugin.registration.name}),
                    ));
                    plugin.state = PluginState::Retired;
                }
            }
        }
        self.pending.clear();
        self.closed = true;
        messages.push(success(id, Value::Null));
        Ok(messages)
    }
}

type RequestResult = Result<Vec<Value>, (i64, String)>;
fn decode<T: DeserializeOwned>(value: Value) -> Result<T, (i64, String)> {
    serde_json::from_value(value).map_err(|error| (-32602, error.to_string()))
}
fn empty(value: &Value) -> Result<(), (i64, String)> {
    if value.as_object().is_some_and(serde_json::Map::is_empty) {
        Ok(())
    } else {
        Err((-32602, "expected empty params object".into()))
    }
}
fn ready(value: &Value) -> Result<(), String> {
    if value == &json!({"ready":true}) {
        Ok(())
    } else {
        Err("configuration callback must return {ready:true}".into())
    }
}
fn invalid(error: impl Into<Box<dyn std::error::Error + Send + Sync>>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error)
}
fn fields(value: &Value, allowed: &[&str]) -> io::Result<()> {
    let object = value
        .as_object()
        .ok_or_else(|| invalid("expected object"))?;
    if object.keys().any(|key| !allowed.contains(&key.as_str())) {
        return Err(invalid("unknown envelope field"));
    }
    Ok(())
}
fn notify(method: &str, params: Value) -> Value {
    let mut value = json!({"jsonrpc":"2.0","method":method});
    value["params"] = params;
    value
}
fn progress(id: u64, callback: &str, phase: &str) -> Value {
    notify(
        "testhost/progress",
        json!({"id":id,"callback":callback,"phase":phase}),
    )
}
fn success(id: u64, result: Value) -> Value {
    let mut value = json!({"jsonrpc":"2.0","id":id});
    value["result"] = result;
    value
}
fn failure(id: u64, code: i64, message: &str, data: Option<Value>) -> Value {
    let mut error = json!({"code":code,"message":message});
    if let Some(data) = data {
        error["data"] = data;
    }
    // Deserializer errors may quote an arbitrarily large unknown field. Keep
    // local diagnostics bounded; remote data is preflighted by the caller.
    if message.len() > 4096 {
        error["message"] = json!("invalid oversized protocol payload");
    }
    json!({"jsonrpc":"2.0","id":id,"error":error})
}

fn fits(value: &Value) -> bool {
    // Serialize to a counting writer rather than allocate a second body. Every
    // outgoing application payload is preflighted before committing its state.
    struct Counter(usize);
    impl io::Write for Counter {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.0 = self
                .0
                .checked_add(bytes.len())
                .filter(|n| *n <= crate::framing::MAX_BODY)
                .ok_or_else(|| invalid("outgoing frame exceeds limit"))?;
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    serde_json::to_writer(Counter(0), value).is_ok()
}
fn response_fits(id: u64, value: &Value) -> Result<(), String> {
    if fits(&json!({"jsonrpc":"2.0","id":id,"result":value})) {
        Ok(())
    } else {
        Err("callback result exceeds frame limit".into())
    }
}
