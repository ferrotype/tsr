//! Source syntax classification and parent/edge utilities. Graph reads retain
//! the caller's ownership checks; pure predicates preserve the open Kind domain.
use crate::NodeAccess;
use crate::{
    modifier_flags, node_flags, AstView, NodeId, NodeKind, NodeRead, SourceFileRead,
    SourceFileState, SyntaxKind as K,
};
use ts_arena::Error;
use ts_core::TextRange;

/// port: tsc/internal/ast/utilities.go:IsObjectBindingOrAssignmentElement
pub fn is_object_binding_or_assignment_element(node: &(impl NodeAccess + ?Sized)) -> bool {
    matches!(
        node.kind().known(),
        Some(
            K::BindingElement
                | K::PropertyAssignment
                | K::ShorthandPropertyAssignment
                | K::SpreadAssignment
        )
    )
}

/// port: tsc/internal/ast/utilities.go:IsPropertyNameLiteral
pub fn is_property_name_literal(node: &(impl NodeAccess + ?Sized)) -> bool {
    is_property_name_literal_kind(node.kind())
}

pub fn is_property_name_literal_kind(kind: NodeKind) -> bool {
    matches!(
        kind.known(),
        Some(
            K::Identifier | K::StringLiteral | K::NoSubstitutionTemplateLiteral | K::NumericLiteral
        )
    )
}

/// port: tsc/internal/ast/utilities.go:IsPropertyName
pub fn is_property_name(node: &(impl NodeAccess + ?Sized)) -> bool {
    matches!(
        node.kind().known(),
        Some(
            K::Identifier
                | K::PrivateIdentifier
                | K::StringLiteral
                | K::NumericLiteral
                | K::ComputedPropertyName
        )
    )
}

/// port: tsc/internal/ast/utilities.go:IsClassElement
pub fn is_class_element(node: &(impl NodeAccess + ?Sized)) -> bool {
    matches!(
        node.kind().known(),
        Some(
            K::Constructor
                | K::PropertyDeclaration
                | K::MethodDeclaration
                | K::GetAccessor
                | K::SetAccessor
                | K::IndexSignature
                | K::ClassStaticBlockDeclaration
                | K::SemicolonClassElement
        )
    )
}

/// port: tsc/internal/ast/utilities.go:IsMethodOrAccessor
pub fn is_method_or_accessor(node: &(impl NodeAccess + ?Sized)) -> bool {
    is_method_or_accessor_kind(node.kind())
}

pub fn is_method_or_accessor_kind(kind: NodeKind) -> bool {
    matches!(
        kind.known(),
        Some(K::MethodDeclaration | K::GetAccessor | K::SetAccessor)
    )
}

/// port: tsc/internal/ast/utilities.go:IsTypeElement
pub fn is_type_element(node: &(impl NodeAccess + ?Sized)) -> bool {
    matches!(
        node.kind().known(),
        Some(
            K::ConstructSignature
                | K::CallSignature
                | K::PropertySignature
                | K::MethodSignature
                | K::IndexSignature
                | K::GetAccessor
                | K::SetAccessor
                | K::NotEmittedTypeElement
        )
    )
}

/// port: tsc/internal/ast/utilities.go:IsObjectLiteralElement
pub fn is_object_literal_element(node: &(impl NodeAccess + ?Sized)) -> bool {
    matches!(
        node.kind().known(),
        Some(
            K::PropertyAssignment
                | K::ShorthandPropertyAssignment
                | K::SpreadAssignment
                | K::MethodDeclaration
                | K::GetAccessor
                | K::SetAccessor
        )
    )
}

/// port: tsc/internal/ast/utilities.go:IsJsxChild
pub fn is_jsx_child(node: &(impl NodeAccess + ?Sized)) -> bool {
    matches!(
        node.kind().known(),
        Some(
            K::JsxElement
                | K::JsxExpression
                | K::JsxSelfClosingElement
                | K::JsxText
                | K::JsxFragment
        )
    )
}

/// port: tsc/internal/ast/utilities.go:CanHaveSymbol
pub fn can_have_symbol(node: &(impl NodeAccess + ?Sized)) -> bool {
    matches!(
        node.kind().known(),
        Some(
            K::ArrowFunction
                | K::BinaryExpression
                | K::BindingElement
                | K::CallExpression
                | K::CallSignature
                | K::ClassDeclaration
                | K::ClassExpression
                | K::ClassStaticBlockDeclaration
                | K::Constructor
                | K::ConstructorType
                | K::ConstructSignature
                | K::ElementAccessExpression
                | K::EnumDeclaration
                | K::EnumMember
                | K::ExportAssignment
                | K::ExportDeclaration
                | K::ExportSpecifier
                | K::FunctionDeclaration
                | K::FunctionExpression
                | K::FunctionType
                | K::GetAccessor
                | K::ImportClause
                | K::ImportEqualsDeclaration
                | K::ImportSpecifier
                | K::IndexSignature
                | K::InterfaceDeclaration
                | K::JSTypeAliasDeclaration
                | K::JsxAttribute
                | K::JsxAttributes
                | K::JsxSpreadAttribute
                | K::MappedType
                | K::MethodDeclaration
                | K::MethodSignature
                | K::ModuleDeclaration
                | K::NamedTupleMember
                | K::NamespaceExport
                | K::NamespaceExportDeclaration
                | K::NamespaceImport
                | K::NewExpression
                | K::NoSubstitutionTemplateLiteral
                | K::NumericLiteral
                | K::ObjectLiteralExpression
                | K::Parameter
                | K::PropertyAccessExpression
                | K::PropertyAssignment
                | K::PropertyDeclaration
                | K::PropertySignature
                | K::SetAccessor
                | K::ShorthandPropertyAssignment
                | K::SourceFile
                | K::SpreadAssignment
                | K::StringLiteral
                | K::TypeAliasDeclaration
                | K::TypeLiteral
                | K::TypeParameter
                | K::VariableDeclaration
        )
    )
}

/// port: tsc/internal/ast/utilities.go:CanHaveIllegalModifiers
pub fn can_have_illegal_modifiers(node: &(impl NodeAccess + ?Sized)) -> bool {
    matches!(
        node.kind().known(),
        Some(
            K::ClassStaticBlockDeclaration
                | K::PropertyAssignment
                | K::ShorthandPropertyAssignment
                | K::MissingDeclaration
                | K::NamespaceExportDeclaration
        )
    )
}

/// port: tsc/internal/ast/utilities.go:CanHaveModifiers
pub fn can_have_modifiers(node: &(impl NodeAccess + ?Sized)) -> bool {
    matches!(
        node.kind().known(),
        Some(
            K::TypeParameter
                | K::Parameter
                | K::PropertySignature
                | K::PropertyDeclaration
                | K::MethodSignature
                | K::MethodDeclaration
                | K::Constructor
                | K::GetAccessor
                | K::SetAccessor
                | K::IndexSignature
                | K::ConstructorType
                | K::FunctionExpression
                | K::ArrowFunction
                | K::ClassExpression
                | K::VariableStatement
                | K::FunctionDeclaration
                | K::ClassDeclaration
                | K::InterfaceDeclaration
                | K::TypeAliasDeclaration
                | K::EnumDeclaration
                | K::ModuleDeclaration
                | K::ImportEqualsDeclaration
                | K::ImportDeclaration
                | K::JSImportDeclaration
                | K::ExportAssignment
                | K::ExportDeclaration
        )
    )
}

/// port: tsc/internal/ast/utilities.go:isUnaryExpressionKind
pub fn is_unary_expression_kind(kind: NodeKind) -> bool {
    matches!(
        kind.known(),
        Some(
            K::PrefixUnaryExpression
                | K::PostfixUnaryExpression
                | K::DeleteExpression
                | K::TypeOfExpression
                | K::VoidExpression
                | K::AwaitExpression
                | K::TypeAssertionExpression
        )
    ) || crate::is_left_hand_side_expression_kind(kind)
}

/// port: tsc/internal/ast/utilities.go:isExpressionKind
pub fn is_expression_kind(kind: NodeKind) -> bool {
    matches!(
        kind.known(),
        Some(
            K::ConditionalExpression
                | K::YieldExpression
                | K::ArrowFunction
                | K::BinaryExpression
                | K::SpreadElement
                | K::AsExpression
                | K::OmittedExpression
                | K::PartiallyEmittedExpression
                | K::SatisfiesExpression
        )
    ) || is_unary_expression_kind(kind)
}

/// port: tsc/internal/ast/utilities.go:isDeclarationStatementKind
pub fn is_declaration_statement_kind(kind: NodeKind) -> bool {
    matches!(
        kind.known(),
        Some(
            K::FunctionDeclaration
                | K::MissingDeclaration
                | K::ClassDeclaration
                | K::InterfaceDeclaration
                | K::TypeAliasDeclaration
                | K::JSTypeAliasDeclaration
                | K::EnumDeclaration
                | K::ModuleDeclaration
                | K::ImportDeclaration
                | K::JSImportDeclaration
                | K::ImportEqualsDeclaration
                | K::ExportDeclaration
                | K::ExportAssignment
                | K::NamespaceExportDeclaration
        )
    )
}

/// port: tsc/internal/ast/utilities.go:isStatementKindButNotDeclarationKind
pub fn is_statement_kind_but_not_declaration_kind(kind: NodeKind) -> bool {
    matches!(
        kind.known(),
        Some(
            K::BreakStatement
                | K::ContinueStatement
                | K::DebuggerStatement
                | K::DoStatement
                | K::ExpressionStatement
                | K::EmptyStatement
                | K::ForInStatement
                | K::ForOfStatement
                | K::ForStatement
                | K::IfStatement
                | K::LabeledStatement
                | K::ReturnStatement
                | K::SwitchStatement
                | K::ThrowStatement
                | K::TryStatement
                | K::VariableStatement
                | K::WhileStatement
                | K::WithStatement
                | K::NotEmittedStatement
        )
    )
}

/// port: tsc/internal/ast/utilities.go:IsBindingPattern
pub fn is_binding_pattern(node: &(impl NodeAccess + ?Sized)) -> bool {
    is_binding_pattern_kind(node.kind())
}

pub fn is_binding_pattern_kind(kind: NodeKind) -> bool {
    matches!(
        kind.known(),
        Some(K::ObjectBindingPattern | K::ArrayBindingPattern)
    )
}

/// port: tsc/internal/ast/utilities.go:IsAccessor
pub fn is_accessor(node: &(impl NodeAccess + ?Sized)) -> bool {
    matches!(node.kind().known(), Some(K::GetAccessor | K::SetAccessor))
}

/// port: tsc/internal/ast/utilities.go:IsMemberName
pub fn is_member_name(node: &(impl NodeAccess + ?Sized)) -> bool {
    matches!(
        node.kind().known(),
        Some(K::Identifier | K::PrivateIdentifier)
    )
}

/// port: tsc/internal/ast/utilities.go:IsEntityName
pub fn is_entity_name(node: &(impl NodeAccess + ?Sized)) -> bool {
    matches!(node.kind().known(), Some(K::Identifier | K::QualifiedName))
}

/// port: tsc/internal/ast/utilities.go:IsBooleanLiteral
pub fn is_boolean_literal(node: &(impl NodeAccess + ?Sized)) -> bool {
    is_boolean_literal_kind(node.kind())
}

pub fn is_boolean_literal_kind(kind: NodeKind) -> bool {
    matches!(kind.known(), Some(K::TrueKeyword | K::FalseKeyword))
}

/// port: tsc/internal/ast/utilities.go:IsStringLiteralLike
pub fn is_string_literal_like(node: &(impl NodeAccess + ?Sized)) -> bool {
    is_string_literal_like_kind(node.kind())
}

pub fn is_string_literal_like_kind(kind: NodeKind) -> bool {
    matches!(
        kind.known(),
        Some(K::StringLiteral | K::NoSubstitutionTemplateLiteral)
    )
}

/// port: tsc/internal/ast/utilities.go:IsStringOrNumericLiteralLike
pub fn is_string_or_numeric_literal_like(node: &(impl NodeAccess + ?Sized)) -> bool {
    is_string_or_numeric_literal_like_kind(node.kind())
}

pub fn is_string_or_numeric_literal_like_kind(kind: NodeKind) -> bool {
    matches!(
        kind.known(),
        Some(K::StringLiteral | K::NoSubstitutionTemplateLiteral | K::NumericLiteral)
    )
}

/// port: tsc/internal/ast/utilities.go:IsAssertionExpression
pub fn is_assertion_expression(node: &(impl NodeAccess + ?Sized)) -> bool {
    matches!(
        node.kind().known(),
        Some(K::TypeAssertionExpression | K::AsExpression)
    )
}

/// port: tsc/internal/ast/utilities.go:IsAccessExpression
pub fn is_access_expression(node: &(impl NodeAccess + ?Sized)) -> bool {
    matches!(
        node.kind().known(),
        Some(K::PropertyAccessExpression | K::ElementAccessExpression)
    )
}

/// port: tsc/internal/ast/utilities.go:IsClassLike
pub fn is_class_like(node: &(impl NodeAccess + ?Sized)) -> bool {
    matches!(
        node.kind().known(),
        Some(K::ClassDeclaration | K::ClassExpression)
    )
}

/// port: tsc/internal/ast/utilities.go:IsClassOrInterfaceLike
pub fn is_class_or_interface_like(node: &(impl NodeAccess + ?Sized)) -> bool {
    matches!(
        node.kind().known(),
        Some(K::ClassDeclaration | K::ClassExpression | K::InterfaceDeclaration)
    )
}

/// port: tsc/internal/ast/utilities.go:IsJsxAttributeLike
pub fn is_jsx_attribute_like(node: &(impl NodeAccess + ?Sized)) -> bool {
    matches!(
        node.kind().known(),
        Some(K::JsxAttribute | K::JsxSpreadAttribute)
    )
}

/// port: tsc/internal/ast/utilities.go:IsFunctionExpressionOrArrowFunction
pub fn is_function_expression_or_arrow_function(node: &(impl NodeAccess + ?Sized)) -> bool {
    matches!(
        node.kind().known(),
        Some(K::FunctionExpression | K::ArrowFunction)
    )
}

/// port: tsc/internal/ast/utilities.go:IsModuleOrEnumDeclaration
pub fn is_module_or_enum_declaration(node: &(impl NodeAccess + ?Sized)) -> bool {
    matches!(
        node.kind().known(),
        Some(K::ModuleDeclaration | K::EnumDeclaration)
    )
}

/// port: tsc/internal/ast/utilities.go:IsImportOrExportSpecifier
pub fn is_import_or_export_specifier(node: &(impl NodeAccess + ?Sized)) -> bool {
    matches!(
        node.kind().known(),
        Some(K::ImportSpecifier | K::ExportSpecifier)
    )
}

/// port: tsc/internal/ast/utilities.go:NodeIsSynthesized
pub fn node_is_synthesized(node: &(impl NodeAccess + ?Sized)) -> bool {
    range_is_synthesized(node.range())
}

/// port: tsc/internal/ast/utilities.go:IsModifier
pub fn is_modifier(node: &(impl NodeAccess + ?Sized)) -> bool {
    crate::is_modifier_kind(node.kind())
}

/// port: tsc/internal/ast/utilities.go:IsModifierLike
pub fn is_modifier_like(node: &(impl NodeAccess + ?Sized)) -> bool {
    is_modifier(node) || node.kind() == K::Decorator
}

/// port: tsc/internal/ast/utilities.go:IsLiteralExpression
pub fn is_literal_expression(node: &(impl NodeAccess + ?Sized)) -> bool {
    crate::is_literal_kind(node.kind())
}

/// port: tsc/internal/ast/utilities.go:IsDeclarationStatement
pub fn is_declaration_statement(node: &(impl NodeAccess + ?Sized)) -> bool {
    is_declaration_statement_kind(node.kind())
}

/// port: tsc/internal/ast/utilities.go:IsStatementButNotDeclaration
pub fn is_statement_but_not_declaration(node: &(impl NodeAccess + ?Sized)) -> bool {
    is_statement_kind_but_not_declaration_kind(node.kind())
}

/// port: tsc/internal/ast/utilities.go:IsTypeNode
pub fn is_type_node(node: &(impl NodeAccess + ?Sized)) -> bool {
    is_type_node_kind(node.kind())
}

/// port: tsc/internal/ast/utilities.go:PositionIsSynthesized
pub fn position_is_synthesized(position: isize) -> bool {
    position < 0
}

/// port: tsc/internal/ast/utilities.go:RangeIsSynthesized
pub fn range_is_synthesized(range: TextRange) -> bool {
    position_is_synthesized(range.pos() as isize) || position_is_synthesized(range.end() as isize)
}

/// port: tsc/internal/ast/utilities.go:NodeKindIs
pub fn node_kind_is(node: &(impl NodeAccess + ?Sized), kinds: &[NodeKind]) -> bool {
    kinds.contains(&node.kind())
}

/// port: tsc/internal/ast/utilities.go:IsForInOrOfStatement
pub fn is_for_in_or_of_statement(node: Option<&(impl NodeAccess + ?Sized)>) -> bool {
    node.is_some_and(|node| is_for_in_or_of_statement_kind(node.kind()))
}

pub fn is_for_in_or_of_statement_kind(kind: NodeKind) -> bool {
    matches!(kind.known(), Some(K::ForInStatement | K::ForOfStatement))
}

/// port: tsc/internal/ast/utilities.go:IsFunctionLikeDeclaration
pub fn is_function_like_declaration(node: Option<&(impl NodeAccess + ?Sized)>) -> bool {
    node.is_some_and(|node| is_function_like_declaration_kind(node.kind()))
}

/// port: tsc/internal/ast/utilities.go:IsFunctionLike
pub fn is_function_like(node: Option<&(impl NodeAccess + ?Sized)>) -> bool {
    node.is_some_and(|node| is_function_like_kind(node.kind()))
}

/// port: tsc/internal/ast/utilities.go:IsFunctionLikeOrClassStaticBlockDeclaration
pub fn is_function_like_or_class_static_block_declaration(
    node: Option<&(impl NodeAccess + ?Sized)>,
) -> bool {
    node.is_some_and(|node| {
        is_function_like(Some(node)) || node.kind() == K::ClassStaticBlockDeclaration
    })
}

/// port: tsc/internal/ast/utilities.go:IsFunctionOrSourceFile
pub fn is_function_or_source_file(node: &(impl NodeAccess + ?Sized)) -> bool {
    is_function_like(Some(node)) || node.kind() == K::SourceFile
}

/// port: tsc/internal/ast/utilities.go:IsInJSFile
pub fn is_in_js_file(node: Option<&(impl NodeAccess + ?Sized)>) -> bool {
    node.is_some_and(|node| node.flags() & node_flags::JAVA_SCRIPT_FILE != 0)
}

/// port: tsc/internal/ast/utilities.go:IsInJsonFile
pub fn is_in_json_file(node: &(impl NodeAccess + ?Sized)) -> bool {
    node.flags() & node_flags::JSON_FILE != 0
}

/// port: tsc/internal/ast/utilities.go:IsSourceFileJS
pub fn is_source_file_js(file: &SourceFileState) -> bool {
    file.is_js()
}

/// port: tsc/internal/ast/utilities.go:IsCheckJSEnabledForFile
pub fn is_check_js_enabled_for_file(
    file: &SourceFileState,
    options: &ts_core::CompilerOptions,
) -> bool {
    file.check_js_directive
        .map_or(options.check_js == ts_core::Tristate::TRUE, |directive| {
            directive.enabled
        })
}

/// Mirrors the AccessKind enumeration in tsc/internal/ast/ast.go.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AccessKind {
    /// Only reads from a variable.
    Read,
    /// Only writes to a variable without ever reading it, as in `x = 1`.
    Write,
    /// Reads from and writes to a variable, as in `f(x++)` or `x /= 1`.
    ReadWrite,
}

/// port: tsc/internal/ast/ast.go:IsWriteOnlyAccess
pub fn is_write_only_access(view: AstView<'_>, node: NodeId) -> Result<bool, Error> {
    Ok(access_kind(view, node)? == AccessKind::Write)
}

/// port: tsc/internal/ast/ast.go:IsWriteAccess
pub fn is_write_access(view: AstView<'_>, node: NodeId) -> Result<bool, Error> {
    Ok(access_kind(view, node)? != AccessKind::Read)
}

/// Nested parentheses and destructuring patterns recurse once per level.
/// port: tsc/internal/ast/ast.go:accessKind
pub fn access_kind(view: AstView<'_>, node: NodeId) -> Result<AccessKind, Error> {
    stacker::maybe_grow(64 * 1024, 1024 * 1024, || access_kind_worker(view, node))
}

fn access_kind_worker(view: AstView<'_>, node: NodeId) -> Result<AccessKind, Error> {
    let Some(parent) = view.node(node)?.parent() else {
        return Ok(AccessKind::Read);
    };
    let read = view.node(parent)?;
    let data = read.data_source();
    Ok(match read.kind().known() {
        Some(K::ParenthesizedExpression | K::ArrayLiteralExpression) => access_kind(view, parent)?,
        Some(K::PrefixUnaryExpression) => {
            let operator = data
                .as_prefix_unary_expression()
                .ok_or(Error::InvalidGraph)?
                .operator();
            if operator == K::PlusPlusToken || operator == K::MinusMinusToken {
                AccessKind::ReadWrite
            } else {
                AccessKind::Read
            }
        }
        Some(K::PostfixUnaryExpression) => {
            let operator = data
                .as_postfix_unary_expression()
                .ok_or(Error::InvalidGraph)?
                .operator();
            if operator == K::PlusPlusToken || operator == K::MinusMinusToken {
                AccessKind::ReadWrite
            } else {
                AccessKind::Read
            }
        }
        Some(K::BinaryExpression) => {
            let binary = data.as_binary_expression().ok_or(Error::InvalidGraph)?;
            if binary.left() == Some(node) {
                let operator = view
                    .node(binary.operator_token().ok_or(Error::InvalidGraph)?)?
                    .kind();
                if !crate::is_assignment_operator(operator) {
                    AccessKind::Read
                } else if operator == K::EqualsToken {
                    AccessKind::Write
                } else {
                    AccessKind::ReadWrite
                }
            } else {
                AccessKind::Read
            }
        }
        Some(K::PropertyAccessExpression) => {
            let access = data
                .as_property_access_expression()
                .ok_or(Error::InvalidGraph)?;
            if access.name() == Some(node) {
                access_kind(view, parent)?
            } else {
                AccessKind::Read
            }
        }
        Some(K::PropertyAssignment) => {
            let parent_access = access_kind(view, read.parent().ok_or(Error::InvalidGraph)?)?;
            // In `({ x: varname }) = { x: 1 }`, the left `x` is a read and the right `x` a write.
            let assignment = data.as_property_assignment().ok_or(Error::InvalidGraph)?;
            if assignment.name() == Some(node) {
                reverse_access_kind(parent_access)
            } else {
                parent_access
            }
        }
        Some(K::ShorthandPropertyAssignment) => {
            let assignment = data
                .as_shorthand_property_assignment()
                .ok_or(Error::InvalidGraph)?;
            if assignment.object_assignment_initializer() == Some(node) {
                AccessKind::Read
            } else {
                access_kind(view, read.parent().ok_or(Error::InvalidGraph)?)?
            }
        }
        Some(K::ForInStatement | K::ForOfStatement) => {
            let statement = data
                .as_for_in_or_of_statement()
                .ok_or(Error::InvalidGraph)?;
            if statement.initializer() == Some(node) {
                AccessKind::Write
            } else {
                AccessKind::Read
            }
        }
        _ => AccessKind::Read,
    })
}

/// port: tsc/internal/ast/ast.go:reverseAccessKind
fn reverse_access_kind(access: AccessKind) -> AccessKind {
    match access {
        AccessKind::Read => AccessKind::Write,
        AccessKind::Write => AccessKind::Read,
        AccessKind::ReadWrite => AccessKind::ReadWrite,
    }
}

/// port: tsc/internal/ast/utilities.go:IsJsonSourceFile
pub fn is_json_source_file(file: &SourceFileState) -> bool {
    file.script_kind == ts_core::ScriptKind::JSON
}

/// port: tsc/internal/ast/utilities.go:IsExternalModule
pub fn is_external_module(file: &SourceFileState) -> bool {
    file.external_module_indicator.is_some()
}

/// port: tsc/internal/ast/utilities.go:IsExternalOrCommonJSModule
pub fn is_external_or_common_js_module(file: &SourceFileRead<'_>) -> bool {
    file.external_module_indicator.is_some() || file.common_js_module_indicator().is_some()
}

/// port: tsc/internal/ast/utilities.go:IsCompoundAssignment
pub fn is_compound_assignment(kind: NodeKind) -> bool {
    kind.raw() >= K::PlusEqualsToken as i16 && kind.raw() <= K::CaretEqualsToken as i16
}

/// port: tsc/internal/ast/utilities.go:IsLogicalBinaryOperator
pub fn is_logical_binary_operator(kind: NodeKind) -> bool {
    kind == K::BarBarToken || kind == K::AmpersandAmpersandToken
}

/// port: tsc/internal/ast/utilities.go:IsLogicalOrCoalescingBinaryOperator
pub fn is_logical_or_coalescing_binary_operator(kind: NodeKind) -> bool {
    is_logical_binary_operator(kind) || kind == K::QuestionQuestionToken
}

/// port: tsc/internal/ast/utilities.go:IsJSDocKind
pub fn is_js_doc_kind(kind: NodeKind) -> bool {
    kind.raw() >= K::JSDocTypeExpression as i16 && kind.raw() <= K::JSDocImportTag as i16
}

fn required(node: Option<NodeId>) -> NodeId {
    node.expect("nil node in source AST utility")
}
fn parent(view: AstView<'_>, node: NodeId) -> Result<NodeId, Error> {
    Ok(required(view.node(node)?.parent()))
}

/// port: tsc/internal/ast/utilities.go:FindLastVisibleNode
pub fn find_last_visible_node(
    view: AstView<'_>,
    nodes: &[Option<NodeId>],
) -> Result<Option<NodeId>, Error> {
    for &node in nodes.iter().rev() {
        let id = required(node);
        if view.node(id)?.flags() & node_flags::REPARSED == 0 {
            return Ok(Some(id));
        }
    }
    Ok(None)
}
/// port: tsc/internal/ast/utilities.go:FindAncestor
pub fn find_ancestor(
    view: AstView<'_>,
    mut node: Option<NodeId>,
    mut callback: impl FnMut(&NodeRead<'_>) -> bool,
) -> Result<Option<NodeId>, Error> {
    while let Some(id) = node {
        let current = view.node(id)?;
        if callback(&current) {
            return Ok(node);
        }
        node = current.parent();
    }
    Ok(None)
}
/// port: tsc/internal/ast/utilities.go:FindManyAncestors
pub fn find_many_ancestors(
    view: AstView<'_>,
    mut node: Option<NodeId>,
    callbacks: &mut [&mut dyn FnMut(&NodeRead<'_>) -> bool],
) -> Result<Vec<Option<NodeId>>, Error> {
    let mut ancestors = vec![None; callbacks.len()];
    let mut found = 0;
    while let Some(id) = node {
        let current = view.node(id)?;
        for (index, callback) in callbacks.iter_mut().enumerate() {
            if ancestors[index].is_none() && callback(&current) {
                ancestors[index] = Some(id);
                found += 1;
                if found == callbacks.len() {
                    return Ok(ancestors);
                }
                break;
            }
        }
        node = current.parent();
    }
    Ok(ancestors)
}
/// port: tsc/internal/ast/utilities.go:FindAncestorKind
pub fn find_ancestor_kind(
    view: AstView<'_>,
    node: Option<NodeId>,
    kind: NodeKind,
) -> Result<Option<NodeId>, Error> {
    find_ancestor(view, node, |node| node.kind() == kind)
}
/// Open Go int32 result: other values keep walking, like False.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FindAncestorResult(pub i32);
impl FindAncestorResult {
    pub const FALSE: Self = Self(0);
    pub const TRUE: Self = Self(1);
    pub const QUIT: Self = Self(2);
}
/// port: tsc/internal/ast/utilities.go:ToFindAncestorResult
pub fn to_find_ancestor_result(value: bool) -> FindAncestorResult {
    if value {
        FindAncestorResult::TRUE
    } else {
        FindAncestorResult::FALSE
    }
}
/// port: tsc/internal/ast/utilities.go:FindAncestorOrQuit
pub fn find_ancestor_or_quit(
    view: AstView<'_>,
    mut node: Option<NodeId>,
    mut callback: impl FnMut(&NodeRead<'_>) -> FindAncestorResult,
) -> Result<Option<NodeId>, Error> {
    while let Some(id) = node {
        let current = view.node(id)?;
        match callback(&current) {
            FindAncestorResult::QUIT => return Ok(None),
            FindAncestorResult::TRUE => return Ok(node),
            _ => {}
        }
        node = current.parent();
    }
    Ok(None)
}
/// port: tsc/internal/ast/utilities.go:IsNodeDescendantOf
pub fn is_node_descendant_of(
    view: AstView<'_>,
    mut node: Option<NodeId>,
    ancestor: Option<NodeId>,
) -> Result<bool, Error> {
    while let Some(id) = node {
        let current = view.node(id)?;
        if node == ancestor {
            return Ok(true);
        }
        node = current.parent();
    }
    Ok(false)
}
/// port: tsc/internal/ast/utilities.go:GetSourceFileOfNode
pub fn get_source_file_of_node(
    view: AstView<'_>,
    node: Option<NodeId>,
) -> Result<Option<NodeId>, Error> {
    let found = find_ancestor_kind(view, node, K::SourceFile.into())?;
    if let Some(id) = found {
        view.node(id)?
            .data_source()
            .as_source_file()
            .expect("SourceFile payload");
    }
    Ok(found)
}
/// port: tsc/internal/ast/utilities.go:GetContainingClass
pub fn get_containing_class(view: AstView<'_>, node: NodeId) -> Result<Option<NodeId>, Error> {
    find_ancestor(view, view.node(node)?.parent(), |node| is_class_like(node))
}
/// port: tsc/internal/ast/utilities.go:WalkUpParenthesizedExpressions
pub fn walk_up_parenthesized_expressions(
    view: AstView<'_>,
    mut node: Option<NodeId>,
) -> Result<Option<NodeId>, Error> {
    while let Some(id) = node {
        let current = view.node(id)?;
        if current.kind() != K::ParenthesizedExpression {
            break;
        }
        node = current.parent();
    }
    Ok(node)
}
/// port: tsc/internal/ast/utilities.go:WalkUpParenthesizedTypes
pub fn walk_up_parenthesized_types(
    view: AstView<'_>,
    mut node: Option<NodeId>,
) -> Result<Option<NodeId>, Error> {
    while let Some(id) = node {
        let current = view.node(id)?;
        if current.kind() != K::ParenthesizedType {
            break;
        }
        node = current.parent();
    }
    Ok(node)
}
/// port: tsc/internal/ast/utilities.go:GetRootDeclaration
pub fn get_root_declaration(view: AstView<'_>, node: NodeId) -> Result<NodeId, Error> {
    crate::declaration_helpers::root_declaration(&view, node)
}
/// port: tsc/internal/ast/utilities.go:WalkUpBindingElementsAndPatterns
pub fn walk_up_binding_elements_and_patterns(
    view: AstView<'_>,
    binding: NodeId,
) -> Result<Option<NodeId>, Error> {
    let mut node = parent(view, binding)?;
    loop {
        let p = parent(view, node)?;
        if view.node(p)?.kind() != K::BindingElement {
            break;
        }
        node = parent(view, p)?;
    }
    Ok(view.node(node)?.parent())
}
/// port: tsc/internal/ast/utilities.go:GetCombinedNodeFlags
pub fn get_combined_node_flags(view: AstView<'_>, node: NodeId) -> Result<u32, Error> {
    let id = get_root_declaration(view, node)?;
    let root = view.node(id)?;
    let mut flags = root.flags();
    let mut current = Some(id);
    if root.kind() == K::VariableDeclaration {
        current = root.parent();
    }
    if let Some(id) = current {
        let n = view.node(id)?;
        if n.kind() == K::VariableDeclarationList {
            flags |= n.flags();
            current = n.parent();
        }
    }
    if let Some(id) = current {
        let n = view.node(id)?;
        if n.kind() == K::VariableStatement {
            flags |= n.flags();
        }
    }
    Ok(flags)
}
/// port: tsc/internal/ast/utilities.go:IsVarAwaitUsing
pub fn is_var_await_using(view: AstView<'_>, node: NodeId) -> Result<bool, Error> {
    Ok(get_combined_node_flags(view, node)? & node_flags::BLOCK_SCOPED == node_flags::AWAIT_USING)
}
/// port: tsc/internal/ast/utilities.go:IsVarUsing
pub fn is_var_using(view: AstView<'_>, node: NodeId) -> Result<bool, Error> {
    Ok(get_combined_node_flags(view, node)? & node_flags::BLOCK_SCOPED == node_flags::USING)
}
/// port: tsc/internal/ast/utilities.go:IsVarConst
pub fn is_var_const(view: AstView<'_>, node: NodeId) -> Result<bool, Error> {
    Ok(get_combined_node_flags(view, node)? & node_flags::BLOCK_SCOPED == node_flags::CONST)
}
/// port: tsc/internal/ast/utilities.go:IsVarLet
pub fn is_var_let(view: AstView<'_>, node: NodeId) -> Result<bool, Error> {
    Ok(get_combined_node_flags(view, node)? & node_flags::BLOCK_SCOPED == node_flags::LET)
}
/// port: tsc/internal/ast/utilities.go:IsVarConstLike
pub fn is_var_const_like(view: AstView<'_>, node: NodeId) -> Result<bool, Error> {
    Ok(matches!(
        get_combined_node_flags(view, node)? & node_flags::BLOCK_SCOPED,
        node_flags::CONST | node_flags::USING | node_flags::AWAIT_USING
    ))
}
/// port: tsc/internal/ast/utilities.go:IsPartOfParameterDeclaration
pub fn is_part_of_parameter_declaration(view: AstView<'_>, node: NodeId) -> Result<bool, Error> {
    Ok(view.node(get_root_declaration(view, node)?)?.kind() == K::Parameter)
}
/// port: tsc/internal/ast/utilities.go:IsCatchClauseVariableDeclarationOrBindingElement
pub fn is_catch_clause_variable_declaration_or_binding_element(
    view: AstView<'_>,
    node: NodeId,
) -> Result<bool, Error> {
    let id = get_root_declaration(view, node)?;
    let n = view.node(id)?;
    Ok(n.kind() == K::VariableDeclaration
        && view.node(required(n.parent()))?.kind() == K::CatchClause)
}
/// port: tsc/internal/ast/utilities.go:IsBlockOrCatchScoped
pub fn is_block_or_catch_scoped(view: AstView<'_>, node: NodeId) -> Result<bool, Error> {
    Ok(
        get_combined_node_flags(view, node)? & node_flags::BLOCK_SCOPED != 0
            || is_catch_clause_variable_declaration_or_binding_element(view, node)?,
    )
}
/// port: tsc/internal/ast/utilities.go:IsFunctionBlock
pub fn is_function_block(view: AstView<'_>, node: Option<NodeId>) -> Result<bool, Error> {
    let Some(id) = node else {
        return Ok(false);
    };
    let n = view.node(id)?;
    Ok(n.kind() == K::Block
        && n.parent()
            .map(|id| view.node(id).map(|n| is_function_like(Some(&n))))
            .transpose()?
            .unwrap_or(false))
}
/// port: tsc/internal/ast/utilities.go:isBlockStatement
pub fn is_block_statement(view: AstView<'_>, node: NodeId) -> Result<bool, Error> {
    let n = view.node(node)?;
    if n.kind() != K::Block {
        return Ok(false);
    }
    if let Some(p) = n.parent() {
        if matches!(
            view.node(p)?.kind().known(),
            Some(K::TryStatement | K::CatchClause)
        ) {
            return Ok(false);
        }
    }
    Ok(!is_function_block(view, Some(node))?)
}
/// port: tsc/internal/ast/utilities.go:IsStatement
pub fn is_statement(view: AstView<'_>, node: NodeId) -> Result<bool, Error> {
    let kind = view.node(node)?.kind();
    Ok(is_statement_kind_but_not_declaration_kind(kind)
        || is_declaration_statement_kind(kind)
        || is_block_statement(view, node)?)
}
/// port: tsc/internal/ast/utilities.go:IsObjectLiteralMethod
pub fn is_object_literal_method(view: AstView<'_>, node: Option<NodeId>) -> Result<bool, Error> {
    let Some(node) = node else {
        return Ok(false);
    };
    let n = view.node(node)?;
    Ok(n.kind() == K::MethodDeclaration
        && view.node(required(n.parent()))?.kind() == K::ObjectLiteralExpression)
}
/// port: tsc/internal/ast/utilities.go:IsObjectLiteralOrClassExpressionMethodOrAccessor
pub fn is_object_literal_or_class_expression_method_or_accessor(
    view: AstView<'_>,
    node: NodeId,
) -> Result<bool, Error> {
    let n = view.node(node)?;
    Ok(is_method_or_accessor(&n)
        && is_object_literal_or_class_expression_kind(view.node(required(n.parent()))?.kind()))
}

pub fn is_object_literal_or_class_expression_kind(kind: NodeKind) -> bool {
    matches!(
        kind.known(),
        Some(K::ObjectLiteralExpression | K::ClassExpression)
    )
}
/// port: tsc/internal/ast/utilities.go:IsFunctionOrModuleBlock
pub fn is_function_or_module_block(view: AstView<'_>, node: NodeId) -> Result<bool, Error> {
    let n = view.node(node)?;
    Ok(
        matches!(n.kind().known(), Some(K::SourceFile | K::ModuleBlock))
            || n.kind() == K::Block
                && n.parent()
                    .map(|id| view.node(id).map(|n| is_function_like(Some(&n))))
                    .transpose()?
                    .unwrap_or(false),
    )
}
/// port: tsc/internal/ast/utilities.go:IsGlobalScopeAugmentation
pub fn is_global_scope_augmentation(node: &(impl NodeAccess + ?Sized)) -> bool {
    node.kind() == K::ModuleDeclaration
        && node
            .data_source()
            .as_module_declaration()
            .expect("ModuleDeclaration payload")
            .keyword()
            == K::GlobalKeyword
}
/// port: tsc/internal/ast/utilities.go:IsAnyImportSyntax
pub fn is_any_import_syntax(node: &(impl NodeAccess + ?Sized)) -> bool {
    matches!(
        node.kind().known(),
        Some(K::ImportDeclaration | K::ImportEqualsDeclaration)
    )
}
/// port: tsc/internal/ast/utilities.go:IsImportNode
pub fn is_import_node(node: &(impl NodeAccess + ?Sized)) -> bool {
    is_any_import_syntax(node) || node.kind() == K::JSImportDeclaration
}
/// port: tsc/internal/ast/utilities.go:IsAnyImportOrReExport
pub fn is_any_import_or_re_export(node: &(impl NodeAccess + ?Sized)) -> bool {
    is_import_node(node) || node.kind() == K::ExportDeclaration
}

/// port: tsc/internal/ast/utilities.go:isFunctionLikeDeclarationKind
pub fn is_function_like_declaration_kind(kind: NodeKind) -> bool {
    matches!(
        kind.known(),
        Some(
            K::FunctionDeclaration
                | K::MethodDeclaration
                | K::Constructor
                | K::GetAccessor
                | K::SetAccessor
                | K::FunctionExpression
                | K::ArrowFunction
        )
    )
}

/// port: tsc/internal/ast/utilities.go:IsFunctionLikeKind
pub fn is_function_like_kind(kind: NodeKind) -> bool {
    matches!(
        kind.known(),
        Some(
            K::MethodSignature
                | K::CallSignature
                | K::JSDocSignature
                | K::ConstructSignature
                | K::IndexSignature
                | K::FunctionType
                | K::ConstructorType
        )
    ) || is_function_like_declaration_kind(kind)
}

/// port: tsc/internal/ast/utilities.go:IsTypeNodeKind
pub fn is_type_node_kind(kind: NodeKind) -> bool {
    matches!(
        kind.known(),
        Some(
            K::AnyKeyword
                | K::UnknownKeyword
                | K::NumberKeyword
                | K::BigIntKeyword
                | K::ObjectKeyword
                | K::BooleanKeyword
                | K::StringKeyword
                | K::SymbolKeyword
                | K::VoidKeyword
                | K::UndefinedKeyword
                | K::NeverKeyword
                | K::IntrinsicKeyword
                | K::ExpressionWithTypeArguments
                | K::JSDocAllType
                | K::JSDocNullableType
                | K::JSDocNonNullableType
                | K::JSDocOptionalType
                | K::JSDocVariadicType
        )
    ) || kind.raw() >= K::TypePredicate as i16 && kind.raw() <= K::ImportType as i16
}
/// port: tsc/internal/ast/utilities.go:HasSyntacticModifier
pub fn has_syntactic_modifier(view: AstView<'_>, node: NodeId, flags: u32) -> Result<bool, Error> {
    crate::declaration_helpers::has_modifier(&view, node, flags)
}
/// port: tsc/internal/ast/utilities.go:HasAccessorModifier
pub fn has_accessor_modifier(view: AstView<'_>, node: NodeId) -> Result<bool, Error> {
    has_syntactic_modifier(view, node, modifier_flags::ACCESSOR)
}
/// port: tsc/internal/ast/utilities.go:HasStaticModifier
pub fn has_static_modifier(view: AstView<'_>, node: NodeId) -> Result<bool, Error> {
    has_syntactic_modifier(view, node, modifier_flags::STATIC)
}
/// port: tsc/internal/ast/utilities.go:IsStatic
pub fn is_static(view: AstView<'_>, node: NodeId) -> Result<bool, Error> {
    let n = view.node(node)?;
    Ok(is_class_element(&n) && has_static_modifier(view, node)?
        || n.kind() == K::ClassStaticBlockDeclaration)
}
/// port: tsc/internal/ast/utilities.go:IsAutoAccessorPropertyDeclaration
pub fn is_auto_accessor_property_declaration(
    view: AstView<'_>,
    node: NodeId,
) -> Result<bool, Error> {
    Ok(view.node(node)?.kind() == K::PropertyDeclaration && has_accessor_modifier(view, node)?)
}
/// port: tsc/internal/ast/utilities.go:IsParameterPropertyDeclaration
pub fn is_parameter_property_declaration(
    view: AstView<'_>,
    node: NodeId,
    parent: NodeId,
) -> Result<bool, Error> {
    Ok(view.node(node)?.kind() == K::Parameter
        && has_syntactic_modifier(view, node, modifier_flags::PARAMETER_PROPERTY_MODIFIER)?
        && view.node(parent)?.kind() == K::Constructor)
}
/// port: tsc/internal/ast/utilities.go:GetCombinedModifierFlags
pub fn get_combined_modifier_flags(view: AstView<'_>, node: NodeId) -> Result<u32, Error> {
    crate::declaration_helpers::combined_modifier_flags(&view, node)
}
/// port: tsc/internal/ast/utilities.go:IsEnumConst
pub fn is_enum_const(view: AstView<'_>, node: NodeId) -> Result<bool, Error> {
    Ok(get_combined_modifier_flags(view, node)? & modifier_flags::CONST != 0)
}
fn binary_operator(
    view: AstView<'_>,
    node: &(impl NodeAccess + ?Sized),
) -> Result<NodeKind, Error> {
    let op = required(
        node.data_source()
            .as_binary_expression()
            .expect("BinaryExpression payload")
            .operator_token(),
    );
    Ok(view.node(op)?.kind())
}
/// port: tsc/internal/ast/utilities.go:IsLogicalOrCoalescingBinaryExpression
pub fn is_logical_or_coalescing_binary_expression(
    view: AstView<'_>,
    node: NodeId,
) -> Result<bool, Error> {
    crate::syntax_helpers::is_logical_or_coalescing_binary_expression(&view, node)
}
/// port: tsc/internal/ast/utilities.go:IsLogicalOrCoalescingAssignmentExpression
pub fn is_logical_or_coalescing_assignment_expression(
    view: AstView<'_>,
    node: NodeId,
) -> Result<bool, Error> {
    crate::syntax_helpers::is_logical_or_coalescing_assignment_expression(&view, node)
}
/// port: tsc/internal/ast/utilities.go:IsNullishCoalesce
pub fn is_nullish_coalesce(view: AstView<'_>, node: NodeId) -> Result<bool, Error> {
    crate::syntax_helpers::is_nullish_coalesce(&view, node)
}
/// port: tsc/internal/ast/utilities.go:IsCommaExpression
pub fn is_comma_expression(view: AstView<'_>, node: NodeId) -> Result<bool, Error> {
    let n = view.node(node)?;
    Ok(n.kind() == K::BinaryExpression && binary_operator(view, &n)? == K::CommaToken)
}
/// port: tsc/internal/ast/utilities.go:IsCommaSequence
pub fn is_comma_sequence(view: AstView<'_>, node: NodeId) -> Result<bool, Error> {
    is_comma_expression(view, node)
}
/// port: tsc/internal/ast/utilities.go:IsInstanceOfExpression
pub fn is_instance_of_expression(view: AstView<'_>, node: NodeId) -> Result<bool, Error> {
    let n = view.node(node)?;
    Ok(n.kind() == K::BinaryExpression && binary_operator(view, &n)? == K::InstanceOfKeyword)
}
/// port: tsc/internal/ast/utilities.go:IsSignedNumericLiteral
pub fn is_signed_numeric_literal(view: AstView<'_>, node: NodeId) -> Result<bool, Error> {
    crate::declaration_helpers::signed_numeric_literal(&view, node)
}
/// port: tsc/internal/ast/utilities.go:IsOptionalChain
pub fn is_optional_chain(node: &(impl NodeAccess + ?Sized)) -> bool {
    crate::syntax_helpers::is_optional_chain(node)
}
/// port: tsc/internal/ast/utilities.go:getQuestionDotToken
pub fn get_question_dot_token(node: &(impl NodeAccess + ?Sized)) -> Option<NodeId> {
    node.question_dot_token()
}
/// port: tsc/internal/ast/utilities.go:IsOptionalChainRoot
pub fn is_optional_chain_root(node: &(impl NodeAccess + ?Sized)) -> bool {
    crate::syntax_helpers::is_optional_chain_root(node)
}
/// port: tsc/internal/ast/utilities.go:IsOutermostOptionalChain
pub fn is_outermost_optional_chain(view: AstView<'_>, node: NodeId) -> Result<bool, Error> {
    crate::syntax_helpers::is_outermost_optional_chain(&view, node)
}
/// port: tsc/internal/ast/utilities.go:IsExpressionOfOptionalChainRoot
pub fn is_expression_of_optional_chain_root(
    view: AstView<'_>,
    node: NodeId,
) -> Result<bool, Error> {
    crate::syntax_helpers::is_expression_of_optional_chain_root(&view, node)
}
/// port: tsc/internal/ast/utilities.go:IsLogicalExpression
pub fn is_logical_expression(view: AstView<'_>, node: NodeId) -> Result<bool, Error> {
    crate::syntax_helpers::is_logical_expression(&view, node)
}
/// port: tsc/internal/ast/utilities.go:IsPrivateIdentifierClassElementDeclaration
pub fn is_private_identifier_class_element_declaration(
    view: AstView<'_>,
    node: NodeId,
) -> Result<bool, Error> {
    let n = view.node(node)?;
    Ok(
        (n.kind() == K::PropertyDeclaration || is_method_or_accessor(&n))
            && view.node(required(n.name()))?.kind() == K::PrivateIdentifier,
    )
}
/// port: tsc/internal/ast/utilities.go:IsPrologueDirective
pub fn is_prologue_directive(view: AstView<'_>, node: NodeId) -> Result<bool, Error> {
    let n = view.node(node)?;
    Ok(n.kind() == K::ExpressionStatement
        && view.node(required(n.expression()))?.kind() == K::StringLiteral)
}
