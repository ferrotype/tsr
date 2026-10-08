//! The API server's entry points into the language service: completions
//! with the symbol behind each entry, and referenced symbols as nodes. The
//! LSP handlers convert the same results to protocol positions; the API
//! keeps nodes and symbols so its client can follow them by handle.
use crate::{LanguageService, Result};
use std::collections::{HashMap, HashSet};
use tsr_arena::NodeId;
use tsr_ast::{
    utilities as ast, utilities_middle, utilities_tail, AstView, JsDocProvider, SyntaxKind as K,
};
use tsr_checker::{Operation, SymbolRef};
use tsr_lsproto as lsp;

/// One usage of a signature declaration: the referencing name and the call
/// it is the callee of, when it is one.
/// The pin's `SignatureUsage` (findallreferences.go).
pub struct ApiSignatureUsage {
    pub name: NodeId,
    pub call: Option<NodeId>,
}

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

    /// The plain-text documentation of a symbol: the comment of each distinct
    /// declaration, deduplicated and joined by newlines.
    /// port: tsc/internal/ls/jsdoc.go:GetSymbolDocumentationComment
    pub fn api_symbol_documentation_comment(
        &mut self,
        checker: &mut Operation<'_>,
        symbol: SymbolRef,
    ) -> Result<String> {
        let declarations: Vec<NodeId> = checker
            .symbol_declarations(symbol)?
            .iter()
            .flatten()
            .collect();
        let mut seen = HashSet::new();
        let mut parts: Vec<String> = Vec::new();
        for declaration in declarations {
            if !seen.insert(declaration) {
                continue;
            }
            let doc = self.declaration_documentation(checker, declaration, false, true)?;
            if !doc.is_empty() && !parts.contains(&doc) {
                parts.push(doc);
            }
        }
        Ok(parts.join("\n"))
    }

    /// A symbol's JSDoc tags as (name, text) pairs, the text rendered as a
    /// plain string.
    /// port: tsc/internal/ls/jsdoc.go:GetSymbolJSDocTags
    pub fn api_symbol_jsdoc_tags(
        &self,
        checker: &mut Operation<'_>,
        symbol: SymbolRef,
    ) -> Result<Vec<(String, String)>> {
        let declarations: Vec<NodeId> = checker
            .symbol_declarations(symbol)?
            .iter()
            .flatten()
            .collect();
        let mut seen = HashSet::new();
        let mut infos = Vec::new();
        for declaration in declarations {
            if !seen.insert(declaration) {
                continue;
            }
            let view = self.view(declaration)?;
            let tags = Self::declaration_jsdoc_tags(view, declaration)?;
            // A comment holding @typedef or @callback documents no particular
            // declaration, unless it also carries @param or @return.
            let kind = |tag: &NodeId| view.node(*tag).map(|node| node.kind().known());
            let has_typedef = tags.iter().any(|tag| {
                matches!(
                    kind(tag),
                    Ok(Some(K::JSDocTypedefTag | K::JSDocCallbackTag))
                )
            });
            let has_param_or_return = tags.iter().any(|tag| {
                matches!(
                    kind(tag),
                    Ok(Some(K::JSDocParameterTag | K::JSDocReturnTag))
                )
            });
            if has_typedef && !has_param_or_return {
                continue;
            }
            for tag in tags {
                let read = view.node(tag)?;
                let name = match read.tag_name() {
                    Some(name) => {
                        String::from_utf8_lossy(view.node_text(name)?.as_bytes()).into_owned()
                    }
                    None => String::new(),
                };
                infos.push((name, Self::jsdoc_tag_text(view, tag)?));
            }
        }
        Ok(infos)
    }

    /// The tags of the JSDoc comment that documents a declaration, walking
    /// the comment locations the checker's `getAllJSDocTags` walks.
    /// port: tsc/internal/ls/jsdoc.go:declarationJSDocTags
    fn declaration_jsdoc_tags(view: AstView<'_>, node: NodeId) -> Result<Vec<NodeId>> {
        if view.node(node)?.flags() & tsr_ast::node_flags::JS_DOC != 0 {
            return Ok(Vec::new());
        }
        let source = ast::get_source_file_of_node(view, Some(node))?
            .ok_or(tsr_arena::Error::InvalidGraph)?;
        let mut provider = tsr_parser::ParserJsDocProvider::default();
        let mut current = Some(node);
        while let Some(location) = current {
            let docs = provider.jsdoc(view, source, location)?;
            if let Some(last) = docs.last() {
                if let Some(tags) = view
                    .node(*last)?
                    .data_source()
                    .as_js_doc()
                    .and_then(|doc| doc.tags())
                {
                    return crate::documentation::list(view, Some(tags));
                }
            }
            current = utilities_tail::get_next_js_doc_comment_location(view, location)?;
        }
        Ok(Vec::new())
    }

    /// The text of one tag as a plain string.
    /// port: tsc/internal/ls/jsdoc.go:getJSDocTagText
    fn jsdoc_tag_text(view: AstView<'_>, tag: NodeId) -> Result<String> {
        let read = view.node(tag)?;
        let comment = String::from_utf8_lossy(
            tsr_scanner::get_text_of_jsdoc_comment(view, read.comment_list())?.as_bytes(),
        )
        .into_owned();
        let text_of = |node: NodeId| -> Result<String> {
            Ok(String::from_utf8_lossy(view.node_text(node)?.as_bytes()).into_owned())
        };
        let add_comment = |text: String| {
            if comment.is_empty() {
                text
            } else {
                format!("{text} {comment}")
            }
        };
        let data = read.data_source();
        Ok(match read.kind().known() {
            Some(K::JSDocThrowsTag) => match data
                .as_js_doc_throws_tag()
                .and_then(|tag| tag.type_expression())
            {
                Some(type_expression) => add_comment(text_of(type_expression)?),
                None => comment.clone(),
            },
            Some(K::JSDocImplementsTag | K::JSDocAugmentsTag) => match read.class_name() {
                Some(class_name) => add_comment(text_of(class_name)?),
                None => comment.clone(),
            },
            Some(K::JSDocTemplateTag) => {
                let mut out = String::new();
                if let Some(constraint) = data
                    .as_js_doc_template_tag()
                    .and_then(|tag| tag.constraint())
                {
                    out.push_str(&text_of(constraint)?);
                }
                let parameters: Vec<NodeId> = view
                    .node_slice(read.type_parameters(view)?)?
                    .iter()
                    .flatten()
                    .collect();
                for (index, parameter) in parameters.iter().enumerate() {
                    if index == 0 && !out.is_empty() {
                        out.push(' ');
                    }
                    if index != 0 {
                        out.push_str(", ");
                    }
                    out.push_str(&text_of(*parameter)?);
                }
                if !comment.is_empty() {
                    if !out.is_empty() {
                        out.push(' ');
                    }
                    out.push_str(&comment);
                }
                out
            }
            Some(K::JSDocTypeTag) => match data
                .as_js_doc_type_tag()
                .and_then(|tag| tag.type_expression())
            {
                Some(type_expression) => add_comment(text_of(type_expression)?),
                None => comment.clone(),
            },
            Some(K::JSDocSatisfiesTag) => match data
                .as_js_doc_satisfies_tag()
                .and_then(|tag| tag.type_expression())
            {
                Some(type_expression) => add_comment(text_of(type_expression)?),
                None => comment.clone(),
            },
            Some(K::JSDocSeeTag) => match data
                .as_js_doc_see_tag()
                .and_then(|tag| tag.name_expression())
            {
                Some(name_expression) => add_comment(text_of(name_expression)?),
                None => comment.clone(),
            },
            Some(K::JSDocParameterTag | K::JSDocPropertyTag) => match read.name() {
                Some(name) => add_comment(text_of(name)?),
                None => comment.clone(),
            },
            _ => comment.clone(),
        })
    }

    /// Every usage of a signature declaration as a name and, when the name
    /// is the callee, its call; the declarations' own names are left out.
    /// port: tsc/internal/ls/findallreferences.go:LanguageService.GetSignatureUsages
    pub fn api_signature_usages(
        &mut self,
        checker: &mut Operation<'_>,
        declaration: NodeId,
    ) -> Result<Vec<ApiSignatureUsage>> {
        let view = self.view(declaration)?;
        let Some(name) = view.node(declaration)?.name() else {
            return Ok(Vec::new());
        };
        if view.node(name)?.kind() != K::Identifier {
            return Ok(Vec::new());
        }
        let position = i64::from(view.node(name)?.pos());
        let groups = self.api_referenced_symbols(checker, name, position)?;
        let mut declaration_names = HashSet::new();
        for group in &groups {
            let Some(symbol) = group.symbol else {
                continue;
            };
            let declarations: Vec<NodeId> = checker
                .symbol_declarations(symbol)?
                .iter()
                .flatten()
                .collect();
            for declaration in declarations {
                if let Some(name) = self.view(declaration)?.node(declaration)?.name() {
                    declaration_names.insert(name);
                }
            }
        }
        let mut result = Vec::new();
        for group in groups {
            for node in group.references {
                if declaration_names.contains(&node) {
                    continue;
                }
                let view = self.view(node)?;
                let called = utilities_middle::climb_past_property_access(view, node)?;
                let call = match view.node(called)?.parent() {
                    Some(parent)
                        if view.node(parent)?.kind() == K::CallExpression
                            && view.node(parent)?.expression() == Some(called) =>
                    {
                        Some(parent)
                    }
                    _ => None,
                };
                result.push(ApiSignatureUsage { name: node, call });
            }
        }
        Ok(result)
    }

    /// The edits that import the given exported symbols into `source`: each
    /// symbol's best fix by the auto-import ranking, coalesced by the import
    /// adder and placed against the file's text. The registry is prepared on
    /// demand (the pin clones the snapshot with auto-imports first).
    /// port: tsc/internal/api/session.go:Session.handleGetImportAdderEdits
    pub fn api_import_adder_edits(
        &mut self,
        checker: &mut Operation<'_>,
        source: NodeId,
        actions: &[(SymbolRef, bool)],
        options: &crate::CompletionOptions,
    ) -> Result<Vec<lsp::TextEdit>> {
        let syntax = crate::syntax::Syntax::new(self.view(source)?, source)?;
        let registry = self.prepare_auto_imports(checker, &syntax, &options.auto_import)?;
        let ranking =
            tsr_autoimport::ranking::Ranking::new(self.program, source, &options.auto_import)?;
        let verbatim = self.program.options().verbatim_module_syntax.is_true();
        let mut adder = tsr_autoimport::ImportAdder::default();
        for (exported, is_valid_type_only_use_site) in actions {
            // port: tsc/internal/ls/autoimport/import_adder.go:importAdder.AddImportFromExportedSymbol
            let symbol = checker.skip_alias(*exported)?;
            let target = checker.get_merged_symbol(symbol)?;
            let Some(id) = tsr_autoimport::export_id_for_symbol(self.program, checker, target)?
            else {
                // An export the registry cannot see (filtered by the exclude
                // patterns) produces no import.
                continue;
            };
            let mut fixes = Vec::new();
            for export in
                registry.index.entries().iter().filter(|entry| {
                    entry.id == id && entry.id.module.as_bytes() != syntax.file.path()
                })
            {
                self.check_canceled()?;
                fixes.extend(tsr_autoimport::fix::fixes_with_info(
                    self.program,
                    checker,
                    source,
                    export,
                    tsr_autoimport::fix::Usage {
                        type_only: *is_valid_type_only_use_site,
                        ..Default::default()
                    },
                    &options.auto_import,
                )?);
            }
            fixes.sort_by(|a, b| ranking.rank(a, b));
            if let Some(fix) = fixes.into_iter().next() {
                adder.add(fix.protocol, verbatim);
            }
        }
        if !adder.has_fixes() {
            return Ok(Vec::new());
        }
        Ok(self
            .import_adder_action_edits(&syntax, options, &adder)?
            .into_iter()
            .flatten()
            .map(|edit| *edit)
            .collect())
    }
}
