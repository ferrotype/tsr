//! Declaration identity and display shared by all three call-hierarchy requests.
use crate::{
    definition::ancestor, documentation::list, reference_helpers::declarations, syntax::Syntax,
    LanguageService, Result,
};
use tsr_ast::{utilities as ast, utilities_positions as pos, AstView, NodeId, SyntaxKind as K};
use tsr_checker::{Operation, SymbolRef};
use tsr_lsproto as lsp;
use tsr_printer::EmitTextWriter;

fn variable_like(view: AstView<'_>, node: NodeId) -> Result<bool> {
    Ok(matches!(
        view.node(node)?.kind().known(),
        Some(K::PropertyDeclaration | K::VariableDeclaration)
    ))
}

// port: tsc/internal/ls/callhierarchy.go:isAssignedExpression
pub(crate) fn assigned(view: AstView<'_>, node: NodeId) -> Result<bool> {
    let n = view.node(node)?;
    if !matches!(
        n.kind().known(),
        Some(K::FunctionExpression | K::ArrowFunction | K::ClassExpression)
    ) || n.name().is_some()
    {
        return Ok(false);
    }
    let Some(parent) = n.parent() else {
        return Ok(false);
    };
    let p = view.node(parent)?;
    Ok(variable_like(view, parent)?
        && p.initializer() == Some(node)
        && p.name()
            .is_some_and(|id| view.node(id).is_ok_and(|n| n.kind() == K::Identifier))
        && (p.kind() == K::PropertyDeclaration
            || ast::get_combined_node_flags(view, parent)? & tsr_ast::node_flags::CONST != 0))
}
fn possible(view: AstView<'_>, node: NodeId) -> Result<bool> {
    Ok(matches!(
        view.node(node)?.kind().known(),
        Some(
            K::SourceFile
                | K::ModuleDeclaration
                | K::FunctionDeclaration
                | K::FunctionExpression
                | K::ClassDeclaration
                | K::ClassExpression
                | K::ClassStaticBlockDeclaration
                | K::MethodDeclaration
                | K::MethodSignature
                | K::GetAccessor
                | K::SetAccessor
        )
    ))
}
// port: tsc/internal/ls/callhierarchy.go:isValidCallHierarchyDeclaration
pub(crate) fn valid(view: AstView<'_>, node: NodeId) -> Result<bool> {
    let n = view.node(node)?;
    Ok(match n.kind().known() {
        Some(
            K::SourceFile
            | K::FunctionDeclaration
            | K::ClassDeclaration
            | K::ClassStaticBlockDeclaration
            | K::MethodDeclaration
            | K::MethodSignature
            | K::GetAccessor
            | K::SetAccessor,
        ) => true,
        Some(K::ModuleDeclaration) => n
            .name()
            .is_some_and(|id| view.node(id).is_ok_and(|n| n.kind() == K::Identifier)),
        Some(K::ClassExpression | K::FunctionExpression) => {
            n.name()
                .is_some_and(|id| view.node(id).is_ok_and(|n| n.kind() == K::Identifier))
                || assigned(view, node)?
        }
        _ => assigned(view, node)?,
    })
}
// port: tsc/internal/ls/callhierarchy.go:getCallHierarchyDeclarationReferenceNode
pub(crate) fn reference(view: AstView<'_>, node: NodeId) -> Result<Option<NodeId>> {
    let n = view.node(node)?;
    if n.kind() == K::SourceFile {
        return Ok(Some(node));
    }
    if n.name().is_some() {
        return Ok(n.name());
    }
    if assigned(view, node)? {
        return Ok(view.node(n.parent().unwrap())?.name());
    }
    for modifier in list(view, n.modifiers())? {
        if view.node(modifier)?.kind() == K::DefaultKeyword {
            return Ok(Some(modifier));
        }
    }
    Ok(None)
}
fn symbol(
    l: &LanguageService<'_>,
    c: &mut Operation<'_>,
    node: NodeId,
) -> Result<Option<SymbolRef>> {
    let view = l.view(node)?;
    if view.node(node)?.kind() == K::ClassStaticBlockDeclaration {
        return Ok(None);
    }
    reference(view, node)?
        .map(|n| c.get_symbol_at_location(n).map_err(Into::into))
        .transpose()
        .map(Option::flatten)
}
// port: tsc/internal/ls/callhierarchy.go:findImplementation
pub(crate) fn implementation(
    l: &LanguageService<'_>,
    c: &mut Operation<'_>,
    node: NodeId,
) -> Result<Option<NodeId>> {
    let view = l.view(node)?;
    let n = view.node(node)?;
    if !ast::is_function_like_declaration(Some(&n)) || n.body().is_some() {
        return Ok(Some(node));
    }
    if n.kind() == K::Constructor {
        if let Some(parent) = n.parent() {
            for member in list(view, view.node(parent)?.member_list())? {
                let m = view.node(member)?;
                if m.kind() == K::Constructor && m.body().is_some() {
                    return Ok(Some(member));
                }
            }
        }
        return Ok(None);
    }
    if matches!(
        n.kind().known(),
        Some(K::FunctionDeclaration | K::MethodDeclaration)
    ) {
        if let Some(s) = symbol(l, c, node)? {
            if let Some(decl) = c.symbol(s)?.value_declaration() {
                let n = c.node(decl)?;
                if ast::is_function_like_declaration(Some(&n)) && n.body().is_some() {
                    return Ok(Some(decl));
                }
            }
        }
        return Ok(None);
    }
    Ok(Some(node))
}
// port: tsc/internal/ls/callhierarchy.go:findImplementationOrAllInitialDeclarations
fn initial(l: &LanguageService<'_>, c: &mut Operation<'_>, node: NodeId) -> Result<Vec<NodeId>> {
    let n = c.node(node)?;
    if n.kind() == K::ClassStaticBlockDeclaration {
        return Ok(vec![node]);
    }
    if ast::is_function_like_declaration(Some(&n)) {
        if let Some(implementation) = implementation(l, c, node)? {
            return Ok(vec![implementation]);
        }
    }
    let mut sorted = Vec::new();
    if let Some(symbol) = symbol(l, c, node)? {
        for decl in declarations(c, symbol)? {
            let view = l.view(decl)?;
            let source = ast::get_source_file_of_node(view, Some(decl))?
                .ok_or(tsr_arena::Error::InvalidGraph)?;
            sorted.push((
                (
                    l.source(source)?.file_name().to_vec(),
                    view.node(decl)?.pos(),
                ),
                decl,
            ));
        }
    }
    sorted.sort_by(|(a, _), (b, _)| a.cmp(b));
    let mut result = Vec::new();
    let mut last = None;
    for (_, decl) in sorted {
        let view = l.view(decl)?;
        if valid(view, decl)? {
            let n = view.node(decl)?;
            if last.is_none_or(|(parent, end)| parent != n.parent() || end != n.pos()) {
                result.push(decl);
            }
            last = Some((n.parent(), n.end()));
        }
    }
    if result.is_empty() {
        result.push(node);
    }
    Ok(result)
}
// port: tsc/internal/ls/callhierarchy.go:resolveCallHierarchyDeclaration
pub(crate) fn resolve(
    l: &LanguageService<'_>,
    c: &mut Operation<'_>,
    mut node: NodeId,
) -> Result<Vec<NodeId>> {
    let mut following = false;
    loop {
        let view = l.view(node)?;
        let n = view.node(node)?;
        if valid(view, node)? {
            return initial(l, c, node);
        }
        if possible(view, node)? {
            if let Some(a) = ancestor(view, Some(node), |n| valid(view, n))? {
                return initial(l, c, a);
            }
        }
        if pos::is_declaration_name(view, node)? {
            let parent = n.parent().ok_or(tsr_arena::Error::InvalidGraph)?;
            if valid(view, parent)? {
                return initial(l, c, parent);
            }
            if possible(view, parent)? {
                if let Some(a) = ancestor(view, Some(parent), |n| valid(view, n))? {
                    return initial(l, c, a);
                }
            }
            if variable_like(view, parent)? {
                if let Some(init) = view.node(parent)?.initializer() {
                    if assigned(view, init)? {
                        return Ok(vec![init]);
                    }
                }
            }
            return Ok(Vec::new());
        }
        if n.kind() == K::Constructor {
            return Ok(n
                .parent()
                .filter(|id| valid(view, *id).unwrap_or(false))
                .into_iter()
                .collect());
        }
        if n.kind() == K::StaticKeyword {
            if let Some(parent) = n.parent() {
                if view.node(parent)?.kind() == K::ClassStaticBlockDeclaration {
                    node = parent;
                    continue;
                }
            }
        }
        if n.kind() == K::VariableDeclaration {
            if let Some(init) = n.initializer() {
                if assigned(view, init)? {
                    return Ok(vec![init]);
                }
            }
        }
        if !following {
            if let Some(mut symbol) = c.get_symbol_at_location(node)? {
                if c.symbol(symbol)?.flags() & tsr_ast::symbol_flags::ALIAS != 0 {
                    symbol = c.get_aliased_symbol(symbol)?;
                }
                if let Some(decl) = c.symbol(symbol)?.value_declaration() {
                    following = true;
                    node = decl;
                    continue;
                }
            }
        }
        return Ok(Vec::new());
    }
}

impl LanguageService<'_> {
    fn call_name_text(&self, c: &mut Operation<'_>, name: NodeId, print: NodeId) -> Result<String> {
        let view = self.view(name)?;
        let n = view.node(name)?;
        if matches!(
            n.kind().known(),
            Some(
                K::Identifier
                    | K::StringLiteral
                    | K::NumericLiteral
                    | K::BigIntLiteral
                    | K::NoSubstitutionTemplateLiteral
            )
        ) {
            return Ok(String::from_utf8_lossy(view.node_text(name)?.as_bytes()).into_owned());
        }
        if n.kind() == K::ComputedPropertyName {
            if let Some(expr) = n.expression() {
                if matches!(
                    view.node(expr)?.kind().known(),
                    Some(
                        K::StringLiteral
                            | K::NumericLiteral
                            | K::BigIntLiteral
                            | K::NoSubstitutionTemplateLiteral
                    )
                ) {
                    return Ok(
                        String::from_utf8_lossy(view.node_text(expr)?.as_bytes()).into_owned()
                    );
                }
            }
        }
        if let Some(symbol) = c.get_symbol_at_location(name)? {
            let text = c.symbol_to_string(symbol)?;
            if !text.is_empty() {
                return Ok(String::from_utf8_lossy(text.as_bytes()).into_owned());
            }
        }
        let emit = tsr_printer::EmitContext::default();
        let printer = tsr_printer::Printer::new(
            tsr_printer::PrinterOptions {
                remove_comments: true,
                ..Default::default()
            },
            &emit,
        );
        let mut writer = tsr_printer::SingleLineStringWriter::new();
        let source = ast::get_source_file_of_node(view, Some(print))?;
        printer.write(view, print, source, &mut writer, None)?;
        Ok(String::from_utf8_lossy(writer.text()).into_owned())
    }
    // port: tsc/internal/ls/callhierarchy.go:getCallHierarchyItemContainerName
    fn call_container(&self, c: &mut Operation<'_>, node: NodeId) -> Result<String> {
        let view = self.view(node)?;
        let n = view.node(node)?;
        let Some(parent) = n.parent() else {
            return Ok(String::new());
        };
        let p = view.node(parent)?;
        let mut name = None;
        if assigned(view, node)? {
            if p.kind() == K::PropertyDeclaration {
                if let Some(class) = p.parent() {
                    if ast::is_class_like(&view.node(class)?) {
                        name = if view.node(class)?.kind() == K::ClassExpression {
                            tsr_ast::get_assigned_name(view, class)?
                        } else {
                            view.node(class)?.name()
                        };
                    }
                }
            }
            if name.is_none() {
                if let Some(list) = p.parent() {
                    if let Some(statement) = view.node(list)?.parent() {
                        if let Some(block) = view.node(statement)?.parent() {
                            if view.node(block)?.kind() == K::ModuleBlock {
                                if let Some(module) = view.node(block)?.parent() {
                                    name = view.node(module)?.name().filter(|id| {
                                        view.node(*id).is_ok_and(|n| n.kind() == K::Identifier)
                                    });
                                }
                            }
                        }
                    }
                }
            }
        } else {
            match n.kind().known() {
                Some(K::GetAccessor | K::SetAccessor | K::MethodDeclaration) => {
                    if p.kind() == K::ObjectLiteralExpression {
                        name = tsr_ast::get_assigned_name(view, parent)?;
                    }
                    if name.is_none() {
                        name = tsr_ast::get_name_of_declaration(view, Some(parent))?;
                    }
                }
                Some(K::FunctionDeclaration | K::ClassDeclaration | K::ModuleDeclaration)
                    if p.kind() == K::ModuleBlock =>
                {
                    if let Some(module) = p.parent() {
                        name = view
                            .node(module)?
                            .name()
                            .filter(|id| view.node(*id).is_ok_and(|n| n.kind() == K::Identifier));
                    }
                }
                _ => {}
            }
        }
        name.map(|n| self.call_name_text(c, n, n))
            .transpose()
            .map(Option::unwrap_or_default)
    }
    // port: tsc/internal/ls/callhierarchy.go:LanguageService.createCallHierarchyItem
    pub(crate) fn call_item(
        &mut self,
        c: &mut Operation<'_>,
        node: NodeId,
    ) -> Result<Option<lsp::CallHierarchyItem>> {
        use tsr_ast::span_map::FEATURE_CALL_HIERARCHY;
        let view = self.view(node)?;
        let source = ast::get_source_file_of_node(view, Some(node))?
            .ok_or(tsr_arena::Error::InvalidGraph)?;
        let mut syntax = Syntax::new(view, source)?;
        let n = view.node(node)?;
        let after_modifiers = list(view, n.modifiers())?
            .last()
            .map(|id| view.node(*id).map(|n| i64::from(n.end())))
            .transpose()?
            .unwrap_or(i64::from(n.pos()));
        let keyword_pos = tsr_scanner::skip_trivia(syntax.file.text().as_bytes(), after_modifiers);
        let (name, start, end) = if n.kind() == K::SourceFile {
            (
                String::from_utf8_lossy(syntax.file.file_name()).into_owned(),
                0,
                0,
            )
        } else if n.kind() == K::ClassStaticBlockDeclaration {
            let mut name = String::new();
            if let Some(parent) = n.parent() {
                if let Some(symbol) = c.get_symbol_at_location(parent)? {
                    name.push_str(&String::from_utf8_lossy(
                        c.symbol_to_string(symbol)?.as_bytes(),
                    ));
                    name.push(' ');
                }
            }
            name.push_str("static {}");
            (name, keyword_pos, keyword_pos + 6)
        } else {
            let mut decl_name = if assigned(view, node)? {
                view.node(n.parent().unwrap())?.name()
            } else {
                tsr_ast::get_name_of_declaration(view, Some(node))?
            };
            if decl_name.is_none()
                && matches!(
                    n.kind().known(),
                    Some(K::FunctionDeclaration | K::ClassDeclaration)
                )
            {
                decl_name = list(view, n.modifiers())?
                    .into_iter()
                    .find(|id| view.node(*id).is_ok_and(|n| n.kind() == K::DefaultKeyword));
            }
            if let Some(name) =
                decl_name.filter(|id| view.node(*id).is_ok_and(|n| n.pos() != n.end()))
            {
                let text = if view.node(name)?.kind() == K::DefaultKeyword {
                    "default".into()
                } else {
                    self.call_name_text(c, name, node)?
                };
                (text, syntax.start(name)?, i64::from(view.node(name)?.end()))
            } else {
                let len = if ast::is_class_like(&n) { 5 } else { 8 };
                ("(anonymous)".into(), keyword_pos, keyword_pos + len)
            }
        };
        let container = self.call_container(c, node)?;
        let start_full = tsr_scanner::skip_trivia_ex(
            syntax.file.text().as_bytes(),
            i64::from(n.pos()),
            Some(&tsr_scanner::SkipTriviaOptions {
                stop_at_comments: true,
                ..Default::default()
            }),
        );
        let (mut range, f) = self.range(
            source,
            tsr_core::TextRange::new(start_full, i64::from(n.end())),
            FEATURE_CALL_HIERARCHY,
        )?;
        let (selection, s) = self.range(
            source,
            tsr_core::TextRange::new(start, end),
            FEATURE_CALL_HIERARCHY,
        )?;
        if !s.is_single_segment() {
            return Ok(None);
        }
        if f.is_none()
            || !syntax.file.content_mapper().is_empty()
                && !crate::definition::range_contains(&range, &selection)
        {
            range = selection.clone();
        }
        Ok(Some(lsp::CallHierarchyItem {
            name,
            kind: crate::symbols::symbol_kind(view, node)?,
            uri: lsp::DocumentUri::from_file_name(syntax.file.original_file_name()?.as_bytes()),
            range,
            selection_range: selection,
            detail: (!container.is_empty()).then(|| Box::new(container)),
            ..Default::default()
        }))
    }
}
