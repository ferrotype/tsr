//! Connection-order preparation over a real project session. Prepared checker
//! work retains its snapshot and can run concurrently with later notifications.
use crate::{
    client::{self, Client},
    diagnostics,
    logger::Logger,
    progress::LoadingProgress,
    Server,
};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    thread::JoinHandle,
    time::Duration,
};
use tsr_ipc::Context;
use tsr_json::RawValue;
use tsr_jsstring::JsString;
use tsr_lsproto as lsp;
use tsr_project::{
    parse_cache::{ContentMappedParseCache, ParseCache},
    session::{Session, SessionEvent, SessionOptions},
};
use tsr_vfs::FileSystem;

pub type Work = Box<dyn FnOnce() -> Result<RawValue, lsp::ResponseError> + Send>;
pub enum Dispatch {
    Ready(RawValue),
    Work(Work),
    Exit,
}
pub struct Options {
    pub project: SessionOptions,
    pub host: Arc<dyn FileSystem>,
    pub progress_delay: Duration,
    pub parent_process: Option<Arc<dyn Fn(i32) + Send + Sync>>,
    pub parse_cache: Option<Arc<ParseCache>>,
    pub mapped_parse_cache: Option<Arc<ContentMappedParseCache>>,
    pub inferred_options: Option<tsr_core::CompilerOptions>,
    pub native_watch: bool,
}
impl Options {
    pub fn new(project: SessionOptions, host: Arc<dyn FileSystem>) -> Self {
        Self {
            project,
            host,
            progress_delay: Duration::from_millis(250),
            parent_process: None,
            parse_cache: None,
            mapped_parse_cache: None,
            inferred_options: None,
            native_watch: true,
        }
    }
}
#[derive(Clone)]
struct Settings {
    locale: tsr_locale::Locale,
    validation: bool,
    style_warnings: bool,
    config_name: String,
    exclude_library_symbols: bool,
    workspace_current_project: bool,
    maximum_hover_length: usize,
    inlay: tsr_ls::InlayHintsOptions,
    inlay_flags: [Option<bool>; 7],
    code_lens: tsr_ls::CodeLensOptions,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            locale: tsr_locale::Locale::default(),
            validation: true,
            style_warnings: true,
            config_name: String::new(),
            exclude_library_symbols: true,
            workspace_current_project: false,
            maximum_hover_length: 500,
            inlay: tsr_ls::InlayHintsOptions::default(),
            inlay_flags: [None; 7],
            code_lens: tsr_ls::CodeLensOptions::default(),
        }
    }
}
pub struct Runtime {
    options: Options,
    context: Context,
    client: Arc<dyn Client>,
    logger: Arc<Logger>,
    recovery: crate::recovery::Recovery,
    initialize: Option<lsp::InitializeParams>,
    capabilities: lsp::ClientCapabilities,
    initialization: lsp::InitializationOptions,
    settings: Arc<Mutex<Settings>>,
    server: Option<Server>,
    initialized: bool,
    shutdown: bool,
    observer: Option<JoinHandle<()>>,
    native_watcher: Option<Arc<crate::watcher::Watcher>>,
}
impl Runtime {
    pub fn new(
        options: Options,
        context: Context,
        client: Arc<dyn Client>,
        stderr: Box<dyn std::io::Write + Send>,
    ) -> Self {
        let logger = Arc::new(Logger::new(client.clone(), context.clone(), stderr));
        Self {
            recovery: crate::recovery::Recovery::new(client.clone(), logger.clone()),
            logger,
            options,
            context,
            client,
            initialize: None,
            capabilities: lsp::ClientCapabilities::default(),
            initialization: lsp::InitializationOptions::default(),
            settings: Arc::default(),
            server: None,
            initialized: false,
            shutdown: false,
            observer: None,
            native_watcher: None,
        }
    }
    pub fn server(&self) -> Option<&Server> {
        self.server.as_ref()
    }
    pub fn logger(&self) -> &Arc<Logger> {
        &self.logger
    }
    pub fn context(&self) -> &Context {
        &self.context
    }

    /// Test hosts may push options between initialize and initialized, before
    /// there is a Session. The initialized barrier applies that latest value.
    pub fn set_inferred_options(
        &mut self,
        options: tsr_core::CompilerOptions,
        host: Arc<dyn FileSystem>,
    ) -> Result<(), lsp::ResponseError> {
        if let Some(server) = &self.server {
            server
                .session()
                .apply_inferred_options(options, host)
                .map_err(crate::project_error)?;
        } else {
            self.options.inferred_options = Some(options);
        }
        Ok(())
    }

    pub fn prepare(
        &mut self,
        context: &Context,
        request: &lsp::Message,
        host: Arc<dyn FileSystem>,
    ) -> Result<Dispatch, lsp::ResponseError> {
        let recovery = self.recovery.clone();
        let result = recovery.run(&request.method, || {
            self.prepare_inner(context, request, host)
        })?;
        Ok(match result {
            Dispatch::Work(work) => {
                let method = request.method.clone();
                Dispatch::Work(Box::new(move || recovery.run(&method, work)))
            }
            result => result,
        })
    }

    // port: tsc/internal/lsp/server.go:Server.handleRequestOrNotification
    fn prepare_inner(
        &mut self,
        context: &Context,
        request: &lsp::Message,
        host: Arc<dyn FileSystem>,
    ) -> Result<Dispatch, lsp::ResponseError> {
        if context.err().is_some() {
            return Err(crate::canceled());
        }
        let params = request.params.as_ref();
        let method = request.method.as_str();
        if method == "initialize" {
            return self
                .initialize(params)
                .and_then(|r| client::raw(&r))
                .map(Dispatch::Ready);
        }
        if self.initialize.is_none() || (!self.initialized && method != "initialized") {
            return Err(crate::coded_error(
                lsp::ErrorCode::SERVER_NOT_INITIALIZED,
                None,
            ));
        }
        if method == "exit" {
            let _: lsp::NoParams = crate::decode(params)?;
            return Ok(Dispatch::Exit);
        }
        if self.shutdown {
            return Err(crate::coded_error(
                lsp::ErrorCode::INVALID_REQUEST,
                Some("server is shut down"),
            ));
        }
        match method {
            "initialized" => {
                let _: lsp::InitializedParams = crate::decode(params)?;
                self.initialized(context, host)?;
            }
            "shutdown" => {
                let _: lsp::NoParams = crate::decode(params)?;
                self.close();
                self.shutdown = true;
            }
            "$/setTrace" => {
                let _: lsp::SetTraceParams = crate::decode(params)?;
            }
            "custom/setLogVerbosity" => {
                let value: lsp::SetLogVerbosityParams = crate::decode(params)?;
                if !crate::logger::is_valid_log_verbosity(value.verbosity) {
                    return Err(crate::invalid(&format!(
                        "invalid log verbosity {}",
                        value.verbosity.0
                    )));
                }
                self.logger.set_verbosity(value.verbosity);
            }
            "workspace/didChangeConfiguration" => {
                let value: lsp::DidChangeConfigurationParams = crate::decode(params)?;
                if matches!(value.settings, lsp::Any::Object(_)) {
                    self.apply_settings(&value.settings)?;
                }
            }
            "textDocument/didOpen"
            | "textDocument/didChange"
            | "textDocument/didClose"
            | "textDocument/didSave"
            | "workspace/didChangeWatchedFiles" => {
                self.ready()?.notification(method, params, host)?;
            }
            "custom/projectInfo" => {
                let value: lsp::ProjectInfoParams = crate::decode(params)?;
                let uri = &value.text_document.uri;
                let snapshot = self
                    .ready()?
                    .session()
                    .flush_with_host(Some(uri), host)
                    .map_err(crate::project_error)?;
                let path = uri.path(
                    snapshot
                        .filesystem()
                        .unwrap()
                        .use_case_sensitive_file_names(),
                );
                let name = snapshot
                    .project_for_file(path.as_bytes())
                    .and_then(|p| p.data())
                    .filter(|p| p.kind == tsr_project::project::ProjectKind::Configured)
                    .map(|p| wire_string(p.name.as_bytes()))
                    .transpose()?
                    .unwrap_or_default();
                return client::raw(&lsp::ProjectInfoResult {
                    config_file_path: name,
                })
                .map(Dispatch::Ready);
            }
            "textDocument/diagnostic" => {
                let value: lsp::DocumentDiagnosticParams = crate::decode(params)?;
                let uri = value.text_document.uri;
                let server = self.ready()?;
                let snapshot = server
                    .session()
                    .flush_with_host(Some(&uri), host)
                    .map_err(crate::project_error)?;
                let path = uri.path(
                    snapshot
                        .filesystem()
                        .unwrap()
                        .use_case_sensitive_file_names(),
                );
                let Some(project) = snapshot.project_for_file(path.as_bytes()).cloned() else {
                    // contentMapperFallbackResponse applies only to an existing
                    // file of unknown script kind, not every missing project.
                    if snapshot
                        .filesystem()
                        .unwrap()
                        .get_file(uri.file_name().as_bytes())
                        .map_err(|e| crate::project_error(e.into()))?
                        .is_some_and(|file| file.kind() == tsr_core::ScriptKind::UNKNOWN)
                    {
                        return client::raw(&lsp::RelatedFullDocumentDiagnosticReport::default())
                            .map(Dispatch::Ready);
                    }
                    return Err(crate::error(
                        -32603,
                        format!("no project found for URI {}", uri.0),
                    ));
                };
                let context = context.clone();
                let request_id = request
                    .id
                    .as_ref()
                    .map(ToString::to_string)
                    .unwrap_or_default();
                let settings = self.settings.lock().unwrap().clone();
                let encoding = self.options.project.position_encoding;
                let options = diagnostics::options(
                    &self.capabilities,
                    settings.locale.clone(),
                    true,
                    settings.style_warnings,
                );
                let push_options =
                    diagnostics::options(&self.capabilities, settings.locale, false, false);
                let client = self.client.clone();
                let push = !self
                    .initialization
                    .disable_push_diagnostics
                    .as_deref()
                    .copied()
                    .unwrap_or(false);
                return Ok(Dispatch::Work(Box::new(move || {
                    let _snapshot = snapshot;
                    let result = diagnostics::document(
                        &context,
                        &request_id,
                        &project,
                        &uri,
                        encoding,
                        &options,
                        settings.validation,
                    )?;
                    if push
                        && project.data().unwrap().kind
                            == tsr_project::project::ProjectKind::Configured
                        && project.scheduler().unwrap().take_new_global_diagnostics()
                    {
                        diagnostics::publish_project(
                            client.as_ref(),
                            &project,
                            encoding,
                            &push_options,
                            settings.validation,
                        )?;
                    }
                    client::raw(&result)
                })));
            }
            "workspace/symbol" => {
                let params: lsp::WorkspaceSymbolParams = crate::decode(params)?;
                let settings = self.settings.lock().unwrap().clone();
                let uri = params
                    .text_document
                    .as_deref()
                    .map(|d| &d.uri)
                    .filter(|_| settings.workspace_current_project);
                let snapshot = self
                    .ready()?
                    .session()
                    .flush_with_host(uri, host)
                    .map_err(crate::project_error)?;
                let path = uri.map(|u| {
                    u.path(
                        snapshot
                            .filesystem()
                            .unwrap()
                            .use_case_sensitive_file_names(),
                    )
                });
                let context = context.clone();
                let encoding = self.options.project.position_encoding;
                return Ok(Dispatch::Work(Box::new(move || {
                    let cancellation = tsr_core::CancellationToken::new();
                    let cancel = cancellation.clone();
                    struct Stop(tsr_ipc::AfterFuncStop);
                    impl Drop for Stop {
                        fn drop(&mut self) {
                            self.0.stop();
                        }
                    }
                    let _stop = Stop(context.after_func(move || cancel.cancel()));
                    let programs: Vec<_> = snapshot
                        .projects()
                        .into_iter()
                        .filter_map(|p| p.program().map(AsRef::as_ref))
                        .filter(|p| {
                            path.as_ref()
                                .is_none_or(|path| p.source_file(path.as_bytes()).is_some())
                        })
                        .collect();
                    let response = tsr_ls::workspace_symbols(
                        &programs,
                        encoding,
                        &cancellation,
                        &params.query,
                        settings.exclude_library_symbols,
                    )
                    .map_err(|e| crate::error(-32603, e.to_string()))?;
                    client::raw(&response)
                })));
            }
            _ if crate::language_features::handles(method) => {
                let feature = crate::language_features::Request::decode(method, params)?;
                let uri = feature.uri();
                let snapshot = self
                    .ready()?
                    .session()
                    .flush_with_host(Some(uri), host)
                    .map_err(crate::project_error)?;
                let path = uri.path(
                    snapshot
                        .filesystem()
                        .unwrap()
                        .use_case_sensitive_file_names(),
                );
                let project = snapshot.project_for_file(path.as_bytes()).cloned();
                if project.is_none()
                    && feature.unknown_script_fallback()
                    && snapshot
                        .filesystem()
                        .unwrap()
                        .get_file(uri.file_name().as_bytes())
                        .map_err(|e| crate::project_error(e.into()))?
                        .is_some_and(|file| file.kind() == tsr_core::ScriptKind::UNKNOWN)
                {
                    return client::raw(&lsp::Null).map(Dispatch::Ready);
                }
                let context = context.clone();
                let capabilities = self.capabilities.clone();
                let settings = self.settings.lock().unwrap().clone();
                let options = crate::language_features::Options {
                    maximum_hover_length: settings.maximum_hover_length,
                    inlay: settings.inlay,
                    code_lens: settings.code_lens,
                    lens_command: self
                        .initialization
                        .code_lens_show_locations_command_name
                        .as_deref()
                        .cloned(),
                    locale: settings.locale,
                };
                let encoding = self.options.project.position_encoding;
                let request_id = request
                    .id
                    .as_ref()
                    .map(ToString::to_string)
                    .unwrap_or_default();
                return Ok(Dispatch::Work(Box::new(move || {
                    let _snapshot = snapshot;
                    crate::language_features::execute(
                        &context,
                        &request_id,
                        project.as_ref(),
                        feature,
                        encoding,
                        &capabilities,
                        &options,
                    )
                })));
            }
            _ if unimplemented_method(method) => {
                return Err(crate::error(
                    -32601,
                    format!("method not implemented: {method}"),
                ))
            }
            _ if request.id.is_some() => {
                return Err(crate::coded_error(lsp::ErrorCode::INVALID_REQUEST, None));
            }
            _ => {} // The pin ignores unknown notifications.
        }
        Ok(Dispatch::Ready(RawValue(b"null".to_vec())))
    }
    fn ready(&self) -> Result<&Server, lsp::ResponseError> {
        if !self.initialized {
            return Err(crate::coded_error(
                lsp::ErrorCode::SERVER_NOT_INITIALIZED,
                None,
            ));
        }
        self.server
            .as_ref()
            .ok_or_else(|| crate::coded_error(lsp::ErrorCode::SERVER_NOT_INITIALIZED, None))
    }
    // port: tsc/internal/lsp/server.go:Server.handleInitialize
    fn initialize(
        &mut self,
        params: Option<&RawValue>,
    ) -> Result<lsp::InitializeResult, lsp::ResponseError> {
        if self.initialize.is_some() {
            return Err(crate::error(-32600, "initialize called twice"));
        }
        let params: lsp::InitializeParams = crate::decode(params)?;
        self.logger.initialize_started();
        self.capabilities = params.capabilities.as_deref().cloned().unwrap_or_default();
        self.initialization = params
            .initialization_options
            .as_deref()
            .and_then(|v| v.initialization_options.as_deref())
            .cloned()
            .unwrap_or_default();
        if let Some(level) = self
            .initialization
            .log_verbosity
            .as_deref()
            .copied()
            .filter(|v| crate::logger::is_valid_log_verbosity(*v))
        {
            self.logger.set_verbosity(level);
        }
        let encoding = crate::capabilities::encoding(&self.capabilities);
        self.options.project.position_encoding = encoding;
        let locale = params
            .locale
            .as_deref()
            .map(|l| tsr_locale::Locale::parse(l).0)
            .unwrap_or_default();
        self.settings.lock().unwrap().locale = locale;
        if let Some(callback) = &self.options.parent_process {
            if let Some(pid) = params.process_id.integer.as_deref() {
                callback(*pid);
            }
        }
        self.initialize = Some(params);
        Ok(crate::capabilities::initialize(
            &self.capabilities,
            encoding,
        ))
    }
    // port: tsc/internal/lsp/server.go:Server.handleInitialized
    fn initialized(
        &mut self,
        context: &Context,
        host: Arc<dyn FileSystem>,
    ) -> Result<(), lsp::ResponseError> {
        if self.initialized {
            return Err(crate::error(-32600, "initialized called twice"));
        }
        let params = self.initialize.as_ref().unwrap();
        let owned_workspace = self.capabilities.workspace.clone();
        let workspace = owned_workspace.as_deref();
        let mut cwd = self.options.project.current_directory.clone();
        if workspace
            .and_then(|w| w.workspace_folders.as_deref())
            .copied()
            .unwrap_or(false)
            && params
                .workspace_folders
                .as_deref()
                .and_then(|w| w.workspace_folders.as_deref())
                .is_some_and(|w| w.len() == 1)
        {
            if let Some(folder) = params
                .workspace_folders
                .as_deref()
                .unwrap()
                .workspace_folders
                .as_deref()
                .unwrap()[0]
                .as_deref()
            {
                cwd = lsp::DocumentUri(folder.uri.0.clone()).file_name();
            }
        } else if let Some(uri) = params.root_uri.document_uri.as_deref() {
            cwd = uri.file_name();
        } else if let Some(path) = params
            .root_path
            .as_deref()
            .and_then(|p| p.string.as_deref())
        {
            cwd = JsString::from_bytes(path.as_bytes());
        }
        if tsr_tspath::path_is_absolute(cwd.as_bytes()) {
            self.options.project.current_directory = cwd;
        }
        self.options.project.run_external_code = self
            .initialization
            .run_external_code
            .as_deref()
            .copied()
            .unwrap_or(false);
        self.recovery.set_telemetry(
            self.initialization
                .enable_telemetry
                .as_deref()
                .copied()
                .unwrap_or(false),
        );
        self.options.project.debounce_delay = Duration::from_millis(500);
        self.options.project.logger = tsr_project::logging::Logger::from_sink(self.logger.clone());
        self.options.project.relative_watch_patterns = workspace
            .and_then(|w| w.did_change_watched_files.as_deref())
            .and_then(|w| w.relative_pattern_support.as_deref())
            .copied()
            .unwrap_or(false);
        if self.server.is_none() {
            let caches = || tsr_project::ref_count_cache::RefCountCacheOptions::default();
            let session = Session::with_caches(
                self.options.project.clone(),
                self.options.host.clone(),
                &tsr_arena::Counters::new(),
                self.options
                    .parse_cache
                    .clone()
                    .unwrap_or_else(|| Arc::new(ParseCache::new(caches()))),
                self.options
                    .mapped_parse_cache
                    .clone()
                    .unwrap_or_else(|| Arc::new(ContentMappedParseCache::new(caches()))),
            );
            self.server = Some(Server::new(session));
        }
        let session = self.server.as_ref().unwrap().session().clone();
        let session = if workspace
            .and_then(|w| w.did_change_watched_files.as_deref())
            .and_then(|w| w.dynamic_registration.as_deref())
            .copied()
            .unwrap_or(false)
        {
            session.with_watch_client(Arc::new(RemoteWatch(self.client.clone())))
        } else if self.options.native_watch
            && tsr_fswatch::default_watcher().has_fast_recursive_backend()
        {
            let weak = Arc::downgrade(&session);
            let watcher = crate::watcher::Watcher::new(
                self.options.host.clone(),
                Arc::new(move |changes| {
                    if let Some(session) = weak.upgrade() {
                        let _ = session.did_change_watched_files(changes);
                    }
                }),
                self.logger.clone(),
            );
            self.native_watcher = Some(watcher.clone());
            session.with_watch_client(watcher)
        } else {
            session
        };
        self.server = Some(Server::new(session));
        self.start_observer();
        if let Some(options) = self.options.inferred_options.take() {
            self.server
                .as_ref()
                .unwrap()
                .session()
                .apply_inferred_options(options, host)
                .map_err(crate::project_error)?;
        }
        self.initialized = true;
        let prefs = if workspace
            .and_then(|w| w.configuration.as_deref())
            .copied()
            .unwrap_or(false)
        {
            let response = self.client.request(context, "workspace/configuration", RawValue(br#"{"items":[{"section":"js/ts"},{"section":"typescript"},{"section":"javascript"},{"section":"editor"}]}"#.to_vec()))?;
            let mut values: Vec<lsp::Any> = Vec::new();
            tsr_json::unmarshal(&response.0, &mut values, tsr_json::Options::default())
                .map_err(|e| crate::invalid(&e.to_string()))?;
            lsp::Any::Object(
                ["js/ts", "typescript", "javascript", "editor"]
                    .into_iter()
                    .zip(values)
                    .map(|(name, value)| (name.into(), value))
                    .collect(),
            )
        } else {
            lsp::Any::Object(HashMap::from([(
                "js/ts".into(),
                self.initialization
                    .user_preferences
                    .as_deref()
                    .cloned()
                    .unwrap_or_default(),
            )]))
        };
        self.apply_settings(&prefs)?;
        {
            self.client.request(context, "client/registerCapability", RawValue(br#"{"registrations":[{"id":"typescript-config-watch-id","method":"workspace/didChangeConfiguration","registerOptions":{"section":["js/ts","typescript","javascript","editor"]}}]}"#.to_vec()))?;
        }
        Ok(())
    }
    fn start_observer(&mut self) {
        let receive = self.server.as_ref().unwrap().session().subscribe();
        let client = self.client.clone();
        let settings = self.settings.clone();
        let caps = self.capabilities.clone();
        let encoding = self.options.project.position_encoding;
        let push = !self
            .initialization
            .disable_push_diagnostics
            .as_deref()
            .copied()
            .unwrap_or(false);
        let progress = self
            .capabilities
            .window
            .as_deref()
            .and_then(|w| w.work_done_progress.as_deref())
            .copied()
            .unwrap_or(false)
            .then(|| {
                let locale = self.settings.lock().unwrap().locale.clone();
                let title = tsr_diagnostics::Loading.localize(&locale, &[]);
                LoadingProgress::new(
                    &self.context,
                    self.client.clone(),
                    self.options.progress_delay,
                    String::from_utf8_lossy(&title).into_owned(),
                )
            });
        let logger = self.logger.clone();
        self.observer = Some(std::thread::spawn(move || {
            while let Ok(event) = receive.recv() {
                let settings = settings.lock().unwrap().clone();
                match event {
                    SessionEvent::ProjectLoading { name, finished } => {
                        if let Some(progress) = &progress {
                            let message = tsr_diagnostics::Project_0.localize(
                                &settings.locale,
                                &[tsr_diagnostics::Argument::Bytes(name.as_bytes().to_vec())],
                            );
                            let message = String::from_utf8_lossy(&message).into_owned();
                            if finished {
                                progress.finish(message);
                            } else {
                                progress.start(message);
                            }
                        }
                    }
                    SessionEvent::Published { previous, current } if push => {
                        let options = diagnostics::options(&caps, settings.locale, false, false);
                        if let Err(e) = diagnostics::published(
                            client.as_ref(),
                            &previous,
                            &current,
                            encoding,
                            &options,
                            settings.validation,
                        ) {
                            logger.send(lsp::MessageType::ERROR, e.message);
                        }
                    }
                    SessionEvent::Published { .. } => {}
                    SessionEvent::DiagnosticsRefresh { cancellation } => {
                        if !cancellation.is_canceled() {
                            if let Err(e) = refresh_diagnostics(client.as_ref(), &caps) {
                                logger.send(lsp::MessageType::ERROR, e.message);
                            }
                        }
                    }
                    SessionEvent::Closed => break,
                }
            }
        }));
    }
    fn apply_settings(&mut self, values: &lsp::Any) -> Result<(), lsp::ResponseError> {
        let before = self.settings.lock().unwrap().clone();
        let mut next = Settings {
            locale: before.locale.clone(),
            ..Default::default()
        };
        if let lsp::Any::Object(sections) = values {
            for section in ["javascript", "typescript", "js/ts"] {
                if let Some(lsp::Any::Object(fields)) = sections.get(section) {
                    for raw in [Some(fields), fields.get("unstable").and_then(object)]
                        .into_iter()
                        .flatten()
                    {
                        apply_lens_preferences(raw, true, &mut next.code_lens);
                        apply_inlay_preferences(raw, true, &mut next.inlay, &mut next.inlay_flags);
                        set_bool(raw.get("validateEnabled"), &mut next.validation);
                        if let Some(lsp::Any::Number(length)) = raw.get("maximumHoverLength") {
                            next.maximum_hover_length =
                                if *length > 0.0 { *length as usize } else { 500 };
                        }
                        set_bool(
                            raw.get("excludeLibrarySymbolsInNavTo"),
                            &mut next.exclude_library_symbols,
                        );
                        if let Some(lsp::Any::String(scope)) = raw.get("workspaceSymbolsScope") {
                            next.workspace_current_project = scope == "currentProject";
                        }
                        set_bool(
                            raw.get("reportStyleChecksAsWarnings"),
                            &mut next.style_warnings,
                        );
                        if let Some(lsp::Any::String(name)) = raw.get("customConfigFileName") {
                            next.config_name.clone_from(name);
                        }
                    }
                    apply_lens_preferences(fields, false, &mut next.code_lens);
                    apply_inlay_preferences(fields, false, &mut next.inlay, &mut next.inlay_flags);
                    set_bool(
                        nested(fields, "validate.enabled")
                            .or_else(|| nested(fields, "validate.enable")),
                        &mut next.validation,
                    );
                    set_bool(
                        nested(fields, "workspaceSymbols.excludeLibrarySymbols"),
                        &mut next.exclude_library_symbols,
                    );
                    if let Some(lsp::Any::String(scope)) = nested(fields, "workspaceSymbols.scope")
                    {
                        next.workspace_current_project = scope == "currentProject";
                    }
                    set_bool(
                        fields.get("reportStyleChecksAsWarnings"),
                        &mut next.style_warnings,
                    );
                    if let Some(lsp::Any::String(name)) = fields.get("customConfigFileName") {
                        next.config_name.clone_from(name);
                    }
                    if let Some(lsp::Any::String(locale)) = fields.get("locale") {
                        next.locale = if locale == "auto" {
                            self.initialize
                                .as_ref()
                                .and_then(|p| p.locale.as_deref())
                                .map(|l| tsr_locale::Locale::parse(l).0)
                                .unwrap_or_default()
                        } else {
                            let (locale, valid) = tsr_locale::Locale::parse(locale);
                            if valid {
                                locale
                            } else {
                                next.locale
                            }
                        };
                    }
                    next.config_name = next.config_name.trim().to_owned();
                    if next.config_name.contains(['/', '\\'])
                        || matches!(next.config_name.as_str(), "." | "..")
                    {
                        next.config_name.clear();
                    }
                }
            }
        }
        *self.settings.lock().unwrap() = next.clone();
        if (next.inlay_flags != before.inlay_flags
            || next.inlay.parameter_names != before.inlay.parameter_names)
            && self
                .capabilities
                .workspace
                .as_deref()
                .and_then(|w| w.inlay_hint.as_deref())
                .and_then(|i| i.refresh_support.as_deref())
                .copied()
                .unwrap_or(false)
        {
            if let Err(e) = self
                .client
                .request_without_waiting("workspace/inlayHint/refresh", RawValue(b"null".to_vec()))
            {
                self.logger.send(lsp::MessageType::ERROR, e.message);
            }
        }
        if next.code_lens != before.code_lens
            && self
                .capabilities
                .workspace
                .as_deref()
                .and_then(|w| w.code_lens.as_deref())
                .and_then(|c| c.refresh_support.as_deref())
                .copied()
                .unwrap_or(false)
        {
            if let Err(e) = self
                .client
                .request_without_waiting("workspace/codeLens/refresh", RawValue(b"null".to_vec()))
            {
                self.logger.send(lsp::MessageType::ERROR, e.message);
            }
        }
        if next.config_name != before.config_name {
            self.server
                .as_ref()
                .unwrap()
                .session()
                .set_custom_config_file_name(JsString::from_bytes(next.config_name.as_bytes()))
                .map_err(crate::project_error)?;
        }
        if next.validation != before.validation
            || next.style_warnings != before.style_warnings
            || next.config_name != before.config_name
        {
            refresh_diagnostics(self.client.as_ref(), &self.capabilities)?;
            if next.validation != before.validation
                && !self
                    .initialization
                    .disable_push_diagnostics
                    .as_deref()
                    .copied()
                    .unwrap_or(false)
            {
                let snapshot = self
                    .server
                    .as_ref()
                    .unwrap()
                    .session()
                    .snapshot()
                    .map_err(crate::project_error)?;
                let options = diagnostics::options(&self.capabilities, next.locale, false, false);
                let open = diagnostics::open_projects(&snapshot);
                for project in snapshot.projects() {
                    if project.data().unwrap().kind == tsr_project::project::ProjectKind::Configured
                        && open.contains(project.data().unwrap().path.as_bytes())
                    {
                        diagnostics::publish_project(
                            self.client.as_ref(),
                            project,
                            self.options.project.position_encoding,
                            &options,
                            next.validation,
                        )?;
                    }
                }
            }
        }
        Ok(())
    }
    pub fn close(&mut self) {
        if let Some(watcher) = self.native_watcher.take() {
            watcher.close();
        }
        if let Some(server) = self.server.take() {
            server.close();
        }
        if let Some(observer) = self.observer.take() {
            let _ = observer.join();
        }
    }
}
impl Drop for Runtime {
    fn drop(&mut self) {
        self.close();
    }
}
fn object(value: &lsp::Any) -> Option<&HashMap<String, lsp::Any>> {
    if let lsp::Any::Object(value) = value {
        Some(value)
    } else {
        None
    }
}
fn nested<'a>(fields: &'a HashMap<String, lsp::Any>, path: &str) -> Option<&'a lsp::Any> {
    let mut parts = path.split('.');
    let mut value = fields.get(parts.next()?)?;
    for part in parts {
        value = object(value)?.get(part)?;
    }
    Some(value)
}
fn set_bool(value: Option<&lsp::Any>, target: &mut bool) {
    if let Some(lsp::Any::Boolean(value)) = value {
        *target = *value;
    }
}

// The two user-preference forms share one mapping: unstable/raw fields are
// applied first, then the editor's nested configuration takes precedence.
fn apply_inlay_preferences(
    fields: &HashMap<String, lsp::Any>,
    raw: bool,
    options: &mut tsr_ls::InlayHintsOptions,
    states: &mut [Option<bool>; 7],
) {
    let get = |name, path| {
        if raw {
            fields.get(name)
        } else {
            nested(fields, path)
        }
    };
    if let Some(lsp::Any::String(value)) = get(
        "includeInlayParameterNameHints",
        "inlayHints.parameterNames.enabled",
    ) {
        options.parameter_names = match value.as_str() {
            "all" => tsr_ls::ParameterNameHints::All,
            "literals" => tsr_ls::ParameterNameHints::Literals,
            _ => tsr_ls::ParameterNameHints::None,
        };
    }
    if let Some(lsp::Any::String(value)) = get("quotePreference", "preferences.quoteStyle") {
        options.quote = match value.as_str() {
            "single" => tsr_ls::QuotePreference::Single,
            "double" => tsr_ls::QuotePreference::Double,
            _ => tsr_ls::QuotePreference::Auto,
        };
    }
    for (index, (name, path, invert, target)) in [
        (
            "includeInlayParameterNameHintsWhenArgumentMatchesName",
            "inlayHints.parameterNames.suppressWhenArgumentMatchesName",
            true,
            &mut options.parameter_names_when_matching,
        ),
        (
            "includeInlayFunctionParameterTypeHints",
            "inlayHints.parameterTypes.enabled",
            false,
            &mut options.parameter_types,
        ),
        (
            "includeInlayVariableTypeHints",
            "inlayHints.variableTypes.enabled",
            false,
            &mut options.variable_types,
        ),
        (
            "includeInlayVariableTypeHintsWhenTypeMatchesName",
            "inlayHints.variableTypes.suppressWhenTypeMatchesName",
            true,
            &mut options.variable_types_when_matching,
        ),
        (
            "includeInlayPropertyDeclarationTypeHints",
            "inlayHints.propertyDeclarationTypes.enabled",
            false,
            &mut options.property_types,
        ),
        (
            "includeInlayFunctionLikeReturnTypeHints",
            "inlayHints.functionLikeReturnTypes.enabled",
            false,
            &mut options.return_types,
        ),
        (
            "includeInlayEnumMemberValueHints",
            "inlayHints.enumMemberValues.enabled",
            false,
            &mut options.enum_values,
        ),
    ]
    .into_iter()
    .enumerate()
    {
        if let Some(lsp::Any::Boolean(value)) = get(name, path) {
            *target = if !raw && invert { !value } else { *value };
            states[index] = Some(*target);
        }
    }
}

struct RemoteWatch(Arc<dyn Client>);
impl tsr_project::watch::WatchClient for RemoteWatch {
    fn watch_files(
        &self,
        context: &Context,
        id: &JsString,
        watcher: &tsr_project::watch::Watcher,
    ) -> Result<(), String> {
        let watcher = watcher.to_protocol().map_err(|e| e.to_string())?;
        let params = lsp::RegistrationParams {
            registrations: vec![Some(Box::new(lsp::Registration {
                id: std::str::from_utf8(id.as_bytes())
                    .map_err(|e| e.to_string())?
                    .to_owned(),
                register_options: Some(Box::new(lsp::RegisterOptions {
                    workspace_did_change_watched_files: Some(Box::new(
                        lsp::DidChangeWatchedFilesRegistrationOptions {
                            watchers: vec![Some(Box::new(watcher))],
                        },
                    )),
                    ..Default::default()
                })),
            }))],
        };
        self.0
            .request(
                context,
                "client/registerCapability",
                client::raw(&params).map_err(|e| e.message)?,
            )
            .map(|_| ())
            .map_err(|e| e.message)
    }
    fn unwatch_files(&self, context: &Context, id: &JsString) -> Result<(), String> {
        let params = lsp::UnregistrationParams {
            unregisterations: vec![Some(Box::new(lsp::Unregistration {
                id: std::str::from_utf8(id.as_bytes())
                    .map_err(|e| e.to_string())?
                    .to_owned(),
                method: "workspace/didChangeWatchedFiles".into(),
            }))],
        };
        self.0
            .request(
                context,
                "client/unregisterCapability",
                client::raw(&params).map_err(|e| e.message)?,
            )
            .map(|_| ())
            .map_err(|e| e.message)
    }
}

fn wire_string(bytes: &[u8]) -> Result<String, lsp::ResponseError> {
    std::str::from_utf8(bytes)
        .map(str::to_owned)
        .map_err(|_| crate::invalid("invalid UTF-8 cannot cross the LSP transport (ADR 0019)"))
}

// port: tsc/internal/lsp/server.go:Server.RefreshDiagnostics
fn refresh_diagnostics(
    client: &dyn Client,
    caps: &lsp::ClientCapabilities,
) -> Result<(), lsp::ResponseError> {
    if caps
        .workspace
        .as_deref()
        .and_then(|w| w.diagnostics.as_deref())
        .and_then(|d| d.refresh_support.as_deref())
        .copied()
        .unwrap_or(false)
    {
        client
            .request_without_waiting("workspace/diagnostic/refresh", RawValue(b"null".to_vec()))?;
    }
    Ok(())
}

// Remaining entries in the pin's server.go handlers map. Only these receive
// the temporary MethodNotFound refusal; a genuinely unknown method is an
// InvalidRequest. Keep this list shrinking as L3–L6 install their handlers.
fn unimplemented_method(method: &str) -> bool {
    matches!(
        method,
        "workspace/willRenameFiles"
            | "textDocument/hover"
            | "textDocument/definition"
            | "custom/textDocument/sourceDefinition"
            | "textDocument/typeDefinition"
            | "textDocument/signatureHelp"
            | "textDocument/formatting"
            | "textDocument/rangeFormatting"
            | "textDocument/onTypeFormatting"
            | "textDocument/documentSymbol"
            | "textDocument/documentHighlight"
            | "custom/textDocument/multiDocumentHighlight"
            | "textDocument/selectionRange"
            | "textDocument/inlayHint"
            | "textDocument/codeLens"
            | "textDocument/codeAction"
            | "textDocument/prepareCallHierarchy"
            | "textDocument/foldingRange"
            | "textDocument/prepareRename"
            | "textDocument/linkedEditingRange"
            | "textDocument/completion"
            | "textDocument/_vs_onAutoInsert"
            | "textDocument/references"
            | "textDocument/_vs_references"
            | "textDocument/rename"
            | "textDocument/implementation"
            | "callHierarchy/incomingCalls"
            | "callHierarchy/outgoingCalls"
            | "workspace/symbol"
            | "completionItem/resolve"
            | "codeLens/resolve"
            | "textDocument/semanticTokens/full"
            | "textDocument/semanticTokens/range"
            | "custom/runGC"
            | "custom/saveHeapProfile"
            | "custom/saveAllocProfile"
            | "custom/startCPUProfile"
            | "custom/stopCPUProfile"
            | "custom/initializeAPISession"
            | "custom/setContentMapperContributions"
    )
}

fn apply_lens_preferences(
    fields: &HashMap<String, lsp::Any>,
    raw: bool,
    options: &mut tsr_ls::CodeLensOptions,
) {
    for (name, path, target) in [
        (
            "referencesCodeLensEnabled",
            "referencesCodeLens.enabled",
            &mut options.references,
        ),
        (
            "implementationsCodeLensEnabled",
            "implementationsCodeLens.enabled",
            &mut options.implementations,
        ),
        (
            "referencesCodeLensShowOnAllFunctions",
            "referencesCodeLens.showOnAllFunctions",
            &mut options.all_functions,
        ),
        (
            "implementationsCodeLensShowOnInterfaceMethods",
            "implementationsCodeLens.showOnInterfaceMethods",
            &mut options.interface_methods,
        ),
        (
            "implementationsCodeLensShowOnAllClassMethods",
            "implementationsCodeLens.showOnAllClassMethods",
            &mut options.all_class_methods,
        ),
    ] {
        if let Some(lsp::Any::Boolean(value)) = if raw {
            fields.get(name)
        } else {
            nested(fields, path)
        } {
            *target = Some(*value);
        }
    }
}
