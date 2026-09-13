//! Type constructors (`tsc/internal/checker/checker.go`): intrinsics, literals,
//! object types, tuple targets, type references and type parameters, plus the
//! checker's synthetic AST nodes. Everything here is `&mut self` on the checker
//! state and never holds a borrow into a store across a call (ADR 0008).

use crate::key::{type_list_key, KeyBuilder};
use crate::{
    element_flags, object_flags, type_flags, CheckerState, ElementFlags, Error, IndexInfoId,
    InterfaceData, IntrinsicData, LiteralData, LiteralValue, ObjectData, ObjectFlags, Payload,
    ReferenceData, SignatureId, SymbolList, TupleData, TupleElementInfo, TypeFlags, TypeId,
    TypeKind, TypeList, TypeParameterData,
};
use std::collections::HashMap;
use std::sync::Arc;
use ts_arena::{NodeId, SymbolId};
use ts_ast::{
    check_flags, symbol_flags, Factory, FactoryMethods, JsString, SymbolTableId, SyntaxKind,
    SyntheticExpressionData,
};
use ts_jsnum::{Number, PseudoBigInt};

impl CheckerState {
    // port: tsc/internal/checker/checker.go:Checker.newIntrinsicType
    pub(crate) fn new_intrinsic_type(
        &mut self,
        flags: TypeFlags,
        name: &[u8],
    ) -> Result<TypeId, Error> {
        self.new_intrinsic_type_ex(flags, name, object_flags::NONE)
    }

    // port: tsc/internal/checker/checker.go:Checker.newIntrinsicTypeEx
    pub(crate) fn new_intrinsic_type_ex(
        &mut self,
        flags: TypeFlags,
        name: &[u8],
        object_flags: ObjectFlags,
    ) -> Result<TypeId, Error> {
        self.types.new_type(
            flags,
            object_flags,
            Payload::Intrinsic(IntrinsicData {
                name: JsString::from_bytes(name),
            }),
        )
    }

    // port: tsc/internal/checker/checker.go:Checker.createWideningType
    pub(crate) fn create_widening_type(&mut self, non_widening: TypeId) -> Result<TypeId, Error> {
        if self.options.strict_null_checks {
            return Ok(non_widening);
        }
        let flags = self.types.flags(non_widening)?;
        let name = self.types.intrinsic(non_widening)?.name.clone();
        let t = self.new_intrinsic_type(flags, name.as_bytes())?;
        self.types.get_mut(t)?.object_flags |= object_flags::CONTAINS_WIDENING_TYPE;
        Ok(t)
    }

    // port: tsc/internal/checker/checker.go:Checker.createUnknownUnionType
    pub(crate) fn create_unknown_union_type(&mut self) -> Result<TypeId, Error> {
        if self.options.strict_null_checks {
            let types = [
                self.builtins.undefined_type,
                self.builtins.null_type,
                self.builtins.unknown_empty_object_type,
            ];
            return self.get_union_type(&types);
        }
        Ok(self.builtins.unknown_type)
    }

    /// A literal's regular type is itself unless it is the fresh copy of another.
    // port: tsc/internal/checker/checker.go:Checker.newLiteralType
    pub(crate) fn new_literal_type(
        &mut self,
        flags: TypeFlags,
        value: LiteralValue,
        regular: Option<TypeId>,
    ) -> Result<TypeId, Error> {
        let id = self.types.next_id()?;
        let created = self.types.new_type(
            flags,
            object_flags::NONE,
            Payload::Literal(LiteralData {
                value,
                fresh: None,
                regular: regular.unwrap_or(id),
            }),
        )?;
        debug_assert_eq!(id, created);
        Ok(created)
    }

    // port: tsc/internal/checker/checker.go:Checker.getStringLiteralType
    pub(crate) fn get_string_literal_type(&mut self, value: JsString) -> Result<TypeId, Error> {
        if let Some(t) = self.types.caches.string_literal_types.get(&value) {
            return Ok(*t);
        }
        let t = self.new_literal_type(
            type_flags::STRING_LITERAL,
            LiteralValue::String(value.clone()),
            None,
        )?;
        self.types.caches.string_literal_types.insert(value, t);
        Ok(t)
    }

    /// NaN is cached apart from the map because NaN never equals a map key.
    // port: tsc/internal/checker/checker.go:Checker.getNumberLiteralType
    pub(crate) fn get_number_literal_type(&mut self, value: Number) -> Result<TypeId, Error> {
        let Some(key) = crate::NumberKey::new(value) else {
            if let Some(nan) = self.types.caches.nan_type {
                return Ok(nan);
            }
            let nan = self.new_literal_type(
                type_flags::NUMBER_LITERAL,
                LiteralValue::Number(value),
                None,
            )?;
            self.types.caches.nan_type = Some(nan);
            return Ok(nan);
        };
        if let Some(t) = self.types.caches.number_literal_types.get(&key) {
            return Ok(*t);
        }
        let t = self.new_literal_type(
            type_flags::NUMBER_LITERAL,
            LiteralValue::Number(value),
            None,
        )?;
        self.types.caches.number_literal_types.insert(key, t);
        Ok(t)
    }

    // port: tsc/internal/checker/checker.go:Checker.getBigIntLiteralType
    pub(crate) fn get_big_int_literal_type(
        &mut self,
        value: PseudoBigInt,
    ) -> Result<TypeId, Error> {
        if let Some(t) = self.types.caches.bigint_literal_types.get(&value) {
            return Ok(*t);
        }
        let t = self.new_literal_type(
            type_flags::BIG_INT_LITERAL,
            LiteralValue::BigInt(value.clone()),
            None,
        )?;
        self.types.caches.bigint_literal_types.insert(value, t);
        Ok(t)
    }

    // port: tsc/internal/checker/checker.go:Checker.getFreshTypeOfLiteralType
    pub(crate) fn get_fresh_type_of_literal_type(&mut self, t: TypeId) -> Result<TypeId, Error> {
        let record = *self.types.get(t)?;
        if record.flags & type_flags::FRESHABLE == 0 {
            return Ok(t);
        }
        if let Some(fresh) = self.types.literal(t)?.fresh {
            return Ok(fresh);
        }
        let value = self.types.literal(t)?.value.clone();
        let fresh = self.new_literal_type(record.flags, value, Some(t))?;
        self.types.get_mut(fresh)?.symbol = record.symbol;
        self.types.literal_mut(fresh)?.fresh = Some(fresh);
        self.types.literal_mut(t)?.fresh = Some(fresh);
        Ok(fresh)
    }

    // port: tsc/internal/checker/checker.go:Checker.getRegularTypeOfLiteralType
    pub(crate) fn get_regular_type_of_literal_type(&mut self, t: TypeId) -> Result<TypeId, Error> {
        let flags = self.types.flags(t)?;
        if flags & type_flags::FRESHABLE != 0 {
            return Ok(self.types.literal(t)?.regular);
        }
        if flags & type_flags::UNION != 0 {
            if let Some(regular) = self.types.union(t)?.regular_type {
                return Ok(regular);
            }
            let regular = self
                .map_type(t, &mut |state, s| {
                    state.get_regular_type_of_literal_type(s).map(Some)
                })?
                .ok_or(Error::MissingLink("regular type of union"))?;
            self.types.union_mut(t)?.regular_type = Some(regular);
            return Ok(regular);
        }
        Ok(t)
    }

    // port: tsc/internal/checker/checker.go:isFreshLiteralType
    pub(crate) fn is_fresh_literal_type(&self, t: TypeId) -> Result<bool, Error> {
        Ok(self.types.flags(t)? & type_flags::FRESHABLE != 0
            && self.types.literal(t)?.fresh == Some(t))
    }

    /// Object kinds beyond anonymous, reference, interface and tuple types arrive
    /// with mapped types, reverse mapping, evolving arrays and instantiation
    /// expressions (P3/P4); asking for one is a named failure.
    // port: tsc/internal/checker/checker.go:Checker.newObjectType
    pub(crate) fn new_object_type(
        &mut self,
        object_flags: ObjectFlags,
        symbol: Option<SymbolId>,
    ) -> Result<TypeId, Error> {
        let payload = if object_flags & object_flags::CLASS_OR_INTERFACE != 0 {
            Payload::Interface(InterfaceData::default())
        } else if object_flags & object_flags::TUPLE != 0 {
            Payload::Tuple(TupleData {
                interface: InterfaceData::default(),
                element_infos: Arc::from([]),
                min_length: 0,
                fixed_length: 0,
                combined_flags: element_flags::NONE,
                readonly: false,
            })
        } else if object_flags & object_flags::REFERENCE != 0 {
            Payload::Reference(ReferenceData::default())
        } else if object_flags & object_flags::MAPPED != 0 {
            Payload::Mapped(crate::types::MappedData::default())
        } else if object_flags & object_flags::REVERSE_MAPPED != 0 {
            Payload::ReverseMapped(crate::types::ReverseMappedData::default())
        } else if object_flags & object_flags::EVOLVING_ARRAY != 0 {
            return Err(Error::Unsupported("newObjectType: EvolvingArrayType"));
        } else if object_flags & object_flags::INSTANTIATION_EXPRESSION_TYPE != 0 {
            Payload::InstantiationExpression(crate::types::InstantiationExpressionData::default())
        } else if object_flags & object_flags::ANONYMOUS != 0 {
            Payload::Anonymous(ObjectData::default())
        } else {
            return Err(Error::Unsupported("newObjectType: unhandled object flags"));
        };
        let t = self
            .types
            .new_type(type_flags::OBJECT, object_flags, payload)?;
        self.types.get_mut(t)?.symbol = symbol;
        Ok(t)
    }

    // port: tsc/internal/checker/checker.go:Checker.newAnonymousType
    pub(crate) fn new_anonymous_type(
        &mut self,
        symbol: Option<SymbolId>,
        members: Option<SymbolTableId>,
        call_signatures: &[SignatureId],
        construct_signatures: &[SignatureId],
        index_infos: &[IndexInfoId],
    ) -> Result<TypeId, Error> {
        let t = self.new_object_type(object_flags::ANONYMOUS, symbol)?;
        self.set_structured_type_members(
            t,
            members,
            call_signatures,
            construct_signatures,
            index_infos,
        )?;
        Ok(t)
    }

    // port: tsc/internal/checker/checker.go:Checker.setStructuredTypeMembers
    pub(crate) fn set_structured_type_members(
        &mut self,
        t: TypeId,
        members: Option<SymbolTableId>,
        call_signatures: &[SignatureId],
        construct_signatures: &[SignatureId],
        index_infos: &[IndexInfoId],
    ) -> Result<(), Error> {
        let container = self.types.get(t)?.symbol;
        let properties = self.get_named_members(members, container)?;
        let data = self.types.structured_mut(t)?;
        data.members = members;
        data.properties = properties;
        if call_signatures.is_empty() && construct_signatures.is_empty() {
            data.signatures = None;
        } else {
            let mut all = Vec::with_capacity(call_signatures.len() + construct_signatures.len());
            all.extend_from_slice(call_signatures);
            all.extend_from_slice(construct_signatures);
            data.signatures = Some(Arc::from(all));
        }
        data.call_signature_count = call_signatures.len() as u32;
        data.index_infos = if index_infos.is_empty() {
            None
        } else {
            Some(Arc::from(index_infos))
        };
        // An unsupported member read must not leave the completion bit set:
        // a later query would otherwise skip the same unfinished operation.
        self.types.get_mut(t)?.object_flags |= object_flags::MEMBERS_RESOLVED;
        Ok(())
    }

    /// Explicitly declared members of a class or interface precede inherited
    /// ones; each group is sorted with the ported symbol comparator.
    // port: tsc/internal/checker/checker.go:Checker.getNamedMembers
    pub(crate) fn get_named_members(
        &mut self,
        members: Option<SymbolTableId>,
        container: Option<SymbolId>,
    ) -> Result<Option<SymbolList>, Error> {
        let Some(members) = members else {
            return Ok(None);
        };
        let table: Vec<_> = self
            .table(members)?
            .into_iter()
            .map(|(name, symbol)| (JsString::from_bytes(name), symbol))
            .collect();
        if table.is_empty() {
            return Ok(None);
        }
        let container_is_class_like = match container {
            Some(container) => {
                self.symbol(container)?.flags() & (symbol_flags::CLASS | symbol_flags::INTERFACE)
                    != 0
            }
            None => false,
        };
        let mut result = Vec::with_capacity(table.len());
        let mut contained_count = 0;
        if container_is_class_like {
            for (id, symbol) in &table {
                let symbol = *symbol;
                let Some(symbol) = symbol else { continue };
                if self.is_named_member(symbol, id.as_bytes())?
                    && self.is_declaration_contained_by(
                        symbol,
                        container.expect("class-like container"),
                    )?
                {
                    result.push(symbol);
                }
            }
            contained_count = result.len();
        }
        for (id, symbol) in &table {
            let symbol = *symbol;
            let Some(symbol) = symbol else { continue };
            if self.is_named_member(symbol, id.as_bytes())?
                && (!container_is_class_like
                    || !self.is_declaration_contained_by(
                        symbol,
                        container.expect("class-like container"),
                    )?)
            {
                result.push(symbol);
            }
        }
        self.sort_symbols(&mut result[..contained_count])?;
        self.sort_symbols(&mut result[contained_count..])?;
        Ok(Some(Arc::from(result)))
    }

    // port: tsc/internal/checker/checker.go:Checker.isNamedMember
    pub(crate) fn is_named_member(&mut self, symbol: SymbolId, id: &[u8]) -> Result<bool, Error> {
        Ok(!is_reserved_member_name(id) && self.symbol_is_value(symbol)?)
    }

    // port: tsc/internal/checker/checker.go:Checker.symbolIsValue
    pub(crate) fn symbol_is_value(&mut self, symbol: SymbolId) -> Result<bool, Error> {
        self.symbol_is_value_ex(symbol, false)
    }

    // port: tsc/internal/checker/checker.go:Checker.symbolIsValueEx
    pub(crate) fn symbol_is_value_ex(
        &mut self,
        symbol: SymbolId,
        include_type_only_members: bool,
    ) -> Result<bool, Error> {
        let flags = self.symbol(symbol)?.flags();
        Ok(flags & symbol_flags::VALUE != 0
            || flags & symbol_flags::ALIAS != 0
                && self.module_symbol_flags(symbol, !include_type_only_members, false)?
                    & symbol_flags::VALUE
                    != 0)
    }

    // port: tsc/internal/checker/checker.go:Checker.isDeclarationContainedBy
    fn is_declaration_contained_by(
        &self,
        symbol: SymbolId,
        container: SymbolId,
    ) -> Result<bool, Error> {
        if let Some(declaration) = self.symbol(symbol)?.value_declaration() {
            let node = self.ast(declaration)?.node(declaration)?;
            for declaration in self.symbol_declarations(container)?.iter().flatten() {
                let containing = self.ast(declaration)?.node(declaration)?;
                // Source compares ranges, without an extra same-file condition.
                if node.pos() >= containing.pos() && node.end() <= containing.end() {
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }

    // port: tsc/internal/checker/checker.go:Checker.newTypeParameter
    pub(crate) fn new_type_parameter(&mut self, symbol: Option<SymbolId>) -> Result<TypeId, Error> {
        let t = self.types.new_type(
            type_flags::TYPE_PARAMETER,
            object_flags::NONE,
            Payload::TypeParameter(TypeParameterData::default()),
        )?;
        self.types.get_mut(t)?.symbol = symbol;
        Ok(t)
    }

    // port: tsc/internal/checker/checker.go:Checker.createTupleType
    pub(crate) fn create_tuple_type(&mut self, element_types: &[TypeId]) -> Result<TypeId, Error> {
        let infos: Vec<TupleElementInfo> = element_types
            .iter()
            .map(|_| TupleElementInfo {
                flags: element_flags::REQUIRED,
                labeled_declaration: None,
            })
            .collect();
        self.create_tuple_type_ex(element_types, &infos, false)
    }

    // port: tsc/internal/checker/checker.go:Checker.createTupleTypeEx
    pub(crate) fn create_tuple_type_ex(
        &mut self,
        element_types: &[TypeId],
        element_infos: &[TupleElementInfo],
        readonly: bool,
    ) -> Result<TypeId, Error> {
        let tuple_target = self.get_tuple_target_type(element_infos, readonly)?;
        if tuple_target == self.builtins.empty_generic_type {
            return Ok(self.builtins.empty_object_type);
        }
        if !element_types.is_empty() {
            return self.create_normalized_type_reference(tuple_target, element_types);
        }
        Ok(tuple_target)
    }

    /// `[...X[]]` is `X[]`, which needs the global `Array` types (P2).
    // port: tsc/internal/checker/checker.go:Checker.getTupleTargetType
    pub(crate) fn get_tuple_target_type(
        &mut self,
        element_infos: &[TupleElementInfo],
        readonly: bool,
    ) -> Result<TypeId, Error> {
        if element_infos.len() == 1 && element_infos[0].flags & element_flags::REST != 0 {
            return self.array_target(readonly);
        }
        let key = tuple_key(element_infos, readonly);
        if let Some(t) = self.types.caches.tuple_types.get(&key) {
            return Ok(*t);
        }
        let t = self.create_tuple_target_type(element_infos, readonly)?;
        self.types.caches.tuple_types.insert(key, t);
        Ok(t)
    }

    /// Tuple types are references to a synthesized generic interface
    /// `Tuple<T0, T1, ...> extends Array<T0 | T1 | ...> { 0: T0, 1: T1, ... }`
    /// with no symbol; its type parameters have none either.
    // port: tsc/internal/checker/checker.go:Checker.createTupleTargetType
    pub(crate) fn create_tuple_target_type(
        &mut self,
        element_infos: &[TupleElementInfo],
        readonly: bool,
    ) -> Result<TypeId, Error> {
        let arity = element_infos.len();
        let min_length = element_infos
            .iter()
            .filter(|e| e.flags & (element_flags::REQUIRED | element_flags::VARIADIC) != 0)
            .count();
        let mut type_parameters: Vec<TypeId> = Vec::new();
        let mut members: HashMap<JsString, Option<SymbolId>> = HashMap::new();
        let mut combined_flags: ElementFlags = element_flags::NONE;
        let readonly_flags = if readonly { check_flags::READONLY } else { 0 };
        if arity != 0 {
            type_parameters.reserve_exact(arity);
            for (i, info) in element_infos.iter().enumerate() {
                let type_parameter = self.new_type_parameter(None)?;
                type_parameters.push(type_parameter);
                let flags = info.flags;
                combined_flags |= flags;
                if combined_flags & element_flags::VARIABLE == 0 {
                    let optional = if flags & element_flags::OPTIONAL != 0 {
                        symbol_flags::OPTIONAL
                    } else {
                        0
                    };
                    let property = self.new_symbol_ex(
                        symbol_flags::PROPERTY | optional,
                        JsString::from_bytes(i.to_string().into_bytes()),
                        readonly_flags,
                    )?;
                    self.value_symbol_links
                        .get_or_default(property)
                        .resolved_type = Some(type_parameter);
                    let name = self.symbol(property)?.name_to_owned();
                    members.insert(name, Some(property));
                }
            }
        }
        let fixed_length = members.len();
        let length_symbol = self.new_symbol_ex(
            symbol_flags::PROPERTY,
            JsString::from_bytes(&b"length"[..]),
            readonly_flags,
        )?;
        if combined_flags & element_flags::VARIABLE != 0 {
            self.value_symbol_links
                .get_or_default(length_symbol)
                .resolved_type = Some(self.builtins.number_type);
        } else {
            let mut literal_types = Vec::with_capacity(arity + 1 - min_length);
            for i in min_length..=arity {
                literal_types.push(self.get_number_literal_type(Number::new(i as f64))?);
            }
            let length = self.get_union_type(&literal_types)?;
            self.value_symbol_links
                .get_or_default(length_symbol)
                .resolved_type = Some(length);
        }
        members.insert(JsString::from_bytes(&b"length"[..]), Some(length_symbol));
        let t = self.new_object_type(object_flags::TUPLE | object_flags::REFERENCE, None)?;
        let this_type = self.new_type_parameter(None)?;
        {
            let this = self.types.type_parameter_mut(this_type)?;
            this.is_this_type = true;
            this.constraint = Some(t);
        }
        let mut all_type_parameters = type_parameters.clone();
        all_type_parameters.push(this_type);
        let declared_members = self.alloc_symbol_table(members);
        let type_parameter_list: TypeList = Arc::from(type_parameters);
        let mut instantiations = crate::types::Map::default();
        instantiations.insert(type_list_key(&type_parameter_list), t);
        let data = self.types.tuple_mut(t)?;
        data.interface.this_type = Some(this_type);
        data.interface.all_type_parameters = Some(Arc::from(all_type_parameters));
        data.interface.reference.object.instantiations = Some(Box::new(instantiations));
        data.interface.reference.object.target = Some(t);
        data.interface.reference.resolved_type_arguments = Some(type_parameter_list);
        data.interface.declared_members_resolved = true;
        data.interface.declared_members = Some(declared_members);
        data.element_infos = Arc::from(element_infos);
        data.min_length = min_length as u32;
        data.fixed_length = fixed_length as u32;
        data.combined_flags = combined_flags;
        data.readonly = readonly;
        Ok(t)
    }

    // port: tsc/internal/checker/checker.go:Checker.createNormalizedTypeReference
    pub(crate) fn create_normalized_type_reference(
        &mut self,
        target: TypeId,
        type_arguments: &[TypeId],
    ) -> Result<TypeId, Error> {
        if self.types.object_flags(target)? & object_flags::TUPLE != 0 {
            return self.create_normalized_tuple_type(target, type_arguments);
        }
        self.create_type_reference(target, type_arguments)
    }

    // port: tsc/internal/checker/checker.go:Checker.createNormalizedTupleType
    pub(crate) fn create_normalized_tuple_type(
        &mut self,
        target: TypeId,
        element_types: &[TypeId],
    ) -> Result<TypeId, Error> {
        self.create_normalized_tuple_type_ex(target, element_types, object_flags::NONE)
    }

    /// Optional, rest and variadic elements need the tuple normalizer and the
    /// cross-product union check (P3); required-only tuples do not.
    // port: tsc/internal/checker/checker.go:Checker.createNormalizedTupleTypeEx
    pub(crate) fn create_normalized_tuple_type_ex(
        &mut self,
        target: TypeId,
        element_types: &[TypeId],
        object_flags: ObjectFlags,
    ) -> Result<TypeId, Error> {
        let combined_flags = self.types.tuple(target)?.combined_flags;
        if combined_flags & element_flags::NON_REQUIRED == 0 {
            // No need to normalize when we only have regular required elements.
            return self.create_type_reference_ex(target, element_types, object_flags);
        }
        self.normalize_tuple(target, element_types, object_flags)
    }

    // port: tsc/internal/checker/checker.go:Checker.createTypeReference
    pub(crate) fn create_type_reference(
        &mut self,
        target: TypeId,
        type_arguments: &[TypeId],
    ) -> Result<TypeId, Error> {
        self.create_type_reference_ex(target, type_arguments, object_flags::NONE)
    }

    // port: tsc/internal/checker/checker.go:Checker.createTypeReferenceEx
    pub(crate) fn create_type_reference_ex(
        &mut self,
        target: TypeId,
        type_arguments: &[TypeId],
        object_flags: ObjectFlags,
    ) -> Result<TypeId, Error> {
        let id = type_list_key(type_arguments);
        if let Some(existing) = self
            .types
            .interface(target)?
            .reference
            .object
            .instantiations
            .as_ref()
            .and_then(|map| map.get(&id))
        {
            return Ok(*existing);
        }
        let symbol = self.types.get(target)?.symbol;
        let propagating = self.get_propagating_flags_of_types(type_arguments, type_flags::NONE)?;
        let t =
            self.new_object_type(object_flags::REFERENCE | object_flags | propagating, symbol)?;
        {
            let data = self.types.type_reference_mut(t)?;
            data.object.target = Some(target);
            data.resolved_type_arguments = Some(Arc::from(type_arguments));
        }
        self.types
            .interface_mut(target)?
            .reference
            .object
            .instantiations
            .as_mut()
            .ok_or(Error::MissingLink("InterfaceType.instantiations"))?
            .insert(id, t);
        Ok(t)
    }

    /// A synthetic signature whose declaration is a checker-created
    /// `() => any` function type node.
    // port: tsc/internal/checker/checker.go:Checker.newCallSignature
    pub(crate) fn new_call_signature(
        &mut self,
        type_parameters: Option<TypeList>,
        this_parameter: Option<SymbolId>,
        parameters: Option<SymbolList>,
        return_type: TypeId,
    ) -> Result<SignatureId, Error> {
        let any = self
            .factory
            .new_keyword_type_node(SyntaxKind::AnyKeyword.into());
        let declaration = self.factory.new_function_type_node(None, None, Some(any));
        let parameter_count = parameters.as_ref().map_or(0, |list| list.len());
        self.signatures.new_signature(
            0,
            Some(declaration),
            type_parameters,
            this_parameter,
            parameters,
            Some(return_type),
            None,
            i32::try_from(parameter_count).map_err(|_| Error::IdExhausted)?,
        )
    }

    /// The checker-created node that embeds a type (`createSyntheticExpression`
    /// without its parent and location wiring, which needs node reads; P2).
    pub(crate) fn new_synthetic_expression(
        &mut self,
        t: TypeId,
        is_spread: bool,
        tuple_name_source: Option<NodeId>,
    ) -> Result<NodeId, Error> {
        self.types.get(t)?;
        let node = self.factory.new_synthetic_expression_data(
            SyntaxKind::SyntheticExpression.into(),
            SyntheticExpressionData {
                is_spread,
                tuple_name_source,
            },
        );
        self.synthetic_expression_types.insert(node, t);
        Ok(node)
    }

    /// `Type.Alias()` reads for cache keys: the alias symbol's runtime id and its
    /// arguments.
    pub(crate) fn alias_key_parts(
        &self,
        alias: Option<crate::AliasId>,
    ) -> Result<Option<(u64, TypeList)>, Error> {
        let Some(alias) = alias else {
            return Ok(None);
        };
        let alias = self.types.alias(alias)?;
        Ok(Some((
            self.symbol_runtime_id(alias.symbol)?,
            alias.type_arguments.clone(),
        )))
    }

    // port: tsc/internal/checker/checker.go:getAliasKey
    pub(crate) fn alias_key(
        &self,
        alias: Option<crate::AliasId>,
    ) -> Result<crate::CacheKey, Error> {
        let parts = self.alias_key_parts(alias)?;
        let mut builder = KeyBuilder::new();
        builder.write_alias(parts.as_ref().map(|(symbol, args)| (*symbol, &args[..])));
        Ok(builder.finish())
    }

    /// `Type.IsTupleType()`.
    // port: tsc/internal/checker/checker.go:isTupleType
    pub(crate) fn is_tuple_type(&self, t: TypeId) -> Result<bool, Error> {
        if self.types.object_flags(t)? & object_flags::REFERENCE == 0 {
            return Ok(false);
        }
        let target = self.types.target(t)?;
        Ok(self.types.object_flags(target)? & object_flags::TUPLE != 0)
    }

    pub(crate) fn kind(&self, t: TypeId) -> Result<TypeKind, Error> {
        Ok(self.types.get(t)?.kind)
    }
}

/// A reserved member name is `0xFE` followed by a character other than `@` or `#`.
// port: tsc/internal/checker/utilities.go:isReservedMemberName
pub(crate) fn is_reserved_member_name(name: &[u8]) -> bool {
    name.len() >= 2 && name[0] == 0xFE && name[1] != b'@' && name[1] != b'#'
}

// port: tsc/internal/checker/checker.go:getTupleKey
pub(crate) fn tuple_key(element_infos: &[TupleElementInfo], readonly: bool) -> crate::CacheKey {
    let mut builder = KeyBuilder::new();
    for e in element_infos {
        builder.write_byte(if e.flags & element_flags::REQUIRED != 0 {
            b'#'
        } else if e.flags & element_flags::OPTIONAL != 0 {
            b'?'
        } else if e.flags & element_flags::REST != 0 {
            b'.'
        } else {
            b'*'
        });
        if e.labeled_declaration.is_some() {
            builder.write_node(e.labeled_declaration);
        }
    }
    if readonly {
        builder.write_byte(b'!');
    }
    builder.finish()
}
