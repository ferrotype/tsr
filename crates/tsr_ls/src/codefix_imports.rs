use crate::{
    code_actions::{localized, Fix, Provider},
    syntax::Syntax,
    CompletionOptions, LanguageService, OrganizeOptions, Result,
};
use tsr_ast::{symbol_flags as sf, NodeId, SyntaxKind as K};
use tsr_autoimport::index::Named;
use tsr_checker::{Operation, SymbolRef};
use tsr_core::TextRange;
use tsr_lsproto as lsp;

struct Info {
    fix: lsp::AutoImportFix,
    alias: Option<NodeId>,
    namespace: bool,
    name: Vec<u8>,
}
impl LanguageService<'_> {
    // port: tsc/internal/ls/codeactions_importfixes.go:getImportCodeActions
    #[allow(
        clippy::too_many_arguments,
        reason = "Import action context includes the originating diagnostic and both editor preference families"
    )]
    pub(crate) fn import_fixes(
        &mut self,
        c: &mut Operation<'_>,
        source: NodeId,
        span: TextRange,
        code: i32,
        diagnostic: Option<&lsp::Diagnostic>,
        options: &CompletionOptions,
        organize: &OrganizeOptions,
        locale: &tsr_locale::Locale,
    ) -> Result<Vec<Fix>> {
        let mut syntax = Syntax::new(self.view(source)?, source)?;
        let mut infos = self.import_fix_infos(c, &mut syntax, span.pos(), code, options)?;
        if code == 1361 && infos.len() > 1 {
            if let Some(message) = diagnostic.and_then(|d| d.message.string.as_deref()) {
                if infos
                    .iter()
                    .any(|i| message.contains(&format!("'{}'", String::from_utf8_lossy(&i.name))))
                {
                    infos.retain(|i| {
                        message.contains(&format!("'{}'", String::from_utf8_lossy(&i.name)))
                    });
                }
            }
        }
        let mut actions = Vec::new();
        for info in infos {
            self.check_canceled()?;
            if let Some(alias) = info.alias {
                if let Some(fix) =
                    self.promote_import(&mut syntax, alias, options, organize, locale)?
                {
                    actions.push(fix);
                }
                continue;
            }
            let usage = info
                .fix
                .usage_position
                .as_deref()
                .map(|p| {
                    self.converters.from_lsp_position_for_source_file(
                        self.program,
                        source,
                        p,
                        tsr_ast::span_map::FEATURE_CODE_ACTIONS,
                    )
                })
                .transpose()?
                .and_then(|p| p.first().map(|p| i64::from(p.mapped.position)));
            let settings = crate::completion_snippets::settings(options, &syntax)?;
            let single = crate::inlay_hints::single_quote(&syntax, options.quote)?;
            let (edits, title) = tsr_autoimport::edits::edits(
                syntax.view,
                source,
                &info.fix,
                &tsr_autoimport::edits::Options {
                    format: &options.format,
                    locale,
                    usage,
                    single_quote: single,
                    semicolons: settings.semicolons != tsr_format::SemicolonPreference::Remove,
                    prefer_type_only: options.prefer_type_only,
                    verbatim: self.program.options().verbatim_module_syntax.is_true(),
                    newline: self.program.options().new_line.as_str(),
                },
            )?;
            let mut tracker = crate::change::Tracker::default();
            for edit in edits {
                let range = TextRange::new(edit.start, edit.end);
                let text = crate::change_nodes::reindent(
                    &syntax.file,
                    range,
                    &crate::change_nodes::NodeOptions::default(),
                    self.program.options().new_line.as_str(),
                    edit.text,
                );
                tracker.replace_text(source, range, text);
            }
            let changes = tracker.finish(self)?;
            if !changes.unmappable.is_empty() {
                continue;
            }
            let uri =
                lsp::DocumentUri::from_file_name(syntax.file.original_file_name()?.as_bytes());
            let edits = changes.edits.get(&uri).cloned().unwrap_or_default();
            actions.push(Fix { title, edits });
        }
        Ok(actions)
    }
    // port: tsc/internal/ls/codeactions_importfixes.go:getAllImportCodeActions
    pub(crate) fn all_import_fixes(
        &mut self,
        c: &mut Operation<'_>,
        source: NodeId,
        options: &CompletionOptions,
        _organize: &OrganizeOptions,
        locale: &tsr_locale::Locale,
    ) -> Result<Option<Fix>> {
        let mut syntax = Syntax::new(self.view(source)?, source)?;
        if tsr_tspath::is_dynamic_file_name(syntax.file.file_name()) {
            return Ok(None);
        }
        let file = self
            .program
            .file_of_node(source)
            .ok_or(tsr_arena::Error::WrongOwner)?;
        let diagnostics =
            self.program
                .semantic_diagnostics_in(c, file, Some(&self.cancellation))?;
        let diagnostics = self.program.filter_and_sort_diagnostics(&diagnostics)?;
        let codes = Provider::Import.codes();
        let mut adder = tsr_autoimport::ImportAdder::default();
        // Namespace/JSDoc qualification needs each diagnostic's own position.
        let mut individual = crate::change::Tracker::default();
        for diagnostic in diagnostics {
            if !diagnostic.source.is_empty() || !codes.contains(&diagnostic.code) {
                continue;
            }
            let Some(info) = self
                .import_fix_infos(
                    c,
                    &mut syntax,
                    diagnostic.loc.pos(),
                    diagnostic.code,
                    options,
                )?
                .into_iter()
                .next()
            else {
                continue;
            };
            if info.alias.is_some() {
                continue;
            }
            if matches!(
                info.fix.kind,
                lsp::AutoImportFixKind::USE_NAMESPACE | lsp::AutoImportFixKind::JSDOC_TYPE_IMPORT
            ) {
                let prefix = if info.fix.kind == lsp::AutoImportFixKind::USE_NAMESPACE {
                    format!("{}.", info.fix.namespace_prefix)
                } else {
                    format!(
                        "import({}).",
                        tsr_autoimport::edits::quote_module(
                            &info.fix.module_specifier,
                            crate::inlay_hints::single_quote(&syntax, options.quote)?
                        )
                    )
                };
                let token = syntax.nav().get_token_at_position(diagnostic.loc.pos())?;
                let start = syntax.start(token)?;
                individual.replace_text(source, TextRange::new(start, start), prefix);
            } else {
                adder.add(
                    info.fix,
                    self.program.options().verbatim_module_syntax.is_true(),
                );
            }
        }
        let mut changes = individual.finish(self)?;
        if !changes.unmappable.is_empty() {
            return Ok(None);
        }
        let mut edits = changes
            .edits
            .remove(&lsp::DocumentUri::from_file_name(
                syntax.file.original_file_name()?.as_bytes(),
            ))
            .unwrap_or_default();
        edits.extend(
            self.import_adder_action_edits(&syntax, options, &adder)?
                .into_iter()
                .flatten()
                .map(|e| *e),
        );
        if edits.is_empty() {
            return Ok(None);
        }
        Ok(Some(Fix {
            title: localized(tsr_diagnostics::Add_all_missing_imports, locale, &[]),
            edits,
        }))
    }
    // port: tsc/internal/ls/codeactions_importfixes.go:getFixInfos
    fn import_fix_infos(
        &mut self,
        c: &mut Operation<'_>,
        syntax: &mut Syntax<'_>,
        pos: i64,
        code: i32,
        options: &CompletionOptions,
    ) -> Result<Vec<Info>> {
        if tsr_tspath::is_dynamic_file_name(syntax.file.file_name()) {
            return Ok(Vec::new());
        }
        let token = syntax.nav().get_token_at_position(pos)?;
        let read = syntax.view.node(token)?;
        if code != 2686 && read.kind() != K::Identifier {
            return Ok(Vec::new());
        }
        let names = if code == 2686 {
            Vec::new()
        } else {
            symbol_names(c, syntax, token, self.program.options().jsx)?
        };
        if code == 1361 {
            let mut infos = Vec::new();
            for (name, type_only) in names {
                if !type_only {
                    continue;
                }
                if let Some(symbol) = c.resolve_name(&name, Some(token), sf::VALUE, true)? {
                    if let Some(alias) = c.get_type_only_alias_declaration(symbol)? {
                        if self
                            .program
                            .file_of_node(alias)
                            .is_some_and(|f| f.source() == syntax.source)
                        {
                            infos.push(Info {
                                fix: lsp::AutoImportFix::default(),
                                alias: Some(alias),
                                namespace: false,
                                name,
                            });
                        }
                    }
                }
            }
            return Ok(infos);
        }
        let registry = self.prepare_auto_imports(c, syntax, &options.auto_import)?;
        let type_site =
            tsr_ast::utilities_modules::is_valid_type_only_alias_use_site(syntax.view, token)?;
        let token_start = syntax.start(token)? as i32;
        let (usage, fidelity) = self.converters.to_lsp_position(
            &crate::converters::Script {
                file_name: syntax.file.file_name(),
                text: syntax.file.text().as_bytes(),
                original_file_name: syntax.file.original_file_name()?.as_bytes(),
                original_text: syntax.file.original_text(),
                span_map: syntax.file.span_map(),
            },
            token_start,
        );
        if !fidelity.is_exact() {
            return Ok(Vec::new());
        }
        let mut infos = Vec::new();
        if code == 2686 {
            if let Some(symbol) = umd_symbol(c, syntax, token)? {
                let id = tsr_autoimport::export_id_for_symbol(self.program, c, symbol)?;
                for export in registry
                    .index
                    .entries()
                    .iter()
                    .filter(|e| Some(&e.id) == id.as_ref())
                {
                    for fix in tsr_autoimport::fix::fixes(
                        self.program,
                        c,
                        syntax.source,
                        export,
                        tsr_autoimport::fix::Usage {
                            type_only: type_site,
                            ..Default::default()
                        },
                        &options.auto_import,
                    )? {
                        infos.push(Info {
                            fix,
                            alias: None,
                            namespace: false,
                            name: Vec::new(),
                        });
                    }
                }
            }
        } else {
            for (name, type_only) in names {
                if type_only || name == b"default" {
                    continue;
                }
                let own = syntax.view.node_text(token)?.as_bytes() == name;
                let jsx = own && tsr_ast::utilities_targets::is_jsx_tag_name(syntax.view, token)?;
                for export in registry.index.find(&name, !jsx) {
                    if jsx && export.name() != name && !export.is_renameable() {
                        continue;
                    }
                    for fix in tsr_autoimport::fix::fixes(
                        self.program,
                        c,
                        syntax.source,
                        export,
                        tsr_autoimport::fix::Usage {
                            type_only: type_site,
                            jsx,
                            position: Some(usage.clone()),
                        },
                        &options.auto_import,
                    )? {
                        infos.push(Info {
                            fix,
                            alias: None,
                            namespace: !own,
                            name: name.clone(),
                        });
                    }
                }
            }
        }
        infos.sort_by(|a, b| {
            a.namespace
                .cmp(&b.namespace)
                .then_with(|| crate::auto_imports::compare(&a.fix, &b.fix))
        });
        Ok(infos)
    }
}
// port: tsc/internal/ls/codeactions_importfixes.go:getSymbolNamesToImport
fn symbol_names(
    c: &mut Operation<'_>,
    syntax: &Syntax<'_>,
    token: NodeId,
    jsx: tsr_core::JsxEmit,
) -> Result<Vec<(Vec<u8>, bool)>> {
    let read = syntax.view.node(token)?;
    let name = syntax.view.node_text(token)?.as_bytes().to_vec();
    let type_only = |c: &mut Operation<'_>, name: &[u8]| -> Result<bool> {
        Ok(
            if let Some(s) = c.resolve_name(name, Some(token), sf::VALUE, true)? {
                c.get_type_only_alias_declaration(s)?.is_some()
            } else {
                false
            },
        )
    };
    if let Some(parent) = read.parent() {
        let p = syntax.view.node(parent)?;
        if matches!(
            p.kind().known(),
            Some(K::JsxOpeningElement | K::JsxSelfClosingElement | K::JsxClosingElement)
        ) && p.tag_name() == Some(token)
            && matches!(
                jsx,
                tsr_core::JsxEmit::REACT | tsr_core::JsxEmit::REACT_NATIVE
            )
        {
            let namespace = c
                .get_jsx_namespace(Some(syntax.source))?
                .as_bytes()
                .to_vec();
            let intrinsic = tsr_scanner::is_intrinsic_jsx_name(&name);
            let ns = c.resolve_name(&namespace, Some(token), sf::VALUE, true)?;
            let needs = if let Some(symbol) = ns.filter(|_| !intrinsic) {
                let mut typed = false;
                for d in c.symbol_declarations(symbol)?.iter().flatten() {
                    if tsr_ast::utilities_modules::is_type_only_import_or_export_declaration(
                        syntax.view,
                        d,
                    )? {
                        typed = true;
                        break;
                    }
                }
                typed && c.symbol(symbol)?.flags() & sf::VALUE == 0
            } else {
                true
            };
            if needs {
                let mut result = Vec::new();
                if !intrinsic {
                    match c.resolve_name(&name, Some(token), sf::VALUE, false)? {
                        None => result.push((name, false)),
                        Some(s) => {
                            if c.get_type_only_alias_declaration(s)?.is_some() {
                                result.push((name, true));
                            }
                        }
                    }
                }
                let typed = type_only(c, &namespace)?;
                result.push((namespace, typed));
                return Ok(result);
            }
        }
    }
    let typed = type_only(c, &name)?;
    Ok(vec![(name, typed)])
}
// port: tsc/internal/ls/codeactions_importfixes.go:getUmdSymbol
fn umd_symbol(
    c: &mut Operation<'_>,
    syntax: &Syntax<'_>,
    token: NodeId,
) -> Result<Option<SymbolRef>> {
    let is_umd = |c: &Operation<'_>, symbol: SymbolRef| -> Result<bool> {
        Ok(c.symbol_declarations(symbol)?
            .iter()
            .flatten()
            .next()
            .is_some_and(|d| {
                c.node(d)
                    .is_ok_and(|n| n.kind() == K::NamespaceExportDeclaration)
            }))
    };
    let read = syntax.view.node(token)?;
    if read.kind() == K::Identifier {
        let symbol = c.get_resolved_symbol(token)?;
        if is_umd(c, symbol)? {
            return Ok(Some(symbol));
        }
    }
    if let Some(parent) = read.parent() {
        let p = syntax.view.node(parent)?;
        if matches!(
            p.kind().known(),
            Some(K::JsxOpeningElement | K::JsxSelfClosingElement)
        ) && p.tag_name() == Some(token)
            || p.kind() == K::JsxOpeningFragment
        {
            let name = c.get_jsx_namespace(Some(parent))?;
            if let Some(s) = c.resolve_name(
                name.as_bytes(),
                Some(if p.kind() == K::JsxOpeningFragment {
                    parent
                } else {
                    token
                }),
                sf::VALUE,
                false,
            )? {
                if is_umd(c, s)? {
                    return Ok(Some(s));
                }
            }
        }
    }
    Ok(None)
}
