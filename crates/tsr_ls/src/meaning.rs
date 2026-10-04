//! Shared location adjustment and value/type/namespace meaning.
use crate::{documentation::list, syntax::Syntax, Result};
use tsr_ast::utilities_positions::semantic_meaning as M;
use tsr_ast::{
    utilities as ast, utilities_middle as middle, utilities_positions as pos, AstView, NodeId,
    SyntaxKind as K,
};

// port: tsc/internal/ls/utilities.go:getAdjustedLocationForImportDeclaration
fn adjusted_import(view: AstView<'_>, node: NodeId, rename: bool) -> Result<Option<NodeId>> {
    let read = view.node(node)?;
    let d = read.data_source().as_import_declaration().unwrap();
    if let Some(clause) = d.import_clause() {
        let clause = view.node(clause)?;
        let data = clause.data_source().as_import_clause().unwrap();
        if clause.name().is_some() {
            return Ok(if data.named_bindings().is_none() {
                clause.name()
            } else {
                None
            });
        }
        if let Some(bindings) = data.named_bindings() {
            let read = view.node(bindings)?;
            match read.kind().known() {
                Some(K::NamedImports) => {
                    let elements: Vec<_> = view
                        .node_slice(read.elements(view)?)?
                        .iter()
                        .flatten()
                        .collect();
                    return Ok(if elements.len() == 1 {
                        view.node(elements[0])?.name()
                    } else {
                        None
                    });
                }
                Some(K::NamespaceImport) => return Ok(read.name()),
                _ => {}
            }
        }
    }
    Ok((!rename).then(|| d.module_specifier()).flatten())
}
// port: tsc/internal/ls/utilities.go:getAdjustedLocationForExportDeclaration
fn adjusted_export(view: AstView<'_>, node: NodeId, rename: bool) -> Result<Option<NodeId>> {
    let read = view.node(node)?;
    let d = read.data_source().as_export_declaration().unwrap();
    if let Some(clause) = d.export_clause() {
        let read = view.node(clause)?;
        match read.kind().known() {
            Some(K::NamedExports) => {
                let elements: Vec<_> = view
                    .node_slice(read.elements(view)?)?
                    .iter()
                    .flatten()
                    .collect();
                return Ok(if elements.len() == 1 {
                    view.node(elements[0])?.name()
                } else {
                    None
                });
            }
            Some(K::NamespaceExport) => return Ok(read.name()),
            _ => {}
        }
    }
    Ok((!rename).then(|| d.module_specifier()).flatten())
}
// port: tsc/internal/ls/utilities.go:getAdjustedLocation
pub(crate) fn adjusted_location(view: AstView<'_>, node: NodeId, rename: bool) -> Result<NodeId> {
    let read = view.node(node)?;
    let Some(parent_id) = read.parent() else {
        return Ok(node);
    };
    let parent = view.node(parent_id)?;
    let kind = read.kind();
    let pk = parent.kind();
    let modifiers = list(view, parent.modifiers())?;
    let is_modifier = ast::is_modifier(&read)
        && (rename || kind != K::DefaultKeyword)
        && modifiers.contains(&node)
        || matches!(
            (kind.known(), pk.known()),
            (Some(K::ClassKeyword), Some(K::ClassDeclaration))
                | (Some(K::FunctionKeyword), Some(K::FunctionDeclaration))
                | (Some(K::InterfaceKeyword), Some(K::InterfaceDeclaration))
                | (Some(K::EnumKeyword), Some(K::EnumDeclaration))
                | (Some(K::TypeKeyword), Some(K::TypeAliasDeclaration))
                | (
                    Some(K::NamespaceKeyword | K::ModuleKeyword),
                    Some(K::ModuleDeclaration)
                )
                | (Some(K::ImportKeyword), Some(K::ImportEqualsDeclaration))
                | (Some(K::GetKeyword), Some(K::GetAccessor))
                | (Some(K::SetKeyword), Some(K::SetAccessor))
        );
    if is_modifier {
        if let Some(name) = parent.name() {
            return Ok(name);
        }
        if !rename {
            // Preserve the pin's default-modifier predicate (it tests the declaration).
            if pk == K::Constructor {
                return Ok(parent_id);
            }
            if pk == K::ClassExpression || pk == K::FunctionExpression {
                let source = ast::get_source_file_of_node(view, Some(node))?
                    .ok_or(tsr_arena::Error::InvalidGraph)?;
                let mut syntax = Syntax::new(view, source)?;
                let token = if pk == K::ClassExpression {
                    K::ClassKeyword
                } else {
                    K::FunctionKeyword
                };
                if let Some(child) = syntax.nav().find_child_of_kind(parent_id, token)? {
                    return Ok(child);
                }
            }
        }
    }
    if matches!(
        kind.known(),
        Some(K::VarKeyword | K::ConstKeyword | K::LetKeyword)
    ) && pk == K::VariableDeclarationList
    {
        let decls = list(
            view,
            parent
                .data_source()
                .as_variable_declaration_list()
                .unwrap()
                .declarations(),
        )?;
        if decls.len() == 1 {
            if let Some(name) = view.node(decls[0])?.name() {
                if view.node(name)?.kind() == K::Identifier {
                    return Ok(name);
                }
            }
        }
    }
    if kind == K::TypeKeyword {
        if pk == K::ImportClause && parent.is_type_only() {
            if let Some(import) = parent.parent() {
                if let Some(n) = adjusted_import(view, import, rename)? {
                    return Ok(n);
                }
            }
        }
        if pk == K::ExportDeclaration
            && parent
                .data_source()
                .as_export_declaration()
                .unwrap()
                .is_type_only()
        {
            if let Some(n) = adjusted_export(view, parent_id, rename)? {
                return Ok(n);
            }
        }
    }
    if kind == K::AsKeyword {
        if matches!(pk.known(), Some(K::ImportSpecifier | K::ExportSpecifier))
            && parent.property_name().is_some()
            || matches!(pk.known(), Some(K::NamespaceImport | K::NamespaceExport))
        {
            if let Some(n) = parent.name() {
                return Ok(n);
            }
        }
        if pk == K::ExportDeclaration {
            if let Some(c) = parent
                .data_source()
                .as_export_declaration()
                .unwrap()
                .export_clause()
            {
                if view.node(c)?.kind() == K::NamespaceExport {
                    if let Some(n) = view.node(c)?.name() {
                        return Ok(n);
                    }
                }
            }
        }
    }
    if kind == K::ImportKeyword && pk == K::ImportDeclaration {
        if let Some(n) = adjusted_import(view, parent_id, rename)? {
            return Ok(n);
        }
    }
    if kind == K::ExportKeyword {
        if pk == K::ExportDeclaration {
            if let Some(n) = adjusted_export(view, parent_id, rename)? {
                return Ok(n);
            }
        }
        if pk == K::ExportAssignment {
            if let Some(e) = parent.expression() {
                return Ok(ast::skip_outer_expressions(
                    view,
                    e,
                    tsr_ast::evaluator::outer_expression_kinds::ALL,
                )?);
            }
        }
    }
    if kind == K::RequireKeyword && pk == K::ExternalModuleReference {
        if let Some(e) = parent.expression() {
            return Ok(e);
        }
    }
    if kind == K::FromKeyword {
        if let Some(n) = parent
            .data_source()
            .as_import_declaration()
            .and_then(|d| d.module_specifier())
            .or_else(|| {
                parent
                    .data_source()
                    .as_export_declaration()
                    .and_then(|d| d.module_specifier())
            })
        {
            return Ok(n);
        }
    }
    if matches!(kind.known(), Some(K::ExtendsKeyword | K::ImplementsKeyword))
        && pk == K::HeritageClause
    {
        let d = parent.data_source().as_heritage_clause().unwrap();
        if d.token() == kind {
            let types = list(view, d.types())?;
            if types.len() == 1 {
                if let Some(n) =
                    tsr_ast::utilities_class::get_heritage_clause_element_name(view, types[0])?
                {
                    return Ok(n);
                }
            }
        }
    }
    let reference_name = |node: Option<NodeId>| -> Result<Option<NodeId>> {
        Ok(node.map(|n| view.node(n)).transpose()?.and_then(|n| {
            n.data_source()
                .as_type_reference_node()
                .and_then(|d| d.type_name())
        }))
    };
    if kind == K::ExtendsKeyword {
        if let Some(n) = reference_name(
            parent
                .data_source()
                .as_type_parameter_declaration()
                .and_then(|d| d.constraint())
                .or_else(|| {
                    parent
                        .data_source()
                        .as_conditional_type_node()
                        .and_then(|d| d.extends_type())
                }),
        )? {
            return Ok(n);
        }
    }
    if kind == K::InferKeyword {
        if let Some(tp) = parent
            .data_source()
            .as_infer_type_node()
            .and_then(|d| d.type_parameter())
        {
            if let Some(n) = view.node(tp)?.name() {
                return Ok(n);
            }
        }
    }
    if kind == K::InKeyword
        && pk == K::TypeParameter
        && parent
            .parent()
            .is_some_and(|p| view.node(p).is_ok_and(|n| n.kind() == K::MappedType))
    {
        if let Some(n) = parent.name() {
            return Ok(n);
        }
    }
    if let Some(d) = parent.data_source().as_type_operator_node() {
        if kind == K::KeyOfKeyword && d.operator() == kind {
            if let Some(n) = reference_name(d.r#type())? {
                return Ok(n);
            }
        }
        if kind == K::ReadonlyKeyword && d.operator() == kind {
            if let Some(ty) = d.r#type() {
                if let Some(array) = view.node(ty)?.data_source().as_array_type_node() {
                    if let Some(n) = reference_name(array.element_type())? {
                        return Ok(n);
                    }
                }
            }
        }
    }
    if !rename {
        if matches!(
            (kind.known(), pk.known()),
            (Some(K::NewKeyword), Some(K::NewExpression))
                | (Some(K::VoidKeyword), Some(K::VoidExpression))
                | (Some(K::TypeOfKeyword), Some(K::TypeOfExpression))
                | (Some(K::AwaitKeyword), Some(K::AwaitExpression))
                | (Some(K::YieldKeyword), Some(K::YieldExpression))
                | (Some(K::DeleteKeyword), Some(K::DeleteExpression))
                | (Some(K::InKeyword), Some(K::ForInStatement))
                | (Some(K::OfKeyword), Some(K::ForOfStatement))
        ) {
            if let Some(e) = parent.expression() {
                return Ok(ast::skip_outer_expressions(
                    view,
                    e,
                    tsr_ast::evaluator::outer_expression_kinds::ALL,
                )?);
            }
        }
        if matches!(kind.known(), Some(K::InKeyword | K::InstanceOfKeyword)) {
            if let Some(d) = parent.data_source().as_binary_expression() {
                if d.operator_token() == Some(node) {
                    if let Some(e) = d.right() {
                        return Ok(ast::skip_outer_expressions(
                            view,
                            e,
                            tsr_ast::evaluator::outer_expression_kinds::ALL,
                        )?);
                    }
                }
            }
        }
        if kind == K::AsKeyword && pk == K::AsExpression {
            if let Some(n) = reference_name(parent.type_node())? {
                return Ok(n);
            }
        }
    }
    Ok(node)
}
// port: tsc/internal/ls/utilities.go:getMeaningFromLocation
pub(crate) fn meaning(
    view: AstView<'_>,
    node: NodeId,
    checker: &tsr_checker::Operation<'_>,
) -> Result<i32> {
    let node = tsr_ast::utilities_containers::get_reparsed_node_for_node(view, Some(node))?
        .unwrap_or(node);
    let node = adjusted_location(view, node, false)?;
    let read = view.node(node)?;
    let Some(parent_id) = read.parent() else {
        return Ok(M::VALUE);
    };
    let parent = view.node(parent_id)?;
    if matches!(
        parent.kind().known(),
        Some(
            K::ExportAssignment
                | K::ExportSpecifier
                | K::ExternalModuleReference
                | K::ImportSpecifier
                | K::ImportClause
        )
    ) || parent.kind() == K::ImportEqualsDeclaration && parent.name() == Some(node)
    {
        return Ok(M::ALL);
    }
    let mut root = node;
    while let Some(p) = view
        .node(root)?
        .parent()
        .filter(|p| view.node(*p).is_ok_and(|n| n.kind() == K::QualifiedName))
    {
        root = p;
    }
    if let Some(p) = view.node(root)?.parent() {
        if view
            .node(p)?
            .data_source()
            .as_import_equals_declaration()
            .is_some_and(|d| d.module_reference() == Some(root))
        {
            let name = if read.kind() == K::QualifiedName {
                Some(node)
            } else if parent
                .data_source()
                .as_qualified_name()
                .is_some_and(|d| d.right() == Some(node))
            {
                Some(parent_id)
            } else {
                None
            };
            return Ok(
                if name.is_some_and(|n| view.node(n).is_ok_and(|n| n.parent() == Some(p))) {
                    M::ALL
                } else {
                    M::NAMESPACE
                },
            );
        }
    }
    if pos::is_declaration_name(view, node)? {
        return Ok(pos::get_meaning_from_declaration(view, parent_id)?);
    }
    if ast::is_entity_name(&read)
        && tsr_ast::utilities_tail::is_js_doc_name_reference_context(view, node)?
    {
        return Ok(M::ALL);
    }
    let effective = if middle::is_right_side_of_qualified_name_or_property_access(view, node)? {
        parent_id
    } else {
        node
    };
    let effective_read = view.node(effective)?;
    if effective_read.kind() == K::ThisType
        || effective_read.kind() == K::ThisKeyword && !checker.is_expression_node(effective)?
    {
        return Ok(M::TYPE);
    }
    if let Some(p) = effective_read.parent() {
        let p_read = view.node(p)?;
        if p_read.kind() == K::TypeReference
            || p_read
                .data_source()
                .as_import_type_node()
                .is_some_and(|d| !d.is_type_of())
            || p_read.kind() == K::ExpressionWithTypeArguments && checker.is_part_of_type_node(p)?
        {
            return Ok(M::TYPE);
        }
    }
    if root != node
        && view
            .node(root)?
            .data_source()
            .as_qualified_name()
            .is_some_and(|d| d.right() != Some(node))
        && view
            .node(root)?
            .parent()
            .is_some_and(|p| view.node(p).is_ok_and(|n| n.kind() == K::TypeReference))
    {
        return Ok(M::NAMESPACE);
    }
    let mut root = node;
    while let Some(p) = view.node(root)?.parent().filter(|p| {
        view.node(*p)
            .is_ok_and(|n| n.kind() == K::PropertyAccessExpression)
    }) {
        root = p;
    }
    if root != node && view.node(root)?.name() != Some(node) {
        if let Some(expr) = view.node(root)?.parent() {
            if view.node(expr)?.kind() == K::ExpressionWithTypeArguments {
                if let Some(heritage) = view.node(expr)?.parent() {
                    let h = view.node(heritage)?;
                    if let Some(d) = h.data_source().as_heritage_clause() {
                        if let Some(decl) = h.parent() {
                            if view.node(decl)?.kind() == K::ClassDeclaration
                                && d.token() == K::ImplementsKeyword
                                || view.node(decl)?.kind() == K::InterfaceDeclaration
                                    && d.token() == K::ExtendsKeyword
                            {
                                return Ok(M::NAMESPACE);
                            }
                        }
                    }
                }
            }
        }
    }
    Ok(if parent.kind() == K::TypeParameter {
        M::TYPE
    } else if parent.kind() == K::LiteralType {
        M::VALUE | M::TYPE
    } else {
        M::VALUE
    })
}
