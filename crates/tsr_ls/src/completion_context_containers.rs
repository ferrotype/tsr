//! Object-type and constructor completion containers selected from syntax.
use super::{ast, class_keyword, keyword, middle, positions, Syntax, K};
use crate::Result;
use tsr_ast::NodeId;

// port: tsc/internal/ls/completions.go:isFromObjectTypeDeclaration
pub(super) fn from_object_type(syntax: &Syntax<'_>, node: NodeId) -> Result<bool> {
    let Some(parent) = syntax.view.node(node)?.parent() else {
        return Ok(false);
    };
    let parent = syntax.view.node(parent)?;
    Ok(tsr_ast::utilities_class::is_class_or_type_element(&parent)
        && parent
            .parent()
            .map(|id| {
                syntax
                    .view
                    .node(id)
                    .map(|node| middle::is_object_type_declaration(&node))
            })
            .transpose()?
            == Some(true))
}

// port: tsc/internal/ls/completions.go:isConstructorParameterCompletion
pub(super) fn constructor_parameter(syntax: &Syntax<'_>, node: NodeId) -> Result<bool> {
    let read = syntax.view.node(node)?;
    let Some(parent) = read.parent() else {
        return Ok(false);
    };
    let parent = syntax.view.node(parent)?;
    Ok(parent.kind() == K::Parameter
        && parent
            .parent()
            .map(|id| syntax.view.node(id).map(|node| node.kind()))
            .transpose()?
            == Some(K::Constructor.into())
        && (middle::is_parameter_property_modifier(read.kind())
            || positions::is_declaration_name(syntax.view, node)?))
}

// port: tsc/internal/ls/completions.go:tryGetObjectTypeDeclarationCompletionContainer
pub(super) fn object_type(
    syntax: &mut Syntax<'_>,
    token: Option<NodeId>,
    location: NodeId,
    position: i64,
) -> Result<Option<NodeId>> {
    let view = syntax.view;
    let loc = view.node(location)?;
    match loc.kind().known() {
        Some(K::SyntaxList) => {
            return loc
                .parent()
                .map(|id| {
                    view.node(id)
                        .map(|node| middle::is_object_type_declaration(&node).then_some(id))
                })
                .transpose()
                .map(Option::flatten)
                .map_err(Into::into);
        }
        Some(K::EndOfFile) => {
            if let Some(parent) = loc.parent() {
                if let Some(last) = view
                    .node_slice(view.node(parent)?.statements(view)?)?
                    .iter()
                    .flatten()
                    .last()
                {
                    if middle::is_object_type_declaration(&view.node(last)?)
                        && syntax
                            .nav()
                            .find_child_of_kind(last, K::CloseBraceToken)?
                            .is_none()
                    {
                        return Ok(Some(last));
                    }
                }
            }
        }
        Some(K::PrivateIdentifier) => {
            if loc
                .parent()
                .map(|id| view.node(id).map(|node| node.kind()))
                .transpose()?
                == Some(K::PropertyDeclaration.into())
            {
                return Ok(ast::find_ancestor(view, Some(location), |node| {
                    ast::is_class_like(node)
                })?);
            }
        }
        Some(K::Identifier) => {
            if keyword(syntax, location)? != K::Identifier {
                return Ok(None);
            }
            if let Some(parent) = loc.parent() {
                let parent = view.node(parent)?;
                if parent.kind() == K::PropertyDeclaration && parent.initializer() == Some(location)
                {
                    return Ok(None);
                }
            }
            if from_object_type(syntax, location)? {
                return Ok(ast::find_ancestor(view, Some(location), |node| {
                    middle::is_object_type_declaration(node)
                })?);
            }
        }
        _ => {}
    }
    let Some(token) = token else { return Ok(None) };
    let read = view.node(token)?;
    let parent = read.parent().map(|id| view.node(id)).transpose()?;
    if loc.kind() == K::ConstructorKeyword
        || (read.kind() == K::Identifier
            && parent
                .as_ref()
                .is_some_and(|node| node.kind() == K::PropertyDeclaration)
            && ast::is_class_like(&loc))
    {
        return Ok(ast::find_ancestor(view, Some(token), |node| {
            ast::is_class_like(node)
        })?);
    }
    Ok(match read.kind().known() {
        Some(K::EqualsToken) => None,
        Some(K::SemicolonToken | K::CloseBraceToken) => {
            if from_object_type(syntax, location)?
                && view.node(loc.parent().unwrap())?.name() == Some(location)
            {
                view.node(loc.parent().unwrap())?.parent()
            } else {
                middle::is_object_type_declaration(&loc).then_some(location)
            }
        }
        Some(K::OpenBraceToken | K::CommaToken) => parent
            .filter(middle::is_object_type_declaration)
            .and(read.parent()),
        _ => {
            if !middle::is_object_type_declaration(&loc) {
                return Ok(None);
            }
            if !syntax.same_line(i64::from(read.end()), position) {
                return Ok(Some(location));
            }
            let grandparent = parent.and_then(|node| node.parent());
            let class = grandparent
                .map(|id| view.node(id).map(|node| ast::is_class_like(&node)))
                .transpose()?
                .unwrap_or(false);
            let kind = keyword(syntax, token)?;
            if (if class {
                class_keyword(kind)
            } else {
                kind == K::ReadonlyKeyword
            }) || read.kind() == K::AsteriskToken
            {
                grandparent
            } else {
                None
            }
        }
    })
}
