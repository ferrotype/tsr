//! Declaration accessibility diagnostics preserve native diagnostic locations
//! and module-name distinctions. Selectors are stored as source identities by
//! the walker, rather than closures retaining a checker borrow.
use tsr_arena::{Error, NodeId};
use tsr_ast::{modifier_flags as mf, AstView, SyntaxKind as K};
use tsr_diagnostics::{self as d, Message};
use tsr_printer::emit_resolver::{SymbolAccessibility, SymbolAccessibilityResult};

#[derive(Clone, Copy, Debug)]
pub struct SymbolAccessibilityDiagnostic {
    pub error_node: Option<NodeId>,
    pub diagnostic_message: &'static Message,
    pub type_name: Option<NodeId>,
}

fn parent(view: AstView<'_>, node: NodeId) -> Result<NodeId, Error> {
    view.node(node)?.parent().ok_or(Error::InvalidGraph)
}
fn static_node(view: AstView<'_>, node: NodeId) -> Result<bool, Error> {
    tsr_ast::utilities::is_static(view, node)
}
fn class_parent(view: AstView<'_>, node: NodeId) -> Result<bool, Error> {
    Ok(view.node(parent(view, node)?)?.kind() == K::ClassDeclaration)
}
// port: tsc/internal/transformers/declarations/diagnostics.go:selectDiagnosticBasedOnModuleName
fn module_message(
    result: &SymbolAccessibilityResult,
    external: &'static Message,
    private: &'static Message,
    name: &'static Message,
) -> &'static Message {
    if result.error_module_name.is_empty() {
        name
    } else if result.accessibility == SymbolAccessibility::CannotBeNamed {
        external
    } else {
        private
    }
}
// port: tsc/internal/transformers/declarations/diagnostics.go:selectDiagnosticBasedOnModuleNameNoNameCheck
fn private_message(
    result: &SymbolAccessibilityResult,
    private: &'static Message,
    name: &'static Message,
) -> &'static Message {
    if result.error_module_name.is_empty() {
        name
    } else {
        private
    }
}

// port: tsc/internal/transformers/declarations/diagnostics.go:getAccessorNameVisibilityDiagnosticMessage
fn accessor_name_message(
    view: AstView<'_>,
    node: NodeId,
    result: &SymbolAccessibilityResult,
) -> Result<&'static Message, Error> {
    Ok(if static_node(view, node)? {
        module_message(result, d::Public_static_property_0_of_exported_class_has_or_is_using_name_1_from_external_module_2_but_cannot_be_named, d::Public_static_property_0_of_exported_class_has_or_is_using_name_1_from_private_module_2, d::Public_static_property_0_of_exported_class_has_or_is_using_private_name_1)
    } else if class_parent(view, node)? {
        module_message(result, d::Public_property_0_of_exported_class_has_or_is_using_name_1_from_external_module_2_but_cannot_be_named, d::Public_property_0_of_exported_class_has_or_is_using_name_1_from_private_module_2, d::Public_property_0_of_exported_class_has_or_is_using_private_name_1)
    } else {
        private_message(
            result,
            d::Property_0_of_exported_interface_has_or_is_using_name_1_from_private_module_2,
            d::Property_0_of_exported_interface_has_or_is_using_private_name_1,
        )
    })
}

// port: tsc/internal/transformers/declarations/diagnostics.go:getMethodNameVisibilityDiagnosticMessage
fn method_name_message(
    view: AstView<'_>,
    node: NodeId,
    result: &SymbolAccessibilityResult,
) -> Result<&'static Message, Error> {
    Ok(if static_node(view, node)? {
        module_message(result, d::Public_static_method_0_of_exported_class_has_or_is_using_name_1_from_external_module_2_but_cannot_be_named, d::Public_static_method_0_of_exported_class_has_or_is_using_name_1_from_private_module_2, d::Public_static_method_0_of_exported_class_has_or_is_using_private_name_1)
    } else if class_parent(view, node)? {
        module_message(result, d::Public_method_0_of_exported_class_has_or_is_using_name_1_from_external_module_2_but_cannot_be_named, d::Public_method_0_of_exported_class_has_or_is_using_name_1_from_private_module_2, d::Public_method_0_of_exported_class_has_or_is_using_private_name_1)
    } else {
        private_message(
            result,
            d::Method_0_of_exported_interface_has_or_is_using_name_1_from_private_module_2,
            d::Method_0_of_exported_interface_has_or_is_using_private_name_1,
        )
    })
}

// port: tsc/internal/transformers/declarations/diagnostics.go:getVariableDeclarationTypeVisibilityDiagnosticMessage
fn variable_message(
    view: AstView<'_>,
    node: NodeId,
    result: &SymbolAccessibilityResult,
) -> Result<Option<&'static Message>, Error> {
    let kind = view.node(node)?.kind().known();
    if matches!(kind, Some(K::VariableDeclaration | K::BindingElement)) {
        return Ok(Some(module_message(result, d::Exported_variable_0_has_or_is_using_name_1_from_external_module_2_but_cannot_be_named, d::Exported_variable_0_has_or_is_using_name_1_from_private_module_2, d::Exported_variable_0_has_or_is_using_private_name_1)));
    }
    if matches!(
        kind,
        Some(
            K::PropertyDeclaration
                | K::PropertyAccessExpression
                | K::ElementAccessExpression
                | K::BinaryExpression
                | K::PropertySignature
        )
    ) || kind == Some(K::Parameter)
        && tsr_ast::utilities::has_syntactic_modifier(view, parent(view, node)?, mf::PRIVATE)?
    {
        return Ok(Some(if static_node(view, node)? {
            module_message(result, d::Public_static_property_0_of_exported_class_has_or_is_using_name_1_from_external_module_2_but_cannot_be_named, d::Public_static_property_0_of_exported_class_has_or_is_using_name_1_from_private_module_2, d::Public_static_property_0_of_exported_class_has_or_is_using_private_name_1)
        } else if class_parent(view, node)? || kind == Some(K::Parameter) {
            module_message(result, d::Public_property_0_of_exported_class_has_or_is_using_name_1_from_external_module_2_but_cannot_be_named, d::Public_property_0_of_exported_class_has_or_is_using_name_1_from_private_module_2, d::Public_property_0_of_exported_class_has_or_is_using_private_name_1)
        } else {
            private_message(
                result,
                d::Property_0_of_exported_interface_has_or_is_using_name_1_from_private_module_2,
                d::Property_0_of_exported_interface_has_or_is_using_private_name_1,
            )
        }));
    }
    // Native returns nil for constructor contexts and other inapplicable nodes.
    Ok(None)
}

// port: tsc/internal/transformers/declarations/diagnostics.go:getAccessorDeclarationTypeVisibilityDiagnosticMessage
fn accessor_type_message(
    view: AstView<'_>,
    node: NodeId,
    result: &SymbolAccessibilityResult,
) -> Result<&'static Message, Error> {
    Ok(if view.node(node)?.kind() == K::SetAccessor {
        if static_node(view, node)? {
            private_message(result, d::Parameter_type_of_public_static_setter_0_from_exported_class_has_or_is_using_name_1_from_private_module_2, d::Parameter_type_of_public_static_setter_0_from_exported_class_has_or_is_using_private_name_1)
        } else {
            private_message(result, d::Parameter_type_of_public_setter_0_from_exported_class_has_or_is_using_name_1_from_private_module_2, d::Parameter_type_of_public_setter_0_from_exported_class_has_or_is_using_private_name_1)
        }
    } else if static_node(view, node)? {
        module_message(result, d::Return_type_of_public_static_getter_0_from_exported_class_has_or_is_using_name_1_from_external_module_2_but_cannot_be_named, d::Return_type_of_public_static_getter_0_from_exported_class_has_or_is_using_name_1_from_private_module_2, d::Return_type_of_public_static_getter_0_from_exported_class_has_or_is_using_private_name_1)
    } else {
        module_message(result, d::Return_type_of_public_getter_0_from_exported_class_has_or_is_using_name_1_from_external_module_2_but_cannot_be_named, d::Return_type_of_public_getter_0_from_exported_class_has_or_is_using_name_1_from_private_module_2, d::Return_type_of_public_getter_0_from_exported_class_has_or_is_using_private_name_1)
    })
}

// port: tsc/internal/transformers/declarations/diagnostics.go:getReturnTypeVisibilityDiagnosticMessage
fn return_message(
    view: AstView<'_>,
    node: NodeId,
    result: &SymbolAccessibilityResult,
) -> Result<&'static Message, Error> {
    Ok(match view.node(node)?.kind().known() {
        Some(K::ConstructSignature)=>private_message(result, d::Return_type_of_constructor_signature_from_exported_interface_has_or_is_using_name_0_from_private_module_1, d::Return_type_of_constructor_signature_from_exported_interface_has_or_is_using_private_name_0),
        Some(K::CallSignature)=>private_message(result, d::Return_type_of_call_signature_from_exported_interface_has_or_is_using_name_0_from_private_module_1, d::Return_type_of_call_signature_from_exported_interface_has_or_is_using_private_name_0),
        Some(K::IndexSignature)=>private_message(result, d::Return_type_of_index_signature_from_exported_interface_has_or_is_using_name_0_from_private_module_1, d::Return_type_of_index_signature_from_exported_interface_has_or_is_using_private_name_0),
        Some(K::MethodDeclaration|K::MethodSignature)=>if static_node(view,node)? { module_message(result, d::Return_type_of_public_static_method_from_exported_class_has_or_is_using_name_0_from_external_module_1_but_cannot_be_named, d::Return_type_of_public_static_method_from_exported_class_has_or_is_using_name_0_from_private_module_1, d::Return_type_of_public_static_method_from_exported_class_has_or_is_using_private_name_0) } else if class_parent(view,node)? { module_message(result, d::Return_type_of_public_method_from_exported_class_has_or_is_using_name_0_from_external_module_1_but_cannot_be_named, d::Return_type_of_public_method_from_exported_class_has_or_is_using_name_0_from_private_module_1, d::Return_type_of_public_method_from_exported_class_has_or_is_using_private_name_0) } else { private_message(result, d::Return_type_of_method_from_exported_interface_has_or_is_using_name_0_from_private_module_1, d::Return_type_of_method_from_exported_interface_has_or_is_using_private_name_0) },
        Some(K::FunctionDeclaration)=>module_message(result, d::Return_type_of_exported_function_has_or_is_using_name_0_from_external_module_1_but_cannot_be_named, d::Return_type_of_exported_function_has_or_is_using_name_0_from_private_module_1, d::Return_type_of_exported_function_has_or_is_using_private_name_0),
        _=>return Err(Error::InvalidGraph),
    })
}

// port: tsc/internal/transformers/declarations/diagnostics.go:getParameterDeclarationTypeVisibilityDiagnosticMessage
fn parameter_message(
    view: AstView<'_>,
    node: NodeId,
    result: &SymbolAccessibilityResult,
) -> Result<&'static Message, Error> {
    let owner = parent(view, node)?;
    Ok(match view.node(owner)?.kind().known() {
        Some(K::Constructor)=>module_message(result, d::Parameter_0_of_constructor_from_exported_class_has_or_is_using_name_1_from_external_module_2_but_cannot_be_named, d::Parameter_0_of_constructor_from_exported_class_has_or_is_using_name_1_from_private_module_2, d::Parameter_0_of_constructor_from_exported_class_has_or_is_using_private_name_1),
        Some(K::ConstructSignature|K::ConstructorType)=>private_message(result, d::Parameter_0_of_constructor_signature_from_exported_interface_has_or_is_using_name_1_from_private_module_2, d::Parameter_0_of_constructor_signature_from_exported_interface_has_or_is_using_private_name_1),
        Some(K::CallSignature)=>private_message(result, d::Parameter_0_of_call_signature_from_exported_interface_has_or_is_using_name_1_from_private_module_2, d::Parameter_0_of_call_signature_from_exported_interface_has_or_is_using_private_name_1),
        Some(K::IndexSignature)=>private_message(result, d::Parameter_0_of_index_signature_from_exported_interface_has_or_is_using_name_1_from_private_module_2, d::Parameter_0_of_index_signature_from_exported_interface_has_or_is_using_private_name_1),
        Some(K::MethodDeclaration|K::MethodSignature)=>if static_node(view,owner)? { module_message(result, d::Parameter_0_of_public_static_method_from_exported_class_has_or_is_using_name_1_from_external_module_2_but_cannot_be_named, d::Parameter_0_of_public_static_method_from_exported_class_has_or_is_using_name_1_from_private_module_2, d::Parameter_0_of_public_static_method_from_exported_class_has_or_is_using_private_name_1) } else if class_parent(view,owner)? { module_message(result, d::Parameter_0_of_public_method_from_exported_class_has_or_is_using_name_1_from_external_module_2_but_cannot_be_named, d::Parameter_0_of_public_method_from_exported_class_has_or_is_using_name_1_from_private_module_2, d::Parameter_0_of_public_method_from_exported_class_has_or_is_using_private_name_1) } else { private_message(result, d::Parameter_0_of_method_from_exported_interface_has_or_is_using_name_1_from_private_module_2, d::Parameter_0_of_method_from_exported_interface_has_or_is_using_private_name_1) },
        Some(K::FunctionDeclaration|K::FunctionType|K::ArrowFunction|K::FunctionExpression)=>module_message(result, d::Parameter_0_of_exported_function_has_or_is_using_name_1_from_external_module_2_but_cannot_be_named, d::Parameter_0_of_exported_function_has_or_is_using_name_1_from_private_module_2, d::Parameter_0_of_exported_function_has_or_is_using_private_name_1),
        Some(K::SetAccessor|K::GetAccessor)=>module_message(result, d::Parameter_0_of_accessor_has_or_is_using_name_1_from_external_module_2_but_cannot_be_named, d::Parameter_0_of_accessor_has_or_is_using_name_1_from_private_module_2, d::Parameter_0_of_accessor_has_or_is_using_private_name_1),
        _=>return Err(Error::InvalidGraph),
    })
}

// port: tsc/internal/transformers/declarations/diagnostics.go:getRelatedSuggestionByDeclarationKind
#[allow(
    clippy::match_same_arms,
    reason = "Keep the pinned upstream per-kind dispatch auditable when individual syntax cases change"
)]
pub(super) fn related_suggestion(kind: K) -> Option<&'static Message> {
    match kind {
        K::ArrowFunction => Some(d::Add_a_return_type_to_the_function_expression),
        K::FunctionExpression => Some(d::Add_a_return_type_to_the_function_expression),
        K::MethodDeclaration => Some(d::Add_a_return_type_to_the_method),
        K::GetAccessor => Some(d::Add_a_return_type_to_the_get_accessor_declaration),
        K::SetAccessor => Some(d::Add_a_type_to_parameter_of_the_set_accessor_declaration),
        K::FunctionDeclaration => Some(d::Add_a_return_type_to_the_function_declaration),
        K::ConstructSignature => Some(d::Add_a_return_type_to_the_function_declaration),
        K::Parameter => Some(d::Add_a_type_annotation_to_the_parameter_0),
        K::VariableDeclaration => Some(d::Add_a_type_annotation_to_the_variable_0),
        K::PropertyDeclaration => Some(d::Add_a_type_annotation_to_the_property_0),
        K::PropertySignature => Some(d::Add_a_type_annotation_to_the_property_0),
        K::ExportAssignment => Some(
            d::Move_the_expression_in_default_export_to_a_variable_and_add_a_type_annotation_to_it,
        ),
        _ => None,
    }
}

// port: tsc/internal/transformers/declarations/diagnostics.go:getErrorByDeclarationKind
#[allow(
    clippy::match_same_arms,
    reason = "Keep the pinned upstream per-kind dispatch auditable when individual syntax cases change"
)]
pub(super) fn isolated_error_message(kind: K) -> Option<&'static Message> {
    match kind {
        K::FunctionExpression=>Some(d::Function_must_have_an_explicit_return_type_annotation_with_isolatedDeclarations),
        K::FunctionDeclaration=>Some(d::Function_must_have_an_explicit_return_type_annotation_with_isolatedDeclarations),
        K::ArrowFunction=>Some(d::Function_must_have_an_explicit_return_type_annotation_with_isolatedDeclarations),
        K::MethodDeclaration=>Some(d::Method_must_have_an_explicit_return_type_annotation_with_isolatedDeclarations),
        K::ConstructSignature=>Some(d::Method_must_have_an_explicit_return_type_annotation_with_isolatedDeclarations),
        K::GetAccessor=>Some(d::At_least_one_accessor_must_have_an_explicit_type_annotation_with_isolatedDeclarations),
        K::SetAccessor=>Some(d::At_least_one_accessor_must_have_an_explicit_type_annotation_with_isolatedDeclarations),
        K::Parameter=>Some(d::Parameter_must_have_an_explicit_type_annotation_with_isolatedDeclarations),
        K::VariableDeclaration=>Some(d::Variable_must_have_an_explicit_type_annotation_with_isolatedDeclarations),
        K::PropertyDeclaration=>Some(d::Property_must_have_an_explicit_type_annotation_with_isolatedDeclarations),
        K::PropertySignature=>Some(d::Property_must_have_an_explicit_type_annotation_with_isolatedDeclarations),
        K::ComputedPropertyName=>Some(d::Computed_property_names_on_class_or_object_literals_cannot_be_inferred_with_isolatedDeclarations),
        K::SpreadAssignment=>Some(d::Objects_that_contain_spread_assignments_can_t_be_inferred_with_isolatedDeclarations),
        K::ShorthandPropertyAssignment=>Some(d::Objects_that_contain_shorthand_properties_can_t_be_inferred_with_isolatedDeclarations),
        K::ArrayLiteralExpression=>Some(d::Only_const_arrays_can_be_inferred_with_isolatedDeclarations),
        K::ExportAssignment=>Some(d::Default_exports_can_t_be_inferred_with_isolatedDeclarations),
        K::SpreadElement=>Some(d::Arrays_with_spread_elements_can_t_inferred_with_isolatedDeclarations),
        _=>None,
    }
}

// port: tsc/internal/transformers/declarations/diagnostics.go:getTypeParameterConstraintVisibilityDiagnosticMessage
fn type_parameter_message(view: AstView<'_>, node: NodeId) -> Result<&'static Message, Error> {
    let owner = parent(view, node)?;
    Ok(match view.node(owner)?.kind().known() {
        Some(K::ClassDeclaration)=>d::Type_parameter_0_of_exported_class_has_or_is_using_private_name_1,
        Some(K::InterfaceDeclaration)=>d::Type_parameter_0_of_exported_interface_has_or_is_using_private_name_1,
        Some(K::MappedType)=>d::Type_parameter_0_of_exported_mapped_object_type_is_using_private_name_1,
        Some(K::ConstructorType|K::ConstructSignature)=>d::Type_parameter_0_of_constructor_signature_from_exported_interface_has_or_is_using_private_name_1,
        Some(K::CallSignature)=>d::Type_parameter_0_of_call_signature_from_exported_interface_has_or_is_using_private_name_1,
        Some(K::MethodDeclaration|K::MethodSignature)=>if static_node(view,owner)? {
            d::Type_parameter_0_of_public_static_method_from_exported_class_has_or_is_using_private_name_1
        } else if class_parent(view,owner)? {
            d::Type_parameter_0_of_public_method_from_exported_class_has_or_is_using_private_name_1
        } else { d::Type_parameter_0_of_method_from_exported_interface_has_or_is_using_private_name_1 },
        Some(K::FunctionType|K::FunctionDeclaration)=>d::Type_parameter_0_of_exported_function_has_or_is_using_private_name_1,
        Some(K::InferType)=>d::Extends_clause_for_inferred_type_0_has_or_is_using_private_name_1,
        Some(K::TypeAliasDeclaration|K::JSTypeAliasDeclaration)=>d::Type_parameter_0_of_exported_type_alias_has_or_is_using_private_name_1,
        _=>return Err(Error::InvalidGraph),
    })
}

// port: tsc/internal/transformers/declarations/diagnostics.go:createGetSymbolAccessibilityDiagnosticForNodeName
// port: tsc/internal/transformers/declarations/diagnostics.go:createGetSymbolAccessibilityDiagnosticForNode
pub fn accessibility_diagnostic(
    view: AstView<'_>,
    node: NodeId,
    name_context: bool,
    result: &SymbolAccessibilityResult,
) -> Result<Option<SymbolAccessibilityDiagnostic>, Error> {
    let kind = view.node(node)?.kind().known().ok_or(Error::InvalidGraph)?;
    let name = tsr_ast::get_name_of_declaration(view, Some(node))?;
    let simple = |diagnostic_message| SymbolAccessibilityDiagnostic {
        error_node: Some(node),
        diagnostic_message,
        type_name: name,
    };
    if name_context {
        if matches!(kind, K::SetAccessor | K::GetAccessor) {
            return Ok(Some(simple(accessor_name_message(view, node, result)?)));
        }
        if matches!(kind, K::MethodDeclaration | K::MethodSignature) {
            return Ok(Some(simple(method_name_message(view, node, result)?)));
        }
    }
    let selection = match kind {
        K::VariableDeclaration
        | K::PropertyDeclaration
        | K::PropertySignature
        | K::PropertyAccessExpression
        | K::ElementAccessExpression
        | K::BinaryExpression
        | K::BindingElement
        | K::Constructor => {
            return Ok(variable_message(view, node, result)?.map(simple));
        }
        K::SetAccessor | K::GetAccessor => SymbolAccessibilityDiagnostic {
            error_node: name,
            diagnostic_message: accessor_type_message(view, node, result)?,
            type_name: name,
        },
        K::ConstructSignature
        | K::CallSignature
        | K::MethodDeclaration
        | K::MethodSignature
        | K::FunctionDeclaration
        | K::IndexSignature => SymbolAccessibilityDiagnostic {
            error_node: name.or(Some(node)),
            diagnostic_message: return_message(view, node, result)?,
            type_name: None,
        },
        K::Parameter => {
            let owner = parent(view, node)?;
            if tsr_ast::utilities::is_parameter_property_declaration(view, node, owner)?
                && tsr_ast::utilities::has_syntactic_modifier(view, owner, mf::PRIVATE)?
            {
                return Ok(variable_message(view, node, result)?.map(simple));
            }
            simple(parameter_message(view, node, result)?)
        }
        K::TypeParameter => simple(type_parameter_message(view, node)?),
        K::ExpressionWithTypeArguments => {
            let clause = parent(view, node)?;
            let owner = parent(view, clause)?;
            let diagnostic_message = if view.node(owner)?.kind() == K::ClassDeclaration {
                let implements = view.node(clause)?.kind() == K::HeritageClause
                    && view
                        .node(clause)?
                        .data_source()
                        .as_heritage_clause()
                        .ok_or(Error::InvalidGraph)?
                        .token()
                        == K::ImplementsKeyword;
                if implements {
                    d::Implements_clause_of_exported_class_0_has_or_is_using_private_name_1
                } else if view.node(owner)?.name().is_some() {
                    d::X_extends_clause_of_exported_class_0_has_or_is_using_private_name_1
                } else {
                    d::X_extends_clause_of_exported_class_has_or_is_using_private_name_0
                }
            } else {
                d::X_extends_clause_of_exported_interface_0_has_or_is_using_private_name_1
            };
            SymbolAccessibilityDiagnostic {
                error_node: Some(node),
                diagnostic_message,
                type_name: tsr_ast::get_name_of_declaration(view, Some(owner))?,
            }
        }
        K::ImportEqualsDeclaration => simple(d::Import_declaration_0_is_using_private_name_1),
        K::TypeAliasDeclaration | K::JSTypeAliasDeclaration => SymbolAccessibilityDiagnostic {
            error_node: view.node(node)?.type_node(),
            diagnostic_message: private_message(
                result,
                d::Exported_type_alias_0_has_or_is_using_private_name_1_from_module_2,
                d::Exported_type_alias_0_has_or_is_using_private_name_1,
            ),
            type_name: name,
        },
        K::CallExpression => {
            let arguments = view.node(node)?.arguments(view)?;
            let target = view
                .node_slice(arguments)?
                .get(1)
                .flatten()
                .ok_or(Error::InvalidGraph)?;
            SymbolAccessibilityDiagnostic {
                error_node:Some(target),type_name:Some(target),
                diagnostic_message:module_message(result,d::Exported_variable_0_has_or_is_using_name_1_from_external_module_2_but_cannot_be_named,d::Exported_variable_0_has_or_is_using_name_1_from_private_module_2,d::Exported_variable_0_has_or_is_using_private_name_1),
            }
        }
        _ => return Err(Error::InvalidGraph),
    };
    Ok(Some(selection))
}

// port: tsc/internal/checker/utilities.go:NewDiagnosticForNode
pub(super) fn diagnostic_for_node(
    view: AstView<'_>,
    node: Option<NodeId>,
    message: &'static Message,
    args: Vec<tsr_ast::JsString>,
) -> Result<tsr_ast::Diagnostic, Error> {
    let (file, range) = if let Some(node) = node {
        let file = tsr_ast::utilities::get_source_file_of_node(view, Some(node))?
            .ok_or(Error::InvalidGraph)?;
        (
            Some(file),
            tsr_scanner::get_error_range_for_node(view, file, node)?,
        )
    } else {
        (None, tsr_core::TextRange::default())
    };
    Ok(tsr_ast::Diagnostic::new(file, range, message, args))
}

// port: tsc/internal/transformers/declarations/diagnostics.go:findNearestDeclaration
fn nearest_declaration(view: AstView<'_>, mut node: NodeId) -> Result<Option<NodeId>, Error> {
    loop {
        let read = view.node(node)?;
        let kind = read.kind().known();
        if kind == Some(K::ExportAssignment) {
            return Ok(Some(node));
        }
        if matches!(
            kind,
            Some(K::VariableDeclaration | K::PropertyDeclaration | K::Parameter)
        ) {
            return Ok(Some(node));
        }
        if tsr_ast::utilities::is_statement(view, node)? {
            if kind != Some(K::ReturnStatement) {
                return Ok(None);
            }
            let mut current = Some(node);
            while let Some(node) = current {
                let read = view.node(node)?;
                if tsr_ast::utilities::is_function_like_declaration(Some(&read))
                    && read.kind() != K::Constructor
                {
                    return Ok(Some(node));
                }
                current = read.parent();
            }
            return Ok(None);
        }
        match read.parent() {
            Some(parent) => node = parent,
            None => return Ok(None),
        }
    }
}

fn declaration_target_text(view: AstView<'_>, node: NodeId) -> Result<tsr_ast::JsString, Error> {
    if view.node(node)?.kind() != K::ExportAssignment {
        if let Some(name) = view.node(node)?.name() {
            return tsr_scanner::get_text_of_node(view, name);
        }
    }
    Ok(tsr_ast::JsString::default())
}
fn kind(view: AstView<'_>, node: NodeId) -> Result<K, Error> {
    view.node(node)?.kind().known().ok_or(Error::InvalidGraph)
}
fn required_message(message: Option<&'static Message>) -> Result<&'static Message, Error> {
    message.ok_or(Error::InvalidGraph)
}
// port: tsc/internal/transformers/declarations/diagnostics.go:addParentDeclarationRelatedInfo
fn add_parent_related_info(
    view: AstView<'_>,
    node: NodeId,
    diagnostic: &mut tsr_ast::Diagnostic,
) -> Result<(), Error> {
    if let Some(declaration) = nearest_declaration(view, node)? {
        let text = declaration_target_text(view, declaration)?;
        let related = diagnostic_for_node(
            view,
            Some(declaration),
            required_message(related_suggestion(kind(view, declaration)?))?,
            vec![text],
        )?;
        diagnostic
            .related_information
            .push(std::sync::Arc::new(related));
    }
    Ok(())
}

// port: tsc/internal/transformers/declarations/diagnostics.go:createExpressionErrorEx
fn expression_error(
    view: AstView<'_>,
    node: NodeId,
    mut message: Option<&'static Message>,
) -> Result<tsr_ast::Diagnostic, Error> {
    let Some(declaration) = nearest_declaration(view, node)? else {
        return diagnostic_for_node(
            view,
            Some(node),
            message.unwrap_or(d::Expression_type_can_t_be_inferred_with_isolatedDeclarations),
            vec![],
        );
    };
    let text = declaration_target_text(view, declaration)?;
    let mut target = view.node(node)?.parent();
    while let Some(parent) = target {
        let read = view.node(parent)?;
        if read.kind() == K::ExportAssignment {
            break;
        }
        if tsr_ast::utilities::is_statement(view, parent)? {
            target = None;
            break;
        }
        if read.kind() != K::ParenthesizedExpression
            && !tsr_ast::utilities::is_assertion_expression(&read)
        {
            break;
        }
        target = read.parent();
    }
    let direct = target == Some(declaration);
    if message.is_none() {
        message = if direct {
            isolated_error_message(kind(view, declaration)?)
        } else {
            Some(d::Expression_type_can_t_be_inferred_with_isolatedDeclarations)
        };
    }
    let mut diagnostic = diagnostic_for_node(view, Some(node), required_message(message)?, vec![])?;
    diagnostic
        .related_information
        .push(std::sync::Arc::new(diagnostic_for_node(
            view,
            Some(declaration),
            required_message(related_suggestion(kind(view, declaration)?))?,
            vec![text],
        )?));
    if !direct {
        diagnostic.related_information.push(std::sync::Arc::new(diagnostic_for_node(view,Some(node),d::Add_satisfies_and_a_type_assertion_to_this_expression_satisfies_T_as_T_to_make_the_type_explicit,vec![])?));
    }
    Ok(diagnostic)
}

// port: tsc/internal/transformers/declarations/diagnostics.go:createAccessorTypeError
fn accessor_error<R: tsr_printer::emit_resolver::DeclarationEmitResolver>(
    resolver: &mut R,
    node: NodeId,
) -> Result<tsr_ast::Diagnostic, R::Error> {
    let current_kind = kind(resolver.ast(node)?, node)?;
    let other_kind = if current_kind == K::SetAccessor {
        K::GetAccessor
    } else {
        K::SetAccessor
    };
    let symbol = resolver
        .bound_symbol_of_declaration(node)?
        .ok_or(Error::InvalidGraph)?;
    let mut other = None;
    for declaration in resolver.symbol_declarations(symbol)? {
        if kind(resolver.ast(declaration)?, declaration)? == other_kind {
            other = Some(declaration);
            break;
        }
    }
    let (getter, setter) = if current_kind == K::SetAccessor {
        (other, Some(node))
    } else {
        (Some(node), other)
    };
    let view = resolver.ast(node)?;
    let mut target = node;
    if current_kind == K::SetAccessor {
        let parameters = view.node(node)?.parameters(view)?;
        if let Some(Some(parameter)) = view.node_slice(parameters)?.get(0) {
            target = parameter;
        }
    }
    let mut diagnostic = diagnostic_for_node(
        view,
        Some(target),
        required_message(isolated_error_message(current_kind))?,
        vec![],
    )?;
    // Native related order is setter then getter, independent of source order.
    for declaration in [setter, getter].into_iter().flatten() {
        let view = resolver.ast(declaration)?;
        diagnostic
            .related_information
            .push(std::sync::Arc::new(diagnostic_for_node(
                view,
                Some(declaration),
                required_message(related_suggestion(kind(view, declaration)?))?,
                vec![],
            )?));
    }
    Ok(diagnostic)
}

// port: tsc/internal/ast/utilities.go:isPartOfTypeExpressionWithTypeArguments
fn type_heritage(view: AstView<'_>, node: NodeId) -> Result<bool, Error> {
    let owner = parent(view, node)?;
    let read = view.node(owner)?;
    Ok(match read.kind().known() {
        Some(K::HeritageClause) => {
            let container = parent(view, owner)?;
            !matches!(
                view.node(container)?.kind().known(),
                Some(K::ClassDeclaration | K::ClassExpression)
            ) || read
                .data_source()
                .as_heritage_clause()
                .ok_or(Error::InvalidGraph)?
                .token()
                == K::ImplementsKeyword
        }
        Some(K::JSDocImplementsTag | K::JSDocAugmentsTag) => true,
        _ => false,
    })
}
fn type_kind_range(kind: K) -> bool {
    (K::FirstTypeNode..=K::LastTypeNode).contains(&kind)
}
// port: tsc/internal/ast/utilities.go:isPartOfTypeNodeInParent
fn part_of_type_in_parent(view: AstView<'_>, node: NodeId) -> Result<bool, Error> {
    let owner = parent(view, node)?;
    let read = view.node(owner)?;
    let Some(kind) = read.kind().known() else {
        return Err(Error::InvalidGraph);
    };
    if kind == K::TypeQuery {
        return Ok(false);
    }
    if kind == K::ImportType {
        return Ok(!read
            .data_source()
            .as_import_type_node()
            .ok_or(Error::InvalidGraph)?
            .is_type_of());
    }
    if type_kind_range(kind) {
        return Ok(true);
    }
    Ok(match kind {
        K::ExpressionWithTypeArguments => type_heritage(view, owner)?,
        K::TypeParameter => {
            read.data_source()
                .as_type_parameter_declaration()
                .ok_or(Error::InvalidGraph)?
                .constraint()
                == Some(node)
        }
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
        | K::TypeAssertionExpression => read.type_node() == Some(node),
        K::CallExpression | K::NewExpression | K::TaggedTemplateExpression => view
            .node_slice(read.type_arguments(view)?)?
            .iter()
            .flatten()
            .any(|argument| argument == node),
        _ => false,
    })
}
// port: tsc/internal/ast/utilities.go:IsPartOfTypeNode
pub(super) fn part_of_type_node(view: AstView<'_>, node: NodeId) -> Result<bool, Error> {
    let read = view.node(node)?;
    let kind = read.kind().known().ok_or(Error::InvalidGraph)?;
    if type_kind_range(kind) {
        return Ok(true);
    }
    Ok(match kind {
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
        | K::NeverKeyword => true,
        K::VoidKeyword => view.node(parent(view, node)?)?.kind() != K::VoidExpression,
        K::ExpressionWithTypeArguments => type_heritage(view, node)?,
        K::TypeParameter => matches!(
            view.node(parent(view, node)?)?.kind().known(),
            Some(K::MappedType | K::InferType)
        ),
        K::Identifier => {
            let owner = parent(view, node)?;
            let owner_read = view.node(owner)?;
            let right = if let Some(data) = owner_read.data_source().as_qualified_name() {
                data.right()
            } else if owner_read.kind() == K::PropertyAccessExpression {
                owner_read.name()
            } else {
                None
            };
            part_of_type_in_parent(view, if right == Some(node) { owner } else { node })?
        }
        K::QualifiedName | K::PropertyAccessExpression | K::ThisKeyword => {
            part_of_type_in_parent(view, node)?
        }
        _ => false,
    })
}

// port: tsc/internal/transformers/declarations/diagnostics.go:createGetIsolatedDeclarationErrors
pub(super) fn isolated_declaration_error<R: tsr_printer::emit_resolver::DeclarationEmitResolver>(
    resolver: &mut R,
    node: NodeId,
) -> Result<tsr_ast::Diagnostic, R::Error> {
    let view = resolver.ast(node)?;
    let mut ancestor = Some(node);
    while let Some(current) = ancestor {
        let read = view.node(current)?;
        if read.kind() == K::HeritageClause {
            return Ok(diagnostic_for_node(
                view,
                Some(node),
                d::Extends_clause_can_t_contain_an_expression_with_isolatedDeclarations,
                vec![],
            )?);
        }
        ancestor = read.parent();
    }
    let node_kind = kind(view, node)?;
    if part_of_type_node(view, node)?
        || node_kind == K::TypeQuery
        || tsr_ast::utilities::is_entity_name(&view.node(node)?)
        || tsr_ast::is_entity_name_expression(view, node)?
    {
        let mut diagnostic = diagnostic_for_node(
            view,
            Some(node),
            d::Type_containing_private_name_0_can_t_be_used_with_isolatedDeclarations,
            vec![tsr_scanner::get_text_of_node(view, node)?],
        )?;
        add_parent_related_info(view, node, &mut diagnostic)?;
        return Ok(diagnostic);
    }
    let diagnostic=match node_kind {
        K::GetAccessor|K::SetAccessor=>return accessor_error(resolver,node),
        K::ComputedPropertyName|K::ShorthandPropertyAssignment|K::SpreadAssignment|K::ArrayLiteralExpression|K::SpreadElement=>{
            let mut diagnostic=diagnostic_for_node(view,Some(node),required_message(isolated_error_message(node_kind))?,vec![])?;
            add_parent_related_info(view,node,&mut diagnostic)?;
            diagnostic
        }
        K::MethodDeclaration|K::ConstructSignature|K::FunctionExpression|K::ArrowFunction|K::FunctionDeclaration=>{
            let mut diagnostic=diagnostic_for_node(view,Some(node),required_message(isolated_error_message(node_kind))?,vec![])?;
            add_parent_related_info(view,node,&mut diagnostic)?;
            diagnostic.related_information.push(std::sync::Arc::new(diagnostic_for_node(view,Some(node),required_message(related_suggestion(node_kind))?,vec![])?));
            diagnostic
        }
        K::BindingElement=>diagnostic_for_node(view,Some(node),d::Binding_elements_with_initializers_can_t_be_exported_directly_with_isolatedDeclarations,vec![])?,
        K::PropertyDeclaration|K::VariableDeclaration=>{
            let mut diagnostic=diagnostic_for_node(view,Some(node),required_message(isolated_error_message(node_kind))?,vec![])?;
            let name=view.node(node)?.name().ok_or(Error::InvalidGraph)?;
            diagnostic.related_information.push(std::sync::Arc::new(diagnostic_for_node(view,Some(node),required_message(related_suggestion(node_kind))?,vec![tsr_scanner::get_text_of_node(view,name)?])?));
            diagnostic
        }
        K::Parameter=>{
            let owner=parent(view,node)?;
            if view.node(owner)?.kind()==K::SetAccessor{return accessor_error(resolver,owner);}
            let add_undefined=resolver.requires_adding_implicit_undefined_unsafe(node,None,None)?;
            let view=resolver.ast(node)?;
            if !add_undefined {
                if let Some(initializer)=view.node(node)?.initializer(){return Ok(expression_error(view,initializer,None)?);}
            }
            let message=if add_undefined{d::Declaration_emit_for_this_parameter_requires_implicitly_adding_undefined_to_its_type_This_is_not_supported_with_isolatedDeclarations}else{required_message(isolated_error_message(node_kind))?};
            let mut diagnostic=diagnostic_for_node(view,Some(node),message,vec![])?;
            let name=view.node(node)?.name().ok_or(Error::InvalidGraph)?;
            diagnostic.related_information.push(std::sync::Arc::new(diagnostic_for_node(view,Some(node),required_message(related_suggestion(node_kind))?,vec![tsr_scanner::get_text_of_node(view,name)?])?));
            diagnostic
        }
        K::PropertyAssignment=>expression_error(view,view.node(node)?.initializer().ok_or(Error::InvalidGraph)?,None)?,
        K::ClassExpression=>expression_error(view,node,Some(d::Inference_from_class_expressions_is_not_supported_with_isolatedDeclarations))?,
        _=>expression_error(view,node,None)?,
    };
    Ok(diagnostic)
}

#[cfg(test)]
#[path = "diagnostics_tests.rs"]
mod tests;
