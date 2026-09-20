//! Shared declaration-name and modifier rules. JS assignment classification stays
//! at the explicit checked boundary; ordinary graph traversal stays in its view.
use crate::{
    syntax_helpers::{self, Read, View},
    SyntaxKind as K,
};

pub(crate) enum Name<Id> {
    Resolved(Option<Id>),
    Assignment(Id),
}

fn required<Id>(node: Option<Id>) -> Id {
    node.expect("nil node in source AST utility")
}

pub(crate) fn access_name<V: View>(view: &V, id: V::Id) -> Result<Option<V::Id>, V::Error> {
    let node = view.read(id)?;
    match node.kind().known() {
        Some(K::PropertyAccessExpression) => Ok((view.read(required(node.name()))?.kind()
            == K::Identifier)
            .then_some(node.name())
            .flatten()),
        Some(K::ElementAccessExpression) => {
            let arg = syntax_helpers::skip_parentheses(
                view,
                node.element_argument()
                    .expect("nil element access argument"),
            )?;
            Ok(
                crate::utilities::is_string_or_numeric_literal_like_kind(view.read(arg)?.kind())
                    .then_some(arg),
            )
        }
        _ => panic!("Unhandled case in GetElementOrPropertyAccessName"),
    }
}

pub(crate) fn assigned_name<V: View>(view: &V, id: V::Id) -> Result<Option<V::Id>, V::Error> {
    let Some(parent) = view.read(id)?.parent() else {
        return Ok(None);
    };
    let node = view.read(parent)?;
    match node.kind().known() {
        Some(K::PropertyAssignment) => return Ok(node.property_assignment_name()),
        Some(K::BindingElement) => return Ok(node.binding_element_name()),
        Some(K::BinaryExpression) => {
            let (left, right) = node.declaration_binary_operands();
            if right == Some(id) {
                let left = left.expect("nil assigned-name left operand");
                let read = view.read(left)?;
                match read.kind().known() {
                    Some(K::Identifier) => return Ok(Some(left)),
                    Some(K::PropertyAccessExpression) => return Ok(read.name()),
                    Some(K::ElementAccessExpression) => return access_name(view, left),
                    _ => {}
                }
            }
        }
        Some(K::VariableDeclaration)
            if view.read(required(node.name()))?.kind() == K::Identifier =>
        {
            return Ok(node.name())
        }
        _ => {}
    }
    Ok(None)
}

pub(crate) fn non_assigned_name<V: View>(view: &V, id: V::Id) -> Result<Name<V::Id>, V::Error> {
    let node = view.read(id)?;
    Ok(match node.kind().known() {
        Some(K::BinaryExpression | K::CallExpression) => Name::Assignment(id),
        Some(K::ExportAssignment) => Name::Resolved(
            (view.read(required(node.expression()))?.kind() == K::Identifier)
                .then_some(node.expression())
                .flatten(),
        ),
        _ => Name::Resolved(node.name()),
    })
}

pub(crate) fn name<V: View>(view: &V, id: Option<V::Id>) -> Result<Name<V::Id>, V::Error> {
    let Some(id) = id else {
        return Ok(Name::Resolved(None));
    };
    match non_assigned_name(view, id)? {
        Name::Resolved(None) => {}
        result => return Ok(result),
    }
    if matches!(
        view.read(id)?.kind().known(),
        Some(K::FunctionExpression | K::ArrowFunction | K::ClassExpression)
    ) {
        return Ok(Name::Resolved(assigned_name(view, id)?));
    }
    Ok(Name::Resolved(None))
}

pub(crate) fn signed_numeric_literal<V: View>(view: &V, id: V::Id) -> Result<bool, V::Error> {
    let node = view.read(id)?;
    if node.kind() != K::PrefixUnaryExpression {
        return Ok(false);
    }
    Ok(matches!(
        node.prefix_operator().known(),
        Some(K::PlusToken | K::MinusToken)
    ) && view.read(required(node.prefix_operand()))?.kind() == K::NumericLiteral)
}

pub(crate) fn dynamic_name<V: View>(view: &V, id: V::Id) -> Result<bool, V::Error> {
    let node = view.read(id)?;
    let expression = match node.kind().known() {
        Some(K::ComputedPropertyName) => {
            node.expression().expect("nil computed property expression")
        }
        Some(K::ElementAccessExpression) => syntax_helpers::skip_parentheses(
            view,
            node.element_argument()
                .expect("nil element access argument"),
        )?,
        _ => return Ok(false),
    };
    Ok(
        !crate::utilities::is_string_or_numeric_literal_like_kind(view.read(expression)?.kind())
            && !signed_numeric_literal(view, expression)?,
    )
}

pub(crate) fn ambient_module<V: View>(view: &V, id: V::Id) -> Result<bool, V::Error> {
    let node = view.read(id)?;
    Ok(node.kind() == K::ModuleDeclaration
        && (view.read(required(node.module_name()))?.kind() == K::StringLiteral
            || node.module_keyword() == K::GlobalKeyword))
}

pub(crate) fn root_declaration<V: View>(view: &V, mut id: V::Id) -> Result<V::Id, V::Error> {
    while view.read(id)?.kind() == K::BindingElement {
        id = required(view.read(required(view.read(id)?.parent()))?.parent());
    }
    Ok(id)
}

pub(crate) fn modifier_flags<V: View>(view: &V, id: V::Id) -> Result<u32, V::Error> {
    view.modifier_flags(&view.read(id)?)
}

pub(crate) fn has_modifier<V: View>(view: &V, id: V::Id, flags: u32) -> Result<bool, V::Error> {
    Ok(modifier_flags(view, id)? & flags != 0)
}

pub(crate) fn combined_modifier_flags<V: View>(view: &V, id: V::Id) -> Result<u32, V::Error> {
    let id = root_declaration(view, id)?;
    let root = view.read(id)?;
    let mut flags = view.modifier_flags(&root)?;
    let mut current = Some(id);
    if root.kind() == K::VariableDeclaration {
        current = root.parent();
    }
    if let Some(id) = current {
        let node = view.read(id)?;
        if node.kind() == K::VariableDeclarationList {
            flags |= view.modifier_flags(&node)?;
            current = node.parent();
        }
    }
    if let Some(id) = current {
        let node = view.read(id)?;
        if node.kind() == K::VariableStatement {
            flags |= view.modifier_flags(&node)?;
        }
    }
    Ok(flags)
}
