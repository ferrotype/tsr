//! The API server's entry points into the language service: completions
//! with the symbol behind each entry, and referenced symbols as nodes. The
//! LSP handlers convert the same results to protocol positions; the API
//! keeps nodes and symbols so its client can follow them by handle.
use crate::{LanguageService, Result};
use std::collections::HashMap;
use tsr_arena::NodeId;
use tsr_checker::{Operation, SymbolRef};
use tsr_lsproto as lsp;

/// One group of references: the definition node, its symbol and the
/// referencing nodes.
/// The pin's `ReferencedSymbolEntry` (findallreferences.go).
pub struct ApiReferenceGroup {
    pub definition: Option<NodeId>,
    pub symbol: Option<SymbolRef>,
    pub references: Vec<NodeId>,
}

impl LanguageService<'_> {
    /// Completions at a UTF-8 position of `source`, with the symbol of each
    /// symbol-backed entry by label (labels are unique in a list).
    /// port: tsc/internal/ls/completions.go:LanguageService.GetCompletionsAtPosition
    pub fn api_completions(
        &mut self,
        checker: &mut Operation<'_>,
        source: NodeId,
        position: i64,
        trigger_character: Option<&str>,
        options: &crate::CompletionOptions,
    ) -> Result<Option<(lsp::CompletionList, HashMap<String, SymbolRef>)>> {
        let file = self.source(source)?;
        let (line, character) =
            tsr_jsstring::scanner_positions::get_ecma_line_and_utf16_character_of_position(
                file.text().as_bytes(),
                isize::try_from(position).unwrap_or(isize::MAX),
            );
        let params = lsp::CompletionParams {
            text_document: lsp::TextDocumentIdentifier {
                uri: lsp::DocumentUri::from_file_name(file.file_name()),
            },
            position: lsp::Position {
                line: u32::try_from(line).unwrap_or(u32::MAX),
                character: u32::try_from(character).unwrap_or(u32::MAX),
            },
            context: trigger_character.map(|trigger| {
                Box::new(lsp::CompletionContext {
                    trigger_kind: lsp::CompletionTriggerKind::TRIGGER_CHARACTER,
                    trigger_character: Some(Box::new(trigger.to_string())),
                })
            }),
            ..Default::default()
        };
        self.completion_symbols = Some(HashMap::new());
        let result = self.completion_worker(checker, &params, options, source, position);
        let symbols = self.completion_symbols.take().unwrap_or_default();
        let mut response = result?;
        let Some(mut list) = response.list.take() else {
            return Ok(None);
        };
        self.completion_data(source, position, &mut list)?;
        Ok(Some((*list, symbols)))
    }

    /// The referenced symbols of a node, as the LSP references handler finds
    /// them, kept as nodes.
    /// port: tsc/internal/ls/findallreferences.go:LanguageService.GetReferencedSymbolsForNode
    pub fn api_referenced_symbols(
        &mut self,
        checker: &mut Operation<'_>,
        node: NodeId,
        position: i64,
    ) -> Result<Vec<ApiReferenceGroup>> {
        let files: Vec<NodeId> = self
            .program
            .files()
            .iter()
            .map(|file| file.source())
            .collect();
        let mut state = crate::references::SearchState::new(
            self,
            checker,
            files,
            crate::references::ReferenceOptions {
                adjust: true,
                ..Default::default()
            },
        );
        let groups = state.for_node(node, position)?;
        let mut result = Vec::with_capacity(groups.len());
        for group in groups {
            // port: tsc/internal/ls/findallreferences.go:SymbolAndEntries.DefinitionNode
            let mut definition = group.node;
            if definition.is_none() {
                if let Some(symbol) = group.symbol {
                    definition = checker.symbol_declarations(symbol)?.iter().flatten().next();
                }
            }
            result.push(ApiReferenceGroup {
                definition,
                symbol: group.symbol,
                // port: tsc/internal/ls/findallreferences.go:ReferenceEntry.IsNodeEntry
                references: group
                    .entries
                    .iter()
                    .filter_map(|entry| entry.node)
                    .collect(),
            });
        }
        Ok(result)
    }
}
