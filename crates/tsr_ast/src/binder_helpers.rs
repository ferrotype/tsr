//! Source syntax helpers used by binding. Every graph read goes through the
//! supplied view, including virtual-parent module analysis before parents exist.
use crate::NodeAccess;
use crate::{modifier_flags, node_flags, AstView, NodeDataRead, NodeId, NodeRead, SyntaxKind as K};
use std::collections::HashMap;
use ts_arena::Error;

fn required(view: AstView<'_>, id: Option<NodeId>) -> Result<NodeRead<'_>, Error> {
    view.node(id.expect("nil node in source AST utility"))
}
fn parent(view: AstView<'_>, id: NodeId) -> Result<NodeId, Error> {
    Ok(view
        .node(id)?
        .parent()
        .expect("nil node in source AST utility"))
}
macro_rules! payload {
    ($node:expr, $variant:ident) => {
        match $node.data() {
            NodeDataRead::$variant(data) => data,
            _ => panic!(
                "interface conversion: ast.nodeData is *ast.{}, not *ast.{}",
                $node.data().name(),
                stringify!($variant)
            ),
        }
    };
}

/// port: tsc/internal/ast/ast.go:IsDeclarationNode
pub fn is_declaration_node(node: &(impl NodeAccess + ?Sized)) -> bool {
    matches!(
        node.data(),
        NodeDataRead::VariableDeclaration(_)
            | NodeDataRead::ParameterDeclaration(_)
            | NodeDataRead::BindingElement(_)
            | NodeDataRead::MissingDeclaration(_)
            | NodeDataRead::FunctionDeclaration(_)
            | NodeDataRead::ClassDeclaration(_)
            | NodeDataRead::ClassExpression(_)
            | NodeDataRead::InterfaceDeclaration(_)
            | NodeDataRead::TypeAliasDeclaration(_)
            | NodeDataRead::EnumMember(_)
            | NodeDataRead::EnumDeclaration(_)
            | NodeDataRead::ImportDeclaration(_)
            | NodeDataRead::NamespaceImport(_)
            | NodeDataRead::ExportAssignment(_)
            | NodeDataRead::NamespaceExportDeclaration(_)
            | NodeDataRead::NamespaceExport(_)
            | NodeDataRead::ExportSpecifier(_)
            | NodeDataRead::CallSignatureDeclaration(_)
            | NodeDataRead::ConstructSignatureDeclaration(_)
            | NodeDataRead::ConstructorDeclaration(_)
            | NodeDataRead::GetAccessorDeclaration(_)
            | NodeDataRead::SetAccessorDeclaration(_)
            | NodeDataRead::IndexSignatureDeclaration(_)
            | NodeDataRead::MethodSignatureDeclaration(_)
            | NodeDataRead::MethodDeclaration(_)
            | NodeDataRead::PropertySignatureDeclaration(_)
            | NodeDataRead::PropertyDeclaration(_)
            | NodeDataRead::SemicolonClassElement(_)
            | NodeDataRead::ClassStaticBlockDeclaration(_)
            | NodeDataRead::NoSubstitutionTemplateLiteral(_)
            | NodeDataRead::BinaryExpression(_)
            | NodeDataRead::ArrowFunction(_)
            | NodeDataRead::FunctionExpression(_)
            | NodeDataRead::CallExpression(_)
            | NodeDataRead::ObjectLiteralExpression(_)
            | NodeDataRead::SpreadAssignment(_)
            | NodeDataRead::PropertyAssignment(_)
            | NodeDataRead::ShorthandPropertyAssignment(_)
            | NodeDataRead::MappedTypeNode(_)
            | NodeDataRead::TypeLiteralNode(_)
            | NodeDataRead::NamedTupleMember(_)
            | NodeDataRead::FunctionTypeNode(_)
            | NodeDataRead::ConstructorTypeNode(_)
            | NodeDataRead::JsxAttributes(_)
            | NodeDataRead::JsxAttribute(_)
            | NodeDataRead::JSDocSignature(_)
            | NodeDataRead::SourceFile(_)
            | NodeDataRead::ModuleDeclaration(_)
            | NodeDataRead::ImportEqualsDeclaration(_)
            | NodeDataRead::ExportDeclaration(_)
            | NodeDataRead::ImportClause(_)
            | NodeDataRead::ImportSpecifier(_)
            | NodeDataRead::TypeParameterDeclaration(_)
            | NodeDataRead::JSDocTypeLiteral(_)
    )
}
/// port: tsc/internal/ast/utilities.go:IsDeclaration
pub fn is_declaration(node: &(impl NodeAccess + ?Sized)) -> bool {
    if node.kind() == K::TypeParameter {
        node.parent().is_some()
    } else {
        is_declaration_node(node)
    }
}

/// port: tsc/internal/ast/utilities.go:NodeIsMissing
pub fn node_is_missing(node: Option<&(impl NodeAccess + ?Sized)>) -> bool {
    node.is_none_or(|n| n.pos() == n.end() && n.pos() >= 0 && n.kind() != K::EndOfFile)
}
/// port: tsc/internal/ast/utilities.go:NodeIsPresent
pub fn node_is_present(node: Option<&(impl NodeAccess + ?Sized)>) -> bool {
    !node_is_missing(node)
}

/// port: tsc/internal/ast/utilities.go:SkipParentheses
pub fn skip_parentheses(view: AstView<'_>, id: NodeId) -> Result<NodeId, Error> {
    crate::syntax_helpers::skip_parentheses(&view, id)
}
/// port: tsc/internal/ast/utilities.go:SkipPartiallyEmittedExpressions
pub fn skip_partially_emitted_expressions(view: AstView<'_>, id: NodeId) -> Result<NodeId, Error> {
    crate::syntax_helpers::skip_partially_emitted_expressions(&view, id)
}
/// port: tsc/internal/ast/utilities.go:IsLeftHandSideExpression
pub fn is_left_hand_side_expression(view: AstView<'_>, id: NodeId) -> Result<bool, Error> {
    crate::syntax_helpers::is_left_hand_side_expression(&view, id)
}
/// port: tsc/internal/ast/utilities.go:IsAssignmentExpression
pub fn is_assignment_expression(
    view: AstView<'_>,
    id: NodeId,
    exclude_compound_assignment: bool,
) -> Result<bool, Error> {
    let node = view.node(id)?;
    if node.kind() != K::BinaryExpression {
        return Ok(false);
    }
    let data = payload!(node, BinaryExpression);
    let operator = required(view, data.operator_token())?.kind();
    Ok((operator == K::EqualsToken
        || !exclude_compound_assignment && crate::is_assignment_operator(operator))
        && is_left_hand_side_expression(view, data.left().expect("nil assignment left operand"))?)
}
/// port: tsc/internal/ast/utilities.go:IsDestructuringAssignment
pub fn is_destructuring_assignment(view: AstView<'_>, id: NodeId) -> Result<bool, Error> {
    if !is_assignment_expression(view, id, true)? {
        return Ok(false);
    }
    let node = view.node(id)?;
    Ok(matches!(
        required(view, payload!(node, BinaryExpression).left())?
            .kind()
            .known(),
        Some(K::ObjectLiteralExpression | K::ArrayLiteralExpression)
    ))
}
/// port: tsc/internal/ast/utilities.go:GetAssignmentTarget
pub fn get_assignment_target(view: AstView<'_>, mut id: NodeId) -> Result<Option<NodeId>, Error> {
    loop {
        let parent_id = parent(view, id)?;
        let p = view.node(parent_id)?;
        match p.kind().known() {
            Some(K::BinaryExpression) => {
                let data = payload!(p, BinaryExpression);
                return Ok((crate::is_assignment_operator(
                    required(view, data.operator_token())?.kind(),
                ) && data.left() == Some(id))
                .then_some(parent_id));
            }
            Some(K::PrefixUnaryExpression) => {
                return Ok(matches!(
                    payload!(p, PrefixUnaryExpression).operator().known(),
                    Some(K::PlusPlusToken | K::MinusMinusToken)
                )
                .then_some(parent_id));
            }
            Some(K::PostfixUnaryExpression) => {
                return Ok(matches!(
                    payload!(p, PostfixUnaryExpression).operator().known(),
                    Some(K::PlusPlusToken | K::MinusMinusToken)
                )
                .then_some(parent_id));
            }
            Some(K::ForInStatement | K::ForOfStatement) => {
                return Ok((p.initializer() == Some(id)).then_some(parent_id));
            }
            Some(
                K::ParenthesizedExpression
                | K::ArrayLiteralExpression
                | K::SpreadElement
                | K::NonNullExpression,
            ) => id = parent_id,
            Some(K::SpreadAssignment) => id = parent(view, parent_id)?,
            Some(K::ShorthandPropertyAssignment) => {
                if payload!(p, ShorthandPropertyAssignment).name() != Some(id) {
                    return Ok(None);
                }
                id = parent(view, parent_id)?;
            }
            Some(K::PropertyAssignment) => {
                if payload!(p, PropertyAssignment).name() == Some(id) {
                    return Ok(None);
                }
                id = parent(view, parent_id)?;
            }
            _ => return Ok(None),
        }
    }
}
/// port: tsc/internal/ast/utilities.go:IsAssignmentTarget
pub fn is_assignment_target(view: AstView<'_>, id: NodeId) -> Result<bool, Error> {
    Ok(get_assignment_target(view, id)?.is_some())
}

/// port: tsc/internal/ast/utilities.go:IsIdentifierName
pub fn is_identifier_name(view: AstView<'_>, id: NodeId) -> Result<bool, Error> {
    let p = view.node(parent(view, id)?)?;
    Ok(match identifier_name_role(p.kind()) {
        IdentifierNameRole::Name => p.name() == Some(id),
        IdentifierNameRole::Right => payload!(p, QualifiedName).right() == Some(id),
        IdentifierNameRole::PropertyName => p.property_name() == Some(id),
        IdentifierNameRole::Always => true,
        IdentifierNameRole::Never => false,
    })
}

pub(crate) enum IdentifierNameRole {
    Name,
    Right,
    PropertyName,
    Always,
    Never,
}

pub(crate) fn identifier_name_role(kind: crate::NodeKind) -> IdentifierNameRole {
    match kind.known() {
        Some(
            K::PropertyDeclaration
            | K::PropertySignature
            | K::MethodDeclaration
            | K::MethodSignature
            | K::GetAccessor
            | K::SetAccessor
            | K::EnumMember
            | K::PropertyAssignment
            | K::PropertyAccessExpression,
        ) => IdentifierNameRole::Name,
        Some(K::QualifiedName) => IdentifierNameRole::Right,
        Some(K::BindingElement | K::ImportSpecifier) => IdentifierNameRole::PropertyName,
        Some(
            K::ExportSpecifier
            | K::JsxAttribute
            | K::JsxSelfClosingElement
            | K::JsxOpeningElement
            | K::JsxClosingElement,
        ) => IdentifierNameRole::Always,
        _ => IdentifierNameRole::Never,
    }
}
/// port: tsc/internal/ast/utilities.go:IsPushOrUnshiftIdentifier
pub fn is_push_or_unshift_identifier(view: AstView<'_>, id: NodeId) -> Result<bool, Error> {
    Ok(is_push_or_unshift_text(view.node_text(id)?.as_bytes()))
}

pub(crate) fn is_push_or_unshift_text(bytes: &[u8]) -> bool {
    matches!(bytes, b"push" | b"unshift")
}

/// port: tsc/internal/ast/utilities.go:IsExportsIdentifier
pub fn is_exports_identifier(view: AstView<'_>, id: NodeId) -> Result<bool, Error> {
    Ok(view.node(id)?.kind() == K::Identifier && view.node_text(id)?.as_bytes() == b"exports")
}
/// port: tsc/internal/ast/utilities.go:IsModuleIdentifier
pub fn is_module_identifier(view: AstView<'_>, id: NodeId) -> Result<bool, Error> {
    Ok(view.node(id)?.kind() == K::Identifier && view.node_text(id)?.as_bytes() == b"module")
}
/// port: tsc/internal/ast/utilities.go:IsEntityNameExpression
pub fn is_entity_name_expression(view: AstView<'_>, id: NodeId) -> Result<bool, Error> {
    is_entity_name_expression_ex(view, id, false)
}
/// port: tsc/internal/ast/utilities.go:IsEntityNameExpressionEx
pub fn is_entity_name_expression_ex(
    view: AstView<'_>,
    id: NodeId,
    allow_js: bool,
) -> Result<bool, Error> {
    crate::syntax_helpers::is_entity_name_expression(&view, id, allow_js)
}
/// port: tsc/internal/ast/utilities.go:IsPropertyAccessEntityNameExpression
pub fn is_property_access_entity_name_expression(
    view: AstView<'_>,
    id: NodeId,
    allow_js: bool,
) -> Result<bool, Error> {
    let node = view.node(id)?;
    Ok(node.kind() == K::PropertyAccessExpression
        && required(view, node.name())?.kind() == K::Identifier
        && is_entity_name_expression_ex(
            view,
            node.expression().expect("nil entity-name expression"),
            allow_js,
        )?)
}
/// port: tsc/internal/ast/utilities.go:isElementAccessEntityNameExpression
pub fn is_element_access_entity_name_expression(
    view: AstView<'_>,
    id: NodeId,
    allow_js: bool,
) -> Result<bool, Error> {
    let node = view.node(id)?;
    Ok(node.kind() == K::ElementAccessExpression
        && crate::utilities::is_string_or_numeric_literal_like(&required(
            view,
            payload!(node, ElementAccessExpression).argument_expression(),
        )?)
        && is_entity_name_expression_ex(
            view,
            node.expression().expect("nil entity-name expression"),
            allow_js,
        )?)
}
/// port: tsc/internal/ast/utilities.go:IsDottedName
pub fn is_dotted_name(view: AstView<'_>, id: NodeId) -> Result<bool, Error> {
    crate::syntax_helpers::is_dotted_name(&view, id)
}
/// port: tsc/internal/ast/utilities.go:IsLiteralLikeElementAccess
pub fn is_literal_like_element_access(view: AstView<'_>, id: NodeId) -> Result<bool, Error> {
    let node = view.node(id)?;
    Ok(node.kind() == K::ElementAccessExpression
        && crate::utilities::is_string_or_numeric_literal_like(&required(
            view,
            payload!(node, ElementAccessExpression).argument_expression(),
        )?))
}
/// port: tsc/internal/ast/utilities.go:IsBindableStaticAccessExpression
pub fn is_bindable_static_access_expression(
    view: AstView<'_>,
    id: NodeId,
    exclude_this: bool,
) -> Result<bool, Error> {
    stacker::maybe_grow(64 * 1024, 1024 * 1024, || {
        let node = view.node(id)?;
        if node.kind() == K::PropertyAccessExpression {
            let expression = node.expression().expect("nil static access expression");
            if !exclude_this && view.node(expression)?.kind() == K::ThisKeyword
                || required(view, node.name())?.kind() == K::Identifier
                    && is_bindable_static_name_expression(view, expression, true)?
            {
                return Ok(true);
            }
        }
        is_bindable_static_element_access_expression(view, id, exclude_this)
    })
}
/// port: tsc/internal/ast/utilities.go:IsBindableStaticElementAccessExpression
pub fn is_bindable_static_element_access_expression(
    view: AstView<'_>,
    id: NodeId,
    exclude_this: bool,
) -> Result<bool, Error> {
    if !is_literal_like_element_access(view, id)? {
        return Ok(false);
    }
    let expression = view
        .node(id)?
        .expression()
        .expect("nil static element access expression");
    Ok(
        !exclude_this && view.node(expression)?.kind() == K::ThisKeyword
            || is_entity_name_expression(view, expression)?
            || is_bindable_static_access_expression(view, expression, true)?,
    )
}
/// port: tsc/internal/ast/utilities.go:IsBindableStaticNameExpression
pub fn is_bindable_static_name_expression(
    view: AstView<'_>,
    id: NodeId,
    exclude_this: bool,
) -> Result<bool, Error> {
    Ok(is_entity_name_expression(view, id)?
        || is_bindable_static_access_expression(view, id, exclude_this)?)
}
/// port: tsc/internal/ast/utilities.go:GetElementOrPropertyAccessName
pub fn get_element_or_property_access_name(
    view: AstView<'_>,
    id: NodeId,
) -> Result<Option<NodeId>, Error> {
    crate::declaration_helpers::access_name(&view, id)
}
/// port: tsc/internal/ast/utilities.go:IsModuleExportsAccessExpression
pub fn is_module_exports_access_expression(view: AstView<'_>, id: NodeId) -> Result<bool, Error> {
    let node = view.node(id)?;
    if crate::utilities::is_access_expression(&node)
        && is_module_identifier(
            view,
            node.expression().expect("nil module exports expression"),
        )?
    {
        if let Some(name) = get_element_or_property_access_name(view, id)? {
            return Ok(view.node_text(name)?.as_bytes() == b"exports");
        }
    }
    Ok(false)
}
/// port: tsc/internal/ast/utilities.go:IsBindableObjectDefinePropertyCall
pub fn is_bindable_object_define_property_call(
    view: AstView<'_>,
    id: NodeId,
) -> Result<bool, Error> {
    let node = view.node(id)?;
    let arguments = view.node_slice(node.arguments(view)?)?;
    if arguments.len() != 3 {
        return Ok(false);
    }
    let expr = required(view, node.expression())?;
    Ok(expr.kind() == K::PropertyAccessExpression
        && required(view, expr.expression())?.kind() == K::Identifier
        && view
            .node_text(expr.expression().expect("Object expression"))?
            .as_bytes()
            == b"Object"
        && view
            .node_text(expr.name().expect("nil defineProperty name"))?
            .as_bytes()
            == b"defineProperty"
        && crate::utilities::is_string_or_numeric_literal_like(&required(view, arguments.at(1))?)
        && is_bindable_static_name_expression(
            view,
            arguments.at(0).expect("nil defineProperty target"),
            true,
        )?)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(i64)]
pub enum JSDeclarationKind {
    None,
    ModuleExports,
    ExportsProperty,
    ThisProperty,
    Property,
    ObjectDefinePropertyValue,
    ObjectDefinePropertyExports,
}

/// port: tsc/internal/ast/utilities.go:GetAssignmentDeclarationKind
pub fn get_assignment_declaration_kind(
    view: AstView<'_>,
    id: NodeId,
) -> Result<JSDeclarationKind, Error> {
    let node = view.node(id)?;
    match node.kind().known() {
        Some(K::BinaryExpression) => {
            let bin = payload!(node, BinaryExpression);
            if required(view, bin.operator_token())?.kind() == K::EqualsToken {
                let left_id = bin.left().expect("nil assignment left operand");
                let left = view.node(left_id)?;
                if crate::utilities::is_access_expression(&left) {
                    let js = crate::utilities::is_in_js_file(Some(&left));
                    if js {
                        if is_module_exports_access_expression(view, left_id)?
                            && !is_exports_identifier(
                                view,
                                bin.right().expect("nil assignment right operand"),
                            )?
                        {
                            return Ok(JSDeclarationKind::ModuleExports);
                        }
                        let expr = left.expression().expect("nil assignment access expression");
                        if (is_module_exports_access_expression(view, expr)?
                            || is_exports_identifier(view, expr)?)
                            && get_element_or_property_access_name(view, left_id)?.is_some()
                        {
                            return Ok(JSDeclarationKind::ExportsProperty);
                        }
                        if view.node(expr)?.kind() == K::ThisKeyword {
                            return Ok(JSDeclarationKind::ThisProperty);
                        }
                    }
                    if left.kind() == K::PropertyAccessExpression
                        && is_entity_name_expression_ex(
                            view,
                            left.expression().expect("nil assignment access expression"),
                            js,
                        )?
                        && required(view, left.name())?.kind() == K::Identifier
                        || left.kind() == K::ElementAccessExpression
                            && is_entity_name_expression_ex(
                                view,
                                left.expression().expect("nil assignment access expression"),
                                js,
                            )?
                    {
                        return Ok(JSDeclarationKind::Property);
                    }
                }
            }
        }
        Some(K::CallExpression)
            if crate::utilities::is_in_js_file(Some(&node))
                && is_bindable_object_define_property_call(view, id)? =>
        {
            let args = view.node_slice(node.arguments(view)?)?;
            let entity = args.at(0).expect("nil defineProperty target");
            return Ok(
                if is_exports_identifier(view, entity)?
                    || is_module_exports_access_expression(view, entity)?
                {
                    JSDeclarationKind::ObjectDefinePropertyExports
                } else {
                    JSDeclarationKind::ObjectDefinePropertyValue
                },
            );
        }
        _ => {}
    }
    Ok(JSDeclarationKind::None)
}

/// port: tsc/internal/ast/utilities.go:GetAssignedName
pub fn get_assigned_name(view: AstView<'_>, id: NodeId) -> Result<Option<NodeId>, Error> {
    crate::declaration_helpers::assigned_name(&view, id)
}
// Cold JS assignment/name classification shared by checked and scoped callers.
pub(crate) fn assignment_name_of_declaration(
    view: AstView<'_>,
    id: NodeId,
) -> Result<Option<NodeId>, Error> {
    let node = view.node(id)?;
    match node.kind().known() {
        Some(K::BinaryExpression | K::CallExpression) => {
            Ok(match get_assignment_declaration_kind(view, id)? {
                JSDeclarationKind::Property
                | JSDeclarationKind::ThisProperty
                | JSDeclarationKind::ExportsProperty => {
                    let left = payload!(node, BinaryExpression)
                        .left()
                        .expect("nil declaration left operand");
                    Some(get_element_or_property_access_name(view, left)?.unwrap_or(left))
                }
                JSDeclarationKind::ObjectDefinePropertyValue
                | JSDeclarationKind::ObjectDefinePropertyExports => {
                    view.node_slice(node.arguments(view)?)?.at(1)
                }
                _ => None,
            })
        }
        _ => unreachable!("checked assignment-name branch"),
    }
}

/// port: tsc/internal/ast/utilities.go:GetNonAssignedNameOfDeclaration
pub fn get_non_assigned_name_of_declaration(
    view: AstView<'_>,
    id: NodeId,
) -> Result<Option<NodeId>, Error> {
    match crate::declaration_helpers::non_assigned_name(&view, id)? {
        crate::declaration_helpers::Name::Resolved(name) => Ok(name),
        crate::declaration_helpers::Name::Assignment(id) => {
            assignment_name_of_declaration(view, id)
        }
    }
}
/// port: tsc/internal/ast/utilities.go:GetNameOfDeclaration
pub fn get_name_of_declaration(
    view: AstView<'_>,
    id: Option<NodeId>,
) -> Result<Option<NodeId>, Error> {
    match crate::declaration_helpers::name(&view, id)? {
        crate::declaration_helpers::Name::Resolved(name) => Ok(name),
        crate::declaration_helpers::Name::Assignment(id) => {
            assignment_name_of_declaration(view, id)
        }
    }
}
/// port: tsc/internal/ast/utilities.go:IsDynamicName
pub fn is_dynamic_name(view: AstView<'_>, id: NodeId) -> Result<bool, Error> {
    crate::declaration_helpers::dynamic_name(&view, id)
}
/// port: tsc/internal/ast/utilities.go:HasDynamicName
pub fn has_dynamic_name(view: AstView<'_>, id: Option<NodeId>) -> Result<bool, Error> {
    get_name_of_declaration(view, id)?.map_or(Ok(false), |name| is_dynamic_name(view, name))
}
/// port: tsc/internal/ast/utilities.go:ExpressionIsAlias
pub fn expression_is_alias(view: AstView<'_>, id: NodeId) -> Result<bool, Error> {
    Ok(is_entity_name_expression(view, id)? || view.node(id)?.kind() == K::ClassExpression)
}

/// port: tsc/internal/ast/utilities.go:IsAmbientModule
pub fn is_ambient_module(view: AstView<'_>, id: NodeId) -> Result<bool, Error> {
    crate::declaration_helpers::ambient_module(&view, id)
}
/// port: tsc/internal/ast/utilities.go:IsModuleAugmentationExternal
pub fn is_module_augmentation_external(view: AstView<'_>, id: NodeId) -> Result<bool, Error> {
    let p = parent(view, id)?;
    match view.node(p)?.kind().known() {
        Some(K::SourceFile) => Ok(crate::utilities::is_external_module(&*view.source_file(p)?)),
        Some(K::ModuleBlock) => {
            let grand = parent(view, p)?;
            if !is_ambient_module(view, grand)? {
                return Ok(false);
            }
            let great = parent(view, grand)?;
            Ok(view.node(great)?.kind() == K::SourceFile
                && !crate::utilities::is_external_module(&*view.source_file(great)?))
        }
        _ => Ok(false),
    }
}
/// port: tsc/internal/ast/utilities.go:IsPartOfTypeQuery
pub fn is_part_of_type_query(view: AstView<'_>, mut id: NodeId) -> Result<bool, Error> {
    while matches!(
        view.node(id)?.kind().known(),
        Some(K::QualifiedName | K::Identifier)
    ) {
        id = parent(view, id)?;
    }
    Ok(view.node(id)?.kind() == K::TypeQuery)
}
/// port: tsc/internal/ast/utilities.go:GetThisContainer
pub fn get_this_container(
    view: AstView<'_>,
    mut id: NodeId,
    include_arrows: bool,
    include_class_computed_name: bool,
) -> Result<NodeId, Error> {
    loop {
        id = view
            .node(id)?
            .parent()
            .expect("nil parent in getThisContainer");
        match view.node(id)?.kind().known() {
            Some(K::ComputedPropertyName) => {
                let grand = parent(view, parent(view, id)?)?;
                if include_class_computed_name
                    && crate::utilities::is_class_like(&view.node(grand)?)
                {
                    return Ok(id);
                }
                id = grand;
            }
            Some(K::Decorator) => {
                let p = parent(view, id)?;
                if view.node(p)?.kind() == K::Parameter
                    && crate::utilities::is_class_element(&view.node(parent(view, p)?)?)
                {
                    id = parent(view, p)?;
                } else if crate::utilities::is_class_element(&view.node(p)?) {
                    id = p;
                }
            }
            Some(K::ArrowFunction) if include_arrows => return Ok(id),
            Some(
                K::FunctionDeclaration
                | K::FunctionExpression
                | K::ModuleDeclaration
                | K::ClassStaticBlockDeclaration
                | K::PropertyDeclaration
                | K::PropertySignature
                | K::MethodDeclaration
                | K::MethodSignature
                | K::Constructor
                | K::GetAccessor
                | K::SetAccessor
                | K::CallSignature
                | K::ConstructSignature
                | K::IndexSignature
                | K::EnumDeclaration
                | K::SourceFile,
            ) => return Ok(id),
            _ => {}
        }
    }
}
/// port: tsc/internal/ast/utilities.go:IsInTopLevelContext
pub fn is_in_top_level_context(view: AstView<'_>, mut id: NodeId) -> Result<bool, Error> {
    if view.node(id)?.kind() == K::Identifier {
        let p = parent(view, id)?;
        let node = view.node(p)?;
        if matches!(
            node.kind().known(),
            Some(K::ClassDeclaration | K::FunctionDeclaration)
        ) && node.name() == Some(id)
        {
            id = p;
        }
    }
    Ok(view
        .node(get_this_container(view, id, true, false)?)?
        .kind()
        == K::SourceFile)
}
/// port: tsc/internal/ast/utilities.go:GetImmediatelyInvokedFunctionExpression
pub fn get_immediately_invoked_function_expression(
    view: AstView<'_>,
    id: NodeId,
) -> Result<Option<NodeId>, Error> {
    if !matches!(
        view.node(id)?.kind().known(),
        Some(K::FunctionExpression | K::ArrowFunction)
    ) {
        return Ok(None);
    }
    let mut previous = id;
    let mut p = parent(view, id)?;
    while view.node(p)?.kind() == K::ParenthesizedExpression {
        previous = p;
        p = parent(view, p)?;
    }
    let node = view.node(p)?;
    Ok((node.kind() == K::CallExpression && node.expression() == Some(previous)).then_some(p))
}
/// port: tsc/internal/ast/utilities.go:GetDeclarationContainer
pub fn get_declaration_container(view: AstView<'_>, id: NodeId) -> Result<Option<NodeId>, Error> {
    let root = crate::utilities::get_root_declaration(view, id)?;
    let container = crate::utilities::find_ancestor(view, Some(root), |n| {
        !matches!(
            n.kind().known(),
            Some(
                K::VariableDeclaration
                    | K::VariableDeclarationList
                    | K::ImportSpecifier
                    | K::NamedImports
                    | K::NamespaceImport
                    | K::ImportClause
            )
        )
    })?;
    Ok(required(view, container)?.parent())
}

/// port: tsc/internal/ast/utilities.go:FindConstructorDeclaration
pub fn find_constructor_declaration(
    view: AstView<'_>,
    id: NodeId,
) -> Result<Option<NodeId>, Error> {
    let node = view.node(id)?;
    for member in view.node_slice(node.members(view)?)?.iter() {
        let member = member.expect("nil class member");
        let node = view.node(member)?;
        if node.kind() == K::Constructor {
            let body = node.body().map(|id| view.node(id)).transpose()?;
            if node_is_present(body.as_ref()) {
                return Ok(Some(member));
            }
        }
    }
    Ok(None)
}
/// port: tsc/internal/ast/utilities.go:IsExpandoInitializer
pub fn is_expando_initializer(
    view: AstView<'_>,
    declaration: NodeId,
    initializer: Option<NodeId>,
) -> Result<bool, Error> {
    let Some(initializer) = initializer else {
        return Ok(false);
    };
    let node = view.node(initializer)?;
    if matches!(
        node.kind().known(),
        Some(K::FunctionExpression | K::ArrowFunction)
    ) {
        return Ok(true);
    }
    Ok(crate::utilities::is_in_js_file(Some(&node))
        && (node.kind() == K::ClassExpression
            || node.kind() == K::ObjectLiteralExpression
                && node.properties(view)?.is_empty()
                && view.node(declaration)?.type_node().is_none()))
}

/// port: tsc/internal/ast/utilities.go:GetLeftmostAccessExpression
pub fn get_leftmost_access_expression(view: AstView<'_>, mut id: NodeId) -> Result<NodeId, Error> {
    while crate::utilities::is_access_expression(&view.node(id)?) {
        id = view.node(id)?.expression().expect("nil access expression");
    }
    Ok(id)
}
/// port: tsc/internal/ast/utilities.go:isVariableDeclarationInitializedWithRequireHelper
fn variable_initialized_with_require(
    view: AstView<'_>,
    id: NodeId,
    allow_accessed: bool,
) -> Result<bool, Error> {
    let node = view.node(id)?;
    if !crate::utilities::is_in_js_file(Some(&node)) || node.kind() != K::VariableDeclaration {
        return Ok(false);
    }
    let Some(mut initializer) = node.initializer() else {
        return Ok(false);
    };
    if allow_accessed {
        initializer = get_leftmost_access_expression(view, initializer)?;
    }
    Ok(view
        .node(parent(view, parent(view, id)?)?)?
        .modifier_flags(view)?
        & modifier_flags::EXPORT
        == 0
        && node.type_node().is_none()
        && crate::utilities_middle::is_require_call(view, &view.node(initializer)?, true)?)
}
/// port: tsc/internal/ast/utilities.go:IsVariableDeclarationInitializedToRequire
pub fn is_variable_declaration_initialized_to_require(
    view: AstView<'_>,
    mut id: NodeId,
) -> Result<bool, Error> {
    if view.node(id)?.kind() == K::BindingElement {
        id = parent(view, parent(view, id)?)?;
    }
    variable_initialized_with_require(view, id, false)
}
/// port: tsc/internal/ast/utilities.go:IsVariableDeclarationInitializedToBareOrAccessedRequire
pub fn is_variable_declaration_initialized_to_bare_or_accessed_require(
    view: AstView<'_>,
    id: NodeId,
) -> Result<bool, Error> {
    variable_initialized_with_require(view, id, true)
}
/// port: tsc/internal/ast/utilities.go:IsImplicitlyExportedJSDocDeclaration
pub fn is_implicitly_exported_js_doc_declaration(
    view: AstView<'_>,
    id: NodeId,
) -> Result<bool, Error> {
    let node = view.node(id)?;
    let p = node
        .parent()
        .expect("nil implicit JSDoc declaration parent");
    if view.node(p)?.kind() != K::SourceFile
        || !crate::utilities::is_external_or_common_js_module(&view.source_file(p)?)
    {
        return Ok(false);
    }
    Ok(node.kind() == K::JSTypeAliasDeclaration
        || node.kind() == K::ModuleDeclaration && node.flags() & node_flags::REPARSED != 0)
}
/// port: tsc/internal/ast/utilities.go:IsPotentiallyExecutableNode
pub fn is_potentially_executable_node(view: AstView<'_>, id: NodeId) -> Result<bool, Error> {
    let node = view.node(id)?;
    if (K::FirstStatement as i16..=K::LastStatement as i16).contains(&node.kind().raw()) {
        if node.kind() == K::VariableStatement {
            let list = payload!(node, VariableStatement)
                .declaration_list()
                .expect("nil variable declaration list");
            if crate::utilities::get_combined_node_flags(view, list)? & node_flags::BLOCK_SCOPED
                != 0
            {
                return Ok(true);
            }
            let list = view.node(list)?;
            let declarations = payload!(list, VariableDeclarationList)
                .declarations()
                .expect("nil variable declarations");
            for id in view.node_slice(view.list(declarations)?.nodes())?.iter() {
                if required(view, id)?.initializer().is_some() {
                    return Ok(true);
                }
            }
            return Ok(false);
        }
        return Ok(true);
    }
    Ok(matches!(
        node.kind().known(),
        Some(K::ClassDeclaration | K::EnumDeclaration | K::ModuleDeclaration)
    ))
}
/// port: tsc/internal/ast/utilities.go:IsAsyncFunction
pub fn is_async_function(view: AstView<'_>, id: NodeId) -> Result<bool, Error> {
    let node = view.node(id)?;
    let (body, asterisk) = match node.kind().known() {
        Some(K::FunctionDeclaration) => {
            let d = payload!(node, FunctionDeclaration);
            (d.body(), d.asterisk_token())
        }
        Some(K::FunctionExpression) => {
            let d = payload!(node, FunctionExpression);
            (d.body(), d.asterisk_token())
        }
        Some(K::ArrowFunction) => {
            let d = payload!(node, ArrowFunction);
            (d.body(), d.asterisk_token())
        }
        Some(K::MethodDeclaration) => {
            let d = payload!(node, MethodDeclaration);
            (d.body(), d.asterisk_token())
        }
        _ => return Ok(false),
    };
    Ok(body.is_some()
        && asterisk.is_none()
        && crate::utilities::has_syntactic_modifier(view, id, modifier_flags::ASYNC)?)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[repr(i32)]
pub enum ModuleInstanceState {
    Unknown,
    NonInstantiated,
    Instantiated,
    ConstEnumOnly,
}

/// port: tsc/internal/ast/utilities.go:GetModuleInstanceState
pub fn get_module_instance_state(
    view: AstView<'_>,
    id: NodeId,
) -> Result<ModuleInstanceState, Error> {
    module_instance_state(view, id, &[], &mut HashMap::new())
}
/// port: tsc/internal/ast/utilities.go:pushAncestor
fn push_ancestor(ancestors: &[NodeId], id: NodeId) -> Vec<NodeId> {
    let mut result = Vec::with_capacity(ancestors.len() + 1);
    result.extend_from_slice(ancestors);
    result.push(id);
    result
}
/// port: tsc/internal/ast/utilities.go:popAncestor
fn pop_ancestor<'a>(
    view: AstView<'_>,
    ancestors: &'a [NodeId],
    id: NodeId,
) -> Result<(&'a [NodeId], Option<NodeId>), Error> {
    Ok(match ancestors.split_last() {
        Some((last, rest)) => (rest, Some(*last)),
        None => (&[], view.node(id)?.parent()),
    })
}
/// port: tsc/internal/ast/utilities.go:getModuleInstanceState
fn module_instance_state(
    view: AstView<'_>,
    id: NodeId,
    ancestors: &[NodeId],
    visited: &mut HashMap<u64, ModuleInstanceState>,
) -> Result<ModuleInstanceState, Error> {
    let node = view.node(id)?;
    let body = payload!(node, ModuleDeclaration).body();
    body.map_or(Ok(ModuleInstanceState::Instantiated), |body| {
        module_instance_state_cached(view, body, &push_ancestor(ancestors, id), visited)
    })
}
/// port: tsc/internal/ast/utilities.go:getModuleInstanceStateCached
fn module_instance_state_cached(
    view: AstView<'_>,
    id: NodeId,
    ancestors: &[NodeId],
    visited: &mut HashMap<u64, ModuleInstanceState>,
) -> Result<ModuleInstanceState, Error> {
    stacker::maybe_grow(64 * 1024, 1024 * 1024, || {
        let runtime_id = crate::runtime_node_id(&view.node(id)?);
        if let Some(&state) = visited.get(&runtime_id) {
            return Ok(if state == ModuleInstanceState::Unknown {
                ModuleInstanceState::NonInstantiated
            } else {
                state
            });
        }
        visited.insert(runtime_id, ModuleInstanceState::Unknown);
        let result = module_instance_state_worker(view, id, ancestors, visited)?;
        visited.insert(runtime_id, result);
        Ok(result)
    })
}
/// port: tsc/internal/ast/utilities.go:getModuleInstanceStateWorker
fn module_instance_state_worker(
    view: AstView<'_>,
    id: NodeId,
    ancestors: &[NodeId],
    visited: &mut HashMap<u64, ModuleInstanceState>,
) -> Result<ModuleInstanceState, Error> {
    use ModuleInstanceState as State;
    let node = view.node(id)?;
    match node.kind().known() {
        Some(K::InterfaceDeclaration | K::TypeAliasDeclaration | K::JSTypeAliasDeclaration) => {
            return Ok(State::NonInstantiated);
        }
        Some(K::EnumDeclaration) if crate::utilities::is_enum_const(view, id)? => {
            return Ok(State::ConstEnumOnly);
        }
        Some(K::ImportDeclaration | K::JSImportDeclaration | K::ImportEqualsDeclaration)
            if !crate::utilities::has_syntactic_modifier(view, id, modifier_flags::EXPORT)? =>
        {
            return Ok(State::NonInstantiated);
        }
        Some(K::ExportDeclaration) => {
            let decl = payload!(node, ExportDeclaration);
            if decl.module_specifier().is_none() {
                if let Some(clause_id) = decl.export_clause() {
                    let clause = view.node(clause_id)?;
                    if clause.kind() == K::NamedExports {
                        let mut state = State::NonInstantiated;
                        let ancestors = push_ancestor(&push_ancestor(ancestors, id), clause_id);
                        for specifier in view.node_slice(clause.elements(view)?)?.iter() {
                            state = state.max(module_instance_state_for_alias_target(
                                view,
                                specifier.expect("nil export specifier"),
                                &ancestors,
                                visited,
                            )?);
                            if state == State::Instantiated {
                                return Ok(state);
                            }
                        }
                        return Ok(state);
                    }
                }
            }
        }
        Some(K::ModuleBlock) => {
            let ancestors = push_ancestor(ancestors, id);
            let mut visitor = ModuleStateVisitor {
                view,
                ancestors: &ancestors,
                visited,
                state: State::NonInstantiated,
                error: None,
            };
            // Use the generated kind-and-payload dispatch of Node.ForEachChild.
            // Nil entries in a present list reach the callback and panic upstream.
            let _ = node.for_each_child(&mut visitor);
            return visitor.error.map_or(Ok(visitor.state), Err);
        }
        Some(K::ModuleDeclaration) => return module_instance_state(view, id, ancestors, visited),
        _ => {}
    }
    Ok(State::Instantiated)
}
struct ModuleStateVisitor<'a, 'b> {
    view: AstView<'a>,
    ancestors: &'b [NodeId],
    visited: &'b mut HashMap<u64, ModuleInstanceState>,
    state: ModuleInstanceState,
    error: Option<Error>,
}
impl crate::ChildVisitor for ModuleStateVisitor<'_, '_> {
    fn visit_node(&mut self, node: NodeId) -> std::ops::ControlFlow<()> {
        use std::ops::ControlFlow;
        use ModuleInstanceState as State;
        match module_instance_state_cached(self.view, node, self.ancestors, self.visited) {
            Ok(State::NonInstantiated) => ControlFlow::Continue(()),
            Ok(State::ConstEnumOnly) => {
                self.state = State::ConstEnumOnly;
                ControlFlow::Continue(())
            }
            Ok(State::Instantiated) => {
                self.state = State::Instantiated;
                ControlFlow::Break(())
            }
            Ok(State::Unknown) => panic!("Unhandled case in getModuleInstanceStateWorker"),
            Err(error) => {
                self.error = Some(error);
                ControlFlow::Break(())
            }
        }
    }
    fn visit_list(&mut self, list: crate::NodeListId) -> std::ops::ControlFlow<()> {
        match self.view.list(list) {
            Ok(list) => self.visit_node_slice(list.nodes()),
            Err(error) => {
                self.error = Some(error);
                std::ops::ControlFlow::Break(())
            }
        }
    }
    fn visit_node_slice(&mut self, nodes: crate::NodeSlice) -> std::ops::ControlFlow<()> {
        let nodes = match self.view.node_slice(nodes) {
            Ok(nodes) => nodes,
            Err(error) => {
                self.error = Some(error);
                return std::ops::ControlFlow::Break(());
            }
        };
        for child in nodes.iter() {
            self.visit_node(child.expect("nil node in source AST utility"))?;
        }
        std::ops::ControlFlow::Continue(())
    }
}

/// port: tsc/internal/ast/utilities.go:getModuleInstanceStateForAliasTarget
fn module_instance_state_for_alias_target(
    view: AstView<'_>,
    id: NodeId,
    mut ancestors: &[NodeId],
    visited: &mut HashMap<u64, ModuleInstanceState>,
) -> Result<ModuleInstanceState, Error> {
    use ModuleInstanceState as State;
    let name = view
        .node(id)?
        .property_name_or_name()
        .expect("nil export name");
    if view.node(name)?.kind() != K::Identifier {
        return Ok(State::Instantiated);
    }
    let mut current = id;
    loop {
        let (rest, p) = pop_ancestor(view, ancestors, current)?;
        ancestors = rest;
        let Some(p) = p else {
            break;
        };
        current = p;
        let parent = view.node(p)?;
        if matches!(
            parent.kind().known(),
            Some(K::Block | K::ModuleBlock | K::SourceFile)
        ) {
            let mut found = State::Unknown;
            let statement_ancestors = push_ancestor(ancestors, p);
            for statement in view.node_slice(parent.statements(view)?)?.iter() {
                let statement = statement.expect("nil statement while resolving module alias");
                if node_has_name(view, statement, name)? {
                    let state = module_instance_state_cached(
                        view,
                        statement,
                        &statement_ancestors,
                        visited,
                    )?;
                    if found == State::Unknown || state > found {
                        found = state;
                    }
                    if found == State::Instantiated {
                        return Ok(found);
                    }
                    if view.node(statement)?.kind() == K::ImportEqualsDeclaration {
                        found = State::Instantiated;
                    }
                }
            }
            if found != State::Unknown {
                return Ok(found);
            }
        }
    }
    Ok(State::Instantiated)
}
/// port: tsc/internal/ast/utilities.go:IsInstantiatedModule
pub fn is_instantiated_module(
    view: AstView<'_>,
    id: NodeId,
    preserve_const_enums: bool,
) -> Result<bool, Error> {
    let state = get_module_instance_state(view, id)?;
    Ok(state == ModuleInstanceState::Instantiated
        || preserve_const_enums && state == ModuleInstanceState::ConstEnumOnly)
}
/// port: tsc/internal/ast/utilities.go:NodeHasName
pub fn node_has_name(view: AstView<'_>, id: NodeId, name: NodeId) -> Result<bool, Error> {
    let node = view.node(id)?;
    if let Some(n) = node.name() {
        return Ok(view.node(n)?.kind() == K::Identifier
            && view.node_text(n)?.as_bytes() == view.node_text(name)?.as_bytes());
    }
    if node.kind() == K::VariableStatement {
        let list = required(view, payload!(node, VariableStatement).declaration_list())?;
        let declarations = payload!(list, VariableDeclarationList)
            .declarations()
            .expect("nil variable declarations");
        for declaration in view.node_slice(view.list(declarations)?.nodes())?.iter() {
            if node_has_name(view, declaration.expect("nil variable declaration"), name)? {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

#[cfg(test)]
#[path = "binder_helpers_tests.rs"]
mod tests;
