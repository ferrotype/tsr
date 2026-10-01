//! Declaration accessibility diagnostics preserve native diagnostic locations
//! and module-name distinctions. The pin's diagnostic selectors are closures
//! over a node; here they are `GetSymbolAccessibilityDiagnostic` values
//! naming the node and the message selector, evaluated against a view of the
//! node's file.
use tsr_arena::{Error, NodeId};
use tsr_ast::{modifier_flags as mf, AstView, NodeKind, SyntaxKind as K};
use tsr_diagnostics::{self as d, Message};
use tsr_printer::emit_resolver::{SymbolAccessibility, SymbolAccessibilityResult};

#[derive(Clone, Copy, Debug)]
pub struct SymbolAccessibilityDiagnostic {
    pub error_node: Option<NodeId>,
    pub diagnostic_message: &'static Message,
    pub type_name: Option<NodeId>,
}

/// Why a selector gave no diagnostic: a storage failure, or the pin's panic,
/// raised when the selector is evaluated.
#[derive(Clone, Debug)]
pub(super) enum Failure {
    Arena(Error),
    Panic(String),
}
impl From<Error> for Failure {
    fn from(error: Error) -> Self {
        Self::Arena(error)
    }
}

/// Go's `ast.Kind.String()`.
pub(super) fn kind_string(kind: NodeKind) -> String {
    match kind.known() {
        Some(kind) => format!("Kind{}", kind.as_str()),
        None => format!("Kind({})", kind.raw()),
    }
}

/// The message selectors a wrapped selector calls.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum MessageSelector {
    AccessorName,
    MethodName,
    VariableDeclarationType,
    AccessorDeclarationType,
    ReturnType,
    ParameterDeclarationType,
    TypeParameterConstraint,
    ImportDeclaration,
}

/// Go's `GetSymbolAccessibilityDiagnostic` closures.
#[derive(Clone, Copy, Debug)]
pub(super) enum GetSymbolAccessibilityDiagnostic {
    Simple(NodeId, MessageSelector),
    Named(NodeId, MessageSelector),
    Fallback(NodeId, MessageSelector),
    /// The inline closure of an `ExpressionWithTypeArguments` context.
    HeritageClause(NodeId),
    /// The inline closure of a type alias context.
    TypeAlias(NodeId),
    /// The inline closure of an `Object.defineProperty` call context.
    DefineProperty(NodeId),
}

const NIL: &str = "runtime error: invalid memory address or nil pointer dereference";

/// A selector's `node.Parent`: a nil parent is the pin's dereference panic,
/// raised when the selector is called.
fn parent(view: AstView<'_>, node: NodeId) -> Result<NodeId, Failure> {
    view.node(node)?
        .parent()
        .ok_or_else(|| Failure::Panic(NIL.to_owned()))
}
fn static_node(view: AstView<'_>, node: NodeId) -> Result<bool, Error> {
    tsr_ast::utilities::is_static(view, node)
}
fn class_parent(view: AstView<'_>, node: NodeId) -> Result<bool, Failure> {
    Ok(view.node(parent(view, node)?)?.kind() == K::ClassDeclaration)
}
fn select_message(
    view: AstView<'_>,
    selector: MessageSelector,
    node: NodeId,
    result: &SymbolAccessibilityResult,
) -> Result<Option<&'static Message>, Failure> {
    Ok(Some(match selector {
        MessageSelector::AccessorName => accessor_name_message(view, node, result)?,
        MessageSelector::MethodName => method_name_message(view, node, result)?,
        MessageSelector::VariableDeclarationType => return variable_message(view, node, result),
        MessageSelector::AccessorDeclarationType => accessor_type_message(view, node, result)?,
        MessageSelector::ReturnType => return_message(view, node, result)?,
        MessageSelector::ParameterDeclarationType => parameter_message(view, node, result)?,
        MessageSelector::TypeParameterConstraint => type_parameter_message(view, node)?,
        MessageSelector::ImportDeclaration => d::Import_declaration_0_is_using_private_name_1,
    }))
}

// port: tsc/internal/transformers/declarations/diagnostics.go:wrapSimpleDiagnosticSelector
fn wrap_simple_diagnostic_selector(
    view: AstView<'_>,
    node: NodeId,
    selector: MessageSelector,
    result: &SymbolAccessibilityResult,
) -> Result<Option<SymbolAccessibilityDiagnostic>, Failure> {
    let Some(diagnostic_message) = select_message(view, selector, node, result)? else {
        return Ok(None);
    };
    Ok(Some(SymbolAccessibilityDiagnostic {
        error_node: Some(node),
        diagnostic_message,
        type_name: tsr_ast::get_name_of_declaration(view, Some(node))?,
    }))
}

// port: tsc/internal/transformers/declarations/diagnostics.go:wrapNamedDiagnosticSelector
fn wrap_named_diagnostic_selector(
    view: AstView<'_>,
    node: NodeId,
    selector: MessageSelector,
    result: &SymbolAccessibilityResult,
) -> Result<Option<SymbolAccessibilityDiagnostic>, Failure> {
    let Some(diagnostic_message) = select_message(view, selector, node, result)? else {
        return Ok(None);
    };
    let name = tsr_ast::get_name_of_declaration(view, Some(node))?;
    Ok(Some(SymbolAccessibilityDiagnostic {
        error_node: name,
        diagnostic_message,
        type_name: name,
    }))
}

// port: tsc/internal/transformers/declarations/diagnostics.go:wrapFallbackErrorDiagnosticSelector
fn wrap_fallback_error_diagnostic_selector(
    view: AstView<'_>,
    node: NodeId,
    selector: MessageSelector,
    result: &SymbolAccessibilityResult,
) -> Result<Option<SymbolAccessibilityDiagnostic>, Failure> {
    let Some(diagnostic_message) = select_message(view, selector, node, result)? else {
        return Ok(None);
    };
    let error_node = tsr_ast::get_name_of_declaration(view, Some(node))?.or(Some(node));
    Ok(Some(SymbolAccessibilityDiagnostic {
        error_node,
        diagnostic_message,
        type_name: None,
    }))
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
) -> Result<&'static Message, Failure> {
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
) -> Result<&'static Message, Failure> {
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
) -> Result<Option<&'static Message>, Failure> {
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
) -> Result<&'static Message, Failure> {
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
) -> Result<&'static Message, Failure> {
    Ok(match view.node(node)?.kind().known() {
        Some(K::ConstructSignature)=>private_message(result, d::Return_type_of_constructor_signature_from_exported_interface_has_or_is_using_name_0_from_private_module_1, d::Return_type_of_constructor_signature_from_exported_interface_has_or_is_using_private_name_0),
        Some(K::CallSignature)=>private_message(result, d::Return_type_of_call_signature_from_exported_interface_has_or_is_using_name_0_from_private_module_1, d::Return_type_of_call_signature_from_exported_interface_has_or_is_using_private_name_0),
        Some(K::IndexSignature)=>private_message(result, d::Return_type_of_index_signature_from_exported_interface_has_or_is_using_name_0_from_private_module_1, d::Return_type_of_index_signature_from_exported_interface_has_or_is_using_private_name_0),
        Some(K::MethodDeclaration|K::MethodSignature)=>if static_node(view,node)? { module_message(result, d::Return_type_of_public_static_method_from_exported_class_has_or_is_using_name_0_from_external_module_1_but_cannot_be_named, d::Return_type_of_public_static_method_from_exported_class_has_or_is_using_name_0_from_private_module_1, d::Return_type_of_public_static_method_from_exported_class_has_or_is_using_private_name_0) } else if class_parent(view,node)? { module_message(result, d::Return_type_of_public_method_from_exported_class_has_or_is_using_name_0_from_external_module_1_but_cannot_be_named, d::Return_type_of_public_method_from_exported_class_has_or_is_using_name_0_from_private_module_1, d::Return_type_of_public_method_from_exported_class_has_or_is_using_private_name_0) } else { private_message(result, d::Return_type_of_method_from_exported_interface_has_or_is_using_name_0_from_private_module_1, d::Return_type_of_method_from_exported_interface_has_or_is_using_private_name_0) },
        Some(K::FunctionDeclaration)=>module_message(result, d::Return_type_of_exported_function_has_or_is_using_name_0_from_external_module_1_but_cannot_be_named, d::Return_type_of_exported_function_has_or_is_using_name_0_from_private_module_1, d::Return_type_of_exported_function_has_or_is_using_private_name_0),
        _=>return Err(Failure::Panic(format!("This is unknown kind for signature: {}", kind_string(view.node(node)?.kind())))),
    })
}

// port: tsc/internal/transformers/declarations/diagnostics.go:getParameterDeclarationTypeVisibilityDiagnosticMessage
fn parameter_message(
    view: AstView<'_>,
    node: NodeId,
    result: &SymbolAccessibilityResult,
) -> Result<&'static Message, Failure> {
    let owner = parent(view, node)?;
    Ok(match view.node(owner)?.kind().known() {
        Some(K::Constructor)=>module_message(result, d::Parameter_0_of_constructor_from_exported_class_has_or_is_using_name_1_from_external_module_2_but_cannot_be_named, d::Parameter_0_of_constructor_from_exported_class_has_or_is_using_name_1_from_private_module_2, d::Parameter_0_of_constructor_from_exported_class_has_or_is_using_private_name_1),
        Some(K::ConstructSignature|K::ConstructorType)=>private_message(result, d::Parameter_0_of_constructor_signature_from_exported_interface_has_or_is_using_name_1_from_private_module_2, d::Parameter_0_of_constructor_signature_from_exported_interface_has_or_is_using_private_name_1),
        Some(K::CallSignature)=>private_message(result, d::Parameter_0_of_call_signature_from_exported_interface_has_or_is_using_name_1_from_private_module_2, d::Parameter_0_of_call_signature_from_exported_interface_has_or_is_using_private_name_1),
        Some(K::IndexSignature)=>private_message(result, d::Parameter_0_of_index_signature_from_exported_interface_has_or_is_using_name_1_from_private_module_2, d::Parameter_0_of_index_signature_from_exported_interface_has_or_is_using_private_name_1),
        Some(K::MethodDeclaration|K::MethodSignature)=>if static_node(view,owner)? { module_message(result, d::Parameter_0_of_public_static_method_from_exported_class_has_or_is_using_name_1_from_external_module_2_but_cannot_be_named, d::Parameter_0_of_public_static_method_from_exported_class_has_or_is_using_name_1_from_private_module_2, d::Parameter_0_of_public_static_method_from_exported_class_has_or_is_using_private_name_1) } else if class_parent(view,owner)? { module_message(result, d::Parameter_0_of_public_method_from_exported_class_has_or_is_using_name_1_from_external_module_2_but_cannot_be_named, d::Parameter_0_of_public_method_from_exported_class_has_or_is_using_name_1_from_private_module_2, d::Parameter_0_of_public_method_from_exported_class_has_or_is_using_private_name_1) } else { private_message(result, d::Parameter_0_of_method_from_exported_interface_has_or_is_using_name_1_from_private_module_2, d::Parameter_0_of_method_from_exported_interface_has_or_is_using_private_name_1) },
        Some(K::FunctionDeclaration|K::FunctionType|K::ArrowFunction|K::FunctionExpression)=>module_message(result, d::Parameter_0_of_exported_function_has_or_is_using_name_1_from_external_module_2_but_cannot_be_named, d::Parameter_0_of_exported_function_has_or_is_using_name_1_from_private_module_2, d::Parameter_0_of_exported_function_has_or_is_using_private_name_1),
        Some(K::SetAccessor|K::GetAccessor)=>module_message(result, d::Parameter_0_of_accessor_has_or_is_using_name_1_from_external_module_2_but_cannot_be_named, d::Parameter_0_of_accessor_has_or_is_using_name_1_from_private_module_2, d::Parameter_0_of_accessor_has_or_is_using_private_name_1),
        _=>return Err(Failure::Panic(format!("Unknown parent for parameter: {}", kind_string(view.node(owner)?.kind())))),
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
fn type_parameter_message(view: AstView<'_>, node: NodeId) -> Result<&'static Message, Failure> {
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
        _=>return Err(Failure::Panic(format!("This is unknown parent for type parameter: {}", kind_string(view.node(owner)?.kind())))),
    })
}

// port: tsc/internal/transformers/declarations/diagnostics.go:createGetSymbolAccessibilityDiagnosticForNodeName
pub(super) fn create_get_symbol_accessibility_diagnostic_for_node_name(
    view: AstView<'_>,
    node: NodeId,
) -> Result<GetSymbolAccessibilityDiagnostic, Error> {
    let kind = view.node(node)?.kind().known();
    if matches!(kind, Some(K::SetAccessor | K::GetAccessor)) {
        Ok(GetSymbolAccessibilityDiagnostic::Simple(
            node,
            MessageSelector::AccessorName,
        ))
    } else if matches!(kind, Some(K::MethodDeclaration | K::MethodSignature)) {
        Ok(GetSymbolAccessibilityDiagnostic::Simple(
            node,
            MessageSelector::MethodName,
        ))
    } else {
        create_get_symbol_accessibility_diagnostic_for_node(view, node)
    }
}

// port: tsc/internal/transformers/declarations/diagnostics.go:createGetSymbolAccessibilityDiagnosticForNode
pub(super) fn create_get_symbol_accessibility_diagnostic_for_node(
    view: AstView<'_>,
    node: NodeId,
) -> Result<GetSymbolAccessibilityDiagnostic, Error> {
    use GetSymbolAccessibilityDiagnostic as G;
    let read = view.node(node)?;
    Ok(match read.kind().known() {
        Some(
            K::VariableDeclaration
            | K::PropertyDeclaration
            | K::PropertySignature
            | K::PropertyAccessExpression
            | K::ElementAccessExpression
            | K::BinaryExpression
            | K::BindingElement
            | K::Constructor,
        ) => G::Simple(node, MessageSelector::VariableDeclarationType),
        Some(K::SetAccessor | K::GetAccessor) => {
            G::Named(node, MessageSelector::AccessorDeclarationType)
        }
        Some(
            K::ConstructSignature
            | K::CallSignature
            | K::MethodDeclaration
            | K::MethodSignature
            | K::FunctionDeclaration
            | K::IndexSignature,
        ) => G::Fallback(node, MessageSelector::ReturnType),
        Some(K::Parameter) => {
            let owner = read.parent().expect(NIL);
            if tsr_ast::utilities::is_parameter_property_declaration(view, node, owner)?
                && tsr_ast::utilities::has_syntactic_modifier(view, owner, mf::PRIVATE)?
            {
                G::Simple(node, MessageSelector::VariableDeclarationType)
            } else {
                G::Simple(node, MessageSelector::ParameterDeclarationType)
            }
        }
        Some(K::TypeParameter) => G::Simple(node, MessageSelector::TypeParameterConstraint),
        Some(K::ExpressionWithTypeArguments) => G::HeritageClause(node),
        Some(K::ImportEqualsDeclaration) => G::Simple(node, MessageSelector::ImportDeclaration),
        Some(K::TypeAliasDeclaration | K::JSTypeAliasDeclaration) => G::TypeAlias(node),
        Some(K::CallExpression) => G::DefineProperty(node),
        _ => panic!(
            "Attempted to set a declaration diagnostic context for unhandled node kind: {}",
            kind_string(read.kind())
        ),
    })
}

// port: tsc/internal/transformers/declarations/transform.go:throwDiagnostic
pub(super) fn throw_diagnostic() -> Failure {
    Failure::Panic("Diagnostic emitted without context".to_owned())
}

impl GetSymbolAccessibilityDiagnostic {
    /// Calls the closure with one accessibility result.
    pub(super) fn evaluate(
        self,
        view: AstView<'_>,
        result: &SymbolAccessibilityResult,
    ) -> Result<Option<SymbolAccessibilityDiagnostic>, Failure> {
        match self {
            Self::Simple(node, selector) => {
                wrap_simple_diagnostic_selector(view, node, selector, result)
            }
            Self::Named(node, selector) => {
                wrap_named_diagnostic_selector(view, node, selector, result)
            }
            Self::Fallback(node, selector) => {
                wrap_fallback_error_diagnostic_selector(view, node, selector, result)
            }
            Self::HeritageClause(node) => {
                let clause = parent(view, node)?;
                let owner = parent(view, clause)?;
                // Heritage clause is written by user so it can always be named
                let diagnostic_message = if view.node(owner)?.kind() == K::ClassDeclaration {
                    // Class or Interface implemented/extended is inaccessible
                    let implements = view.node(clause)?.kind() == K::HeritageClause
                        && view
                            .node(clause)?
                            .data_source()
                            .as_heritage_clause()
                            .expect("heritage clause payload")
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
                    // interface is inaccessible
                    d::X_extends_clause_of_exported_interface_0_has_or_is_using_private_name_1
                };
                Ok(Some(SymbolAccessibilityDiagnostic {
                    diagnostic_message,
                    error_node: Some(node),
                    type_name: tsr_ast::get_name_of_declaration(view, Some(owner))?,
                }))
            }
            Self::TypeAlias(node) => {
                let diagnostic_message = private_message(
                    result,
                    d::Exported_type_alias_0_has_or_is_using_private_name_1_from_module_2,
                    d::Exported_type_alias_0_has_or_is_using_private_name_1,
                );
                let read = view.node(node)?;
                Ok(Some(SymbolAccessibilityDiagnostic {
                    error_node: read.type_node(),
                    diagnostic_message,
                    type_name: read.name(),
                }))
            }
            Self::DefineProperty(node) => {
                let diagnostic_message = module_message(
                    result,
                    d::Exported_variable_0_has_or_is_using_name_1_from_external_module_2_but_cannot_be_named,
                    d::Exported_variable_0_has_or_is_using_name_1_from_private_module_2,
                    d::Exported_variable_0_has_or_is_using_private_name_1,
                );
                let arguments = view.node(node)?.arguments(view)?;
                let arguments = view.node_slice(arguments)?;
                let Some(target) = arguments.get(1) else {
                    return Err(Failure::Panic(format!(
                        "runtime error: index out of range [1] with length {}",
                        arguments.len()
                    )));
                };
                Ok(Some(SymbolAccessibilityDiagnostic {
                    error_node: target,
                    diagnostic_message,
                    type_name: target,
                }))
            }
        }
    }
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

// port: tsc/internal/transformers/declarations/tracker.go:createDiagnosticForNode
pub(super) fn create_diagnostic_for_node(
    view: AstView<'_>,
    node: NodeId,
    message: &'static Message,
    args: Vec<tsr_ast::JsString>,
) -> Result<tsr_ast::Diagnostic, Error> {
    diagnostic_for_node(view, Some(node), message, args)
}

fn add_related_info(diagnostic: &mut tsr_ast::Diagnostic, related: tsr_ast::Diagnostic) {
    diagnostic
        .related_information
        .push(std::sync::Arc::new(related));
}

// port: tsc/internal/transformers/declarations/diagnostics.go:isDeclarationEnoughForErrors
fn is_declaration_enough_for_errors(view: AstView<'_>, node: NodeId) -> Result<bool, Error> {
    Ok(matches!(
        view.node(node)?.kind().known(),
        Some(K::ExportAssignment | K::VariableDeclaration | K::PropertyDeclaration | K::Parameter)
    ) || tsr_ast::utilities::is_statement(view, node)?)
}

// port: tsc/internal/transformers/declarations/diagnostics.go:isFunctionLikeAndNotConstructor
fn is_function_like_and_not_constructor(view: AstView<'_>, node: NodeId) -> Result<bool, Error> {
    let read = view.node(node)?;
    Ok(
        tsr_ast::utilities::is_function_like_declaration(Some(&read))
            && read.kind() != K::Constructor,
    )
}

fn find_ancestor_by(
    view: AstView<'_>,
    mut node: Option<NodeId>,
    predicate: fn(AstView<'_>, NodeId) -> Result<bool, Error>,
) -> Result<Option<NodeId>, Error> {
    while let Some(current) = node {
        if predicate(view, current)? {
            return Ok(Some(current));
        }
        node = view.node(current)?.parent();
    }
    Ok(None)
}

// port: tsc/internal/transformers/declarations/diagnostics.go:findNearestDeclaration
fn find_nearest_declaration(view: AstView<'_>, node: NodeId) -> Result<Option<NodeId>, Error> {
    let Some(result) = find_ancestor_by(view, Some(node), is_declaration_enough_for_errors)? else {
        return Ok(None);
    };
    let kind = view.node(result)?.kind();
    if kind == K::ExportAssignment {
        return Ok(Some(result));
    }
    if kind == K::ReturnStatement {
        return find_ancestor_by(view, Some(result), is_function_like_and_not_constructor);
    }
    if tsr_ast::utilities::is_statement(view, result)? {
        return Ok(None);
    }
    Ok(Some(result))
}

fn declaration_target_text(view: AstView<'_>, node: NodeId) -> Result<tsr_ast::JsString, Error> {
    if view.node(node)?.kind() != K::ExportAssignment {
        if let Some(name) = view.node(node)?.name() {
            return tsr_scanner::get_text_of_node(view, name);
        }
    }
    Ok(tsr_ast::JsString::default())
}

/// A nil message reaching `NewDiagnosticForNode` is the pin's nil dereference.
fn required_message(message: Option<&'static Message>) -> &'static Message {
    message.expect(NIL)
}

fn kind(view: AstView<'_>, node: NodeId) -> Result<K, Error> {
    view.node(node)?.kind().known().ok_or(Error::InvalidGraph)
}

// port: tsc/internal/transformers/declarations/diagnostics.go:createEntityInTypeNodeError
fn create_entity_in_type_node_error(
    view: AstView<'_>,
    node: NodeId,
) -> Result<tsr_ast::Diagnostic, Error> {
    let mut diagnostic = create_diagnostic_for_node(
        view,
        node,
        d::Type_containing_private_name_0_can_t_be_used_with_isolatedDeclarations,
        vec![tsr_scanner::get_text_of_node(view, node)?],
    )?;
    add_parent_declaration_related_info(view, node, &mut diagnostic)?;
    Ok(diagnostic)
}

// port: tsc/internal/transformers/declarations/diagnostics.go:addParentDeclarationRelatedInfo
fn add_parent_declaration_related_info(
    view: AstView<'_>,
    node: NodeId,
    diagnostic: &mut tsr_ast::Diagnostic,
) -> Result<(), Error> {
    let Some(parent_declaration) = find_nearest_declaration(view, node)? else {
        return Ok(());
    };
    let target = declaration_target_text(view, parent_declaration)?;
    let related = create_diagnostic_for_node(
        view,
        parent_declaration,
        required_message(related_suggestion(kind(view, parent_declaration)?)),
        vec![target],
    )?;
    add_related_info(diagnostic, related);
    Ok(())
}

// port: tsc/internal/transformers/declarations/diagnostics.go:createAccessorTypeError
fn create_accessor_type_error<R: tsr_printer::emit_resolver::DeclarationEmitResolver>(
    resolver: &mut R,
    node: NodeId,
) -> Result<tsr_ast::Diagnostic, R::Error> {
    let current_kind = kind(resolver.ast(node)?, node)?;
    let symbol = resolver.bound_symbol_of_declaration(node)?.expect(NIL);
    let declarations = resolver.symbol_declarations(symbol)?;
    let view = resolver.ast(node)?;
    let all = tsr_ast::utilities_class::get_all_accessor_declarations_for_declaration(
        view,
        node,
        &declarations,
    )?;
    let (get_accessor, set_accessor) = (all.get_accessor, all.set_accessor);
    let mut target = node;
    if current_kind == K::SetAccessor {
        let parameters = view.node(node)?.parameters(view)?;
        if let Some(Some(parameter)) = view.node_slice(parameters)?.get(0) {
            target = parameter;
        }
    }
    let mut diagnostic = create_diagnostic_for_node(
        view,
        target,
        required_message(isolated_error_message(current_kind)),
        vec![],
    )?;
    if let Some(set_accessor) = set_accessor {
        let view = resolver.ast(set_accessor)?;
        let related = create_diagnostic_for_node(
            view,
            set_accessor,
            required_message(related_suggestion(kind(view, set_accessor)?)),
            vec![],
        )?;
        add_related_info(&mut diagnostic, related);
    }
    if let Some(get_accessor) = get_accessor {
        let view = resolver.ast(get_accessor)?;
        let related = create_diagnostic_for_node(
            view,
            get_accessor,
            required_message(related_suggestion(kind(view, get_accessor)?)),
            vec![],
        )?;
        add_related_info(&mut diagnostic, related);
    }
    Ok(diagnostic)
}

// port: tsc/internal/transformers/declarations/diagnostics.go:createObjectLiteralError
fn create_object_literal_error(
    view: AstView<'_>,
    node: NodeId,
) -> Result<tsr_ast::Diagnostic, Error> {
    let mut diagnostic = create_diagnostic_for_node(
        view,
        node,
        required_message(isolated_error_message(kind(view, node)?)),
        vec![],
    )?;
    add_parent_declaration_related_info(view, node, &mut diagnostic)?;
    Ok(diagnostic)
}

// port: tsc/internal/transformers/declarations/diagnostics.go:createArrayLiteralError
fn create_array_literal_error(
    view: AstView<'_>,
    node: NodeId,
) -> Result<tsr_ast::Diagnostic, Error> {
    let mut diagnostic = create_diagnostic_for_node(
        view,
        node,
        required_message(isolated_error_message(kind(view, node)?)),
        vec![],
    )?;
    add_parent_declaration_related_info(view, node, &mut diagnostic)?;
    Ok(diagnostic)
}

// port: tsc/internal/transformers/declarations/diagnostics.go:createReturnTypeError
fn create_return_type_error(view: AstView<'_>, node: NodeId) -> Result<tsr_ast::Diagnostic, Error> {
    let node_kind = kind(view, node)?;
    let mut diagnostic = create_diagnostic_for_node(
        view,
        node,
        required_message(isolated_error_message(node_kind)),
        vec![],
    )?;
    add_parent_declaration_related_info(view, node, &mut diagnostic)?;
    let related = create_diagnostic_for_node(
        view,
        node,
        required_message(related_suggestion(node_kind)),
        vec![],
    )?;
    add_related_info(&mut diagnostic, related);
    Ok(diagnostic)
}

// port: tsc/internal/transformers/declarations/diagnostics.go:createBindingElementError
fn create_binding_element_error(
    view: AstView<'_>,
    node: NodeId,
) -> Result<tsr_ast::Diagnostic, Error> {
    create_diagnostic_for_node(
        view,
        node,
        d::Binding_elements_with_initializers_can_t_be_exported_directly_with_isolatedDeclarations,
        vec![],
    )
}

// port: tsc/internal/transformers/declarations/diagnostics.go:createVariableOrPropertyError
fn create_variable_or_property_error(
    view: AstView<'_>,
    node: NodeId,
) -> Result<tsr_ast::Diagnostic, Error> {
    let node_kind = kind(view, node)?;
    let mut diagnostic = create_diagnostic_for_node(
        view,
        node,
        required_message(isolated_error_message(node_kind)),
        vec![],
    )?;
    let name = view.node(node)?.name().expect(NIL);
    let related = create_diagnostic_for_node(
        view,
        node,
        required_message(related_suggestion(node_kind)),
        vec![tsr_scanner::get_text_of_node(view, name)?],
    )?;
    add_related_info(&mut diagnostic, related);
    Ok(diagnostic)
}

// port: tsc/internal/transformers/declarations/diagnostics.go:createExpressionError
fn create_expression_error(view: AstView<'_>, node: NodeId) -> Result<tsr_ast::Diagnostic, Error> {
    create_expression_error_ex(view, node, None)
}

// port: tsc/internal/transformers/declarations/diagnostics.go:createClassExpressionError
fn create_class_expression_error(
    view: AstView<'_>,
    node: NodeId,
) -> Result<tsr_ast::Diagnostic, Error> {
    create_expression_error_ex(
        view,
        node,
        Some(d::Inference_from_class_expressions_is_not_supported_with_isolatedDeclarations),
    )
}

// port: tsc/internal/transformers/declarations/diagnostics.go:isParentForIDDIagnostic
fn is_parent_for_idd_diagnostic(
    view: AstView<'_>,
    node: NodeId,
) -> Result<tsr_ast::utilities::FindAncestorResult, Error> {
    use tsr_ast::utilities::FindAncestorResult as F;
    let read = view.node(node)?;
    if read.kind() == K::ExportAssignment {
        return Ok(F::TRUE);
    }
    if tsr_ast::utilities::is_statement(view, node)? {
        return Ok(F::QUIT);
    }
    Ok(tsr_ast::utilities::to_find_ancestor_result(
        read.kind() != K::ParenthesizedExpression
            && !tsr_ast::utilities::is_assertion_expression(&read),
    ))
}

// port: tsc/internal/transformers/declarations/diagnostics.go:createExpressionErrorEx
fn create_expression_error_ex(
    view: AstView<'_>,
    node: NodeId,
    mut diagnostic_message: Option<&'static Message>,
) -> Result<tsr_ast::Diagnostic, Error> {
    use tsr_ast::utilities::FindAncestorResult as F;
    let Some(parent_declaration) = find_nearest_declaration(view, node)? else {
        let message = diagnostic_message
            .unwrap_or(d::Expression_type_can_t_be_inferred_with_isolatedDeclarations);
        return create_diagnostic_for_node(view, node, message, vec![]);
    };
    let target = declaration_target_text(view, parent_declaration)?;
    // ast.FindAncestorOrQuit(node.Parent, isParentForIDDIagnostic)
    let mut current = view.node(node)?.parent();
    let mut parent = None;
    while let Some(candidate) = current {
        match is_parent_for_idd_diagnostic(view, candidate)? {
            F::QUIT => break,
            F::TRUE => {
                parent = Some(candidate);
                break;
            }
            _ => {}
        }
        current = view.node(candidate)?.parent();
    }
    if parent == Some(parent_declaration) {
        let message = diagnostic_message.unwrap_or_else(|| {
            required_message(isolated_error_message(
                kind(view, parent_declaration).expect("parent declaration kind"),
            ))
        });
        let mut diagnostic = create_diagnostic_for_node(view, node, message, vec![])?;
        let related = create_diagnostic_for_node(
            view,
            parent_declaration,
            required_message(related_suggestion(kind(view, parent_declaration)?)),
            vec![target],
        )?;
        add_related_info(&mut diagnostic, related);
        return Ok(diagnostic);
    }
    if diagnostic_message.is_none() {
        diagnostic_message = Some(d::Expression_type_can_t_be_inferred_with_isolatedDeclarations);
    }
    let mut diagnostic =
        create_diagnostic_for_node(view, node, required_message(diagnostic_message), vec![])?;
    let related = create_diagnostic_for_node(
        view,
        parent_declaration,
        required_message(related_suggestion(kind(view, parent_declaration)?)),
        vec![target],
    )?;
    add_related_info(&mut diagnostic, related);
    let related = create_diagnostic_for_node(
        view,
        node,
        d::Add_satisfies_and_a_type_assertion_to_this_expression_satisfies_T_as_T_to_make_the_type_explicit,
        vec![],
    )?;
    add_related_info(&mut diagnostic, related);
    Ok(diagnostic)
}

/// `createParameterError`, the closure of `createGetIsolatedDeclarationErrors`.
fn create_parameter_error<R: tsr_printer::emit_resolver::DeclarationEmitResolver>(
    resolver: &mut R,
    node: NodeId,
) -> Result<tsr_ast::Diagnostic, R::Error> {
    let owner = resolver.ast(node)?.node(node)?.parent().expect(NIL);
    if kind(resolver.ast(owner)?, owner)? == K::SetAccessor {
        return create_accessor_type_error(resolver, owner);
    }
    // skip checker lock - node builder will already have one
    let add_undefined = resolver.requires_adding_implicit_undefined_unsafe(node, None, None)?;
    let view = resolver.ast(node)?;
    if !add_undefined {
        if let Some(initializer) = view.node(node)?.initializer() {
            return Ok(create_expression_error(view, initializer)?);
        }
    }
    let node_kind = kind(view, node)?;
    let message = if add_undefined {
        d::Declaration_emit_for_this_parameter_requires_implicitly_adding_undefined_to_its_type_This_is_not_supported_with_isolatedDeclarations
    } else {
        required_message(isolated_error_message(node_kind))
    };
    let mut diagnostic = create_diagnostic_for_node(view, node, message, vec![])?;
    let name = view.node(node)?.name().expect(NIL);
    let target = tsr_scanner::get_text_of_node(view, name)?;
    let related = create_diagnostic_for_node(
        view,
        node,
        required_message(related_suggestion(node_kind)),
        vec![target],
    )?;
    add_related_info(&mut diagnostic, related);
    Ok(diagnostic)
}

// port: tsc/internal/transformers/declarations/diagnostics.go:createGetIsolatedDeclarationErrors
pub(super) fn create_get_isolated_declaration_errors<
    R: tsr_printer::emit_resolver::DeclarationEmitResolver,
>(
    resolver: &mut R,
    node: NodeId,
) -> Result<tsr_ast::Diagnostic, R::Error> {
    let view = resolver.ast(node)?;
    let heritage_clause =
        tsr_ast::utilities::find_ancestor_kind(view, Some(node), K::HeritageClause.into())?;
    if heritage_clause.is_some() {
        return Ok(create_diagnostic_for_node(
            view,
            node,
            d::Extends_clause_can_t_contain_an_expression_with_isolatedDeclarations,
            vec![],
        )?);
    }
    let node_kind = kind(view, node)?;
    if tsr_ast::utilities_positions::is_part_of_type_node(view, node)? || node_kind == K::TypeQuery
    {
        return Ok(create_entity_in_type_node_error(view, node)?);
    }
    if tsr_ast::utilities::is_entity_name(&view.node(node)?)
        || tsr_ast::is_entity_name_expression(view, node)?
    {
        return Ok(create_entity_in_type_node_error(view, node)?);
    }
    Ok(match node_kind {
        K::GetAccessor | K::SetAccessor => return create_accessor_type_error(resolver, node),
        K::ComputedPropertyName | K::ShorthandPropertyAssignment | K::SpreadAssignment => {
            create_object_literal_error(view, node)?
        }
        K::ArrayLiteralExpression | K::SpreadElement => create_array_literal_error(view, node)?,
        K::MethodDeclaration
        | K::ConstructSignature
        | K::FunctionExpression
        | K::ArrowFunction
        | K::FunctionDeclaration => create_return_type_error(view, node)?,
        K::BindingElement => create_binding_element_error(view, node)?,
        K::PropertyDeclaration | K::VariableDeclaration => {
            create_variable_or_property_error(view, node)?
        }
        K::Parameter => return create_parameter_error(resolver, node),
        K::PropertyAssignment => {
            create_expression_error(view, view.node(node)?.initializer().expect(NIL))?
        }
        K::ClassExpression => create_class_expression_error(view, node)?,
        _ => create_expression_error(view, node)?,
    })
}

#[cfg(test)]
#[path = "diagnostics_tests.rs"]
mod tests;
