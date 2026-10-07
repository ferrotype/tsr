//! Language-service queries: completions with the symbol behind each entry,
//! referenced symbols for a node, and a symbol's references in a file.
//! port: tsc/internal/api/session.go
use super::handles::{node_handle, resolve_node_handle, CheckerSetup};
use super::{client_error, ApiSession, SessionError, SessionResult};
use crate::proto::{
    CompletionEntryLabelDetailsResponse, CompletionEntryResponse, CompletionInfoResponse,
    GetCompletionsAtPositionParams, GetReferencedSymbolsForNodeParams,
    GetReferencesToSymbolInFileParams, NodeHandle, ReferencedSymbolEntry,
};
use tsr_checker::Operation;

fn service_error(error: impl std::fmt::Display) -> SessionError {
    SessionError::Other(format!("{error}"))
}

impl ApiSession {
    /// The language service over the project's program; the API checker is
    /// the one it queries, so symbol handles stay resolvable. Module-export
    /// completions read the project's auto-import registry, which the
    /// service builds on demand: the pin's retry through a snapshot cloned
    /// with auto-imports has no Rust counterpart.
    /// port: tsc/internal/api/session.go:Session.setupLanguageService
    fn language_service<'a>(
        setup: &CheckerSetup<'a>,
    ) -> SessionResult<tsr_ls::LanguageService<'a>> {
        let project = setup.data.project(&setup.project)?;
        let mut service = tsr_ls::LanguageService::new(
            setup.program,
            tsr_jsstring::PositionEncoding::Utf16,
            tsr_core::CancellationToken::new(),
        );
        if let Some(host) = project.completion_file_system() {
            service.set_completion_file_system(host);
        }
        if let Some(cache) = project.auto_import_cache() {
            service.set_auto_import_cache(cache);
        }
        Ok(service)
    }

    /// port: tsc/internal/api/session.go:Session.handleGetCompletionsAtPosition
    pub(super) fn handle_get_completions_at_position(
        &self,
        params: &GetCompletionsAtPositionParams,
    ) -> SessionResult<Option<CompletionInfoResponse>> {
        let data = self.snapshot_data(params.snapshot)?;
        let setup = data.setup_checker(&params.project)?;
        let Some(file) = setup
            .program
            .source_file(params.file.to_file_name().as_bytes())
        else {
            return Ok(None);
        };
        let view = file.bound().view().ast();
        let position = view
            .source_file(file.source())
            .map_err(service_error)?
            .position_map()
            .utf16_to_utf8(isize::try_from(params.position).unwrap_or(isize::MAX));
        let mut operation = setup.registry.operation()?;
        let mut service = Self::language_service(&setup)?;
        let options = tsr_ls::CompletionOptions {
            label_details: true,
            ..Default::default()
        };
        let Some((list, symbols)) = service
            .api_completions(
                &mut operation,
                file.source(),
                i64::try_from(position).unwrap_or(i64::MAX),
                params.trigger_character.as_deref().map(String::as_str),
                &options,
            )
            .map_err(service_error)?
        else {
            return Ok(None);
        };
        let mut entries = Vec::with_capacity(list.items.len());
        for item in list.items.iter().flatten() {
            let mut entry = CompletionEntryResponse {
                name: item.label.clone(),
                kind: item.kind.as_deref().map_or(0, |kind| kind.0),
                sort_text: item.sort_text.clone(),
                insert_text: item.insert_text.clone(),
                filter_text: item.filter_text.clone(),
                detail: item.detail.clone(),
                label_details: item.label_details.as_deref().map(|details| {
                    Box::new(CompletionEntryLabelDetailsResponse {
                        detail: details.detail.clone(),
                        description: details.description.clone(),
                    })
                }),
                symbol: None,
            };
            if params.include_symbol {
                if let Some(symbol) = symbols.get(&item.label) {
                    entry.symbol = Some(Box::new(setup.symbol_response(&mut operation, *symbol)?));
                }
            }
            entries.push(Some(Box::new(entry)));
        }
        Ok(Some(CompletionInfoResponse {
            is_incomplete: list.is_incomplete,
            entries,
        }))
    }

    /// port: tsc/internal/api/session.go:Session.handleGetReferencedSymbolsForNode
    pub(super) fn handle_get_referenced_symbols_for_node(
        &self,
        params: &GetReferencedSymbolsForNodeParams,
    ) -> SessionResult<Option<Vec<ReferencedSymbolEntry>>> {
        let data = self.snapshot_data(params.snapshot)?;
        let setup = data.setup_checker(&params.project)?;
        let node = resolve_node_handle(setup.program, &params.node)?;
        let mut operation = setup.registry.operation()?;
        let mut service = Self::language_service(&setup)?;
        let groups = service
            .api_referenced_symbols(&mut operation, node, params.position)
            .map_err(service_error)?;
        if groups.is_empty() {
            return Ok(None);
        }
        let mut result = Vec::with_capacity(groups.len());
        for group in groups {
            let Some(definition) = group.definition else {
                continue;
            };
            let mut references = Vec::with_capacity(group.references.len());
            for reference in group.references {
                references.push(node_handle(&operation, reference)?);
            }
            result.push(ReferencedSymbolEntry {
                definition: node_handle(&operation, definition)?,
                symbol: match group.symbol {
                    Some(symbol) => Some(Box::new(setup.symbol_response(&mut operation, symbol)?)),
                    None => None,
                },
                references,
            });
        }
        Ok(Some(result))
    }

    /// port: tsc/internal/api/session.go:Session.handleGetReferencesToSymbolInFile
    pub(super) fn handle_get_references_to_symbol_in_file(
        &self,
        params: &GetReferencesToSymbolInFileParams,
    ) -> SessionResult<Vec<NodeHandle>> {
        let data = self.snapshot_data(params.snapshot)?;
        let setup = data.setup_checker(&params.project)?;
        let mut operation: Operation<'_> = setup.registry.operation()?;
        let symbol = data.registries.resolve_symbol(&operation, params.symbol)?;
        let file = setup
            .program
            .source_file(params.file.to_file_name().as_bytes())
            .ok_or_else(|| {
                client_error(format!("source file not found: {}", params.file.display()))
            })?;
        let nodes = operation
            .get_references_to_symbol_in_file(file.source(), symbol)
            .map_err(service_error)?;
        nodes
            .into_iter()
            .map(|node| node_handle(&operation, node))
            .collect()
    }
}
