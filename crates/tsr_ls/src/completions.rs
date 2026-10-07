//! Completion collection and resolution over the request's retained program.
//! Wire data contains source coordinates and names, never checker-local IDs.
use crate::{
    completion_context::{Container, Context},
    completion_items as items,
    completion_keywords::{self as keywords, Filter},
    symbol_display::modifiers,
    syntax::Syntax,
    LanguageService, Result,
};
use std::collections::HashSet;
use tsr_ast::{
    span_map::FEATURE_COMPLETION, symbol_flags as sf, utilities as ast, NodeId, SyntaxKind as K,
};
use tsr_checker::{Operation, SymbolRef, TypeRef, VerbosityContext};
use tsr_compiler::diagnostic_writer::DiagnosticSources;
use tsr_core::TextRange;
use tsr_lsproto as lsp;
use tsr_printer::EmitTextWriter;

pub const COMPLETION_TRIGGER_CHARACTERS: &[&str] =
    &[".", "\"", "'", "`", "/", "@", "<", "#", " ", "*"];
#[derive(Clone, Debug, Default)]
pub struct CompletionOptions {
    pub snippets: bool,
    pub commit_characters: bool,
    pub insert_replace: bool,
    pub default_commit_characters: bool,
    pub default_edit_range: bool,
    pub label_details: bool,
    pub markdown: bool,
    pub module_exports: Option<bool>,
    pub prefer_type_only: bool,
    pub quote: crate::QuotePreference,
    pub enable_jsdoc: Option<bool>,
    pub generate_return: Option<bool>,
    pub newline: Option<String>,
    pub format: tsr_format::FormatCodeSettings,
    pub locale: tsr_locale::Locale,
    pub automatic_optional_chain: Option<bool>,
    pub jsx_attribute_style: Option<String>,
    pub auto_import: tsr_autoimport::Preferences,
    pub import_statements: Option<bool>,
    pub class_member_snippets: bool,
    pub object_method_snippets: bool,
    /// Organize-imports settings, which order specifiers added by import edits.
    pub organize: crate::OrganizeOptions,
}

pub(crate) struct Candidate {
    pub symbol: SymbolRef,
    pub sort: &'static str,
    pub nullable: bool,
    pub this_member: bool,
    pub promise: bool,
    /// The accessible name a computed symbol property starts with, as `[N]`.
    pub symbol_member: bool,
}
pub(crate) fn properties(checker: &mut Operation<'_>, ty: TypeRef) -> Result<Vec<SymbolRef>> {
    Ok(
        if checker.type_flags(ty)? & tsr_checker::type_flags::UNION != 0 {
            let types = checker.constituents(ty)?;
            checker.get_all_possible_properties_of_types(&types)?
        } else {
            checker.get_apparent_properties(ty)?
        },
    )
}
pub(crate) fn symbol_name(checker: &Operation<'_>, symbol: SymbolRef) -> Result<String> {
    let name = checker.symbol_display_name(symbol)?;
    Ok(String::from_utf8_lossy(name.as_bytes()).into_owned())
}
fn quote(name: &str) -> String {
    String::from_utf8(
        tsr_json::marshal(&name.to_owned(), tsr_json::Options::default())
            .expect("string JSON encoding"),
    )
    .expect("JSON is UTF-8")
}

impl LanguageService<'_> {
    // port: tsc/internal/ls/completions.go:LanguageService.ProvideCompletion
    pub fn completion(
        &mut self,
        checker: &mut Operation<'_>,
        params: &lsp::CompletionParams,
        options: &CompletionOptions,
    ) -> Result<lsp::CompletionItemsOrListOrNull> {
        let source = self.file(&params.text_document.uri)?;
        let projections = self.converters.from_lsp_position_for_source_file(
            self.program,
            source,
            &params.position,
            FEATURE_COMPLETION,
        )?;
        let Some(projection) = projections.first().filter(|p| p.mapped.fidelity.is_exact()) else {
            return Ok(lsp::CompletionItemsOrListOrNull::default());
        };
        let source = projection.script;
        let position = i64::from(projection.mapped.position);
        let mut response = self.completion_worker(checker, params, options, source, position)?;
        if let Some(list) = response.list.as_deref_mut() {
            self.completion_data(source, position, list)?;
            if self.source(source)?.span_map().is_some() {
                let mut syntax = Syntax::new(self.view(source)?, source)?;
                self.filter_content_mapped_auto_imports(&mut syntax, options, list)?;
            }
        }
        Ok(response)
    }

    fn completion_worker(
        &mut self,
        checker: &mut Operation<'_>,
        params: &lsp::CompletionParams,
        options: &CompletionOptions,
        source: NodeId,
        position: i64,
    ) -> Result<lsp::CompletionItemsOrListOrNull> {
        let mut syntax = Syntax::new(self.view(source)?, source)?;
        if options.module_exports != Some(false) && self.auto_imports.is_prepared() {
            self.prepare_auto_imports(checker, &syntax, &options.auto_import)?;
        }
        let mut context = Context::collect(&mut syntax, position)?;
        context.refine_type_arguments(&mut syntax, checker)?;
        let in_string =
            crate::string_completions::in_string(&mut syntax, context.previous, position)?;
        if let Some(trigger) = params
            .context
            .as_deref()
            .and_then(|c| c.trigger_character.as_deref())
        {
            if in_string.is_none()
                && !Self::completion_trigger(&mut syntax, &context, position, trigger)?
            {
                return Ok(lsp::CompletionItemsOrListOrNull::default());
            }
            if trigger == " " {
                if options.import_statements != Some(true) {
                    return Ok(lsp::CompletionItemsOrListOrNull::default());
                }
                return Ok(lsp::CompletionItemsOrListOrNull {
                    list: Some(Box::new(lsp::CompletionList {
                        is_incomplete: true,
                        ..Default::default()
                    })),
                    ..Default::default()
                });
            }
        }
        if let Some(list) = self.jsdoc_snippet(&mut syntax, position, options)? {
            return Ok(lsp::CompletionItemsOrListOrNull {
                list: Some(Box::new(list)),
                ..Default::default()
            });
        }
        if let Some(literal) =
            crate::string_completions::in_string(&mut syntax, context.previous, position)?
        {
            let list = self.string_completions(checker, &mut syntax, literal, position, options)?;
            return Ok(lsp::CompletionItemsOrListOrNull {
                list: list.map(Box::new),
                ..Default::default()
            });
        }
        if let Some(list) =
            self.reference_path_completions(checker, &mut syntax, position, options)?
        {
            return Ok(lsp::CompletionItemsOrListOrNull {
                list: Some(Box::new(list)),
                ..Default::default()
            });
        }
        let in_comment = syntax.in_comment(position)?;
        if in_comment {
            match self.jsdoc_completions(checker, &mut syntax, position, options)? {
                crate::jsdoc_completions::JsDocCompletion::Code => {}
                crate::jsdoc_completions::JsDocCompletion::Prose => {
                    return Ok(lsp::CompletionItemsOrListOrNull::default());
                }
                crate::jsdoc_completions::JsDocCompletion::List(list) => {
                    return Ok(lsp::CompletionItemsOrListOrNull {
                        list: Some(Box::new(list)),
                        ..Default::default()
                    });
                }
            }
            context.type_only = true;
            if context.container.is_none() {
                context.filter = Filter::Type;
            }
        }
        if let Some(list) =
            self.closing_tag_completion(&mut syntax, &context, &params.position, options)?
        {
            return Ok(lsp::CompletionItemsOrListOrNull {
                list: Some(Box::new(list)),
                ..Default::default()
            });
        }
        let import_info = crate::completion_imports::context(&mut syntax, context.token)?;
        if import_info.keyword_only
            || import_info.replacement.is_some() && options.import_statements == Some(true)
        {
            context.new_identifier = import_info.new_identifier;
            context.type_only = import_info.top_level_type_only || import_info.specifier_type_only;
            let mut list = lsp::CompletionList::default();
            if let Some(keyword) = import_info.keyword {
                list.items.push(Some(Box::new(lsp::CompletionItem {
                    label: tsr_scanner::token_to_string(keyword).into(),
                    kind: Some(Box::new(lsp::CompletionItemKind::KEYWORD)),
                    sort_text: Some(Box::new("15".into())),
                    ..Default::default()
                })));
            }
            if !import_info.keyword_only {
                self.auto_import_completions(
                    checker,
                    &mut syntax,
                    &context,
                    position,
                    options,
                    &mut list,
                    Some(&import_info),
                    &HashSet::new(),
                )?;
            }
            let replacement = if import_info.keyword_only {
                context
                    .previous
                    .map(|p| self.completion_replacement(&mut syntax, p))
                    .transpose()?
                    .flatten()
            } else {
                self.completion_replacement(&mut syntax, context.location)?
            };
            items::defaults(
                &mut list,
                options,
                &params.position,
                replacement,
                if import_info.new_identifier {
                    &[]
                } else {
                    crate::completion_context::ALL
                },
            );
            return Ok(lsp::CompletionItemsOrListOrNull {
                list: Some(Box::new(list)),
                ..Default::default()
            });
        }
        if context.blocked(&mut syntax, checker, position)? {
            return Ok(lsp::CompletionItemsOrListOrNull::default());
        }
        if let Some(list) =
            self.label_completions(&mut syntax, &context, &params.position, options)?
        {
            return Ok(lsp::CompletionItemsOrListOrNull {
                list: (!list.items.is_empty()).then(|| Box::new(list)),
                ..Default::default()
            });
        }
        let candidates =
            self.completion_symbols(checker, &mut syntax, &mut context, position, options)?;
        let checked = !syntax.file.is_js()
            || ast::is_check_js_enabled_for_file(&syntax.file, self.program.options());
        let filter = if context.member.is_some() {
            Filter::None
        } else {
            context.filter
        };
        if checked && candidates.is_empty() && !context.new_identifier && filter == Filter::None {
            return Ok(lsp::CompletionItemsOrListOrNull::default());
        }
        let recommended = self.recommended_completion(checker, &mut syntax, &context, position)?;
        let mut list = lsp::CompletionList::default();
        let used_cases =
            crate::completion_switch::expression_case_values(checker, &syntax, context.token)?;
        let mut names = HashSet::new();
        let mut shadowed_names = HashSet::new();
        let mut method_snippets = Vec::new();
        let js_file = syntax.file.is_js();
        for candidate in candidates {
            self.check_canceled()?;
            // In a JS value location, symbols that seem type-only are skipped.
            if js_file && !context.type_only && Self::appears_type_only(checker, candidate.symbol)?
            {
                continue;
            }
            if let Some(used) = &used_cases {
                if checker.symbol(candidate.symbol)?.flags() & sf::ENUM_MEMBER != 0 {
                    if let Some(decl) = checker.symbol(candidate.symbol)?.value_declaration() {
                        if checker
                            .constant_value(decl)?
                            .is_some_and(|value| used.contains_value(&value))
                        {
                            continue;
                        }
                    }
                }
            }
            if let Some(mut item) = self.completion_symbol_item(
                checker,
                &mut syntax,
                &context,
                &candidate,
                position,
                options,
            )? {
                if recommended == Some(candidate.symbol)
                    || checker.symbol(candidate.symbol)?.flags() & sf::EXPORT_VALUE != 0
                        && recommended
                            == Some(checker.get_export_symbol_of_symbol(candidate.symbol)?)
                {
                    item.preselect = Some(Box::new(true));
                }
                if let Some(snippet) = self.object_method_snippet(
                    checker,
                    &syntax,
                    &context,
                    candidate.symbol,
                    item.clone(),
                    options,
                )? {
                    method_snippets.push(Some(Box::new(snippet)));
                }
                let Some(item) = self.class_member_snippet(
                    checker,
                    &mut syntax,
                    &context,
                    candidate.symbol,
                    position,
                    item,
                    options,
                )?
                else {
                    continue;
                };
                if names.insert(item.label.clone()) {
                    let symbol = checker.symbol(candidate.symbol)?;
                    let local = checker
                        .symbol_declarations(candidate.symbol)?
                        .iter()
                        .flatten()
                        .any(|decl| {
                            self.program
                                .file_of_node(decl)
                                .is_some_and(|file| file.source() == syntax.source)
                        });
                    if !candidate.this_member && (symbol.parent().is_some() || local) {
                        shadowed_names.insert(item.label.clone());
                    }
                    list.items.push(Some(Box::new(item)));
                }
            }
        }
        list.items.extend(method_snippets);
        let js = syntax.file.is_js();
        for item in keywords::keywords(filter, js && !in_comment) {
            let kind = tsr_scanner::string_to_token(item.label.as_bytes());
            if context.type_only && keywords::type_keyword(kind)
                || !context.type_only && keywords::contextual_expression(&item.label)
                || !names.contains(&item.label)
            {
                names.insert(item.label.clone());
                list.items.push(Some(Box::new(item)));
            }
        }
        if Self::assert_keyword_position(&syntax, context.token, position)?
            && names.insert("assert".into())
        {
            list.items.push(Some(Box::new(keywords::keyword("assert"))));
        }
        if context.member.is_none() && context.container.is_none() {
            self.auto_import_completions(
                checker,
                &mut syntax,
                &context,
                position,
                options,
                &mut list,
                None,
                &shadowed_names,
            )?;
        }
        list.items.extend(Self::literal_completions(
            checker,
            &mut syntax,
            &context,
            position,
            options,
        )?);
        names.extend(list.items.iter().flatten().map(|item| item.label.clone()));
        if !checked {
            Self::js_completion_entries(&mut syntax, position, &mut names, &mut list)?;
        }
        if let Some(item) =
            self.switch_case_completion(checker, &mut syntax, &context, position, options)?
        {
            list.items.push(Some(Box::new(item)));
        }
        let replacement = self.completion_replacement(&mut syntax, context.location)?;
        items::defaults(
            &mut list,
            options,
            &params.position,
            replacement,
            context.commit,
        );
        Ok(lsp::CompletionItemsOrListOrNull {
            list: Some(Box::new(list)),
            ..Default::default()
        })
    }

    // port: tsc/internal/ls/completions.go:LanguageService.getJSCompletionEntries
    fn js_completion_entries(
        syntax: &mut Syntax<'_>,
        position: i64,
        names: &mut HashSet<String>,
        list: &mut lsp::CompletionList,
    ) -> Result<()> {
        for (name, pos) in tsr_ast::source_file_tables::get_name_table(
            syntax.view,
            &mut syntax.docs,
            syntax.source,
        )? {
            if i64::from(pos) == position
                || !tsr_scanner::is_identifier_text(&name, tsr_core::LanguageVariant::STANDARD)
            {
                continue;
            }
            let name = String::from_utf8_lossy(&name).into_owned();
            if names.insert(name.clone()) {
                list.items.push(Some(Box::new(lsp::CompletionItem {
                    label: name,
                    kind: Some(Box::new(lsp::CompletionItemKind::TEXT)),
                    sort_text: Some(Box::new("18".into())),
                    commit_characters: Some(Box::default()),
                    ..Default::default()
                })));
            }
        }
        Ok(())
    }

    fn completion_trigger(
        syntax: &mut Syntax<'_>,
        context: &Context,
        position: i64,
        trigger: &str,
    ) -> Result<bool> {
        let Some(previous) = context.previous else {
            return Ok(matches!(trigger, "." | "@"));
        };
        let read = syntax.view.node(previous)?;
        Ok(match trigger {
            "." | "@" => true,
            "\"" | "'" | "`" => {
                ast::is_string_literal_like(&read) && position == syntax.start(previous)? + 1
            }
            "#" => {
                read.kind() == K::PrivateIdentifier
                    && tsr_ast::utilities::get_containing_class(syntax.view, previous)?.is_some()
            }
            "<" => {
                read.kind() == K::LessThanToken
                    && read.parent().is_none_or(|p| {
                        syntax.view.node(p).is_ok_and(|n| {
                            n.kind() != K::BinaryExpression
                                || n.data_source()
                                    .as_binary_expression()
                                    .and_then(|b| b.left())
                                    .is_none_or(|left| {
                                        syntax
                                            .view
                                            .node(left)
                                            .is_ok_and(|r| tsr_ast::node_is_missing(Some(&r)))
                                    })
                        })
                    })
            }
            "/" => {
                if ast::is_string_literal_like(&read) {
                    tsr_ast::utilities_modules::try_get_import_from_module_specifier(
                        syntax.view,
                        previous,
                    )?
                    .is_some()
                } else {
                    read.kind() == K::LessThanSlashToken
                        && read.parent().is_some_and(|p| {
                            syntax
                                .view
                                .node(p)
                                .is_ok_and(|r| r.kind() == K::JsxClosingElement)
                        })
                }
            }
            "*" => crate::jsdoc_template::snippet_range(syntax, position as usize).is_some(),
            " " => read.kind() == K::ImportKeyword && read.parent() == Some(syntax.source),
            _ => false,
        })
    }
    // port: tsc/internal/ls/completions.go:LanguageService.getOptionalReplacementSpan
    pub(crate) fn completion_replacement(
        &mut self,
        syntax: &mut Syntax<'_>,
        location: NodeId,
    ) -> Result<Option<lsp::Range>> {
        let read = syntax.view.node(location)?;
        if !matches!(
            read.kind().known(),
            Some(K::Identifier | K::PrivateIdentifier)
        ) {
            return Ok(None);
        }
        let (range, fidelity) = self.range(
            syntax.source,
            TextRange::new(syntax.start(location)?, i64::from(read.end())),
            FEATURE_COMPLETION,
        )?;
        Ok(fidelity.is_exact().then_some(range))
    }
    // port: tsc/internal/ls/completions.go:supplementalFileIndex
    pub(crate) fn completion_source_index(&self, source: NodeId) -> Result<Option<Box<i32>>> {
        let file = self.source(source)?;
        let canonical = if let Some(canonical) = file.canonical_source_file() {
            Some(canonical)
        } else {
            file.canonical_file_name()
                .map(|name| {
                    self.program
                        .source_file(name.as_bytes())
                        .map(tsr_compiler::ProgramFile::source)
                        .ok_or(tsr_arena::Error::InvalidGraph)
                })
                .transpose()?
        };
        let Some(canonical) = canonical else {
            return Ok(None);
        };
        let index = self
            .program
            .supplemental_sources(canonical)?
            .iter()
            .position(|&id| id == source)
            .ok_or(tsr_arena::Error::InvalidGraph)?;
        Ok(Some(Box::new(index as i32)))
    }

    // port: tsc/internal/ls/completions.go:sourceFileForSupplementalFileIndex
    pub fn completion_source(&self, data: &lsp::CompletionItemData) -> Result<NodeId> {
        let file = self
            .program
            .source_file(data.file_name.as_bytes())
            .ok_or_else(|| crate::Error::MissingFile(data.file_name.clone()))?;
        if let Some(&index) = data.supplemental_file_index.as_deref() {
            let sources = self.program.supplemental_sources(file.source())?;
            return usize::try_from(index)
                .ok()
                .and_then(|index| sources.get(index))
                .copied()
                .ok_or(crate::Error::MissingSupplementalFile(index));
        }
        Ok(file.source())
    }

    // port: tsc/internal/ls/completions.go:ensureItemData
    pub(crate) fn completion_data(
        &self,
        source: NodeId,
        position: i64,
        list: &mut lsp::CompletionList,
    ) -> Result<()> {
        let file_name =
            String::from_utf8_lossy(self.source(source)?.original_file_name()?.as_bytes())
                .into_owned();
        let supplemental_file_index = self.completion_source_index(source)?;
        for item in list.items.iter_mut().flatten() {
            item.data.get_or_insert_with(|| {
                Box::new(lsp::CompletionItemData {
                    file_name: file_name.clone(),
                    position: position as i32,
                    name: item.label.clone(),
                    supplemental_file_index: supplemental_file_index.clone(),
                    ..Default::default()
                })
            });
        }
        Ok(())
    }
    // port: tsc/internal/ls/completions.go:isStaticProperty
    fn property_completion_sort(
        &self,
        checker: &Operation<'_>,
        symbol: SymbolRef,
    ) -> Result<&'static str> {
        if let Some(declaration) = checker.symbol(symbol)?.value_declaration() {
            let view = self.view(declaration)?;
            let read = view.node(declaration)?;
            if read.modifier_flags(view)? & tsr_ast::modifier_flags::STATIC != 0 {
                if let Some(parent) = read.parent() {
                    if matches!(
                        view.node(parent)?.kind().known(),
                        Some(K::ClassDeclaration | K::ClassExpression)
                    ) {
                        return Ok("10");
                    }
                }
            }
        }
        Ok("11")
    }

    fn completion_symbols(
        &self,
        checker: &mut Operation<'_>,
        syntax: &mut Syntax<'_>,
        context: &mut Context,
        position: i64,
        options: &CompletionOptions,
    ) -> Result<Vec<Candidate>> {
        if let Some((access, expression)) = context.member {
            let mut candidates = Vec::new();
            let mut merged_value_type = None;
            if let Some(symbol) = checker.get_symbol_at_location(expression)? {
                let symbol = checker.skip_alias(symbol)?;
                if checker.symbol(symbol)?.flags() & (sf::MODULE | sf::ENUM) != 0 {
                    let namespace_name = syntax.view.node(access)?.kind() == K::ModuleDeclaration;
                    if namespace_name {
                        context.new_identifier = true;
                        context.commit = &[];
                    }
                    for symbol in checker.get_exports_of_module(symbol)? {
                        let name = checker.symbol(symbol)?.name_bytes().to_vec().clone();
                        let valid = if namespace_name {
                            // Dotted namespace declarations offer only namespace members
                            // declared elsewhere, not the declaration being completed.
                            // A ModuleDeclaration is not a property-access checker input.
                            let mut declared_elsewhere = false;
                            if checker.symbol(symbol)?.flags() & sf::NAMESPACE != 0 {
                                for declaration in
                                    checker.symbol_declarations(symbol)?.iter().flatten()
                                {
                                    if self.view(declaration)?.node(declaration)?.parent()
                                        != Some(access)
                                    {
                                        declared_elsewhere = true;
                                        break;
                                    }
                                }
                            }
                            declared_elsewhere
                        } else if context.type_only {
                            Self::completion_type_symbol(checker, symbol, &mut HashSet::new())?
                        } else {
                            checker.is_valid_property_access(access, &name)?
                        };
                        if valid {
                            candidates.push(Candidate {
                                symbol,
                                sort: self.property_completion_sort(checker, symbol)?,
                                nullable: false,
                                this_member: false,
                                promise: false,
                                symbol_member: false,
                            });
                        }
                    }
                    let mut merged_with_value = false;
                    if !context.type_only && !namespace_name {
                        for declaration in checker.symbol_declarations(symbol)?.iter().flatten() {
                            if !matches!(
                                checker.node(declaration)?.kind().known(),
                                Some(K::SourceFile | K::ModuleDeclaration | K::EnumDeclaration)
                            ) {
                                merged_with_value = true;
                                break;
                            }
                        }
                    }
                    if !merged_with_value {
                        return Ok(candidates);
                    }
                    // Pin getTypeScriptMemberSymbols adds the value type's
                    // properties after namespace exports, including inherited
                    // static members of a merged class.
                    merged_value_type =
                        Some(checker.get_type_of_symbol_at_location(symbol, Some(expression))?);
                }
            }
            // A type location takes the value type's properties only inside a
            // type query.
            if merged_value_type.is_none()
                && context.type_only
                && !in_type_query(syntax.view, expression)?
            {
                return Ok(candidates);
            }
            let ty = if let Some(ty) = merged_value_type {
                ty
            } else {
                checker.try_get_this_type_at_ex(expression, false, None)?;
                checker.get_type_at_location(expression)?
            };
            let mut ty = checker.get_non_optional_type(ty)?;
            let mut nullable = false;
            if context.type_only {
                // A type query in a type location reads the non-nullable type
                // without optional chaining.
                ty = checker.get_non_nullable_type(ty)?;
            } else if checker.is_nullable_type(ty)? {
                let question = context.token.is_some_and(|id| {
                    syntax
                        .view
                        .node(id)
                        .is_ok_and(|n| n.kind() == K::QuestionDotToken)
                });
                if question || options.automatic_optional_chain != Some(false) {
                    ty = checker.get_non_nullable_type(ty)?;
                    nullable = !question;
                }
            }
            if checker.get_string_index_type(ty)?.is_some() {
                context.new_identifier = true;
                context.commit = &[];
            }
            let mut seen_members = HashSet::new();
            let checked = !syntax.file.is_js()
                || ast::is_check_js_enabled_for_file(&syntax.file, self.program.options());
            // An unchecked JS file offers every member of a union's types.
            let properties =
                if !checked && checker.type_flags(ty)? & tsr_checker::type_flags::UNION != 0 {
                    let types = checker.constituents(ty)?;
                    checker.get_all_possible_properties_of_types(&types)?
                } else {
                    checker.get_apparent_properties(ty)?
                };
            for symbol in properties {
                if checker.is_valid_property_access_for_completions(access, ty, symbol)? {
                    if !checked {
                        candidates.push(Candidate {
                            symbol,
                            sort: self.property_completion_sort(checker, symbol)?,
                            nullable,
                            this_member: false,
                            promise: false,
                            symbol_member: false,
                        });
                        continue;
                    }
                    // A computed symbol property is offered through the first
                    // accessible name of its key, such as `N` for `[N.sym]`.
                    if let Some(first) = self.computed_member(checker, context.token, symbol)? {
                        if seen_members.insert(first) {
                            candidates.push(Candidate {
                                symbol: first,
                                sort: "15",
                                nullable,
                                this_member: false,
                                promise: false,
                                symbol_member: true,
                            });
                        }
                        continue;
                    }
                    candidates.push(Candidate {
                        symbol,
                        sort: self.property_completion_sort(checker, symbol)?,
                        nullable,
                        this_member: false,
                        promise: false,
                        symbol_member: false,
                    });
                }
            }
            if !context.type_only
                && syntax.view.node(expression)?.flags() & tsr_ast::node_flags::AWAIT_CONTEXT != 0
            {
                if let Some(promised) = checker.get_promised_type_of_promise(ty)? {
                    for symbol in checker.get_apparent_properties(promised)? {
                        if checker
                            .is_valid_property_access_for_completions(access, promised, symbol)?
                        {
                            candidates.push(Candidate {
                                symbol,
                                sort: self.property_completion_sort(checker, symbol)?,
                                nullable,
                                this_member: false,
                                promise: true,
                                symbol_member: false,
                            });
                        }
                    }
                }
            }
            return Ok(candidates);
        }
        if let Some(candidates) = Self::completion_type_argument_members(checker, syntax, context)?
        {
            return Ok(candidates);
        }
        if let Some(candidates) = self.completion_container(checker, syntax, context, position)? {
            return Ok(candidates);
        }
        if let Some(candidates) = Self::import_attribute_completions(checker, syntax, context)? {
            return Ok(candidates);
        }
        (context.new_identifier, context.commit) =
            crate::completion_context::commits(syntax, context.token, position)?;
        let adjusted = if context.previous == context.token {
            position
        } else {
            syntax.start(context.previous.expect("different token is present"))?
        };
        let mut scope = context.token;
        while let Some(id) = scope {
            let mut file = tsr_format::FormatFile {
                view: syntax.view,
                source: syntax.source,
                jsdoc: &mut syntax.docs,
            };
            if file.position_belongs_to_node(id, adjusted)? {
                break;
            }
            scope = syntax.view.node(id)?.parent();
        }
        let scope = scope.unwrap_or(syntax.source);
        let meaning =
            sf::TYPE | sf::NAMESPACE | sf::ALIAS | if context.type_only { 0 } else { sf::VALUE };
        let mut result = Vec::new();
        if crate::completion_jsx::open_tag(syntax, context)? {
            context.filter = Filter::None;
            for symbol in checker.get_jsx_intrinsic_tag_names_at(context.location)? {
                result.push(Candidate {
                    symbol,
                    sort: "11",
                    nullable: false,
                    this_member: false,
                    promise: false,
                    symbol_member: false,
                });
            }
        }
        for symbol in checker.get_symbols_in_scope(scope, meaning)? {
            // Pin shouldIncludeSymbol preserves the local declaration meaning
            // and the exported/aliased target meaning of a scope symbol.
            let origin = checker.skip_alias(symbol)?;
            let mut flags = checker.symbol(symbol)?.flags() | checker.symbol(origin)?.flags();
            if let Some(export) = checker.symbol(origin)?.export_symbol() {
                flags |= checker.symbol(checker.symbol_ref(export)?)?.flags();
            }
            if checker.symbol(symbol)?.flags() & sf::ALIAS != 0 {
                flags |= checker.get_symbol_flags(symbol)?;
            }
            if context.type_only {
                if !Self::completion_type_symbol(checker, symbol, &mut HashSet::new())? {
                    continue;
                }
            } else if flags & sf::VALUE == 0 {
                continue;
            }
            if !Self::completion_visible(checker, syntax, context, symbol)? {
                continue;
            }
            let mut local = checker.is_arguments_symbol(symbol)?;
            for decl in checker.symbol_declarations(symbol)?.iter().flatten() {
                if self
                    .program
                    .file_of_node(decl)
                    .is_some_and(|f| f.source() == syntax.source)
                {
                    local = true;
                    break;
                }
            }
            // A module file reaches a UMD global through an import, so the
            // auto-import suggestion replaces the global.
            if !local
                && origin != symbol
                && syntax.file.external_module_indicator.is_some()
                && !self.program.options().allow_umd_global_access.is_true()
            {
                if let Some(parent) = checker.symbol(symbol)?.parent() {
                    let parent = checker.symbol_ref(parent)?;
                    if checker.symbol(parent)?.is_external_module() {
                        continue;
                    }
                }
            }
            result.push(Candidate {
                symbol,
                sort: if local { "11" } else { "15" },
                nullable: false,
                this_member: false,
                promise: false,
                symbol_member: false,
            });
        }
        if scope != syntax.source {
            let ty = checker.try_get_this_type_at_ex(scope, false, None)?;
            if let Some(ty) = ty {
                for symbol in properties(checker, ty)? {
                    result.push(Candidate {
                        symbol,
                        sort: "14",
                        nullable: false,
                        this_member: true,
                        promise: false,
                        symbol_member: false,
                    });
                }
            }
        }
        Ok(result)
    }
    /// The first name accessible at `token` in a property's computed key, if
    /// the property has one and any name of the key is accessible.
    // Pin getCompletionData: addPropertySymbol closure.
    fn computed_member(
        &self,
        checker: &mut Operation<'_>,
        token: Option<tsr_ast::NodeId>,
        symbol: SymbolRef,
    ) -> Result<Option<SymbolRef>> {
        let mut computed = None;
        for declaration in checker.symbol_declarations(symbol)?.iter().flatten() {
            let name =
                tsr_ast::get_name_of_declaration(self.view(declaration)?, Some(declaration))?;
            if let Some(name) = name.filter(|&n| {
                checker
                    .node(n)
                    .is_ok_and(|r| r.kind() == K::ComputedPropertyName)
            }) {
                computed = Some(name);
                break;
            }
        }
        let Some(computed) = computed else {
            return Ok(None);
        };
        // port: tsc/internal/ls/completions.go:getLeftMostName
        let mut left = checker.node(computed)?.expression();
        while let Some(id) = left {
            match checker.node(id)?.kind().known() {
                Some(K::Identifier) => break,
                Some(K::PropertyAccessExpression) => left = checker.node(id)?.expression(),
                _ => left = None,
            }
        }
        let Some(left) = left else {
            return Ok(None);
        };
        let Some(name_symbol) = checker.get_symbol_at_location(left)? else {
            return Ok(None);
        };
        // port: tsc/internal/ls/completions.go:getFirstSymbolInChain
        let mut current = name_symbol;
        loop {
            let chain = checker.get_accessible_symbol_chain(current, token, sf::ALL, false)?;
            if let Some(&first) = chain.first() {
                return Ok(Some(first));
            }
            let Some(parent) = checker.symbol(current)?.parent() else {
                return Ok(None);
            };
            let parent = checker.symbol_ref(parent)?;
            let module = checker
                .symbol_declarations(parent)?
                .iter()
                .flatten()
                .any(|d| checker.node(d).is_ok_and(|r| r.kind() == K::SourceFile));
            if module {
                return Ok(Some(current));
            }
            current = parent;
        }
    }
    /// An import attributes clause may follow an import's or re-export's
    /// module specifier on the same line.
    // port: tsc/internal/ls/completions.go:getContextualKeywords
    fn assert_keyword_position(
        syntax: &Syntax<'_>,
        token: Option<tsr_ast::NodeId>,
        position: i64,
    ) -> Result<bool> {
        let Some(token) = token else {
            return Ok(false);
        };
        let read = syntax.view.node(token)?;
        let Some(parent) = read.parent() else {
            return Ok(false);
        };
        let pr = syntax.view.node(parent)?;
        Ok(matches!(
            pr.kind().known(),
            Some(K::ImportDeclaration | K::ExportDeclaration)
        ) && pr.module_specifier() == Some(token)
            && syntax.same_line(i64::from(read.end()), position))
    }
    // Pin getCompletionData: tryGetImportAttributesCompletionSymbols closure.
    fn import_attribute_completions(
        checker: &mut Operation<'_>,
        syntax: &Syntax<'_>,
        context: &mut Context,
    ) -> Result<Option<Vec<Candidate>>> {
        let Some(token) = context.token else {
            return Ok(None);
        };
        let view = syntax.view;
        let read = view.node(token)?;
        let attributes = match read.kind().known() {
            Some(K::OpenBraceToken | K::CommaToken) => read.parent(),
            Some(K::ColonToken) => read
                .parent()
                .and_then(|parent| view.node(parent).ok()?.parent()),
            _ => None,
        };
        let Some(attributes) =
            attributes.filter(|&id| view.node(id).is_ok_and(|n| n.kind() == K::ImportAttributes))
        else {
            return Ok(None);
        };
        let mut existing = HashSet::new();
        let list = view
            .node(attributes)?
            .data_source()
            .as_import_attributes()
            .and_then(|data| data.attributes());
        if let Some(list) = list {
            for element in view.node_slice(view.list(list)?.nodes())?.iter().flatten() {
                if let Some(name) = view.node(element)?.name() {
                    existing.insert(view.node_text(name)?.as_bytes().to_vec());
                }
            }
        }
        let ty = checker.get_type_at_location(attributes)?;
        let mut candidates = Vec::new();
        for symbol in checker.get_apparent_properties(ty)? {
            if !existing.contains(checker.symbol(symbol)?.name_bytes()) {
                candidates.push(Candidate {
                    symbol,
                    sort: "11",
                    nullable: false,
                    this_member: false,
                    promise: false,
                    symbol_member: false,
                });
            }
        }
        context.filter = Filter::None;
        context.new_identifier = false;
        Ok(Some(candidates))
    }
    // port: tsc/internal/ls/completions.go:symbolAppearsToBeTypeOnly
    fn appears_type_only(checker: &mut Operation<'_>, symbol: SymbolRef) -> Result<bool> {
        let target = checker.skip_alias(symbol)?;
        let mut flags = checker.symbol(target)?.flags();
        if let Some(export) = checker.symbol(target)?.export_symbol() {
            flags |= checker.symbol(checker.symbol_ref(export)?)?.flags();
        }
        if flags & sf::VALUE != 0 {
            return Ok(false);
        }
        let first = checker.symbol_declarations(symbol)?.iter().flatten().next();
        Ok(match first {
            Some(declaration) => {
                !ast::is_in_js_file(Some(&checker.node(declaration)?)) || flags & sf::TYPE != 0
            }
            None => true,
        })
    }
    fn completion_type_symbol(
        checker: &mut Operation<'_>,
        symbol: SymbolRef,
        seen: &mut HashSet<SymbolRef>,
    ) -> Result<bool> {
        let mut pending = vec![symbol];
        while let Some(symbol) = pending.pop() {
            if !seen.insert(symbol) {
                continue;
            }
            // Aliases may merge with declarations. Test the local symbol too,
            // rather than discarding its meanings when following the target.
            let export = checker.get_export_symbol_of_symbol(symbol)?;
            let target = checker.skip_alias(export)?;
            if target != symbol {
                pending.push(target);
            }
            let flags = checker.symbol(symbol)?.flags();
            if flags & sf::TYPE != 0 || checker.is_unknown_symbol(symbol)? {
                return Ok(true);
            }
            if flags & sf::MODULE != 0 {
                pending.extend(checker.get_exports_of_module(symbol)?.into_iter().rev());
            }
        }
        Ok(false)
    }
    fn completion_visible(
        checker: &Operation<'_>,
        syntax: &Syntax<'_>,
        context: &Context,
        symbol: SymbolRef,
    ) -> Result<bool> {
        let mut closest = None;
        for (start, parameters) in [(context.token, true), (Some(context.location), false)] {
            let mut node = start;
            while let Some(id) = node {
                let read = syntax.view.node(id)?;
                if ast::is_function_block(syntax.view, Some(id))?
                    || ast::is_binding_pattern(&read)
                    || read.parent().is_some_and(|p| {
                        syntax.view.node(p).is_ok_and(|r| {
                            r.kind() == K::ArrowFunction
                                && (r.body() == Some(id)
                                    || read.kind() == K::EqualsGreaterThanToken)
                        })
                    })
                {
                    break;
                }
                if parameters
                    && matches!(read.kind().known(), Some(K::Parameter | K::TypeParameter))
                    && !read.parent().is_some_and(|parent| {
                        syntax
                            .view
                            .node(parent)
                            .is_ok_and(|node| node.kind() == K::IndexSignature)
                    })
                    || !parameters && read.kind() == K::VariableDeclaration
                {
                    closest = Some(id);
                    break;
                }
                node = read.parent();
            }
            if closest.is_some() {
                break;
            }
        }
        while let Some(id) = closest {
            let read = syntax.view.node(id)?;
            if matches!(
                read.kind().known(),
                Some(K::VariableDeclaration | K::Parameter | K::TypeParameter)
            ) {
                let declaration = match checker.symbol(symbol)?.value_declaration() {
                    Some(id) => Some(id),
                    None => checker.symbol_declarations(symbol)?.iter().flatten().next(),
                };
                if let Some(decl) = declaration {
                    if read.kind() == K::VariableDeclaration && decl == id {
                        return Ok(false);
                    }
                    // Declarations in other files cannot be later parameters of this list.
                    if let Ok(dr) = syntax.view.node(decl) {
                        if read.kind() == K::Parameter && dr.kind() == K::Parameter {
                            if let Some(list) = read
                                .parent()
                                .map(|n| syntax.view.node(n))
                                .transpose()?
                                .and_then(|n| n.parameter_list())
                            {
                                if dr.pos() >= read.pos()
                                    && dr.pos() < syntax.view.list(list)?.loc().end() as i32
                                {
                                    return Ok(false);
                                }
                            }
                        } else if read.kind() == K::TypeParameter && dr.kind() == K::TypeParameter {
                            if id == decl
                                && context.token.is_some_and(|n| {
                                    syntax
                                        .view
                                        .node(n)
                                        .is_ok_and(|n| n.kind() == K::ExtendsKeyword)
                                })
                            {
                                return Ok(false);
                            }
                            if Self::in_type_parameter_default(syntax, context.token)? {
                                if let Some(parent) = read.parent() {
                                    let pr = syntax.view.node(parent)?;
                                    if pr.kind() != K::InferType {
                                        if let Some(list) = pr.type_parameter_list() {
                                            if dr.pos() >= read.pos()
                                                && dr.pos()
                                                    < syntax.view.list(list)?.loc().end() as i32
                                            {
                                                return Ok(false);
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                break;
            }
            closest = read.parent();
        }
        Ok(true)
    }
    // port: tsc/internal/ls/completions.go:isInTypeParameterDefault
    fn in_type_parameter_default(syntax: &Syntax<'_>, mut node: Option<NodeId>) -> Result<bool> {
        while let Some(id) = node {
            let read = syntax.view.node(id)?;
            let Some(parent) = read.parent() else {
                break;
            };
            let pr = syntax.view.node(parent)?;
            if let Some(data) = pr.data_source().as_type_parameter_declaration() {
                return Ok(data.default_type() == Some(id) || read.kind() == K::EqualsToken);
            }
            node = Some(parent);
        }
        Ok(false)
    }
    pub(crate) fn completion_symbol_item(
        &mut self,
        checker: &mut Operation<'_>,
        syntax: &mut Syntax<'_>,
        context: &Context,
        candidate: &Candidate,
        position: i64,
        options: &CompletionOptions,
    ) -> Result<Option<lsp::CompletionItem>> {
        let symbol = candidate.symbol;
        let mut name = symbol_name(checker, symbol)?;
        let class_member = context
            .container
            .is_some_and(|(kind, _)| kind == Container::Class);
        let computed_class_member = class_member
            && checker
                .symbol(symbol)?
                .value_declaration()
                .and_then(|id| checker.node(id).ok()?.name())
                .is_some_and(|name| {
                    checker
                        .node(name)
                        .is_ok_and(|n| n.kind() == K::ComputedPropertyName)
                });
        if computed_class_member {
            name =
                String::from_utf8_lossy(checker.symbol_to_string(symbol)?.as_bytes()).into_owned();
        }
        // A unique symbol's property has an internal `\xFE@` name.
        // port: tsc/internal/checker/utilities.go:IsKnownSymbol
        let known_symbol = checker.symbol(symbol)?.name_bytes().starts_with(b"\xFE@");
        if name.is_empty()
            || !computed_class_member && (name.starts_with("__@") || known_symbol)
            || checker.symbol(symbol)?.flags() & sf::MODULE != 0 && name.starts_with(['\'', '"'])
        {
            return Ok(None);
        }
        let private_identifier = if name.starts_with('#') {
            if let Some(declaration) = checker.symbol(symbol)?.value_declaration() {
                ast::is_private_identifier_class_element_declaration(
                    self.view(declaration)?,
                    declaration,
                )?
            } else {
                false
            }
        } else {
            false
        };
        let valid =
            tsr_scanner::is_identifier_text(name.as_bytes(), tsr_core::LanguageVariant::STANDARD)
                || private_identifier
                || computed_class_member;
        if !valid && class_member {
            return Ok(None);
        }
        if !valid && name.starts_with(' ') {
            return Ok(None);
        }
        if !valid
            && context.container.is_some_and(|(kind, _)| {
                matches!(
                    kind,
                    Container::Object | Container::TypeLiteral | Container::Class
                )
            })
        {
            name = quote(&name);
        }
        let mut insert = String::new();
        let mut snippet = false;
        let mut edit = None;
        if candidate.this_member {
            insert = if valid {
                format!("this.{name}")
            } else {
                format!(
                    "this[{}]",
                    quote_property_name(&name, syntax, options.quote)?
                )
            };
        } else if let Some((access, _)) = context.member {
            if !valid || candidate.nullable || candidate.symbol_member {
                insert = if candidate.symbol_member && valid {
                    format!("[{name}]")
                } else if valid {
                    name.clone()
                } else {
                    format!("[{}]", quote_property_name(&name, syntax, options.quote)?)
                };
                let token = context.token.expect("member context has a dot");
                if candidate.nullable || syntax.view.node(token)?.kind() == K::QuestionDotToken {
                    insert = format!("?.{insert}");
                }
                let ar = syntax.view.node(access)?;
                let suffix = ar.name().map(|id| syntax.view.node_text(id)).transpose()?;
                let end = if suffix
                    .as_ref()
                    .is_some_and(|s| name.as_bytes().starts_with(s.as_bytes()))
                {
                    ar.end()
                } else {
                    syntax.view.node(token)?.end()
                };
                let (range, fidelity) = self.range(
                    syntax.source,
                    TextRange::new(syntax.start(token)?, i64::from(end)),
                    FEATURE_COMPLETION,
                )?;
                if !fidelity.is_exact() {
                    return Ok(None);
                }
                edit = Some(Box::new(lsp::TextEditOrInsertReplaceEdit {
                    text_edit: Some(Box::new(lsp::TextEdit {
                        range,
                        new_text: insert.clone(),
                    })),
                    ..Default::default()
                }));
            }
        }
        if candidate.promise {
            if let Some((access, expression)) = context.member {
                let ar = syntax.view.node(access)?;
                let preceding = syntax.nav().find_preceding_token(i64::from(ar.pos()))?;
                let asi = if let Some(token) = preceding {
                    let tr = syntax.view.node(token)?;
                    if let Some(parent) = tr.parent() {
                        tsr_format::FormatFile {
                            view: syntax.view,
                            source: syntax.source,
                            jsdoc: &mut syntax.docs,
                        }
                        .position_is_asi_candidate(i64::from(tr.end()), parent)?
                    } else {
                        false
                    }
                } else {
                    false
                };
                let start = syntax.start(expression)? as usize;
                let expression_text = String::from_utf8_lossy(
                    &syntax.file.text().as_bytes()
                        [start..syntax.view.node(expression)?.end() as usize],
                );
                let prefix = if asi { ";" } else { "" };
                if insert.is_empty() {
                    insert.clone_from(&name);
                }
                let separator = if !valid {
                    ""
                } else if candidate.nullable {
                    "?."
                } else {
                    "."
                };
                // The pin prefixes the promise after nullable conversion; this
                // deliberately retains its doubled `?.` for nullable promises.
                insert = format!("{prefix}(await {expression_text}){separator}{insert}");
                let wrap = ar
                    .parent()
                    .filter(|n| {
                        syntax
                            .view
                            .node(*n)
                            .is_ok_and(|r| r.kind() == K::AwaitExpression)
                    })
                    .unwrap_or(expression);
                let (range, fidelity) = self.range(
                    syntax.source,
                    TextRange::new(syntax.start(wrap)?, i64::from(ar.end())),
                    FEATURE_COMPLETION,
                )?;
                if !fidelity.is_exact() {
                    return Ok(None);
                }
                edit = Some(Box::new(lsp::TextEditOrInsertReplaceEdit {
                    text_edit: Some(Box::new(lsp::TextEdit {
                        range,
                        new_text: insert.clone(),
                    })),
                    ..Default::default()
                }));
            }
        }
        if context
            .container
            .is_some_and(|(kind, _)| kind == Container::Jsx)
        {
            if let Some(text) =
                Self::jsx_attribute_insert(checker, syntax, context, symbol, &name, options)?
            {
                insert = text;
                snippet = true;
            }
        }
        if let Some(named) = ast::find_ancestor(syntax.view, Some(context.location), |node| {
            matches!(node.kind().known(), Some(K::NamedImports | K::NamedExports))
        })? {
            let imports = syntax.view.node(named)?.kind() == K::NamedImports;
            if !tsr_scanner::is_identifier_text(
                name.as_bytes(),
                tsr_core::LanguageVariant::STANDARD,
            ) {
                insert = quote_property_name(&name, syntax, options.quote)?;
                if imports && !followed_by_alias(syntax.file.text().as_bytes(), position) {
                    insert = format!("{insert} as {}", identifier_for_arbitrary_string(&name));
                }
            } else if imports {
                let token = tsr_scanner::string_to_token(name.as_bytes());
                if token == K::AwaitKeyword
                    || tsr_ast::utilities_tail::is_non_contextual_keyword(token.into())
                {
                    insert = format!("{name} as {name}_");
                }
            }
        }
        let kind = self.symbol_kind(checker, symbol, context.location)?;
        let modifiers = self.symbol_modifiers(checker, symbol)?;
        let bytes = syntax.file.text().as_bytes();
        let mut word = position as usize;
        while word > 0
            && (bytes[word - 1].is_ascii_alphanumeric()
                || matches!(bytes[word - 1], b'_' | b'$' | b'#')
                || bytes[word - 1] >= 128)
        {
            word -= 1;
        }
        // port: tsc/internal/ls/completions.go:getDotAccessor
        let dot = if bytes[..word].ends_with(b"?.") {
            "?."
        } else if bytes[..word].ends_with(b".") {
            "."
        } else {
            ""
        };
        let mut filter = items::filter_text(&insert, &name, bytes.get(word).copied(), dot);
        let mut label = name.clone();
        if (context.member.is_some()
            || context.container.is_some_and(|(kind, _)| {
                matches!(
                    kind,
                    Container::Object
                        | Container::TypeLiteral
                        | Container::Binding
                        | Container::Class
                        | Container::Jsx
                )
            }))
            && modifiers & modifiers::OPTIONAL != 0
        {
            if insert.is_empty() {
                insert.clone_from(&name);
            }
            if filter.is_empty() || snippet {
                filter.clone_from(&name);
            }
            label.push('?');
        }
        let deprecated = modifiers & modifiers::DEPRECATED != 0;
        let commit = (options.commit_characters
            && matches!(
                kind,
                crate::symbol_display::ScriptElementKind::Warning
                    | crate::symbol_display::ScriptElementKind::String
            ))
        .then(|| Box::new(Vec::new()));
        let mut type_only_alias = false;
        if checker.symbol(symbol)?.flags() & sf::VALUE == 0 {
            if let Some(previous) = context.previous {
                if !tsr_ast::utilities_modules::is_valid_type_only_alias_use_site(
                    syntax.view,
                    previous,
                )? {
                    for declaration in checker.symbol_declarations(symbol)?.iter().flatten() {
                        if let Some(file) = self.program.file_of_node(declaration) {
                            if tsr_ast::utilities_modules::is_type_only_import_declaration(
                                file.bound().view().ast(),
                                declaration,
                            )? {
                                type_only_alias = true;
                                break;
                            }
                        }
                    }
                }
            }
        }
        let sort = if options.object_method_snippets
            && context
                .container
                .is_some_and(|(kind, _)| kind == Container::Object)
        {
            format!("{}\0{name}\0", candidate.sort)
        } else {
            candidate.sort.into()
        };
        Ok(Some(lsp::CompletionItem {
            label,
            kind: Some(Box::new(items::kind(kind))),
            sort_text: Some(Box::new(format!(
                "{}{sort}",
                if deprecated { "z" } else { "" },
                sort = sort
            ))),
            filter_text: (!filter.is_empty()).then(|| Box::new(filter)),
            insert_text: (!insert.is_empty()).then(|| Box::new(insert)),
            insert_text_format: snippet.then(|| Box::new(lsp::InsertTextFormat::SNIPPET)),
            text_edit: edit,
            tags: deprecated.then(|| Box::new(vec![lsp::CompletionItemTag::DEPRECATED])),
            commit_characters: commit,
            data: Some(Box::new(lsp::CompletionItemData {
                supplemental_file_index: self.completion_source_index(syntax.source)?,
                file_name: String::from_utf8_lossy(syntax.file.original_file_name()?.as_bytes())
                    .into_owned(),
                position: position as i32,
                name,
                source: if candidate.this_member {
                    "ThisProperty/".into()
                } else if type_only_alias {
                    "TypeOnlyAlias/".into()
                } else {
                    String::new()
                },
                ..Default::default()
            })),
            ..Default::default()
        }))
    }
    // port: tsc/internal/ls/completions.go:LanguageService.ResolveCompletionItem
    pub fn resolve_completion(
        &mut self,
        checker: &mut Operation<'_>,
        mut item: lsp::CompletionItem,
        options: &CompletionOptions,
    ) -> Result<lsp::CompletionItem> {
        let data = item
            .data
            .as_deref()
            .ok_or(tsr_arena::Error::InvalidGraph)?
            .clone();
        let source = self.completion_source(&data)?;
        let mut syntax = Syntax::new(self.view(source)?, source)?;
        if data.is_import_statement_completion {
            return Ok(item);
        }
        if let Some(fix) = data.auto_import.as_deref() {
            return self.resolve_auto_import(item, fix, &mut syntax, options);
        }
        if data.source == "SwitchCases/" {
            return Ok(item);
        }
        let in_comment = syntax.in_comment(i64::from(data.position))?;
        if in_comment
            && matches!(
                self.jsdoc_completions(checker, &mut syntax, i64::from(data.position), options)?,
                crate::jsdoc_completions::JsDocCompletion::List(_)
            )
        {
            if item.detail.is_none() {
                item.detail = Some(Box::new(data.name));
            }
            return Ok(item);
        }
        let mut context = Context::collect(&mut syntax, i64::from(data.position))?;
        context.refine_type_arguments(&mut syntax, checker)?;
        if let Some(literal) = crate::string_completions::in_string(
            &mut syntax,
            context.previous,
            i64::from(data.position),
        )? {
            if let Some(symbol) = Self::string_completion_symbols(checker, &syntax, literal)?
                .into_iter()
                .find(|&s| symbol_name(checker, s).is_ok_and(|n| n == data.name))
            {
                return self.completion_details(checker, item, symbol, literal, options);
            }
            if item.detail.is_none() {
                item.detail = Some(Box::new(data.name));
            }
            return Ok(item);
        }
        if in_comment {
            context.type_only = true;
            context.filter = Filter::Type;
        }
        if Self::literal_completions(
            checker,
            &mut syntax,
            &context,
            i64::from(data.position),
            options,
        )?
        .iter()
        .flatten()
        .any(|i| i.label == data.name)
        {
            item.detail.get_or_insert_with(|| Box::new(data.name));
            return Ok(item);
        }
        for candidate in self.completion_symbols(
            checker,
            &mut syntax,
            &mut context,
            i64::from(data.position),
            options,
        )? {
            let mut name = symbol_name(checker, candidate.symbol)?;
            if context.container.is_some_and(|(kind, _)| {
                matches!(
                    kind,
                    Container::Object | Container::TypeLiteral | Container::Class
                )
            }) && !tsr_scanner::is_identifier_text(
                name.as_bytes(),
                tsr_core::LanguageVariant::STANDARD,
            ) {
                name = quote(&name);
            }
            if name != data.name {
                continue;
            }
            return self.completion_details(
                checker,
                item,
                candidate.symbol,
                context.location,
                options,
            );
        }
        if tsr_scanner::string_to_token(data.name.as_bytes()) != K::Unknown && item.detail.is_none()
        {
            item.detail = Some(Box::new(data.name));
        }
        Ok(item)
    }
    fn completion_details(
        &mut self,
        checker: &mut Operation<'_>,
        mut item: lsp::CompletionItem,
        symbol: SymbolRef,
        location: NodeId,
        options: &CompletionOptions,
    ) -> Result<lsp::CompletionItem> {
        let mut vc = VerbosityContext::default();
        let (display, declaration) = self.quick_info(
            checker,
            Some(symbol),
            location,
            &mut vc,
            false,
            tsr_ast::utilities_positions::semantic_meaning::ALL,
        )?;
        let text = String::from_utf8_lossy(display.text()).into_owned();
        if item.detail.is_none() && !text.is_empty() {
            item.detail = Some(Box::new(text));
        }
        let doc = self.documentation(
            checker,
            Some(symbol),
            location,
            declaration,
            options.markdown,
            false,
        )?;
        if !doc.is_empty() {
            item.documentation = Some(Box::new(lsp::StringOrMarkupContent {
                markup_content: Some(Box::new(lsp::MarkupContent {
                    kind: lsp::MarkupKind(
                        if options.markdown {
                            lsp::MarkupKind::MARKDOWN
                        } else {
                            lsp::MarkupKind::PLAIN_TEXT
                        }
                        .into(),
                    ),
                    value: doc,
                })),
                ..Default::default()
            }));
        }
        Ok(item)
    }
}

// port: tsc/internal/ls/completions.go:quotePropertyName
fn quote_property_name(
    name: &str,
    syntax: &Syntax<'_>,
    preference: crate::QuotePreference,
) -> Result<String> {
    if name
        .chars()
        .next()
        .is_some_and(tsr_jsstring::go_unicode::is_digit)
    {
        Ok(name.to_owned())
    } else {
        Ok(tsr_autoimport::edits::quote_module(
            name,
            crate::inlay_hints::single_quote(syntax, preference)?,
        ))
    }
}

#[cfg(test)]
#[path = "completion_item_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "completion_namespace_tests.rs"]
mod namespace_tests;

#[cfg(test)]
#[path = "completion_property_access_tests.rs"]
mod property_access_tests;

// port: tsc/internal/checker/utilities.go:IsInTypeQuery
fn in_type_query(view: tsr_ast::AstView<'_>, node: tsr_ast::NodeId) -> Result<bool> {
    let mut current = Some(node);
    while let Some(id) = current {
        let read = view.node(id)?;
        match read.kind().known() {
            Some(K::TypeQuery) => return Ok(true),
            Some(K::Identifier | K::QualifiedName) => current = read.parent(),
            _ => return Ok(false),
        }
    }
    Ok(false)
}

/// `{ ^here as name }`: the import already names its local binding.
fn followed_by_alias(text: &[u8], position: i64) -> bool {
    let skip = |at: usize| tsr_scanner::skip_trivia(text, at as i64) as usize;
    let at = skip(position.max(0) as usize);
    let word_end = |at: usize| {
        let mut end = at;
        while end < text.len()
            && (text[end].is_ascii_alphanumeric()
                || matches!(text[end], b'_' | b'$')
                || text[end] >= 128)
        {
            end += 1;
        }
        end
    };
    let end = word_end(at);
    if &text[at..end] != b"as" {
        return false;
    }
    let next = skip(end);
    word_end(next) > next && !text[next].is_ascii_digit()
}

// port: tsc/internal/ls/completions.go:generateIdentifierForArbitraryString
fn identifier_for_arbitrary_string(text: &str) -> String {
    let mut needs_underscore = false;
    let mut identifier = String::new();
    for (index, ch) in text.char_indices() {
        let valid = if index == 0 {
            tsr_scanner::is_identifier_start(ch as i32)
        } else {
            tsr_scanner::is_identifier_part(ch as i32)
        };
        if valid {
            if needs_underscore {
                identifier.push('_');
            }
            identifier.push(ch);
            needs_underscore = false;
        } else {
            needs_underscore = true;
        }
    }
    if needs_underscore {
        identifier.push('_');
    }
    if identifier.is_empty() {
        "_".into()
    } else {
        identifier
    }
}

#[cfg(test)]
#[path = "completion_residual_tests.rs"]
mod residual_tests;
