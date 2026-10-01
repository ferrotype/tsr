//! The script transforms' projection of `printer.EmitResolver` and of
//! `binder.ReferenceResolver`, which it embeds. Declaration emit uses
//! [`crate::emit_resolver::DeclarationEmitResolver`]; these traits are
//! object-safe so a transformer holds a resolver without naming the checker.
use crate::emit_resolver::{ConstantValue, EnumMemberValue};
use tsr_ast::{JsString, NodeId};

/// A resolver failure, opaque to the transforms: they record it and stop.
/// The emitter that supplied the resolver recovers its own error type.
#[derive(Debug)]
pub struct EmitResolverError(Box<dyn std::error::Error + Send + Sync + 'static>);

impl EmitResolverError {
    pub fn new(error: impl std::error::Error + Send + Sync + 'static) -> Self {
        Self(Box::new(error))
    }

    pub fn downcast_ref<T: std::error::Error + 'static>(&self) -> Option<&T> {
        self.0.downcast_ref()
    }

    pub fn into_inner(self) -> Box<dyn std::error::Error + Send + Sync + 'static> {
        self.0
    }
}

impl std::fmt::Display for EmitResolverError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl std::error::Error for EmitResolverError {}

impl From<tsr_arena::Error> for EmitResolverError {
    fn from(error: tsr_arena::Error) -> Self {
        Self::new(error)
    }
}

pub type ResolverResult<T> = Result<T, EmitResolverError>;

/// The identifier texts of an entity name, left to right; never empty.
pub type EntityName = Vec<JsString>;

/// Indicates how to serialize the name for a TypeReferenceNode when emitting
/// decorator metadata.
// Source: tsc/internal/printer/emitresolver.go:TypeReferenceSerializationKind
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(i32)]
pub enum TypeReferenceSerializationKind {
    /// The TypeReferenceNode could not be resolved.
    /// The type name should be emitted using a safe fallback.
    Unknown,
    /// The TypeReferenceNode resolves to a type with a constructor
    /// function that can be reached at runtime (e.g. a `class`
    /// declaration or a `var` declaration for the static side
    /// of a type, such as the global `Promise` type in lib.d.ts).
    TypeWithConstructSignatureAndValue,
    /// The TypeReferenceNode resolves to a Void-like, Nullable, or Never type.
    VoidNullableOrNeverType,
    /// The TypeReferenceNode resolves to a Number-like type.
    NumberLikeType,
    /// The TypeReferenceNode resolves to a BigInt-like type.
    BigIntLikeType,
    /// The TypeReferenceNode resolves to a String-like type.
    StringLikeType,
    /// The TypeReferenceNode resolves to a Boolean-like type.
    BooleanType,
    /// The TypeReferenceNode resolves to an Array-like type.
    ArrayLikeType,
    /// The TypeReferenceNode resolves to the ESSymbol type.
    EsSymbolType,
    /// The TypeReferenceNode resolved to the global Promise constructor symbol.
    Promise,
    /// The TypeReferenceNode resolves to a Function type or a type with call signatures.
    TypeWithCallSignature,
    /// The TypeReferenceNode resolves to any other type.
    ObjectType,
}

/// `binder.ReferenceResolver`. A nil result is `None`.
///
/// A node of the transform's own factory is never a parse-tree node, and the
/// checker behind a resolver cannot read it. An implementation answers such a
/// node as upstream's `!ast.IsParseTreeNode(node)` guard does, without reading
/// it. The one transform that makes a factory node look like a parse-tree node
/// to the resolver has its own entry
/// (`EmitResolver::get_referenced_export_container_of_name`).
pub trait ReferenceResolver {
    /// The SourceFile, ModuleDeclaration or EnumDeclaration that contains the
    /// export an identifier refers to.
    fn get_referenced_export_container(
        &mut self,
        node: NodeId,
        prefix_locals: bool,
    ) -> ResolverResult<Option<NodeId>>;
    fn get_referenced_import_declaration(&mut self, node: NodeId)
        -> ResolverResult<Option<NodeId>>;
    fn get_referenced_value_declaration(&mut self, node: NodeId) -> ResolverResult<Option<NodeId>>;
    /// Upstream's nil slice is `None`.
    fn get_referenced_value_declarations(
        &mut self,
        node: NodeId,
    ) -> ResolverResult<Option<Vec<NodeId>>>;
    fn get_element_access_expression_name(
        &mut self,
        expression: NodeId,
    ) -> ResolverResult<JsString>;
    fn get_referenced_member_value_declaration(
        &mut self,
        node: NodeId,
    ) -> ResolverResult<Option<NodeId>>;
}

/// `printer.EmitResolver`, without its declaration-emit projections.
pub trait EmitResolver: ReferenceResolver {
    fn is_referenced_alias_declaration(&mut self, node: NodeId) -> ResolverResult<bool>;
    fn is_value_alias_declaration(&mut self, node: NodeId) -> ResolverResult<bool>;
    fn is_top_level_value_import_equals_with_entity_name(
        &mut self,
        node: NodeId,
    ) -> ResolverResult<bool>;
    fn mark_linked_references_recursively(&mut self, file: NodeId) -> ResolverResult<()>;
    /// The source file a declaration's module specifier resolves to.
    fn get_external_module_file_from_declaration(
        &mut self,
        node: NodeId,
    ) -> ResolverResult<Option<NodeId>>;
    /// `flags` and the result are `ast.ModifierFlags`.
    fn get_effective_declaration_flags(&mut self, node: NodeId, flags: u32) -> ResolverResult<u32>;

    // decorator metadata
    fn get_type_reference_serialization_kind(
        &mut self,
        name: Option<NodeId>,
        serial_scope: Option<NodeId>,
    ) -> ResolverResult<TypeReferenceSerializationKind>;

    // const enum inlining
    /// Upstream returns `any`: nil, a string or a number.
    fn get_constant_value(&mut self, node: NodeId) -> ResolverResult<Option<ConstantValue>>;
    fn get_enum_member_value(&mut self, node: NodeId) -> ResolverResult<EnumMemberValue>;

    // JSX Emit
    /// Upstream returns the entity name node. The checker parses it from an
    /// option or a pragma into its own storage, which no output factory
    /// retains, and the transform reads only its identifiers' texts: the
    /// resolver hands over those, left to right (`A.B.C` is `[A, B, C]`).
    fn get_jsx_factory_entity(&mut self, location: NodeId) -> ResolverResult<Option<EntityName>>;
    fn get_jsx_fragment_factory_entity(
        &mut self,
        location: NodeId,
    ) -> ResolverResult<Option<EntityName>>;
    /// `GetReferencedExportContainer` for an identifier named `name` that the
    /// transform created, cleared `Synthesized` on and parented to the
    /// parse-tree node `parent` (`createReactNamespace`). The resolver cannot
    /// read the transform's node, so it resolves an identifier of its own with
    /// that name and parent.
    fn get_referenced_export_container_of_name(
        &mut self,
        name: &[u8],
        parent: Option<NodeId>,
        prefix_locals: bool,
    ) -> ResolverResult<Option<NodeId>>;
    /// Overrides the reference resolver's answer for a generated identifier.
    fn set_referenced_import_declaration(
        &mut self,
        node: NodeId,
        reference: NodeId,
    ) -> ResolverResult<()>;
}

/// A borrowed resolver is a resolver, so a transformation can share one it
/// does not own.
impl<T: ReferenceResolver + ?Sized> ReferenceResolver for &mut T {
    fn get_referenced_export_container(
        &mut self,
        node: NodeId,
        prefix_locals: bool,
    ) -> ResolverResult<Option<NodeId>> {
        (**self).get_referenced_export_container(node, prefix_locals)
    }
    fn get_referenced_import_declaration(
        &mut self,
        node: NodeId,
    ) -> ResolverResult<Option<NodeId>> {
        (**self).get_referenced_import_declaration(node)
    }
    fn get_referenced_value_declaration(&mut self, node: NodeId) -> ResolverResult<Option<NodeId>> {
        (**self).get_referenced_value_declaration(node)
    }
    fn get_referenced_value_declarations(
        &mut self,
        node: NodeId,
    ) -> ResolverResult<Option<Vec<NodeId>>> {
        (**self).get_referenced_value_declarations(node)
    }
    fn get_element_access_expression_name(
        &mut self,
        expression: NodeId,
    ) -> ResolverResult<JsString> {
        (**self).get_element_access_expression_name(expression)
    }
    fn get_referenced_member_value_declaration(
        &mut self,
        node: NodeId,
    ) -> ResolverResult<Option<NodeId>> {
        (**self).get_referenced_member_value_declaration(node)
    }
}

impl<T: EmitResolver + ?Sized> EmitResolver for &mut T {
    fn is_referenced_alias_declaration(&mut self, node: NodeId) -> ResolverResult<bool> {
        (**self).is_referenced_alias_declaration(node)
    }
    fn is_value_alias_declaration(&mut self, node: NodeId) -> ResolverResult<bool> {
        (**self).is_value_alias_declaration(node)
    }
    fn is_top_level_value_import_equals_with_entity_name(
        &mut self,
        node: NodeId,
    ) -> ResolverResult<bool> {
        (**self).is_top_level_value_import_equals_with_entity_name(node)
    }
    fn mark_linked_references_recursively(&mut self, file: NodeId) -> ResolverResult<()> {
        (**self).mark_linked_references_recursively(file)
    }
    fn get_external_module_file_from_declaration(
        &mut self,
        node: NodeId,
    ) -> ResolverResult<Option<NodeId>> {
        (**self).get_external_module_file_from_declaration(node)
    }
    fn get_effective_declaration_flags(&mut self, node: NodeId, flags: u32) -> ResolverResult<u32> {
        (**self).get_effective_declaration_flags(node, flags)
    }
    fn get_type_reference_serialization_kind(
        &mut self,
        name: Option<NodeId>,
        serial_scope: Option<NodeId>,
    ) -> ResolverResult<TypeReferenceSerializationKind> {
        (**self).get_type_reference_serialization_kind(name, serial_scope)
    }
    fn get_constant_value(&mut self, node: NodeId) -> ResolverResult<Option<ConstantValue>> {
        (**self).get_constant_value(node)
    }
    fn get_enum_member_value(&mut self, node: NodeId) -> ResolverResult<EnumMemberValue> {
        (**self).get_enum_member_value(node)
    }
    fn get_jsx_factory_entity(&mut self, location: NodeId) -> ResolverResult<Option<EntityName>> {
        (**self).get_jsx_factory_entity(location)
    }
    fn get_jsx_fragment_factory_entity(
        &mut self,
        location: NodeId,
    ) -> ResolverResult<Option<EntityName>> {
        (**self).get_jsx_fragment_factory_entity(location)
    }
    fn get_referenced_export_container_of_name(
        &mut self,
        name: &[u8],
        parent: Option<NodeId>,
        prefix_locals: bool,
    ) -> ResolverResult<Option<NodeId>> {
        (**self).get_referenced_export_container_of_name(name, parent, prefix_locals)
    }
    fn set_referenced_import_declaration(
        &mut self,
        node: NodeId,
        reference: NodeId,
    ) -> ResolverResult<()> {
        (**self).set_referenced_import_declaration(node, reference)
    }
}
