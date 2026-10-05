//! Classified references retain definition groups and their native order.
use crate::{
    reference_helpers as h,
    references::{DefinitionKind, ReferenceEntry, ReferenceOptions, SearchState},
    syntax::Syntax,
    LanguageService, Result,
};
use tsr_ast::{span_map::FEATURE_REFERENCES, NodeId};
use tsr_checker::{Operation, SymbolRef, VerbosityContext};
use tsr_lsproto as lsp;

impl LanguageService<'_> {
    fn reference_display(
        &mut self,
        c: &mut Operation<'_>,
        symbol: SymbolRef,
        node: NodeId,
        classified: bool,
    ) -> Result<lsp::VSClassifiedTextElement> {
        let meaning = SearchState::new(self, c, Vec::new(), ReferenceOptions::default())
            .search_meaning(Some(node), symbol)?;
        let (parts, _) = self.quick_info(
            c,
            Some(symbol),
            node,
            &mut VerbosityContext::default(),
            classified,
            meaning,
        )?;
        let runs = if classified {
            parts.runs(c)?
        } else {
            vec![lsp::VSClassifiedTextRun {
                text: parts.string(),
                classification_type_name: "text".into(),
                ..Default::default()
            }]
        };
        Ok(lsp::VSClassifiedTextElement {
            runs: runs.into_iter().map(|r| Some(Box::new(r))).collect(),
            ..Default::default()
        })
    }
    // port: tsc/internal/ls/findallreferences.go:LanguageService.ProvideVSReferences
    pub fn vs_references(
        &mut self,
        c: &mut Operation<'_>,
        params: &lsp::ReferenceParams,
        project: &str,
        classified: bool,
    ) -> Result<lsp::VSReferenceItemsOrNull> {
        let source = self.file(&params.text_document.uri)?;
        let mapped = self.converters.from_lsp_position_for_source_file(
            self.program,
            source,
            &params.position,
            FEATURE_REFERENCES,
        )?;
        let files = self
            .program
            .files()
            .iter()
            .map(|f| f.source())
            .collect::<Vec<_>>();
        let mut items = Vec::new();
        for mapped in mapped {
            if !mapped.mapped.fidelity.is_single_segment() {
                continue;
            }
            let original = Syntax::new(self.view(mapped.script)?, mapped.script)?
                .nav()
                .get_touching_property_name(i64::from(mapped.mapped.position))?;
            let groups = SearchState::new(
                self,
                c,
                files.clone(),
                ReferenceOptions {
                    adjust: true,
                    ..Default::default()
                },
            )
            .for_node(original, i64::from(mapped.mapped.position))?;
            for group in groups {
                let (node, display) = match group.kind {
                    DefinitionKind::Symbol => {
                        let Some(symbol) = group.symbol else { continue };
                        let node = if let Some(decl) = h::declarations(c, symbol)?.first() {
                            c.node(*decl)?.name().unwrap_or(*decl)
                        } else {
                            original
                        };
                        (
                            node,
                            self.reference_display(c, symbol, original, classified)?,
                        )
                    }
                    DefinitionKind::This => {
                        let (Some(node), Some(symbol)) = (group.node, group.symbol) else {
                            continue;
                        };
                        (node, self.reference_display(c, symbol, node, classified)?)
                    }
                    kind => {
                        let Some(node) = group.node else { continue };
                        let (text, classification) = match kind {
                            DefinitionKind::Keyword => (
                                c.node(node)?
                                    .kind()
                                    .known()
                                    .map(tsr_scanner::token_to_string)
                                    .unwrap_or("")
                                    .to_string(),
                                "keyword",
                            ),
                            DefinitionKind::String => (
                                String::from_utf8_lossy(
                                    self.view(node)?.node_text(node)?.as_bytes(),
                                )
                                .into_owned(),
                                "string",
                            ),
                            _ => (
                                String::from_utf8_lossy(
                                    self.view(node)?.node_text(node)?.as_bytes(),
                                )
                                .into_owned(),
                                "text",
                            ),
                        };
                        (
                            node,
                            lsp::VSClassifiedTextElement {
                                runs: vec![Some(Box::new(lsp::VSClassifiedTextRun {
                                    text,
                                    classification_type_name: classification.into(),
                                    ..Default::default()
                                }))],
                                ..Default::default()
                            },
                        )
                    }
                };
                let node = c.node(node)?.name().unwrap_or(node);
                let entry = ReferenceEntry {
                    kind: crate::references::EntryKind::Node,
                    node: Some(node),
                    context: None,
                    range: None,
                    source: self
                        .program
                        .file_of_node(node)
                        .ok_or(tsr_arena::Error::WrongOwner)?
                        .source(),
                };
                let Some(location) = self.entry_location(&entry, FEATURE_REFERENCES)? else {
                    continue;
                };
                let definition_id = items.len() as i32;
                items.push(Some(Box::new(lsp::VSReferenceItem {
                    vs_id: definition_id,
                    vs_location: location,
                    vs_definition_text: Some(Box::new(display)),
                    vs_kind: Some(Box::new(vec![lsp::VSReferenceKind::UNKNOWN])),
                    vs_project_name: Some(Box::new(project.into())),
                    vs_containing_type: Some(Box::new(String::new())),
                    ..Default::default()
                })));
                for entry in group.entries {
                    if let (Some(node), Some(symbol)) = (entry.node, group.symbol) {
                        if crate::references::is_declaration(
                            self.program
                                .file_of_node(node)
                                .ok_or(tsr_arena::Error::WrongOwner)?
                                .bound()
                                .view(),
                            c,
                            node,
                            symbol,
                        )? {
                            continue;
                        }
                    }
                    let Some(location) = self.entry_location(&entry, FEATURE_REFERENCES)? else {
                        continue;
                    };
                    let write = self.entry_write(&entry)?;
                    items.push(Some(Box::new(lsp::VSReferenceItem {
                        vs_id: items.len() as i32,
                        vs_definition_id: Some(Box::new(definition_id)),
                        vs_location: location,
                        vs_kind: Some(Box::new(vec![if write {
                            lsp::VSReferenceKind::WRITE
                        } else {
                            lsp::VSReferenceKind::READ
                        }])),
                        vs_project_name: Some(Box::new(project.into())),
                        ..Default::default()
                    })));
                }
            }
        }
        Ok(lsp::VSReferenceItemsOrNull {
            vs_reference_items: Some(Box::new(items)),
        })
    }
}
