//! Request execution over the snapshot selected at dispatch. Parsing
//! and project updates happen before this boundary; worker work retains both.
use crate::{client, error};
use tsr_ipc::Context;
use tsr_json::RawValue;
use tsr_lsproto as lsp;

struct Stop(tsr_ipc::AfterFuncStop);
impl Drop for Stop {
    fn drop(&mut self) {
        self.0.stop();
    }
}

pub fn handles(method: &str) -> bool {
    matches!(
        method,
        "textDocument/_vs_onAutoInsert"
            | "textDocument/prepareRename"
            | "textDocument/codeAction"
            | "textDocument/rename"
            | "textDocument/formatting"
            | "textDocument/rangeFormatting"
            | "textDocument/onTypeFormatting"
            | "textDocument/completion"
            | "completionItem/resolve"
            | "textDocument/linkedEditingRange"
            | "textDocument/hover"
            | "textDocument/signatureHelp"
            | "textDocument/inlayHint"
            | "textDocument/codeLens"
            | "codeLens/resolve"
            | "textDocument/prepareCallHierarchy"
            | "callHierarchy/incomingCalls"
            | "callHierarchy/outgoingCalls"
            | "textDocument/documentHighlight"
            | "custom/textDocument/multiDocumentHighlight"
            | "textDocument/references"
            | "textDocument/_vs_references"
            | "textDocument/implementation"
            | "textDocument/selectionRange"
            | "textDocument/foldingRange"
            | "textDocument/semanticTokens/full"
            | "textDocument/semanticTokens/range"
            | "textDocument/documentSymbol"
            | "textDocument/definition"
            | "custom/textDocument/sourceDefinition"
            | "textDocument/typeDefinition"
    )
}
#[derive(Clone)]
pub enum Request {
    CodeActions(lsp::CodeActionParams),
    PrepareRename(lsp::PrepareRenameParams),
    Rename(lsp::RenameParams),
    Format(lsp::DocumentFormattingParams),
    FormatRange(lsp::DocumentRangeFormattingParams),
    FormatType(lsp::DocumentOnTypeFormattingParams),
    AutoInsert(lsp::VSOnAutoInsertParams),
    Completion(lsp::CompletionParams),
    ResolveCompletion(lsp::CompletionItem, lsp::DocumentUri),
    Hover(lsp::HoverParams),
    SignatureHelp(lsp::SignatureHelpParams),
    InlayHints(lsp::InlayHintParams),
    CodeLenses(lsp::CodeLensParams),
    ResolveLens(lsp::CodeLens),
    // Internal stages of cross-project requests, never wire methods.
    LensLocations(lsp::CodeLens),
    LensImplementations(lsp::ImplementationParams),
    IncomingPositions(Box<lsp::CallHierarchyItem>),
    IncomingAt(lsp::TextDocumentPositionParams),
    CallPrepare(lsp::CallHierarchyPrepareParams),
    CallIncoming(Box<lsp::CallHierarchyItem>),
    CallOutgoing(Box<lsp::CallHierarchyItem>),
    Highlights(lsp::DocumentHighlightParams),
    MultiHighlights(lsp::MultiDocumentHighlightParams),
    References(lsp::ReferenceParams),
    VSReferences(lsp::ReferenceParams),
    Implementation(lsp::ImplementationParams),
    Linked(lsp::LinkedEditingRangeParams),
    Selection(lsp::SelectionRangeParams),
    Folding(lsp::FoldingRangeParams),
    Semantic(lsp::SemanticTokensParams),
    SemanticRange(lsp::SemanticTokensRangeParams),
    DocumentSymbols(lsp::DocumentSymbolParams),
    Definition(lsp::DefinitionParams),
    SourceDefinition(lsp::DefinitionParams),
    TypeDefinition(lsp::TypeDefinitionParams),
}
impl Request {
    pub fn decode(method: &str, params: Option<&RawValue>) -> Result<Self, lsp::ResponseError> {
        // These pointer-valued protocol fields may decode as null, but the
        // formatter requires settings. Refuse before entering the service.
        fn format_options(
            options: Option<&lsp::FormattingOptions>,
        ) -> Result<(), lsp::ResponseError> {
            if options.is_none() {
                return Err(crate::coded_error(
                    lsp::ErrorCode::INVALID_PARAMS,
                    Some("missing formatting options"),
                ));
            }
            Ok(())
        }
        Ok(match method {
            "textDocument/codeAction" => Self::CodeActions(crate::decode(params)?),
            "textDocument/prepareRename" => Self::PrepareRename(crate::decode(params)?),
            "textDocument/rename" => Self::Rename(crate::decode(params)?),
            "textDocument/formatting" => {
                let p: lsp::DocumentFormattingParams = crate::decode(params)?;
                format_options(p.options.as_deref())?;
                Self::Format(p)
            }
            "textDocument/rangeFormatting" => {
                let p: lsp::DocumentRangeFormattingParams = crate::decode(params)?;
                format_options(p.options.as_deref())?;
                Self::FormatRange(p)
            }
            "textDocument/onTypeFormatting" => {
                let p: lsp::DocumentOnTypeFormattingParams = crate::decode(params)?;
                format_options(p.options.as_deref())?;
                Self::FormatType(p)
            }
            "textDocument/_vs_onAutoInsert" => Self::AutoInsert(crate::decode(params)?),
            "textDocument/completion" => Self::Completion(crate::decode(params)?),
            "completionItem/resolve" => {
                let item: lsp::CompletionItem = crate::decode(params)?;
                let data = item
                    .data
                    .as_deref()
                    .ok_or_else(|| error(-32603, "completion item data is nil"))?;
                let uri = lsp::DocumentUri::from_file_name(data.file_name.as_bytes());
                Self::ResolveCompletion(item, uri)
            }
            "textDocument/codeLens" => Self::CodeLenses(crate::decode(params)?),
            "codeLens/resolve" => {
                let lens: lsp::CodeLens = crate::decode(params)?;
                if lens.data.is_none() {
                    return Err(crate::coded_error(
                        lsp::ErrorCode::INVALID_PARAMS,
                        Some("missing code lens data"),
                    ));
                }
                Self::ResolveLens(lens)
            }
            "textDocument/prepareCallHierarchy" => Self::CallPrepare(crate::decode(params)?),
            "callHierarchy/incomingCalls" => Self::CallIncoming(
                crate::decode::<lsp::CallHierarchyIncomingCallsParams>(params)?
                    .item
                    .ok_or_else(|| {
                        crate::coded_error(
                            lsp::ErrorCode::INVALID_PARAMS,
                            Some("missing call hierarchy item"),
                        )
                    })?,
            ),
            "callHierarchy/outgoingCalls" => Self::CallOutgoing(
                crate::decode::<lsp::CallHierarchyOutgoingCallsParams>(params)?
                    .item
                    .ok_or_else(|| {
                        crate::coded_error(
                            lsp::ErrorCode::INVALID_PARAMS,
                            Some("missing call hierarchy item"),
                        )
                    })?,
            ),
            "textDocument/documentHighlight" => Self::Highlights(crate::decode(params)?),
            "custom/textDocument/multiDocumentHighlight" => {
                Self::MultiHighlights(crate::decode(params)?)
            }
            "textDocument/references" => Self::References(crate::decode(params)?),
            "textDocument/_vs_references" => Self::VSReferences(crate::decode(params)?),
            "textDocument/implementation" => Self::Implementation(crate::decode(params)?),
            "textDocument/inlayHint" => Self::InlayHints(crate::decode(params)?),
            "textDocument/signatureHelp" => Self::SignatureHelp(crate::decode(params)?),
            "textDocument/hover" => Self::Hover(crate::decode(params)?),
            "textDocument/linkedEditingRange" => Self::Linked(crate::decode(params)?),
            "textDocument/selectionRange" => Self::Selection(crate::decode(params)?),
            "textDocument/foldingRange" => Self::Folding(crate::decode(params)?),
            "textDocument/semanticTokens/full" => Self::Semantic(crate::decode(params)?),
            "textDocument/semanticTokens/range" => Self::SemanticRange(crate::decode(params)?),
            "custom/textDocument/sourceDefinition" => {
                Self::SourceDefinition(crate::decode(params)?)
            }
            "textDocument/definition" => Self::Definition(crate::decode(params)?),
            "textDocument/typeDefinition" => Self::TypeDefinition(crate::decode(params)?),
            "textDocument/documentSymbol" => Self::DocumentSymbols(crate::decode(params)?),
            _ => return Err(crate::coded_error(lsp::ErrorCode::INVALID_REQUEST, None)),
        })
    }
    // contentMapperFallbackResponse is restricted to the methods listed by Go.
    pub fn unknown_script_fallback(&self) -> bool {
        matches!(
            self,
            Self::Completion(_)
                | Self::Hover(_)
                | Self::SignatureHelp(_)
                | Self::Definition(_)
                | Self::TypeDefinition(_)
                | Self::References(_)
                | Self::Rename(_)
                | Self::Highlights(_)
                | Self::Implementation(_)
        )
    }
    pub fn uri(&self) -> &lsp::DocumentUri {
        match self {
            Self::PrepareRename(p) => &p.text_document.uri,
            Self::CodeActions(p) => &p.text_document.uri,
            Self::Rename(p) => &p.text_document.uri,
            Self::Format(p) => &p.text_document.uri,
            Self::FormatRange(p) => &p.text_document.uri,
            Self::FormatType(p) => &p.text_document.uri,
            Self::AutoInsert(p) => &p.vs_text_document.uri,
            Self::Completion(p) => &p.text_document.uri,
            Self::ResolveCompletion(_, uri) => uri,
            Self::SignatureHelp(p) => &p.text_document.uri,
            Self::InlayHints(p) => &p.text_document.uri,
            Self::CodeLenses(p) => &p.text_document.uri,
            Self::ResolveLens(p) | Self::LensLocations(p) => {
                &p.data.as_ref().expect("validated code lens data").uri
            }
            Self::CallPrepare(p) => &p.text_document.uri,
            Self::CallIncoming(p) | Self::CallOutgoing(p) | Self::IncomingPositions(p) => &p.uri,
            Self::Highlights(p) => &p.text_document.uri,
            Self::MultiHighlights(p) => &p.text_document.uri,
            Self::References(p) | Self::VSReferences(p) => &p.text_document.uri,
            Self::Implementation(p) | Self::LensImplementations(p) => &p.text_document.uri,
            Self::IncomingAt(p) => &p.text_document.uri,
            Self::Hover(p) => &p.text_document.uri,
            Self::Linked(p) => &p.text_document.uri,
            Self::Selection(p) => &p.text_document.uri,
            Self::Folding(p) => &p.text_document.uri,
            Self::Semantic(p) => &p.text_document.uri,
            Self::SemanticRange(p) => &p.text_document.uri,
            Self::Definition(p) | Self::SourceDefinition(p) => &p.text_document.uri,
            Self::TypeDefinition(p) => &p.text_document.uri,
            Self::DocumentSymbols(p) => &p.text_document.uri,
        }
    }
}
pub(crate) fn service_error(e: tsr_ls::Error) -> lsp::ResponseError {
    match e {
        tsr_ls::Error::Canceled => crate::canceled(),
        e => error(-32603, e.to_string()),
    }
}
struct RequestChecker<'a> {
    project: &'a tsr_project::Project,
    context: &'a Context,
    request_id: &'a str,
}
impl tsr_ls::QueryChecker for RequestChecker<'_> {
    fn with_checker<T>(
        &mut self,
        source: tsr_ast::NodeId,
        query: impl FnOnce(&mut tsr_checker::Operation<'_>) -> tsr_ls::Result<T>,
    ) -> tsr_ls::Result<T> {
        let checker = self
            .project
            .scheduler()
            .unwrap()
            .acquire(
                tsr_checker::CheckerLifetime::Temporary,
                Some(source),
                self.context,
                self.request_id,
            )
            .map_err(|e| match e {
                tsr_project::scheduler::AcquireError::Canceled(_) => tsr_ls::Error::Canceled,
                tsr_project::scheduler::AcquireError::Checker(e) => tsr_ls::Error::Checker(e),
            })?;
        let mut operation = checker.operation()?;
        query(&mut operation)
    }
}
#[derive(Clone)]
pub struct Options {
    pub organize: tsr_ls::OrganizeOptions,
    pub rename: tsr_ls::RenameOptions,
    pub formatting: bool,
    pub completion: tsr_ls::CompletionOptions,
    pub auto_closing_tags: bool,
    pub maximum_hover_length: usize,
    pub prefer_source_definition: bool,
    pub inlay: tsr_ls::InlayHintsOptions,
    pub code_lens: tsr_ls::CodeLensOptions,
    pub lens_command: Option<String>,
    pub locale: tsr_locale::Locale,
}
pub fn execute(
    context: &Context,
    request_id: &str,
    project: Option<&tsr_project::Project>,
    request: Request,
    encoding: tsr_jsstring::PositionEncoding,
    capabilities: &lsp::ClientCapabilities,
    options: &Options,
) -> Result<RawValue, lsp::ResponseError> {
    execute_impl(
        context,
        request_id,
        project,
        request,
        encoding,
        capabilities,
        options,
        false,
    )
    .map(|(response, _)| response)
}

pub(crate) fn execute_with_targets(
    context: &Context,
    request_id: &str,
    project: Option<&tsr_project::Project>,
    request: Request,
    encoding: tsr_jsstring::PositionEncoding,
    capabilities: &lsp::ClientCapabilities,
    options: &Options,
) -> Result<(RawValue, tsr_ls::CrossProjectTargets), lsp::ResponseError> {
    execute_impl(
        context,
        request_id,
        project,
        request,
        encoding,
        capabilities,
        options,
        true,
    )
}

#[allow(
    clippy::too_many_arguments,
    reason = "request context plus routing collection policy"
)]
fn execute_impl(
    context: &Context,
    request_id: &str,
    project: Option<&tsr_project::Project>,
    request: Request,
    encoding: tsr_jsstring::PositionEncoding,
    capabilities: &lsp::ClientCapabilities,
    options: &Options,
    collect_targets: bool,
) -> Result<(RawValue, tsr_ls::CrossProjectTargets), lsp::ResponseError> {
    if context.err().is_some() {
        return Err(crate::canceled());
    }
    let project = project.ok_or_else(|| {
        error(
            -32603,
            format!("no project found for URI {}", request.uri().0),
        )
    })?;
    let program = project
        .program()
        .ok_or_else(|| error(-32603, "project has no program"))?;
    let cancellation = tsr_core::CancellationToken::new();
    let cancel = cancellation.clone();
    let _stop = Stop(context.after_func(move || cancel.cancel()));
    let mut service = tsr_ls::LanguageService::new(program, encoding, cancellation);
    let query = |service: &mut tsr_ls::LanguageService<'_>| {
        execute_request(
            context,
            request_id,
            project,
            request,
            capabilities,
            options,
            service,
        )
    };
    if collect_targets {
        service.with_cross_project_targets(query)
    } else {
        query(&mut service).map(|response| (response, tsr_ls::CrossProjectTargets::default()))
    }
}

fn execute_request(
    context: &Context,
    request_id: &str,
    project: &tsr_project::Project,
    request: Request,
    capabilities: &lsp::ClientCapabilities,
    options: &Options,
    service: &mut tsr_ls::LanguageService<'_>,
) -> Result<RawValue, lsp::ResponseError> {
    let program = project.program().expect("validated project program");
    if let Some(host) = project.completion_file_system() {
        service.set_completion_file_system(host);
    }
    if matches!(request, Request::InlayHints(_)) && !options.inlay.enabled() {
        return client::raw(&lsp::Null);
    }
    let source_definition = match &request {
        Request::SourceDefinition(p) => Some(p),
        Request::Definition(p) if options.prefer_source_definition => Some(p),
        _ => None,
    };
    if let Some(p) = source_definition {
        let links = capabilities
            .text_document
            .as_deref()
            .and_then(|c| c.definition.as_deref())
            .and_then(|c| c.link_support.as_deref())
            .copied()
            .unwrap_or(false);
        let mut checker = RequestChecker {
            project,
            context,
            request_id,
        };
        return client::raw(
            &service
                .source_definition(&mut checker, &p.text_document.uri, &p.position, links)
                .map_err(service_error)?,
        );
    }
    if matches!(
        request,
        Request::PrepareRename(_)
            | Request::CodeActions(_)
            | Request::Rename(_)
            | Request::Completion(_)
            | Request::ResolveCompletion(_, _)
            | Request::Hover(_)
            | Request::SignatureHelp(_)
            | Request::InlayHints(_)
            | Request::ResolveLens(_)
            | Request::LensLocations(_)
            | Request::LensImplementations(_)
            | Request::IncomingPositions(_)
            | Request::IncomingAt(_)
            | Request::CallPrepare(_)
            | Request::CallIncoming(_)
            | Request::CallOutgoing(_)
            | Request::Highlights(_)
            | Request::MultiHighlights(_)
            | Request::References(_)
            | Request::VSReferences(_)
            | Request::Implementation(_)
            | Request::Semantic(_)
            | Request::SemanticRange(_)
            | Request::SourceDefinition(_)
            | Request::Definition(_)
            | Request::TypeDefinition(_)
    ) {
        let uri = request.uri();
        let file = program
            .source_file(uri.file_name().as_bytes())
            .ok_or_else(|| error(-32603, "file is not in the project"))?;
        let checker = project
            .scheduler()
            .unwrap()
            .acquire(
                tsr_checker::CheckerLifetime::Temporary,
                Some(file.source()),
                context,
                request_id,
            )
            .map_err(|e| match e {
                tsr_project::scheduler::AcquireError::Canceled(_) => crate::canceled(),
                e @ tsr_project::scheduler::AcquireError::Checker(_) => {
                    error(-32603, e.to_string())
                }
            })?;
        let mut operation = checker
            .operation()
            .map_err(|e| error(-32603, e.to_string()))?;
        if matches!(&request, Request::CodeActions(_)) {
            if let Some(cache) = project.auto_import_cache() {
                service.set_auto_import_cache(cache);
            }
        }
        let text_caps = capabilities.text_document.as_deref();
        if matches!(
            &request,
            Request::Completion(_) | Request::ResolveCompletion(_, _)
        ) {
            if let Some(cache) = project.auto_import_cache() {
                service.set_auto_import_cache(cache);
            }
            let caps = text_caps.and_then(|c| c.completion.as_deref());
            let item = caps.and_then(|c| c.completion_item.as_deref());
            let defaults = caps
                .and_then(|c| c.completion_list.as_deref())
                .and_then(|c| c.item_defaults.as_deref());
            let supports = |name: &str| defaults.is_some_and(|d| d.iter().any(|s| s == name));
            let options = tsr_ls::CompletionOptions {
                snippets: item
                    .and_then(|i| i.snippet_support.as_deref())
                    .copied()
                    .unwrap_or(false),
                commit_characters: item
                    .and_then(|i| i.commit_characters_support.as_deref())
                    .copied()
                    .unwrap_or(false),
                insert_replace: item
                    .and_then(|i| i.insert_replace_support.as_deref())
                    .copied()
                    .unwrap_or(false),
                label_details: item
                    .and_then(|i| i.label_details_support.as_deref())
                    .copied()
                    .unwrap_or(false),
                default_commit_characters: supports("commitCharacters"),
                default_edit_range: supports("editRange"),
                markdown: item
                    .and_then(|i| i.documentation_format.as_deref())
                    .and_then(|v| v.first())
                    .is_some_and(|m| m.0 == lsp::MarkupKind::MARKDOWN),
                ..options.completion.clone()
            };
            return match &request {
                Request::Completion(params) => client::raw(
                    &service
                        .completion(&mut operation, params, &options)
                        .map_err(service_error)?,
                ),
                Request::ResolveCompletion(item, _) => client::raw(
                    &service
                        .resolve_completion(&mut operation, item.clone(), &options)
                        .map_err(service_error)?,
                ),
                _ => unreachable!(),
            };
        }

        if let Request::Hover(params) = &request {
            let content = text_caps
                .and_then(|c| c.hover.as_deref())
                .and_then(|c| c.content_format.as_ref())
                .and_then(|c| c.first());
            let options = tsr_ls::HoverOptions {
                markdown: content.is_some_and(|kind| kind.0 == lsp::MarkupKind::MARKDOWN),
                classified: capabilities
                    .vs_supports_visual_studio_extensions
                    .as_deref()
                    .copied()
                    .unwrap_or(false),
                verbosity_signals: capabilities
                    .experimental
                    .as_deref()
                    .and_then(|c| c.hover_verbosity_level.as_deref())
                    .copied()
                    .unwrap_or(false),
                maximum_length: options.maximum_hover_length,
            };
            return client::raw(
                &service
                    .hover(&mut operation, params, options)
                    .map_err(service_error)?,
            );
        }
        if let Request::SignatureHelp(params) = &request {
            let caps = text_caps
                .and_then(|c| c.signature_help.as_deref())
                .and_then(|c| c.signature_information.as_deref());
            let options = tsr_ls::SignatureHelpOptions {
                format: caps
                    .and_then(|c| c.documentation_format.as_deref())
                    .and_then(|f| f.first())
                    .cloned()
                    .unwrap_or_else(|| lsp::MarkupKind(lsp::MarkupKind::PLAIN_TEXT.into())),
                classified: capabilities
                    .vs_supports_visual_studio_extensions
                    .as_deref()
                    .copied()
                    .unwrap_or(false),
                per_signature_active_parameter: caps
                    .and_then(|c| c.active_parameter_support.as_deref())
                    .copied()
                    .unwrap_or(false),
                null_active_parameter: caps
                    .and_then(|c| c.no_active_parameter_support.as_deref())
                    .copied()
                    .unwrap_or(false),
            };
            return client::raw(
                &service
                    .signature_help(&mut operation, params, &options)
                    .map_err(service_error)?,
            );
        }
        if let Request::InlayHints(params) = &request {
            return client::raw(
                &service
                    .inlay_hints(&mut operation, params, options.inlay)
                    .map_err(service_error)?,
            );
        }
        if let Request::LensLocations(lens) = &request {
            let locations = service
                .code_lens_locations(&mut operation, lens)
                .map_err(service_error)?;
            return client::raw(&lsp::LocationsOrNull {
                locations: Some(Box::new(locations)),
            });
        }
        if let Request::LensImplementations(params) = &request {
            return client::raw(
                &service
                    .implementations_with_options(&mut operation, params, false, true)
                    .map_err(service_error)?,
            );
        }
        if let Request::IncomingPositions(item) = &request {
            let positions = service
                .incoming_call_positions(&mut operation, item)
                .map_err(service_error)?;
            return client::raw(
                &positions
                    .into_iter()
                    .map(|p| lsp::TextDocumentPositionParams {
                        text_document: lsp::TextDocumentIdentifier { uri: p.uri },
                        position: p.position,
                    })
                    .collect::<Vec<_>>(),
            );
        }
        if let Request::IncomingAt(params) = &request {
            return client::raw(
                &service
                    .incoming_calls_at(&mut operation, &params.text_document.uri, &params.position)
                    .map_err(service_error)?,
            );
        }
        if let Request::ResolveLens(lens) = &request {
            return client::raw(
                &service
                    .resolve_code_lens(
                        &mut operation,
                        lens.clone(),
                        options.lens_command.as_deref(),
                        &options.locale,
                    )
                    .map_err(service_error)?,
            );
        }
        if let Request::CallPrepare(params) = &request {
            return client::raw(
                &service
                    .prepare_call_hierarchy(&mut operation, params)
                    .map_err(service_error)?,
            );
        }
        if let Request::CallIncoming(params) = &request {
            return client::raw(
                &service
                    .incoming_calls(&mut operation, params)
                    .map_err(service_error)?,
            );
        }
        if let Request::CallOutgoing(params) = &request {
            return client::raw(
                &service
                    .outgoing_calls(&mut operation, params)
                    .map_err(service_error)?,
            );
        }
        if let Request::Highlights(params) = &request {
            return client::raw(
                &service
                    .document_highlights(&mut operation, params)
                    .map_err(service_error)?,
            );
        }
        if let Request::MultiHighlights(params) = &request {
            return client::raw(
                &service
                    .multi_document_highlights(&mut operation, params)
                    .map_err(service_error)?,
            );
        }
        if let Request::VSReferences(params) = &request {
            let classified = capabilities
                .vs_supports_visual_studio_extensions
                .as_deref()
                .copied()
                .unwrap_or(false);
            return client::raw(
                &service
                    .vs_references(
                        &mut operation,
                        params,
                        &String::from_utf8_lossy(
                            project
                                .data()
                                .ok_or_else(|| error(-32603, "project has no data"))?
                                .path
                                .as_bytes(),
                        ),
                        classified,
                    )
                    .map_err(service_error)?,
            );
        }
        if let Request::CodeActions(params) = &request {
            return client::raw(
                &service
                    .code_actions(
                        &mut operation,
                        params,
                        &options.organize,
                        &options.completion,
                        &options.locale,
                    )
                    .map_err(service_error)?,
            );
        }
        if matches!(&request, Request::PrepareRename(_) | Request::Rename(_)) {
            let caps = capabilities
                .workspace
                .as_deref()
                .and_then(|c| c.workspace_edit.as_deref());
            let rename = tsr_ls::RenameOptions {
                document_changes: caps
                    .and_then(|c| c.document_changes.as_deref())
                    .copied()
                    .unwrap_or(false),
                rename_resources: caps
                    .and_then(|c| c.resource_operations.as_deref())
                    .is_some_and(|r| r.iter().any(|r| r.0 == "rename")),
                quote: options.completion.quote,
                ..options.rename
            };
            if let Request::PrepareRename(p) = &request {
                let info = service
                    .rename_info(
                        &mut operation,
                        &p.text_document.uri,
                        &p.position,
                        "",
                        rename,
                        &options.locale,
                    )
                    .map_err(service_error)?;
                if !info.can_rename {
                    return Err(error(-32803, info.error));
                }
                return client::raw(
                    &lsp::RangeOrPrepareRenamePlaceholderOrPrepareRenameDefaultBehaviorOrNull {
                        prepare_rename_placeholder: Some(Box::new(lsp::PrepareRenamePlaceholder {
                            range: info.range,
                            placeholder: info.name,
                        })),
                        ..Default::default()
                    },
                );
            }
            if let Request::Rename(p) = &request {
                let info = service
                    .rename_info(
                        &mut operation,
                        &p.text_document.uri,
                        &p.position,
                        &p.new_name,
                        rename,
                        &options.locale,
                    )
                    .map_err(service_error)?;
                if let Some((old, new)) = info.file_to_rename.filter(|_| info.can_rename) {
                    let will_rename = capabilities
                        .workspace
                        .as_deref()
                        .and_then(|w| w.file_operations.as_deref())
                        .and_then(|o| o.will_rename.as_deref())
                        .copied()
                        .unwrap_or(false);
                    let mut changes = if will_rename {
                        Vec::new()
                    } else {
                        service
                            .file_rename(
                                &mut operation,
                                &old,
                                &new,
                                &options.completion.auto_import,
                                &options.completion.format,
                            )
                            .map_err(service_error)?
                    };
                    changes.push(resource_rename(old, new));
                    return client::raw(&file_rename_response(changes, rename.document_changes));
                }
                return client::raw(
                    &service
                        .rename(&mut operation, p, rename, &options.locale)
                        .map_err(service_error)?,
                );
            }
        }
        if let Request::References(params) = &request {
            return client::raw(
                &service
                    .references(&mut operation, params)
                    .map_err(service_error)?,
            );
        }
        if let Request::Implementation(params) = &request {
            let links = text_caps
                .and_then(|c| c.implementation.as_deref())
                .and_then(|c| c.link_support.as_deref())
                .copied()
                .unwrap_or(false);
            return client::raw(
                &service
                    .implementations(&mut operation, params, links)
                    .map_err(service_error)?,
            );
        }
        let definition = match &request {
            Request::Definition(p) => Some((
                &p.position,
                false,
                text_caps
                    .and_then(|c| c.definition.as_deref())
                    .and_then(|c| c.link_support.as_deref()),
            )),
            Request::TypeDefinition(p) => Some((
                &p.position,
                true,
                text_caps
                    .and_then(|c| c.type_definition.as_deref())
                    .and_then(|c| c.link_support.as_deref()),
            )),
            _ => None,
        };
        if let Some((position, type_definition, links)) = definition {
            return client::raw(
                &service
                    .definition(
                        &mut operation,
                        uri,
                        position,
                        type_definition,
                        links.copied().unwrap_or(false),
                    )
                    .map_err(service_error)?,
            );
        }
        let caps = capabilities
            .text_document
            .as_deref()
            .and_then(|c| c.semantic_tokens.as_deref());
        let range = match &request {
            Request::SemanticRange(p) => Some(&p.range),
            _ => None,
        };
        return client::raw(
            &service
                .semantic_tokens(&mut operation, uri, range, caps)
                .map_err(service_error)?,
        );
    }
    match request {
        Request::Format(p) => client::raw(
            &service
                .format_document(&p, &options.completion.format, options.formatting)
                .map_err(service_error)?,
        ),
        Request::FormatRange(p) => client::raw(
            &service
                .format_range(&p, &options.completion.format, options.formatting)
                .map_err(service_error)?,
        ),
        Request::FormatType(p) => client::raw(
            &service
                .format_on_type(&p, &options.completion.format, options.formatting)
                .map_err(service_error)?,
        ),
        Request::AutoInsert(p) => client::raw(
            &service
                .auto_insert(&p, options.auto_closing_tags)
                .map_err(service_error)?,
        ),
        Request::CodeLenses(p) => client::raw(
            &service
                .code_lenses(&p, options.code_lens)
                .map_err(service_error)?,
        ),
        Request::Linked(p) => client::raw(&service.linked_editing(&p).map_err(service_error)?),
        Request::Selection(p) => client::raw(&service.selection_ranges(&p).map_err(service_error)?),
        Request::Folding(p) => {
            let caps = capabilities
                .text_document
                .as_deref()
                .and_then(|c| c.folding_range.as_deref());
            let options = tsr_ls::FoldingOptions {
                line_folding_only: caps
                    .and_then(|c| c.line_folding_only.as_deref())
                    .copied()
                    .unwrap_or(false),
                collapsed_text: caps
                    .and_then(|c| c.folding_range.as_deref())
                    .and_then(|c| c.collapsed_text.as_deref())
                    .copied()
                    .unwrap_or(false),
            };
            client::raw(
                &service
                    .folding_ranges(&p.text_document.uri, options)
                    .map_err(service_error)?,
            )
        }
        Request::PrepareRename(_)
        | Request::CodeActions(_)
        | Request::Rename(_)
        | Request::Completion(_)
        | Request::ResolveCompletion(_, _)
        | Request::Hover(_)
        | Request::SignatureHelp(_)
        | Request::InlayHints(_)
        | Request::ResolveLens(_)
        | Request::LensLocations(_)
        | Request::LensImplementations(_)
        | Request::IncomingPositions(_)
        | Request::IncomingAt(_)
        | Request::CallPrepare(_)
        | Request::CallIncoming(_)
        | Request::CallOutgoing(_)
        | Request::Highlights(_)
        | Request::MultiHighlights(_)
        | Request::References(_)
        | Request::VSReferences(_)
        | Request::Implementation(_)
        | Request::Semantic(_)
        | Request::SemanticRange(_)
        | Request::SourceDefinition(_)
        | Request::Definition(_)
        | Request::TypeDefinition(_) => {
            unreachable!("semantic requests acquired their checker above")
        }
        Request::DocumentSymbols(p) => {
            let hierarchical = capabilities
                .text_document
                .as_deref()
                .and_then(|c| c.document_symbol.as_deref())
                .and_then(|c| c.hierarchical_document_symbol_support.as_deref())
                .copied()
                .unwrap_or(false);
            client::raw(
                &service
                    .document_symbols(&p.text_document.uri, hierarchical)
                    .map_err(service_error)?,
            )
        }
    }
}

fn resource_rename(
    old_uri: lsp::DocumentUri,
    new_uri: lsp::DocumentUri,
) -> lsp::TextDocumentEditOrCreateFileOrRenameFileOrDeleteFile {
    lsp::TextDocumentEditOrCreateFileOrRenameFileOrDeleteFile {
        rename_file: Some(Box::new(lsp::RenameFile {
            old_uri,
            new_uri,
            ..Default::default()
        })),
        ..Default::default()
    }
}
fn file_rename_response(
    changes: Vec<lsp::TextDocumentEditOrCreateFileOrRenameFileOrDeleteFile>,
    document_changes: bool,
) -> lsp::WorkspaceEditOrNull {
    if changes.is_empty() {
        return lsp::WorkspaceEditOrNull::default();
    }
    let mut seen_edits = std::collections::HashMap::new();
    let mut seen_renames = std::collections::HashSet::new();
    let mut result = Vec::new();
    for mut change in changes {
        if let Some(rename) = &change.rename_file {
            if seen_renames.insert(rename.old_uri.clone()) {
                result.push(change);
            }
        } else if let Some(document) = &mut change.text_document_edit {
            document.edits.retain(|edit| {
                let Some(edit) = &edit.text_edit else {
                    return true;
                };
                let key = (
                    document.text_document.uri.clone(),
                    edit.range.start.line,
                    edit.range.start.character,
                    edit.range.end.line,
                    edit.range.end.character,
                );
                if seen_edits.get(&key) == Some(&edit.new_text) {
                    return false;
                }
                seen_edits.insert(key, edit.new_text.clone());
                true
            });
            if !document.edits.is_empty() {
                result.push(change);
            }
        }
    }
    if result.is_empty() {
        return lsp::WorkspaceEditOrNull::default();
    }
    let edit = if document_changes {
        lsp::WorkspaceEdit {
            document_changes: Some(Box::new(result)),
            ..Default::default()
        }
    } else {
        let mut changes: std::collections::HashMap<_, Vec<_>> = std::collections::HashMap::new();
        for change in result {
            if let Some(document) = change.text_document_edit {
                for edit in document.edits {
                    if let Some(edit) = edit.text_edit {
                        changes
                            .entry(document.text_document.uri.clone())
                            .or_default()
                            .push(Some(edit));
                    }
                }
            }
        }
        lsp::WorkspaceEdit {
            changes: Some(Box::new(changes)),
            ..Default::default()
        }
    };
    lsp::WorkspaceEditOrNull {
        workspace_edit: Some(Box::new(edit)),
    }
}

pub fn file_renames(
    context: &Context,
    request_id: &str,
    snapshot: &tsr_project::Snapshot,
    params: &lsp::RenameFilesParams,
    encoding: tsr_jsstring::PositionEncoding,
    capabilities: &lsp::ClientCapabilities,
    options: &tsr_ls::CompletionOptions,
) -> Result<RawValue, lsp::ResponseError> {
    let mut changes = Vec::new();
    for project in snapshot.projects() {
        if context.err().is_some() {
            return Err(crate::canceled());
        }
        let (Some(program), Some(scheduler)) = (project.program(), project.scheduler()) else {
            continue;
        };
        let cancellation = tsr_core::CancellationToken::new();
        let cancel = cancellation.clone();
        let _stop = Stop(context.after_func(move || cancel.cancel()));
        let result = (|| {
            let checker = scheduler
                .acquire(
                    tsr_checker::CheckerLifetime::Temporary,
                    None,
                    context,
                    request_id,
                )
                .map_err(|e| {
                    if context.err().is_some() {
                        crate::canceled()
                    } else {
                        error(-32603, e.to_string())
                    }
                })?;
            let mut operation = checker
                .operation()
                .map_err(|e| error(-32603, e.to_string()))?;
            let mut service = tsr_ls::LanguageService::new(program, encoding, cancellation);
            for file in params.files.iter().flatten() {
                changes.extend(
                    service
                        .file_rename(
                            &mut operation,
                            &lsp::DocumentUri(file.old_uri.clone()),
                            &lsp::DocumentUri(file.new_uri.clone()),
                            &options.auto_import,
                            &options.format,
                        )
                        .map_err(service_error)?,
                );
            }
            Ok::<_, lsp::ResponseError>(())
        })();
        result?;
    }
    let document_changes = capabilities
        .workspace
        .as_deref()
        .and_then(|w| w.workspace_edit.as_deref())
        .and_then(|e| e.document_changes.as_deref())
        .copied()
        .unwrap_or(false);
    client::raw(&file_rename_response(changes, document_changes))
}
