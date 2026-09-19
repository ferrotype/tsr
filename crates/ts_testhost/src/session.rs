use std::{collections::BTreeMap, io};

use serde::{de::DeserializeOwned, Deserialize};
use serde_json::value::RawValue;

use crate::wire::{self, raw, wire, Json};

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
    options: Json,
    plugins: Vec<Registration>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Registration {
    name: String,
    options: Json,
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
    options: Json,
    plugins: Vec<Plugin>,
}
impl Configuration {
    fn from_wire(wire: Initialize) -> Result<Self, String> {
        if wire.version != 1 {
            return Err("unsupported test-host version".into());
        }
        if !wire::object(&wire.options) {
            return Err("options must be an object".into());
        }
        if wire.plugins.len() > MAX_PLUGINS {
            return Err("too many registered plugins".into());
        }
        let mut names = std::collections::BTreeSet::new();
        for plugin in &wire.plugins {
            if plugin.name.is_empty()
                || !names.insert(&plugin.name)
                || !wire::object(&plugin.options)
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
    fn wire(&self) -> Json {
        self.wire_with_options(&self.options, false)
    }

    fn wire_with_options(&self, options: &RawValue, reserve_states: bool) -> Json {
        let plugins: Vec<_> = self
            .plugins
            .iter()
            .map(|plugin| {
                let state = if reserve_states {
                    PluginState::Registered
                } else {
                    plugin.state
                };
                wire!({"name": plugin.registration.name, "options": plugin.registration.options,
                   "state": state.text()})
            })
            .collect();
        wire!({"version": 1, "caseSensitive": self.host.case_sensitive,
               "options": options, "plugins": plugins})
    }
}

enum Action {
    Initialize(Box<Configuration>),
    Options(Json),
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

    pub fn receive(&mut self, message: &crate::framing::Message<'_>) -> io::Result<Vec<Json>> {
        if self.closed {
            return Err(invalid("message on closed test-host session"));
        }
        let envelope: BTreeMap<String, &RawValue> =
            serde_json::from_str(message.as_str()).map_err(invalid)?;
        let string = |name: &str| {
            envelope
                .get(name)
                .and_then(|value| serde_json::from_str::<String>(value.get()).ok())
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
                let id = serde_json::from_str::<u64>(id.get())
                    .ok()
                    .filter(|id| *id > self.last_request && *id <= MAX_CLIENT_ID)
                    .ok_or_else(|| {
                        invalid("client request IDs must be increasing positive safe integers")
                    })?;
                self.last_request = id;
                Ok(self.request(id, &method, params))
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
                    .and_then(|v| serde_json::from_str::<i64>(v.get()).ok())
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

    fn request(&mut self, id: u64, method: &str, params: &RawValue) -> Vec<Json> {
        let result = match method {
            "test/initialize" => self.initialize(id, params),
            "test/shutdown" => self.shutdown(id, params),
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
            "test/state" => empty(params)
                .map(|()| vec![success(id, &self.configuration.as_ref().unwrap().wire())]),
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

    fn initialize(&mut self, id: u64, params: &RawValue) -> RequestResult {
        // Cancellation completes the client request, but the host may still be
        // processing configuration. Keep its ordering barrier until the late
        // callback response is consumed (or the connection is discarded).
        if self.configuration.is_some() || !self.pending.is_empty() {
            return Err((-32002, "initialization already started or complete".into()));
        }
        let wire: Initialize = decode(params)?;
        let configuration = Configuration::from_wire(wire).map_err(|error| (-32602, error))?;
        response_fits(MAX_CLIENT_ID, &configuration.wire()).map_err(|error| (-32602, error))?;
        let params = wire!({"options":configuration.options});
        self.start(
            id,
            "testhost/configuration",
            &params,
            Action::Initialize(Box::new(configuration)),
        )
    }

    fn options(&mut self, id: u64, params: &RawValue) -> RequestResult {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Options {
            options: Json,
        }
        let options: Options = decode(params)?;
        if !wire::object(&options.options) {
            return Err((-32602, "options must be an object".into()));
        }
        if !self.pending.is_empty() {
            return Err((-32002, "options require an idle session".into()));
        }
        // Check all locally-known response sizes before the host can apply a change.
        // Registered is the longest plugin state; reserve it even for ready plugins.
        let state = self
            .configuration
            .as_ref()
            .unwrap()
            .wire_with_options(&options.options, true);
        response_fits(MAX_CLIENT_ID, &state).map_err(|error| (-32602, error))?;
        response_fits(id, &wire!({"options": options.options})).map_err(|error| (-32602, error))?;
        self.start(
            id,
            "testhost/configuration",
            &wire!({"options":options.options}),
            Action::Options(options.options),
        )
    }

    fn file(&mut self, id: u64, params: &RawValue) -> RequestResult {
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
                &raw(&path),
                Action::File { operation, path },
            )
        } else {
            host.complete(&operation, &path, serde_json::Value::Null)
                .map(|value| vec![success(id, &raw(&value))])
                .map_err(|error| (-32001, error))
        }
    }

    fn plugin(&mut self, id: u64, params: &RawValue) -> RequestResult {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Call {
            name: String,
            method: String,
            params: Json,
        }
        let call: Call = decode(params)?;
        if !wire::object(&call.params) {
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
        let params = wire!({"name":call.name,"method":call.method,"params":call.params,"options":plugin.registration.options});
        self.start(
            id,
            "testhost/plugin",
            &params,
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
        result: Option<Json>,
        error: Option<Json>,
    ) -> io::Result<Vec<Json>> {
        let pending = self
            .pending
            .remove(callback)
            .ok_or_else(|| invalid("unknown or duplicate callback response"))?;
        let configuration = matches!(pending.action, Action::Initialize(_) | Action::Options(_));
        if configuration
            && error.is_none()
            && (pending.canceled || result.as_deref().is_some_and(|value| ready(value).is_err()))
        {
            // An error promises no applied change (or a completed rollback).
            // A canceled success or malformed acknowledgment cannot establish
            // agreement with the host. Even direct Session callers must stop.
            self.closed = true;
            self.pending.clear();
            self.configuration = None;
            return Err(invalid(if pending.canceled {
                "canceled configuration may have been applied; discard session"
            } else {
                "invalid configuration acknowledgment; discard session"
            }));
        }
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
                messages.push(success(pending.request, &value));
                if initialize {
                    messages.push(notify("testhost/initialized", &wire!({"version":1})));
                }
            }
            Err(error) => messages.push(failure(pending.request, -32001, &error, None)),
        }
        Ok(messages)
    }

    fn complete(&mut self, request: u64, action: Action, result: Json) -> Result<Json, String> {
        match action {
            Action::Initialize(configuration) => {
                ready(&result)?;
                let value = configuration.wire();
                self.configuration = Some(*configuration);
                Ok(value)
            }
            Action::Options(options) => {
                ready(&result)?;
                let value = wire!({"options":options});
                self.configuration.as_mut().unwrap().options = options;
                Ok(value)
            }
            Action::File { operation, path } => {
                let value = self.configuration.as_ref().unwrap().host.complete(
                    &operation,
                    &path,
                    serde_json::from_str(result.get()).map_err(|e| e.to_string())?,
                )?;
                let value = raw(&value);
                response_fits(request, &value)?;
                Ok(value)
            }
            Action::Plugin { index, method } => {
                response_fits(request, &result)?;
                let plugin = &mut self.configuration.as_mut().unwrap().plugins[index];
                match method.as_str() {
                    "spawn" | "dispose" => {
                        if result.get() != "null" {
                            return Err("spawn/dispose result must be null".into());
                        }
                        plugin.state = if method == "spawn" {
                            PluginState::Spawned
                        } else {
                            PluginState::Registered
                        };
                    }
                    "initialize" => {
                        if !wire::object(&result) {
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

    fn notification(&mut self, method: &str, params: &RawValue) -> io::Result<Vec<Json>> {
        match method {
            "$/cancelRequest" => {
                #[derive(Deserialize)]
                #[serde(deny_unknown_fields)]
                struct Cancel {
                    id: u64,
                }
                let cancel: Cancel = serde_json::from_str(params.get()).map_err(invalid)?;
                Ok(self.cancel(cancel.id))
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
                let request = pending.request;
                let message = notify(
                    "testhost/progress",
                    &wire!({
                        "id":request,"callback":report.callback,"phase":"report","value":report.value,
                    }),
                );
                if fits(&message) {
                    Ok(vec![message])
                } else {
                    Ok(self.interrupt(request, -32001, "progress exceeds frame limit"))
                }
            }
            _ => Ok(vec![]),
        }
    }

    fn cancel(&mut self, request: u64) -> Vec<Json> {
        self.interrupt(request, -32800, "request canceled")
    }

    fn interrupt(&mut self, request: u64, code: i64, message: &str) -> Vec<Json> {
        let Some((callback, pending)) = self
            .pending
            .iter_mut()
            .find(|(_, p)| p.request == request && !p.canceled)
        else {
            return vec![];
        };
        pending.canceled = true;
        let mut messages = vec![notify("$/cancelRequest", &wire!({"id":callback}))];
        if let Action::Plugin { index, .. } = pending.action {
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

    fn shutdown(&mut self, id: u64, params: &RawValue) -> RequestResult {
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
                        &wire!({"name":plugin.registration.name}),
                    ));
                    plugin.state = PluginState::Retired;
                }
            }
        }
        self.pending.clear();
        self.closed = true;
        messages.push(success(id, &raw(&())));
        Ok(messages)
    }
}

type RequestResult = Result<Vec<Json>, (i64, String)>;
fn decode<T: DeserializeOwned>(value: &RawValue) -> Result<T, (i64, String)> {
    serde_json::from_str(value.get()).map_err(|error| (-32602, error.to_string()))
}
fn empty(value: &RawValue) -> Result<(), (i64, String)> {
    if wire::fields(value).is_ok_and(|fields| fields.is_empty()) {
        Ok(())
    } else {
        Err((-32602, "expected empty params object".into()))
    }
}
fn ready(value: &RawValue) -> Result<(), String> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Ready {
        ready: bool,
    }
    if serde_json::from_str::<Ready>(value.get()).is_ok_and(|value| value.ready) {
        Ok(())
    } else {
        Err("configuration callback must return {ready:true}".into())
    }
}
fn invalid(error: impl Into<Box<dyn std::error::Error + Send + Sync>>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error)
}
fn fields(object: &BTreeMap<String, &RawValue>, allowed: &[&str]) -> io::Result<()> {
    if object.keys().any(|key| !allowed.contains(&key.as_str())) {
        return Err(invalid("unknown envelope field"));
    }
    Ok(())
}
fn notify(method: &str, params: &RawValue) -> Json {
    wire!({"jsonrpc":"2.0","method":method,"params":params})
}
fn progress(id: u64, callback: &str, phase: &str) -> Json {
    notify(
        "testhost/progress",
        &wire!({"id":id,"callback":callback,"phase":phase}),
    )
}
fn success(id: u64, result: &RawValue) -> Json {
    wire!({"jsonrpc":"2.0","id":id,"result":result})
}
fn failure(id: u64, code: i64, message: &str, data: Option<Json>) -> Json {
    // Deserializer errors can quote an arbitrarily large unknown field.
    let message = if message.len() > 4096 {
        "invalid oversized protocol payload"
    } else {
        message
    };
    let error = if let Some(data) = data {
        wire!({"code":code,"message":message,"data":data})
    } else {
        wire!({"code":code,"message":message})
    };
    wire!({"jsonrpc":"2.0","id":id,"error":error})
}
fn fits(value: &RawValue) -> bool {
    value.get().len() <= crate::framing::MAX_BODY
}
fn response_fits(id: u64, value: &RawValue) -> Result<(), String> {
    if fits(&success(id, value)) {
        Ok(())
    } else {
        Err("response exceeds frame limit".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn send(session: &mut Session, text: &str) -> io::Result<Vec<Json>> {
        session.receive(&crate::framing::parse_json(text.as_bytes())?)
    }

    #[test]
    fn canceled_configuration_failure_retires_the_reusable_session() {
        let mut session = Session::default();
        send(&mut session, r#"{"jsonrpc":"2.0","id":1,"method":"test/initialize","params":{"version":1,"caseSensitive":true,"base":{},"symlinks":{},"callbacks":[],"options":{},"plugins":[]}}"#).unwrap();
        send(
            &mut session,
            r#"{"jsonrpc":"2.0","method":"$/cancelRequest","params":{"id":1}}"#,
        )
        .unwrap();
        assert!(send(
            &mut session,
            r#"{"jsonrpc":"2.0","id":"callback:1","result":{"ready":true}}"#
        )
        .is_err());
        assert!(session.is_closed());
        assert!(!session.has_pending());
        assert!(send(
            &mut session,
            r#"{"jsonrpc":"2.0","id":2,"method":"test/state","params":{}}"#
        )
        .is_err());
    }

    #[test]
    fn malformed_configuration_acknowledgments_cannot_resume_old_state() {
        for result in [
            r#"{"ready":false}"#,
            r#"{"ready":"true"}"#,
            r#"{"ready":true,"extra":1}"#,
        ] {
            let mut session = Session::default();
            send(&mut session, r#"{"jsonrpc":"2.0","id":1,"method":"test/initialize","params":{"version":1,"caseSensitive":true,"base":{},"symlinks":{},"callbacks":[],"options":{},"plugins":[]}}"#).unwrap();
            let response = format!(r#"{{"jsonrpc":"2.0","id":"callback:1","result":{result}}}"#);
            assert!(send(&mut session, &response).is_err());
            assert!(session.is_closed());
        }
    }

    #[test]
    fn escaped_envelope_strings_and_extreme_numbers_keep_their_meaning() {
        let mut session = Session::default();
        let messages = send(&mut session, r#"{"jsonrpc":"2.\u0030","id":1,"method":"test\/initialize","params":{"version":1,"caseSensitive":true,"base":{},"symlinks":{},"callbacks":[],"options":{"huge":1e400,"tiny":1e-400},"plugins":[]}}"#).unwrap();
        assert!(messages[1]
            .get()
            .contains(r#""options":{"huge":1e400,"tiny":1e-400}"#));
        let messages = send(
            &mut session,
            r#"{"jsonrpc":"2.0","id":"callback:\u0031","result":{"ready":true}}"#,
        )
        .unwrap();
        assert!(messages[1]
            .get()
            .contains(r#""options":{"huge":1e400,"tiny":1e-400}"#));
    }
}
