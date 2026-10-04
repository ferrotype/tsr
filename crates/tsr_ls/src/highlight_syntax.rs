//! Syntax highlights use control-flow ownership, never text matching alone.
use crate::{
    definition::ancestor, documentation::list, reference_helpers::declarations, syntax::Syntax,
    Result,
};
use tsr_ast::{utilities as ast, NodeId, SyntaxKind as K};
use tsr_checker::Operation;
use tsr_core::TextRange;

fn is_loop(kind: K) -> bool {
    matches!(
        kind,
        K::ForStatement
            | K::ForInStatement
            | K::ForOfStatement
            | K::WhileStatement
            | K::DoStatement
    )
}

// port: tsc/internal/ls/documenthighlights.go:getBreakOrContinueOwner
fn jump_owner(s: &Syntax<'_>, statement: NodeId) -> Result<Option<NodeId>> {
    let v = s.view;
    let n = v.node(statement)?;
    let label = n.label().map(|id| v.node_text(id)).transpose()?;
    let mut at = n.parent();
    while let Some(id) = at {
        let n = v.node(id)?;
        if ast::is_function_like(Some(&n)) {
            return Ok(None);
        }
        if n.kind().known().is_some_and(is_loop)
            || n.kind() == K::SwitchStatement && v.node(statement)?.kind() == K::BreakStatement
        {
            if let Some(label) = &label {
                let mut p = n.parent();
                while let Some(p_id) = p {
                    let parent = v.node(p_id)?;
                    if parent.kind() != K::LabeledStatement {
                        break;
                    }
                    if let Some(name) = parent.label() {
                        if v.node_text(name)?.as_bytes() == label.as_bytes() {
                            return Ok(Some(id));
                        }
                    }
                    p = parent.parent();
                }
            } else {
                return Ok(Some(id));
            }
        }
        at = n.parent();
    }
    Ok(None)
}

// port: tsc/internal/ls/documenthighlights.go:aggregateOwnedThrowStatements
fn throws(s: &Syntax<'_>, root: NodeId) -> Result<Vec<NodeId>> {
    let mut stack = vec![root];
    let mut out = Vec::new();
    while let Some(id) = stack.pop() {
        let n = s.view.node(id)?;
        if n.kind() == K::ThrowStatement {
            out.push(id);
            continue;
        }
        if ast::is_function_like(Some(&n)) {
            continue;
        }
        if let Some(d) = n.data_source().as_try_statement() {
            stack.extend(d.finally_block());
            stack.extend(d.catch_clause().or(d.try_block()));
        } else {
            stack.extend(s.children(id)?.into_iter().rev());
        }
    }
    Ok(out)
}

fn within_function(s: &Syntax<'_>, root: NodeId, kind: K, restricted: bool) -> Result<Vec<NodeId>> {
    let mut stack = vec![root];
    let mut out = Vec::new();
    while let Some(id) = stack.pop() {
        let n = s.view.node(id)?;
        if n.kind() == kind {
            out.push(id);
        }
        if ast::is_function_like(Some(&n))
            || restricted
                && (ast::is_class_like(&n)
                    || matches!(
                        n.kind().known(),
                        Some(
                            K::InterfaceDeclaration
                                | K::ModuleDeclaration
                                | K::TypeAliasDeclaration
                        )
                    )
                    || ast::is_type_node(&n))
        {
            continue;
        }
        stack.extend(s.children(id)?.into_iter().rev());
    }
    Ok(out)
}

// port: tsc/internal/ls/documenthighlights.go:getThrowStatementOwner
fn throw_owner(s: &Syntax<'_>, mut child: NodeId) -> Result<Option<NodeId>> {
    while let Some(parent) = s.view.node(child)?.parent() {
        let p = s.view.node(parent)?;
        if ast::is_function_block(s.view, Some(parent))? || p.kind() == K::SourceFile {
            return Ok(Some(parent));
        }
        if p.data_source()
            .as_try_statement()
            .is_some_and(|d| d.try_block() == Some(child) && d.catch_clause().is_some())
        {
            return Ok(Some(child));
        }
        child = parent;
    }
    Ok(None)
}

fn add_tokens(
    s: &mut Syntax<'_>,
    nodes: Vec<NodeId>,
    kind: K,
    out: &mut Vec<NodeId>,
) -> Result<()> {
    for node in nodes {
        out.extend(s.nav().find_child_of_kind(node, kind)?);
    }
    Ok(())
}

// port: tsc/internal/ls/documenthighlights.go:getSwitchCaseDefaultOccurrences
// port: tsc/internal/ls/documenthighlights.go:getLoopBreakContinueOccurrences
fn loop_switch(s: &mut Syntax<'_>, owner: NodeId) -> Result<Vec<NodeId>> {
    let n = s.view.node(owner)?;
    let mut out = Vec::new();
    if n.kind() == K::SwitchStatement {
        out.extend(s.nav().find_child_of_kind(owner, K::SwitchKeyword)?);
        if let Some(block) = n
            .data_source()
            .as_switch_statement()
            .and_then(|d| d.case_block())
        {
            for clause in list(
                s.view,
                s.view
                    .node(block)?
                    .data_source()
                    .as_case_block()
                    .and_then(|d| d.clauses()),
            )? {
                let kind = if s.view.node(clause)?.kind() == K::CaseClause {
                    K::CaseKeyword
                } else {
                    K::DefaultKeyword
                };
                out.extend(s.nav().find_child_of_kind(clause, kind)?);
                add_owned_jumps(s, clause, owner, true, &mut out)?;
            }
        }
    } else {
        let kind = match n.kind().known() {
            Some(K::DoStatement) => K::DoKeyword,
            Some(K::WhileStatement) => K::WhileKeyword,
            _ => K::ForKeyword,
        };
        out.extend(s.nav().find_child_of_kind(owner, kind)?);
        if n.kind() == K::DoStatement {
            out.extend(s.nav().find_child_of_kind(owner, K::WhileKeyword)?);
        }
        add_owned_jumps(s, owner, owner, false, &mut out)?;
    }
    Ok(out)
}
fn add_owned_jumps(
    s: &mut Syntax<'_>,
    root: NodeId,
    owner: NodeId,
    only_break: bool,
    out: &mut Vec<NodeId>,
) -> Result<()> {
    let mut stack = vec![root];
    while let Some(id) = stack.pop() {
        let n = s.view.node(id)?;
        if matches!(
            n.kind().known(),
            Some(K::BreakStatement | K::ContinueStatement)
        ) {
            if (!only_break || n.kind() == K::BreakStatement) && jump_owner(s, id)? == Some(owner) {
                let kind = if n.kind() == K::BreakStatement {
                    K::BreakKeyword
                } else {
                    K::ContinueKeyword
                };
                out.extend(s.nav().find_child_of_kind(id, kind)?);
            }
        } else if !ast::is_function_like(Some(&n)) {
            stack.extend(s.children(id)?.into_iter().rev());
        }
    }
    Ok(())
}

// port: tsc/internal/ls/documenthighlights.go:getIfElseKeywords
fn if_keywords(s: &Syntax<'_>, mut node: NodeId) -> Result<Vec<NodeId>> {
    while let Some(p) = s.view.node(node)?.parent() {
        if s.view
            .node(p)?
            .data_source()
            .as_if_statement()
            .is_some_and(|d| d.else_statement() == Some(node))
        {
            node = p;
        } else {
            break;
        }
    }
    let mut out = Vec::new();
    loop {
        let children = s.token_children(node)?;
        if let Some(&first) = children.first() {
            if s.view.node(first)?.kind() == K::IfKeyword {
                out.push(first);
            }
        }
        for &child in children.iter().rev() {
            if s.view.node(child)?.kind() == K::ElseKeyword {
                out.push(child);
                break;
            }
        }
        let Some(next) = s
            .view
            .node(node)?
            .data_source()
            .as_if_statement()
            .and_then(|d| d.else_statement())
        else {
            break;
        };
        if s.view.node(next)?.kind() != K::IfStatement {
            break;
        }
        node = next;
    }
    Ok(out)
}

// port: tsc/internal/ls/documenthighlights.go:getNodesToSearchForModifier
fn modifier_nodes(s: &Syntax<'_>, declaration: NodeId, kind: K) -> Result<Vec<NodeId>> {
    let v = s.view;
    let Some(parent) = v.node(declaration)?.parent() else {
        return Ok(Vec::new());
    };
    let p = v.node(parent)?;
    let mut result = match p.kind().known() {
        Some(K::ModuleBlock | K::SourceFile | K::Block | K::CaseClause | K::DefaultClause) => {
            if kind == K::AbstractKeyword && v.node(declaration)?.kind() == K::ClassDeclaration {
                let mut members = list(v, v.node(declaration)?.member_list())?;
                members.push(declaration);
                members
            } else {
                list(v, p.statement_list())?
            }
        }
        Some(K::Constructor | K::MethodDeclaration | K::FunctionDeclaration) => {
            let mut params = list(v, p.parameter_list())?;
            if let Some(parent) = p.parent() {
                if ast::is_class_like(&v.node(parent)?) {
                    params.extend(list(v, v.node(parent)?.member_list())?);
                }
            }
            params
        }
        Some(
            K::ClassDeclaration | K::ClassExpression | K::InterfaceDeclaration | K::TypeLiteral,
        ) => {
            let mut nodes = list(v, p.member_list())?;
            if matches!(
                kind,
                K::PublicKeyword | K::PrivateKeyword | K::ProtectedKeyword | K::ReadonlyKeyword
            ) {
                for &member in &nodes {
                    if v.node(member)?.kind() == K::Constructor {
                        let params = list(v, v.node(member)?.parameter_list())?;
                        nodes.extend(params);
                        break;
                    }
                }
            } else if kind == K::AbstractKeyword {
                nodes.push(parent);
            }
            nodes
        }
        _ => Vec::new(),
    };
    let mut out = Vec::new();
    for n in result.drain(..) {
        for modifier in list(v, v.node(n)?.modifiers())? {
            if v.node(modifier)?.kind() == kind {
                out.push(modifier);
                break;
            }
        }
    }
    Ok(out)
}

// port: tsc/internal/ls/documenthighlights.go:LanguageService.getSyntacticDocumentHighlights
pub(crate) fn syntax_highlights(
    s: &mut Syntax<'_>,
    c: &Operation<'_>,
    node: NodeId,
) -> Result<Vec<TextRange>> {
    let v = s.view;
    let n = v.node(node)?;
    let Some(parent) = n.parent() else {
        return Ok(Vec::new());
    };
    let p = v.node(parent)?;
    let mut nodes = Vec::new();
    let mut combine_if = false;
    match n.kind().known() {
        Some(K::IfKeyword | K::ElseKeyword) if p.kind() == K::IfStatement => {
            nodes = if_keywords(s, parent)?;
            combine_if = true;
        }
        Some(K::ReturnKeyword) if p.kind() == K::ReturnStatement => {
            if let Some(fun) = ancestor(v, p.parent(), |id| {
                Ok(ast::is_function_like(Some(&v.node(id)?)))
            })? {
                if let Some(body) = v.node(fun)?.body() {
                    let returns = within_function(s, body, K::ReturnStatement, false)?;
                    add_tokens(s, returns, K::ReturnKeyword, &mut nodes)?;
                    let throws = throws(s, body)?;
                    add_tokens(s, throws, K::ThrowKeyword, &mut nodes)?;
                }
            }
        }
        Some(K::ThrowKeyword) if p.kind() == K::ThrowStatement => {
            if let Some(owner) = throw_owner(s, parent)? {
                let throws = throws(s, owner)?;
                add_tokens(s, throws, K::ThrowKeyword, &mut nodes)?;
                if ast::is_function_block(v, Some(owner))? {
                    let returns = within_function(s, owner, K::ReturnStatement, false)?;
                    add_tokens(s, returns, K::ReturnKeyword, &mut nodes)?;
                }
            }
        }
        Some(K::TryKeyword | K::CatchKeyword | K::FinallyKeyword) => {
            let owner = if n.kind() == K::CatchKeyword {
                p.parent()
            } else {
                Some(parent)
            };
            if let Some(owner) = owner {
                if let Some(d) = v.node(owner)?.data_source().as_try_statement() {
                    nodes.extend(s.nav().find_child_of_kind(owner, K::TryKeyword)?);
                    if d.catch_clause().is_some() {
                        nodes.extend(s.nav().find_child_of_kind(owner, K::CatchKeyword)?);
                    }
                    if d.finally_block().is_some() {
                        nodes.extend(s.nav().find_child_of_kind(owner, K::FinallyKeyword)?);
                    }
                }
            }
        }
        Some(K::SwitchKeyword) if p.kind() == K::SwitchStatement => nodes = loop_switch(s, parent)?,
        Some(K::CaseKeyword | K::DefaultKeyword)
            if matches!(p.kind().known(), Some(K::CaseClause | K::DefaultClause)) =>
        {
            if let Some(block) = p.parent() {
                if let Some(owner) = v.node(block)?.parent() {
                    nodes = loop_switch(s, owner)?;
                }
            }
        }
        Some(K::BreakKeyword | K::ContinueKeyword)
            if matches!(
                p.kind().known(),
                Some(K::BreakStatement | K::ContinueStatement)
            ) =>
        {
            if let Some(owner) = jump_owner(s, parent)? {
                nodes = loop_switch(s, owner)?;
            }
        }
        Some(K::ForKeyword | K::WhileKeyword | K::DoKeyword)
            if p.kind().known().is_some_and(is_loop) =>
        {
            nodes = loop_switch(s, parent)?;
        }
        Some(K::ConstructorKeyword | K::GetKeyword | K::SetKeyword) => {
            let constructor = n.kind() == K::ConstructorKeyword;
            let valid = |k: K| {
                if constructor {
                    k == K::Constructor
                } else {
                    matches!(k, K::GetAccessor | K::SetAccessor)
                }
            };
            if p.kind().known().is_some_and(valid) {
                if let Some(symbol) = c.bound_symbol_of_node(parent)? {
                    for decl in declarations(c, symbol)? {
                        if v.node(decl)?.kind().known().is_some_and(valid) {
                            for child in s.token_children(decl)? {
                                if matches!(
                                    v.node(child)?.kind().known(),
                                    Some(K::ConstructorKeyword | K::GetKeyword | K::SetKeyword)
                                ) {
                                    nodes.push(child);
                                    break;
                                }
                            }
                        }
                    }
                }
            }
        }
        Some(K::AsyncKeyword | K::AwaitKeyword | K::YieldKeyword) => {
            let yield_ = n.kind() == K::YieldKeyword;
            if let Some(fun) = ancestor(v, Some(parent), |id| {
                Ok(ast::is_function_like(Some(&v.node(id)?)))
            })? {
                if !yield_ {
                    for modifier in list(v, v.node(fun)?.modifiers())? {
                        if v.node(modifier)?.kind() == K::AsyncKeyword {
                            nodes.push(modifier);
                        }
                    }
                }
                for child in s.children(fun)? {
                    let expressions = within_function(
                        s,
                        child,
                        if yield_ {
                            K::YieldExpression
                        } else {
                            K::AwaitExpression
                        },
                        true,
                    )?;
                    add_tokens(
                        s,
                        expressions,
                        if yield_ {
                            K::YieldKeyword
                        } else {
                            K::AwaitKeyword
                        },
                        &mut nodes,
                    )?;
                }
            }
        }
        Some(K::InKeyword | K::OutKeyword) => {}
        Some(kind)
            if ast::is_modifier(&n)
                && (tsr_ast::is_declaration(&p) || p.kind() == K::VariableStatement) =>
        {
            nodes = modifier_nodes(s, parent, kind)?;
        }
        _ => {}
    }
    let mut out = Vec::new();
    let mut i = 0;
    while i < nodes.len() {
        let node = nodes[i];
        let mut end = v.node(node)?.end();
        if combine_if && v.node(node)?.kind() == K::ElseKeyword && i + 1 < nodes.len() {
            let next = nodes[i + 1];
            let start = s.start(next)?.max(i64::from(v.node(next)?.pos()));
            if (i64::from(end)..start).all(|p| {
                tsr_scanner::is_white_space_single_line(i32::from(
                    s.file.text().as_bytes()[p as usize],
                ))
            }) {
                end = v.node(next)?.end();
                i += 1;
            }
        }
        out.push(TextRange::new(s.start(node)?, i64::from(end)));
        i += 1;
    }
    Ok(out)
}
