use crate::{
    definition::object_literal_element,
    documentation, meaning,
    symbol_display::{modifiers, ScriptElementKind},
    syntax::Syntax,
    LanguageService, Result,
};
use tsr_ast::{
    span_map::FEATURE_HOVER, symbol_flags as sf, utilities as ast, NodeId, SyntaxKind as K,
};
use tsr_checker::{Operation, SymbolRef, VerbosityContext};
use tsr_lsproto as lsp;

#[derive(Clone, Copy, Debug)]
pub struct HoverOptions {
    pub markdown: bool,
    pub classified: bool,
    pub verbosity_signals: bool,
    pub maximum_length: usize,
}
impl Default for HoverOptions {
    fn default() -> Self {
        Self {
            markdown: false,
            classified: false,
            verbosity_signals: false,
            maximum_length: 500,
        }
    }
}
pub(crate) fn is_import_meta(view: tsr_ast::AstView<'_>, node: NodeId) -> Result<bool> {
    let read = view.node(node)?;
    Ok(read
        .data_source()
        .as_meta_property()
        .is_some_and(|d| d.keyword_token() == K::ImportKeyword)
        && read
            .name()
            .map(|n| view.node_text(n))
            .transpose()?
            .is_some_and(|t| t.as_bytes() == b"meta"))
}
// port: tsc/internal/ls/hover.go:getNodeForQuickInfo
fn quick_info_node(view: tsr_ast::AstView<'_>, node: NodeId) -> Result<NodeId> {
    let read = view.node(node)?;
    let Some(parent) = read.parent() else {
        return Ok(node);
    };
    let pr = view.node(parent)?;
    if pr.kind() == K::NewExpression && read.pos() == pr.pos() {
        return Ok(pr.expression().unwrap_or(node));
    }
    if pr.kind() == K::NamedTupleMember && read.pos() == pr.pos()
        || is_import_meta(view, parent)? && pr.name() == Some(node)
        || pr.kind() == K::JsxNamespacedName
    {
        return Ok(parent);
    }
    Ok(node)
}
impl LanguageService<'_> {
    // port: tsc/internal/ls/hover.go:getSymbolAtLocationForQuickInfo
    fn quick_info_symbol(
        &self,
        checker: &mut Operation<'_>,
        node: NodeId,
    ) -> Result<Option<SymbolRef>> {
        let view = self.view(node)?;
        if let Some(element) = object_literal_element(view, node)? {
            if let Some(parent) = view.node(element)?.parent() {
                if let Some(ty) =
                    checker.get_contextual_type(parent, tsr_checker::context_flags::NONE)?
                {
                    let properties =
                        checker.get_property_symbols_from_contextual_type(element, ty, false)?;
                    if properties.len() == 1 {
                        return Ok(properties.first().copied());
                    }
                }
            }
        }
        Ok(checker.get_symbol_at_location(node)?)
    }
    // port: tsc/internal/ls/hover.go:LanguageService.ProvideHover
    pub fn hover(
        &mut self,
        checker: &mut Operation<'_>,
        params: &lsp::HoverParams,
        options: HoverOptions,
    ) -> Result<lsp::HoverOrNull> {
        let file = self.file(&params.text_document.uri)?;
        let projections = self.converters.from_lsp_position_for_source_file(
            self.program,
            file,
            &params.position,
            FEATURE_HOVER,
        )?;
        let mut hovers = Vec::new();
        for projection in projections {
            self.check_canceled()?;
            if !projection.mapped.fidelity.is_single_segment() {
                continue;
            }
            let view = self.view(projection.script)?;
            let mut syntax = Syntax::new(view, projection.script)?;
            let position = i64::from(projection.mapped.position);
            let node = syntax.nav().get_touching_property_name(position)?;
            if view.node(node)?.kind() == K::SourceFile
                || matches!(
                    view.node(node)?.kind().known(),
                    Some(K::PropertyAccessExpression | K::QualifiedName)
                ) && !syntax.in_comment(position)?
            {
                continue;
            }
            let node = quick_info_node(view, node)?;
            let symbol = self.quick_info_symbol(checker, node)?;
            let mut vc = VerbosityContext {
                level: params.verbosity_level.as_deref().copied().unwrap_or(0),
                max_truncation_length: if options.maximum_length == 0 {
                    500
                } else {
                    options.maximum_length
                },
                ..VerbosityContext::default()
            };
            let meaning = meaning::meaning(view, node, checker)?;
            let (info, decl) =
                self.quick_info(checker, symbol, node, &mut vc, options.classified, meaning)?;
            let text = info.string();
            if text.is_empty() {
                continue;
            }
            let docs = self.documentation(checker, symbol, node, decl, options.markdown, false)?;
            let mut value = String::new();
            if options.markdown {
                documentation::code(&mut value, "typescript", &text);
            } else {
                value.push_str(&text);
            }
            value.push_str(&docs);
            let source = ast::get_source_file_of_node(view, Some(node))?
                .ok_or(tsr_arena::Error::InvalidGraph)?;
            let (range, fidelity) =
                self.range(source, syntax.reference_range(node, None)?, FEATURE_HOVER)?;
            let mut hover = lsp::Hover {
                contents: lsp::MarkupContentOrStringOrMarkedStringWithLanguageOrMarkedStrings {
                    markup_content: Some(Box::new(lsp::MarkupContent {
                        kind: lsp::MarkupKind(
                            if options.markdown {
                                lsp::MarkupKind::MARKDOWN
                            } else {
                                lsp::MarkupKind::PLAIN_TEXT
                            }
                            .into(),
                        ),
                        value,
                    })),
                    ..Default::default()
                },
                range: fidelity.is_single_segment().then(|| Box::new(range)),
                can_increase_verbosity: options.verbosity_signals
                    && vc.can_increase_verbosity
                    && !vc.truncated,
                ..Default::default()
            };
            if options.classified {
                let runs = info.runs(checker)?;
                if !runs.is_empty() {
                    let (kind, modifiers) = if let Some(symbol) = symbol {
                        let icon = if checker.symbol(symbol)?.flags() & sf::ALIAS != 0 {
                            checker.get_aliased_symbol(symbol)?
                        } else {
                            symbol
                        };
                        (
                            self.symbol_kind(checker, icon, node)?,
                            self.symbol_modifiers(checker, symbol)?,
                        )
                    } else {
                        (ScriptElementKind::Keyword, 0)
                    };
                    let docs = self.documentation(checker, symbol, node, decl, false, true)?;
                    hover.vs_raw_content = Some(Box::new(raw_content(
                        image_id(kind, modifiers),
                        runs,
                        docs.trim_start_matches('\n'),
                    )));
                }
            }
            hovers.push(hover);
        }
        let Some(mut combined) = hovers.first().cloned() else {
            return Ok(lsp::HoverOrNull::default());
        };
        if hovers.len() > 1 {
            let mut contents = Vec::new();
            let mut raw = Vec::new();
            for hover in hovers {
                let text = hover
                    .contents
                    .markup_content
                    .unwrap()
                    .value
                    .trim_end_matches('\n')
                    .to_owned();
                if !contents.contains(&text) {
                    contents.push(text);
                    if let Some(container) = hover.vs_raw_content {
                        raw.push(
                            lsp::VSImageElementOrClassifiedTextElementOrContainerElement {
                                container_element: Some(container),
                                ..Default::default()
                            },
                        );
                    }
                }
                combined.can_increase_verbosity |= hover.can_increase_verbosity;
                if combined.range != hover.range {
                    combined.range = None;
                }
            }
            combined.contents.markup_content.as_mut().unwrap().value =
                contents.join(if options.markdown {
                    "\n\n---\n\n"
                } else {
                    "\n\n"
                });
            combined.vs_raw_content = match raw.len() {
                0 => None,
                1 => raw.pop().unwrap().container_element,
                _ => Some(Box::new(lsp::VSContainerElement {
                    style: lsp::VSContainerElementStyle::STACKED,
                    elements: raw,
                    ..Default::default()
                })),
            };
        }
        Ok(lsp::HoverOrNull {
            hover: Some(Box::new(combined)),
        })
    }
}
// port: tsc/internal/ls/hovericon.go:getVSHoverImageId
fn image_id(kind: ScriptElementKind, flags: u32) -> lsp::VSImageId {
    use ScriptElementKind as E;
    let pick = |private, protected, public| {
        if flags & modifiers::PRIVATE != 0 {
            private
        } else if flags & modifiers::PROTECTED != 0 {
            protected
        } else {
            public
        }
    };
    let id = match kind {
        E::Warning => 0x637,
        E::Keyword => 0x635,
        E::ScriptElement => pick(0x77d, 0x77e, 0x77f),
        E::PrimitiveType | E::TypeParameterElement => 0xca1,
        E::ModuleElement => 0x79f,
        E::ConstructorImplementationElement
        | E::ClassElement
        | E::LocalClassElement
        | E::TypeElement => pick(0x1d7, 0x1d8, 0x1d9),
        E::InterfaceElement => pick(0x646, 0x647, 0x648),
        E::EnumElement => pick(0x469, 0x46a, 0x46b),
        E::EnumMemberElement => 0x465,
        E::ParameterElement
        | E::VariableElement
        | E::LocalVariableElement
        | E::VariableUsingElement
        | E::VariableAwaitUsingElement
        | E::LetElement
        | E::String => 0x6d3,
        E::ConstElement => pick(0x26a, 0x26b, 0x26c),
        E::MemberGetAccessorElement
        | E::MemberSetAccessorElement
        | E::MemberVariableElement
        | E::MemberAccessorVariableElement => pick(0x982, 0x983, 0x984),
        E::FunctionElement
        | E::LocalFunctionElement
        | E::MemberFunctionElement
        | E::CallSignatureElement
        | E::IndexSignatureElement
        | E::ConstructSignatureElement => pick(0x756, 0x757, 0x758),
        E::Label => 0x67d,
        E::Alias => 0x77f,
        _ => 0xc4,
    };
    lsp::VSImageId {
        guid: "ae27a6b0-e345-4288-96df-5eaf394ee369".into(),
        id,
        ..Default::default()
    }
}
// port: tsc/internal/ls/hovericon.go:buildVSHoverRawContent
fn raw_content(
    image: lsp::VSImageId,
    runs: Vec<lsp::VSClassifiedTextRun>,
    docs: &str,
) -> lsp::VSContainerElement {
    let line = lsp::VSContainerElement {
        style: lsp::VSContainerElementStyle::WRAPPED,
        elements: vec![
            lsp::VSImageElementOrClassifiedTextElementOrContainerElement {
                image_element: Some(Box::new(lsp::VSImageElement {
                    image_id: Some(Box::new(image)),
                    ..Default::default()
                })),
                ..Default::default()
            },
            lsp::VSImageElementOrClassifiedTextElementOrContainerElement {
                classified_text_element: Some(Box::new(lsp::VSClassifiedTextElement {
                    runs: runs.into_iter().map(|r| Some(Box::new(r))).collect(),
                    ..Default::default()
                })),
                ..Default::default()
            },
        ],
        ..Default::default()
    };
    if docs.is_empty() {
        return line;
    }
    lsp::VSContainerElement {
        style: lsp::VSContainerElementStyle::STACKED,
        elements: vec![
            lsp::VSImageElementOrClassifiedTextElementOrContainerElement {
                container_element: Some(Box::new(line)),
                ..Default::default()
            },
            lsp::VSImageElementOrClassifiedTextElementOrContainerElement {
                classified_text_element: Some(Box::new(lsp::VSClassifiedTextElement {
                    runs: vec![Some(Box::new(lsp::VSClassifiedTextRun {
                        classification_type_name: "text".into(),
                        text: docs.into(),
                        ..Default::default()
                    }))],
                    ..Default::default()
                })),
                ..Default::default()
            },
        ],
        ..Default::default()
    }
}
