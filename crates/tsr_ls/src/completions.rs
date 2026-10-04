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
    pub locale: tsr_locale::Locale,
    pub automatic_optional_chain: Option<bool>,
    pub jsx_attribute_style: Option<String>,
    pub import_module_specifier_ending: Option<String>,
}

pub(crate) struct Candidate {
    pub symbol: SymbolRef,
    pub sort: &'static str,
    pub nullable: bool,
    pub this_member: bool,
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
    let name = checker.symbol(symbol)?.name_bytes().to_vec();
    Ok(String::from_utf8_lossy(&name).into_owned())
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
        let mut syntax = Syntax::new(self.view(source)?, source)?;
        if options.module_exports != Some(false) && self.auto_imports.is_prepared() {
            self.prepare_auto_imports(checker, &syntax)?;
        }
        let mut context = Context::collect(&mut syntax, position)?;
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
        if syntax.in_comment(position)? {
            match self.jsdoc_completions(checker, &mut syntax, position, options)? {
                crate::jsdoc_completions::JsDocCompletion::Code => {}
                crate::jsdoc_completions::JsDocCompletion::Prose => {
                    return Ok(lsp::CompletionItemsOrListOrNull::default())
                }
                crate::jsdoc_completions::JsDocCompletion::List(list) => {
                    return Ok(lsp::CompletionItemsOrListOrNull {
                        list: Some(Box::new(list)),
                        ..Default::default()
                    })
                }
            }
            context.type_only = true;
            context.filter = Filter::Type;
        }
        if let Some(mut list) =
            self.closing_tag_completion(&mut syntax, &context, &params.position, options)?
        {
            self.completion_data(source, position, &mut list)?;
            return Ok(lsp::CompletionItemsOrListOrNull {
                list: Some(Box::new(list)),
                ..Default::default()
            });
        }
        if context.blocked(&syntax, position)? {
            return Ok(lsp::CompletionItemsOrListOrNull::default());
        }
        if let Some(mut list) =
            self.label_completions(&mut syntax, &context, &params.position, options)?
        {
            self.completion_data(source, position, &mut list)?;
            return Ok(lsp::CompletionItemsOrListOrNull {
                list: (!list.items.is_empty()).then(|| Box::new(list)),
                ..Default::default()
            });
        }
        let candidates =
            self.completion_symbols(checker, &mut syntax, &mut context, position, options)?;
        if candidates.is_empty()
            && !context.new_identifier
            && context.filter == Filter::None
            && context.member.is_none()
        {
            return Ok(lsp::CompletionItemsOrListOrNull::default());
        }
        let mut list = lsp::CompletionList::default();
        let mut names = HashSet::new();
        for candidate in candidates {
            self.check_canceled()?;
            if let Some(item) = self.completion_symbol_item(
                checker,
                &mut syntax,
                &context,
                &candidate,
                position,
                options,
            )? {
                if names.insert(item.label.clone()) {
                    list.items.push(Some(Box::new(item)));
                }
            }
        }
        let js = syntax.file.is_js();
        let filter = if context.member.is_some() {
            Filter::None
        } else {
            context.filter
        };
        for item in keywords::keywords(filter, js) {
            let kind = tsr_scanner::string_to_token(item.label.as_bytes());
            if context.type_only && keywords::type_keyword(kind)
                || !context.type_only && keywords::contextual_expression(&item.label)
                || !names.contains(&item.label)
            {
                names.insert(item.label.clone());
                list.items.push(Some(Box::new(item)));
            }
        }
        if context.member.is_none() && context.container.is_none() {
            self.auto_import_completions(
                checker,
                &mut syntax,
                &context,
                position,
                options,
                &mut list,
            )?;
        }
        list.items.extend(Self::literal_completions(
            checker,
            &mut syntax,
            &context,
            position,
            options,
        )?);
        let replacement = self.completion_replacement(&mut syntax, context.location)?;
        items::defaults(
            &mut list,
            options,
            &params.position,
            replacement,
            context.commit,
        );
        self.completion_data(source, position, &mut list)?;
        Ok(lsp::CompletionItemsOrListOrNull {
            list: Some(Box::new(list)),
            ..Default::default()
        })
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
        for item in list.items.iter_mut().flatten() {
            item.data.get_or_insert_with(|| {
                Box::new(lsp::CompletionItemData {
                    file_name: file_name.clone(),
                    position: position as i32,
                    name: item.label.clone(),
                    ..Default::default()
                })
            });
        }
        Ok(())
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
            if let Some(symbol) = checker.get_symbol_at_location(expression)? {
                let symbol = checker.skip_alias(symbol)?;
                if checker.symbol(symbol)?.flags() & (sf::MODULE | sf::ENUM) != 0 {
                    let mut candidates = Vec::new();
                    for symbol in checker.get_exports_of_module(symbol)? {
                        let name = checker.symbol(symbol)?.name_bytes().to_vec().clone();
                        if context.type_only
                            && Self::completion_type_symbol(checker, symbol, &mut HashSet::new())?
                            || !context.type_only
                                && checker.is_valid_property_access(access, &name)?
                        {
                            candidates.push(Candidate {
                                symbol,
                                sort: "11",
                                nullable: false,
                                this_member: false,
                            });
                        }
                    }
                    return Ok(candidates);
                }
            }
            checker.try_get_this_type_at_ex(expression, false, None)?;
            let ty = checker.get_type_at_location(expression)?;
            let mut ty = checker.get_non_optional_type(ty)?;
            let mut nullable = false;
            if checker.is_nullable_type(ty)? {
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
            let mut candidates = Vec::new();
            for symbol in checker.get_apparent_properties(ty)? {
                if checker.is_valid_property_access_for_completions(access, ty, symbol)? {
                    candidates.push(Candidate {
                        symbol,
                        sort: "11",
                        nullable,
                        this_member: false,
                    });
                }
            }
            return Ok(candidates);
        }
        if let Some(candidates) = self.completion_container(checker, syntax, context, position)? {
            return Ok(candidates);
        }
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
                });
            }
        }
        for symbol in checker.get_symbols_in_scope(scope, meaning)? {
            let flags = checker.get_symbol_flags(symbol)?;
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
            result.push(Candidate {
                symbol,
                sort: if local { "11" } else { "15" },
                nullable: false,
                this_member: false,
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
                    });
                }
            }
        }
        Ok(result)
    }
    fn completion_type_symbol(
        checker: &mut Operation<'_>,
        symbol: SymbolRef,
        seen: &mut HashSet<SymbolRef>,
    ) -> Result<bool> {
        let mut pending = vec![symbol];
        while let Some(symbol) = pending.pop() {
            let symbol = checker.skip_alias(symbol)?;
            if !seen.insert(symbol) {
                continue;
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
                if let Some(decl) = checker.symbol(symbol)?.value_declaration() {
                    if read.kind() == K::VariableDeclaration && decl == id {
                        return Ok(false);
                    }
                    if read.kind() == K::Parameter
                        && syntax.view.node(decl)?.kind() == K::Parameter
                        && syntax.view.node(decl)?.parent() == read.parent()
                        && syntax.view.node(decl)?.pos() >= read.pos()
                    {
                        return Ok(false);
                    }
                }
                break;
            }
            closest = read.parent();
        }
        Ok(true)
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
        let name = symbol_name(checker, symbol)?;
        if name.is_empty()
            || name.starts_with("__@")
            || checker.symbol(symbol)?.flags() & sf::MODULE != 0 && name.starts_with(['\'', '"'])
        {
            return Ok(None);
        }
        let valid =
            tsr_scanner::is_identifier_text(name.as_bytes(), tsr_core::LanguageVariant::STANDARD)
                || name.starts_with('#');
        if !valid && name.starts_with(' ') {
            return Ok(None);
        }
        let mut insert = String::new();
        let mut snippet = false;
        let mut edit = None;
        if candidate.this_member {
            insert = if valid {
                format!("this.{name}")
            } else {
                format!("this[{}]", quote(&name))
            };
        } else if let Some((access, _)) = context.member {
            if !valid || candidate.nullable {
                insert = if valid {
                    name.clone()
                } else {
                    format!("[{}]", quote(&name))
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
        let dot = if word > 0 && bytes[word - 1] == b'.' {
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
                    Container::Object | Container::Binding | Container::Class | Container::Jsx
                )
            }))
            && modifiers & modifiers::OPTIONAL != 0
        {
            if insert.is_empty() {
                insert.clone_from(&name);
            }
            if filter.is_empty() {
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
        Ok(Some(lsp::CompletionItem {
            label,
            kind: Some(Box::new(items::kind(kind))),
            sort_text: Some(Box::new(format!(
                "{}{sort}",
                if deprecated { "z" } else { "" },
                sort = candidate.sort
            ))),
            filter_text: (!filter.is_empty()).then(|| Box::new(filter)),
            insert_text: (!insert.is_empty()).then(|| Box::new(insert)),
            insert_text_format: snippet.then(|| Box::new(lsp::InsertTextFormat::SNIPPET)),
            text_edit: edit,
            tags: deprecated.then(|| Box::new(vec![lsp::CompletionItemTag::DEPRECATED])),
            commit_characters: commit,
            data: Some(Box::new(lsp::CompletionItemData {
                file_name: String::from_utf8_lossy(syntax.file.original_file_name()?.as_bytes())
                    .into_owned(),
                position: position as i32,
                name,
                source: if candidate.this_member {
                    "ThisProperty/".into()
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
        let file = self
            .program
            .source_file(data.file_name.as_bytes())
            .ok_or_else(|| crate::Error::MissingFile(data.file_name.clone()))?;
        let mut syntax = Syntax::new(file.bound().view().ast(), file.source())?;
        if let Some(fix) = data.auto_import.as_deref() {
            return self.resolve_auto_import(item, fix, &mut syntax, options);
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
            if symbol_name(checker, candidate.symbol)? != data.name {
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
