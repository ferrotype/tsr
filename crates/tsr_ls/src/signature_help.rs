use crate::{
    display_parts::DisplayParts,
    signature_arguments::{ArgumentInfo, Invocation},
    syntax::Syntax,
    LanguageService, Result,
};
use tsr_ast::{span_map::FEATURE_SIGNATURE_HELP, utilities as ast, NodeId, SyntaxKind as K};
use tsr_checker::{BuilderRequest, Operation, SignatureKind, SignatureRef, SymbolRef, TypeRef};
use tsr_lsproto as lsp;
use tsr_printer::{EmitTextWriter, Printer, PrinterOptions};

pub const SIGNATURE_HELP_TRIGGER_CHARACTERS: &[&str] = &["(", ",", "<"];
pub const SIGNATURE_HELP_RETRIGGER_CHARACTERS: &[&str] = &[")"];
const BUILDER_FLAGS: u32 = tsr_nodebuilder::flags::OMIT_PARAMETER_MODIFIERS
    | tsr_nodebuilder::flags::IGNORE_ERRORS
    | tsr_nodebuilder::flags::USE_ALIAS_DEFINED_OUTSIDE_CURRENT_SCOPE;
#[derive(Clone, Debug)]
pub struct SignatureHelpOptions {
    pub format: lsp::MarkupKind,
    pub classified: bool,
    pub per_signature_active_parameter: bool,
    pub null_active_parameter: bool,
}
impl Default for SignatureHelpOptions {
    fn default() -> Self {
        Self {
            format: lsp::MarkupKind(lsp::MarkupKind::PLAIN_TEXT.into()),
            classified: false,
            per_signature_active_parameter: false,
            null_active_parameter: false,
        }
    }
}
#[derive(Clone)]
struct Parameter {
    info: lsp::ParameterInformation,
    rest: bool,
}
struct Item {
    info: lsp::SignatureInformation,
    parameters: Vec<Parameter>,
    variadic: bool,
}
impl Item {
    // port: tsc/internal/ls/signaturehelp.go:LanguageService.computeActiveParameter
    fn active(&self, index: usize, null: bool) -> Option<Box<lsp::UintegerOrNull>> {
        if self.parameters.is_empty() {
            return None;
        }
        let mut index = index;
        if self.variadic {
            if self
                .parameters
                .iter()
                .position(|p| p.rest)
                .is_some_and(|first| first + 1 < self.parameters.len())
            {
                return Some(Box::new(lsp::UintegerOrNull {
                    uinteger: (!null).then(|| Box::new(self.parameters.len() as u32)),
                }));
            }
            index = index.min(self.parameters.len() - 1);
        }
        Some(Box::new(lsp::UintegerOrNull {
            uinteger: Some(Box::new(index as u32)),
        }))
    }
}
fn markup(text: String, kind: &lsp::MarkupKind) -> Option<Box<lsp::StringOrMarkupContent>> {
    (!text.is_empty()).then(|| {
        Box::new(lsp::StringOrMarkupContent {
            markup_content: Some(Box::new(lsp::MarkupContent {
                kind: kind.clone(),
                value: text,
            })),
            ..Default::default()
        })
    })
}
#[derive(Clone, Copy)]
enum DisplayNode {
    Type(TypeRef),
    Parameter(SymbolRef),
    TypeParameter(TypeRef),
}
fn display(
    c: &mut Operation<'_>,
    node: DisplayNode,
    source: NodeId,
    enclosing: NodeId,
    classified: bool,
) -> Result<DisplayParts> {
    let mut builder = c.node_builder();
    builder.retain_source_node(source)?;
    let request = BuilderRequest {
        enclosing: Some(enclosing),
        flags: BUILDER_FLAGS,
        internal_flags: 0,
    };
    let built = match node {
        DisplayNode::Type(ty) => {
            builder.type_to_type_node(ty, Some(enclosing), BUILDER_FLAGS, 0)?
        }
        DisplayNode::Parameter(symbol) => {
            builder.symbol_to_parameter_declaration(symbol, request)?
        }
        DisplayNode::TypeParameter(ty) => builder.type_parameter_to_declaration(ty, request)?,
    };
    let mut out = DisplayParts::new(classified);
    if let Some(built) = built {
        if matches!(node, DisplayNode::TypeParameter(_)) {
            // The pin gives this builder a fresh emit context, separate from
            // the printer's. In particular its single-line flags do not apply.
            let emit = tsr_printer::EmitContext::new();
            let mut printer = Printer::new(PrinterOptions::default(), &emit);
            out.write(&printer.emit(builder.view(), built, Some(source))?);
        } else {
            let printer = Printer::new(PrinterOptions::default(), builder.emit_context());
            printer.write(builder.view(), built, Some(source), &mut out, None)?;
        }
    } else if let DisplayNode::Type(ty) = node {
        drop(builder);
        out.write(c.type_to_string_default(ty)?.as_bytes());
    }
    Ok(out)
}
impl LanguageService<'_> {
    // port: tsc/internal/ls/signaturehelp.go:LanguageService.ProvideSignatureHelp
    pub fn signature_help(
        &mut self,
        c: &mut Operation<'_>,
        params: &lsp::SignatureHelpParams,
        options: &SignatureHelpOptions,
    ) -> Result<lsp::SignatureHelpOrNull> {
        let file = self.file(&params.text_document.uri)?;
        let positions = self.converters.from_lsp_position_for_source_file(
            self.program,
            file,
            &params.position,
            FEATURE_SIGNATURE_HELP,
        )?;
        for projection in positions {
            self.check_canceled()?;
            if !projection.mapped.fidelity.is_single_segment() {
                continue;
            }
            let source = projection.script;
            let mut syntax = Syntax::new(self.view(source)?, source)?;
            let position = i64::from(projection.mapped.position);
            let Some(starting) = syntax.nav().find_preceding_token(position)? else {
                continue;
            };
            let (typed, manual) = match params.context.as_deref() {
                None => (false, false),
                Some(ctx) => match ctx.trigger_kind {
                    lsp::SignatureHelpTriggerKind::TRIGGER_CHARACTER
                        if ctx.trigger_character.is_some() =>
                    {
                        (!ctx.is_retrigger, false)
                    }
                    lsp::SignatureHelpTriggerKind::CONTENT_CHANGE => (!ctx.is_retrigger, false),
                    _ => (false, true),
                },
            };
            if typed && (syntax.in_string(position, starting)? || syntax.in_comment(position)?) {
                continue;
            }
            let Some(info) = syntax.containing_argument(starting, position, manual, c)? else {
                continue;
            };
            self.check_canceled()?;
            let mut candidates = Vec::new();
            let mut resolved = None;
            let mut type_symbol = None;
            match info.invocation {
                Invocation::Call(node) => {
                    if !typed || syntax.syntactic_owner(starting, node)? {
                        (resolved, candidates) =
                            c.get_resolved_signature_for_signature_help(node, info.count)?;
                    }
                }
                Invocation::TypeArguments(called) => {
                    let read = syntax.view.node(called)?;
                    let container = if read.kind() == K::Identifier {
                        read.parent().unwrap_or(called)
                    } else {
                        called
                    };
                    if !typed || syntax.contains_preceding(starting, container)? {
                        let mut ty = c.get_type_at_location(called)?;
                        let mut kind = SignatureKind::Call;
                        if let Some(parent) = read.parent() {
                            let pr = syntax.view.node(parent)?;
                            if ast::is_optional_chain(&pr) {
                                ty = if ast::is_optional_chain_root(&pr) {
                                    c.get_non_nullable_type(ty)?
                                } else {
                                    c.get_non_optional_type(ty)?
                                };
                            }
                            if pr.kind() == K::NewExpression {
                                kind = SignatureKind::Construct;
                            }
                        }
                        for signature in c.get_signatures_of_type(ty, kind)? {
                            let tps = c.signature_type_parameters(signature)?;
                            if !tps.is_empty() && tps.len() >= info.count {
                                candidates.push(signature);
                            }
                        }
                        resolved = candidates.first().copied();
                        if candidates.is_empty() {
                            type_symbol = c.get_symbol_at_location(called)?;
                        }
                    }
                }
                Invocation::Contextual { signature, .. } => {
                    candidates.push(signature);
                    resolved = Some(signature);
                }
            }
            self.check_canceled()?;
            let result = if !candidates.is_empty() {
                self.signature_items(c, &candidates, resolved, &info, source, typed, options)?
            } else if let Some(symbol) = type_symbol {
                Self::type_help(c, symbol, &info, source, options)?
            } else if syntax.file.is_js() {
                self.js_signature_help(c, &info, &syntax, options)?
            } else {
                None
            };
            if let Some(result) = result {
                return Ok(lsp::SignatureHelpOrNull {
                    signature_help: Some(Box::new(result)),
                });
            }
        }
        Ok(lsp::SignatureHelpOrNull::default())
    }
    // port: tsc/internal/ls/signaturehelp.go:createTypeHelpItems
    fn type_help(
        c: &mut Operation<'_>,
        symbol: SymbolRef,
        info: &ArgumentInfo,
        source: NodeId,
        options: &SignatureHelpOptions,
    ) -> Result<Option<lsp::SignatureHelp>> {
        let parameters = c.get_local_type_parameters_of_class_or_interface_or_type_alias(symbol)?;
        if parameters.is_empty() {
            return Ok(None);
        }
        let mut label =
            String::from_utf8_lossy(c.symbol_to_string(symbol)?.as_bytes()).into_owned();
        let mut params = Vec::new();
        label.push('<');
        for (i, ty) in parameters.into_iter().enumerate() {
            let text = display(
                c,
                DisplayNode::TypeParameter(ty),
                source,
                info.invocation.enclosing(),
                false,
            )?
            .string();
            if i > 0 {
                label.push_str(", ");
            }
            label.push_str(&text);
            params.push(Some(Box::new(lsp::ParameterInformation {
                label: lsp::StringOrTuple {
                    string: Some(Box::new(text)),
                    ..Default::default()
                },
                documentation: None,
            })));
        }
        label.push('>');
        let active = Some(Box::new(lsp::UintegerOrNull {
            uinteger: Some(Box::new(info.index as u32)),
        }));
        Ok(Some(lsp::SignatureHelp {
            signatures: vec![Some(Box::new(lsp::SignatureInformation {
                label,
                parameters: Some(Box::new(params)),
                active_parameter: if options.per_signature_active_parameter {
                    active.clone()
                } else {
                    None
                },
                ..Default::default()
            }))],
            active_signature: Some(Box::new(0)),
            active_parameter: if options.per_signature_active_parameter {
                None
            } else {
                active
            },
        }))
    }
    // port: tsc/internal/ls/signaturehelp.go:LanguageService.createJSSignatureHelpItems
    fn js_signature_help(
        &mut self,
        c: &mut Operation<'_>,
        info: &ArgumentInfo,
        syntax: &Syntax<'_>,
        options: &SignatureHelpOptions,
    ) -> Result<Option<lsp::SignatureHelp>> {
        let Some(expression) = info.invocation.expression(syntax)? else {
            return Ok(None);
        };
        let expression = syntax.view.node(expression)?;
        if expression.kind() != K::PropertyAccessExpression {
            return Ok(None);
        }
        let Some(name) = expression.name() else {
            return Ok(None);
        };
        let name = syntax.view.node_text(name)?;
        if name.is_empty() {
            return Ok(None);
        }
        for file in self.program.files() {
            let view = file.bound().view().ast();
            let mut stack = vec![file.source()];
            while let Some(node) = stack.pop() {
                self.check_canceled()?;
                if tsr_ast::source_file_tables::get_declaration_name(view, node)?.as_slice()
                    == name.as_bytes()
                {
                    if let Some(symbol) = self.bound_symbol(c, node)? {
                        let ty = c.get_type_of_symbol_at_location(symbol, Some(node))?;
                        let signatures = c.get_signatures_of_type(ty, SignatureKind::Call)?;
                        if !signatures.is_empty() {
                            let result = self.signature_items(
                                c,
                                &signatures,
                                signatures.first().copied(),
                                info,
                                file.source(),
                                true,
                                options,
                            )?;
                            if result.is_some() {
                                return Ok(result);
                            }
                        }
                    }
                }
                let mut children = Syntax::new(view, file.source())?.children(node)?;
                children.reverse();
                stack.extend(children);
            }
        }
        Ok(None)
    }
    // port: tsc/internal/ls/signaturehelp.go:LanguageService.createSignatureHelpItems
    #[allow(
        clippy::too_many_arguments,
        reason = "Preserves the pinned signature renderer inputs and separate client capabilities"
    )]
    fn signature_items(
        &mut self,
        c: &mut Operation<'_>,
        candidates: &[SignatureRef],
        resolved: Option<SignatureRef>,
        info: &ArgumentInfo,
        source: NodeId,
        full_prefix: bool,
        options: &SignatureHelpOptions,
    ) -> Result<Option<lsp::SignatureHelp>> {
        let enclosing = info.invocation.enclosing();
        let mut symbol = if let Invocation::Contextual { symbol, .. } = info.invocation {
            symbol
        } else {
            // JS fallback may render a signature found in another source file.
            // Invocation nodes still belong to the request's source arena.
            let invocation_source = self
                .program
                .file_of_node(enclosing)
                .ok_or(tsr_arena::Error::InvalidGraph)?
                .source();
            let syntax = Syntax::new(self.view(enclosing)?, invocation_source)?;
            match info.invocation.expression(&syntax)? {
                Some(expr) => c.get_symbol_at_location(expr)?,
                None => None,
            }
        };
        if symbol.is_none() && full_prefix {
            if let Some(resolved) = resolved {
                if let Some(declaration) = c.signature_declaration(resolved)? {
                    symbol = c.bound_symbol_of_node(declaration.id())?;
                }
            }
        }
        let mut prefix = DisplayParts::new(options.classified);
        if let Some(symbol) = symbol {
            if !c.symbol(symbol)?.name_bytes().starts_with(b"\xfe") {
                let text = if full_prefix {
                    c.symbol_to_string_at(
                        symbol,
                        Some(source),
                        0,
                        tsr_checker::symbol_format_flags::USE_ALIAS_DEFINED_OUTSIDE_CURRENT_SCOPE,
                    )?
                } else {
                    c.symbol_to_string(symbol)?
                };
                prefix.write_symbol(text.as_bytes(), Some(symbol.id()));
            }
        }
        let mut items = Vec::new();
        let mut selected = 0;
        for &candidate in candidates {
            let next = self.signature_item(
                c,
                candidate,
                info.type_arguments,
                &prefix,
                source,
                enclosing,
                options,
            )?;
            if Some(candidate) == resolved {
                selected = items.len();
                if next.len() > 1 {
                    if let Some(index) = next
                        .iter()
                        .position(|item| item.variadic || item.parameters.len() >= info.count)
                    {
                        selected += index;
                    }
                }
            }
            items.extend(next);
        }
        if items.is_empty() {
            return Ok(None);
        }
        let active = if options.per_signature_active_parameter {
            None
        } else {
            items[selected].active(info.index, options.null_active_parameter)
        };
        let signatures = items
            .into_iter()
            .map(|mut item| {
                if options.per_signature_active_parameter {
                    item.info.active_parameter =
                        item.active(info.index, options.null_active_parameter);
                }
                item.info.parameters = Some(Box::new(
                    item.parameters
                        .into_iter()
                        .map(|p| Some(Box::new(p.info)))
                        .collect(),
                ));
                Some(Box::new(item.info))
            })
            .collect();
        Ok(Some(lsp::SignatureHelp {
            signatures,
            active_signature: Some(Box::new(selected as u32)),
            active_parameter: active,
        }))
    }
    // port: tsc/internal/ls/signaturehelp.go:LanguageService.getSignatureHelpItem
    #[allow(
        clippy::too_many_arguments,
        reason = "Preserves the pinned signature renderer inputs and separate client capabilities"
    )]
    fn signature_item(
        &mut self,
        c: &mut Operation<'_>,
        signature: SignatureRef,
        type_arguments: bool,
        prefix: &DisplayParts,
        source: NodeId,
        enclosing: NodeId,
        options: &SignatureHelpOptions,
    ) -> Result<Vec<Item>> {
        let parameter_signature = if type_arguments {
            c.signature_target(signature)?.unwrap_or(signature)
        } else {
            signature
        };
        let type_parameters = c.signature_type_parameters(parameter_signature)?;
        let mut parameters = Vec::new();
        for ty in type_parameters {
            let text =
                display(c, DisplayNode::TypeParameter(ty), source, enclosing, false)?.string();
            parameters.push(Parameter {
                info: lsp::ParameterInformation {
                    label: lsp::StringOrTuple {
                        string: Some(Box::new(text)),
                        ..Default::default()
                    },
                    documentation: None,
                },
                rest: false,
            });
        }
        let mut head = prefix.clone();
        if type_arguments || !parameters.is_empty() {
            head.write_punctuation(b"<");
            for (index, param) in parameters.iter().enumerate() {
                if index > 0 {
                    head.write_punctuation(b", ");
                }
                head.add(
                    "type parameter name",
                    param.info.label.string.as_deref().unwrap().as_bytes(),
                    None,
                );
            }
            head.write_punctuation(b">");
        }
        let lists = c.get_expanded_parameters(signature, false)?;
        if !lists.is_empty() {
            head.write_punctuation(b"(");
        }
        let has_rest = c.has_effective_rest_parameter(signature)?;
        let list_count = lists.len();
        let mut infos = Vec::new();
        for list in lists {
            let mut label = head.clone();
            let mut params = Vec::new();
            let variadic = !type_arguments
                && has_rest
                && (list_count == 1
                    || list.last().is_some_and(|s| {
                        c.symbol(*s).is_ok_and(|s| {
                            s.check_flags() & tsr_ast::check_flags::REST_PARAMETER != 0
                        })
                    }));
            for (index, symbol) in list.into_iter().enumerate() {
                if index > 0 {
                    label.write_punctuation(b", ");
                }
                let text = display(
                    c,
                    DisplayNode::Parameter(symbol),
                    source,
                    enclosing,
                    options.classified,
                )?;
                let param_label = text.string();
                label.append(text);
                let rest =
                    c.symbol(symbol)?.check_flags() & tsr_ast::check_flags::REST_PARAMETER != 0;
                let docs = if let Some(decl) = c.symbol(symbol)?.value_declaration() {
                    self.signature_documentation(c, decl, options)?
                } else {
                    None
                };
                params.push(Parameter {
                    info: lsp::ParameterInformation {
                        label: lsp::StringOrTuple {
                            string: Some(Box::new(param_label)),
                            ..Default::default()
                        },
                        documentation: docs,
                    },
                    rest,
                });
            }
            label.write_punctuation(b")");
            infos.push((
                label,
                if type_arguments {
                    parameters.clone()
                } else {
                    params
                },
                variadic,
            ));
        }
        let mut suffix = DisplayParts::new(options.classified);
        suffix.write_punctuation(b": ");
        if let Some(predicate) = c.get_type_predicate_of_signature(signature)? {
            suffix.write(c.type_predicate_to_string(predicate)?.as_bytes());
        } else {
            let ty = c.get_return_type_of_signature(signature)?;
            suffix.append(display(
                c,
                DisplayNode::Type(ty),
                source,
                enclosing,
                options.classified,
            )?);
        }
        let docs = if let Some(decl) = c.signature_declaration(signature)? {
            self.signature_documentation(c, decl.id(), options)?
        } else {
            None
        };
        infos
            .into_iter()
            .map(|(mut label, parameters, variadic)| {
                label.append(suffix.clone());
                let runs = label.runs(c)?;
                Ok(Item {
                    info: lsp::SignatureInformation {
                        label: label.string(),
                        documentation: docs.clone(),
                        vs_colorized_label: (!runs.is_empty()).then(|| {
                            Box::new(lsp::VSClassifiedTextElement {
                                runs: runs.into_iter().map(|r| Some(Box::new(r))).collect(),
                                ..Default::default()
                            })
                        }),
                        ..Default::default()
                    },
                    parameters,
                    variadic,
                })
            })
            .collect()
    }
    fn signature_documentation(
        &mut self,
        c: &mut Operation<'_>,
        node: NodeId,
        options: &SignatureHelpOptions,
    ) -> Result<Option<Box<lsp::StringOrMarkupContent>>> {
        let text = self.declaration_documentation_for_feature(
            c,
            node,
            options.format.0 == lsp::MarkupKind::MARKDOWN,
            true,
            FEATURE_SIGNATURE_HELP,
        )?;
        Ok(markup(text, &options.format))
    }
}

#[cfg(test)]
#[path = "signature_help_tests.rs"]
mod tests;
