//! Syntactic declaration selection for source-definition navigation. These
//! targets may live outside the checked program; each retains its bound owner.
use crate::{documentation::list, reference_helpers, syntax::Syntax, Result};
use std::{collections::HashSet, sync::Arc};
use tsr_ast::{utilities as ast, AstView, NodeId, SyntaxKind as K};
use tsr_compiler::ProgramFile;

#[derive(Clone)]
pub(crate) struct Declaration {
    pub file: Arc<ProgramFile>,
    pub node: NodeId,
}
impl Declaration {
    pub fn view(&self) -> AstView<'_> {
        self.file.bound().view().ast()
    }
}

// port: tsc/internal/ls/sourcedefinition.go:isDefaultImportName
pub(crate) fn default_import(view: AstView<'_>, node: NodeId) -> Result<bool> {
    let Some(parent) = view.node(node)?.parent() else {
        return Ok(false);
    };
    let read = view.node(parent)?;
    Ok(read.kind() == K::ImportClause
        && read.name() == Some(node)
        && read
            .parent()
            .is_some_and(|p| view.node(p).is_ok_and(|n| n.kind() == K::ImportDeclaration)))
}
// port: tsc/internal/ls/sourcedefinition.go:getSourceDefinitionEntryNode
pub(crate) fn entry(file: Arc<ProgramFile>) -> Result<Declaration> {
    let view = file.bound().view().ast();
    let node = list(view, view.node(file.source())?.statement_list())?
        .first()
        .copied()
        .unwrap_or(file.source());
    Ok(Declaration { file, node })
}
// port: tsc/internal/ls/sourcedefinition.go:findContainingModuleSpecifier
pub(crate) fn module_specifier(view: AstView<'_>, node: NodeId) -> Result<Option<NodeId>> {
    let mut current = Some(node);
    while let Some(id) = current {
        let read = view.node(id)?;
        if ast::is_any_import_or_re_export(&read)
            || tsr_ast::utilities_middle::is_require_call(view, &read, true)?
            || tsr_ast::utilities_positions::is_import_call(view, id)?
        {
            if let Some(spec) = tsr_ast::utilities_modules::get_external_module_name(view, id)? {
                if ast::is_string_literal_like(&view.node(spec)?) {
                    return Ok(Some(spec));
                }
            }
        }
        current = read.parent();
    }
    Ok(None)
}
// port: tsc/internal/ls/sourcedefinition.go:getCandidateSourceDeclarationNames
pub(crate) fn names(
    original: AstView<'_>,
    node: NodeId,
    declaration: Option<&Declaration>,
) -> Result<Vec<Vec<u8>>> {
    let mut names = Vec::new();
    if let Some(d) = declaration {
        let view = d.view();
        let read = view.node(d.node)?;
        if let Some(name) = tsr_ast::get_name_of_declaration(view, Some(d.node))? {
            let name = tsr_ast::utilities_targets::get_text_of_property_name(view, name)?;
            if !name.is_empty() {
                names.push(name);
            }
        }
        if read.kind() == K::ExportAssignment
            || matches!(
                read.kind().known(),
                Some(K::FunctionDeclaration | K::ClassDeclaration)
            ) && (ast::has_syntactic_modifier(view, d.node, tsr_ast::modifier_flags::EXPORT)?
                && ast::has_syntactic_modifier(view, d.node, tsr_ast::modifier_flags::DEFAULT)?)
        {
            names.push(b"default".to_vec());
        }
        if matches!(
            read.kind().known(),
            Some(K::ImportSpecifier | K::ExportSpecifier)
        ) {
            if let Some(name) = read.property_name() {
                names.push(view.node_text(name)?.as_bytes().to_vec());
            }
        }
    }
    let read = original.node(node)?;
    if matches!(
        read.kind().known(),
        Some(K::Identifier | K::PrivateIdentifier)
    ) {
        names.push(original.node_text(node)?.as_bytes().to_vec());
    }
    if default_import(original, node)? {
        names.push(b"default".to_vec());
    }
    if let Some(parent) = read.parent() {
        let p = original.node(parent)?;
        if matches!(
            p.kind().known(),
            Some(K::ImportSpecifier | K::ExportSpecifier)
        ) {
            if let Some(name) = p.property_name() {
                names.push(original.node_text(name)?.as_bytes().to_vec());
            }
        }
    }
    Ok(names)
}
// port: tsc/internal/ls/sourcedefinition.go:isConcreteSourceDeclaration
pub(crate) fn concrete(d: &Declaration) -> Result<bool> {
    let view = d.view();
    let n = view.node(d.node)?;
    if !tsr_ast::is_declaration(&n) || n.kind() == K::ExportAssignment {
        return Ok(false);
    }
    if matches!(
        n.kind().known(),
        Some(K::BinaryExpression | K::CallExpression)
    ) && tsr_ast::get_assignment_declaration_kind(view, d.node)?
        != tsr_ast::JSDeclarationKind::None
    {
        return Ok(false);
    }
    Ok(!matches!(
        n.kind().known(),
        Some(
            K::Parameter
                | K::TypeParameter
                | K::BindingElement
                | K::ImportClause
                | K::ImportSpecifier
                | K::NamespaceImport
                | K::ExportSpecifier
                | K::PropertyAccessExpression
                | K::ElementAccessExpression
        )
    ))
}
pub(crate) fn has_concrete(declarations: &[Declaration]) -> Result<bool> {
    for d in declarations {
        if concrete(d)? {
            return Ok(true);
        }
    }
    Ok(false)
}
// port: tsc/internal/ls/sourcedefinition.go:filterPreferredSourceDeclarations
// port: tsc/internal/ls/sourcedefinition.go:getPropertyLikeSourceDeclarations
pub(crate) fn preferred(
    view: AstView<'_>,
    node: NodeId,
    declarations: Vec<Declaration>,
) -> Result<Vec<Declaration>> {
    if declarations.len() <= 1 {
        return Ok(declarations);
    }
    let read = view.node(node)?;
    let property = if let Some(parent) = read.parent() {
        let p = view.node(parent)?;
        ast::is_access_expression(&p) && p.name() == Some(node)
    } else {
        false
    };
    if property {
        let mut out = Vec::new();
        for d in &declarations {
            if matches!(
                d.view().node(d.node)?.kind().known(),
                Some(
                    K::PropertyAssignment
                        | K::ShorthandPropertyAssignment
                        | K::PropertyDeclaration
                        | K::PropertySignature
                        | K::MethodDeclaration
                        | K::MethodSignature
                        | K::GetAccessor
                        | K::SetAccessor
                        | K::EnumMember
                )
            ) {
                out.push(d.clone());
            }
        }
        if !out.is_empty() {
            return Ok(out);
        }
    }
    let mut out = Vec::new();
    for d in &declarations {
        if concrete(d)? {
            out.push(d.clone());
        }
    }
    Ok(if out.is_empty() { declarations } else { out })
}
// port: tsc/internal/ls/sourcedefinition.go:uniqueDeclarationNodes
pub(crate) fn unique(declarations: Vec<Declaration>) -> Result<Vec<Declaration>> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for d in declarations {
        let view = d.view();
        let n = view.node(d.node)?;
        if seen.insert((
            view.source_file(d.file.source())?.file_name().to_vec(),
            n.pos(),
            n.end(),
        )) {
            out.push(d);
        }
    }
    Ok(out)
}
// port: tsc/internal/ls/sourcedefinition.go:findDeclarationNodesByName
pub(crate) fn find(
    file: &Arc<ProgramFile>,
    names: &[Vec<u8>],
    cancellation: &tsr_core::CancellationToken,
) -> Result<Vec<Declaration>> {
    let wanted: HashSet<_> = names
        .iter()
        .filter(|n| !n.is_empty() && n.as_slice() != b"default")
        .map(Vec::as_slice)
        .collect();
    let default = names.iter().any(|n| n == b"default");
    let view = file.bound().view().ast();
    let syntax = Syntax::new(view, file.source())?;
    let mut stack = syntax.children(file.source())?;
    stack.reverse();
    let mut found = Vec::new();
    let mut min_depth = usize::MAX;
    while let Some(node) = stack.pop() {
        if cancellation.is_canceled() {
            return Err(crate::Error::Canceled);
        }
        let n = view.node(node)?;
        let mut matched = false;
        if let Some(name) = tsr_ast::get_name_of_declaration(view, Some(node))? {
            let text = tsr_ast::utilities_targets::get_text_of_property_name(view, name)?;
            matched = !text.is_empty() && wanted.contains(text.as_slice());
        }
        matched |= default
            && (n.kind() == K::ExportAssignment
                || matches!(
                    n.kind().known(),
                    Some(K::ClassDeclaration | K::FunctionDeclaration)
                ) && (ast::has_syntactic_modifier(
                    view,
                    node,
                    tsr_ast::modifier_flags::EXPORT,
                )? && ast::has_syntactic_modifier(
                    view,
                    node,
                    tsr_ast::modifier_flags::DEFAULT,
                )?));
        if matched {
            let mut depth = 0;
            let mut current = Some(node);
            while let Some(id) = current {
                current = reference_helpers::container(view, id)?;
                depth += 1;
            }
            min_depth = min_depth.min(depth);
            found.push((node, depth));
        }
        stack.extend(syntax.children(node)?.into_iter().rev());
    }
    unique(
        found
            .into_iter()
            .filter(|(_, d)| *d == min_depth)
            .map(|(node, _)| Declaration {
                file: file.clone(),
                node,
            })
            .collect(),
    )
}
// port: tsc/internal/ls/sourcedefinition.go:findClosestDeclarationNode
pub(crate) fn closest(file: Arc<ProgramFile>, position: i64) -> Result<Declaration> {
    let view = file.bound().view().ast();
    let mut syntax = Syntax::new(view, file.source())?;
    let mut current = Some(syntax.nav().get_touching_property_name(position)?);
    while let Some(node) = current {
        let n = view.node(node)?;
        if tsr_ast::is_declaration(&n) || n.kind() == K::ExportAssignment {
            return Ok(Declaration { file, node });
        }
        current = n.parent();
    }
    entry(file)
}
