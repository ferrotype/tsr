//! Source-defined AST queries that do not require binding or type checking.
//! Graph reads borrow their owner. Invalid ownership returns `Error`; missing
//! required Go edges and incompatible kind/payload pairs remain contract panics.

use crate::NodeAccess;
use crate::{node_flags, AstView, NodeId, NodeKind, NodeListId, SyntaxKind as K};
use ts_arena::Error;

fn required(id: Option<NodeId>) -> NodeId {
    id.expect("nil node in AST utility")
}

fn list_empty(view: AstView<'_>, list: Option<NodeListId>) -> Result<bool, Error> {
    match list {
        None => Ok(true),
        Some(list) => Ok(view.node_slice(view.list(list)?.nodes())?.is_empty()),
    }
}

// port: tsc/internal/ast/utilities.go:IsEmptyObjectLiteral
pub fn is_empty_object_literal(
    view: AstView<'_>,
    node: &(impl NodeAccess + ?Sized),
) -> Result<bool, Error> {
    if node.kind() != K::ObjectLiteralExpression {
        return Ok(false);
    }
    list_empty(
        view,
        node.data_source()
            .as_object_literal_expression()
            .expect("ObjectLiteralExpression payload")
            .properties(),
    )
}

// port: tsc/internal/ast/utilities.go:IsEmptyArrayLiteral
pub fn is_empty_array_literal(
    view: AstView<'_>,
    node: &(impl NodeAccess + ?Sized),
) -> Result<bool, Error> {
    if node.kind() != K::ArrayLiteralExpression {
        return Ok(false);
    }
    list_empty(
        view,
        node.data_source()
            .as_array_literal_expression()
            .expect("ArrayLiteralExpression payload")
            .elements(),
    )
}

// port: tsc/internal/ast/utilities.go:GetRestIndicatorOfBindingOrAssignmentElement
pub fn get_rest_indicator_of_binding_or_assignment_element(
    view: AstView<'_>,
    id: NodeId,
) -> Result<Option<NodeId>, Error> {
    let node = view.node(id)?;
    Ok(match node.kind().known() {
        Some(K::Parameter) => node
            .data_source()
            .as_parameter_declaration()
            .expect("Parameter payload")
            .dot_dot_dot_token(),
        Some(K::BindingElement) => node
            .data_source()
            .as_binding_element()
            .expect("BindingElement payload")
            .dot_dot_dot_token(),
        Some(K::SpreadElement | K::SpreadAssignment) => Some(id),
        _ => None,
    })
}

// port: tsc/internal/ast/utilities.go:IsJSDocNameReferenceContext
pub fn is_js_doc_name_reference_context(view: AstView<'_>, id: NodeId) -> Result<bool, Error> {
    if view.node(id)?.flags() & node_flags::JS_DOC == 0 {
        return Ok(false);
    }
    let mut current = Some(id);
    while let Some(id) = current {
        let node = view.node(id)?;
        if matches!(
            node.kind().known(),
            Some(K::JSDocNameReference | K::JSDocLink | K::JSDocLinkCode | K::JSDocLinkPlain)
        ) {
            return Ok(true);
        }
        current = node.parent();
    }
    Ok(false)
}

// port: tsc/internal/ast/utilities.go:GetJSDocRoot
pub fn get_js_doc_root(view: AstView<'_>, id: NodeId) -> Result<Option<NodeId>, Error> {
    let mut current = view.node(id)?.parent();
    while let Some(id) = current {
        let node = view.node(id)?;
        if node.kind() == K::JSDoc {
            return Ok(Some(id));
        }
        current = node.parent();
    }
    Ok(None)
}

// port: tsc/internal/ast/utilities.go:GetJSDocHost
pub fn get_js_doc_host(view: AstView<'_>, id: NodeId) -> Result<Option<NodeId>, Error> {
    get_js_doc_root(view, id)?
        .map(|root| Ok(view.node(root)?.parent()))
        .unwrap_or(Ok(None))
}

// port: tsc/internal/ast/utilities.go:GetNextJSDocCommentLocation
pub fn get_next_js_doc_comment_location(
    view: AstView<'_>,
    id: NodeId,
) -> Result<Option<NodeId>, Error> {
    let Some(parent) = view.node(id)?.parent() else {
        return Ok(None);
    };
    let node = view.node(parent)?;
    match node.kind().known() {
        Some(
            K::PropertyAssignment
            | K::ExportAssignment
            | K::PropertyDeclaration
            | K::VariableDeclaration
            | K::SatisfiesExpression
            | K::ReturnStatement
            | K::VariableStatement
            | K::ExpressionStatement,
        ) => Ok(Some(parent)),
        Some(K::VariableDeclarationList) => {
            let list = node
                .data_source()
                .as_variable_declaration_list()
                .expect("VariableDeclarationList payload")
                .declarations()
                .expect("nil declarations in GetNextJSDocCommentLocation");
            Ok((view.node_slice(view.list(list)?.nodes())?.at(0) == Some(id)).then_some(parent))
        }
        _ => Ok(None),
    }
}

// port: tsc/internal/ast/utilities.go:IsImportOrImportEqualsDeclaration
pub fn is_import_or_import_equals_declaration(node: &(impl NodeAccess + ?Sized)) -> bool {
    matches!(
        node.kind().known(),
        Some(K::ImportDeclaration | K::ImportEqualsDeclaration)
    )
}

// port: tsc/internal/ast/utilities.go:IsPrimitiveLiteralValue
pub fn is_primitive_literal_value(
    view: AstView<'_>,
    node: &(impl NodeAccess + ?Sized),
    include_big_int: bool,
) -> Result<bool, Error> {
    Ok(match node.kind().known() {
        Some(
            K::TrueKeyword
            | K::FalseKeyword
            | K::NumericLiteral
            | K::StringLiteral
            | K::NoSubstitutionTemplateLiteral,
        ) => true,
        Some(K::BigIntLiteral) => include_big_int,
        Some(K::PrefixUnaryExpression) => {
            let data = node
                .data_source()
                .as_prefix_unary_expression()
                .expect("PrefixUnaryExpression payload");
            if data.operator() == K::MinusToken || data.operator() == K::PlusToken {
                let kind = view.node(required(data.operand()))?.kind();
                kind == K::NumericLiteral
                    || (data.operator() == K::MinusToken
                        && include_big_int
                        && kind == K::BigIntLiteral)
            } else {
                false
            }
        }
        _ => false,
    })
}

// port: tsc/internal/ast/utilities.go:HasInferredType
pub fn has_inferred_type(node: &(impl NodeAccess + ?Sized)) -> bool {
    matches!(
        node.kind().known(),
        Some(
            K::Parameter
                | K::PropertySignature
                | K::PropertyDeclaration
                | K::BindingElement
                | K::PropertyAccessExpression
                | K::ElementAccessExpression
                | K::BinaryExpression
                | K::CallExpression
                | K::VariableDeclaration
                | K::ExportAssignment
                | K::PropertyAssignment
                | K::ShorthandPropertyAssignment
                | K::JSDocParameterTag
                | K::JSDocPropertyTag
        )
    )
}

// port: tsc/internal/ast/utilities.go:IsKeyword
pub fn is_keyword(kind: NodeKind) -> bool {
    (K::FirstKeyword as i16..=K::LastKeyword as i16).contains(&kind.raw())
}

// port: tsc/internal/ast/utilities.go:IsNonContextualKeyword
pub fn is_non_contextual_keyword(kind: NodeKind) -> bool {
    is_keyword(kind)
        && !(K::FirstContextualKeyword as i16..=K::LastContextualKeyword as i16)
            .contains(&kind.raw())
}

// port: tsc/internal/ast/utilities.go:IsInfinityOrNaNString
pub fn is_infinity_or_na_n_string(name: &[u8]) -> bool {
    matches!(name, b"Infinity" | b"-Infinity" | b"NaN")
}

// port: tsc/internal/ast/utilities.go:GetRestParameterElementType
pub fn get_rest_parameter_element_type(
    view: AstView<'_>,
    id: Option<NodeId>,
) -> Result<Option<NodeId>, Error> {
    let Some(id) = id else {
        return Ok(None);
    };
    let node = view.node(id)?;
    Ok(match node.kind().known() {
        Some(K::ArrayType) => node
            .data_source()
            .as_array_type_node()
            .expect("ArrayType payload")
            .element_type(),
        Some(K::TypeReference) => match node
            .data_source()
            .as_type_reference_node()
            .expect("TypeReference payload")
            .type_arguments()
        {
            Some(list) => view.node_slice(view.list(list)?.nodes())?.first().flatten(),
            None => None,
        },
        _ => None,
    })
}

fn text_equal(
    view: AstView<'_>,
    left: Option<NodeId>,
    right: Option<NodeId>,
) -> Result<bool, Error> {
    Ok(view.node_text(required(left))?.as_bytes() == view.node_text(required(right))?.as_bytes())
}

// port: tsc/internal/ast/utilities.go:TagNamesAreEquivalent
pub fn tag_names_are_equivalent(
    view: AstView<'_>,
    mut lhs: NodeId,
    mut rhs: NodeId,
) -> Result<bool, Error> {
    loop {
        let left = view.node(lhs)?;
        let right = view.node(rhs)?;
        if left.kind() != right.kind() {
            return Ok(false);
        }
        match left.kind().known() {
            Some(K::Identifier) => return text_equal(view, Some(lhs), Some(rhs)),
            Some(K::ThisKeyword) => return Ok(true),
            Some(K::JsxNamespacedName) => {
                let left = left
                    .data_source()
                    .as_jsx_namespaced_name()
                    .expect("JsxNamespacedName payload");
                let right = right
                    .data_source()
                    .as_jsx_namespaced_name()
                    .expect("JsxNamespacedName payload");
                return Ok(text_equal(view, left.namespace(), right.namespace())?
                    && text_equal(view, left.name(), right.name())?);
            }
            Some(K::PropertyAccessExpression) => {
                let left = left
                    .data_source()
                    .as_property_access_expression()
                    .expect("PropertyAccessExpression payload");
                let right = right
                    .data_source()
                    .as_property_access_expression()
                    .expect("PropertyAccessExpression payload");
                if !text_equal(view, left.name(), right.name())? {
                    return Ok(false);
                }
                lhs = required(left.expression());
                rhs = required(right.expression());
            }
            _ => panic!("Unhandled case in TagNamesAreEquivalent"),
        }
    }
}

// port: tsc/internal/ast/utilities.go:IsTagName
pub fn is_tag_name(view: AstView<'_>, id: NodeId) -> Result<bool, Error> {
    let Some(parent) = view.node(id)?.parent() else {
        return Ok(false);
    };
    let parent = view.node(parent)?;
    if !(K::FirstJSDocTagNode as i16..=K::LastJSDocTagNode as i16).contains(&parent.kind().raw()) {
        return Ok(false);
    }
    let tag_name = parent.tag_name();
    Ok(tag_name == Some(id))
}

// port: tsc/internal/ast/utilities.go:isArgumentOfElementAccessExpression
pub fn is_argument_of_element_access_expression(
    view: AstView<'_>,
    id: Option<NodeId>,
) -> Result<bool, Error> {
    let Some(id) = id else {
        return Ok(false);
    };
    let Some(parent) = view.node(id)?.parent() else {
        return Ok(false);
    };
    let parent = view.node(parent)?;
    Ok(parent.kind() == K::ElementAccessExpression
        && parent
            .data_source()
            .as_element_access_expression()
            .expect("ElementAccessExpression payload")
            .argument_expression()
            == Some(id))
}

// port: tsc/internal/ast/utilities.go:IsExpandoPropertyDeclaration
pub fn is_expando_property_declaration(node: Option<&(impl NodeAccess + ?Sized)>) -> bool {
    node.is_some_and(|node| node.kind() == K::BinaryExpression)
}

// port: tsc/internal/ast/utilities.go:IsSuperProperty
pub fn is_super_property(
    view: AstView<'_>,
    node: &(impl NodeAccess + ?Sized),
) -> Result<bool, Error> {
    let expression = match node.kind().known() {
        Some(K::PropertyAccessExpression) => node
            .data_source()
            .as_property_access_expression()
            .expect("PropertyAccessExpression payload")
            .expression(),
        Some(K::ElementAccessExpression) => node
            .data_source()
            .as_element_access_expression()
            .expect("ElementAccessExpression payload")
            .expression(),
        _ => return Ok(false),
    };
    Ok(view.node(required(expression))?.kind() == K::SuperKeyword)
}

// port: tsc/internal/ast/utilities.go:IsNamedEvaluationSource
pub fn is_named_evaluation_source(
    view: AstView<'_>,
    node: &(impl NodeAccess + ?Sized),
) -> Result<bool, Error> {
    let (name, initializer, rest) = match node.kind().known() {
        Some(K::PropertyAssignment) => {
            return Ok(!is_proto_setter(
                &view.node(required(
                    node.data_source()
                        .as_property_assignment()
                        .expect("PropertyAssignment payload")
                        .name(),
                ))?,
            ))
        }
        Some(K::ShorthandPropertyAssignment) => {
            return Ok(node
                .data_source()
                .as_shorthand_property_assignment()
                .expect("ShorthandPropertyAssignment payload")
                .object_assignment_initializer()
                .is_some())
        }
        Some(K::VariableDeclaration) => {
            let data = node
                .data_source()
                .as_variable_declaration()
                .expect("VariableDeclaration payload");
            (data.name(), data.initializer(), None)
        }
        Some(K::Parameter) => {
            let data = node
                .data_source()
                .as_parameter_declaration()
                .expect("Parameter payload");
            (data.name(), data.initializer(), data.dot_dot_dot_token())
        }
        Some(K::BindingElement) => {
            let data = node
                .data_source()
                .as_binding_element()
                .expect("BindingElement payload");
            (data.name(), data.initializer(), data.dot_dot_dot_token())
        }
        Some(K::PropertyDeclaration) => {
            return Ok(node
                .data_source()
                .as_property_declaration()
                .expect("PropertyDeclaration payload")
                .initializer()
                .is_some())
        }
        Some(K::BinaryExpression) => {
            let data = node
                .data_source()
                .as_binary_expression()
                .expect("BinaryExpression payload");
            return Ok(matches!(
                view.node(required(data.operator_token()))?.kind().known(),
                Some(
                    K::EqualsToken
                        | K::AmpersandAmpersandEqualsToken
                        | K::BarBarEqualsToken
                        | K::QuestionQuestionEqualsToken
                )
            ) && view.node(required(data.left()))?.kind() == K::Identifier);
        }
        Some(K::ExportAssignment) => return Ok(true),
        _ => return Ok(false),
    };
    Ok(view.node(required(name))?.kind() == K::Identifier
        && initializer.is_some()
        && rest.is_none())
}

// port: tsc/internal/ast/utilities.go:IsProtoSetter
pub fn is_proto_setter(node: &(impl NodeAccess + ?Sized)) -> bool {
    let text = match node.kind().known() {
        Some(K::Identifier) => node
            .data_source()
            .as_identifier()
            .expect("Identifier payload")
            .text(),
        Some(K::StringLiteral) => node
            .data_source()
            .as_string_literal()
            .expect("StringLiteral payload")
            .text(),
        _ => return false,
    };
    text == b"__proto__"
}

// port: tsc/internal/ast/utilities.go:IsStringLiteralLikeType
pub fn is_string_literal_like_type(
    view: AstView<'_>,
    node: &(impl NodeAccess + ?Sized),
) -> Result<bool, Error> {
    if node.kind() != K::LiteralType {
        return Ok(false);
    }
    let literal = node
        .data_source()
        .as_literal_type_node()
        .expect("LiteralType payload")
        .literal();
    Ok(matches!(
        view.node(required(literal))?.kind().known(),
        Some(K::StringLiteral | K::NoSubstitutionTemplateLiteral)
    ))
}
