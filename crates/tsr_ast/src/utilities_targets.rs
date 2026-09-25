//! AST utilities: callee targets, JSX tags, JSDoc deprecation and member
//! helpers.
//!
//! Ports of `tsc/internal/ast/utilities.go`, witnessed by the `targets` group of the Phase 1
//! operation tables (`docs/PHASE1-mutation-witnesses.md`, section 9).
use crate::{AstView, NodeId, SyntaxKind as K};
use tsr_arena::Error;

const NIL: &str = "runtime error: invalid memory address or nil pointer dereference";

/// Go reads `node.Parent.Kind`, so a parentless node panics as Go's nil
/// dereference does.
/// port: tsc/internal/ast/utilities.go:IsJsxTagName
pub fn is_jsx_tag_name(view: AstView<'_>, node: NodeId) -> Result<bool, Error> {
    let parent = view.node(view.node(node)?.parent().expect(NIL))?;
    Ok(matches!(
        parent.kind().known(),
        Some(K::JsxOpeningElement | K::JsxClosingElement | K::JsxSelfClosingElement)
    ) && parent.tag_name() == Some(node))
}

/// Go's `getTextOfNode` argument: the text a caller reads for a parsed name.
pub type TextOfNode<'f> = &'f dyn Fn(NodeId) -> Result<Vec<u8>, Error>;

/// Recurses once per qualified-name, property-access or namespaced-name level.
/// port: tsc/internal/ast/utilities.go:EntityNameToString
pub fn entity_name_to_string(
    view: AstView<'_>,
    name: NodeId,
    get_text_of_node: Option<TextOfNode<'_>>,
) -> Result<Vec<u8>, Error> {
    stacker::maybe_grow(64 * 1024, 1024 * 1024, || {
        let node = view.node(name)?;
        let joined = |left: Option<NodeId>, separator: u8, right: Option<NodeId>| {
            let mut text = entity_name_to_string(view, left.expect(NIL), get_text_of_node)?;
            text.push(separator);
            text.extend(entity_name_to_string(
                view,
                right.expect(NIL),
                get_text_of_node,
            )?);
            Ok(text)
        };
        match node.kind().known() {
            Some(K::ThisKeyword) => Ok(b"this".to_vec()),
            Some(K::Identifier | K::PrivateIdentifier) => match get_text_of_node {
                Some(text_of) if !crate::utilities::node_is_synthesized(&node) => text_of(name),
                _ => Ok(view.node_text(name)?.as_bytes().to_vec()),
            },
            Some(K::QualifiedName) => {
                let qualified = node
                    .data_source()
                    .as_qualified_name()
                    .ok_or(Error::InvalidGraph)?;
                joined(qualified.left(), b'.', qualified.right())
            }
            Some(K::PropertyAccessExpression) => joined(node.expression(), b'.', node.name()),
            Some(K::JsxNamespacedName) => {
                let namespaced = node
                    .data_source()
                    .as_jsx_namespaced_name()
                    .ok_or(Error::InvalidGraph)?;
                joined(namespaced.namespace(), b':', node.name())
            }
            _ => panic!("Unhandled case in EntityNameToString"),
        }
    })
}

/// port: tsc/internal/ast/utilities.go:GetHostSignatureFromJSDoc
pub fn get_host_signature_from_js_doc(
    view: AstView<'_>,
    node: NodeId,
) -> Result<Option<NodeId>, Error> {
    let Some(host) = crate::utilities_tail::get_js_doc_host(view, node)? else {
        return Ok(None);
    };
    let read = view.node(host)?;
    if crate::is_property_signature_declaration(&read) {
        if let Some(type_node) = read.type_node() {
            if crate::utilities::is_function_like(Some(&view.node(type_node)?)) {
                return Ok(Some(type_node));
            }
        }
    }
    Ok(crate::utilities::is_function_like(Some(&read)).then_some(host))
}

/// `source` is the file the node belongs to, whose lazy JSDoc `jsdoc` parses.
/// port: tsc/internal/ast/utilities.go:GetJSDocDeprecatedTag
pub fn get_js_doc_deprecated_tag(
    view: AstView<'_>,
    jsdoc: &mut dyn crate::JsDocProvider,
    source: NodeId,
    node: NodeId,
) -> Result<Option<NodeId>, Error> {
    for doc in jsdoc.jsdoc(view, source, node)?.to_vec() {
        let tags = view
            .node(doc)?
            .data_source()
            .as_js_doc()
            .ok_or(Error::InvalidGraph)?
            .tags();
        let Some(tags) = tags else {
            continue;
        };
        for tag in view.node_slice(view.list(tags)?.nodes())?.iter().flatten() {
            if crate::is_js_doc_deprecated_tag(&view.node(tag)?) {
                return Ok(Some(tag));
            }
        }
    }
    Ok(None)
}

/// port: tsc/internal/ast/utilities.go:GetPropertyNameForPropertyNameNode
pub fn get_property_name_for_property_name_node(
    view: AstView<'_>,
    name: NodeId,
) -> Result<Vec<u8>, Error> {
    let node = view.node(name)?;
    match node.kind().known() {
        Some(
            K::Identifier
            | K::PrivateIdentifier
            | K::StringLiteral
            | K::NoSubstitutionTemplateLiteral
            | K::NumericLiteral
            | K::BigIntLiteral
            | K::JsxNamespacedName,
        ) => Ok(view.node_text(name)?.as_bytes().to_vec()),
        Some(K::ComputedPropertyName) => {
            let expression = node.expression().expect(NIL);
            if crate::utilities::is_string_or_numeric_literal_like(&view.node(expression)?) {
                return Ok(view.node_text(expression)?.as_bytes().to_vec());
            }
            if crate::utilities::is_signed_numeric_literal(view, expression)? {
                let read = view.node(expression)?;
                let prefix = read
                    .data_source()
                    .as_prefix_unary_expression()
                    .ok_or(Error::InvalidGraph)?;
                let mut text = view
                    .node_text(prefix.operand().expect(NIL))?
                    .as_bytes()
                    .to_vec();
                if prefix.operator() == K::MinusToken {
                    text.insert(0, b'-');
                }
                return Ok(text);
            }
            Ok(crate::symbols::internal_symbol_names::MISSING.to_vec())
        }
        _ => panic!("Unhandled case in getPropertyNameForPropertyNameNode"),
    }
}

/// port: tsc/internal/ast/utilities.go:GetTypeAnnotationNode
pub fn get_type_annotation_node(view: AstView<'_>, node: NodeId) -> Result<Option<NodeId>, Error> {
    let read = view.node(node)?;
    Ok(match read.kind().known() {
        Some(
            K::VariableDeclaration
            | K::Parameter
            | K::PropertySignature
            | K::PropertyDeclaration
            | K::TypePredicate
            | K::ParenthesizedType
            | K::TypeOperator
            | K::MappedType
            | K::TypeAssertionExpression
            | K::AsExpression
            | K::SatisfiesExpression
            | K::TypeAliasDeclaration
            | K::JSTypeAliasDeclaration
            | K::NamedTupleMember
            | K::OptionalType
            | K::RestType
            | K::TemplateLiteralTypeSpan
            | K::JSDocTypeExpression
            | K::JSDocPropertyTag
            | K::JSDocNullableType
            | K::JSDocNonNullableType
            | K::JSDocOptionalType
            // The kinds whose data embeds FunctionLikeBase.
            | K::GetAccessor
            | K::SetAccessor
            | K::ArrowFunction
            | K::CallSignature
            | K::ConstructSignature
            | K::Constructor
            | K::ConstructorType
            | K::FunctionDeclaration
            | K::FunctionExpression
            | K::FunctionType
            | K::IndexSignature
            | K::JSDocSignature
            | K::MethodDeclaration
            | K::MethodSignature,
        ) => read.type_node(),
        _ => None,
    })
}

/// port: tsc/internal/ast/utilities.go:IsCallExpressionTarget
pub fn is_call_expression_target(
    view: AstView<'_>,
    node: NodeId,
    include_element_access: bool,
    skip_past_outer_expressions: bool,
) -> Result<bool, Error> {
    is_callee_worker(
        view,
        node,
        |node| crate::is_call_expression(node),
        |node| {
            crate::utilities_middle::select_expression_of_call_or_new_expression_or_decorator(node)
        },
        include_element_access,
        skip_past_outer_expressions,
    )
}

/// port: tsc/internal/ast/utilities.go:IsNewExpressionTarget
pub fn is_new_expression_target(
    view: AstView<'_>,
    node: NodeId,
    include_element_access: bool,
    skip_past_outer_expressions: bool,
) -> Result<bool, Error> {
    is_callee_worker(
        view,
        node,
        |node| crate::is_new_expression(node),
        |node| {
            crate::utilities_middle::select_expression_of_call_or_new_expression_or_decorator(node)
        },
        include_element_access,
        skip_past_outer_expressions,
    )
}

/// port: tsc/internal/ast/utilities.go:IsCallOrNewExpressionTarget
pub fn is_call_or_new_expression_target(
    view: AstView<'_>,
    node: NodeId,
    include_element_access: bool,
    skip_past_outer_expressions: bool,
) -> Result<bool, Error> {
    is_callee_worker(
        view,
        node,
        |node| crate::utilities_middle::is_call_or_new_expression(node),
        |node| {
            crate::utilities_middle::select_expression_of_call_or_new_expression_or_decorator(node)
        },
        include_element_access,
        skip_past_outer_expressions,
    )
}

/// port: tsc/internal/ast/utilities.go:IsDecoratorTarget
pub fn is_decorator_target(
    view: AstView<'_>,
    node: NodeId,
    include_element_access: bool,
    skip_past_outer_expressions: bool,
) -> Result<bool, Error> {
    is_callee_worker(
        view,
        node,
        |node| crate::is_decorator(node),
        |node| {
            crate::utilities_middle::select_expression_of_call_or_new_expression_or_decorator(node)
        },
        include_element_access,
        skip_past_outer_expressions,
    )
}

/// port: tsc/internal/ast/utilities.go:IsTaggedTemplateTag
pub fn is_tagged_template_tag(
    view: AstView<'_>,
    node: NodeId,
    include_element_access: bool,
    skip_past_outer_expressions: bool,
) -> Result<bool, Error> {
    is_callee_worker(
        view,
        node,
        |node| crate::is_tagged_template_expression(node),
        |node| crate::utilities_middle::select_tag_of_tagged_template_expression(node),
        include_element_access,
        skip_past_outer_expressions,
    )
}

/// port: tsc/internal/ast/utilities.go:IsJsxOpeningLikeElementTagName
pub fn is_jsx_opening_like_element_tag_name(
    view: AstView<'_>,
    node: NodeId,
    include_element_access: bool,
    skip_past_outer_expressions: bool,
) -> Result<bool, Error> {
    is_callee_worker(
        view,
        node,
        |node| crate::utilities_middle::is_jsx_opening_like_element(node),
        |node| crate::utilities_middle::select_tag_name_of_jsx_opening_like_element(node),
        include_element_access,
        skip_past_outer_expressions,
    )
}

/// port: tsc/internal/ast/utilities.go:isCalleeWorker
fn is_callee_worker(
    view: AstView<'_>,
    node: NodeId,
    pred: fn(&crate::NodeRead<'_>) -> bool,
    callee_selector: fn(&crate::NodeRead<'_>) -> Option<NodeId>,
    include_element_access: bool,
    skip_past_outer_expressions: bool,
) -> Result<bool, Error> {
    let mut target = if include_element_access {
        crate::utilities_middle::climb_past_property_or_element_access(view, node)?
    } else {
        crate::utilities_middle::climb_past_property_access(view, node)?
    };
    if skip_past_outer_expressions && crate::utilities_positions::is_expression(view, target)? {
        target = crate::utilities::skip_outer_expressions(
            view,
            target,
            crate::utilities::outer_expression_kinds::ALL,
        )?;
    }
    let Some(parent) = view.node(target)?.parent() else {
        return Ok(false);
    };
    let parent = view.node(parent)?;
    Ok(pred(&parent) && callee_selector(&parent) == Some(target))
}

/// `jsdoc` and `source` read the JSDoc of the declaration's file.
/// port: tsc/internal/ast/utilities.go:IsDeprecatedDeclaration
pub fn is_deprecated_declaration(
    view: AstView<'_>,
    jsdoc: &mut dyn crate::JsDocProvider,
    source: NodeId,
    declaration: NodeId,
) -> Result<bool, Error> {
    let flags = crate::utilities::get_combined_node_flags(view, declaration)?;
    is_deprecated_declaration_with_cached_flags(view, jsdoc, source, declaration, flags)
}

/// port: tsc/internal/ast/utilities.go:IsDeprecatedDeclarationWithCachedFlags
pub fn is_deprecated_declaration_with_cached_flags(
    view: AstView<'_>,
    jsdoc: &mut dyn crate::JsDocProvider,
    source: NodeId,
    declaration: NodeId,
    combined_flags: u32,
) -> Result<bool, Error> {
    if combined_flags & crate::node_flags::POSSIBLY_CONTAINS_DEPRECATED_TAG == 0 {
        return Ok(false);
    }
    let mut current = Some(declaration);
    while let Some(node) = current {
        let read = view.node(node)?;
        if read.flags() & crate::node_flags::POSSIBLY_CONTAINS_DEPRECATED_TAG != 0 {
            return Ok(get_js_doc_deprecated_tag(view, jsdoc, source, node)?.is_some());
        }
        current = read.parent();
    }
    Ok(false)
}

/// Recurses once per nested label.
/// port: tsc/internal/ast/utilities.go:IsIterationStatement
pub fn is_iteration_statement(
    view: AstView<'_>,
    node: NodeId,
    look_in_labeled_statements: bool,
) -> Result<bool, Error> {
    let read = view.node(node)?;
    Ok(match read.kind().known() {
        Some(
            K::ForStatement
            | K::ForInStatement
            | K::ForOfStatement
            | K::DoStatement
            | K::WhileStatement,
        ) => true,
        Some(K::LabeledStatement) => {
            look_in_labeled_statements
                && stacker::maybe_grow(64 * 1024, 1024 * 1024, || {
                    is_iteration_statement(
                        view,
                        read.statement().expect(NIL),
                        look_in_labeled_statements,
                    )
                })?
        }
        _ => false,
    })
}

/// port: tsc/internal/ast/utilities.go:IsLet
pub fn is_let(view: AstView<'_>, node: NodeId) -> Result<bool, Error> {
    Ok(
        crate::utilities::get_combined_node_flags(view, node)? & crate::node_flags::BLOCK_SCOPED
            == crate::node_flags::LET,
    )
}

/// port: tsc/internal/ast/utilities.go:IsPrototypeAccess
pub fn is_prototype_access(view: AstView<'_>, node: NodeId) -> Result<bool, Error> {
    if crate::binder_helpers::is_bindable_static_access_expression(view, node, false)? {
        if let Some(name) = crate::binder_helpers::get_element_or_property_access_name(view, node)?
        {
            return Ok(view.node_text(name)?.as_bytes() == b"prototype");
        }
    }
    Ok(false)
}

/// Go's `(string, bool)` as an option.
/// port: tsc/internal/ast/utilities.go:TryGetTextOfPropertyName
pub fn try_get_text_of_property_name(
    view: AstView<'_>,
    name: NodeId,
) -> Result<Option<Vec<u8>>, Error> {
    let read = view.node(name)?;
    Ok(match read.kind().known() {
        Some(
            K::Identifier
            | K::PrivateIdentifier
            | K::StringLiteral
            | K::NumericLiteral
            | K::BigIntLiteral
            | K::NoSubstitutionTemplateLiteral,
        ) => Some(view.node_text(name)?.as_bytes().to_vec()),
        Some(K::ComputedPropertyName) => {
            let expression = read.expression().expect(NIL);
            if crate::utilities::is_string_or_numeric_literal_like(&view.node(expression)?) {
                Some(view.node_text(expression)?.as_bytes().to_vec())
            } else {
                None
            }
        }
        Some(K::JsxNamespacedName) => {
            let namespace = read
                .data_source()
                .as_jsx_namespaced_name()
                .ok_or(Error::InvalidGraph)?
                .namespace()
                .expect(NIL);
            let mut text = view.node_text(namespace)?.as_bytes().to_vec();
            text.push(b':');
            text.extend_from_slice(view.node_text(read.name().expect(NIL))?.as_bytes());
            Some(text)
        }
        _ => None,
    })
}

/// port: tsc/internal/ast/utilities.go:GetTextOfPropertyName
pub fn get_text_of_property_name(view: AstView<'_>, name: NodeId) -> Result<Vec<u8>, Error> {
    Ok(try_get_text_of_property_name(view, name)?.unwrap_or_default())
}

/// port: tsc/internal/ast/utilities.go:IsComputedNonLiteralName
pub fn is_computed_non_literal_name(view: AstView<'_>, name: NodeId) -> Result<bool, Error> {
    let read = view.node(name)?;
    Ok(read.kind() == K::ComputedPropertyName
        && !crate::utilities::is_string_or_numeric_literal_like(
            &view.node(read.expression().expect(NIL))?,
        ))
}
