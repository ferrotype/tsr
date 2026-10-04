//! Rename validation and exact edits over the shared symbol-reference search.
use crate::{
    reference_helpers as h,
    references::{EntryKind, ReferenceEntry, ReferenceOptions, SearchState},
    syntax::Syntax,
    LanguageService, QuotePreference, Result,
};
use std::collections::HashMap;
use tsr_ast::{
    span_map::FEATURE_RENAME, symbol_flags as sf, utilities as ast, utilities_modules as modules,
    NodeId, SyntaxKind as K,
};
use tsr_checker::{Operation, SymbolRef};
use tsr_core::TextRange;
use tsr_lsproto as lsp;

#[derive(Clone, Copy)]
pub struct RenameOptions {
    pub aliases: bool,
    pub import_paths: bool,
    pub document_changes: bool,
    pub rename_resources: bool,
    pub quote: QuotePreference,
}
impl Default for RenameOptions {
    fn default() -> Self {
        Self {
            aliases: true,
            import_paths: true,
            document_changes: false,
            rename_resources: false,
            quote: QuotePreference::Auto,
        }
    }
}
#[derive(Default)]
pub struct RenameInfo {
    pub can_rename: bool,
    pub error: String,
    pub name: String,
    pub range: lsp::Range,
    pub file_to_rename: Option<(lsp::DocumentUri, lsp::DocumentUri)>,
}
fn refused(message: &tsr_diagnostics::Message, locale: &tsr_locale::Locale) -> RenameInfo {
    RenameInfo {
        error: String::from_utf8_lossy(&message.localize(locale, &[])).into_owned(),
        ..Default::default()
    }
}
// port: tsc/internal/ls/rename.go:nodeIsEligibleForRename
fn eligible(view: tsr_ast::AstView<'_>, node: NodeId) -> Result<bool> {
    Ok(match view.node(node)?.kind().known() {
        Some(
            K::Identifier
            | K::PrivateIdentifier
            | K::StringLiteral
            | K::NoSubstitutionTemplateLiteral
            | K::ThisKeyword,
        ) => true,
        Some(K::NumericLiteral) => h::literal_property(view, node)?,
        _ => false,
    })
}
impl LanguageService<'_> {
    // port: tsc/internal/ls/rename.go:LanguageService.GetRenameInfo
    pub fn rename_info(
        &mut self,
        c: &mut Operation<'_>,
        uri: &lsp::DocumentUri,
        position: &lsp::Position,
        new_name: &str,
        options: RenameOptions,
        locale: &tsr_locale::Locale,
    ) -> Result<RenameInfo> {
        self.check_canceled()?;
        let source = self.file(uri)?;
        for mapped in self.converters.from_lsp_position_for_source_file(
            self.program,
            source,
            position,
            FEATURE_RENAME,
        )? {
            if !mapped.mapped.fidelity.is_exact() {
                continue;
            }
            let source = mapped.script;
            let mut syntax = Syntax::new(self.view(source)?, source)?;
            let node = syntax
                .nav()
                .get_touching_property_name(i64::from(mapped.mapped.position))?;
            let node = crate::meaning::adjusted_location(syntax.view, node, true)?;
            if eligible(syntax.view, node)? {
                if let Some(info) = self.rename_node(c, source, node, new_name, options, locale)? {
                    return Ok(info);
                }
            }
        }
        Ok(refused(
            tsr_diagnostics::You_cannot_rename_this_element,
            locale,
        ))
    }
    // port: tsc/internal/ls/rename.go:LanguageService.getRenameInfoForNode
    fn rename_node(
        &mut self,
        c: &mut Operation<'_>,
        source: NodeId,
        node: NodeId,
        new_name: &str,
        options: RenameOptions,
        locale: &tsr_locale::Locale,
    ) -> Result<Option<RenameInfo>> {
        let view = self.view(node)?;
        let n = view.node(node)?;
        let name = if let Some(symbol) = c.get_symbol_at_location(node)? {
            if h::declarations(c, symbol)?.is_empty() {
                return Ok(None);
            }
            if let Some(reason) = self.rename_blocked(c, source, node, symbol, options)? {
                return Ok(Some(refused(reason, locale)));
            }
            if ast::is_string_literal_like(&n)
                && modules::import_from_module_specifier(view, node)?.is_some()
            {
                return if options.import_paths {
                    self.rename_module(c, source, node, symbol, new_name, options, locale)
                } else {
                    Ok(None)
                };
            }
            c.symbol_to_string(symbol)?.as_bytes().to_vec()
        } else {
            if ast::is_string_literal_like(&n) {
                let mut state =
                    SearchState::new(self, c, vec![source], ReferenceOptions::default());
                let Some(ty) = state.contextual_literal(node)? else {
                    return Ok(None);
                };
                let flags = state.c.type_flags(ty)?;
                let mut string = flags & tsr_checker::type_flags::STRING_LITERAL != 0;
                if flags & tsr_checker::type_flags::UNION != 0 {
                    string = true;
                    for ty in state.c.constituents(ty)? {
                        string &=
                            state.c.type_flags(ty)? & tsr_checker::type_flags::STRING_LITERAL != 0;
                    }
                }
                if !string {
                    return Ok(None);
                }
            } else if !tsr_ast::utilities_middle::is_label_name(view, node)? {
                return Ok(None);
            }
            view.node_text(node)?.as_bytes().to_vec()
        };
        let mut start = Syntax::new(view, source)?.start(node)?;
        let mut end = i64::from(n.end());
        if ast::is_string_literal_like(&n) {
            start += 1;
            end -= 1;
        }
        let (range, fidelity) = self.unrestricted_range(source, TextRange::new(start, end))?;
        Ok(Some(RenameInfo {
            can_rename: fidelity.is_exact(),
            name: String::from_utf8_lossy(&name).into_owned(),
            range,
            ..Default::default()
        }))
    }
    // port: tsc/internal/ls/rename.go:LanguageService.renameBlockedReason
    fn rename_blocked(
        &self,
        c: &mut Operation<'_>,
        source: NodeId,
        node: NodeId,
        mut symbol: SymbolRef,
        options: RenameOptions,
    ) -> Result<Option<&'static tsr_diagnostics::Message>> {
        for decl in h::declarations(c, symbol)? {
            let file = self
                .program
                .file_of_node(decl)
                .ok_or(tsr_arena::Error::WrongOwner)?;
            if self.program.is_lib(self.source(file.source())?.path())
                && tsr_tspath::is_declaration_file_name(self.source(file.source())?.file_name())
            {
                return Ok(Some(tsr_diagnostics::You_cannot_rename_elements_that_are_defined_in_the_standard_TypeScript_library));
            }
        }
        let view = self.view(node)?;
        if view.node(node)?.kind() == K::Identifier
            && view.node_text(node)?.as_bytes() == b"default"
        {
            if let Some(parent) = h::parent_symbol(c, symbol)? {
                if c.symbol(parent)?.flags() & sf::MODULE != 0 {
                    return Ok(Some(tsr_diagnostics::You_cannot_rename_this_element));
                }
            }
        }
        if !options.aliases && c.symbol(symbol)?.flags() & sf::ALIAS != 0 {
            for decl in h::declarations(c, symbol)? {
                if c.node(decl)?.kind() == K::ImportSpecifier {
                    if c.node(decl)?.property_name().is_none() {
                        symbol = c.get_aliased_symbol(symbol)?;
                    }
                    break;
                }
            }
        }
        let original =
            tsr_module::parse_node_module_from_path(self.source(source)?.file_name(), false);
        for decl in h::declarations(c, symbol)? {
            let file = self
                .program
                .file_of_node(decl)
                .ok_or(tsr_arena::Error::WrongOwner)?;
            let path = self.source(file.source())?.file_name();
            if original.is_empty() {
                if path
                    .windows(b"/node_modules/".len())
                    .any(|part| part == b"/node_modules/")
                {
                    return Ok(Some(tsr_diagnostics::You_cannot_rename_elements_that_are_defined_in_a_node_modules_folder));
                }
            } else {
                let other = tsr_module::parse_node_module_from_path(path, false);
                if !other.is_empty() && other != original {
                    return Ok(Some(tsr_diagnostics::You_cannot_rename_elements_that_are_defined_in_another_node_modules_folder));
                }
            }
        }
        Ok(None)
    }
    // port: tsc/internal/ls/rename.go:LanguageService.getRenameInfoForModule
    #[allow(
        clippy::too_many_arguments,
        reason = "The pinned module-rename validation needs the checker, source projection, capabilities and localized errors from this request"
    )]
    fn rename_module(
        &mut self,
        c: &Operation<'_>,
        source: NodeId,
        node: NodeId,
        symbol: SymbolRef,
        new_name: &str,
        options: RenameOptions,
        locale: &tsr_locale::Locale,
    ) -> Result<Option<RenameInfo>> {
        let view = self.view(node)?;
        let text = view.node_text(node)?;
        if !tsr_tspath::is_external_module_name_relative(text.as_bytes()) {
            return Ok(Some(refused(
                tsr_diagnostics::You_cannot_rename_a_module_via_a_global_import,
                locale,
            )));
        }
        if !options.document_changes || !options.rename_resources {
            return Ok(Some(refused(
                tsr_diagnostics::File_rename_is_not_supported_by_the_editor,
                locale,
            )));
        }
        let Some(target) = h::declarations(c, symbol)?
            .into_iter()
            .find(|id| c.node(*id).is_ok_and(|n| n.kind() == K::SourceFile))
        else {
            return Ok(None);
        };
        let mut path = self.source(target)?.file_name().to_vec();
        if !text.as_bytes().ends_with(b"/index") && !text.as_bytes().ends_with(b"/index.js") {
            let without_extension = tsr_tspath::remove_file_extension(&path);
            if let Some(directory) = without_extension.strip_suffix(b"/index") {
                path = directory.to_vec();
            }
        }
        let new_path = self.renamed_module_path(&path, text.as_bytes(), new_name.as_bytes());
        let component = text
            .as_bytes()
            .iter()
            .rposition(|&b| b == b'/')
            .map_or(0, |i| i + 1);
        let start = Syntax::new(view, source)?.start(node)? + 1 + component as i64;
        let (range, fidelity) = self.unrestricted_range(
            source,
            TextRange::new(start, start + (text.len() - component) as i64),
        )?;
        if !fidelity.is_exact() {
            return Ok(None);
        }
        Ok(Some(RenameInfo {
            can_rename: true,
            name: String::from_utf8_lossy(&text.as_bytes()[component..]).into_owned(),
            range,
            file_to_rename: Some((
                lsp::DocumentUri::from_file_name(&path),
                lsp::DocumentUri::from_file_name(&new_path),
            )),
            ..Default::default()
        }))
    }
    // port: tsc/internal/ls/rename.go:LanguageService.getNewFileNameForModuleRename
    fn renamed_module_path(&self, old: &[u8], specifier: &[u8], name: &[u8]) -> Vec<u8> {
        let mut new = tsr_tspath::combine(&tsr_tspath::directory(old), &[name]);
        let ignore_case = !self.program.host().use_case_sensitive_file_names();
        let extension = if tsr_tspath::is_declaration_file_name(old) {
            tsr_tspath::declaration_file_extension(old)
        } else {
            tsr_tspath::any_extension_from_path::<&[u8]>(old, &[], ignore_case)
        };
        if !tsr_tspath::has_extension(&new) {
            new.extend_from_slice(extension);
        } else if tsr_tspath::any_extension_from_path::<&[u8]>(&new, &[], ignore_case)
            == tsr_tspath::any_extension_from_path::<&[u8]>(specifier, &[], ignore_case)
        {
            new = tsr_tspath::change_any_extension::<&[u8]>(&new, extension, &[], ignore_case);
        }
        new
    }
    // port: tsc/internal/ls/rename.go:LanguageService.ProvideRename
    pub fn rename(
        &mut self,
        c: &mut Operation<'_>,
        params: &lsp::RenameParams,
        options: RenameOptions,
        locale: &tsr_locale::Locale,
    ) -> Result<lsp::WorkspaceEditOrNull> {
        let source = self.file(&params.text_document.uri)?;
        let positions = self.converters.from_lsp_position_for_source_file(
            self.program,
            source,
            &params.position,
            FEATURE_RENAME,
        )?;
        let files = self
            .program
            .files()
            .iter()
            .map(|file| file.source())
            .collect::<Vec<_>>();
        let mut edits = Vec::new();
        let mut eligible_projection = false;
        for mapped in positions {
            if !mapped.mapped.fidelity.is_exact() {
                continue;
            }
            let mut syntax = Syntax::new(self.view(mapped.script)?, mapped.script)?;
            let original = syntax
                .nav()
                .get_touching_property_name(i64::from(mapped.mapped.position))?;
            let original = crate::meaning::adjusted_location(syntax.view, original, true)?;
            if !eligible(syntax.view, original)? {
                continue;
            }
            if !self
                .rename_node(
                    c,
                    mapped.script,
                    original,
                    &params.new_name,
                    options,
                    locale,
                )?
                .is_some_and(|info| info.can_rename)
            {
                return Ok(lsp::WorkspaceEditOrNull::default());
            }
            eligible_projection = true;
            let single_quote = crate::inlay_hints::single_quote(&syntax, options.quote)?;
            let mut state = SearchState::new(
                self,
                c,
                files.clone(),
                ReferenceOptions {
                    rename: true,
                    aliases: options.aliases,
                    adjust: true,
                    ..Default::default()
                },
            );
            for group in state.for_node(original, i64::from(mapped.mapped.position))? {
                for entry in group.entries {
                    self.check_canceled()?;
                    if let Some(node) = entry.node {
                        let view = self.view(node)?;
                        if !options.import_paths
                            && ast::is_string_literal_like(&view.node(node)?)
                            && modules::import_from_module_specifier(view, node)?.is_some()
                        {
                            continue;
                        }
                    }
                    let range = self.entry_range(&entry)?;
                    let (location, fidelity) =
                        self.file_location(&self.source(entry.source)?, range, None)?;
                    if !fidelity.is_exact() {
                        continue;
                    }
                    let new_text = self.rename_text(
                        c,
                        original,
                        &entry,
                        &params.new_name,
                        single_quote,
                        options.aliases,
                    )?;
                    edits.push((
                        location.uri,
                        lsp::TextEdit {
                            range: location.range,
                            new_text,
                        },
                    ));
                }
            }
        }
        Ok(if eligible_projection {
            deduplicate(edits)
        } else {
            lsp::WorkspaceEditOrNull::default()
        })
    }
    // port: tsc/internal/ls/rename.go:LanguageService.getTextForRename
    fn rename_text(
        &self,
        c: &mut Operation<'_>,
        original: NodeId,
        entry: &ReferenceEntry,
        text: &str,
        single_quote: bool,
        aliases: bool,
    ) -> Result<String> {
        let Some(node) = entry.node else {
            return Ok(text.into());
        };
        let view = self.view(node)?;
        let n = view.node(node)?;
        let Some(parent) = n.parent() else {
            return Ok(text.into());
        };
        let p = view.node(parent)?;
        let view_original = self.view(original)?;
        let o = view_original.node(original)?;
        if aliases && (o.kind() == K::Identifier || ast::is_string_literal_like(&o)) {
            let name = self.view(original)?.node_text(original)?;
            let name = String::from_utf8_lossy(name.as_bytes());
            let shorthand = p.kind() == K::ShorthandPropertyAssignment;
            let binding = h::binding_without_property(view, parent)?
                && p.name() == Some(node)
                && p.data_source()
                    .as_binding_element()
                    .is_some_and(|d| d.dot_dot_dot_token().is_none());
            if shorthand || binding {
                if entry.kind == EntryKind::LocalFoundProperty {
                    return Ok(format!("{name}: {text}"));
                }
                if entry.kind == EntryKind::PropertyFoundLocal {
                    return Ok(format!("{text}: {name}"));
                }
                if shorthand {
                    if let Some(grand) = p.parent() {
                        let grand = view.node(grand)?;
                        if grand.kind() == K::ObjectLiteralExpression {
                            if let Some(binary) = grand.parent() {
                                if let Some(data) =
                                    view.node(binary)?.data_source().as_binary_expression()
                                {
                                    if let Some(left) = data.left() {
                                        if tsr_ast::is_module_exports_access_expression(view, left)?
                                        {
                                            return Ok(format!("{name}: {text}"));
                                        }
                                    }
                                }
                            }
                        }
                    }
                    return Ok(format!("{text}: {name}"));
                }
                return Ok(format!("{name}: {text}"));
            }
            if p.kind() == K::ImportSpecifier && p.property_name().is_none() {
                let original_symbol = if let Some(export) = o.parent().filter(|id| {
                    view_original
                        .node(*id)
                        .is_ok_and(|n| n.kind() == K::ExportSpecifier)
                }) {
                    c.get_export_specifier_local_target_symbol(export)?
                } else {
                    c.get_symbol_at_location(original)?
                };
                if let Some(symbol) = original_symbol {
                    if h::declarations(c, symbol)?.contains(&parent) {
                        return Ok(format!("{name} as {text}"));
                    }
                }
            } else if p.kind() == K::ExportSpecifier && p.property_name().is_none() {
                if original == node
                    || c.get_symbol_at_location(original)? == c.get_symbol_at_location(node)?
                {
                    return Ok(format!("{name} as {text}"));
                }
                return Ok(format!("{text} as {name}"));
            }
        }
        if n.kind() == K::NumericLiteral && ast::is_access_expression(&p) {
            let quote = if single_quote { '\'' } else { '"' };
            return Ok(format!("{quote}{text}{quote}"));
        }
        Ok(text.into())
    }
}

// port: tsc/internal/ls/rename.go:deduplicateRenameEdits
fn deduplicate(edits: Vec<(lsp::DocumentUri, lsp::TextEdit)>) -> lsp::WorkspaceEditOrNull {
    let mut seen = HashMap::new();
    let mut changes: HashMap<lsp::DocumentUri, Vec<Option<Box<lsp::TextEdit>>>> = HashMap::new();
    for (uri, edit) in edits {
        let key = (
            uri.clone(),
            edit.range.start.line,
            edit.range.start.character,
            edit.range.end.line,
            edit.range.end.character,
        );
        if let Some(text) = seen.get(&key) {
            if *text != edit.new_text {
                return lsp::WorkspaceEditOrNull::default();
            }
            continue;
        }
        seen.insert(key, edit.new_text.clone());
        changes.entry(uri).or_default().push(Some(Box::new(edit)));
    }
    lsp::WorkspaceEditOrNull {
        workspace_edit: Some(Box::new(lsp::WorkspaceEdit {
            changes: Some(Box::new(changes)),
            ..Default::default()
        })),
    }
}
