use crate::{
    completion_context::Context, completion_items, syntax::Syntax, CompletionOptions,
    LanguageService, Result,
};
use std::{collections::HashMap, sync::Arc};
use tsr_ast::{span_map::FEATURE_COMPLETION, symbol_flags as sf, SyntaxKind as K};
use tsr_autoimport::{index::Named, Export, Registry};
use tsr_checker::{Operation, SymbolRef};
use tsr_lsproto as lsp;

impl LanguageService<'_> {
    /// Installs a registry built from this request's program. The project owns
    /// its publication slot; replacing a program cannot publish into an older snapshot.
    pub fn set_auto_import_cache(&mut self, registry: Arc<tsr_autoimport::Cache>) {
        self.auto_imports = registry;
    }
    pub(crate) fn prepare_auto_imports(
        &self,
        checker: &mut Operation<'_>,
        syntax: &Syntax<'_>,
        preferences: &tsr_autoimport::Preferences,
    ) -> Result<Arc<Registry>> {
        let registry = if let Some(registry) =
            self.auto_imports
                .get(self.program, syntax.file.path(), preferences)?
        {
            registry
        } else {
            let mut registry =
                Registry::build(self.program, checker, || self.cancellation.is_canceled())?
                    .ok_or(crate::Error::Canceled)?;
            self.complete_export_metadata(checker, &mut registry, syntax.source)?;
            let host = self
                .completion_host
                .clone()
                .unwrap_or_else(|| self.program.shared_host());
            let dependencies = tsr_autoimport::DependencyTracker::default();
            let host = dependencies.wrap(host);
            let packages = tsr_autoimport::packages::discover(
                self.program,
                checker,
                syntax.file.path(),
                &host,
                preferences,
                || self.cancellation.is_canceled(),
            )
            .map_err(crate::Error::Compiler)?
            .ok_or(crate::Error::Canceled)?;
            for package in packages {
                self.check_canceled()?;
                let counters = tsr_arena::Counters::new();
                let program = package.load(self.program, &counters)?;
                let Some(file) = program.files().first() else {
                    continue;
                };
                let owner = Arc::new(tsr_checker::CheckerOwner::for_program(
                    tsr_arena::CheckerIdentity::new(
                        tsr_arena::Generation::new(&counters),
                        &counters,
                    ),
                    &counters,
                    Arc::new(tsr_compiler::ProgramCheckerHost::new(program.clone())),
                )?);
                let mut checker = owner.operation()?;
                let mut package_registry = Registry::build_package(&program, &mut checker, || {
                    self.cancellation.is_canceled()
                })?
                .ok_or(crate::Error::Canceled)?;
                package.retain_entrypoints(&mut package_registry);
                let service = LanguageService::new(
                    &program,
                    tsr_jsstring::PositionEncoding::Utf16,
                    self.cancellation.clone(),
                );
                service.complete_export_metadata(
                    &mut checker,
                    &mut package_registry,
                    file.source(),
                )?;
                // Package exports are supplied by this independently scoped
                // index, including files also loaded by the user's program.
                let package_paths: std::collections::HashSet<_> = package
                    .entrypoints
                    .iter()
                    .flat_map(|e| {
                        [
                            e.resolved_file_name.clone(),
                            tsr_jsstring::JsString::from_bytes(e.symlink_or_realpath()),
                        ]
                    })
                    .collect();
                registry.index = registry
                    .index
                    .filtered(|e| !package_paths.contains(&e.path));
                for export in package_registry.index.entries() {
                    registry.index.insert(export.clone());
                }
            }
            if let Some(excludes) =
                preferences.file_matcher(self.program.host().use_case_sensitive_file_names())
            {
                registry.index = registry
                    .index
                    .filtered(|export| !excludes.matches(export.path.as_bytes()));
            }
            registry.set_build_preferences(preferences);
            registry.dependencies = dependencies.snapshot();
            registry.requested_file = Some(tsr_jsstring::JsString::from_bytes(syntax.file.path()));
            self.auto_imports.publish(registry)
        };
        Ok(registry)
    }
    fn complete_export_metadata(
        &self,
        checker: &mut Operation<'_>,
        registry: &mut Registry,
        fallback: tsr_ast::NodeId,
    ) -> Result<()> {
        for export in registry.index.entries_mut() {
            if let Some(symbol) = self.export_symbol(checker, export)? {
                let target = checker.skip_alias(symbol)?;
                let location = checker
                    .symbol_declarations(target)?
                    .iter()
                    .flatten()
                    .next()
                    .unwrap_or(fallback);
                export.completion_kind = Some(completion_items::kind(
                    self.symbol_kind(checker, target, location)?,
                ));
                export.modifiers = self.symbol_modifiers(checker, target)?;
            }
        }
        Ok(())
    }
    #[allow(
        clippy::too_many_arguments,
        reason = "The optional import-statement context and output list share the enclosing completion request"
    )]
    pub(crate) fn auto_import_completions(
        &mut self,
        checker: &mut Operation<'_>,
        syntax: &mut Syntax<'_>,
        context: &Context,
        position: i64,
        options: &CompletionOptions,
        list: &mut lsp::CompletionList,
        statement: Option<&crate::completion_imports::ImportStatement>,
    ) -> Result<()> {
        if options.module_exports == Some(false) && statement.is_none()
            || tsr_tspath::is_dynamic_file_name(syntax.file.file_name())
        {
            return Ok(());
        }
        let registry = self.prepare_auto_imports(checker, syntax, &options.auto_import)?;
        let (prefix, usage) = if let Some(previous) = context
            .previous
            .filter(|&n| syntax.view.node(n).is_ok_and(|n| n.kind() == K::Identifier))
        {
            (
                if statement.is_some() && context.previous == context.token {
                    Vec::new()
                } else {
                    syntax.view.node_text(previous)?.as_bytes().to_vec()
                },
                syntax.start(previous)?,
            )
        } else {
            (Vec::new(), position)
        };
        let (usage_range, fidelity) = self.range(
            syntax.source,
            tsr_core::TextRange::new(usage, usage),
            FEATURE_COMPLETION,
        )?;
        if !fidelity.is_exact() {
            return Ok(());
        }
        let shadowed: std::collections::HashSet<_> = list
            .items
            .iter()
            .flatten()
            .filter(|i| i.kind.as_deref() != Some(&lsp::CompletionItemKind::KEYWORD))
            .map(|i| i.label.as_str())
            .collect();
        let mut groups: HashMap<_, Vec<(&Export, lsp::AutoImportFix)>> = HashMap::new();
        for export in registry.search(syntax.file.path(), &prefix) {
            self.check_canceled()?;
            if shadowed.contains(String::from_utf8_lossy(export.name()).as_ref()) {
                continue;
            }
            if !tsr_scanner::is_identifier_text(export.name(), tsr_core::LanguageVariant::STANDARD)
                || tsr_ast::utilities_tail::is_non_contextual_keyword(
                    tsr_scanner::string_to_token(export.name()).into(),
                )
            {
                continue;
            }
            if !export.is_unresolved_alias()
                && (if context.type_only {
                    export.flags & (sf::TYPE | sf::MODULE) == 0
                } else {
                    statement.is_none() && export.flags & sf::VALUE == 0
                })
            {
                continue;
            }
            let key = (
                export.target.as_ref().unwrap_or(&export.id).clone(),
                export.name().to_vec(),
                export.ambient_module_name().to_vec(),
                export.package_name.clone(),
            );
            for fix in tsr_autoimport::fix::fixes(
                self.program,
                checker,
                syntax.source,
                export,
                tsr_autoimport::fix::Usage {
                    type_only: context.type_only,
                    position: Some(usage_range.start.clone()),
                    ..Default::default()
                },
                &options.auto_import,
            )? {
                groups.entry(key.clone()).or_default().push((export, fix));
            }
        }
        let mut chosen = Vec::new();
        for mut group in groups.into_values() {
            group.sort_by(|a, b| rank(&a.1, &b.1));
            if let Some((_, best)) = group.first() {
                let count = group
                    .iter()
                    .take_while(|(_, fix)| rank(fix, best).is_eq())
                    .count();
                group.truncate(count);
                chosen.extend(group);
            }
        }
        chosen.sort_by(|a, b| compare(&a.1, &b.1));
        for (export, fix) in chosen {
            let Some(kind) = export.completion_kind else {
                continue;
            };
            let modifiers = export.modifiers;
            let name = fix.name.clone();
            let mut item = lsp::CompletionItem {
                label: name.clone(),
                kind: Some(Box::new(kind)),
                label_details: Some(Box::new(lsp::CompletionItemLabelDetails {
                    description: Some(Box::new(fix.module_specifier.clone())),
                    ..Default::default()
                })),
                sort_text: Some(Box::new(
                    if modifiers & crate::symbol_display::modifiers::DEPRECATED != 0 {
                        "z16"
                    } else {
                        "16"
                    }
                    .into(),
                )),
                tags: (modifiers & crate::symbol_display::modifiers::DEPRECATED != 0)
                    .then(|| Box::new(vec![lsp::CompletionItemTag::DEPRECATED])),
                data: Some(Box::new(lsp::CompletionItemData {
                    name,
                    position: position as i32,
                    file_name: String::from_utf8_lossy(
                        syntax.file.original_file_name()?.as_bytes(),
                    )
                    .into_owned(),
                    source: fix.module_specifier.clone(),
                    is_import_statement_completion: statement.is_some(),
                    auto_import: Some(Box::new(fix.clone())),
                    ..Default::default()
                })),
                ..Default::default()
            };
            if let Some(statement) = statement {
                let kind = tsr_autoimport::fix::import_kind(
                    self.program,
                    checker,
                    syntax.source,
                    export,
                    true,
                )?;
                let semicolons = tsr_format::FormatFile {
                    view: syntax.view,
                    source: syntax.source,
                    jsdoc: &mut syntax.docs,
                }
                .probably_uses_semicolons()?;
                let quote = crate::inlay_hints::single_quote(syntax, options.quote)?;
                let text = crate::completion_imports::insert_text(
                    statement,
                    kind,
                    &fix.name,
                    &fix.module_specifier,
                    options.snippets,
                    semicolons,
                    quote,
                );
                let (range, fidelity) = self.range(
                    syntax.source,
                    statement.replacement.expect("import completion range"),
                    FEATURE_COMPLETION,
                )?;
                if !fidelity.is_exact() {
                    continue;
                }
                item.text_edit = Some(Box::new(lsp::TextEditOrInsertReplaceEdit {
                    text_edit: Some(Box::new(lsp::TextEdit {
                        range,
                        new_text: text,
                    })),
                    ..Default::default()
                }));
                item.filter_text = Some(Box::new(fix.name.clone()));
                item.sort_text = Some(Box::new("11".into()));
                item.insert_text_format = options
                    .snippets
                    .then(|| Box::new(lsp::InsertTextFormat::SNIPPET));
            }
            list.items.push(Some(Box::new(item)));
        }
        Ok(())
    }
    pub(crate) fn export_symbol(
        &self,
        checker: &mut Operation<'_>,
        export: &Export,
    ) -> Result<Option<SymbolRef>> {
        let module = if export.module_file_name.as_bytes().is_empty() {
            checker.try_find_ambient_module(export.id.module.as_bytes())?
        } else if let Some(file) = self.program.source_file(export.module_file_name.as_bytes()) {
            checker.bound_symbol_of_node(file.source())?
        } else {
            None
        };
        let Some(module) = module else {
            return Ok(None);
        };
        Ok(tsr_autoimport::lookup_export(
            checker,
            module,
            export.id.name.as_bytes(),
        )?)
    }
    pub(crate) fn resolve_auto_import(
        &mut self,
        mut item: lsp::CompletionItem,
        fix: &lsp::AutoImportFix,
        syntax: &mut Syntax<'_>,
        options: &CompletionOptions,
    ) -> Result<lsp::CompletionItem> {
        let quote = crate::inlay_hints::single_quote(syntax, options.quote)?;
        let usage = fix
            .usage_position
            .as_deref()
            .map(|position| {
                self.converters.from_lsp_position_for_source_file(
                    self.program,
                    syntax.source,
                    position,
                    FEATURE_COMPLETION,
                )
            })
            .transpose()?
            .and_then(|p| {
                p.first()
                    .filter(|p| p.mapped.fidelity.is_exact())
                    .map(|p| i64::from(p.mapped.position))
            });
        let settings = crate::completion_snippets::settings(options, syntax)?;
        let semicolons = settings.semicolons != tsr_format::SemicolonPreference::Remove;
        let (edits, message) = tsr_autoimport::edits::edits(
            syntax.view,
            syntax.source,
            fix,
            &tsr_autoimport::edits::Options {
                format: &options.format,
                locale: &options.locale,
                usage,
                semicolons,
                single_quote: quote,
                prefer_type_only: options.prefer_type_only,
                verbatim: self.program.options().verbatim_module_syntax.is_true(),
                newline: self.program.options().new_line.as_str(),
            },
        )?;
        let mut result = Vec::new();
        for edit in edits {
            let (range, fidelity) = self.range(
                syntax.source,
                tsr_core::TextRange::new(edit.start, edit.end),
                FEATURE_COMPLETION,
            )?;
            if !fidelity.is_exact() {
                return Ok(item);
            }
            result.push(Some(Box::new(lsp::TextEdit {
                range,
                new_text: edit.text,
            })));
        }
        item.additional_text_edits = Some(Box::new(result));
        item.detail = Some(Box::new(message));
        Ok(item)
    }
}
pub(crate) fn rank(a: &lsp::AutoImportFix, b: &lsp::AutoImportFix) -> std::cmp::Ordering {
    a.kind.0.cmp(&b.kind.0).then_with(|| {
        a.module_specifier
            .bytes()
            .filter(|&b| b == b'/')
            .count()
            .cmp(&b.module_specifier.bytes().filter(|&b| b == b'/').count())
    })
}
pub(crate) fn compare(a: &lsp::AutoImportFix, b: &lsp::AutoImportFix) -> std::cmp::Ordering {
    rank(a, b)
        .then_with(|| {
            b.module_specifier
                .starts_with("./")
                .cmp(&a.module_specifier.starts_with("./"))
        })
        .then_with(|| a.module_specifier.cmp(&b.module_specifier))
        .then_with(|| a.import_kind.0.cmp(&b.import_kind.0))
}
