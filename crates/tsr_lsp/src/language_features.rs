//! Read-only request execution over the snapshot selected at dispatch. Parsing
//! and project updates happen before this boundary; worker work retains both.
use crate::{client, error};
use tsr_ipc::Context;
use tsr_json::RawValue;
use tsr_lsproto as lsp;

pub fn handles(method: &str) -> bool {
    matches!(
        method,
        "textDocument/linkedEditingRange"
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
pub enum Request {
    Hover(lsp::HoverParams),
    SignatureHelp(lsp::SignatureHelpParams),
    InlayHints(lsp::InlayHintParams),
    CodeLenses(lsp::CodeLensParams),
    ResolveLens(lsp::CodeLens),
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
        Ok(match method {
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
            Self::Hover(_)
                | Self::SignatureHelp(_)
                | Self::SourceDefinition(_)
                | Self::Definition(_)
                | Self::TypeDefinition(_)
                | Self::References(_)
                | Self::Implementation(_)
        )
    }
    pub fn uri(&self) -> &lsp::DocumentUri {
        match self {
            Self::SignatureHelp(p) => &p.text_document.uri,
            Self::InlayHints(p) => &p.text_document.uri,
            Self::CodeLenses(p) => &p.text_document.uri,
            Self::ResolveLens(p) => &p.data.as_ref().expect("validated code lens data").uri,
            Self::CallPrepare(p) => &p.text_document.uri,
            Self::CallIncoming(p) | Self::CallOutgoing(p) => &p.uri,
            Self::Highlights(p) => &p.text_document.uri,
            Self::MultiHighlights(p) => &p.text_document.uri,
            Self::References(p) | Self::VSReferences(p) => &p.text_document.uri,
            Self::Implementation(p) => &p.text_document.uri,
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
fn service_error(e: tsr_ls::Error) -> lsp::ResponseError {
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
    struct Stop(tsr_ipc::AfterFuncStop);
    impl Drop for Stop {
        fn drop(&mut self) {
            self.0.stop();
        }
    }
    let _stop = Stop(context.after_func(move || cancel.cancel()));
    let mut service = tsr_ls::LanguageService::new(program, encoding, cancellation);
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
        Request::Hover(_)
            | Request::SignatureHelp(_)
            | Request::InlayHints(_)
            | Request::ResolveLens(_)
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
        let text_caps = capabilities.text_document.as_deref();
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
        Request::Hover(_)
        | Request::SignatureHelp(_)
        | Request::InlayHints(_)
        | Request::ResolveLens(_)
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
