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
    rename: tsr_ls::RenameOptions,
    organize: tsr_ls::OrganizeOptions,
    formatting: bool,
    completion: tsr_ls::CompletionOptions,
    auto_closing_tags: bool,
    locale: tsr_locale::Locale,
    validation: bool,
    disable_automatic_type_acquisition: bool,
    style_warnings: bool,
    config_name: String,
    exclude_library_symbols: bool,
    workspace_current_project: bool,
    maximum_hover_length: usize,
    prefer_source_definition: bool,
    inlay: tsr_ls::InlayHintsOptions,
    inlay_flags: [Option<bool>; 7],
    code_lens: tsr_ls::CodeLensOptions,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            locale: tsr_locale::Locale::default(),
            validation: true,
            disable_automatic_type_acquisition: false,
            formatting: true,
            rename: tsr_ls::RenameOptions::default(),
            organize: tsr_ls::OrganizeOptions::default(),
            style_warnings: true,
            config_name: String::new(),
            exclude_library_symbols: true,
            workspace_current_project: false,
            completion: tsr_ls::CompletionOptions::default(),
            auto_closing_tags: true,
            maximum_hover_length: 500,
            prefer_source_definition: false,
            inlay: tsr_ls::InlayHintsOptions::default(),
            inlay_flags: [None; 7],
            code_lens: tsr_ls::CodeLensOptions::default(),
        }
    }
}
pub struct Runtime {
    mapper_registrations: Arc<crate::mapper_registrations::MapperRegistrations>,
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
            mapper_registrations: Arc::default(),
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
            "custom/setContentMapperContributions" => {
                let params: lsp::SetContentMapperContributionsParams = crate::decode(params)?;
                let parsed = crate::content_mappers::parse(&params.contributions)
                    .map_err(|e| crate::invalid(&e))?;
                self.ready()?
                    .session()
                    .set_content_mapper_contributions(
                        tsr_project::content_mappers::Contributions {
                            mappers: parsed.mappers,
                            extensions: parsed.extensions,
                        },
                        params
                            .open_documents
                            .into_iter()
                            .map(|document| document.uri)
                            .collect(),
                    )
                    .map_err(crate::project_error)?;
                // The contribution update succeeds independently of a client
                // refusing dynamic registration, as in the pinned server.
                let _ = self.mapper_registrations.update(
                    self.client.as_ref(),
                    context,
                    &self.capabilities,
                    &self
                        .ready()?
                        .session()
                        .snapshot()
                        .map_err(crate::project_error)?,
                );
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
            "workspace/willRenameFiles" => {
                let params: lsp::RenameFilesParams = crate::decode(params)?;
                if params.files.is_empty() {
                    return client::raw(&lsp::Null).map(Dispatch::Ready);
                }
                let snapshot = self
                    .ready()?
                    .session()
                    .flush_resources(
                        &tsr_project::api::ResourceRequest {
                            documents: params
                                .files
                                .iter()
                                .flatten()
                                .map(|f| lsp::DocumentUri(f.old_uri.clone()))
                                .collect(),
                            project_tree: Some(tsr_project::api::ProjectTreeRequest::All),
                            ..Default::default()
                        },
                        host,
                    )
                    .map_err(crate::project_error)?;
                let context = context.clone();
                let capabilities = self.capabilities.clone();
                let encoding = self.options.project.position_encoding;
                let options = self.settings.lock().unwrap().completion.clone();
                let request_id = request
                    .id
                    .as_ref()
                    .map(ToString::to_string)
                    .unwrap_or_default();
                return Ok(Dispatch::Work(Box::new(move || {
                    crate::language_features::file_renames(
                        &context,
                        &request_id,
                        &snapshot,
                        &params,
                        encoding,
                        &capabilities,
                        &options,
                    )
                })));
            }
            _ if crate::language_features::handles(method) => {
                let feature = crate::language_features::Request::decode(method, params)?;
                let uri = feature.uri();
                let session = self.ready()?.session().clone();
                let snapshot = session
                    .flush_with_host(Some(uri), host.clone())
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
                    rename: settings.rename,
                    organize: settings.organize.clone(),
                    formatting: settings.formatting,
                    completion: settings.completion,
                    auto_closing_tags: settings.auto_closing_tags,
                    maximum_hover_length: settings.maximum_hover_length,
                    prefer_source_definition: settings.prefer_source_definition,
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
                let sync_imports = matches!(
                    &feature,
                    crate::language_features::Request::Completion(_)
                        | crate::language_features::Request::ResolveCompletion(_, _)
                );
                return Ok(Dispatch::Work(Box::new(move || {
                    let result = if feature.crosses_projects() && project.is_some() {
                        crate::crossproject::execute(
                            &context,
                            &request_id,
                            &session,
                            &host,
                            &snapshot,
                            &feature,
                            encoding,
                            &capabilities,
                            &options,
                        )
                    } else {
                        crate::language_features::execute(
                            &context,
                            &request_id,
                            project.as_ref(),
                            feature,
                            encoding,
                            &capabilities,
                            &options,
                        )
                    };
                    if sync_imports {
                        session.sync_auto_import_watches();
                    }
                    result
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
        self.options.project.locale = locale.clone();
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
        let logger = self.logger.clone();
        self.options.project.mapper_logger = Some(Arc::new(move |message| {
            if logger.is_tracing() {
                logger.send(lsp::MessageType::INFO, message);
            }
        }));
        self.options.project.relative_watch_patterns = workspace
            .and_then(|w| w.did_change_watched_files.as_deref())
            .and_then(|w| w.relative_pattern_support.as_deref())
            .copied()
            .unwrap_or(false);
        self.options.project.background_context = self.context.clone();
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
        let context = self.context.clone();
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
        let mapper_registrations = self.mapper_registrations.clone();
        self.observer = Some(std::thread::spawn(move || {
            while let Ok(event) = receive.recv() {
                let settings = settings.lock().unwrap().clone();
                if let SessionEvent::Published { current, .. } = &event {
                    let _ = mapper_registrations.update(client.as_ref(), &context, &caps, current);
                }
                match event {
                    event @ (SessionEvent::ProjectLoading { .. }
                    | SessionEvent::InstallingTypes { .. }) => {
                        if let Some(progress) = &progress {
                            let (name, finished, diagnostic) = match event {
                                SessionEvent::ProjectLoading { name, finished } => {
                                    (name, finished, tsr_diagnostics::Project_0)
                                }
                                SessionEvent::InstallingTypes { name, finished } => {
                                    (name, finished, tsr_diagnostics::Installing_types_for_0)
                                }
                                _ => unreachable!(),
                            };
                            let message = diagnostic.localize(
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
        let mut deprecated_disable_ata = None;
        let mut enable_ata = None;
        if let lsp::Any::Object(sections) = values {
            if let Some(lsp::Any::Object(editor)) = sections.get("editor") {
                set_ata_preference(
                    editor.get("disableAutomaticTypeAcquisition"),
                    &mut deprecated_disable_ata,
                );
                set_ata_preference(
                    editor.get("automaticTypeAcquisitionEnabled"),
                    &mut enable_ata,
                );
                let mut editor = editor.clone();
                if !editor.contains_key("indentSize") {
                    if let Some(value) = editor.get("tabSize").cloned() {
                        editor.insert("indentSize".into(), value);
                    }
                }
                if !editor.contains_key("convertTabsToSpaces") {
                    if let Some(value) = editor.get("insertSpaces").cloned() {
                        editor.insert("convertTabsToSpaces".into(), value);
                    }
                }
                tsr_ls::apply_format_settings(&editor, true, &mut next.completion.format);
                if let Some(lsp::Any::String(value)) = editor.get("newLineCharacter") {
                    next.completion.newline = Some(value.clone());
                }
            }
            for section in ["javascript", "typescript", "js/ts"] {
                if let Some(lsp::Any::Object(fields)) = sections.get(section) {
                    for raw in [Some(fields), fields.get("unstable").and_then(object)]
                        .into_iter()
                        .flatten()
                    {
                        set_ata_preference(
                            raw.get("disableAutomaticTypeAcquisition"),
                            &mut deprecated_disable_ata,
                        );
                        set_ata_preference(
                            raw.get("automaticTypeAcquisitionEnabled"),
                            &mut enable_ata,
                        );
                        set_bool(
                            raw.get("preferGoToSourceDefinition"),
                            &mut next.prefer_source_definition,
                        );
                        apply_lens_preferences(raw, true, &mut next.code_lens);
                        apply_completion_preferences(
                            raw,
                            true,
                            &mut next.completion,
                            &mut next.auto_closing_tags,
                        );
                        apply_inlay_preferences(raw, true, &mut next.inlay, &mut next.inlay_flags);
                        set_bool(raw.get("validateEnabled"), &mut next.validation);
                        set_bool(raw.get("formatEnabled"), &mut next.formatting);
                        apply_organize_preferences(raw, true, &mut next.organize);
                        set_bool(
                            raw.get("providePrefixAndSuffixTextForRename"),
                            &mut next.rename.aliases,
                        );
                        set_bool(
                            raw.get("allowRenameOfImportPath"),
                            &mut next.rename.import_paths,
                        );
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
                    set_ata_preference(
                        fields.get("disableAutomaticTypeAcquisition"),
                        &mut deprecated_disable_ata,
                    );
                    set_ata_preference(
                        nested(fields, "tsserver.automaticTypeAcquisition.enabled"),
                        &mut enable_ata,
                    );
                    set_bool(
                        nested(fields, "preferGoToSourceDefinition"),
                        &mut next.prefer_source_definition,
                    );
                    set_bool(
                        nested(fields, "format.enabled")
                            .or_else(|| nested(fields, "format.enable")),
                        &mut next.formatting,
                    );
                    set_bool(
                        nested(fields, "preferences.useAliasesForRenames"),
                        &mut next.rename.aliases,
                    );
                    apply_lens_preferences(fields, false, &mut next.code_lens);
                    apply_organize_preferences(fields, false, &mut next.organize);
                    apply_completion_preferences(
                        fields,
                        false,
                        &mut next.completion,
                        &mut next.auto_closing_tags,
                    );
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
        next.disable_automatic_type_acquisition =
            enable_ata.map_or(deprecated_disable_ata.unwrap_or(false), |value| !value);
        if next.disable_automatic_type_acquisition != before.disable_automatic_type_acquisition {
            if let Some(server) = &self.server {
                server
                    .session()
                    .set_disable_automatic_type_acquisition(next.disable_automatic_type_acquisition)
                    .map_err(crate::project_error)?;
            }
        }
        next.completion.locale = next.locale.clone();
        if let Some(server) = &self.server {
            server.session().set_locale(next.locale.clone());
        }
        if next.validation != before.validation {
            self.server
                .as_ref()
                .unwrap()
                .session()
                .set_validation_enabled(next.validation)
                .map_err(crate::project_error)?;
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

fn set_ata_preference(value: Option<&lsp::Any>, target: &mut Option<bool>) {
    // The pin's tristate parser ignores null but resets other invalid values
    // to Unknown, allowing the deprecated setting or default to take effect.
    match value {
        Some(lsp::Any::Boolean(value)) => *target = Some(*value),
        None | Some(lsp::Any::Null) => {}
        Some(_) => *target = None,
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
        options.quote = parse_quote_preference(value);
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
        "custom/runGC"
            | "custom/saveHeapProfile"
            | "custom/saveAllocProfile"
            | "custom/startCPUProfile"
            | "custom/stopCPUProfile"
            | "custom/initializeAPISession"
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

fn parse_quote_preference(value: &str) -> tsr_ls::QuotePreference {
    match tsr_jsstring::helpers::to_lower_go(value.as_bytes()).as_slice() {
        b"single" => tsr_ls::QuotePreference::Single,
        b"double" => tsr_ls::QuotePreference::Double,
        _ => tsr_ls::QuotePreference::Auto,
    }
}

fn apply_completion_preferences(
    fields: &HashMap<String, lsp::Any>,
    raw: bool,
    options: &mut tsr_ls::CompletionOptions,
    auto_closing: &mut bool,
) {
    tsr_ls::apply_format_settings(fields, raw, &mut options.format);
    let get = |name, path| {
        if raw {
            fields.get(name)
        } else {
            nested(fields, path)
        }
    };
    for (name, path, target) in [
        (
            "includeCompletionsForModuleExports",
            "suggest.autoImports",
            &mut options.module_exports,
        ),
        (
            "includeAutomaticOptionalChainCompletions",
            "suggest.includeAutomaticOptionalChainCompletions",
            &mut options.automatic_optional_chain,
        ),
        (
            "completeJSDocs",
            "suggest.jsdoc.enabled",
            &mut options.enable_jsdoc,
        ),
        (
            "generateReturnInDocTemplate",
            "suggest.jsdoc.generateReturns",
            &mut options.generate_return,
        ),
    ] {
        if let Some(lsp::Any::Boolean(value)) = get(name, path) {
            *target = Some(*value);
        }
    }
    set_bool(
        get(
            "preferTypeOnlyAutoImports",
            "preferences.preferTypeOnlyAutoImports",
        ),
        &mut options.prefer_type_only,
    );
    set_bool(
        get("autoClosingTags", "autoClosingTags.enabled")
            .or_else(|| (!raw).then(|| fields.get("autoClosingTags")).flatten()),
        auto_closing,
    );
    if !raw && nested(fields, "suggest.jsdoc.enabled").is_none() {
        if let Some(lsp::Any::Boolean(value)) = nested(fields, "suggest.completeJSDocs") {
            options.enable_jsdoc = Some(*value);
        }
    }
    if let Some(lsp::Any::String(value)) = get("quotePreference", "preferences.quoteStyle") {
        options.quote = parse_quote_preference(value);
    }
    if let Some(lsp::Any::String(value)) = get(
        "jsxAttributeCompletionStyle",
        "preferences.jsxAttributeCompletionStyle",
    ) {
        options.jsx_attribute_style = Some(match value.as_str() {
            "braces" | "none" => value.clone(),
            _ => "auto".into(),
        });
    }
    if let Some(lsp::Any::String(value)) = get(
        "importModuleSpecifierEnding",
        "preferences.importModuleSpecifierEnding",
    ) {
        options.auto_import.ending = Some(
            match tsr_jsstring::helpers::to_lower_go(value.as_bytes()).as_slice() {
                b"minimal" => "minimal",
                b"index" => "index",
                b"js" => "js",
                _ => "auto",
            }
            .into(),
        );
    }
    if let Some(lsp::Any::String(value)) = get(
        "importModuleSpecifierPreference",
        "preferences.importModuleSpecifier",
    ) {
        options.auto_import.module_specifier = Some(
            match tsr_jsstring::helpers::to_lower_go(value.as_bytes()).as_slice() {
                b"project-relative" => "project-relative",
                b"relative" => "relative",
                b"non-relative" => "non-relative",
                _ => "shortest",
            }
            .into(),
        );
    }
    if let Some(lsp::Any::Boolean(value)) = get(
        "autoImportEntrypointDirectorySearch",
        "preferences.autoImportEntrypointDirectorySearch",
    ) {
        options.auto_import.directory_search = Some(*value);
    }
    if let Some(lsp::Any::Boolean(value)) = get(
        "includeCompletionsForImportStatements",
        "suggest.includeCompletionsForImportStatements",
    ) {
        options.import_statements = Some(*value);
    }
    set_bool(
        get(
            "includeCompletionsWithClassMemberSnippets",
            "suggest.classMemberSnippets.enabled",
        ),
        &mut options.class_member_snippets,
    );
    set_bool(
        get(
            "includeCompletionsWithObjectLiteralMethodSnippets",
            "suggest.objectLiteralMethodSnippets.enabled",
        ),
        &mut options.object_method_snippets,
    );
    if let Some(lsp::Any::Array(values)) = get(
        "autoImportSpecifierExcludeRegexes",
        "preferences.autoImportSpecifierExcludeRegexes",
    ) {
        options.auto_import.exclude_specifiers = values
            .iter()
            .filter_map(|v| match v {
                lsp::Any::String(s) => Some(s.clone()),
                _ => None,
            })
            .collect();
    }
    if let Some(lsp::Any::Array(values)) = get(
        "autoImportFileExcludePatterns",
        "preferences.autoImportFileExcludePatterns",
    ) {
        options.auto_import.exclude_files = values
            .iter()
            .filter_map(|v| match v {
                lsp::Any::String(s) => Some(JsString::from_bytes(s.as_bytes())),
                _ => None,
            })
            .collect();
    }
    if let Some(lsp::Any::String(value)) = get("newLineCharacter", "format.newLineCharacter") {
        options.newline = Some(value.clone());
    }
}

fn apply_organize_preferences(
    fields: &HashMap<String, lsp::Any>,
    raw: bool,
    options: &mut tsr_ls::OrganizeOptions,
) {
    let get = |key, path| {
        if raw {
            fields.get(key)
        } else {
            nested(fields, path)
        }
    };
    for (key, path, output) in [
        (
            "organizeImportsSort",
            "preferences.organizeImports.sort",
            &mut options.sort,
        ),
        (
            "organizeImportsCaseFirst",
            "preferences.organizeImports.caseFirst",
            &mut options.case_first,
        ),
        (
            "organizeImportsTypeOrder",
            "preferences.organizeImports.typeOrder",
            &mut options.type_order,
        ),
    ] {
        if let Some(lsp::Any::String(value)) = get(key, path) {
            output.clone_from(value);
        }
    }
    if let Some(lsp::Any::String(value)) = get(
        "organizeImportsCollation",
        "preferences.organizeImports.unicodeCollation",
    ) {
        options.unicode = tsr_jsstring::helpers::to_lower_go(value.as_bytes()) == b"unicode";
    }
    match get(
        "organizeImportsIgnoreCase",
        "preferences.organizeImports.caseSensitivity",
    ) {
        Some(lsp::Any::Boolean(value)) => options.ignore_case = Some(*value),
        Some(lsp::Any::String(value)) if !raw => {
            options.ignore_case =
                match tsr_jsstring::helpers::to_lower_go(value.as_bytes()).as_slice() {
                    b"caseinsensitive" => Some(true),
                    b"casesensitive" => Some(false),
                    _ => None,
                }
        }
        _ => {}
    }
    if let Some(lsp::Any::Boolean(value)) = get(
        "organizeImportsAccentCollation",
        "preferences.organizeImports.accentCollation",
    ) {
        options.accents = Some(*value);
    }
    set_bool(
        get(
            "organizeImportsNumericCollation",
            "preferences.organizeImports.numericCollation",
        ),
        &mut options.numeric,
    );
}

#[cfg(test)]
mod preference_tests {
    use super::*;

    #[test]
    fn automatic_type_acquisition_configuration_preserves_unified_precedence() {
        let context = Context::background();
        let client = crate::rpc_client::RpcClient::new(context.clone(), Arc::new(|_| Ok(())));
        let fs = Arc::new(tsr_vfs::MemoryBuilder::new(b"/", true).finish());
        let mut runtime = Runtime::new(
            Options::new(SessionOptions::default(), fs),
            context,
            client,
            Box::new(std::io::sink()),
        );
        // Matches ParseUserPreferences: explicit unified enablement wins over
        // the deprecated inverse flag, including across configuration sections.
        for (json, disabled) in [
            (
                r#"{"typescript":{"disableAutomaticTypeAcquisition":true}}"#,
                true,
            ),
            (
                r#"{"js/ts":{"tsserver":{"automaticTypeAcquisition":{"enabled":false}}}}"#,
                true,
            ),
            (
                r#"{"typescript":{"disableAutomaticTypeAcquisition":true},"js/ts":{"tsserver":{"automaticTypeAcquisition":{"enabled":true}}}}"#,
                false,
            ),
            (
                r#"{"js/ts":{"unstable":{"automaticTypeAcquisitionEnabled":false},"tsserver":{"automaticTypeAcquisition":{"enabled":true}}}}"#,
                false,
            ),
            (
                r#"{"js/ts":{"automaticTypeAcquisitionEnabled":false,"disableAutomaticTypeAcquisition":false}}"#,
                true,
            ),
            (
                r#"{"js/ts":{"automaticTypeAcquisitionEnabled":true,"disableAutomaticTypeAcquisition":true}}"#,
                false,
            ),
            (
                r#"{"editor":{"automaticTypeAcquisitionEnabled":false}}"#,
                true,
            ),
            (
                r#"{"editor":{"disableAutomaticTypeAcquisition":true},"javascript":{"disableAutomaticTypeAcquisition":false}}"#,
                false,
            ),
            (
                r#"{"javascript":{"automaticTypeAcquisitionEnabled":false},"js/ts":{"automaticTypeAcquisitionEnabled":"invalid"}}"#,
                false,
            ),
            (
                r#"{"js/ts":{"automaticTypeAcquisitionEnabled":false,"unstable":{"automaticTypeAcquisitionEnabled":7}}}"#,
                false,
            ),
            (
                r#"{"typescript":{"disableAutomaticTypeAcquisition":true},"js/ts":{"automaticTypeAcquisitionEnabled":true,"tsserver":{"automaticTypeAcquisition":{"enabled":[]}}}}"#,
                true,
            ),
            (
                r#"{"javascript":{"disableAutomaticTypeAcquisition":true},"js/ts":{"disableAutomaticTypeAcquisition":{}}}"#,
                false,
            ),
            (
                r#"{"javascript":{"automaticTypeAcquisitionEnabled":false},"js/ts":{"automaticTypeAcquisitionEnabled":null}}"#,
                true,
            ),
            (
                r#"{"js/ts":{"automaticTypeAcquisitionEnabled":false,"tsserver":{"automaticTypeAcquisition":{"enabled":null}}}}"#,
                true,
            ),
            (
                r#"{"typescript":{"disableAutomaticTypeAcquisition":true},"js/ts":{"disableAutomaticTypeAcquisition":null}}"#,
                true,
            ),
            (
                r#"{"typescript":{"disableAutomaticTypeAcquisition":true}}"#,
                true,
            ),
            ("{}", false),
        ] {
            let mut value = lsp::Any::default();
            tsr_json::unmarshal(json.as_bytes(), &mut value, tsr_json::Options::default()).unwrap();
            runtime.apply_settings(&value).unwrap();
            assert_eq!(
                runtime
                    .settings
                    .lock()
                    .unwrap()
                    .disable_automatic_type_acquisition,
                disabled,
                "{json}"
            );
        }
    }

    #[test]
    fn quote_and_module_preferences_use_go_unicode_lowercasing() {
        use tsr_ls::QuotePreference::{Auto, Double, Single};
        for (quote, ending, relative, expected_quote, expected_ending, expected_relative) in [
            ("DoUbLe", "JS", "Relative", Double, "js", "relative"),
            (
                "SİNGLE",
                "MİNİMAL",
                "NON-RELATİVE",
                Single,
                "minimal",
                "non-relative",
            ),
            (
                "AUTO",
                "INDEX",
                "PROJECT-RELATIVE",
                Auto,
                "index",
                "project-relative",
            ),
            ("unknown", "unknown", "unknown", Auto, "auto", "shortest"),
        ] {
            for raw in [false, true] {
                let json = if raw {
                    format!(
                        r#"{{"quotePreference":"{quote}","importModuleSpecifierEnding":"{ending}","importModuleSpecifierPreference":"{relative}"}}"#
                    )
                } else {
                    format!(
                        r#"{{"preferences":{{"quoteStyle":"{quote}","importModuleSpecifierEnding":"{ending}","importModuleSpecifier":"{relative}"}}}}"#
                    )
                };
                let mut fields = HashMap::<String, lsp::Any>::new();
                tsr_json::unmarshal(json.as_bytes(), &mut fields, tsr_json::Options::default())
                    .unwrap();
                let mut options = tsr_ls::CompletionOptions::default();
                let mut auto_closing = false;
                apply_completion_preferences(&fields, raw, &mut options, &mut auto_closing);
                assert_eq!(options.quote, expected_quote);
                assert_eq!(options.auto_import.ending.as_deref(), Some(expected_ending));
                assert_eq!(
                    options.auto_import.module_specifier.as_deref(),
                    Some(expected_relative)
                );
                let mut hints = tsr_ls::InlayHintsOptions::default();
                apply_inlay_preferences(&fields, raw, &mut hints, &mut [None; 7]);
                assert_eq!(hints.quote, expected_quote);
            }
        }
    }

    #[test]
    fn organize_config_accepts_boolean_and_case_insensitive_values() {
        for (case, expected) in [
            ("true", Some(true)),
            ("false", Some(false)),
            ("\"CASEINSENSITIVE\"", Some(true)),
            ("\"caseİnsensitive\"", Some(true)),
            ("\"CASESENSITIVE\"", Some(false)),
            ("\"AUTO\"", None),
        ] {
            let json = format!(
                r#"{{"preferences":{{"organizeImports":{{"unicodeCollation":"UNİCODE","caseSensitivity":{case}}}}}}}"#
            );
            let mut fields = HashMap::<String, lsp::Any>::new();
            tsr_json::unmarshal(json.as_bytes(), &mut fields, tsr_json::Options::default())
                .unwrap();
            let mut options = tsr_ls::OrganizeOptions::default();
            apply_organize_preferences(&fields, false, &mut options);
            assert!(options.unicode);
            assert_eq!(options.ignore_case, expected);
        }
    }
}
