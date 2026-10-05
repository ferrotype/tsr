use crate::{syntax::Syntax, CompletionOptions, Result};
use tsr_ast::{utilities as ast, NodeId, SyntaxKind as K};
use tsr_checker::{type_flags as tf, Operation};
use tsr_lsproto as lsp;

// port: tsc/internal/ls/completions.go:getJSDocParameterCompletions
pub(crate) fn completions(
    checker: &mut Operation<'_>,
    syntax: &mut Syntax<'_>,
    position: i64,
    name_only: bool,
    options: &CompletionOptions,
) -> Result<Vec<Option<Box<lsp::CompletionItem>>>> {
    let token = syntax.nav().get_token_at_position(position)?;
    let read = syntax.view.node(token)?;
    let doc = if read.kind() == K::JSDoc {
        token
    } else if tsr_ast::utilities_middle::is_js_doc_tag(&read) {
        read.parent().ok_or(tsr_arena::Error::InvalidGraph)?
    } else {
        return Ok(Vec::new());
    };
    let read = syntax.view.node(doc)?;
    if read.kind() != K::JSDoc {
        return Ok(Vec::new());
    }
    let Some(function) = read.parent() else {
        return Ok(Vec::new());
    };
    if !ast::is_function_like(Some(&syntax.view.node(function)?)) {
        return Ok(Vec::new());
    }
    let tags: Vec<_> = read
        .data_source()
        .as_js_doc()
        .and_then(|d| d.tags())
        .map(|tags| syntax.view.list(tags).map(|l| l.nodes()))
        .transpose()?
        .map(|nodes| {
            syntax
                .view
                .node_slice(nodes)
                .map(|s| s.iter().flatten().collect())
        })
        .transpose()?
        .unwrap_or_default();
    let mut count = 0;
    for tag in tags {
        let read = syntax.view.node(tag)?;
        if read.kind() == K::JSDocParameterTag
            && read
                .name()
                .is_some_and(|n| syntax.view.node(n).is_ok_and(|r| r.kind() == K::Identifier))
            && syntax.start(tag)? < position
        {
            count += 1;
        }
    }
    let params: Vec<_> = syntax
        .view
        .node_slice(syntax.view.node(function)?.parameters(syntax.view)?)?
        .iter()
        .flatten()
        .collect();
    let mut result = Vec::new();
    for (index, param) in params.into_iter().enumerate().skip(count) {
        let read = syntax.view.node(param)?;
        let Some(name) = read.name() else {
            continue;
        };
        let rest = read
            .data_source()
            .as_parameter_declaration()
            .and_then(|d| d.dot_dot_dot_token())
            .is_some();
        let mut label = if syntax.view.node(name)?.kind() == K::Identifier {
            annotation(
                checker,
                syntax,
                function,
                &String::from_utf8_lossy(syntax.view.node_text(name)?.as_bytes()),
                read.initializer(),
                rest,
                false,
                options,
            )?
        } else if index == count {
            pattern(
                checker,
                syntax,
                function,
                &format!("param{index}"),
                name,
                read.initializer(),
                rest,
                options,
            )?
            .join(&format!("{}* ", options.newline.as_deref().unwrap_or("\n")))
        } else {
            continue;
        };
        if name_only {
            label.remove(0);
        }
        result.push(Some(Box::new(lsp::CompletionItem {
            label,
            kind: Some(Box::new(lsp::CompletionItemKind::VARIABLE)),
            sort_text: Some(Box::new("11".into())),
            ..Default::default()
        })));
    }
    Ok(result)
}
// port: tsc/internal/ls/completions.go:getJSDocParamNameWithInitializer
fn initialized_name(syntax: &Syntax<'_>, name: &str, initializer: NodeId) -> Result<String> {
    let text = tsr_scanner::get_text_of_node(syntax.view, initializer)?;
    let text = tsr_jsstring::JsString::from_bytes(
        String::from_utf8_lossy(text.as_bytes()).trim().as_bytes(),
    );
    Ok(
        if text.as_bytes().len() > 80 || text.as_bytes().contains(&b'\n') {
            format!("[{name}]")
        } else {
            format!("[{name}={}]", String::from_utf8_lossy(text.as_bytes()))
        },
    )
}
#[allow(
    clippy::too_many_arguments,
    reason = "JSDoc parameter rendering carries the pinned declaration, binding and preference inputs"
)]
fn annotation(
    checker: &mut Operation<'_>,
    syntax: &Syntax<'_>,
    function: NodeId,
    name: &str,
    initializer: Option<NodeId>,
    rest: bool,
    object: bool,
    options: &CompletionOptions,
) -> Result<String> {
    let name = initializer.map_or_else(
        || Ok(name.to_owned()),
        |init| initialized_name(syntax, name, init),
    )?;
    if !syntax.file.is_js() {
        return Ok(format!("@param {name} "));
    }
    let mut ty = if object { "object" } else { "*" }.to_owned();
    if !object {
        if let Some(init) = initializer {
            let parent = syntax
                .view
                .node(init)?
                .parent()
                .ok_or(tsr_arena::Error::InvalidGraph)?;
            let inferred = checker.get_type_at_location(parent)?;
            if checker.type_flags(inferred)? & (tf::ANY | tf::VOID) == 0 {
                let flags = if crate::inlay_hints::single_quote(syntax, options.quote)? {
                    tsr_nodebuilder::flags::USE_SINGLE_QUOTES_FOR_STRING_LITERAL_TYPE
                } else {
                    0
                };
                let mut builder = checker.node_builder();
                if let Some(node) = builder.type_to_type_node(inferred, Some(function), flags, 0)? {
                    let mut emit = tsr_printer::EmitContext::new();
                    emit.set_emit_flags(node, tsr_printer::emit_flags::SINGLE_LINE);
                    let mut printer = tsr_printer::Printer::new(
                        tsr_printer::PrinterOptions {
                            remove_comments: true,
                            ..Default::default()
                        },
                        &emit,
                    );
                    ty = String::from_utf8_lossy(&printer.emit(
                        builder.view(),
                        node,
                        Some(syntax.source),
                    )?)
                    .into_owned();
                }
            }
        }
    }
    Ok(format!(
        "@param {{{}{ty}}} {name} ",
        if rest && !object { "..." } else { "" }
    ))
}
#[allow(
    clippy::too_many_arguments,
    reason = "Recursive binding-pattern rendering retains the enclosing function and parameter options"
)]
fn pattern(
    checker: &mut Operation<'_>,
    syntax: &Syntax<'_>,
    function: NodeId,
    path: &str,
    node: NodeId,
    initializer: Option<NodeId>,
    rest: bool,
    options: &CompletionOptions,
) -> Result<Vec<String>> {
    stacker::maybe_grow(64 * 1024, 1024 * 1024, || {
        if syntax.file.is_js() && syntax.view.node(node)?.kind() == K::ObjectBindingPattern && !rest
        {
            let mut children = Vec::new();
            for element in syntax
                .view
                .node_slice(syntax.view.node(node)?.elements(syntax.view)?)?
                .iter()
                .flatten()
            {
                let read = syntax.view.node(element)?;
                let Some(name) = read.name() else {
                    children.clear();
                    break;
                };
                let property = read.property_name().or_else(|| {
                    (syntax
                        .view
                        .node(name)
                        .is_ok_and(|r| r.kind() == K::Identifier))
                    .then_some(name)
                });
                let Some(property) = property else {
                    children.clear();
                    break;
                };
                let Some(property) = tsr_ast::utilities_targets::try_get_text_of_property_name(
                    syntax.view,
                    property,
                )?
                else {
                    children.clear();
                    break;
                };
                if property.is_empty() {
                    children.clear();
                    break;
                }
                let path = format!("{path}.{}", String::from_utf8_lossy(&property));
                let rest = read
                    .data_source()
                    .as_binding_element()
                    .and_then(|d| d.dot_dot_dot_token())
                    .is_some();
                if syntax.view.node(name)?.kind() == K::Identifier {
                    children.push(annotation(
                        checker,
                        syntax,
                        function,
                        &path,
                        read.initializer(),
                        rest,
                        false,
                        options,
                    )?);
                } else {
                    children.extend(pattern(
                        checker,
                        syntax,
                        function,
                        &path,
                        name,
                        read.initializer(),
                        rest,
                        options,
                    )?);
                }
            }
            if !children.is_empty() {
                children.insert(
                    0,
                    annotation(
                        checker,
                        syntax,
                        function,
                        path,
                        initializer,
                        rest,
                        true,
                        options,
                    )?,
                );
                return Ok(children);
            }
        }
        Ok(vec![annotation(
            checker,
            syntax,
            function,
            path,
            initializer,
            rest,
            false,
            options,
        )?])
    })
}
