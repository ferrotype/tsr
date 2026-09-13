//! Checker projections consumed by declaration transformation. The transformer
//! owns output syntax; the resolver never returns unretained checker syntax.
use crate::EmitContext;
use ts_ast::{AstBuilder, AstView, JsString, NodeId, SymbolFlags, SymbolId};

// Source: tsc/internal/printer/emitresolver.go:SymbolAccessibility
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(i32)]
pub enum SymbolAccessibility {
    Accessible,
    NotAccessible,
    CannotBeNamed,
    NotResolved,
}

// Source: tsc/internal/printer/emitresolver.go:SymbolAccessibilityResult
#[derive(Clone, Debug)]
pub struct SymbolAccessibilityResult {
    pub accessibility: SymbolAccessibility,
    pub aliases_to_make_visible: Vec<NodeId>,
    pub error_symbol_name: JsString,
    pub error_node: Option<NodeId>,
    pub error_module_name: JsString,
}

impl SymbolAccessibilityResult {
    pub fn accessible() -> Self {
        Self {
            accessibility: SymbolAccessibility::Accessible,
            aliases_to_make_visible: Vec::new(),
            error_symbol_name: JsString::default(),
            error_node: None,
            error_module_name: JsString::default(),
        }
    }
}

#[derive(Clone, Debug)]
pub enum ConstantValue {
    Number(ts_jsnum::Number),
    String(JsString),
}

// Source: tsc/internal/evaluator/evaluator.go:Result
#[derive(Clone, Debug, Default)]
pub struct EnumMemberValue {
    pub value: Option<ConstantValue>,
    pub is_syntactically_string: bool,
    pub resolved_other_files: bool,
    pub has_external_references: bool,
}

/// Reports emitted during serialization. The transformer retains their order
/// and controls diagnostic context and fallback-node stacks.
#[derive(Clone, Debug)]
pub enum DeclarationTrackerEvent {
    InaccessibleThis,
    PrivateInBaseOfClassExpression(JsString),
    InaccessibleUniqueSymbol,
    CyclicStructure,
    LikelyUnsafeImportRequired {
        specifier: JsString,
        symbol_name: JsString,
    },
    Truncation,
    NonlocalAugmentation {
        containing_file: NodeId,
        parent_symbol: SymbolId,
        augmenting_symbol: SymbolId,
    },
    NonSerializableProperty(JsString),
    InferenceFallback(NodeId),
    PushErrorFallbackNode(Option<NodeId>),
    PopErrorFallbackNode,
}

/// The resolver computes visibility before the callback, avoiding a second
/// mutable borrow of the checker from inside node serialization. Its boolean
/// is consumed immediately, as in the pinned SymbolTracker.TrackSymbol.
pub trait DeclarationSymbolTracker {
    /// Native watched-symbol path handles the identity before accessibility is queried.
    fn track_symbol_without_accessibility(&mut self, _symbol: SymbolId) -> bool {
        false
    }
    fn track_symbol(
        &mut self,
        symbol: SymbolId,
        enclosing_declaration: Option<NodeId>,
        meaning: SymbolFlags,
        accessibility: SymbolAccessibilityResult,
    ) -> bool;
    fn report(&mut self, event: DeclarationTrackerEvent);
}

/// Declaration-only projection of `printer.EmitResolver`. Unavailable
/// projections return a named error; they never stand in for a successful phase.
pub trait DeclarationEmitResolver {
    type Error: From<ts_arena::Error>;

    fn unsupported(operation: &'static str) -> Self::Error;
    fn ast(&self, node: NodeId) -> Result<AstView<'_>, Self::Error>;
    fn retain_source(&self, source: NodeId, output: &mut AstBuilder) -> Result<(), Self::Error>;
    fn bound_symbol_of_declaration(&self, node: NodeId) -> Result<Option<SymbolId>, Self::Error>;
    fn symbol_of_declaration(&mut self, node: NodeId) -> Result<Option<SymbolId>, Self::Error>;
    fn symbol_value_declaration(&self, symbol: SymbolId) -> Result<Option<NodeId>, Self::Error>;
    fn symbol_declarations(&self, symbol: SymbolId) -> Result<Vec<NodeId>, Self::Error>;

    fn precalculate_declaration_emit_visibility(
        &mut self,
        _source: NodeId,
    ) -> Result<(), Self::Error> {
        Err(Self::unsupported("PrecalculateDeclarationEmitVisibility"))
    }
    fn is_declaration_visible(&mut self, _node: NodeId) -> Result<bool, Self::Error> {
        Err(Self::unsupported("IsDeclarationVisible"))
    }
    fn effective_declaration_flags(
        &mut self,
        _node: NodeId,
        _flags: u32,
    ) -> Result<u32, Self::Error> {
        Err(Self::unsupported("GetEffectiveDeclarationFlags"))
    }
    fn implementation_of_overload(&mut self, _node: NodeId) -> Result<bool, Self::Error> {
        Err(Self::unsupported("IsImplementationOfOverload"))
    }
    fn optional_parameter(&mut self, _node: NodeId) -> Result<bool, Self::Error> {
        Err(Self::unsupported("IsOptionalParameter"))
    }
    fn literal_const_declaration(&mut self, _node: NodeId) -> Result<bool, Self::Error> {
        Err(Self::unsupported("IsLiteralConstDeclaration"))
    }
    fn late_bound(&mut self, _node: NodeId) -> Result<bool, Self::Error> {
        Err(Self::unsupported("IsLateBound"))
    }
    fn symbol_accessible(
        &mut self,
        _symbol: Option<SymbolId>,
        _enclosing: Option<NodeId>,
        _meaning: SymbolFlags,
        _compute_aliases: bool,
    ) -> Result<SymbolAccessibilityResult, Self::Error> {
        Err(Self::unsupported("IsSymbolAccessible"))
    }
    fn entity_name_visible(
        &mut self,
        _node: NodeId,
        _enclosing: NodeId,
    ) -> Result<SymbolAccessibilityResult, Self::Error> {
        Err(Self::unsupported("IsEntityNameVisible"))
    }
    fn expando_function_declaration(&mut self, _node: NodeId) -> Result<bool, Self::Error> {
        Err(Self::unsupported("IsExpandoFunctionDeclaration"))
    }
    fn expando_function_declaration_unsafe(&mut self, _node: NodeId) -> Result<bool, Self::Error> {
        Err(Self::unsupported("IsExpandoFunctionDeclarationUnsafe"))
    }
    fn requires_adding_implicit_undefined(
        &mut self,
        _node: NodeId,
        _symbol: Option<SymbolId>,
        _enclosing: Option<NodeId>,
    ) -> Result<bool, Self::Error> {
        Err(Self::unsupported("RequiresAddingImplicitUndefined"))
    }
    fn requires_adding_implicit_undefined_unsafe(
        &mut self,
        _node: NodeId,
        _symbol: Option<SymbolId>,
        _enclosing: Option<NodeId>,
    ) -> Result<bool, Self::Error> {
        Err(Self::unsupported("RequiresAddingImplicitUndefinedUnsafe"))
    }
    fn name_resolvable(&mut self, _node: NodeId, _name: &[u8]) -> Result<bool, Self::Error> {
        Err(Self::unsupported("IsNameResolvable"))
    }
    fn import_required_by_augmentation(&mut self, _node: NodeId) -> Result<bool, Self::Error> {
        Err(Self::unsupported("IsImportRequiredByAugmentation"))
    }
    fn definitely_reference_to_global_symbol_object(
        &mut self,
        _node: NodeId,
    ) -> Result<bool, Self::Error> {
        Err(Self::unsupported(
            "IsDefinitelyReferenceToGlobalSymbolObject",
        ))
    }
    fn enum_member_value(&mut self, _node: NodeId) -> Result<EnumMemberValue, Self::Error> {
        Err(Self::unsupported("GetEnumMemberValue"))
    }
    fn redundant_this_property_assignment(&mut self, _node: NodeId) -> Result<bool, Self::Error> {
        Err(Self::unsupported(
            "IsThisPropertyAssignmentDeclarationRedundant",
        ))
    }
    fn properties_of_container_function(
        &mut self,
        _node: NodeId,
    ) -> Result<Vec<SymbolId>, Self::Error> {
        Err(Self::unsupported("GetPropertiesOfContainerFunction"))
    }
    fn symbol_flags(&mut self, _symbol: SymbolId) -> Result<SymbolFlags, Self::Error> {
        Err(Self::unsupported("declaration emit: symbol flags"))
    }
    fn symbol_export(
        &mut self,
        _symbol: SymbolId,
        _name: &[u8],
    ) -> Result<Option<SymbolId>, Self::Error> {
        Err(Self::unsupported("declaration emit: symbol export"))
    }
    fn referenced_value_declaration(
        &mut self,
        _node: NodeId,
    ) -> Result<Option<NodeId>, Self::Error> {
        Err(Self::unsupported("GetReferencedValueDeclaration"))
    }
    fn element_access_expression_name(&mut self, _node: NodeId) -> Result<JsString, Self::Error> {
        Err(Self::unsupported("GetElementAccessExpressionName"))
    }
    fn referenced_name_declaration(
        &mut self,
        _name: JsString,
        _parent: NodeId,
    ) -> Result<Option<NodeId>, Self::Error> {
        Err(Self::unsupported(
            "GetReferencedValueDeclaration: synthesized name",
        ))
    }
    fn referenced_member_value_declaration(
        &mut self,
        _node: NodeId,
    ) -> Result<Option<NodeId>, Self::Error> {
        Err(Self::unsupported("GetReferencedMemberValueDeclaration"))
    }
    fn create_expando_namespace_scope(
        &mut self,
        _parent: NodeId,
        _name: JsString,
        _host: SymbolId,
        _local_name: JsString,
        _symbol: SymbolId,
    ) -> Result<NodeId, Self::Error> {
        Err(Self::unsupported(
            "declaration emit: synthetic namespace scope",
        ))
    }
    fn referenced_value_declaration_unsafe(
        &mut self,
        _node: NodeId,
    ) -> Result<Option<NodeId>, Self::Error> {
        Err(Self::unsupported("GetReferencedValueDeclarationUnsafe"))
    }
    fn external_module_file_from_declaration(
        &mut self,
        _node: NodeId,
    ) -> Result<Option<NodeId>, Self::Error> {
        Err(Self::unsupported("GetExternalModuleFileFromDeclaration"))
    }

    #[allow(
        clippy::too_many_arguments,
        reason = "The native serialization request carries distinct context, declaration, flags and tracker inputs"
    )]
    fn create_type_of_declaration(
        &mut self,
        _output: &mut AstBuilder,
        _emit: &mut EmitContext,
        _node: NodeId,
        _enclosing: NodeId,
        _flags: ts_nodebuilder::Flags,
        _internal_flags: ts_nodebuilder::InternalFlags,
        _tracker: &mut dyn DeclarationSymbolTracker,
    ) -> Result<Option<NodeId>, Self::Error> {
        Err(Self::unsupported("CreateTypeOfDeclaration"))
    }
    #[allow(
        clippy::too_many_arguments,
        reason = "The native serialization request carries distinct context, declaration, flags and tracker inputs"
    )]
    fn create_return_type_of_signature(
        &mut self,
        _output: &mut AstBuilder,
        _emit: &mut EmitContext,
        _node: NodeId,
        _enclosing: NodeId,
        _flags: ts_nodebuilder::Flags,
        _internal_flags: ts_nodebuilder::InternalFlags,
        _tracker: &mut dyn DeclarationSymbolTracker,
    ) -> Result<Option<NodeId>, Self::Error> {
        Err(Self::unsupported("CreateReturnTypeOfSignatureDeclaration"))
    }
    #[allow(
        clippy::too_many_arguments,
        reason = "The native serialization request carries distinct context, declaration, flags and tracker inputs"
    )]
    fn create_type_parameters_of_signature(
        &mut self,
        _output: &mut AstBuilder,
        _emit: &mut EmitContext,
        _node: NodeId,
        _enclosing: NodeId,
        _flags: ts_nodebuilder::Flags,
        _internal_flags: ts_nodebuilder::InternalFlags,
        _tracker: &mut dyn DeclarationSymbolTracker,
    ) -> Result<Vec<NodeId>, Self::Error> {
        Err(Self::unsupported(
            "CreateTypeParametersOfSignatureDeclaration",
        ))
    }
    fn create_literal_const_value(
        &mut self,
        _output: &mut AstBuilder,
        _emit: &mut EmitContext,
        _node: NodeId,
        _tracker: &mut dyn DeclarationSymbolTracker,
    ) -> Result<Option<NodeId>, Self::Error> {
        Err(Self::unsupported("CreateLiteralConstValue"))
    }
    #[allow(
        clippy::too_many_arguments,
        reason = "The native serialization request carries distinct context, declaration, flags and tracker inputs"
    )]
    fn create_type_of_expression(
        &mut self,
        _output: &mut AstBuilder,
        _emit: &mut EmitContext,
        _node: NodeId,
        _enclosing: NodeId,
        _flags: ts_nodebuilder::Flags,
        _internal_flags: ts_nodebuilder::InternalFlags,
        _tracker: &mut dyn DeclarationSymbolTracker,
    ) -> Result<Option<NodeId>, Self::Error> {
        Err(Self::unsupported("CreateTypeOfExpression"))
    }
    #[allow(
        clippy::too_many_arguments,
        reason = "The native serialization request carries distinct context, declaration, flags and tracker inputs"
    )]
    fn create_late_bound_index_signatures(
        &mut self,
        _output: &mut AstBuilder,
        _emit: &mut EmitContext,
        _node: NodeId,
        _enclosing: NodeId,
        _flags: ts_nodebuilder::Flags,
        _internal_flags: ts_nodebuilder::InternalFlags,
        _tracker: &mut dyn DeclarationSymbolTracker,
    ) -> Result<Vec<NodeId>, Self::Error> {
        Err(Self::unsupported("CreateLateBoundIndexSignatures"))
    }
    #[allow(
        clippy::too_many_arguments,
        reason = "The native serialization request carries distinct context, declaration, flags and tracker inputs"
    )]
    fn try_js_type_node_to_type_node(
        &mut self,
        _output: &mut AstBuilder,
        _emit: &mut EmitContext,
        _node: NodeId,
        _enclosing: NodeId,
        _flags: ts_nodebuilder::Flags,
        _internal_flags: ts_nodebuilder::InternalFlags,
        _tracker: &mut dyn DeclarationSymbolTracker,
    ) -> Result<Option<NodeId>, Self::Error> {
        Err(Self::unsupported("TryJSTypeNodeToTypeNode"))
    }
}
