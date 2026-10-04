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
            | "textDocument/selectionRange"
            | "textDocument/foldingRange"
            | "textDocument/semanticTokens/full"
            | "textDocument/semanticTokens/range"
            | "textDocument/documentSymbol"
            | "textDocument/definition"
            | "textDocument/typeDefinition"
    )
}
pub enum Request {
    Linked(lsp::LinkedEditingRangeParams),
    Selection(lsp::SelectionRangeParams),
    Folding(lsp::FoldingRangeParams),
    Semantic(lsp::SemanticTokensParams),
    SemanticRange(lsp::SemanticTokensRangeParams),
    DocumentSymbols(lsp::DocumentSymbolParams),
    Definition(lsp::DefinitionParams),
    TypeDefinition(lsp::TypeDefinitionParams),
}
impl Request {
    pub fn decode(method: &str, params: Option<&RawValue>) -> Result<Self, lsp::ResponseError> {
        Ok(match method {
            "textDocument/linkedEditingRange" => Self::Linked(crate::decode(params)?),
            "textDocument/selectionRange" => Self::Selection(crate::decode(params)?),
            "textDocument/foldingRange" => Self::Folding(crate::decode(params)?),
            "textDocument/semanticTokens/full" => Self::Semantic(crate::decode(params)?),
            "textDocument/semanticTokens/range" => Self::SemanticRange(crate::decode(params)?),
            "textDocument/definition" => Self::Definition(crate::decode(params)?),
            "textDocument/typeDefinition" => Self::TypeDefinition(crate::decode(params)?),
            "textDocument/documentSymbol" => Self::DocumentSymbols(crate::decode(params)?),
            _ => return Err(crate::coded_error(lsp::ErrorCode::INVALID_REQUEST, None)),
        })
    }
    pub fn uri(&self) -> &lsp::DocumentUri {
        match self {
            Self::Linked(p) => &p.text_document.uri,
            Self::Selection(p) => &p.text_document.uri,
            Self::Folding(p) => &p.text_document.uri,
            Self::Semantic(p) => &p.text_document.uri,
            Self::SemanticRange(p) => &p.text_document.uri,
            Self::Definition(p) => &p.text_document.uri,
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
pub fn execute(
    context: &Context,
    request_id: &str,
    project: Option<&tsr_project::Project>,
    request: Request,
    encoding: tsr_jsstring::PositionEncoding,
    capabilities: &lsp::ClientCapabilities,
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
    if matches!(
        request,
        Request::Semantic(_)
            | Request::SemanticRange(_)
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
        Request::Semantic(_)
        | Request::SemanticRange(_)
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
