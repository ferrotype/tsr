//! Language-service queries: completions with the symbol behind each entry,
//! referenced symbols for a node, and a symbol's references in a file.
//! port: tsc/internal/api/session.go
use super::handles::{node_handle, resolve_node_handle, CheckerSetup, Committed};
use super::{client_error, ApiSession, SessionError, SessionResult};
use crate::proto::{
    CheckerSymbolParams, CompletionEntryLabelDetailsResponse, CompletionEntryResponse,
    CompletionInfoResponse, GetCompletionsAtPositionParams, GetImportAdderEditsParams,
    GetReferencedSymbolsForNodeParams, GetReferencesToSymbolInFileParams, GetSignatureUsagesParams,
    ImportAdderActionKind, JsDocTagInfo, NodeHandle, ReferencedSymbolEntry, SignatureUsageResponse,
    TextEdit,
};
use tsr_checker::Operation;
use tsr_jsstring::{line_map::LspLineMap, position_map::PositionMap};

fn service_error(error: impl std::fmt::Display) -> SessionError {
    SessionError::Other(format!("{error}"))
}

/// Text edits in the client's coordinates: UTF-16 offsets into the original
/// text of a mapped file. An edit outside the text drops the whole result,
/// which goes out as `[]` like the pin's nil slice.
/// port: tsc/internal/api/session.go:toAPITextEdits
pub(super) fn to_api_text_edits(
    original_text: &[u8],
    edits: &[tsr_lsproto::TextEdit],
) -> Option<Vec<TextEdit>> {
    let line_map = LspLineMap::new(original_text);
    let positions = PositionMap::new(original_text);
    let mut result = Vec::with_capacity(edits.len());
    for edit in edits {
        let start = original_text_offset(&line_map, &edit.range.start, original_text.len())?;
        let end = original_text_offset(&line_map, &edit.range.end, original_text.len())?;
        result.push(TextEdit {
            pos: positions.utf8_to_utf16(start) as i64,
            end: positions.utf8_to_utf16(end) as i64,
            new_text: edit.new_text.clone(),
        });
    }
    Some(result)
}

/// The pin adds the LSP character to the line's byte start.
/// port: tsc/internal/api/session.go:originalTextOffset
fn original_text_offset(
    line_map: &LspLineMap,
    position: &tsr_lsproto::Position,
    text_length: usize,
) -> Option<isize> {
    let line_start = isize::try_from(*line_map.line_starts.get(position.line as usize)?).ok()?;
    let offset = line_start + isize::try_from(position.character).ok()?;
    (offset >= line_start && offset <= isize::try_from(text_length).ok()?).then_some(offset)
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
    ) -> SessionResult<Committed> {
        let data = self.snapshot_data(params.snapshot)?;
        let setup = data.setup_checker(&params.project)?;
        let Some(file) = setup
            .program
            .source_file(params.file.to_file_name().as_bytes())
        else {
            return setup.commit(&None::<CompletionInfoResponse>);
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
            return setup.commit(&None::<CompletionInfoResponse>);
        };
        let mut entries = Vec::with_capacity(list.items.len());
        for (index, item) in list.items.iter().enumerate() {
            let Some(item) = item else {
                continue;
            };
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
                if let Some(symbol) = symbols.get(&index) {
                    entry.symbol = Some(Box::new(setup.symbol_response(&mut operation, *symbol)?));
                }
            }
            entries.push(Some(Box::new(entry)));
        }
        setup.commit(&Some(CompletionInfoResponse {
            is_incomplete: list.is_incomplete,
            entries,
        }))
    }

    /// port: tsc/internal/api/session.go:Session.handleGetReferencedSymbolsForNode
    pub(super) fn handle_get_referenced_symbols_for_node(
        &self,
        params: &GetReferencedSymbolsForNodeParams,
    ) -> SessionResult<Committed> {
        let data = self.snapshot_data(params.snapshot)?;
        let setup = data.setup_checker(&params.project)?;
        let node = resolve_node_handle(setup.program, &params.node)?;
        let mut operation = setup.registry.operation()?;
        let mut service = Self::language_service(&setup)?;
        let groups = service
            .api_referenced_symbols(&mut operation, node, params.position)
            .map_err(service_error)?;
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
        setup.commit(&result)
    }

    /// port: tsc/internal/api/session.go:Session.handleGetReferencesToSymbolInFile
    pub(super) fn handle_get_references_to_symbol_in_file(
        &self,
        params: &GetReferencesToSymbolInFileParams,
    ) -> SessionResult<Committed> {
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
        let handles: Vec<NodeHandle> = nodes
            .into_iter()
            .map(|node| node_handle(&operation, node))
            .collect::<SessionResult<_>>()?;
        setup.commit(&handles)
    }

    /// port: tsc/internal/api/session.go:Session.handleGetJSDocTags
    pub(super) fn handle_get_jsdoc_tags(
        &self,
        params: &CheckerSymbolParams,
    ) -> SessionResult<Committed> {
        let data = self.snapshot_data(params.snapshot)?;
        let setup = data.setup_checker(&params.project)?;
        let mut operation = setup.registry.operation()?;
        let symbol = data.registries.resolve_symbol(&operation, params.symbol)?;
        let service = Self::language_service(&setup)?;
        let tags = service
            .api_symbol_jsdoc_tags(&mut operation, symbol)
            .map_err(service_error)?;
        let tags: Vec<Option<Box<JsDocTagInfo>>> = tags
            .into_iter()
            .map(|(name, text)| Some(Box::new(JsDocTagInfo { name, text })))
            .collect();
        setup.commit(&tags)
    }

    /// port: tsc/internal/api/session.go:Session.handleGetDocumentationComment
    pub(super) fn handle_get_documentation_comment(
        &self,
        params: &CheckerSymbolParams,
    ) -> SessionResult<Committed> {
        let data = self.snapshot_data(params.snapshot)?;
        let setup = data.setup_checker(&params.project)?;
        let mut operation = setup.registry.operation()?;
        let symbol = data.registries.resolve_symbol(&operation, params.symbol)?;
        let mut service = Self::language_service(&setup)?;
        let comment = service
            .api_symbol_documentation_comment(&mut operation, symbol)
            .map_err(service_error)?;
        setup.commit(&comment)
    }

    /// port: tsc/internal/api/session.go:Session.handleGetSignatureUsages
    pub(super) fn handle_get_signature_usages(
        &self,
        params: &GetSignatureUsagesParams,
    ) -> SessionResult<Committed> {
        let data = self.snapshot_data(params.snapshot)?;
        let setup = data.setup_checker(&params.project)?;
        let declaration = resolve_node_handle(setup.program, &params.signature_decl)?;
        let mut operation = setup.registry.operation()?;
        let mut service = Self::language_service(&setup)?;
        let usages = service
            .api_signature_usages(&mut operation, declaration)
            .map_err(service_error)?;
        let mut result = Vec::with_capacity(usages.len());
        for usage in usages {
            result.push(SignatureUsageResponse {
                name: node_handle(&operation, usage.name)?,
                call: match usage.call {
                    Some(call) => node_handle(&operation, call)?,
                    None => NodeHandle::default(),
                },
            });
        }
        setup.commit(&result)
    }

    /// port: tsc/internal/api/session.go:Session.handleGetImportAdderEdits
    pub(super) fn handle_get_import_adder_edits(
        &self,
        params: &GetImportAdderEditsParams,
    ) -> SessionResult<Committed> {
        let data = self.snapshot_data(params.snapshot)?;
        let setup = data.setup_checker(&params.project)?;
        let file = setup
            .program
            .source_file(params.file.to_file_name().as_bytes())
            .ok_or_else(|| {
                client_error(format!("source file not found: {}", params.file.display()))
            })?;
        let mut operation = setup.registry.operation()?;
        let mut actions = Vec::with_capacity(params.actions.len());
        for (index, action) in params.actions.iter().enumerate() {
            if action.kind.0 != ImportAdderActionKind::IMPORT_SYMBOL {
                return Err(client_error(format!(
                    "unknown import adder action kind {}",
                    tsr_jsstring::go_quote(action.kind.0.as_bytes())
                )));
            }
            if action.symbol.0 == 0 {
                return Err(client_error(format!(
                    "import adder action {index} missing symbol"
                )));
            }
            let symbol = data.registries.resolve_symbol(&operation, action.symbol)?;
            let is_valid_type_only_use_site = action
                .is_valid_type_only_use_site
                .as_deref()
                .copied()
                .unwrap_or(true);
            actions.push((symbol, is_valid_type_only_use_site));
        }
        let mut service = Self::language_service(&setup)?;
        let edits = service
            .api_import_adder_edits(
                &mut operation,
                file.source(),
                &actions,
                &tsr_ls::CompletionOptions::default(),
            )
            .map_err(service_error)?;
        if edits.is_empty() {
            return setup.commit(&Vec::<TextEdit>::new());
        }
        let view = file.bound().view().ast();
        let source = view.source_file(file.source()).map_err(service_error)?;
        setup.commit(&to_api_text_edits(source.original_text(), &edits).unwrap_or_default())
    }
}
