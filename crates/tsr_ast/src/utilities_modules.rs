//! AST utilities: modules, imports, type-only forms, augmentations and symbol
//! names.
//!
//! Ports of `tsc/internal/ast/utilities.go`, witnessed by the `modules` group of the Phase 1
//! operation tables (`docs/PHASE1-mutation-witnesses.md`, section 9).
use crate::bind_result::BoundView;
use crate::{modifier_flags, node_flags, AstView, NodeAccess, NodeId, SymbolId, SyntaxKind as K};
use tsr_arena::Error;
use tsr_core::{CompilerOptions, ModuleKind};

const NIL: &str = "runtime error: invalid memory address or nil pointer dereference";

fn parent(view: AstView<'_>, node: NodeId) -> Result<NodeId, Error> {
    Ok(view.node(node)?.parent().expect(NIL))
}

/// port: tsc/internal/ast/ast.go:IsAnyExportAssignment
pub fn is_any_export_assignment(node: &(impl NodeAccess + ?Sized)) -> bool {
    node.kind() == K::ExportAssignment
}

/// port: tsc/internal/ast/ast.go:IsImportDeclarationOrJSImportDeclaration
pub fn is_import_declaration_or_js_import_declaration(node: &(impl NodeAccess + ?Sized)) -> bool {
    node.kind() == K::ImportDeclaration || node.kind() == K::JSImportDeclaration
}

/// Go asserts the precondition; a node outside it is a caller bug.
/// port: tsc/internal/ast/utilities.go:GetExternalModuleImportEqualsDeclarationExpression
pub fn get_external_module_import_equals_declaration_expression(
    view: AstView<'_>,
    node: NodeId,
) -> Result<Option<NodeId>, Error> {
    assert!(
        is_external_module_import_equals_declaration(view, node)?,
        "False expression."
    );
    let reference = module_reference(view, node)?;
    Ok(view.node(reference)?.expression())
}

fn module_reference(view: AstView<'_>, node: NodeId) -> Result<NodeId, Error> {
    Ok(view
        .node(node)?
        .data_source()
        .as_import_equals_declaration()
        .ok_or(Error::InvalidGraph)?
        .module_reference()
        .expect(NIL))
}

/// port: tsc/internal/ast/utilities.go:GetExternalModuleName
pub fn get_external_module_name(view: AstView<'_>, node: NodeId) -> Result<Option<NodeId>, Error> {
    let read = view.node(node)?;
    Ok(match read.kind().known() {
        Some(K::ImportDeclaration | K::JSImportDeclaration | K::ExportDeclaration) => {
            read.module_specifier()
        }
        Some(K::ImportEqualsDeclaration) => {
            let reference = view.node(module_reference(view, node)?)?;
            if reference.kind() == K::ExternalModuleReference {
                reference.expression()
            } else {
                None
            }
        }
        Some(K::ImportType) => get_import_type_node_literal(view, node)?,
        Some(K::CallExpression) => view.node_slice(read.arguments(view)?)?.first().flatten(),
        Some(K::ModuleDeclaration) => {
            let name = read.name().expect(NIL);
            (view.node(name)?.kind() == K::StringLiteral).then_some(name)
        }
        _ => panic!("Unhandled case in getExternalModuleName"),
    })
}

/// port: tsc/internal/ast/utilities.go:GetImportAttributes
pub fn get_import_attributes(view: AstView<'_>, node: NodeId) -> Result<Option<NodeId>, Error> {
    let read = view.node(node)?;
    let data = read.data_source();
    Ok(match read.kind().known() {
        Some(K::ImportDeclaration | K::JSImportDeclaration) => data
            .as_import_declaration()
            .ok_or(Error::InvalidGraph)?
            .attributes(),
        Some(K::ExportDeclaration) => data
            .as_export_declaration()
            .ok_or(Error::InvalidGraph)?
            .attributes(),
        Some(K::ImportType) => data
            .as_import_type_node()
            .ok_or(Error::InvalidGraph)?
            .attributes(),
        _ => panic!("Unhandled case in getImportAttributes: {}", read.kind()),
    })
}

/// port: tsc/internal/ast/utilities.go:GetModuleSpecifierOfBareOrAccessedRequire
pub fn get_module_specifier_of_bare_or_accessed_require(
    view: AstView<'_>,
    node: NodeId,
) -> Result<Option<NodeId>, Error> {
    let first_argument = |call: NodeId| -> Result<Option<NodeId>, Error> {
        let arguments = view.node_slice(view.node(call)?.arguments(view)?)?;
        Ok(arguments.at(0))
    };
    if crate::binder_helpers::variable_initialized_with_require(view, node, false)? {
        return first_argument(view.node(node)?.initializer().expect(NIL));
    }
    if crate::binder_helpers::variable_initialized_with_require(view, node, true)? {
        let initializer = view.node(node)?.initializer().expect(NIL);
        let leftmost = crate::utilities_middle::get_leftmost_access_expression(view, initializer)?;
        if crate::utilities_middle::is_require_call(view, &view.node(leftmost)?, true)? {
            return first_argument(leftmost);
        }
    }
    Ok(None)
}

/// port: tsc/internal/ast/utilities.go:GetNonAugmentationDeclaration
pub fn get_non_augmentation_declaration(
    view: AstView<'_>,
    bound: BoundView<'_>,
    symbol: SymbolId,
) -> Result<Option<NodeId>, Error> {
    let symbol = bound.symbol(symbol)?;
    for declaration in bound
        .result()
        .declarations()
        .get(symbol.declarations())?
        .iter()
        .flatten()
    {
        if !is_external_module_augmentation(view, declaration)?
            && !crate::utilities::is_global_scope_augmentation(&view.node(declaration)?)
        {
            return Ok(Some(declaration));
        }
    }
    Ok(None)
}

/// port: tsc/internal/ast/utilities.go:GetSourceFileOfModule
pub fn get_source_file_of_module(
    view: AstView<'_>,
    bound: BoundView<'_>,
    module: SymbolId,
) -> Result<Option<NodeId>, Error> {
    let declaration = match bound.symbol(module)?.value_declaration() {
        Some(declaration) => Some(declaration),
        None => get_non_augmentation_declaration(view, bound, module)?,
    };
    crate::utilities::get_source_file_of_node(view, declaration)
}

/// port: tsc/internal/ast/utilities.go:HasImportAttributes
pub fn has_import_attributes(node: &(impl NodeAccess + ?Sized)) -> bool {
    matches!(
        node.kind().known(),
        Some(K::ImportDeclaration | K::JSImportDeclaration | K::ExportDeclaration | K::ImportType)
    )
}

/// port: tsc/internal/ast/utilities.go:HasResolutionModeOverride
pub fn has_resolution_mode_override(
    view: AstView<'_>,
    node: Option<NodeId>,
) -> Result<bool, Error> {
    let Some(node) = node else {
        return Ok(false);
    };
    let read = view.node(node)?;
    let data = read.data_source();
    let attributes = match read.kind().known() {
        Some(K::ImportType) => data
            .as_import_type_node()
            .ok_or(Error::InvalidGraph)?
            .attributes(),
        Some(K::ImportDeclaration | K::JSImportDeclaration) => data
            .as_import_declaration()
            .ok_or(Error::InvalidGraph)?
            .attributes(),
        Some(K::ExportDeclaration) => data
            .as_export_declaration()
            .ok_or(Error::InvalidGraph)?
            .attributes(),
        _ => None,
    };
    if attributes.is_some() {
        return Ok(
            crate::utilities_middle::import_attributes_resolution_mode(view, attributes)?.is_some(),
        );
    }
    Ok(false)
}

/// Go fails with a bad-syntax-kind assertion when the specifier has no import.
/// port: tsc/internal/ast/utilities.go:ImportFromModuleSpecifier
pub fn import_from_module_specifier(
    view: AstView<'_>,
    node: NodeId,
) -> Result<Option<NodeId>, Error> {
    if let Some(result) = try_get_import_from_module_specifier(view, node)? {
        return Ok(Some(result));
    }
    panic!("Debug Failure. Unexpected node.");
}

/// port: tsc/internal/ast/utilities.go:IsEffectiveExternalModule
/// `file` is the binding-aware read: the binder sets the CommonJS indicator.
pub fn is_effective_external_module(
    file: &crate::SourceFileRead<'_>,
    compiler_options: &CompilerOptions,
) -> bool {
    crate::utilities::is_external_module(file)
        || is_common_js_containing_module_kind(compiler_options.emit_module_kind())
            && file.common_js_module_indicator().is_some()
}

/// port: tsc/internal/ast/utilities.go:isCommonJSContainingModuleKind
fn is_common_js_containing_module_kind(kind: ModuleKind) -> bool {
    kind == ModuleKind::COMMON_JS || ModuleKind::NODE16 <= kind && kind <= ModuleKind::NODE_NEXT
}

/// port: tsc/internal/ast/utilities.go:IsEmittableImport
pub fn is_emittable_import(view: AstView<'_>, node: NodeId) -> Result<bool, Error> {
    let read = view.node(node)?;
    Ok(match read.kind().known() {
        Some(K::ImportDeclaration) => match read.import_clause() {
            Some(clause) => !view.node(clause)?.is_type_only(),
            None => false,
        },
        Some(K::ExportDeclaration | K::ImportEqualsDeclaration) => !read.is_type_only(),
        Some(K::CallExpression) => crate::utilities_positions::is_import_call(view, node)?,
        _ => false,
    })
}

/// Go reads `decl.ExportClause.Kind`, so an export declaration without a
/// clause panics.
/// port: tsc/internal/ast/utilities.go:IsExportNamespaceAsDefaultDeclaration
pub fn is_export_namespace_as_default_declaration(
    view: AstView<'_>,
    node: NodeId,
) -> Result<bool, Error> {
    let read = view.node(node)?;
    if read.kind() != K::ExportDeclaration {
        return Ok(false);
    }
    let clause = read
        .data_source()
        .as_export_declaration()
        .ok_or(Error::InvalidGraph)?
        .export_clause()
        .expect(NIL);
    let clause = view.node(clause)?;
    Ok(clause.kind() == K::NamespaceExport
        && crate::utilities_middle::module_export_name_is_default(view, clause.name().expect(NIL))?)
}

/// port: tsc/internal/ast/utilities.go:IsExternalModuleAugmentation
pub fn is_external_module_augmentation(view: AstView<'_>, node: NodeId) -> Result<bool, Error> {
    Ok(crate::binder_helpers::is_ambient_module(view, node)?
        && crate::binder_helpers::is_module_augmentation_external(view, node)?)
}

/// port: tsc/internal/ast/utilities.go:IsExternalModuleImportEqualsDeclaration
pub fn is_external_module_import_equals_declaration(
    view: AstView<'_>,
    node: NodeId,
) -> Result<bool, Error> {
    Ok(view.node(node)?.kind() == K::ImportEqualsDeclaration
        && view.node(module_reference(view, node)?)?.kind() == K::ExternalModuleReference)
}

/// port: tsc/internal/ast/utilities.go:IsExternalModuleIndicator
pub fn is_external_module_indicator(view: AstView<'_>, node: NodeId) -> Result<bool, Error> {
    let read = view.node(node)?;
    Ok(crate::utilities::is_any_import_or_re_export(&read)
        || read.kind() == K::ExportAssignment
        || crate::utilities::has_syntactic_modifier(view, node, modifier_flags::EXPORT)?)
}

/// port: tsc/internal/ast/utilities.go:IsModuleWithStringLiteralName
pub fn is_module_with_string_literal_name(view: AstView<'_>, node: NodeId) -> Result<bool, Error> {
    let read = view.node(node)?;
    Ok(read.kind() == K::ModuleDeclaration
        && view.node(read.name().expect(NIL))?.kind() == K::StringLiteral)
}

/// port: tsc/internal/ast/utilities.go:IsPartOfTypeOnlyImportOrExportDeclaration
pub fn is_part_of_type_only_import_or_export_declaration(
    view: AstView<'_>,
    node: NodeId,
) -> Result<bool, Error> {
    let mut current = Some(node);
    while let Some(id) = current {
        if is_type_only_import_or_export_declaration(view, id)? {
            return Ok(true);
        }
        current = view.node(id)?.parent();
    }
    Ok(false)
}

/// port: tsc/internal/ast/utilities.go:IsRequireVariableStatement
pub fn is_require_variable_statement(view: AstView<'_>, node: NodeId) -> Result<bool, Error> {
    let read = view.node(node)?;
    if read.kind() != K::VariableStatement {
        return Ok(false);
    }
    let list = read
        .data_source()
        .as_variable_statement()
        .ok_or(Error::InvalidGraph)?
        .declaration_list()
        .expect(NIL);
    let list = view.node(list)?;
    let declarations = list
        .data_source()
        .as_variable_declaration_list()
        .ok_or(Error::InvalidGraph)?
        .declarations()
        .expect(NIL);
    let declarations: Vec<NodeId> = view
        .node_slice(view.list(declarations)?.nodes())?
        .iter()
        .flatten()
        .collect();
    if declarations.is_empty() {
        return Ok(false);
    }
    for declaration in declarations {
        if !crate::binder_helpers::is_variable_declaration_initialized_to_require(
            view,
            declaration,
        )? {
            return Ok(false);
        }
    }
    Ok(true)
}

/// port: tsc/internal/ast/utilities.go:IsTypeOnlyImportDeclaration
pub fn is_type_only_import_declaration(view: AstView<'_>, node: NodeId) -> Result<bool, Error> {
    let read = view.node(node)?;
    Ok(match read.kind().known() {
        Some(K::ImportSpecifier) => {
            read.is_type_only()
                || view
                    .node(parent(view, parent(view, node)?)?)?
                    .is_type_only()
        }
        Some(K::NamespaceImport) => view.node(parent(view, node)?)?.is_type_only(),
        Some(K::ImportClause | K::ImportEqualsDeclaration) => read.is_type_only(),
        _ => false,
    })
}

/// port: tsc/internal/ast/utilities.go:IsTypeOnlyImportOrExportDeclaration
pub fn is_type_only_import_or_export_declaration(
    view: AstView<'_>,
    node: NodeId,
) -> Result<bool, Error> {
    Ok(
        is_type_only_import_declaration(view, node)?
            || is_type_only_export_declaration(view, node)?,
    )
}

/// port: tsc/internal/ast/utilities.go:isTypeOnlyExportDeclaration
fn is_type_only_export_declaration(view: AstView<'_>, node: NodeId) -> Result<bool, Error> {
    let read = view.node(node)?;
    Ok(match read.kind().known() {
        Some(K::ExportSpecifier) => {
            read.is_type_only()
                || view
                    .node(parent(view, parent(view, node)?)?)?
                    .is_type_only()
        }
        Some(K::ExportDeclaration) => {
            let data = read
                .data_source()
                .as_export_declaration()
                .ok_or(Error::InvalidGraph)?;
            read.is_type_only()
                && data.module_specifier().is_some()
                && data.export_clause().is_none()
        }
        Some(K::NamespaceExport) => view.node(parent(view, node)?)?.is_type_only(),
        _ => false,
    })
}

/// port: tsc/internal/ast/utilities.go:IsValidTypeOnlyAliasUseSite
pub fn is_valid_type_only_alias_use_site(
    view: AstView<'_>,
    use_site: NodeId,
) -> Result<bool, Error> {
    Ok(
        view.node(use_site)?.flags() & (node_flags::AMBIENT | node_flags::JS_DOC) != 0
            || crate::binder_helpers::is_part_of_type_query(view, use_site)?
            || is_identifier_in_non_emitting_heritage_clause(view, use_site)?
            || is_part_of_possibly_valid_type_or_abstract_computed_property_name(view, use_site)?
            || !(crate::utilities_positions::is_expression_node(view, use_site)?
                || is_shorthand_property_name_use_site(view, use_site)?),
    )
}

/// port: tsc/internal/ast/utilities.go:isIdentifierInNonEmittingHeritageClause
fn is_identifier_in_non_emitting_heritage_clause(
    view: AstView<'_>,
    node: NodeId,
) -> Result<bool, Error> {
    if view.node(node)?.kind() != K::Identifier {
        return Ok(false);
    }
    let mut current = parent(view, node)?;
    while matches!(
        view.node(current)?.kind().known(),
        Some(K::PropertyAccessExpression | K::ExpressionWithTypeArguments)
    ) {
        current = parent(view, current)?;
    }
    let clause = view.node(current)?;
    if clause.kind() != K::HeritageClause {
        return Ok(false);
    }
    Ok(clause
        .data_source()
        .as_heritage_clause()
        .ok_or(Error::InvalidGraph)?
        .token()
        == K::ImplementsKeyword
        || view.node(parent(view, current)?)?.kind() == K::InterfaceDeclaration)
}

/// port: tsc/internal/ast/utilities.go:isPartOfPossiblyValidTypeOrAbstractComputedPropertyName
fn is_part_of_possibly_valid_type_or_abstract_computed_property_name(
    view: AstView<'_>,
    node: NodeId,
) -> Result<bool, Error> {
    let mut current = node;
    while matches!(
        view.node(current)?.kind().known(),
        Some(K::Identifier | K::PropertyAccessExpression)
    ) {
        current = parent(view, current)?;
    }
    if view.node(current)?.kind() != K::ComputedPropertyName {
        return Ok(false);
    }
    let owner = parent(view, current)?;
    if crate::utilities::has_syntactic_modifier(view, owner, modifier_flags::ABSTRACT)? {
        return Ok(true);
    }
    Ok(matches!(
        view.node(parent(view, owner)?)?.kind().known(),
        Some(K::InterfaceDeclaration | K::TypeLiteral)
    ))
}

/// port: tsc/internal/ast/utilities.go:isShorthandPropertyNameUseSite
fn is_shorthand_property_name_use_site(view: AstView<'_>, use_site: NodeId) -> Result<bool, Error> {
    if view.node(use_site)?.kind() != K::Identifier {
        return Ok(false);
    }
    let owner = view.node(parent(view, use_site)?)?;
    Ok(owner.kind() == K::ShorthandPropertyAssignment && owner.name() == Some(use_site))
}

/// Go reads `node.Parent.Kind`, so a parentless node panics.
/// port: tsc/internal/ast/utilities.go:TryGetImportFromModuleSpecifier
pub fn try_get_import_from_module_specifier(
    view: AstView<'_>,
    node: NodeId,
) -> Result<Option<NodeId>, Error> {
    let parent_id = parent(view, node)?;
    let parent_read = view.node(parent_id)?;
    Ok(match parent_read.kind().known() {
        Some(K::ImportDeclaration | K::JSImportDeclaration | K::ExportDeclaration) => {
            Some(parent_id)
        }
        Some(K::ExternalModuleReference) => Some(parent(view, parent_id)?),
        Some(K::CallExpression) => (crate::utilities_positions::is_import_call(view, parent_id)?
            || crate::utilities_middle::is_require_call(view, &parent_read, false)?)
        .then_some(parent_id),
        Some(K::LiteralType) => {
            if view.node(node)?.kind() != K::StringLiteral {
                return Ok(None);
            }
            let grandparent = parent(view, parent_id)?;
            (view.node(grandparent)?.kind() == K::ImportType).then_some(grandparent)
        }
        _ => None,
    })
}

/// port: tsc/internal/ast/utilities.go:getImportTypeNodeLiteral
fn get_import_type_node_literal(view: AstView<'_>, node: NodeId) -> Result<Option<NodeId>, Error> {
    let read = view.node(node)?;
    if read.kind() != K::ImportType {
        return Ok(None);
    }
    let argument = read
        .data_source()
        .as_import_type_node()
        .ok_or(Error::InvalidGraph)?
        .argument()
        .expect(NIL);
    let argument = view.node(argument)?;
    if argument.kind() != K::LiteralType {
        return Ok(None);
    }
    let literal = argument
        .data_source()
        .as_literal_type_node()
        .ok_or(Error::InvalidGraph)?
        .literal()
        .expect(NIL);
    Ok((view.node(literal)?.kind() == K::StringLiteral).then_some(literal))
}
