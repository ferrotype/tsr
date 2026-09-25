//! AST utilities: names, type and expression positions, access kinds.
//!
//! Ports of `tsc/internal/ast/utilities.go` and `ast.go`, witnessed by the
//! `positions` group of the Phase 1 operation tables
//! (`docs/PHASE1-mutation-witnesses.md`, section 9). A parentless node where Go
//! reads `node.Parent.Kind` is Go's nil dereference, so it panics here too.
use crate::bind_result::BoundView;
use crate::{AstView, NodeAccess, NodeId, SyntaxKind as K};
use tsr_arena::Error;

const NIL: &str = "runtime error: invalid memory address or nil pointer dereference";

fn parent(view: AstView<'_>, id: NodeId) -> Result<Option<NodeId>, Error> {
    Ok(view.node(id)?.parent())
}

fn required_parent(view: AstView<'_>, id: NodeId) -> Result<NodeId, Error> {
    Ok(parent(view, id)?.expect(NIL))
}

fn kind(view: AstView<'_>, id: NodeId) -> Result<crate::NodeKind, Error> {
    Ok(view.node(id)?.kind())
}

/// Whether `node` has a symbol: Go's `Node.Symbol() != nil`. An unbound tree
/// has no symbols.
fn has_symbol(bound: Option<BoundView<'_>>, node: NodeId) -> Result<bool, Error> {
    let Some(bound) = bound else {
        return Ok(false);
    };
    Ok(bound
        .node_binding(node)?
        .is_some_and(|binding| binding.symbol.is_some()))
}

/// Go reads `name.Parent` unconditionally: every parsed node other than the
/// source file has a parent, and the source file is rejected first, so a
/// parentless node here is the nil dereference Go would panic on.
/// port: tsc/internal/ast/utilities.go:IsDeclarationName
pub fn is_declaration_name(view: AstView<'_>, name: NodeId) -> Result<bool, Error> {
    let node = view.node(name)?;
    if crate::is_source_file(&node) || crate::utilities::is_binding_pattern(&node) {
        return Ok(false);
    }
    let parent = view.node(node.parent().expect(NIL))?;
    Ok(crate::is_declaration(&parent) && parent.name() == Some(name))
}

/// port: tsc/internal/ast/ast.go:GetDeclarationFromName
pub fn get_declaration_from_name(
    view: AstView<'_>,
    bound: Option<BoundView<'_>>,
    name: Option<NodeId>,
) -> Result<Option<NodeId>, Error> {
    let Some(name) = name else {
        return Ok(None);
    };
    let Some(parent_id) = parent(view, name)? else {
        return Ok(None);
    };
    let parent = view.node(parent_id)?;
    let name_kind = kind(view, name)?;
    let identifier_like = match name_kind.known() {
        Some(K::StringLiteral | K::NoSubstitutionTemplateLiteral | K::NumericLiteral) => {
            if crate::is_computed_property_name(&parent) {
                return Ok(parent.parent());
            }
            true
        }
        Some(K::Identifier) => true,
        Some(K::PrivateIdentifier) => {
            if crate::is_declaration(&parent) && parent.name() == Some(name) {
                return Ok(Some(parent_id));
            }
            false
        }
        _ => false,
    };
    if !identifier_like {
        return Ok(None);
    }
    if crate::is_declaration(&parent) {
        return Ok((parent.name() == Some(name)).then_some(parent_id));
    }
    if crate::is_qualified_name(&parent) {
        let tag = parent.parent().expect(NIL);
        let tag_read = view.node(tag)?;
        if crate::is_js_doc_parameter_tag(&tag_read) && tag_read.name() == Some(parent_id) {
            return Ok(Some(tag));
        }
        return Ok(None);
    }
    let bin_exp = parent.parent().expect(NIL);
    let bin_read = view.node(bin_exp)?;
    if crate::is_binary_expression(&bin_read)
        && crate::binder_helpers::get_assignment_declaration_kind(view, bin_exp)?
            != crate::binder_helpers::JSDeclarationKind::None
    {
        let left = bin_read
            .data_source()
            .as_binary_expression()
            .ok_or(Error::InvalidGraph)?
            .left();
        let left_has_symbol = match left {
            Some(left) => has_symbol(bound, left)?,
            None => false,
        };
        if (left_has_symbol || has_symbol(bound, bin_exp)?)
            && crate::binder_helpers::get_name_of_declaration(view, Some(bin_exp))? == Some(name)
        {
            return Ok(Some(bin_exp));
        }
    }
    Ok(None)
}

/// Recurses once per enclosing array or object literal level.
/// port: tsc/internal/ast/ast.go:IsArrayLiteralOrObjectLiteralDestructuringPattern
pub fn is_array_literal_or_object_literal_destructuring_pattern(
    view: AstView<'_>,
    node: NodeId,
) -> Result<bool, Error> {
    stacker::maybe_grow(64 * 1024, 1024 * 1024, || {
        let read = view.node(node)?;
        if !(crate::is_array_literal_expression(&read)
            || crate::is_object_literal_expression(&read))
        {
            return Ok(false);
        }
        let parent_id = read.parent().expect(NIL);
        let parent = view.node(parent_id)?;
        if crate::is_binary_expression(&parent) {
            let binary = parent
                .data_source()
                .as_binary_expression()
                .ok_or(Error::InvalidGraph)?;
            if binary.left() == Some(node)
                && kind(view, binary.operator_token().expect(NIL))? == K::EqualsToken
            {
                return Ok(true);
            }
        }
        if crate::is_for_of_statement(&parent) && parent.initializer() == Some(node) {
            return Ok(true);
        }
        if crate::is_property_assignment(&parent) {
            return is_array_literal_or_object_literal_destructuring_pattern(
                view,
                parent.parent().expect(NIL),
            );
        }
        is_array_literal_or_object_literal_destructuring_pattern(view, parent_id)
    })
}

/// port: tsc/internal/ast/ast.go:IsTypeOrJSTypeAliasDeclaration
pub fn is_type_or_js_type_alias_declaration(node: &(impl NodeAccess + ?Sized)) -> bool {
    node.kind() == K::TypeAliasDeclaration || node.kind() == K::JSTypeAliasDeclaration
}

/// port: tsc/internal/ast/ast.go:IsWriteAccessForReference
pub fn is_write_access_for_reference(
    view: AstView<'_>,
    bound: Option<BoundView<'_>>,
    node: NodeId,
) -> Result<bool, Error> {
    let decl = get_declaration_from_name(view, bound, Some(node))?;
    Ok(decl.is_some() && declaration_is_write_access(view, decl)?
        || kind(view, node)? == K::DefaultKeyword
        || crate::utilities::is_write_access(view, node)?)
}

/// Panics on the kinds the pinned switch does not handle, as Go does.
/// port: tsc/internal/ast/ast.go:declarationIsWriteAccess
pub fn declaration_is_write_access(view: AstView<'_>, decl: Option<NodeId>) -> Result<bool, Error> {
    let Some(decl) = decl else {
        return Ok(false);
    };
    let read = view.node(decl)?;
    if read.flags() & crate::node_flags::AMBIENT != 0 {
        return Ok(true);
    }
    Ok(match read.kind().known() {
        Some(
            K::BinaryExpression
            | K::BindingElement
            | K::ClassDeclaration
            | K::ClassExpression
            | K::DefaultKeyword
            | K::EnumDeclaration
            | K::EnumMember
            | K::ExportSpecifier
            | K::ImportClause
            | K::ImportEqualsDeclaration
            | K::ImportSpecifier
            | K::InterfaceDeclaration
            | K::JSDocCallbackTag
            | K::JSDocTypedefTag
            | K::JsxAttribute
            | K::ModuleDeclaration
            | K::NamespaceExportDeclaration
            | K::NamespaceImport
            | K::NamespaceExport
            | K::Parameter
            | K::ShorthandPropertyAssignment
            | K::TypeAliasDeclaration
            | K::JSTypeAliasDeclaration
            | K::TypeParameter,
        ) => true,
        Some(K::PropertyAssignment) => !is_array_literal_or_object_literal_destructuring_pattern(
            view,
            read.parent().expect(NIL),
        )?,
        Some(
            K::FunctionDeclaration
            | K::FunctionExpression
            | K::Constructor
            | K::MethodDeclaration
            | K::GetAccessor
            | K::SetAccessor,
        ) => read.body().is_some(),
        Some(K::VariableDeclaration | K::PropertyDeclaration) => {
            read.initializer().is_some()
                || crate::is_catch_clause(&view.node(read.parent().expect(NIL))?)
        }
        Some(
            K::MethodSignature | K::PropertySignature | K::JSDocPropertyTag | K::JSDocParameterTag,
        ) => false,
        _ => panic!("Unhandled case in declarationIsWriteAccess"),
    })
}

/// `SemanticMeaning` of `tsc/internal/ast/utilities.go`.
pub mod semantic_meaning {
    pub const NONE: i32 = 0;
    pub const VALUE: i32 = 1;
    pub const TYPE: i32 = 1 << 1;
    pub const NAMESPACE: i32 = 1 << 2;
    pub const ALL: i32 = VALUE | TYPE | NAMESPACE;
}

/// port: tsc/internal/ast/utilities.go:GetMeaningFromDeclaration
pub fn get_meaning_from_declaration(view: AstView<'_>, node: NodeId) -> Result<i32, Error> {
    use semantic_meaning as M;
    Ok(match kind(view, node)?.known() {
        Some(
            K::VariableDeclaration
            | K::Parameter
            | K::BindingElement
            | K::PropertyDeclaration
            | K::PropertySignature
            | K::PropertyAssignment
            | K::ShorthandPropertyAssignment
            | K::MethodDeclaration
            | K::MethodSignature
            | K::Constructor
            | K::GetAccessor
            | K::SetAccessor
            | K::FunctionDeclaration
            | K::FunctionExpression
            | K::ArrowFunction
            | K::CatchClause
            | K::JsxAttribute,
        ) => M::VALUE,
        Some(
            K::TypeParameter
            | K::InterfaceDeclaration
            | K::TypeAliasDeclaration
            | K::JSTypeAliasDeclaration
            | K::TypeLiteral,
        ) => M::TYPE,
        Some(K::EnumMember | K::ClassDeclaration) => M::VALUE | M::TYPE,
        Some(K::ModuleDeclaration) => {
            if crate::binder_helpers::is_ambient_module(view, node)?
                || crate::binder_helpers::get_module_instance_state(view, node)?
                    == crate::binder_helpers::ModuleInstanceState::Instantiated
            {
                M::NAMESPACE | M::VALUE
            } else {
                M::NAMESPACE
            }
        }
        Some(K::SourceFile) => M::NAMESPACE | M::VALUE,
        _ => M::ALL,
    })
}

/// port: tsc/internal/ast/utilities.go:IsArrayBindingOrAssignmentElement
pub fn is_array_binding_or_assignment_element(
    view: AstView<'_>,
    node: NodeId,
) -> Result<bool, Error> {
    Ok(match kind(view, node)?.known() {
        Some(
            K::BindingElement
            | K::OmittedExpression
            | K::SpreadElement
            | K::ArrayLiteralExpression
            | K::ObjectLiteralExpression
            | K::Identifier
            | K::PropertyAccessExpression
            | K::ElementAccessExpression,
        ) => true,
        _ => crate::binder_helpers::is_assignment_expression(view, node, true)?,
    })
}

/// port: tsc/internal/ast/utilities.go:IsDeclarationNameOrImportPropertyName
pub fn is_declaration_name_or_import_property_name(
    view: AstView<'_>,
    name: NodeId,
) -> Result<bool, Error> {
    match kind(view, required_parent(view, name)?)?.known() {
        Some(K::ImportSpecifier | K::ExportSpecifier) => {
            let name_kind = kind(view, name)?;
            Ok(name_kind == K::Identifier || name_kind == K::StringLiteral)
        }
        _ => is_declaration_name(view, name),
    }
}

/// port: tsc/internal/ast/utilities.go:IsExpression
pub fn is_expression(view: AstView<'_>, node: NodeId) -> Result<bool, Error> {
    let skipped = crate::binder_helpers::skip_partially_emitted_expressions(view, node)?;
    Ok(crate::utilities::is_expression_kind(kind(view, skipped)?))
}

/// `IsImportCall`, whose home is the parser's reference collector.
pub(crate) fn is_import_call(view: AstView<'_>, node: NodeId) -> Result<bool, Error> {
    let read = view.node(node)?;
    if read.kind() != K::CallExpression {
        return Ok(false);
    }
    let expression = view.node(read.expression().expect(NIL))?;
    if expression.kind() == K::ImportKeyword {
        return Ok(true);
    }
    let Some(meta) = expression.data_source().as_meta_property() else {
        return Ok(false);
    };
    Ok(meta.keyword_token() == K::ImportKeyword
        && view.node_text(meta.name().expect(NIL))?.as_bytes() == b"defer")
}

/// port: tsc/internal/ast/utilities.go:IsExpressionNode
pub fn is_expression_node(view: AstView<'_>, node: NodeId) -> Result<bool, Error> {
    let read = view.node(node)?;
    Ok(match read.kind().known() {
        Some(
            K::SuperKeyword
            | K::NullKeyword
            | K::TrueKeyword
            | K::FalseKeyword
            | K::RegularExpressionLiteral
            | K::ArrayLiteralExpression
            | K::ObjectLiteralExpression
            | K::PropertyAccessExpression
            | K::ElementAccessExpression
            | K::CallExpression
            | K::NewExpression
            | K::TaggedTemplateExpression
            | K::AsExpression
            | K::TypeAssertionExpression
            | K::SatisfiesExpression
            | K::NonNullExpression
            | K::ParenthesizedExpression
            | K::FunctionExpression
            | K::ClassExpression
            | K::ArrowFunction
            | K::VoidExpression
            | K::DeleteExpression
            | K::TypeOfExpression
            | K::PrefixUnaryExpression
            | K::PostfixUnaryExpression
            | K::BinaryExpression
            | K::ConditionalExpression
            | K::SpreadElement
            | K::TemplateExpression
            | K::OmittedExpression
            | K::JsxElement
            | K::JsxSelfClosingElement
            | K::JsxFragment
            | K::YieldExpression
            | K::AwaitExpression,
        ) => true,
        Some(K::MetaProperty) => {
            let parent = read.parent().expect(NIL);
            !is_import_call(view, parent)? || view.node(parent)?.expression() != Some(node)
        }
        Some(K::ExpressionWithTypeArguments) => {
            !crate::is_heritage_clause(&view.node(read.parent().expect(NIL))?)
        }
        Some(K::QualifiedName) => {
            let mut current = node;
            while kind(view, required_parent(view, current)?)? == K::QualifiedName {
                current = required_parent(view, current)?;
            }
            let parent = view.node(required_parent(view, current)?)?;
            crate::is_type_query_node(&parent)
                || crate::utilities_middle::is_js_doc_link_like(&parent)
                || crate::is_js_doc_name_reference(&parent)
                || crate::utilities_targets::is_jsx_tag_name(view, current)?
        }
        Some(K::PrivateIdentifier) => {
            let parent = view.node(read.parent().expect(NIL))?;
            if !crate::is_binary_expression(&parent) {
                return Ok(false);
            }
            let binary = parent
                .data_source()
                .as_binary_expression()
                .ok_or(Error::InvalidGraph)?;
            binary.left() == Some(node)
                && kind(view, binary.operator_token().expect(NIL))? == K::InKeyword
        }
        Some(K::Identifier) => {
            let parent = view.node(read.parent().expect(NIL))?;
            if crate::is_type_query_node(&parent)
                || crate::utilities_middle::is_js_doc_link_like(&parent)
                || crate::is_js_doc_name_reference(&parent)
                || crate::utilities_targets::is_jsx_tag_name(view, node)?
            {
                return Ok(true);
            }
            is_in_expression_context(view, node)?
        }
        Some(
            K::NumericLiteral
            | K::BigIntLiteral
            | K::StringLiteral
            | K::NoSubstitutionTemplateLiteral
            | K::ThisKeyword,
        ) => is_in_expression_context(view, node)?,
        _ => false,
    })
}

/// port: tsc/internal/ast/utilities.go:IsInExpressionContext
pub fn is_in_expression_context(view: AstView<'_>, node: NodeId) -> Result<bool, Error> {
    let parent_id = required_parent(view, node)?;
    let parent = view.node(parent_id)?;
    Ok(match parent.kind().known() {
        Some(
            K::VariableDeclaration
            | K::Parameter
            | K::PropertyDeclaration
            | K::PropertySignature
            | K::EnumMember
            | K::PropertyAssignment
            | K::BindingElement,
        ) => parent.initializer() == Some(node),
        Some(
            K::ExpressionStatement
            | K::IfStatement
            | K::DoStatement
            | K::WhileStatement
            | K::ReturnStatement
            | K::WithStatement
            | K::SwitchStatement
            | K::CaseClause
            | K::DefaultClause
            | K::ThrowStatement
            | K::TypeAssertionExpression
            | K::AsExpression
            | K::TemplateSpan
            | K::ComputedPropertyName
            | K::SatisfiesExpression,
        ) => parent.expression() == Some(node),
        Some(K::ForStatement) => {
            let statement = parent
                .data_source()
                .as_for_statement()
                .ok_or(Error::InvalidGraph)?;
            let initializer = statement.initializer();
            initializer == Some(node) && kind(view, node)? != K::VariableDeclarationList
                || statement.condition() == Some(node)
                || statement.incrementor() == Some(node)
        }
        Some(K::ForInStatement | K::ForOfStatement) => {
            let statement = parent
                .data_source()
                .as_for_in_or_of_statement()
                .ok_or(Error::InvalidGraph)?;
            statement.initializer() == Some(node) && kind(view, node)? != K::VariableDeclarationList
                || statement.expression() == Some(node)
        }
        Some(K::Decorator | K::JsxExpression | K::JsxSpreadAttribute | K::SpreadAssignment) => true,
        Some(K::ExpressionWithTypeArguments) => {
            parent.expression() == Some(node) && !is_part_of_type_node(view, parent_id)?
        }
        Some(K::ShorthandPropertyAssignment) => {
            parent
                .data_source()
                .as_shorthand_property_assignment()
                .ok_or(Error::InvalidGraph)?
                .object_assignment_initializer()
                == Some(node)
        }
        _ => is_expression_node(view, parent_id)?,
    })
}

/// port: tsc/internal/ast/utilities.go:IsLiteralComputedPropertyDeclarationName
pub fn is_literal_computed_property_declaration_name(
    view: AstView<'_>,
    node: NodeId,
) -> Result<bool, Error> {
    if !crate::utilities::is_string_or_numeric_literal_like(&view.node(node)?) {
        return Ok(false);
    }
    let parent = required_parent(view, node)?;
    if kind(view, parent)? != K::ComputedPropertyName {
        return Ok(false);
    }
    Ok(crate::is_declaration(
        &view.node(required_parent(view, parent)?)?,
    ))
}

/// port: tsc/internal/ast/utilities.go:IsParseTreeNode
pub fn is_parse_tree_node(node: &(impl NodeAccess + ?Sized)) -> bool {
    node.flags() & crate::node_flags::SYNTHESIZED == 0
}

/// port: tsc/internal/ast/utilities.go:IsPartOfTypeNode
pub fn is_part_of_type_node(view: AstView<'_>, node: NodeId) -> Result<bool, Error> {
    let read = view.node(node)?;
    let node_kind = read.kind();
    if is_first_to_last_type_node(node_kind) {
        return Ok(true);
    }
    Ok(match node_kind.known() {
        Some(
            K::AnyKeyword
            | K::UnknownKeyword
            | K::NumberKeyword
            | K::BigIntKeyword
            | K::StringKeyword
            | K::BooleanKeyword
            | K::SymbolKeyword
            | K::ObjectKeyword
            | K::UndefinedKeyword
            | K::NullKeyword
            | K::NeverKeyword,
        ) => true,
        Some(K::VoidKeyword) => kind(view, read.parent().expect(NIL))? != K::VoidExpression,
        Some(K::ExpressionWithTypeArguments) => {
            is_part_of_type_expression_with_type_arguments(view, node)?
        }
        Some(K::TypeParameter) => {
            let parent_kind = kind(view, read.parent().expect(NIL))?;
            parent_kind == K::MappedType || parent_kind == K::InferType
        }
        Some(K::Identifier) => {
            let parent_id = read.parent().expect(NIL);
            let parent = view.node(parent_id)?;
            if crate::is_qualified_name(&parent)
                && parent
                    .data_source()
                    .as_qualified_name()
                    .ok_or(Error::InvalidGraph)?
                    .right()
                    == Some(node)
            {
                return is_part_of_type_node_in_parent(view, parent_id);
            }
            if crate::is_property_access_expression(&parent) && parent.name() == Some(node) {
                return is_part_of_type_node_in_parent(view, parent_id);
            }
            is_part_of_type_node_in_parent(view, node)?
        }
        Some(K::QualifiedName | K::PropertyAccessExpression | K::ThisKeyword) => {
            is_part_of_type_node_in_parent(view, node)?
        }
        _ => false,
    })
}

/// `KindFirstTypeNode <= kind <= KindLastTypeNode` of the pinned Kind order.
fn is_first_to_last_type_node(kind: crate::NodeKind) -> bool {
    let raw = kind.raw();
    raw >= crate::NodeKind::from(K::TypePredicate).raw()
        && raw <= crate::NodeKind::from(K::ImportType).raw()
}

/// port: tsc/internal/ast/utilities.go:isPartOfTypeExpressionWithTypeArguments
pub fn is_part_of_type_expression_with_type_arguments(
    view: AstView<'_>,
    node: NodeId,
) -> Result<bool, Error> {
    let parent_id = required_parent(view, node)?;
    let parent = view.node(parent_id)?;
    if crate::is_heritage_clause(&parent) {
        let class_like = crate::utilities::is_class_like(&view.node(parent.parent().expect(NIL))?);
        let token = parent
            .data_source()
            .as_heritage_clause()
            .ok_or(Error::InvalidGraph)?
            .token();
        if !class_like || token == K::ImplementsKeyword {
            return Ok(true);
        }
    }
    Ok(crate::is_js_doc_implements_tag(&parent) || crate::is_js_doc_augments_tag(&parent))
}

/// port: tsc/internal/ast/utilities.go:isPartOfTypeNodeInParent
pub fn is_part_of_type_node_in_parent(view: AstView<'_>, node: NodeId) -> Result<bool, Error> {
    let parent_id = required_parent(view, node)?;
    let parent = view.node(parent_id)?;
    let parent_kind = parent.kind();
    if parent_kind == K::TypeQuery {
        return Ok(false);
    }
    if parent_kind == K::ImportType {
        return Ok(!parent
            .data_source()
            .as_import_type_node()
            .ok_or(Error::InvalidGraph)?
            .is_type_of());
    }
    if is_first_to_last_type_node(parent_kind) {
        return Ok(true);
    }
    Ok(match parent_kind.known() {
        Some(K::ExpressionWithTypeArguments) => {
            is_part_of_type_expression_with_type_arguments(view, parent_id)?
        }
        Some(K::TypeParameter) => {
            parent
                .data_source()
                .as_type_parameter_declaration()
                .ok_or(Error::InvalidGraph)?
                .constraint()
                == Some(node)
        }
        Some(
            K::VariableDeclaration
            | K::Parameter
            | K::PropertyDeclaration
            | K::PropertySignature
            | K::FunctionDeclaration
            | K::FunctionExpression
            | K::ArrowFunction
            | K::Constructor
            | K::MethodDeclaration
            | K::MethodSignature
            | K::GetAccessor
            | K::SetAccessor
            | K::CallSignature
            | K::ConstructSignature
            | K::IndexSignature
            | K::TypeAssertionExpression,
        ) => parent.type_node() == Some(node),
        Some(K::CallExpression | K::NewExpression | K::TaggedTemplateExpression) => view
            .node_slice(parent.type_arguments(view)?)?
            .iter()
            .any(|argument| argument == Some(node)),
        _ => false,
    })
}

/// port: tsc/internal/ast/utilities.go:IsThisInTypeQuery
pub fn is_this_in_type_query(view: AstView<'_>, node: NodeId) -> Result<bool, Error> {
    if !view.is_this_identifier(node) {
        return Ok(false);
    }
    let mut current = node;
    loop {
        let parent_id = required_parent(view, current)?;
        let parent = view.node(parent_id)?;
        if crate::is_qualified_name(&parent)
            && parent
                .data_source()
                .as_qualified_name()
                .ok_or(Error::InvalidGraph)?
                .left()
                == Some(current)
        {
            current = parent_id;
            continue;
        }
        return Ok(parent.kind() == K::TypeQuery);
    }
}

/// port: tsc/internal/ast/utilities.go:IsTypeDeclaration
pub fn is_type_declaration(view: AstView<'_>, node: NodeId) -> Result<bool, Error> {
    let read = view.node(node)?;
    Ok(match read.kind().known() {
        Some(
            K::TypeParameter
            | K::ClassDeclaration
            | K::InterfaceDeclaration
            | K::TypeAliasDeclaration
            | K::JSTypeAliasDeclaration
            | K::EnumDeclaration,
        ) => true,
        Some(K::ImportClause) => read.is_type_only(),
        Some(K::ImportSpecifier | K::ExportSpecifier) => {
            let grandparent = required_parent(view, read.parent().expect(NIL))?;
            view.node(grandparent)?.is_type_only()
        }
        _ => false,
    })
}

/// port: tsc/internal/ast/utilities.go:IsTypeDeclarationName
pub fn is_type_declaration_name(view: AstView<'_>, name: NodeId) -> Result<bool, Error> {
    if kind(view, name)? != K::Identifier {
        return Ok(false);
    }
    let parent = required_parent(view, name)?;
    Ok(is_type_declaration(view, parent)?
        && crate::binder_helpers::get_name_of_declaration(view, Some(parent))? == Some(name))
}

/// port: tsc/internal/ast/utilities.go:SkipTypeParentheses
pub fn skip_type_parentheses(view: AstView<'_>, node: NodeId) -> Result<NodeId, Error> {
    let mut current = node;
    loop {
        let read = view.node(current)?;
        if !crate::is_parenthesized_type_node(&read) {
            return Ok(current);
        }
        current = read.type_node().expect(NIL);
    }
}

/// port: tsc/internal/ast/utilities.go:TryGetPropertyNameOfBindingOrAssignmentElement
pub fn try_get_property_name_of_binding_or_assignment_element(
    view: AstView<'_>,
    binding_element: NodeId,
) -> Result<Option<NodeId>, Error> {
    let read = view.node(binding_element)?;
    let literal_expression = |property_name: NodeId| -> Result<NodeId, Error> {
        let name = view.node(property_name)?;
        if crate::is_computed_property_name(&name) {
            let expression = name.expression().expect(NIL);
            if crate::utilities::is_string_or_numeric_literal_like(&view.node(expression)?) {
                return Ok(expression);
            }
        }
        Ok(property_name)
    };
    match read.kind().known() {
        Some(K::BindingElement) => {
            if let Some(property_name) = read.property_name() {
                return Ok(Some(literal_expression(property_name)?));
            }
        }
        Some(K::PropertyAssignment) => {
            if let Some(name) = read.name() {
                return Ok(Some(literal_expression(name)?));
            }
        }
        Some(K::SpreadAssignment) => return Ok(read.name()),
        _ => {}
    }
    let target = view.target_of_binding_or_assignment_element(binding_element);
    if let Some(target) = target {
        if crate::utilities::is_property_name(&view.node(target)?) {
            return Ok(Some(target));
        }
    }
    Ok(None)
}
