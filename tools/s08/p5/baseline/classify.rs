//! Syntax predicates used by the native type/symbol baseline traversal.
use super::Result;
use ts_arena::NodeId;
use ts_ast::{AstView, SyntaxKind as K};

pub fn declaration_name(view: AstView<'_>, id: NodeId) -> Result<bool> {
    let node = view.node(id)?;
    if node.kind() == K::SourceFile || ts_ast::utilities::is_binding_pattern(&node) {
        return Ok(false);
    }
    let Some(parent) = node.parent() else {
        return Ok(false);
    };
    let parent = view.node(parent)?;
    Ok(ts_ast::is_declaration(&parent) && parent.name() == Some(id))
}

// Harness use of tsc/internal/ast/utilities.go:GetMeaningFromDeclaration.
// Only the Value bit is consumed; other semantic meaning bits are not symbols.
pub fn declaration_has_value(view: AstView<'_>, id: NodeId) -> Result<bool> {
    let node = view.node(id)?;
    Ok(match node.kind().known() {
        Some(
            K::TypeParameter
            | K::InterfaceDeclaration
            | K::TypeAliasDeclaration
            | K::JSTypeAliasDeclaration
            | K::TypeLiteral,
        ) => false,
        Some(K::ModuleDeclaration) => {
            ts_ast::is_ambient_module(view, id)?
                || ts_ast::get_module_instance_state(view, id)?
                    == ts_ast::ModuleInstanceState::Instantiated
        }
        _ => true,
    })
}

pub fn heritage_name(view: AstView<'_>, mut id: NodeId) -> Result<bool> {
    while let Some(parent) = view.node(id)?.parent() {
        if view.node(parent)?.kind() != K::QualifiedName {
            break;
        }
        id = parent;
    }
    let Some(parent) = view.node(id)?.parent() else {
        return Ok(false);
    };
    let read = view.node(parent)?;
    let Some(data) = read.data_source().as_type_reference_node() else {
        return Ok(false);
    };
    Ok(data.type_name() == Some(id)
        && read
            .parent()
            .map(|p| view.node(p).map(|p| p.kind() == K::HeritageClause))
            .transpose()?
            .unwrap_or(false))
}

pub fn class_extends(view: AstView<'_>, id: NodeId) -> Result<bool> {
    let node = view.node(id)?;
    if node.kind() != K::ExpressionWithTypeArguments {
        return Ok(false);
    }
    let Some(parent) = node.parent() else {
        return Ok(false);
    };
    let heritage = view.node(parent)?;
    let Some(data) = heritage.data_source().as_heritage_clause() else {
        return Ok(false);
    };
    if data.token() != K::ExtendsKeyword {
        return Ok(false);
    }
    Ok(heritage
        .parent()
        .map(|p| {
            view.node(p).map(|p| {
                matches!(
                    p.kind().known(),
                    Some(K::ClassDeclaration | K::ClassExpression)
                )
            })
        })
        .transpose()?
        .unwrap_or(false))
}

pub fn import_or_export_name(view: AstView<'_>, id: NodeId, parent: NodeId) -> Result<bool> {
    let parent = view.node(parent)?;
    Ok(match parent.kind().known() {
        Some(K::ImportSpecifier) => {
            parent.name() == Some(id)
                || parent
                    .data_source()
                    .as_import_specifier()
                    .ok_or("import specifier payload")?
                    .property_name()
                    == Some(id)
        }
        Some(K::ExportSpecifier) => {
            parent.name() == Some(id)
                || parent
                    .data_source()
                    .as_export_specifier()
                    .ok_or("export specifier payload")?
                    .property_name()
                    == Some(id)
        }
        Some(K::ImportClause | K::ImportEqualsDeclaration) => parent.name() == Some(id),
        Some(K::ExportAssignment) => parent.expression() == Some(id),
        _ => false,
    })
}

pub fn intrinsic_jsx(view: AstView<'_>, id: NodeId, parent: NodeId, text: &[u8]) -> Result<bool> {
    let parent = view.node(parent)?;
    let tag = match parent.kind().known() {
        Some(K::JsxOpeningElement) => parent
            .data_source()
            .as_jsx_opening_element()
            .ok_or("JSX opening payload")?
            .tag_name(),
        Some(K::JsxClosingElement) => parent
            .data_source()
            .as_jsx_closing_element()
            .ok_or("JSX closing payload")?
            .tag_name(),
        Some(K::JsxSelfClosingElement) => parent
            .data_source()
            .as_jsx_self_closing_element()
            .ok_or("JSX self-closing payload")?
            .tag_name(),
        _ => return Ok(false),
    };
    Ok(tag == Some(id) && ts_scanner::is_intrinsic_jsx_name(text))
}
