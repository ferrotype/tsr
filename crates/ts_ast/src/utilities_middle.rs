//! Syntax utilities from the middle of the pinned AST utility module.
//! Graph reads validate owner-qualified IDs; pure predicates inspect the open
//! kind header, and source nil dereferences remain explicit contract panics.

use crate::NodeAccess;
use crate::{
    modifier_flags, token_flags, AstView, JsString, NodeDataRead, NodeId, NodeKind, NodeListId,
    NodeRead, NodeSlice, Pragma, SourceFileState, SyntaxKind as K,
};
use std::borrow::Cow;
use ts_arena::Error;
use ts_core::{ScriptKind, Tristate};

fn required(view: AstView<'_>, id: Option<NodeId>) -> Result<NodeRead<'_>, Error> {
    view.node(id.expect("nil node in AST utility"))
}

fn list_nodes(view: AstView<'_>, list: Option<NodeListId>) -> Result<NodeSlice, Error> {
    list.map_or(Ok(NodeSlice::empty()), |list| Ok(view.list(list)?.nodes()))
}

// port: tsc/internal/ast/utilities.go:IsJSDocLinkLike
pub fn is_js_doc_link_like(node: &(impl NodeAccess + ?Sized)) -> bool {
    matches!(
        node.kind().known(),
        Some(K::JSDocLink | K::JSDocLinkCode | K::JSDocLinkPlain)
    )
}
// port: tsc/internal/ast/utilities.go:IsJSDocTag
pub fn is_js_doc_tag(node: &(impl NodeAccess + ?Sized)) -> bool {
    (K::FirstJSDocTagNode as i16..=K::LastJSDocTagNode as i16).contains(&node.kind().raw())
}
// port: tsc/internal/ast/utilities.go:IsQuestionToken
pub fn is_question_token(node: Option<&(impl NodeAccess + ?Sized)>) -> bool {
    node.is_some_and(|node| node.kind() == K::QuestionToken)
}
// port: tsc/internal/ast/utilities.go:IsJSDocNode
pub fn is_js_doc_node(node: &(impl NodeAccess + ?Sized)) -> bool {
    (K::FirstJSDocNode as i16..=K::LastJSDocNode as i16).contains(&node.kind().raw())
}
// port: tsc/internal/ast/utilities.go:IsNonWhitespaceToken
pub fn is_non_whitespace_token(node: &(impl NodeAccess + ?Sized)) -> bool {
    crate::is_token_kind(node.kind()) && !is_whitespace_only_jsx_text(node)
}
// port: tsc/internal/ast/utilities.go:IsWhitespaceOnlyJsxText
pub fn is_whitespace_only_jsx_text(node: &(impl NodeAccess + ?Sized)) -> bool {
    node.kind() == K::JsxText
        && node
            .data_source()
            .as_jsx_text()
            .expect("JsxText payload")
            .contains_only_trivia_white_spaces()
}
// port: tsc/internal/ast/utilities.go:IsPropertyAccessOrQualifiedName
pub fn is_property_access_or_qualified_name(node: &(impl NodeAccess + ?Sized)) -> bool {
    matches!(
        node.kind().known(),
        Some(K::PropertyAccessExpression | K::QualifiedName)
    )
}
// port: tsc/internal/ast/utilities.go:IsBreakOrContinueStatement
pub fn is_break_or_continue_statement(node: &(impl NodeAccess + ?Sized)) -> bool {
    matches!(
        node.kind().known(),
        Some(K::BreakStatement | K::ContinueStatement)
    )
}
// port: tsc/internal/ast/utilities.go:IsParameterLike
pub fn is_parameter_like(node: &(impl NodeAccess + ?Sized)) -> bool {
    matches!(node.kind().known(), Some(K::Parameter | K::TypeParameter))
}
// port: tsc/internal/ast/utilities.go:NodeHasKind
pub fn node_has_kind(node: Option<&(impl NodeAccess + ?Sized)>, kind: NodeKind) -> bool {
    node.is_some_and(|node| node.kind() == kind)
}
// port: tsc/internal/ast/utilities.go:IsContextualKeyword
pub fn is_contextual_keyword(kind: NodeKind) -> bool {
    (K::FirstContextualKeyword as i16..=K::LastContextualKeyword as i16).contains(&kind.raw())
}
// port: tsc/internal/ast/utilities.go:IsParameterPropertyModifier
pub fn is_parameter_property_modifier(kind: NodeKind) -> bool {
    crate::modifier_to_flag(kind) & modifier_flags::PARAMETER_PROPERTY_MODIFIER != 0
}
// port: tsc/internal/ast/utilities.go:HasTypeArguments
pub fn has_type_arguments(node: &(impl NodeAccess + ?Sized)) -> bool {
    matches!(
        node.kind().known(),
        Some(
            K::CallExpression
                | K::NewExpression
                | K::TaggedTemplateExpression
                | K::TypeReference
                | K::ExpressionWithTypeArguments
                | K::ImportType
                | K::TypeQuery
                | K::JsxOpeningElement
                | K::JsxSelfClosingElement
        )
    )
}
// port: tsc/internal/ast/utilities.go:IsTypeReferenceType
pub fn is_type_reference_type(node: &(impl NodeAccess + ?Sized)) -> bool {
    matches!(
        node.kind().known(),
        Some(K::TypeReference | K::ExpressionWithTypeArguments)
    )
}
// port: tsc/internal/ast/utilities.go:IsVariableLike
pub fn is_variable_like(node: &(impl NodeAccess + ?Sized)) -> bool {
    matches!(
        node.kind().known(),
        Some(
            K::BindingElement
                | K::EnumMember
                | K::Parameter
                | K::PropertyAssignment
                | K::PropertyDeclaration
                | K::PropertySignature
                | K::ShorthandPropertyAssignment
                | K::VariableDeclaration
        )
    )
}
// port: tsc/internal/ast/utilities.go:HasInitializer
pub fn has_initializer(node: &(impl NodeAccess + ?Sized)) -> bool {
    matches!(
        node.kind().known(),
        Some(
            K::VariableDeclaration
                | K::Parameter
                | K::BindingElement
                | K::PropertyDeclaration
                | K::PropertyAssignment
                | K::EnumMember
                | K::ForStatement
                | K::ForInStatement
                | K::ForOfStatement
                | K::JsxAttribute
        )
    ) && node.initializer().is_some()
}
// port: tsc/internal/ast/utilities.go:IsVariableParameterOrProperty
pub fn is_variable_parameter_or_property(node: &(impl NodeAccess + ?Sized)) -> bool {
    matches!(
        node.kind().known(),
        Some(K::VariableDeclaration | K::Parameter | K::PropertySignature | K::PropertyDeclaration)
    )
}
// port: tsc/internal/ast/utilities.go:IsObjectTypeDeclaration
pub fn is_object_type_declaration(node: &(impl NodeAccess + ?Sized)) -> bool {
    matches!(
        node.kind().known(),
        Some(K::ClassDeclaration | K::ClassExpression | K::InterfaceDeclaration | K::TypeLiteral)
    )
}
// port: tsc/internal/ast/utilities.go:IsTypeKeywordToken
pub fn is_type_keyword_token(node: &(impl NodeAccess + ?Sized)) -> bool {
    node.kind() == K::TypeKeyword
}
// port: tsc/internal/ast/utilities.go:IsResolutionModeOverrideHost
pub fn is_resolution_mode_override_host(node: Option<&(impl NodeAccess + ?Sized)>) -> bool {
    node.is_some_and(|node| {
        matches!(
            node.kind().known(),
            Some(
                K::ImportType
                    | K::ExportDeclaration
                    | K::ImportDeclaration
                    | K::JSImportDeclaration
            )
        )
    })
}
// port: tsc/internal/ast/utilities.go:IsStringTextContainingNode
pub fn is_string_text_containing_node(node: &(impl NodeAccess + ?Sized)) -> bool {
    node.kind() == K::StringLiteral || is_template_literal_kind(node.kind())
}
// port: tsc/internal/ast/utilities.go:IsTemplateLiteralKind
pub fn is_template_literal_kind(kind: NodeKind) -> bool {
    (K::FirstTemplateToken as i16..=K::LastTemplateToken as i16).contains(&kind.raw())
}
// port: tsc/internal/ast/utilities.go:IsTemplateLiteralToken
pub fn is_template_literal_token(node: &(impl NodeAccess + ?Sized)) -> bool {
    is_template_literal_kind(node.kind())
}
// port: tsc/internal/ast/utilities.go:IsLateVisibilityPaintedStatement
pub fn is_late_visibility_painted_statement(node: &(impl NodeAccess + ?Sized)) -> bool {
    matches!(
        node.kind().known(),
        Some(
            K::ImportDeclaration
                | K::JSImportDeclaration
                | K::ImportEqualsDeclaration
                | K::VariableStatement
                | K::ClassDeclaration
                | K::FunctionDeclaration
                | K::ModuleDeclaration
                | K::TypeAliasDeclaration
                | K::JSTypeAliasDeclaration
                | K::InterfaceDeclaration
                | K::EnumDeclaration
        )
    )
}
// port: tsc/internal/ast/utilities.go:IsJsxOpeningLikeElement
pub fn is_jsx_opening_like_element(node: &(impl NodeAccess + ?Sized)) -> bool {
    matches!(
        node.kind().known(),
        Some(K::JsxOpeningElement | K::JsxSelfClosingElement)
    )
}
// port: tsc/internal/ast/utilities.go:IsCallOrNewExpression
pub fn is_call_or_new_expression(node: &(impl NodeAccess + ?Sized)) -> bool {
    matches!(
        node.kind().known(),
        Some(K::CallExpression | K::NewExpression)
    )
}
// port: tsc/internal/ast/utilities.go:IsInitializedProperty
pub fn is_initialized_property(node: &(impl NodeAccess + ?Sized)) -> bool {
    node.kind() == K::PropertyDeclaration && node.initializer().is_some()
}
// port: tsc/internal/ast/utilities.go:IsTrivia
pub fn is_trivia(kind: NodeKind) -> bool {
    (K::FirstTriviaToken as i16..=K::LastTriviaToken as i16).contains(&kind.raw())
}
// port: tsc/internal/ast/utilities.go:hasComment
pub fn has_comment(kind: NodeKind) -> bool {
    matches!(
        kind.known(),
        Some(
            K::JSDoc
                | K::JSDocUnknownTag
                | K::JSDocAugmentsTag
                | K::JSDocImplementsTag
                | K::JSDocDeprecatedTag
                | K::JSDocPublicTag
                | K::JSDocPrivateTag
                | K::JSDocProtectedTag
                | K::JSDocReadonlyTag
                | K::JSDocOverrideTag
                | K::JSDocCallbackTag
                | K::JSDocOverloadTag
                | K::JSDocParameterTag
                | K::JSDocPropertyTag
                | K::JSDocReturnTag
                | K::JSDocThisTag
                | K::JSDocTypeTag
                | K::JSDocTemplateTag
                | K::JSDocTypedefTag
                | K::JSDocSeeTag
                | K::JSDocThrowsTag
                | K::JSDocSatisfiesTag
                | K::JSDocImportTag
        )
    )
}
// port: tsc/internal/ast/utilities.go:IsDeclarationBindingElement
pub fn is_declaration_binding_element(node: &(impl NodeAccess + ?Sized)) -> bool {
    matches!(
        node.kind().known(),
        Some(K::VariableDeclaration | K::Parameter | K::BindingElement)
    )
}
// port: tsc/internal/ast/utilities.go:IsJsxCallLike
pub fn is_jsx_call_like(node: &(impl NodeAccess + ?Sized)) -> bool {
    matches!(
        node.kind().known(),
        Some(K::JsxOpeningElement | K::JsxSelfClosingElement | K::JsxOpeningFragment)
    )
}
// port: tsc/internal/ast/utilities.go:IsPlainJSFile
pub fn is_plain_js_file(file: Option<&SourceFileState>, check_js: Tristate) -> bool {
    file.is_some_and(|file| {
        matches!(file.script_kind, ScriptKind::JS | ScriptKind::JSX)
            && file.check_js_directive.is_none()
            && check_js == Tristate::UNKNOWN
    })
}
// port: tsc/internal/ast/utilities.go:GetPragmaArgument
pub fn get_pragma_argument<'a>(pragma: Option<&'a Pragma>, name: &JsString) -> &'a [u8] {
    pragma
        .and_then(|pragma| pragma.args.get(name))
        .map_or(&[], |argument| argument.value.as_bytes())
}
// port: tsc/internal/ast/utilities.go:CreateModifiersFromModifierFlags
pub fn create_modifiers_from_modifier_flags(
    flags: u32,
    mut create: impl FnMut(NodeKind) -> Option<NodeId>,
) -> Option<Vec<Option<NodeId>>> {
    let mut nodes = Vec::new();
    for (flag, kind) in [
        (modifier_flags::EXPORT, K::ExportKeyword),
        (modifier_flags::AMBIENT, K::DeclareKeyword),
        (modifier_flags::DEFAULT, K::DefaultKeyword),
        (modifier_flags::CONST, K::ConstKeyword),
        (modifier_flags::PUBLIC, K::PublicKeyword),
        (modifier_flags::PRIVATE, K::PrivateKeyword),
        (modifier_flags::PROTECTED, K::ProtectedKeyword),
        (modifier_flags::ABSTRACT, K::AbstractKeyword),
        (modifier_flags::STATIC, K::StaticKeyword),
        (modifier_flags::OVERRIDE, K::OverrideKeyword),
        (modifier_flags::READONLY, K::ReadonlyKeyword),
        (modifier_flags::ACCESSOR, K::AccessorKeyword),
        (modifier_flags::ASYNC, K::AsyncKeyword),
        (modifier_flags::IN, K::InKeyword),
        (modifier_flags::OUT, K::OutKeyword),
    ] {
        if flags & flag != 0 {
            nodes.push(create(kind.into()));
        }
    }
    (!nodes.is_empty()).then_some(nodes)
}
// port: tsc/internal/ast/utilities.go:CompareNodePositions
pub fn compare_node_positions(
    left: &(impl NodeAccess + ?Sized),
    right: &(impl NodeAccess + ?Sized),
) -> i64 {
    let start = i64::from(left.pos()) - i64::from(right.pos());
    if start != 0 {
        start
    } else {
        i64::from(left.end()) - i64::from(right.end())
    }
}
// port: tsc/internal/ast/utilities.go:IndexOfNode
pub fn index_of_node(
    view: AstView<'_>,
    nodes: &[Option<NodeId>],
    target: Option<NodeId>,
) -> Result<i64, Error> {
    let (mut low, mut high) = (0, nodes.len());
    while low < high {
        let middle = low + (high - low) / 2;
        if compare_node_positions(&required(view, nodes[middle])?, &required(view, target)?) < 0 {
            low = middle + 1;
        } else {
            high = middle;
        }
    }
    Ok(
        if low < nodes.len()
            && compare_node_positions(&required(view, nodes[low])?, &required(view, target)?) == 0
        {
            low as i64
        } else {
            -1
        },
    )
}
// port: tsc/internal/ast/utilities.go:IsUnterminatedLiteral
pub fn is_unterminated_literal(node: &(impl NodeAccess + ?Sized)) -> bool {
    if crate::is_literal_kind(node.kind()) {
        let flags = match node.data() {
            NodeDataRead::StringLiteral(data) => data.token_flags(),
            NodeDataRead::NumericLiteral(data) => data.token_flags(),
            NodeDataRead::BigIntLiteral(data) => data.token_flags(),
            NodeDataRead::RegularExpressionLiteral(data) => data.token_flags(),
            NodeDataRead::NoSubstitutionTemplateLiteral(data) => data.token_flags(),
            _ => panic!("LiteralLike payload"),
        };
        if flags & token_flags::UNTERMINATED != 0 {
            return true;
        }
    }
    if !is_template_literal_kind(node.kind()) {
        return false;
    }
    let flags = match node.data() {
        NodeDataRead::NoSubstitutionTemplateLiteral(data) => data.template_flags(),
        NodeDataRead::TemplateHead(data) => data.template_flags(),
        NodeDataRead::TemplateMiddle(data) => data.template_flags(),
        NodeDataRead::TemplateTail(data) => data.template_flags(),
        _ => panic!("TemplateLiteralLike payload"),
    };
    flags & token_flags::UNTERMINATED != 0
}
// port: tsc/internal/ast/utilities.go:IsSuperCall
pub fn is_super_call(view: AstView<'_>, node: &(impl NodeAccess + ?Sized)) -> Result<bool, Error> {
    Ok(node.kind() == K::CallExpression
        && required(view, node.expression())?.kind() == K::SuperKeyword)
}
// port: tsc/internal/ast/utilities.go:IsInternalModuleImportEqualsDeclaration
pub fn is_internal_module_import_equals_declaration(
    view: AstView<'_>,
    node: &(impl NodeAccess + ?Sized),
) -> Result<bool, Error> {
    Ok(node.kind() == K::ImportEqualsDeclaration
        && required(
            view,
            node.data_source()
                .as_import_equals_declaration()
                .expect("ImportEqualsDeclaration payload")
                .module_reference(),
        )?
        .kind()
            != K::ExternalModuleReference)
}
// port: tsc/internal/ast/utilities.go:IsConstTypeReference
pub fn is_const_type_reference(
    view: AstView<'_>,
    node: &(impl NodeAccess + ?Sized),
) -> Result<bool, Error> {
    if node.kind() != K::TypeReference {
        return Ok(false);
    }
    let data = node
        .data_source()
        .as_type_reference_node()
        .expect("TypeReference payload");
    if !view
        .node_slice(list_nodes(view, data.type_arguments())?)?
        .is_empty()
    {
        return Ok(false);
    }
    let name = data.type_name().expect("nil const type name");
    Ok(view.node(name)?.kind() == K::Identifier && view.node_text(name)?.as_bytes() == b"const")
}
// port: tsc/internal/ast/utilities.go:IsConstAssertion
pub fn is_const_assertion(
    view: AstView<'_>,
    node: &(impl NodeAccess + ?Sized),
) -> Result<bool, Error> {
    if matches!(
        node.kind().known(),
        Some(K::AsExpression | K::TypeAssertionExpression)
    ) {
        is_const_type_reference(view, &required(view, node.type_node())?)
    } else {
        Ok(false)
    }
}
// port: tsc/internal/ast/utilities.go:IsGlobalSourceFile
pub fn is_global_source_file(view: AstView<'_>, id: NodeId) -> Result<bool, Error> {
    if view.node(id)?.kind() != K::SourceFile {
        return Ok(false);
    }
    let state = view.source_file(id)?;
    Ok(state.external_module_indicator.is_none() && state.common_js_module_indicator().is_none())
}
// port: tsc/internal/ast/utilities.go:GetFirstIdentifier
pub fn get_first_identifier(view: AstView<'_>, mut id: NodeId) -> Result<NodeId, Error> {
    loop {
        let node = view.node(id)?;
        id = match node.kind().known() {
            Some(K::Identifier) => return Ok(id),
            Some(K::QualifiedName) => node
                .data_source()
                .as_qualified_name()
                .expect("QualifiedName payload")
                .left(),
            Some(K::PropertyAccessExpression) => node.expression(),
            _ => panic!("Unhandled case in GetFirstIdentifier"),
        }
        .expect("nil first identifier chain");
    }
}
// port: tsc/internal/ast/utilities.go:GetNamespaceDeclarationNode
pub fn get_namespace_declaration_node(
    view: AstView<'_>,
    id: NodeId,
) -> Result<Option<NodeId>, Error> {
    let node = view.node(id)?;
    match node.kind().known() {
        Some(K::ImportDeclaration | K::JSImportDeclaration) => {
            let Some(clause) = node.import_clause() else {
                return Ok(None);
            };
            let clause = view.node(clause)?;
            let binding = clause
                .data_source()
                .as_import_clause()
                .expect("ImportClause payload")
                .named_bindings();
            Ok(match binding {
                Some(id) if view.node(id)?.kind() == K::NamespaceImport => Some(id),
                _ => None,
            })
        }
        Some(K::ImportEqualsDeclaration) => Ok(Some(id)),
        Some(K::ExportDeclaration) => {
            let clause = node
                .data_source()
                .as_export_declaration()
                .expect("ExportDeclaration payload")
                .export_clause();
            Ok(match clause {
                Some(id) if view.node(id)?.kind() == K::NamespaceExport => Some(id),
                _ => None,
            })
        }
        _ => panic!("Unhandled case in getNamespaceDeclarationNode"),
    }
}
// port: tsc/internal/ast/utilities.go:ModuleExportNameIsDefault
pub fn module_export_name_is_default(view: AstView<'_>, id: NodeId) -> Result<bool, Error> {
    Ok(view.node_text(id)?.as_bytes() == b"default")
}
// port: tsc/internal/ast/utilities.go:IsDefaultImport
pub fn is_default_import(
    view: AstView<'_>,
    node: &(impl NodeAccess + ?Sized),
) -> Result<bool, Error> {
    if matches!(
        node.kind().known(),
        Some(K::ImportDeclaration | K::JSImportDeclaration)
    ) {
        if let Some(clause) = node.import_clause() {
            return Ok(view
                .node(clause)?
                .data_source()
                .as_import_clause()
                .expect("ImportClause payload")
                .name()
                .is_some());
        }
    }
    Ok(false)
}
// port: tsc/internal/ast/utilities.go:IsCallLikeExpression
pub fn is_call_like_expression(
    view: AstView<'_>,
    node: &(impl NodeAccess + ?Sized),
) -> Result<bool, Error> {
    Ok(match node.kind().known() {
        Some(
            K::JsxOpeningElement
            | K::JsxSelfClosingElement
            | K::JsxOpeningFragment
            | K::CallExpression
            | K::NewExpression
            | K::TaggedTemplateExpression
            | K::Decorator,
        ) => true,
        Some(K::BinaryExpression) => {
            required(
                view,
                node.data_source()
                    .as_binary_expression()
                    .expect("BinaryExpression payload")
                    .operator_token(),
            )?
            .kind()
                == K::InstanceOfKeyword
        }
        _ => false,
    })
}
// port: tsc/internal/ast/utilities.go:IsCallLikeOrFunctionLikeExpression
pub fn is_call_like_or_function_like_expression(
    view: AstView<'_>,
    node: &(impl NodeAccess + ?Sized),
) -> Result<bool, Error> {
    Ok(is_call_like_expression(view, node)?
        || matches!(
            node.kind().known(),
            Some(K::FunctionExpression | K::ArrowFunction)
        ))
}
// port: tsc/internal/ast/utilities.go:GetLeftmostAccessExpression
pub fn get_leftmost_access_expression(view: AstView<'_>, mut id: NodeId) -> Result<NodeId, Error> {
    loop {
        let node = view.node(id)?;
        if !matches!(
            node.kind().known(),
            Some(K::PropertyAccessExpression | K::ElementAccessExpression)
        ) {
            return Ok(id);
        }
        id = node.expression().expect("nil access expression");
    }
}
// port: tsc/internal/ast/utilities.go:IsLabelName
pub fn is_label_name(view: AstView<'_>, id: NodeId) -> Result<bool, Error> {
    Ok(is_label_of_labeled_statement(view, id)? || is_jump_statement_target(view, id)?)
}
// port: tsc/internal/ast/utilities.go:IsLabelOfLabeledStatement
pub fn is_label_of_labeled_statement(view: AstView<'_>, id: NodeId) -> Result<bool, Error> {
    let node = view.node(id)?;
    if node.kind() != K::Identifier {
        return Ok(false);
    }
    let parent = required(view, node.parent())?;
    Ok(parent.kind() == K::LabeledStatement && parent.label() == Some(id))
}
// port: tsc/internal/ast/utilities.go:IsJumpStatementTarget
pub fn is_jump_statement_target(view: AstView<'_>, id: NodeId) -> Result<bool, Error> {
    let node = view.node(id)?;
    if node.kind() != K::Identifier {
        return Ok(false);
    }
    let parent = required(view, node.parent())?;
    Ok(is_break_or_continue_statement(&parent) && parent.label() == Some(id))
}
// port: tsc/internal/ast/utilities.go:IsRightSideOfPropertyAccess
pub fn is_right_side_of_property_access(view: AstView<'_>, id: NodeId) -> Result<bool, Error> {
    let parent = required(view, view.node(id)?.parent())?;
    Ok(parent.kind() == K::PropertyAccessExpression && parent.name() == Some(id))
}
// port: tsc/internal/ast/utilities.go:IsArgumentExpressionOfElementAccess
pub fn is_argument_expression_of_element_access(
    view: AstView<'_>,
    id: NodeId,
) -> Result<bool, Error> {
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
// port: tsc/internal/ast/utilities.go:ClimbPastPropertyAccess
pub fn climb_past_property_access(view: AstView<'_>, id: NodeId) -> Result<NodeId, Error> {
    Ok(if is_right_side_of_property_access(view, id)? {
        view.node(id)?.parent().expect("property parent")
    } else {
        id
    })
}
// port: tsc/internal/ast/utilities.go:climbPastPropertyOrElementAccess
pub fn climb_past_property_or_element_access(
    view: AstView<'_>,
    id: NodeId,
) -> Result<NodeId, Error> {
    Ok(
        if is_right_side_of_property_access(view, id)?
            || is_argument_expression_of_element_access(view, id)?
        {
            view.node(id)?.parent().expect("access parent")
        } else {
            id
        },
    )
}
// port: tsc/internal/ast/utilities.go:selectExpressionOfCallOrNewExpressionOrDecorator
pub fn select_expression_of_call_or_new_expression_or_decorator(
    node: &(impl NodeAccess + ?Sized),
) -> Option<NodeId> {
    if matches!(
        node.kind().known(),
        Some(K::CallExpression | K::NewExpression | K::Decorator)
    ) {
        node.expression()
    } else {
        None
    }
}
// port: tsc/internal/ast/utilities.go:selectTagOfTaggedTemplateExpression
pub fn select_tag_of_tagged_template_expression(
    node: &(impl NodeAccess + ?Sized),
) -> Option<NodeId> {
    if node.kind() == K::TaggedTemplateExpression {
        node.data_source()
            .as_tagged_template_expression()
            .expect("TaggedTemplateExpression payload")
            .tag()
    } else {
        None
    }
}
// port: tsc/internal/ast/utilities.go:selectTagNameOfJsxOpeningLikeElement
pub fn select_tag_name_of_jsx_opening_like_element(
    node: &(impl NodeAccess + ?Sized),
) -> Option<NodeId> {
    if is_jsx_opening_like_element(node) {
        node.tag_name()
    } else {
        None
    }
}
// port: tsc/internal/ast/utilities.go:IsRightSideOfQualifiedNameOrPropertyAccess
pub fn is_right_side_of_qualified_name_or_property_access(
    view: AstView<'_>,
    id: NodeId,
) -> Result<bool, Error> {
    let parent = required(view, view.node(id)?.parent())?;
    Ok(match parent.kind().known() {
        Some(K::QualifiedName) => {
            parent
                .data_source()
                .as_qualified_name()
                .expect("QualifiedName payload")
                .right()
                == Some(id)
        }
        Some(K::PropertyAccessExpression | K::MetaProperty) => parent.name() == Some(id),
        _ => false,
    })
}
// port: tsc/internal/ast/utilities.go:HasQuestionToken
pub fn has_question_token(
    view: AstView<'_>,
    node: &(impl NodeAccess + ?Sized),
) -> Result<bool, Error> {
    Ok(match node.question_token(view)? {
        Some(id) => is_question_token(Some(&view.node(id)?)),
        None => false,
    })
}
// port: tsc/internal/ast/utilities.go:GetInvokedExpression
pub fn get_invoked_expression(view: AstView<'_>, id: NodeId) -> Result<Option<NodeId>, Error> {
    let node = view.node(id)?;
    Ok(match node.kind().known() {
        Some(K::TaggedTemplateExpression) => select_tag_of_tagged_template_expression(&node),
        Some(K::JsxOpeningElement | K::JsxSelfClosingElement) => node.tag_name(),
        Some(K::BinaryExpression) => node
            .data_source()
            .as_binary_expression()
            .expect("BinaryExpression payload")
            .right(),
        Some(K::JsxOpeningFragment) => Some(id),
        _ => node.expression(),
    })
}
// port: tsc/internal/ast/utilities.go:HasDecorators
pub fn has_decorators(view: AstView<'_>, node: &(impl NodeAccess + ?Sized)) -> Result<bool, Error> {
    Ok(node.modifier_flags(view)? & modifier_flags::DECORATOR != 0)
}
// port: tsc/internal/ast/utilities.go:IsJSDocSingleCommentNode
pub fn is_js_doc_single_comment_node(
    view: AstView<'_>,
    node: &(impl NodeAccess + ?Sized),
) -> Result<bool, Error> {
    if !has_comment(node.kind()) {
        return Ok(false);
    }
    Ok(match node.comment_list() {
        Some(list) => view.node_slice(view.list(list)?.nodes())?.len() == 1,
        None => false,
    })
}
// port: tsc/internal/ast/utilities.go:IsJSDocSingleCommentNodeList
pub fn is_js_doc_single_comment_node_list(
    view: AstView<'_>,
    list: Option<NodeListId>,
) -> Result<bool, Error> {
    let Some(list) = list else {
        return Ok(false);
    };
    let nodes = view.node_slice(view.list(list)?.nodes())?;
    let Some(first) = nodes.first() else {
        return Ok(false);
    };
    let Some(parent) = required(view, first)?.parent() else {
        return Ok(false);
    };
    let parent = view.node(parent)?;
    Ok(is_js_doc_single_comment_node(view, &parent)? && parent.comment_list() == Some(list))
}
// port: tsc/internal/ast/utilities.go:IsJSDocSingleCommentNodeComment
pub fn is_js_doc_single_comment_node_comment(
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
    if !is_js_doc_single_comment_node(view, &parent)? {
        return Ok(false);
    }
    Ok(view
        .node_slice(
            view.list(parent.comment_list().expect("single comment list"))?
                .nodes(),
        )?
        .at(0)
        == Some(id))
}
// port: tsc/internal/ast/utilities.go:IsRequireCall
pub fn is_require_call(
    view: AstView<'_>,
    node: &(impl NodeAccess + ?Sized),
    string_literal_argument: bool,
) -> Result<bool, Error> {
    if node.kind() != K::CallExpression {
        return Ok(false);
    }
    let call = node
        .data_source()
        .as_call_expression()
        .expect("CallExpression payload");
    let expression = call.expression().expect("nil require expression");
    if view.node(expression)?.kind() != K::Identifier
        || view.node_text(expression)?.as_bytes() != b"require"
    {
        return Ok(false);
    }
    let arguments = view.list(call.arguments().expect("nil CallExpression arguments"))?;
    let arguments = view.node_slice(arguments.nodes())?;
    if arguments.len() != 1 {
        return Ok(false);
    }
    Ok(!string_literal_argument
        || matches!(
            required(view, arguments.at(0))?.kind().known(),
            Some(K::StringLiteral | K::NoSubstitutionTemplateLiteral)
        ))
}
// port: tsc/internal/ast/utilities.go:GetSemanticJsxChildren
pub fn get_semantic_jsx_children<'a>(
    view: AstView<'_>,
    children: Option<&'a [Option<NodeId>]>,
) -> Result<SemanticJsxChildren<'a>, Error> {
    let Some(children) = children else {
        return Ok(None);
    };
    let keep = |id: Option<NodeId>| -> Result<bool, Error> {
        let node = required(view, id)?;
        Ok(match node.kind().known() {
            Some(K::JsxExpression) => node.expression().is_some(),
            Some(K::JsxText) => !is_whitespace_only_jsx_text(&node),
            _ => true,
        })
    };
    for (index, &child) in children.iter().enumerate() {
        if !keep(child)? {
            let mut result = children[..index].to_vec();
            for &child in &children[index + 1..] {
                if keep(child)? {
                    result.push(child);
                }
            }
            return Ok(Some(Cow::Owned(result)));
        }
    }
    Ok(Some(Cow::Borrowed(children)))
}

/// Preserves a nil input separately from an allocated empty slice.
pub type SemanticJsxChildren<'a> = Option<Cow<'a, [Option<NodeId>]>>;

pub struct FileNameInfo {
    file_name: JsString,
    path: JsString,
}
// port: tsc/internal/ast/utilities.go:NewHasFileName
pub fn new_has_file_name(file_name: JsString, path: JsString) -> FileNameInfo {
    FileNameInfo { file_name, path }
}
impl FileNameInfo {
    // port: tsc/internal/ast/utilities.go:hasFileNameImpl.FileName
    pub fn file_name(&self) -> &[u8] {
        self.file_name.as_bytes()
    }
    // port: tsc/internal/ast/utilities.go:hasFileNameImpl.Path
    pub fn path(&self) -> &[u8] {
        self.path.as_bytes()
    }
}

// port: tsc/internal/ast/ast.go:ImportAttributesNode.GetResolutionModeOverride
/// Read the first resolution-mode attribute without emitting grammar errors.
/// Invalid values return no override, preserving the caller's ordinary mode.
pub fn import_attributes_resolution_mode(
    view: AstView<'_>,
    attributes: Option<NodeId>,
) -> Result<Option<ts_core::ResolutionMode>, Error> {
    import_attributes_resolution_mode_with_invalid_value(view, attributes).map(|(mode, _)| mode)
}

/// Also return the node passed to the native grammar-error callback. Returning
/// its identity lets a checker report after releasing this immutable AST view.
pub fn import_attributes_resolution_mode_with_invalid_value(
    view: AstView<'_>,
    attributes: Option<NodeId>,
) -> Result<(Option<ts_core::ResolutionMode>, Option<NodeId>), Error> {
    let Some(attributes) = attributes else {
        return Ok((None, None));
    };
    let read = view.node(attributes)?;
    let data = read
        .data_source()
        .as_import_attributes()
        .ok_or(Error::InvalidGraph)?;
    let list = data.attributes().ok_or(Error::InvalidGraph)?;
    for node in view.node_slice(view.list(list)?.nodes())?.iter() {
        let read = view.node(node.ok_or(Error::InvalidGraph)?)?;
        let data = read
            .data_source()
            .as_import_attribute()
            .ok_or(Error::InvalidGraph)?;
        let name = data.name().ok_or(Error::InvalidGraph)?;
        if view.node_text(name)?.as_bytes() != b"resolution-mode" {
            continue;
        }
        let value = data.value().ok_or(Error::InvalidGraph)?;
        if !matches!(
            view.node(value)?.kind().known(),
            Some(K::StringLiteral | K::NoSubstitutionTemplateLiteral)
        ) {
            return Ok((None, None));
        }
        return Ok(match view.node_text(value)?.as_bytes() {
            b"import" => (Some(ts_core::ResolutionMode::ESNEXT), None),
            b"require" => (Some(ts_core::ResolutionMode::COMMON_JS), None),
            _ => (None, Some(value)),
        });
    }
    Ok((None, None))
}

#[cfg(test)]
#[path = "utilities_middle_tests.rs"]
mod tests;
