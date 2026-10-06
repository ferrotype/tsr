//! Missing-member syntax shared by inherited class completion suggestions.
use crate::{
    completion_context::{Container, Context},
    completion_snippets::{body, list, settings},
    syntax::Syntax,
    CompletionOptions, LanguageService, Result,
};
use tsr_ast::{
    modifier_flags as mf, symbol_flags as sf, Factory, FactoryMethods, NodeData, NodeId,
    SyntaxKind as K,
};
use tsr_checker::{
    BuilderRequest, GeneratedTypeNodes, Operation, SignatureKind, SignatureRef, SymbolRef,
};
use tsr_lsproto as lsp;
use tsr_nodebuilder::{flags as nf, internal_flags as inf};

#[derive(Default)]
struct Present {
    flags: u32,
    decorators: Vec<NodeId>,
    erase: Option<tsr_core::TextRange>,
}
// port: tsc/internal/ls/completions.go:LanguageService.getPresentMemberModifiers
fn present(syntax: &mut Syntax<'_>, context: &Context, position: i64) -> Result<Present> {
    let Some(token) = context.token else {
        return Ok(Present::default());
    };
    let tr = syntax.view.node(token)?;
    if syntax
        .file
        .text()
        .as_bytes()
        .get(tr.end() as usize..position as usize)
        .is_some_and(|s| s.contains(&b'\n') || s.contains(&b'\r'))
    {
        return Ok(Present::default());
    }
    let Some(parent) = tr.parent() else {
        return Ok(Present::default());
    };
    let pr = syntax.view.node(parent)?;
    if pr.kind() != K::PropertyDeclaration {
        return Ok(Present::default());
    }
    let kind = if tr.kind() == K::Identifier {
        tsr_scanner::string_to_token(syntax.view.node_text(token)?.as_bytes())
    } else {
        tr.kind().known().unwrap_or(K::Unknown)
    };
    if !kind.is_modifier_kind() {
        return Ok(Present::default());
    }
    let mut result = Present::default();
    let mut start = position;
    let mut end = position;
    for modifier in syntax
        .view
        .node_slice(
            pr.modifiers()
                .map(|id| syntax.view.list(id).map(|l| l.nodes()))
                .transpose()?
                .unwrap_or_default(),
        )?
        .iter()
        .flatten()
    {
        let mr = syntax.view.node(modifier)?;
        result.flags |= tsr_ast::modifier_to_flag(mr.kind());
        if mr.kind() == K::Decorator {
            result.decorators.push(modifier);
        }
        start = start.min(syntax.start(modifier)?);
    }
    let flag = tsr_ast::modifier_to_flag(kind.into());
    if result.flags & flag == 0 {
        result.flags |= flag;
        start = start.min(syntax.start(token)?);
    }
    if let Some(name) = pr.name().filter(|&n| n != token) {
        end = syntax.start(name)?;
    }
    if start < end {
        result.erase = Some(tsr_core::TextRange::new(start, end));
    }
    Ok(result)
}
fn modifiers(
    nodes: &mut GeneratedTypeNodes,
    flags: u32,
    decorators: &[NodeId],
) -> Result<Option<tsr_ast::NodeListId>> {
    let mut all: Vec<_> = decorators
        .iter()
        .map(|&id| nodes.clone_node(Some(id)))
        .collect();
    all.extend(
        tsr_ast::utilities_middle::create_modifiers_from_modifier_flags(flags, |kind| {
            Some(nodes.ast.new_modifier(kind))
        })
        .unwrap_or_default(),
    );
    if all.is_empty() {
        Ok(None)
    } else {
        Ok(Some(list(&mut nodes.ast, &all)?))
    }
}
struct Combined {
    names: Vec<tsr_ast::JsString>,
    count: usize,
    minimum: i32,
    rest: bool,
    result: tsr_checker::TypeRef,
}
fn combined(checker: &mut Operation<'_>, signatures: &[SignatureRef]) -> Result<Combined> {
    let mut best = signatures[0];
    let mut minimum = checker.signature_min_argument_count(best)?;
    let mut rest = false;
    let has_rest = |checker: &Operation<'_>, sig| -> Result<bool> {
        Ok(checker.signature_flags(sig)? & tsr_checker::signature_flags::HAS_REST_PARAMETER != 0)
    };
    let mut returns = Vec::new();
    for &signature in signatures {
        minimum = minimum.min(checker.signature_min_argument_count(signature)?);
        rest |= has_rest(checker, signature)?;
        if checker.signature_parameters(signature)?.len()
            >= checker.signature_parameters(best)?.len()
            && (!has_rest(checker, signature)? || has_rest(checker, best)?)
        {
            best = signature;
        }
        returns.push(checker.get_return_type_of_signature(signature)?);
    }
    let names: Vec<_> = checker
        .signature_parameters(best)?
        .into_iter()
        .map(|symbol| {
            Ok(tsr_ast::JsString::from_bytes(
                checker.symbol(symbol)?.name_bytes(),
            ))
        })
        .collect::<Result<_>>()?;
    let count = names.len() - usize::from(has_rest(checker, best)?);
    Ok(Combined {
        names,
        count,
        minimum,
        rest,
        result: checker.get_union_type(&returns)?,
    })
}
impl LanguageService<'_> {
    // port: tsc/internal/ls/completions.go:LanguageService.getEntryForMemberCompletion
    // port: tsc/internal/ls/codeactions_missingmemberfixer.go:missingMemberFixer.createMemberFromSymbol
    #[allow(
        clippy::too_many_arguments,
        reason = "Keep native member context, symbol, and insertion location explicit at the request boundary"
    )]
    pub(crate) fn class_member_snippet(
        &mut self,
        checker: &mut Operation<'_>,
        syntax: &mut Syntax<'_>,
        context: &Context,
        symbol: SymbolRef,
        position: i64,
        mut item: lsp::CompletionItem,
        options: &CompletionOptions,
    ) -> Result<Option<lsp::CompletionItem>> {
        let Some((Container::Class, class)) = context.container else {
            return Ok(Some(item));
        };
        if !options.class_member_snippets || syntax.file.is_js() {
            return Ok(Some(item));
        }
        let declarations: Vec<_> = checker
            .symbol_declarations(symbol)?
            .iter()
            .flatten()
            .collect();
        let declaration = declarations.first().copied();
        let present = present(syntax, context, position)?;
        let abstract_ = present.flags & mf::ABSTRACT != 0
            && tsr_ast::utilities_class::has_abstract_modifier(syntax.view, class)?;
        let signature_only =
            syntax.view.node(class)?.flags() & tsr_ast::node_flags::AMBIENT != 0 || abstract_;
        let effective = checker.get_declaration_modifier_flags_from_symbol(symbol)?;
        let mut flags = effective & mf::STATIC;
        if effective & mf::PUBLIC != 0 {
            flags |= mf::PUBLIC;
        } else if effective & mf::PROTECTED != 0 {
            flags |= mf::PROTECTED;
        }
        if declaration.is_some_and(|d| {
            self.view(d).is_ok_and(|v| {
                tsr_ast::utilities::is_auto_accessor_property_declaration(v, d).unwrap_or(false)
            })
        }) {
            flags |= mf::ACCESSOR;
        }
        if self.program.options().no_implicit_override.is_true() && effective & mf::ABSTRACT != 0 {
            flags |= mf::OVERRIDE;
        }
        if checker.member_override_status_for_flags(class, symbol, flags)?
            == tsr_checker::MemberOverrideStatus::NeedsOverride
        {
            flags |= mf::OVERRIDE;
        }
        if abstract_ {
            flags |= mf::ABSTRACT;
        }
        let allowed = flags
            | mf::OVERRIDE
            | mf::PUBLIC
            | if checker.symbol(symbol)?.flags() & sf::METHOD != 0 {
                mf::ASYNC
            } else {
                mf::AMBIENT | mf::READONLY
            };
        if present.flags & !allowed != 0 {
            return Ok(None);
        }
        if flags & mf::PROTECTED != 0 && present.flags & mf::PUBLIC != 0 {
            flags &= !mf::PROTECTED;
        }
        if present.flags != 0 && present.flags & mf::PUBLIC == 0 {
            flags &= !mf::PUBLIC;
        }
        flags |= present.flags;
        let mut adder = tsr_autoimport::ImportAdder::default();
        let Some((nodes, roots)) = self.member_nodes(
            checker,
            syntax,
            symbol,
            class,
            signature_only,
            flags,
            &present.decorators,
            options,
            false,
            None,
            &tsr_printer::EmitContext::default(),
            &mut adder,
        )?
        else {
            return Ok(Some(item));
        };
        let edits = self.import_adder_edits(syntax, options, &adder)?;
        if !edits.is_empty() {
            item.additional_text_edits = Some(Box::new(edits));
        }
        let name_text = tsr_ast::JsString::from_bytes(checker.symbol(symbol)?.name_bytes());
        let text = crate::snippet_printer::print_many(
            nodes.ast,
            &roots,
            &nodes.emit,
            &syntax.file,
            &settings(options, syntax)?,
        )?;
        if !text.is_empty() {
            item.insert_text = Some(Box::new(
                text.join(options.newline.as_deref().unwrap_or("\n")),
            ));
        }
        item.filter_text = Some(Box::new(
            String::from_utf8_lossy(name_text.as_bytes()).into_owned(),
        ));
        item.insert_text_format = options
            .snippets
            .then(|| Box::new(lsp::InsertTextFormat::SNIPPET));
        if let Some(erase) = present.erase {
            let (range, fidelity) =
                self.range(syntax.source, erase, tsr_ast::span_map::FEATURE_COMPLETION)?;
            if fidelity.is_exact() {
                item.additional_text_edits
                    .get_or_insert_with(Default::default)
                    .push(Some(Box::new(lsp::TextEdit {
                        range,
                        new_text: String::new(),
                    })));
                item.data.as_mut().unwrap().source = "ClassMemberSnippet/".into();
            }
        }
        Ok(Some(item))
    }
    #[allow(
        clippy::too_many_arguments,
        reason = "Member synthesis is shared by completions and fixes with explicit emission policies"
    )]
    pub(crate) fn member_nodes(
        &mut self,
        checker: &mut Operation<'_>,
        syntax: &mut Syntax<'_>,
        symbol: SymbolRef,
        class: NodeId,
        signature_only: bool,
        flags: u32,
        decorators: &[NodeId],
        options: &CompletionOptions,
        method_optional: bool,
        locale: Option<&tsr_locale::Locale>,
        emit: &tsr_printer::EmitContext,
        adder: &mut tsr_autoimport::ImportAdder,
    ) -> Result<Option<(GeneratedTypeNodes, Vec<NodeId>)>> {
        let single_quote = crate::inlay_hints::single_quote(syntax, options.quote)?;
        let declarations: Vec<_> = checker
            .symbol_declarations(symbol)?
            .iter()
            .flatten()
            .collect();
        let declaration = declarations.first().copied();
        let kind = declaration
            .map(|d| {
                self.view(d)
                    .and_then(|v| Ok(v.node(d)?.kind().known().unwrap_or(K::Unknown)))
            })
            .transpose()?
            .unwrap_or(K::PropertySignature);
        let ty = checker.get_type_of_symbol_at_location(symbol, Some(class))?;
        let ty = checker.get_widened_type(ty)?;
        let optional = checker.symbol(symbol)?.flags() & sf::OPTIONAL != 0;
        let name_text = tsr_ast::JsString::from_bytes(checker.symbol(symbol)?.name_bytes());
        let is_method = matches!(kind, K::MethodDeclaration | K::MethodSignature);
        let mut signatures = Vec::new();
        if is_method {
            let types = if checker.type_flags(ty)? & tsr_checker::type_flags::UNION != 0 {
                checker.constituents(ty)?
            } else {
                vec![ty]
            };
            for ty in types {
                signatures.extend(checker.get_signatures_of_type(ty, SignatureKind::Call)?);
            }
        }
        let mut planned = Vec::new();
        let mut combination = None;
        if is_method {
            if signatures.is_empty() {
                return Ok(None);
            }
            if declarations.len() == 1 {
                planned.push((signatures[0], !signature_only));
            } else {
                for &signature in &signatures {
                    let decl = checker
                        .signature_declaration(signature)?
                        .map(tsr_checker::NodeRef::id);
                    if !decl.is_some_and(|d| {
                        self.view(d).is_ok_and(|v| {
                            v.node(d)
                                .is_ok_and(|n| n.flags() & tsr_ast::node_flags::AMBIENT != 0)
                        })
                    }) {
                        planned.push((signature, false));
                    }
                }
                if !signature_only {
                    if declarations.len() > signatures.len() {
                        planned.push((
                            checker
                                .get_signature_from_declaration(*declarations.last().unwrap())?,
                            true,
                        ));
                    } else {
                        combination = Some(combined(checker, &signatures)?);
                    }
                }
            }
        }
        let builder_flags = nf::NO_TRUNCATION
            | if single_quote {
                nf::USE_SINGLE_QUOTES_FOR_STRING_LITERAL_TYPE
            } else {
                0
            };
        let mut builder = checker.node_builder_with_emit(emit);
        let mut roots = Vec::new();
        for (signature, with_body) in planned {
            if let Some(node) = builder.signature_to_signature_declaration(
                signature,
                K::MethodDeclaration,
                BuilderRequest {
                    enclosing: Some(class),
                    flags: builder_flags | nf::SUPPRESS_ANY_RETURN_TYPE | nf::ALLOW_EMPTY_TUPLE,
                    internal_flags: inf::ALLOW_UNRESOLVED_NAMES,
                },
            )? {
                roots.push((node, with_body));
            }
        }
        let type_node = if !is_method {
            builder.type_to_type_node(ty, Some(class), builder_flags, 0)?
        } else if let Some(combination) = &combination {
            builder.type_to_type_node(
                combination.result,
                Some(class),
                builder_flags,
                inf::ALLOW_UNRESOLVED_NAMES,
            )?
        } else {
            None
        };
        let mut nodes = builder.into_syntax();
        for &decl in declarations.iter().chain(decorators.iter()) {
            if let Some(file) = self.program.file_of_node(decl) {
                nodes.ast.retain_completed(file.bound());
            }
        }
        let mut original_name = declaration
            .map(|d| nodes.ast.view().node(d).map(|n| n.name()))
            .transpose()?
            .flatten();
        if checker.symbol(symbol)?.check_flags() & tsr_ast::check_flags::MAPPED != 0 {
            if let Some(name_type) = checker.get_name_type_of_symbol(symbol)? {
                if let Some(name) = checker.get_property_name_from_type(name_type)? {
                    original_name = Some(nodes.ast.new_identifier(name));
                }
            }
        }
        let mut result = Vec::new();
        for (root, with_body) in roots {
            // Code fixes rebuild the method shell while preserving reused
            // signature children and their same-source comment ranges, as
            // missingMemberFixer.createSignatureDeclarationFromSignature does.
            // The transferred builder owns these generated parameters; no
            // source or checker-cached node is mutated here.
            let root = tsr_ast::clone_node(&mut nodes.ast, root);
            let name = property_name(
                &mut nodes,
                original_name,
                &name_text,
                single_quote,
                locale.is_some(),
            );
            let body = with_body
                .then(|| member_body(&mut nodes, options, single_quote, locale))
                .transpose()?;
            let question = optional.then(|| nodes.ast.new_token(K::QuestionToken.into()));
            if syntax.file.is_js() {
                let parameters: Vec<_> = nodes
                    .ast
                    .view()
                    .node_slice(nodes.ast.view().node(root)?.parameters(nodes.ast.view())?)?
                    .iter()
                    .flatten()
                    .collect();
                for parameter in parameters {
                    if let NodeData::ParameterDeclaration(p) =
                        nodes.ast.node_mut(parameter)?.data_mut()
                    {
                        p.question_token = None;
                    }
                }
            }
            let mut read = nodes.ast.node_mut(root)?;
            let NodeData::MethodDeclaration(data) = read.data_mut() else {
                unreachable!()
            };
            data.name = Some(name);
            data.body = body;
            if syntax.file.is_js() {
                data.type_parameters = None;
                data.r#type = None;
            }
            data.postfix_token = method_optional.then_some(question).flatten();
            result.push(root);
        }
        if !is_method {
            let kinds = if matches!(kind, K::GetAccessor | K::SetAccessor) {
                let accessors = tsr_ast::utilities_class::get_all_accessor_declarations(
                    nodes.ast.view(),
                    &declarations,
                    declaration.expect("accessor declaration"),
                )?;
                accessors
                    .first_accessor
                    .into_iter()
                    .chain(accessors.second_accessor)
                    .collect()
            } else {
                vec![declaration.unwrap_or(class)]
            };
            for declaration in kinds {
                let kind = nodes.ast.view().node(declaration)?.kind();
                let name = property_name(
                    &mut nodes,
                    original_name,
                    &name_text,
                    single_quote,
                    locale.is_some(),
                );
                let ty = nodes.clone_node(type_node);
                let node = if kind == K::GetAccessor {
                    let b = (!signature_only)
                        .then(|| member_body(&mut nodes, options, single_quote, locale))
                        .transpose()?;
                    nodes.ast.new_get_accessor_declaration(
                        None,
                        Some(name),
                        None,
                        None,
                        ty,
                        None,
                        b,
                    )
                } else if kind == K::SetAccessor {
                    let parameter = nodes
                        .ast
                        .view()
                        .node_slice(
                            nodes
                                .ast
                                .view()
                                .node(declaration)?
                                .parameters(nodes.ast.view())?,
                        )?
                        .iter()
                        .flatten()
                        .next()
                        .expect("set accessor parameter");
                    let parameter_name = nodes.ast.view().node(parameter)?.name();
                    let parameter_name = nodes.clone_node(parameter_name);
                    let p = nodes.ast.new_parameter_declaration(
                        None,
                        None,
                        parameter_name,
                        None,
                        if syntax.file.is_js() { None } else { ty },
                        None,
                    );
                    let ps = list(&mut nodes.ast, &[Some(p)])?;
                    let b = (!signature_only)
                        .then(|| member_body(&mut nodes, options, single_quote, locale))
                        .transpose()?;
                    nodes.ast.new_set_accessor_declaration(
                        None,
                        Some(name),
                        None,
                        Some(ps),
                        None,
                        None,
                        b,
                    )
                } else {
                    let question = optional.then(|| nodes.ast.new_token(K::QuestionToken.into()));
                    nodes
                        .ast
                        .new_property_declaration(None, Some(name), question, ty, None)
                };
                result.push(node);
            }
        }
        if let Some(combination) = combination {
            let name = property_name(
                &mut nodes,
                original_name,
                &name_text,
                single_quote,
                locale.is_some(),
            );
            let mut parameters = Vec::new();
            let mut name_counts = std::collections::HashMap::<Vec<u8>, usize>::new();
            for n in 0..combination.count + usize::from(combination.rest) {
                let name = combination
                    .names
                    .get(n)
                    .filter(|s| !s.is_empty())
                    .cloned()
                    .unwrap_or_else(|| {
                        tsr_ast::JsString::from_bytes(if n == combination.count {
                            b"rest".to_vec()
                        } else {
                            format!("arg{n}").into_bytes()
                        })
                    });
                let mut text = name.as_bytes().to_vec();
                if n < combination.count {
                    let count = name_counts.entry(text.clone()).or_default();
                    if *count > 0 {
                        text.extend_from_slice(count.to_string().as_bytes());
                    }
                    *count += 1;
                }
                let name = nodes
                    .ast
                    .new_identifier(tsr_ast::JsString::from_bytes(text));
                let question = (n as i32 >= combination.minimum)
                    .then(|| nodes.ast.new_token(K::QuestionToken.into()));
                let mut ty = nodes.ast.new_keyword_type_node(K::UnknownKeyword.into());
                let rest = if n == combination.count {
                    ty = nodes.ast.new_array_type_node(Some(ty));
                    Some(nodes.ast.new_token(K::DotDotDotToken.into()))
                } else {
                    None
                };
                parameters.push(Some(nodes.ast.new_parameter_declaration(
                    None,
                    rest,
                    Some(name),
                    question,
                    if syntax.file.is_js() && n < combination.count {
                        None
                    } else {
                        Some(ty)
                    },
                    None,
                )));
            }
            let ps = list(&mut nodes.ast, &parameters)?;
            let ty = nodes.clone_node(type_node);
            let b = member_body(&mut nodes, options, single_quote, locale)?;
            let question =
                (optional && method_optional).then(|| nodes.ast.new_token(K::QuestionToken.into()));
            result.push(nodes.ast.new_method_declaration(
                None,
                None,
                Some(name),
                question,
                None,
                Some(ps),
                ty,
                None,
                Some(b),
            ));
        }
        self.import_generated_types(checker, syntax, &mut nodes, &mut result, options, adder)?;
        let count = result.len();
        let mut roots = Vec::new();
        for (index, node) in result.into_iter().enumerate() {
            let mods = modifiers(
                &mut nodes,
                flags,
                if index + 1 == count { decorators } else { &[] },
            )?;
            let node = tsr_ast::utilities_class::replace_modifiers(&mut nodes.ast, node, mods);
            roots.push(node);
        }
        Ok(Some((nodes, roots)))
    }
}
fn property_name(
    nodes: &mut GeneratedTypeNodes,
    original: Option<NodeId>,
    name: &tsr_ast::JsString,
    single_quote: bool,
    constructor: bool,
) -> NodeId {
    if constructor && name.as_bytes() == b"constructor" {
        let flags = if single_quote {
            tsr_ast::token_flags::SINGLE_QUOTE
        } else {
            0
        };
        let literal = nodes.ast.new_string_literal(name.clone(), flags);
        return nodes.ast.new_computed_property_name(Some(literal));
    }
    nodes.clone_node(original).unwrap_or_else(|| {
        if tsr_scanner::is_identifier_text(name.as_bytes(), tsr_core::LanguageVariant::STANDARD) {
            nodes.ast.new_identifier(name.clone())
        } else {
            nodes.ast.new_string_literal(name.clone(), 0)
        }
    })
}

fn member_body(
    nodes: &mut GeneratedTypeNodes,
    options: &CompletionOptions,
    single_quote: bool,
    locale: Option<&tsr_locale::Locale>,
) -> Result<NodeId> {
    let Some(locale) = locale else {
        return body(&mut nodes.ast, &mut nodes.emit, options.snippets);
    };
    let text = tsr_ast::JsString::from_bytes(
        tsr_diagnostics::Method_not_implemented.localize(locale, &[]),
    );
    let flags = if single_quote {
        tsr_ast::token_flags::SINGLE_QUOTE
    } else {
        0
    };
    let literal = nodes.ast.new_string_literal(text, flags);
    let args = list(&mut nodes.ast, &[Some(literal)])?;
    let error = nodes
        .ast
        .new_identifier(tsr_ast::JsString::from_bytes(b"Error".as_slice()));
    let new = nodes.ast.new_new_expression(Some(error), None, Some(args));
    let throw = nodes.ast.new_throw_statement(Some(new));
    let statements = list(&mut nodes.ast, &[Some(throw)])?;
    Ok(nodes.ast.new_block(Some(statements), true))
}
