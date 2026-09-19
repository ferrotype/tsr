use ts_ast::{modifier_flags as mf, AstView, NodeAccess, NodeId, SyntaxKind as K};

// port: tsc/internal/transformers/declarations/util.go:canHaveLiteralInitializer
pub(super) fn can_have_literal_initializer(
    node: &(impl NodeAccess + ?Sized),
    effective_flags: u32,
) -> bool {
    match node.kind().known() {
        Some(K::PropertyDeclaration | K::PropertySignature) => effective_flags & mf::PRIVATE == 0,
        Some(K::Parameter | K::VariableDeclaration) => true,
        _ => false,
    }
}
// port: tsc/internal/transformers/declarations/util.go:canProduceDiagnostics
pub(super) fn can_produce_diagnostics(node: &(impl NodeAccess + ?Sized)) -> bool {
    matches!(
        node.kind().known(),
        Some(
            K::VariableDeclaration
                | K::PropertyDeclaration
                | K::PropertySignature
                | K::BindingElement
                | K::SetAccessor
                | K::GetAccessor
                | K::ConstructSignature
                | K::CallSignature
                | K::MethodDeclaration
                | K::MethodSignature
                | K::FunctionDeclaration
                | K::Parameter
                | K::TypeParameter
                | K::ExpressionWithTypeArguments
                | K::ImportEqualsDeclaration
                | K::TypeAliasDeclaration
                | K::JSTypeAliasDeclaration
                | K::Constructor
                | K::IndexSignature
                | K::PropertyAccessExpression
                | K::ElementAccessExpression
                | K::BinaryExpression
                | K::CallExpression
        )
    )
}
// port: tsc/internal/transformers/declarations/util.go:isEnclosingDeclaration
pub(super) fn is_enclosing_declaration(node: &(impl NodeAccess + ?Sized)) -> bool {
    matches!(
        node.kind().known(),
        Some(
            K::SourceFile
                | K::TypeAliasDeclaration
                | K::JSTypeAliasDeclaration
                | K::ModuleDeclaration
                | K::ClassDeclaration
                | K::InterfaceDeclaration
                | K::IndexSignature
                | K::MappedType
                | K::VariableDeclaration
        )
    ) || ts_ast::utilities::is_function_like(Some(node))
}
// port: tsc/internal/transformers/declarations/util.go:maskModifierFlags
pub(super) fn mask_modifier_flags(flags: u32, mask: u32, additions: u32) -> u32 {
    let mut result = flags & mask | additions;
    if result & mf::DEFAULT != 0 {
        if result & mf::EXPORT == 0 {
            result ^= mf::EXPORT;
        }
        if result & mf::AMBIENT != 0 {
            result ^= mf::AMBIENT;
        }
    }
    result
}
// port: tsc/internal/transformers/declarations/util.go:unwrapParenthesizedExpression
pub(super) fn unwrap_parenthesized_expression(
    view: AstView<'_>,
    mut node: NodeId,
) -> Result<NodeId, ts_arena::Error> {
    while view.node(node)?.kind() == K::ParenthesizedExpression {
        node = view
            .node(node)?
            .expression()
            .ok_or(ts_arena::Error::InvalidGraph)?;
    }
    Ok(node)
}

// port: tsc/internal/ast/precedence.go:GetLeftmostExpression
pub(super) fn leftmost_expression(
    view: AstView<'_>,
    mut node: NodeId,
    stop_at_calls: bool,
) -> Result<NodeId, ts_arena::Error> {
    loop {
        let read = view.node(node)?;
        let next = match read.kind().known() {
            Some(K::PostfixUnaryExpression) => read
                .data_source()
                .as_postfix_unary_expression()
                .and_then(|data| data.operand()),
            Some(K::BinaryExpression) => read
                .data_source()
                .as_binary_expression()
                .and_then(|data| data.left()),
            Some(K::ConditionalExpression) => read
                .data_source()
                .as_conditional_expression()
                .and_then(|data| data.condition()),
            Some(K::TaggedTemplateExpression) => read
                .data_source()
                .as_tagged_template_expression()
                .and_then(|data| data.tag()),
            Some(K::CallExpression) if stop_at_calls => return Ok(node),
            Some(
                K::CallExpression
                | K::AsExpression
                | K::ElementAccessExpression
                | K::PropertyAccessExpression
                | K::NonNullExpression
                | K::PartiallyEmittedExpression
                | K::SatisfiesExpression,
            ) => read.expression(),
            _ => return Ok(node),
        };
        node = next.ok_or(ts_arena::Error::InvalidGraph)?;
    }
}
