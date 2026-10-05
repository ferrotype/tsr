//! JSX completion contexts. All syntax is read from the retained request tree.
use crate::{
    completion_context::{Context, ALL},
    completion_items,
    completions::CompletionOptions,
    syntax::Syntax,
    LanguageService, Result,
};
use tsr_ast::{span_map::FEATURE_COMPLETION, NodeId, SyntaxKind as K};
use tsr_checker::{Operation, SymbolRef};
use tsr_lsproto as lsp;

fn opening(syntax: &Syntax<'_>, id: NodeId) -> Result<bool> {
    Ok(matches!(
        syntax.view.node(id)?.kind().known(),
        Some(K::JsxOpeningElement | K::JsxSelfClosingElement)
    ))
}
// port: tsc/internal/ls/completions.go:tryGetContainingJsxElement
pub(crate) fn container(syntax: &mut Syntax<'_>, token: Option<NodeId>) -> Result<Option<NodeId>> {
    let Some(token) = token else { return Ok(None) };
    let read = syntax.view.node(token)?;
    let Some(parent) = read.parent() else {
        return Ok(None);
    };
    let pr = syntax.view.node(parent)?;
    let ancestor = |mut id, n| -> Result<Option<NodeId>> {
        for _ in 0..n {
            let Some(p) = syntax.view.node(id)?.parent() else {
                return Ok(None);
            };
            id = p;
        }
        Ok(Some(id))
    };
    match read.kind().known() {
        Some(
            K::GreaterThanToken
            | K::LessThanSlashToken
            | K::SlashToken
            | K::Identifier
            | K::PropertyAccessExpression
            | K::JsxNamespacedName
            | K::JsxAttributes
            | K::JsxAttribute
            | K::JsxSpreadAttribute,
        ) => {
            if opening(syntax, parent)? {
                if read.kind() == K::GreaterThanToken {
                    if syntax
                        .view
                        .node_slice(pr.type_arguments(syntax.view)?)?
                        .is_empty()
                    {
                        return Ok(None);
                    }
                    if syntax
                        .nav()
                        .find_preceding_token(i64::from(read.pos()))?
                        .is_some_and(|p| {
                            syntax.view.node(p).is_ok_and(|r| r.kind() == K::SlashToken)
                        })
                    {
                        return Ok(None);
                    }
                }
                return Ok(Some(parent));
            }
            if pr.kind() == K::JsxNamespacedName {
                if let Some(parent) = pr.parent() {
                    if opening(syntax, parent)? {
                        return Ok(Some(parent));
                    }
                }
            }
            if pr.kind() == K::JsxAttribute {
                return ancestor(parent, 2);
            }
        }
        Some(K::StringLiteral)
            if matches!(
                pr.kind().known(),
                Some(K::JsxAttribute | K::JsxSpreadAttribute)
            ) =>
        {
            return ancestor(parent, 2)
        }
        Some(K::CloseBraceToken) => {
            if pr.kind() == K::JsxExpression
                && pr.parent().is_some_and(|p| {
                    syntax
                        .view
                        .node(p)
                        .is_ok_and(|r| r.kind() == K::JsxAttribute)
                })
            {
                return ancestor(parent, 3);
            }
            if pr.kind() == K::JsxSpreadAttribute {
                return ancestor(parent, 2);
            }
        }
        _ => {}
    }
    Ok(None)
}
pub(crate) fn open_tag(syntax: &Syntax<'_>, context: &Context) -> Result<bool> {
    let Some(token) = context.token else {
        return Ok(false);
    };
    let read = syntax.view.node(token)?;
    if read.kind() != K::LessThanToken {
        return Ok(false);
    }
    let Some(parent) = read.parent() else {
        return Ok(false);
    };
    let pr = syntax.view.node(parent)?;
    Ok(matches!(
        pr.kind().known(),
        Some(K::JsxOpeningElement | K::JsxSelfClosingElement | K::JsxElement)
    ) || pr.kind() == K::BinaryExpression
        && pr
            .data_source()
            .as_binary_expression()
            .and_then(|d| d.left())
            .is_none_or(|id| {
                syntax
                    .view
                    .node(id)
                    .is_ok_and(|n| tsr_ast::node_is_missing(Some(&n)))
            }))
}

impl LanguageService<'_> {
    // port: tsc/internal/ls/completions.go:LanguageService.getJsxClosingTagCompletion
    pub(crate) fn closing_tag_completion(
        &mut self,
        syntax: &mut Syntax<'_>,
        context: &Context,
        position: &lsp::Position,
        options: &CompletionOptions,
    ) -> Result<Option<lsp::CompletionList>> {
        let mut current = Some(context.location);
        let mut closing = None;
        while let Some(id) = current {
            let read = syntax.view.node(id)?;
            match read.kind().known() {
                Some(K::JsxClosingElement) => {
                    closing = Some(id);
                    break;
                }
                Some(
                    K::LessThanSlashToken
                    | K::GreaterThanToken
                    | K::Identifier
                    | K::PropertyAccessExpression,
                ) => current = read.parent(),
                _ => break,
            }
        }
        let Some(closing) = closing else {
            return Ok(None);
        };
        let read = syntax.view.node(closing)?;
        let Some(element) = read
            .parent()
            .and_then(|p| syntax.view.node(p).ok())
            .and_then(|p| {
                p.data_source()
                    .as_jsx_element()
                    .and_then(|d| d.opening_element())
            })
        else {
            return Ok(None);
        };
        let Some(tag) = syntax.view.node(element)?.tag_name() else {
            return Ok(None);
        };
        let mut label =
            String::from_utf8_lossy(tsr_scanner::get_text_of_node(syntax.view, tag)?.as_bytes())
                .into_owned();
        if syntax
            .nav()
            .find_child_of_kind(closing, K::GreaterThanToken)?
            .is_none()
        {
            label.push('>');
        }
        let Some(name) = read.tag_name() else {
            return Ok(None);
        };
        let (range, fidelity) = self.range(
            syntax.source,
            tsr_core::TextRange::new(
                syntax.start(name)?,
                i64::from(syntax.view.node(name)?.end()),
            ),
            FEATURE_COMPLETION,
        )?;
        if !fidelity.is_exact() {
            return Ok(None);
        }
        let mut list = lsp::CompletionList {
            items: vec![Some(Box::new(lsp::CompletionItem {
                label,
                kind: Some(Box::new(lsp::CompletionItemKind::CLASS)),
                sort_text: Some(Box::new("11".into())),
                ..Default::default()
            }))],
            ..Default::default()
        };
        completion_items::defaults(&mut list, options, position, Some(range), ALL);
        Ok(Some(list))
    }

    pub(crate) fn jsx_attribute_insert(
        checker: &mut Operation<'_>,
        syntax: &mut Syntax<'_>,
        context: &Context,
        symbol: SymbolRef,
        name: &str,
        options: &CompletionOptions,
    ) -> Result<Option<String>> {
        if !options.snippets
            || !matches!(
                options.jsx_attribute_style.as_deref(),
                Some("auto" | "braces")
            )
        {
            return Ok(None);
        }
        let location = syntax.view.node(context.location)?;
        if location.parent().is_some_and(|p| {
            syntax
                .view
                .node(p)
                .is_ok_and(|r| r.kind() == K::JsxAttribute && r.initializer().is_some())
        }) {
            return Ok(None);
        }
        let ty = checker.get_type_of_symbol_at_location(symbol, Some(context.location))?;
        let types = if checker.type_flags(ty)? & tsr_checker::type_flags::UNION != 0 {
            checker.constituents(ty)?
        } else {
            vec![ty]
        };
        let mut braces = options.jsx_attribute_style.as_deref() == Some("braces");
        if !braces {
            for &ty in &types {
                if checker.type_flags(ty)? & tsr_checker::type_flags::BOOLEAN_LIKE != 0 {
                    return Ok(None);
                }
            }
            let mut strings = true;
            for &ty in &types {
                let flags = checker.type_flags(ty)?;
                if flags
                    & (tsr_checker::type_flags::STRING_LIKE | tsr_checker::type_flags::UNDEFINED)
                    == 0
                {
                    strings = false;
                }
            }
            if strings {
                let quote = if crate::inlay_hints::single_quote(syntax, options.quote)? {
                    '\''
                } else {
                    '"'
                };
                return Ok(Some(format!(
                    "{}={quote}$1{quote}",
                    name.replace('$', "\\$")
                )));
            }
            braces = true;
        }
        Ok(braces.then(|| format!("{}={{$1}}", name.replace('$', "\\$"))))
    }
}
