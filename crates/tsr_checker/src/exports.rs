//! The public checker query surface (`tsc/internal/checker/exports.go`): each
//! entry point is a method of the checker operation over owner-bound handles,
//! so results are usable only through the operation that produced them
//! (ADR 0008). Index infos and type predicates get handles of their own.

use crate::handles::{SignatureRef, SymbolRef, TypeRef};
use crate::{
    type_flags as tf, CheckerState, Error, IndexInfoId, MemberOverrideStatus, Operation,
    RelationKind, SignatureKind, TypePredicateId, TypePredicateKind, UnionReduction,
};
use std::cmp::Ordering;
use tsr_arena::{ArenaId, NodeId, SymbolId};
use tsr_ast::{modifier_flags as mf, JsString, SymbolFlags, SyntaxKind as K};
use tsr_diagnostics::Message;

/// An index signature of one checker's type.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct IndexInfoRef {
    pub(crate) owner: ArenaId,
    pub(crate) id: IndexInfoId,
}

/// The parts of an index signature, read through its operation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IndexInfoParts {
    pub key_type: TypeRef,
    pub value_type: TypeRef,
    pub is_readonly: bool,
    pub declaration: Option<NodeId>,
}

/// A type predicate of one checker's signature.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TypePredicateRef {
    pub(crate) owner: ArenaId,
    pub(crate) id: TypePredicateId,
}

/// The parts of a type predicate, read through its operation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TypePredicateParts {
    pub kind: TypePredicateKind,
    pub parameter_index: i32,
    pub parameter_name: JsString,
    pub r#type: Option<TypeRef>,
}

impl Operation<'_> {
    fn index_info_ref(&self, id: IndexInfoId) -> IndexInfoRef {
        IndexInfoRef {
            owner: self.checker(),
            id,
        }
    }

    fn check_index_info(&self, info: IndexInfoRef) -> Result<IndexInfoId, Error> {
        self.lease().validate_identity(info.owner)?;
        self.state().signatures.index_info(info.id)?;
        Ok(info.id)
    }

    fn check_type_predicate(&self, predicate: TypePredicateRef) -> Result<TypePredicateId, Error> {
        self.lease().validate_identity(predicate.owner)?;
        self.state().signatures.predicate(predicate.id)?;
        Ok(predicate.id)
    }

    pub(crate) fn symbol_refs(&self, symbols: &[SymbolId]) -> Result<Vec<SymbolRef>, Error> {
        symbols
            .iter()
            .map(|&symbol| self.symbol_ref(symbol))
            .collect()
    }

    pub(crate) fn optional_symbol(
        &self,
        symbol: Option<SymbolId>,
    ) -> Result<Option<SymbolRef>, Error> {
        symbol.map(|symbol| self.symbol_ref(symbol)).transpose()
    }

    pub(crate) fn type_refs(&self, types: &[TypeId]) -> Vec<TypeRef> {
        types.iter().map(|&ty| self.type_ref(ty)).collect()
    }

    /// The parts of an index signature.
    pub fn index_info_parts(&self, info: IndexInfoRef) -> Result<IndexInfoParts, Error> {
        let id = self.check_index_info(info)?;
        let info = self.state().signatures.index_info(id)?;
        Ok(IndexInfoParts {
            key_type: self.type_ref(info.key_type),
            value_type: self.type_ref(info.value_type),
            is_readonly: info.is_readonly,
            declaration: info.declaration,
        })
    }

    /// The parts of a type predicate.
    pub fn type_predicate_parts(
        &self,
        predicate: TypePredicateRef,
    ) -> Result<TypePredicateParts, Error> {
        let id = self.check_type_predicate(predicate)?;
        let predicate = self.state().signatures.predicate(id)?;
        Ok(TypePredicateParts {
            kind: predicate.kind,
            parameter_index: predicate.parameter_index,
            parameter_name: predicate.parameter_name.clone(),
            r#type: predicate.t.map(|ty| self.type_ref(ty)),
        })
    }

    // port: tsc/internal/checker/exports.go:Checker.GetStringType
    pub fn get_string_type(&self) -> TypeRef {
        self.type_ref(self.state().builtins.string_type)
    }

    // port: tsc/internal/checker/exports.go:Checker.GetNumberType
    pub fn get_number_type(&self) -> TypeRef {
        self.type_ref(self.state().builtins.number_type)
    }

    // port: tsc/internal/checker/exports.go:Checker.GetBooleanType
    pub fn get_boolean_type(&self) -> TypeRef {
        self.type_ref(self.state().builtins.boolean_type)
    }

    // port: tsc/internal/checker/exports.go:Checker.GetVoidType
    pub fn get_void_type(&self) -> TypeRef {
        self.type_ref(self.state().builtins.void_type)
    }

    // port: tsc/internal/checker/exports.go:Checker.GetUndefinedType
    pub fn get_undefined_type(&self) -> TypeRef {
        self.type_ref(self.state().builtins.undefined_type)
    }

    // port: tsc/internal/checker/exports.go:Checker.GetNullType
    pub fn get_null_type(&self) -> TypeRef {
        self.type_ref(self.state().builtins.null_type)
    }

    // port: tsc/internal/checker/exports.go:Checker.GetAnyType
    pub fn get_any_type(&self) -> TypeRef {
        self.type_ref(self.state().builtins.any_type)
    }

    // port: tsc/internal/checker/exports.go:Checker.GetErrorType
    pub fn get_error_type(&self) -> TypeRef {
        self.type_ref(self.state().builtins.error_type)
    }

    // port: tsc/internal/checker/exports.go:Checker.GetNeverType
    pub fn get_never_type(&self) -> TypeRef {
        self.type_ref(self.state().builtins.never_type)
    }

    // port: tsc/internal/checker/exports.go:Checker.GetUnknownType
    pub fn get_unknown_type(&self) -> TypeRef {
        self.type_ref(self.state().builtins.unknown_type)
    }

    // port: tsc/internal/checker/exports.go:Checker.GetBigIntType
    pub fn get_big_int_type(&self) -> TypeRef {
        self.type_ref(self.state().builtins.bigint_type)
    }

    // port: tsc/internal/checker/exports.go:Checker.GetESSymbolType
    pub fn get_es_symbol_type(&self) -> TypeRef {
        self.type_ref(self.state().builtins.es_symbol_type)
    }

    // port: tsc/internal/checker/exports.go:Checker.GetNonPrimitiveType
    pub fn get_non_primitive_type(&self) -> TypeRef {
        self.type_ref(self.state().builtins.non_primitive_type)
    }

    // port: tsc/internal/checker/exports.go:Checker.GetBaseTypeOfLiteralType
    pub fn get_base_type_of_literal_type(&mut self, ty: TypeRef) -> Result<TypeRef, Error> {
        let ty = self.check_type(ty)?;
        let base = self.state_mut().base_literal_type(ty)?;
        Ok(self.type_ref(base))
    }

    // port: tsc/internal/checker/exports.go:Checker.GetUnknownSymbol
    pub fn get_unknown_symbol(&self) -> Result<SymbolRef, Error> {
        self.symbol_ref(self.state().builtins.unknown_symbol)
    }

    // port: tsc/internal/checker/exports.go:Checker.GetUndefinedSymbol
    pub fn get_undefined_symbol(&self) -> Result<SymbolRef, Error> {
        self.symbol_ref(self.state().builtins.undefined_symbol)
    }

    // port: tsc/internal/checker/exports.go:Checker.GetArgumentsSymbol
    pub fn get_arguments_symbol(&self) -> Result<SymbolRef, Error> {
        self.symbol_ref(self.state().builtins.arguments_symbol)
    }

    // port: tsc/internal/checker/exports.go:Checker.GetUnknownSignature
    pub fn get_unknown_signature(&self) -> SignatureRef {
        self.signature_ref(self.state().builtins.unknown_signature)
    }

    // port: tsc/internal/checker/exports.go:Checker.GetUnionType
    pub fn get_union_type(&mut self, types: &[TypeRef]) -> Result<TypeRef, Error> {
        let types = self.check_types(types)?;
        let ty = self.state_mut().get_union_type(&types)?;
        Ok(self.type_ref(ty))
    }

    // port: tsc/internal/checker/exports.go:Checker.GetUnionTypeEx
    pub fn get_union_type_ex(
        &mut self,
        types: &[TypeRef],
        reduction: UnionReduction,
    ) -> Result<TypeRef, Error> {
        let types = self.check_types(types)?;
        let ty = self
            .state_mut()
            .get_union_type_ex(&types, reduction, None, None)?;
        Ok(self.type_ref(ty))
    }

    // port: tsc/internal/checker/exports.go:Checker.GetNameTypeOfSymbol
    pub fn get_name_type_of_symbol(&self, symbol: SymbolRef) -> Result<Option<TypeRef>, Error> {
        self.symbol_name_type(symbol)
    }

    // port: tsc/internal/checker/exports.go:IsTypeUsableAsPropertyName
    // port: tsc/internal/checker/utilities.go:isTypeUsableAsPropertyName
    pub fn is_type_usable_as_property_name(&self, ty: TypeRef) -> Result<bool, Error> {
        let ty = self.check_type(ty)?;
        Ok(self.state().types.flags(ty)? & tf::STRING_OR_NUMBER_LITERAL_OR_UNIQUE != 0)
    }

    // port: tsc/internal/checker/exports.go:GetPropertyNameFromType
    /// `None` where the pin panics: the type is not usable as a property name.
    pub fn get_property_name_from_type(&self, ty: TypeRef) -> Result<Option<JsString>, Error> {
        let ty = self.check_type(ty)?;
        self.state().index_property_name(ty)
    }

    // port: tsc/internal/checker/exports.go:Checker.GetGlobalSymbol
    pub fn get_global_symbol(
        &mut self,
        name: &[u8],
        meaning: SymbolFlags,
        diagnostic: Option<&'static Message>,
    ) -> Result<Option<SymbolRef>, Error> {
        let symbol = self
            .state_mut()
            .resolve_name(None, name, meaning, diagnostic, false)?;
        self.optional_symbol(symbol)
    }

    // port: tsc/internal/checker/exports.go:Checker.GetMergedSymbol
    pub fn get_merged_symbol(&self, symbol: SymbolRef) -> Result<SymbolRef, Error> {
        let symbol = self.check_symbol_ref(symbol)?;
        self.symbol_ref(self.state().get_merged_symbol(symbol))
    }

    // port: tsc/internal/checker/exports.go:Checker.TryFindAmbientModule
    pub fn try_find_ambient_module(&mut self, name: &[u8]) -> Result<Option<SymbolRef>, Error> {
        let symbol = self.state_mut().try_find_ambient_module(name)?;
        self.optional_symbol(symbol)
    }

    // port: tsc/internal/checker/exports.go:Checker.GetImmediateAliasedSymbol
    pub fn get_immediate_aliased_symbol(
        &mut self,
        symbol: SymbolRef,
    ) -> Result<Option<SymbolRef>, Error> {
        let symbol = self.check_symbol_ref(symbol)?;
        let target = self.state_mut().immediate_aliased_symbol(symbol)?;
        self.optional_symbol(target)
    }

    // port: tsc/internal/checker/exports.go:Checker.GetTargetSymbol
    pub fn get_target_symbol(&self, symbol: SymbolRef) -> Result<SymbolRef, Error> {
        let symbol = self.check_symbol_ref(symbol)?;
        self.symbol_ref(self.state().target_symbol(symbol)?)
    }

    // port: tsc/internal/checker/exports.go:Checker.GetTypeOnlyAliasDeclaration
    pub fn get_type_only_alias_declaration(
        &mut self,
        symbol: SymbolRef,
    ) -> Result<Option<NodeId>, Error> {
        let symbol = self.check_symbol_ref(symbol)?;
        self.state_mut().direct_type_only_alias_declaration(symbol)
    }

    // port: tsc/internal/checker/exports.go:Checker.ResolveExternalModuleName
    pub fn resolve_external_module_name(
        &mut self,
        specifier: NodeId,
        import_attributes: Option<TypeRef>,
    ) -> Result<Option<SymbolRef>, Error> {
        let attributes = import_attributes
            .map(|ty| self.check_type(ty))
            .transpose()?;
        let symbol = self
            .state_mut()
            .resolve_external_module_name_with_attributes(specifier, specifier, true, attributes)?;
        self.optional_symbol(symbol)
    }

    // port: tsc/internal/checker/exports.go:Checker.ResolveExternalModuleSymbol
    pub fn resolve_external_module_symbol(
        &mut self,
        module: SymbolRef,
    ) -> Result<Option<SymbolRef>, Error> {
        let module = self.check_symbol_ref(module)?;
        let symbol = self
            .state_mut()
            .resolve_external_module_symbol(Some(module), false)?;
        self.optional_symbol(symbol)
    }

    // port: tsc/internal/checker/exports.go:Checker.GetTypeFromTypeNode
    pub fn get_type_from_type_node(&mut self, node: NodeId) -> Result<TypeRef, Error> {
        let ty = self.state_mut().get_type_from_type_node(node)?;
        Ok(self.type_ref(ty))
    }

    // port: tsc/internal/checker/exports.go:Checker.IsArrayLikeType
    pub fn is_array_like_type(&mut self, ty: TypeRef) -> Result<bool, Error> {
        let ty = self.check_type(ty)?;
        self.state_mut().is_array_like_type(ty)
    }

    // port: tsc/internal/checker/exports.go:Checker.GetPropertiesOfType
    pub fn get_properties_of_type(&mut self, ty: TypeRef) -> Result<Vec<SymbolRef>, Error> {
        self.properties_of_type(ty)
    }

    // port: tsc/internal/checker/exports.go:Checker.GetPropertyOfType
    pub fn get_property_of_type(
        &mut self,
        ty: TypeRef,
        name: &[u8],
    ) -> Result<Option<SymbolRef>, Error> {
        let ty = self.check_type(ty)?;
        let symbol = self.state_mut().constituent_property(ty, name, false)?;
        self.optional_symbol(symbol)
    }

    // port: tsc/internal/checker/exports.go:Checker.TypeHasCallOrConstructSignatures
    // port: tsc/internal/checker/checker.go:Checker.typeHasCallOrConstructSignatures
    pub fn type_has_call_or_construct_signatures(&mut self, ty: TypeRef) -> Result<bool, Error> {
        let ty = self.check_type(ty)?;
        let state = self.state_mut();
        Ok(!state.signatures_of_type(ty, false)?.is_empty()
            || !state.signatures_of_type(ty, true)?.is_empty())
    }

    // port: tsc/internal/checker/exports.go:Checker.IsPropertyAccessible
    /// Whether `property` of `containing` can be accessed at `node`, which
    /// need not be a property access.
    pub fn is_property_accessible(
        &mut self,
        node: NodeId,
        is_super: bool,
        is_write: bool,
        containing: TypeRef,
        property: SymbolRef,
    ) -> Result<bool, Error> {
        let containing = self.check_type(containing)?;
        let property = self.check_symbol_ref(property)?;
        self.state_mut()
            .is_access_property_accessible(node, is_super, is_write, containing, property)
    }

    // port: tsc/internal/checker/exports.go:Checker.GetTypeOfPropertyOfContextualType
    pub fn get_type_of_property_of_contextual_type(
        &mut self,
        ty: TypeRef,
        name: &[u8],
    ) -> Result<Option<TypeRef>, Error> {
        let ty = self.check_type(ty)?;
        let result = self
            .state_mut()
            .type_of_property_of_contextual_type(ty, name)?;
        Ok(result.map(|ty| self.type_ref(ty)))
    }

    // port: tsc/internal/checker/exports.go:GetDeclarationModifierFlagsFromSymbol
    pub fn get_declaration_modifier_flags_from_symbol(
        &self,
        symbol: SymbolRef,
    ) -> Result<tsr_ast::modifier_flags::ModifierFlags, Error> {
        let symbol = self.check_symbol_ref(symbol)?;
        self.state().property_modifiers(symbol)
    }

    // port: tsc/internal/checker/exports.go:Checker.WasCanceled
    /// Whether a check of this checker was canceled; it never clears.
    pub fn was_canceled(&self) -> bool {
        self.state().was_canceled
    }

    // port: tsc/internal/checker/exports.go:Checker.GetSignaturesOfType
    pub fn get_signatures_of_type(
        &mut self,
        ty: TypeRef,
        kind: SignatureKind,
    ) -> Result<Vec<SignatureRef>, Error> {
        let ty = self.check_type(ty)?;
        let signatures = self
            .state_mut()
            .signatures_of_type(ty, kind == SignatureKind::Construct)?;
        Ok(signatures
            .into_iter()
            .map(|signature| self.signature_ref(signature))
            .collect())
    }

    // port: tsc/internal/checker/exports.go:Checker.GetNonMissingTypeOfSymbol
    pub fn get_non_missing_type_of_symbol(&mut self, symbol: SymbolRef) -> Result<TypeRef, Error> {
        let symbol = self.check_symbol_ref(symbol)?;
        let ty = self.state_mut().non_missing_symbol_type(symbol)?;
        Ok(self.type_ref(ty))
    }

    // port: tsc/internal/checker/exports.go:Checker.GetConstraintOfTypeParameter
    pub fn get_constraint_of_type_parameter(
        &mut self,
        parameter: TypeRef,
    ) -> Result<Option<TypeRef>, Error> {
        let parameter = self.check_type(parameter)?;
        let constraint = self.state_mut().constraint_of_type_parameter(parameter)?;
        Ok(constraint.map(|ty| self.type_ref(ty)))
    }

    // port: tsc/internal/checker/exports.go:Checker.GetTrueTypeOfConditionalType
    pub fn get_true_type_of_conditional_type(&mut self, ty: TypeRef) -> Result<TypeRef, Error> {
        let ty = self.check_type(ty)?;
        let result = self.state_mut().conditional_true_type(ty, false)?;
        Ok(self.type_ref(result))
    }

    // port: tsc/internal/checker/exports.go:Checker.GetFalseTypeOfConditionalType
    pub fn get_false_type_of_conditional_type(&mut self, ty: TypeRef) -> Result<TypeRef, Error> {
        let ty = self.check_type(ty)?;
        let result = self.state_mut().conditional_false_type(ty)?;
        Ok(self.type_ref(result))
    }

    // port: tsc/internal/checker/exports.go:Checker.GetDefaultFromTypeParameter
    pub fn get_default_from_type_parameter(
        &mut self,
        parameter: TypeRef,
    ) -> Result<Option<TypeRef>, Error> {
        let parameter = self.check_type(parameter)?;
        let default = self.state_mut().default_from_type_parameter(parameter)?;
        Ok(default.map(|ty| self.type_ref(ty)))
    }

    // port: tsc/internal/checker/exports.go:Checker.GetEffectiveDeclarationFlags
    pub fn get_effective_declaration_flags(
        &self,
        node: NodeId,
        flags: tsr_ast::modifier_flags::ModifierFlags,
    ) -> Result<tsr_ast::modifier_flags::ModifierFlags, Error> {
        self.state().effective_declaration_flags(node, flags)
    }

    // port: tsc/internal/checker/exports.go:Checker.GetBaseConstraintOfType
    pub fn get_base_constraint_of_type(&mut self, ty: TypeRef) -> Result<Option<TypeRef>, Error> {
        let ty = self.check_type(ty)?;
        let constraint = self.state_mut().base_constraint_of_type(ty)?;
        Ok(constraint.map(|ty| self.type_ref(ty)))
    }

    // port: tsc/internal/checker/exports.go:Checker.GetTypePredicateOfSignature
    pub fn get_type_predicate_of_signature(
        &mut self,
        signature: SignatureRef,
    ) -> Result<Option<TypePredicateRef>, Error> {
        let signature = self.check_signature(signature)?;
        let predicate = self.state_mut().type_predicate_of_signature(signature)?;
        Ok(predicate.map(|id| TypePredicateRef {
            owner: self.checker(),
            id,
        }))
    }

    // port: tsc/internal/checker/exports.go:IsTupleType
    pub fn is_tuple_type(&self, ty: TypeRef) -> Result<bool, Error> {
        let ty = self.check_type(ty)?;
        self.state().is_tuple_type(ty)
    }

    // port: tsc/internal/checker/exports.go:IsTupleTypeTarget
    pub fn is_tuple_type_target(&self, ty: TypeRef) -> Result<bool, Error> {
        let ty = self.check_type(ty)?;
        let state = self.state();
        Ok(state.is_tuple_type(ty)? && state.types.target(ty)? == ty)
    }

    // port: tsc/internal/checker/exports.go:Checker.IsArrayType
    pub fn is_array_type(&self, ty: TypeRef) -> Result<bool, Error> {
        let ty = self.check_type(ty)?;
        self.state().is_array_type(ty)
    }

    // port: tsc/internal/checker/exports.go:Checker.IsReadonlySymbol
    pub fn is_readonly_symbol(&mut self, symbol: SymbolRef) -> Result<bool, Error> {
        let symbol = self.check_symbol_ref(symbol)?;
        self.state_mut().is_readonly_symbol(symbol)
    }

    // port: tsc/internal/checker/exports.go:Checker.GetReturnTypeOfSignature
    pub fn get_return_type_of_signature(
        &mut self,
        signature: SignatureRef,
    ) -> Result<TypeRef, Error> {
        let signature = self.check_signature(signature)?;
        let ty = self.state_mut().return_type_of_signature(signature)?;
        Ok(self.type_ref(ty))
    }

    // port: tsc/internal/checker/exports.go:Checker.HasEffectiveRestParameter
    pub fn has_effective_rest_parameter(&mut self, signature: SignatureRef) -> Result<bool, Error> {
        let signature = self.check_signature(signature)?;
        self.state_mut().effective_rest_parameter(signature)
    }

    // port: tsc/internal/checker/exports.go:Checker.GetLocalTypeParametersOfClassOrInterfaceOrTypeAlias
    pub fn get_local_type_parameters_of_class_or_interface_or_type_alias(
        &mut self,
        symbol: SymbolRef,
    ) -> Result<Vec<TypeRef>, Error> {
        let symbol = self.check_symbol_ref(symbol)?;
        let parameters = self.state_mut().get_local_type_parameters(symbol)?;
        Ok(self.type_refs(&parameters.iter().copied().collect::<Vec<_>>()))
    }

    // port: tsc/internal/checker/exports.go:Checker.GetContextualTypeForObjectLiteralElement
    pub fn get_contextual_type_for_object_literal_element(
        &mut self,
        element: NodeId,
        context_flags: u32,
    ) -> Result<Option<TypeRef>, Error> {
        let ty = self
            .state_mut()
            .contextual_property_type_with_flags(element, context_flags)?;
        Ok(ty.map(|ty| self.type_ref(ty)))
    }

    // port: tsc/internal/checker/exports.go:Checker.TypePredicateToString
    pub fn type_predicate_to_string(
        &mut self,
        predicate: TypePredicateRef,
    ) -> Result<JsString, Error> {
        let predicate = self.check_type_predicate(predicate)?;
        self.state_mut().type_predicate_to_string(predicate)
    }

    // port: tsc/internal/checker/exports.go:Checker.GetExpandedParameters
    pub fn get_expanded_parameters(
        &mut self,
        signature: SignatureRef,
        skip_union_expanding: bool,
    ) -> Result<Vec<Vec<SymbolRef>>, Error> {
        let signature = self.check_signature(signature)?;
        let lists = self
            .state_mut()
            .expanded_parameters(signature, skip_union_expanding)?;
        lists.iter().map(|list| self.symbol_refs(list)).collect()
    }

    // port: tsc/internal/checker/exports.go:Checker.GetResolvedSignature
    pub fn get_resolved_signature(&mut self, node: NodeId) -> Result<SignatureRef, Error> {
        let signature = self.state_mut().resolved_call_signature(node)?;
        Ok(self.signature_ref(signature))
    }

    // port: tsc/internal/checker/exports.go:Checker.GetTypeOfPropertyOfType
    /// The type of the named property of `ty`, or `None` without one.
    pub fn get_type_of_property_of_type(
        &mut self,
        ty: TypeRef,
        name: &[u8],
    ) -> Result<Option<TypeRef>, Error> {
        let ty = self.check_type(ty)?;
        let result = self.state_mut().property_type(ty, name)?;
        Ok(result.map(|ty| self.type_ref(ty)))
    }

    // port: tsc/internal/checker/exports.go:Checker.GetContextualTypeForArgumentAtIndex
    pub fn get_contextual_type_for_argument_at_index(
        &mut self,
        call: NodeId,
        index: usize,
    ) -> Result<Option<TypeRef>, Error> {
        let ty = self.state_mut().contextual_call_argument_at(call, index)?;
        Ok(ty.map(|ty| self.type_ref(ty)))
    }

    // port: tsc/internal/checker/exports.go:Checker.GetIndexSignaturesAtLocation
    pub fn get_index_signatures_at_location(&mut self, node: NodeId) -> Result<Vec<NodeId>, Error> {
        self.state_mut().index_signatures_at_location(node)
    }

    // port: tsc/internal/checker/exports.go:Checker.GetResolvedSymbol
    pub fn get_resolved_symbol(&mut self, node: NodeId) -> Result<SymbolRef, Error> {
        let symbol = self.state_mut().resolved_value_symbol(node)?;
        self.symbol_ref(symbol)
    }

    // port: tsc/internal/checker/exports.go:Checker.GetJsxNamespace
    pub fn get_jsx_namespace(&mut self, location: Option<NodeId>) -> Result<JsString, Error> {
        self.state_mut().jsx_namespace(location)
    }

    // port: tsc/internal/checker/exports.go:Checker.GetJsxFragmentFactory
    /// The first identifier of the fragment factory entity, or empty.
    pub fn get_jsx_fragment_factory(&mut self, location: NodeId) -> Result<JsString, Error> {
        let state = self.state_mut();
        let Some(entity) = state.jsx_fragment_factory_entity(Some(location))? else {
            return Ok(JsString::default());
        };
        let first = tsr_ast::utilities_middle::get_first_identifier(state.ast(entity)?, entity)?;
        Ok(state.node_text(first)?.into_js_string())
    }

    // port: tsc/internal/checker/exports.go:Checker.ResolveName
    pub fn resolve_name(
        &mut self,
        name: &[u8],
        location: Option<NodeId>,
        meaning: SymbolFlags,
        exclude_globals: bool,
    ) -> Result<Option<SymbolRef>, Error> {
        let symbol = self.state_mut().resolve_name_ex(
            location,
            name,
            meaning,
            None,
            true,
            exclude_globals,
        )?;
        self.optional_symbol(symbol)
    }

    // port: tsc/internal/checker/exports.go:Checker.GetSymbolFlags
    pub fn get_symbol_flags(&mut self, symbol: SymbolRef) -> Result<SymbolFlags, Error> {
        let symbol = self.check_symbol_ref(symbol)?;
        self.state_mut().module_symbol_flags(symbol, false, false)
    }

    // port: tsc/internal/checker/exports.go:Checker.GetBaseTypes
    pub fn get_base_types(&mut self, ty: TypeRef) -> Result<Vec<TypeRef>, Error> {
        let ty = self.check_type(ty)?;
        let bases = self.state_mut().interface_base_types(ty)?;
        Ok(self.type_refs(&bases.iter().copied().collect::<Vec<_>>()))
    }

    // port: tsc/internal/checker/exports.go:Checker.GetApparentType
    pub fn get_apparent_type(&mut self, ty: TypeRef) -> Result<TypeRef, Error> {
        let ty = self.check_type(ty)?;
        let result = self.state_mut().apparent_type(ty)?;
        Ok(self.type_ref(result))
    }

    // port: tsc/internal/checker/exports.go:Checker.GetReducedType
    pub fn get_reduced_type(&mut self, ty: TypeRef) -> Result<TypeRef, Error> {
        let ty = self.check_type(ty)?;
        let result = self.state_mut().get_reduced_type(ty)?;
        Ok(self.type_ref(result))
    }

    // port: tsc/internal/checker/exports.go:Checker.GetFullyQualifiedName
    /// The symbol's name qualified through its parent chain.
    pub fn get_fully_qualified_name(&mut self, symbol: SymbolRef) -> Result<JsString, Error> {
        let symbol = self.check_symbol_ref(symbol)?;
        self.state_mut().fully_qualified_name(symbol, None)
    }

    // port: tsc/internal/checker/exports.go:Checker.GetBaseConstructorTypeOfClass
    pub fn get_base_constructor_type_of_class(&mut self, ty: TypeRef) -> Result<TypeRef, Error> {
        let ty = self.check_type(ty)?;
        let result = self.state_mut().class_base_constructor_type(ty)?;
        Ok(self.type_ref(result))
    }

    // port: tsc/internal/checker/exports.go:Checker.GetMemberOverrideModifierStatus
    pub fn get_member_override_modifier_status(
        &mut self,
        class: NodeId,
        member: NodeId,
        member_symbol: Option<SymbolRef>,
    ) -> Result<MemberOverrideStatus, Error> {
        let member_symbol = member_symbol
            .map(|symbol| self.check_symbol_ref(symbol))
            .transpose()?;
        self.state_mut()
            .member_override_modifier_status(class, member, member_symbol)
    }

    /// The same override decision for an edit's private generated member,
    /// whose syntax is not part of the checker's immutable input graph.
    pub fn member_override_status_for_flags(
        &mut self,
        class: NodeId,
        symbol: SymbolRef,
        flags: u32,
    ) -> Result<MemberOverrideStatus, Error> {
        let symbol = self.check_symbol_ref(symbol)?;
        self.state_mut().member_override_status_from_flags(
            class,
            symbol,
            flags & mf::OVERRIDE != 0,
            flags & mf::ABSTRACT != 0,
            flags & mf::STATIC != 0,
        )
    }

    // port: tsc/internal/checker/exports.go:Checker.GetRestTypeOfSignature
    pub fn get_rest_type_of_signature(
        &mut self,
        signature: SignatureRef,
    ) -> Result<TypeRef, Error> {
        let signature = self.check_signature(signature)?;
        let any = self.state().builtins.any_type;
        let Some(mut rest) = self.state_mut().rest_parameter_type(signature)? else {
            return Ok(self.type_ref(any));
        };
        // port: tsc/internal/checker/checker.go:Checker.tryGetRestTypeOfSignature
        // A tuple rest parameter is first sliced to its rest element (the
        // union of the elements after its fixed length); a tuple without one
        // has no rest type.
        if self.state().is_tuple_type(rest)? {
            let fixed = {
                let state = self.state();
                state.types.tuple(state.types.target(rest)?)?.fixed_length as usize
            };
            let Some(sliced) = self
                .state_mut()
                .tuple_slice_element_type(rest, fixed, 0, false)?
            else {
                return Ok(self.type_ref(any));
            };
            rest = sliced;
        }
        let number = self.state().builtins.number_type;
        let state = self.state_mut();
        let element = match state.index_info_of_type(rest, number)? {
            Some(info) => state.signatures.index_info(info)?.value_type,
            None => any,
        };
        Ok(self.type_ref(element))
    }

    // port: tsc/internal/checker/exports.go:Checker.GetTypeArguments
    pub fn get_type_arguments(&mut self, ty: TypeRef) -> Result<Vec<TypeRef>, Error> {
        let ty = self.check_type(ty)?;
        let arguments = self.state_mut().get_type_arguments(ty)?;
        Ok(self.type_refs(&arguments.iter().copied().collect::<Vec<_>>()))
    }

    // port: tsc/internal/checker/exports.go:Checker.GetIndexInfoOfType
    pub fn get_index_info_of_type(
        &mut self,
        ty: TypeRef,
        key: TypeRef,
    ) -> Result<Option<IndexInfoRef>, Error> {
        let ty = self.check_type(ty)?;
        let key = self.check_type(key)?;
        let info = self.state_mut().index_info_of_type(ty, key)?;
        Ok(info.map(|id| self.index_info_ref(id)))
    }

    // port: tsc/internal/checker/exports.go:Checker.GetIndexInfosOfType
    pub fn get_index_infos_of_type(&mut self, ty: TypeRef) -> Result<Vec<IndexInfoRef>, Error> {
        let ty = self.check_type(ty)?;
        let infos = self.state_mut().index_infos_of_type(ty)?;
        Ok(infos
            .into_iter()
            .map(|id| self.index_info_ref(id))
            .collect())
    }

    // port: tsc/internal/checker/exports.go:Checker.IsContextSensitive
    pub fn is_context_sensitive(&self, node: NodeId) -> Result<bool, Error> {
        self.state().expression_is_context_sensitive(node)
    }

    // port: tsc/internal/checker/exports.go:Checker.FillMissingTypeArguments
    /// The pin's minimum count is unused by its body.
    pub fn fill_missing_type_arguments(
        &mut self,
        arguments: &[TypeRef],
        parameters: &[TypeRef],
        _min_type_argument_count: usize,
        is_javascript_implicit_any: bool,
    ) -> Result<Vec<TypeRef>, Error> {
        let arguments = self.check_types(arguments)?;
        let parameters = self.check_types(parameters)?;
        let filled = self.state_mut().fill_missing_type_arguments(
            &arguments,
            &parameters,
            is_javascript_implicit_any,
        )?;
        Ok(self.type_refs(&filled.iter().copied().collect::<Vec<_>>()))
    }

    // port: tsc/internal/checker/exports.go:Checker.GetMinTypeArgumentCount
    pub fn get_min_type_argument_count(&self, parameters: &[TypeRef]) -> Result<usize, Error> {
        let parameters = self.check_types(parameters)?;
        self.state().min_type_argument_count(&parameters)
    }

    // port: tsc/internal/checker/exports.go:Checker.GetWidenedLiteralType
    pub fn get_widened_literal_type(&mut self, ty: TypeRef) -> Result<TypeRef, Error> {
        let ty = self.check_type(ty)?;
        let result = self.state_mut().widen_literal_type(ty)?;
        Ok(self.type_ref(result))
    }

    // port: tsc/internal/checker/exports.go:Checker.IsTypeAssignableTo
    pub fn is_type_assignable_to(
        &mut self,
        source: TypeRef,
        target: TypeRef,
    ) -> Result<bool, Error> {
        let source = self.check_type(source)?;
        let target = self.check_type(target)?;
        self.state_mut()
            .is_type_related_to(source, target, RelationKind::Assignable)
    }

    // port: tsc/internal/checker/exports.go:Checker.RequiresAddingImplicitUndefined
    pub fn requires_adding_implicit_undefined(&mut self, node: NodeId) -> Result<bool, Error> {
        let state = self.state_mut();
        let view = state.ast(node)?;
        let enclosing = match tsr_ast::utilities::find_ancestor(view, Some(node), |read| {
            tsr_ast::is_declaration(read)
        })? {
            Some(declaration) => declaration,
            None => view
                .file_info()
                .root
                .ok_or(Error::MissingLink("source file root"))?,
        };
        let Some(symbol) = state.raw_declaration_symbol(node)? else {
            return Ok(false);
        };
        state.emit_requires_undefined(node, Some(symbol), Some(enclosing))
    }

    // port: tsc/internal/checker/exports.go:Checker.RemoveMissingOrUndefinedType
    pub fn remove_missing_or_undefined_type(&mut self, ty: TypeRef) -> Result<TypeRef, Error> {
        let ty = self.check_type(ty)?;
        let result = self.state_mut().remove_missing_or_undefined(ty)?;
        Ok(self.type_ref(result))
    }

    // port: tsc/internal/checker/exports.go:Checker.GetWidenedType
    pub fn get_widened_type(&mut self, ty: TypeRef) -> Result<TypeRef, Error> {
        let ty = self.check_type(ty)?;
        let result = self.state_mut().widened_type(ty)?;
        Ok(self.type_ref(result))
    }

    // port: tsc/internal/checker/exports.go:Checker.CompareSymbols
    pub fn compare_symbols(
        &self,
        first: Option<SymbolRef>,
        second: Option<SymbolRef>,
    ) -> Result<Ordering, Error> {
        let first = first.map(|s| self.check_symbol_ref(s)).transpose()?;
        let second = second.map(|s| self.check_symbol_ref(s)).transpose()?;
        self.state().compare_symbols(first, second)
    }

    // port: tsc/internal/checker/checker.go:Checker.GetAmbientModules
    /// The ambient module symbols: the globals named as modules, then the
    /// merged pattern modules not already listed.
    pub fn get_ambient_modules(&mut self) -> Result<Vec<SymbolRef>, Error> {
        let state = self.state_mut();
        let mut modules = Vec::new();
        if let Some(globals) = state.builtins.globals {
            for (name, symbol) in state.table(globals)? {
                if let Some(symbol) = symbol {
                    if name.first() == Some(&b'"') && !modules.contains(&symbol) {
                        modules.push(symbol);
                    }
                }
            }
        }
        for pattern in state.module_aliases.patterns.clone() {
            let Some(symbol) = pattern.symbol else {
                continue;
            };
            let symbol = state.get_merged_symbol(symbol);
            if !modules.contains(&symbol) {
                modules.push(symbol);
            }
        }
        self.symbol_refs(&modules)
    }

    // port: tsc/internal/checker/checker.go:Checker.GetDiagnostics
    /// The file's semantic diagnostics after checking it.
    pub fn get_diagnostics(&mut self, source: NodeId) -> Result<Vec<tsr_ast::Diagnostic>, Error> {
        self.semantic_diagnostics(source)
    }

    // port: tsc/internal/checker/checker.go:Checker.GetGlobalDiagnostics
    pub fn get_global_diagnostics(&mut self) -> Result<Vec<tsr_ast::Diagnostic>, Error> {
        self.global_diagnostics()
    }

    // port: tsc/internal/checker/checker.go:Checker.GetSuggestionDiagnostics
    pub fn get_suggestion_diagnostics(
        &mut self,
        source: NodeId,
    ) -> Result<Vec<tsr_ast::Diagnostic>, Error> {
        self.recorded_suggestions(source)
    }

    // port: tsc/internal/checker/checker.go:Checker.GetTypeOfSymbolAtLocation
    pub fn get_type_of_symbol_at_location(
        &mut self,
        symbol: SymbolRef,
        location: Option<NodeId>,
    ) -> Result<TypeRef, Error> {
        let symbol = self.check_symbol_ref(symbol)?;
        let ty = self
            .state_mut()
            .type_of_symbol_at_location(symbol, location)?;
        Ok(self.type_ref(ty))
    }

    // port: tsc/internal/checker/checker.go:Checker.IsDeprecatedDeclaration
    pub fn is_deprecated_declaration(&self, declaration: NodeId) -> Result<bool, Error> {
        self.state().is_deprecated_declaration(declaration)
    }

    // port: tsc/internal/checker/checker.go:Checker.IsNullableType
    pub fn is_nullable_type(&mut self, ty: TypeRef) -> Result<bool, Error> {
        let ty = self.check_type(ty)?;
        Ok(self
            .state_mut()
            .type_facts(ty, crate::type_facts::IS_UNDEFINED_OR_NULL)?
            != 0)
    }

    /// `Checker.GetNonNullableType`, ported as `non_nullable_type`.
    pub fn get_non_nullable_type(&mut self, ty: TypeRef) -> Result<TypeRef, Error> {
        let ty = self.check_type(ty)?;
        let result = self.state_mut().non_nullable_type(ty)?;
        Ok(self.type_ref(result))
    }

    /// `Checker.IsEmptyAnonymousObjectType`.
    pub fn is_empty_anonymous_object_type(&mut self, ty: TypeRef) -> Result<bool, Error> {
        let ty = self.check_type(ty)?;
        self.state_mut().is_empty_anonymous_object_type(ty)
    }

    /// `Checker.GetPromisedTypeOfPromise`.
    pub fn get_promised_type_of_promise(&mut self, ty: TypeRef) -> Result<Option<TypeRef>, Error> {
        let ty = self.check_type(ty)?;
        let result = self.state_mut().get_promised_type_of_promise(ty)?;
        Ok(result.map(|ty| self.type_ref(ty)))
    }

    /// `Checker.IsLibTypeForHoverVerbosity`.
    pub fn is_lib_type_for_hover_verbosity(&self, ty: TypeRef) -> Result<bool, Error> {
        let ty = self.check_type(ty)?;
        self.state().is_lib_type_for_hover_verbosity(ty)
    }

    /// `Checker.IsLibSymbolForHoverVerbosity`.
    pub fn is_lib_symbol_for_hover_verbosity(
        &self,
        symbol: Option<SymbolRef>,
    ) -> Result<bool, Error> {
        let symbol = symbol.map(|s| self.check_symbol_ref(s)).transpose()?;
        self.state().is_lib_symbol_for_hover_verbosity(symbol)
    }

    // port: tsc/internal/checker/checker.go:Checker.ResolveAlias
    /// The alias target, and whether it resolved to something other than the
    /// unknown symbol.
    pub fn resolve_alias(&mut self, symbol: SymbolRef) -> Result<(SymbolRef, bool), Error> {
        let symbol = self.check_symbol_ref(symbol)?;
        let state = self.state_mut();
        let resolved = state.resolve_alias(symbol)?;
        let unknown = state.builtins.unknown_symbol;
        Ok((self.symbol_ref(resolved)?, resolved != unknown))
    }

    // port: tsc/internal/checker/checker.go:Checker.GetAliasedSymbol
    pub fn get_aliased_symbol(&mut self, symbol: SymbolRef) -> Result<SymbolRef, Error> {
        let symbol = self.check_symbol_ref(symbol)?;
        let resolved = self.state_mut().resolve_alias(symbol)?;
        self.symbol_ref(resolved)
    }

    // port: tsc/internal/checker/checker.go:Checker.TryGetThisTypeAtEx
    /// The `this` type at `node`; `None` for a JSDoc node the binder never saw.
    pub fn try_get_this_type_at_ex(
        &mut self,
        node: NodeId,
        include_global_this: bool,
        container: Option<NodeId>,
    ) -> Result<Option<TypeRef>, Error> {
        let state = self.state_mut();
        let view = state.ast(node)?;
        let reparsed = tsr_ast::utilities_containers::get_reparsed_node_for_node(view, Some(node))?
            .unwrap_or(node);
        let flags = view.node(reparsed)?.flags();
        if flags & tsr_ast::node_flags::JS_DOC != 0 && flags & tsr_ast::node_flags::REPARSED == 0 {
            return Ok(None);
        }
        let container = tsr_ast::utilities_containers::get_reparsed_node_for_node(view, container)?;
        let ty = state.try_this_type_at(reparsed, include_global_this, container)?;
        Ok(ty.map(|ty| self.type_ref(ty)))
    }

    // port: tsc/internal/checker/checker.go:Checker.isValidPropertyAccessForCompletions
    // port: tsc/internal/checker/services.go:Checker.IsValidPropertyAccessForCompletions
    pub fn is_valid_property_access_for_completions(
        &mut self,
        node: NodeId,
        ty: TypeRef,
        property: SymbolRef,
    ) -> Result<bool, Error> {
        let read = self.state().node(node)?;
        let is_super = read.kind() == K::PropertyAccessExpression
            && match read.expression() {
                Some(expression) => self.state().node(expression)?.kind() == K::SuperKeyword,
                None => false,
            };
        self.is_property_accessible(node, is_super, false, ty, property)
    }

    // port: tsc/internal/checker/checker.go:getNameFromIndexInfo
    /// The parameter name an index signature displays, `x` without a declaration.
    pub fn get_name_from_index_info(&self, info: IndexInfoRef) -> Result<JsString, Error> {
        let id = self.check_index_info(info)?;
        self.state().name_from_index_info(id)
    }
}

impl CheckerState {
    // port: tsc/internal/checker/checker.go:getNameFromIndexInfo
    pub(crate) fn name_from_index_info(&self, info: IndexInfoId) -> Result<JsString, Error> {
        let Some(declaration) = self.signatures.index_info(info)?.declaration else {
            return Ok(JsString::from_bytes(b"x".as_slice()));
        };
        let view = self.ast(declaration)?;
        let parameters = self.source_list(declaration, view.node(declaration)?.parameter_list())?;
        let parameter = *parameters
            .first()
            .ok_or(Error::MissingLink("index signature parameter"))?;
        let name = view
            .node(parameter)?
            .name()
            .ok_or(Error::MissingLink("index parameter name"))?;
        Ok(tsr_scanner::declaration_name_to_string(view, Some(name))?)
    }

    // port: tsc/internal/checker/checker.go:Checker.GetTypeOfSymbolAtLocation
    pub(crate) fn type_of_symbol_at_location(
        &mut self,
        symbol: SymbolId,
        location: Option<NodeId>,
    ) -> Result<TypeId, Error> {
        let symbol = self.get_export_symbol_of_value_symbol_if_exported(symbol)?;
        if let Some(mut location) = location {
            let view = self.ast(location)?;
            let read = view.node(location)?;
            let parent = read.parent();
            let parent_kind = parent
                .map(|parent| view.node(parent).map(|p| p.kind()))
                .transpose()?;
            if matches!(
                read.kind().known(),
                Some(K::Identifier | K::PrivateIdentifier)
            ) && !(tsr_ast::utilities_targets::is_jsx_tag_name(view, location)?
                || matches!(
                    parent_kind.and_then(tsr_ast::NodeKind::known),
                    Some(K::JsxAttribute | K::JsxNamespacedName)
                ))
            {
                if tsr_ast::utilities_middle::is_right_side_of_qualified_name_or_property_access(
                    view, location,
                )? {
                    location = parent.ok_or(Error::MissingLink("access parent"))?;
                }
                let write = tsr_ast::utilities::is_write_access(view, location)?;
                if crate::query::is_expression_node(view, location)?
                    && (!tsr_ast::is_assignment_target(view, location)? || write)
                {
                    let ty = if write && view.node(location)?.kind() == K::PropertyAccessExpression
                    {
                        self.check_property_access_ex(location, 0, true)?
                    } else {
                        self.get_type_of_expression(location)?
                    };
                    let resolved = self
                        .query
                        .resolved_symbols
                        .try_get(location)
                        .copied()
                        .flatten();
                    let resolved = resolved
                        .map(|resolved| {
                            self.get_export_symbol_of_value_symbol_if_exported(resolved)
                        })
                        .transpose()?;
                    if resolved == Some(symbol) {
                        return self.remove_optional_type_marker(ty);
                    }
                }
            }
            let view = self.ast(location)?;
            let parent = view.node(location)?.parent();
            if let Some(parent) = parent {
                if tsr_ast::utilities_positions::is_declaration_name(view, location)?
                    && view.node(parent)?.kind() == K::SetAccessor
                    && self.annotated_accessor_type_node(Some(parent))?.is_some()
                {
                    let accessor = self
                        .raw_declaration_symbol(parent)?
                        .ok_or(Error::MissingLink("accessor symbol"))?;
                    return self.write_type_of_accessors(accessor);
                }
                if self.is_right_side_of_access_expression(location)?
                    && tsr_ast::utilities::is_write_access(view, parent)?
                {
                    return self.write_type_of_symbol(symbol);
                }
            }
        }
        self.non_missing_symbol_type(symbol)
    }

    // port: tsc/internal/checker/utilities.go:isRightSideOfAccessExpression
    pub(crate) fn is_right_side_of_access_expression(&self, node: NodeId) -> Result<bool, Error> {
        let Some(parent) = self.node(node)?.parent() else {
            return Ok(false);
        };
        let read = self.node(parent)?;
        Ok(match read.kind().known() {
            Some(K::PropertyAccessExpression) => read.name() == Some(node),
            Some(K::ElementAccessExpression) => {
                read.data_source()
                    .as_element_access_expression()
                    .and_then(|access| access.argument_expression())
                    == Some(node)
            }
            _ => false,
        })
    }

    // port: tsc/internal/checker/checker.go:Checker.getDefaultFromTypeParameter
    pub(crate) fn default_from_type_parameter(
        &mut self,
        ty: TypeId,
    ) -> Result<Option<TypeId>, Error> {
        if self.types.flags(ty)? & tf::TYPE_PARAMETER == 0 {
            return Ok(None);
        }
        let default = self.resolved_type_parameter_default(ty)?;
        Ok((default != self.builtins.no_constraint_type
            && default != self.builtins.circular_constraint_type)
            .then_some(default))
    }

    // port: tsc/internal/checker/checker.go:Checker.getApplicableIndexInfos
    pub(crate) fn applicable_index_infos(
        &mut self,
        ty: TypeId,
        key: TypeId,
    ) -> Result<Vec<IndexInfoId>, Error> {
        let mut applicable = Vec::new();
        for info in self.index_infos_of_type(ty)? {
            let key_type = self.signatures.index_info(info)?.key_type;
            if self.applicable_index_type(key, key_type)? {
                applicable.push(info);
            }
        }
        Ok(applicable)
    }

    // port: tsc/internal/checker/checker.go:Checker.getIndexSignaturesAtLocation
    pub(crate) fn index_signatures_at_location(
        &mut self,
        node: NodeId,
    ) -> Result<Vec<NodeId>, Error> {
        let mut signatures = Vec::new();
        let read = self.node(node)?;
        let Some(parent) = read.parent() else {
            return Ok(signatures);
        };
        let parent_read = self.node(parent)?;
        if read.kind() == K::Identifier
            && parent_read.kind() == K::PropertyAccessExpression
            && parent_read.name() == Some(node)
        {
            let object = parent_read
                .expression()
                .ok_or(Error::MissingLink("property access expression"))?;
            let key = self.literal_type_from_property_name(node)?;
            let object_type = self.get_type_of_expression(object)?;
            for ty in self.distributed_types(object_type)? {
                for info in self.applicable_index_infos(ty, key)? {
                    if let Some(declaration) = self.signatures.index_info(info)?.declaration {
                        if !signatures.contains(&declaration) {
                            signatures.push(declaration);
                        }
                    }
                }
            }
        }
        Ok(signatures)
    }

    // port: tsc/internal/checker/checker.go:Checker.getMemberOverrideModifierStatus
    pub(crate) fn member_override_modifier_status(
        &mut self,
        node: NodeId,
        member: NodeId,
        member_symbol: Option<SymbolId>,
    ) -> Result<MemberOverrideStatus, Error> {
        let Some(member_symbol) = member_symbol.filter(|_| {
            self.node(member)
                .is_ok_and(|read| tsr_ast::NodeAccess::name(&read).is_some())
        }) else {
            return Ok(MemberOverrideStatus::None);
        };
        let view = self.ast(member)?;
        let has_override = tsr_ast::utilities::has_syntactic_modifier(view, member, mf::OVERRIDE)?;
        let is_abstract = tsr_ast::utilities::has_syntactic_modifier(view, member, mf::ABSTRACT)?;
        let is_static = tsr_ast::utilities::is_static(view, member)?;
        self.member_override_status_from_flags(
            node,
            member_symbol,
            has_override,
            is_abstract,
            is_static,
        )
    }

    pub(crate) fn member_override_status_from_flags(
        &mut self,
        node: NodeId,
        member_symbol: SymbolId,
        has_override: bool,
        is_abstract: bool,
        is_static: bool,
    ) -> Result<MemberOverrideStatus, Error> {
        let Some(class_symbol) = self.get_symbol_of_declaration(node)? else {
            return Ok(MemberOverrideStatus::None);
        };
        let ty = self.get_declared_type_of_symbol(class_symbol)?;
        let with_this = self.get_type_with_optional_this_argument(ty, None, false)?;
        let static_type = self.get_type_of_symbol(class_symbol)?;
        let mut base_with_this = None;
        if tsr_ast::utilities_class::get_class_extends_heritage_element(self.ast(node)?, node)?
            .is_some()
        {
            if let Some(base) = self.interface_base_types(ty)?.first().copied() {
                let this = self.types.interface(ty)?.this_type;
                base_with_this =
                    Some(self.get_type_with_optional_this_argument(base, this, false)?);
            }
        }
        let static_base = self.class_base_constructor_type(ty)?;
        self.override_modifier_status(
            node,
            crate::class_overrides::OverrideTypes {
                ty,
                with_this,
                static_type,
                static_base,
                base: base_with_this,
            },
            crate::class_overrides::OverrideMember {
                has_override,
                is_abstract,
                is_static,
                is_parameter_property: false,
                symbol: member_symbol,
            },
            None,
        )
    }
}

use crate::TypeId;

impl Operation<'_> {
    /// Trailing defaults that can be elided by an editing type annotation.
    // port: tsc/internal/ls/codeactions_fixmissingtypeannotation.go:endOfRequiredTypeParameters
    pub fn required_reference_arguments(&mut self, ty: TypeRef) -> Result<Option<usize>, Error> {
        let id = self.check_type(ty)?;
        if self.state().types.get(id)?.object_flags & crate::object_flags::REFERENCE == 0 {
            return Ok(None);
        }
        let target = self.state().types.target(id)?;
        if !matches!(
            self.state().types.get(target)?.kind(),
            crate::TypeKind::Interface | crate::TypeKind::Tuple
        ) {
            return Ok(None);
        }
        let interface = self.state().types.interface(target)?;
        let parameters = self.type_refs(interface.type_parameters());
        let outer = interface.outer_type_parameter_count as usize;
        let arguments = self.get_type_arguments(ty)?;
        for cutoff in 0..arguments.len() {
            if cutoff < outer || cutoff >= parameters.len() {
                continue;
            }
            let parameter = self.check_type(parameters[cutoff])?;
            let Some(symbol) = self.state().types.get(parameter)?.symbol else {
                continue;
            };
            let symbol = self.symbol_ref(symbol)?;
            let mut default = false;
            for node in self.symbol_declarations(symbol)?.iter().flatten() {
                if self
                    .node(node)?
                    .data_source()
                    .as_type_parameter_declaration()
                    .is_some_and(|d| d.default_type().is_some())
                {
                    default = true;
                    break;
                }
            }
            if !default {
                continue;
            }
            let filled =
                self.fill_missing_type_arguments(&arguments[..cutoff], &parameters, cutoff, false)?;
            if filled.iter().zip(&arguments).all(|(a, b)| a == b) {
                return Ok(Some(cutoff));
            }
        }
        Ok(Some(arguments.len()))
    }
}
