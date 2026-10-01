use super::transform::{Transformer, NIL};
use tsr_ast::{modifier_flags as mf, AstView, NodeAccess, NodeId, SyntaxKind as K};
use tsr_printer::emit_resolver::DeclarationEmitResolver;

// port: tsc/internal/transformers/declarations/util.go:needsScopeMarker
pub(super) fn needs_scope_marker(
    view: AstView<'_>,
    result: NodeId,
) -> Result<bool, tsr_arena::Error> {
    let read = view.node(result)?;
    Ok(!tsr_ast::utilities::is_any_import_or_re_export(&read)
        && read.kind() != K::ExportAssignment
        && !tsr_ast::utilities::has_syntactic_modifier(view, result, mf::EXPORT)?
        && !tsr_ast::is_ambient_module(view, result)?)
}

// port: tsc/internal/transformers/declarations/util.go:canHaveLiteralInitializer
pub(super) fn can_have_literal_initializer<R: DeclarationEmitResolver>(
    tx: &mut Transformer<'_, R>,
    node: NodeId,
) -> Result<bool, R::Error> {
    Ok(match tx.kind(node) {
        K::PropertyDeclaration | K::PropertySignature => {
            tx.resolver.effective_declaration_flags(node, mf::PRIVATE)? == 0
        }
        K::Parameter | K::VariableDeclaration => true,
        _ => false,
    })
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

// port: tsc/internal/transformers/declarations/util.go:canReuseModifierNodes
pub(super) fn can_reuse_modifier_nodes(
    view: AstView<'_>,
    nodes: &[NodeId],
) -> Result<bool, tsr_arena::Error> {
    for &node in nodes {
        let read = view.node(node)?;
        if tsr_ast::utilities::is_modifier(&read)
            && read.flags() & tsr_ast::node_flags::REPARSED != 0
        {
            return Ok(false);
        }
    }
    Ok(true)
}

// port: tsc/internal/transformers/declarations/util.go:isDeclarationAndNotVisible
pub(super) fn is_declaration_and_not_visible<R: DeclarationEmitResolver>(
    tx: &mut Transformer<'_, R>,
    node: NodeId,
) -> Result<bool, R::Error> {
    let node = tx.parse_node(node).expect(NIL);
    Ok(match tx.kind(node) {
        K::FunctionDeclaration
        | K::ModuleDeclaration
        | K::InterfaceDeclaration
        | K::ClassDeclaration
        | K::TypeAliasDeclaration
        | K::JSTypeAliasDeclaration
        | K::EnumDeclaration => !tx.resolver.is_declaration_visible(node)?,
        // The following should be doing their own visibility checks based on filtering their members
        K::VariableDeclaration => !get_binding_name_visible(tx, node)?,
        K::ClassStaticBlockDeclaration => true,
        // ImportEquals, ImportDeclaration, JSImportDeclaration, ExportDeclaration and
        // ExportAssignment answer false like every other kind.
        _ => false,
    })
}

// port: tsc/internal/transformers/declarations/util.go:getBindingNameVisible
pub(super) fn get_binding_name_visible<R: DeclarationEmitResolver>(
    tx: &mut Transformer<'_, R>,
    elem: NodeId,
) -> Result<bool, R::Error> {
    if tx.kind(elem) == K::OmittedExpression {
        return Ok(false);
    }
    // TODO: parseArrayBindingElement _never_ parses out an OmittedExpression anymore, instead producing a nameless binding element
    // Audit if OmittedExpression should be removed
    let Some(name) = tx.node(elem).name() else {
        return Ok(false);
    };
    if matches!(
        tx.kind(name),
        K::ArrayBindingPattern | K::ObjectBindingPattern
    ) {
        // If any child binding pattern element has been marked visible (usually by collect linked aliases), then this is visible
        for elem in tx.list_nodes(tx.node(name).element_list()) {
            if get_binding_name_visible(tx, elem)? {
                return Ok(true);
            }
        }
        Ok(false)
    } else {
        tx.resolver.is_declaration_visible(elem)
    }
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
    ) || tsr_ast::utilities::is_function_like(Some(node))
}

// port: tsc/internal/transformers/declarations/util.go:isAlwaysType
pub(super) fn is_always_type(node: &(impl NodeAccess + ?Sized)) -> bool {
    node.kind() == K::InterfaceDeclaration
}

// port: tsc/internal/transformers/declarations/util.go:maskModifierFlags
pub(super) fn mask_modifier_flags(
    view: AstView<'_>,
    node: NodeId,
    modifier_mask: u32,
    modifier_additions: u32,
) -> Result<u32, tsr_arena::Error> {
    let mut flags = (tsr_ast::utilities::get_combined_modifier_flags(view, node)? & modifier_mask)
        | modifier_additions;
    if flags & mf::DEFAULT != 0 && flags & mf::EXPORT == 0 {
        // A non-exported default is a nonsequitor - we usually try to remove all export modifiers
        // from statements in ambient declarations; but a default export must retain its export modifier to be syntactically valid
        flags ^= mf::EXPORT;
    }
    if flags & mf::DEFAULT != 0 && flags & mf::AMBIENT != 0 {
        flags ^= mf::AMBIENT; // `declare` is never required alongside `default` (and would be an error if printed)
    }
    Ok(flags)
}

// port: tsc/internal/transformers/declarations/util.go:unwrapParenthesizedExpression
pub(super) fn unwrap_parenthesized_expression(
    view: AstView<'_>,
    mut o: NodeId,
) -> Result<NodeId, tsr_arena::Error> {
    while view.node(o)?.kind() == K::ParenthesizedExpression {
        o = view.node(o)?.expression().expect(NIL);
    }
    Ok(o)
}

// port: tsc/internal/transformers/declarations/util.go:isPrivateMethodTypeParameter
pub(super) fn is_private_method_type_parameter<R: DeclarationEmitResolver>(
    tx: &mut Transformer<'_, R>,
    node: NodeId,
) -> Result<bool, R::Error> {
    let parent = tx.parent(node).expect(NIL);
    Ok(tx.kind(parent) == K::MethodDeclaration
        && tx
            .resolver
            .effective_declaration_flags(parent, mf::PRIVATE)?
            != 0)
}

/// Returns true if expando properties should be emitted for this function.
/// Properties are emitted if any overload in the symbol has a body (implementation).
// port: tsc/internal/transformers/declarations/util.go:shouldEmitFunctionProperties
pub(super) fn should_emit_function_properties<R: DeclarationEmitResolver>(
    tx: &mut Transformer<'_, R>,
    input: NodeId,
) -> Result<bool, R::Error> {
    if tx.node(input).body().is_some() {
        return Ok(true);
    }
    let symbol = tx.resolver.bound_symbol_of_declaration(input)?.expect(NIL);
    for declaration in tx.resolver.symbol_declarations(symbol)? {
        let view = tx.resolver.ast(declaration)?;
        let read = view.node(declaration)?;
        if !(read.kind() != K::FunctionDeclaration || read.body().is_none()) {
            return Ok(true);
        }
    }
    Ok(false)
}

// port: tsc/internal/transformers/declarations/util.go:getEffectiveBaseTypeNode
pub(super) fn get_effective_base_type_node(
    view: AstView<'_>,
    node: NodeId,
) -> Result<Option<NodeId>, tsr_arena::Error> {
    // !!! TODO: JSDoc support (an @augments tag of a JS class)
    tsr_ast::utilities_class::get_class_extends_heritage_element(view, node)
}

// port: tsc/internal/transformers/declarations/util.go:isScopeMarker
pub(super) fn is_scope_marker(node: &(impl NodeAccess + ?Sized)) -> bool {
    matches!(
        node.kind().known(),
        Some(K::ExportAssignment | K::ExportDeclaration)
    )
}

// port: tsc/internal/transformers/declarations/util.go:hasScopeMarker
pub(super) fn has_scope_marker(
    view: AstView<'_>,
    statements: &[NodeId],
) -> Result<bool, tsr_arena::Error> {
    for &statement in statements {
        if is_scope_marker(&view.node(statement)?) {
            return Ok(true);
        }
    }
    Ok(false)
}
