//! `keyof`, deferred index identities and indexed reads. Cache keys preserve
//! source aliases and access flags; properties are resolved in source order.

use crate::{object_flags as of, type_flags as tf, CheckerState, Error, IndexInfoId, TypeId};
use ts_ast::{modifier_flags as mf, JsString, SyntaxKind as K};

pub(crate) const STRINGS_ONLY: u32 = 1;
pub(crate) const NO_INDEX_SIGNATURES: u32 = 2;
pub(crate) const NO_REDUCIBLE_CHECK: u32 = 4;

impl CheckerState {
    // port: tsc/internal/checker/checker.go:Checker.newIndexType
    pub(crate) fn new_index_type(&mut self, target: TypeId, flags: u32) -> Result<TypeId, Error> {
        self.types.new_type(
            tf::INDEX,
            of::NONE,
            crate::types::Payload::Index(crate::types::IndexData {
                resolved_base_constraint: None,
                target,
                index_flags: flags,
            }),
        )
    }

    // port: tsc/internal/checker/checker.go:Checker.isGenericReducibleType
    pub(crate) fn is_generic_reducible_type(&mut self, ty: TypeId) -> Result<bool, Error> {
        let record = *self.types.get(ty)?;
        if record.flags & tf::UNION != 0 && record.object_flags & of::CONTAINS_INTERSECTIONS != 0 {
            for &part in self.types.union(ty)?.types.clone().iter() {
                if self.is_generic_reducible_type(part)? {
                    return Ok(true);
                }
            }
        }
        if record.flags & tf::INTERSECTION != 0 {
            let instantiated = if let Some(cached) = self
                .types
                .intersection(ty)?
                .unique_literal_filled_instantiation
            {
                cached
            } else {
                let mapper = self.unique_literal_mapper()?;
                let instantiated = self.instantiate_type(ty, Some(mapper))?;
                self.types
                    .intersection_mut(ty)?
                    .unique_literal_filled_instantiation = Some(instantiated);
                instantiated
            };
            return Ok(self.get_reduced_type(instantiated)? != instantiated);
        }
        Ok(false)
    }

    // port: tsc/internal/checker/checker.go:Checker.shouldDeferIndexType
    pub(crate) fn should_defer_index(&mut self, ty: TypeId, flags: u32) -> Result<bool, Error> {
        let type_flags = self.types.flags(ty)?;
        if type_flags & tf::INSTANTIABLE_NON_PRIMITIVE != 0 || self.is_generic_tuple_type(ty)? {
            return Ok(true);
        }
        if self.is_generic_mapped_type(ty)? && self.mapped_name(ty)?.is_some() {
            return Ok(true);
        }
        if type_flags & tf::UNION != 0
            && flags & NO_REDUCIBLE_CHECK == 0
            && self.is_generic_reducible_type(ty)?
        {
            return Ok(true);
        }
        if type_flags & tf::INTERSECTION != 0 {
            let parts = self.types.intersection(ty)?.types.clone();
            let mut instantiable = false;
            let mut empty = false;
            for &part in parts.iter() {
                instantiable |= self.maybe_type_of_kind(part, tf::INSTANTIABLE)?;
                empty |= self.is_empty_anonymous_object_type(part)?;
            }
            return Ok(instantiable && empty);
        }
        Ok(false)
    }

    pub(crate) fn maybe_type_of_kind(
        &self,
        ty: TypeId,
        flags: crate::TypeFlags,
    ) -> Result<bool, Error> {
        let own = self.types.flags(ty)?;
        if own & flags != 0 {
            return Ok(true);
        }
        if own & tf::UNION_OR_INTERSECTION != 0 {
            for &part in self.types.types_of(ty)? {
                if self.maybe_type_of_kind(part, flags)? {
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }

    // port: tsc/internal/checker/checker.go:Checker.getIndexTypeEx
    pub(crate) fn get_index_type(&mut self, ty: TypeId, flags: u32) -> Result<TypeId, Error> {
        let ty = self.get_reduced_type(ty)?;
        if self.is_no_infer_type(ty)? {
            let base = self.types.substitution(ty)?.base;
            let index = self.get_index_type(base, flags)?;
            return self.no_infer_type(index);
        }
        if self.should_defer_index(ty, flags)? {
            return self.generic_index_type(ty, flags);
        }
        let record = *self.types.get(ty)?;
        if record.flags & tf::UNION_OR_INTERSECTION != 0 {
            let parts = self.types.compound_types(ty)?.clone();
            let mut indexes = Vec::new();
            for &part in parts.iter() {
                indexes.push(self.get_index_type(part, flags)?);
            }
            return if record.flags & tf::UNION != 0 {
                self.get_intersection_type(&indexes)
            } else {
                self.get_union_type(&indexes)
            };
        }
        if record.object_flags & of::MAPPED != 0 {
            return self.index_type_for_mapped(ty, flags);
        }
        if ty == self.builtins.wildcard_type {
            return Ok(ty);
        }
        if record.flags & tf::UNKNOWN != 0 {
            return Ok(self.builtins.never_type);
        }
        if record.flags & (tf::ANY | tf::NEVER) != 0 {
            return Ok(self.builtins.string_number_symbol_type);
        }
        let include = if flags & NO_INDEX_SIGNATURES != 0 {
            tf::STRING_LITERAL
        } else {
            tf::STRING_LIKE
        } | if flags & STRINGS_ONLY != 0 {
            0
        } else {
            tf::NUMBER_LIKE | tf::ES_SYMBOL_LIKE
        };
        self.literal_type_from_properties(ty, include, flags == 0)
    }

    // port: tsc/internal/checker/checker.go:Checker.getIndexTypeForGenericType
    pub(crate) fn generic_index_type(&mut self, ty: TypeId, flags: u32) -> Result<TypeId, Error> {
        let flags = flags & STRINGS_ONLY;
        if let Some(&index) = self.types.caches.index_types.get(&(ty, flags)) {
            return Ok(index);
        }
        let index = self.new_index_type(ty, flags)?;
        self.types.caches.index_types.insert((ty, flags), index);
        Ok(index)
    }

    // port: tsc/internal/checker/checker.go:Checker.getLiteralTypeFromProperties
    fn literal_type_from_properties(
        &mut self,
        ty: TypeId,
        include: crate::TypeFlags,
        include_origin: bool,
    ) -> Result<TypeId, Error> {
        let record = *self.types.get(ty)?;
        let key = (
            ty,
            include,
            include_origin,
            record.object_flags & of::UNRESOLVED_MEMBERS != 0,
        );
        if let Some(&cached) = self.types.caches.property_types.get(&key) {
            return Ok(cached);
        }
        let origin = if include_origin
            && record.object_flags & (of::CLASS_OR_INTERFACE | of::REFERENCE) != 0
            || record.alias.is_some()
        {
            Some(self.new_index_type(ty, 0)?)
        } else {
            None
        };
        let properties = self.get_properties_of_type(ty)?;
        let indexes = self.index_infos_of_type(ty)?;
        let mut types = Vec::new();
        for property in properties {
            types.push(self.literal_type_from_property(property, include)?);
        }
        for index in indexes {
            let key = self.signatures.index_info(index)?.key_type;
            if index != self.builtins.enum_number_index_info
                && self.key_type_included(key, include)?
            {
                types.push(
                    if key == self.builtins.string_type && include & tf::NUMBER != 0 {
                        self.builtins.string_or_number_type
                    } else {
                        key
                    },
                );
            }
        }
        let result =
            self.get_union_type_ex(&types, crate::UnionReduction::Literal, None, origin)?;
        self.types.caches.property_types.insert(key, result);
        Ok(result)
    }

    fn key_type_included(&self, ty: TypeId, include: crate::TypeFlags) -> Result<bool, Error> {
        if self.types.flags(ty)? & tf::INTERSECTION != 0 {
            for &part in self.types.types_of(ty)? {
                if !self.key_type_included(part, include)? {
                    return Ok(false);
                }
            }
            return Ok(true);
        }
        Ok(self.types.flags(ty)? & include != 0)
    }

    // port: tsc/internal/checker/checker.go:Checker.getLiteralTypeFromProperty
    pub(crate) fn literal_type_from_property(
        &mut self,
        symbol: ts_arena::SymbolId,
        include: crate::TypeFlags,
    ) -> Result<TypeId, Error> {
        let symbol = self.get_merged_symbol(symbol);
        let read = self.symbol(symbol)?;
        let name = read.name_to_owned();
        if let Some(declaration) = read.value_declaration() {
            let view = self.ast(declaration)?;
            if view.node(declaration)?.modifier_flags(view)? & mf::NON_PUBLIC_ACCESSIBILITY_MODIFIER
                != 0
            {
                return Ok(self.builtins.never_type);
            }
        }
        let mut ty = self
            .value_symbol_links
            .try_get(symbol)
            .and_then(|links| links.name_type);
        if ty.is_none() {
            if name.as_bytes() == ts_ast::internal_symbol_names::DEFAULT {
                ty = Some(
                    self.get_string_literal_type(JsString::from_bytes(b"default".as_slice()))?,
                );
            } else {
                if let Some(declaration) = self.symbol(symbol)?.value_declaration() {
                    let view = self.ast(declaration)?;
                    if let Some(name_node) = view.node(declaration)?.name() {
                        let kind = view.node(name_node)?.kind();
                        ty = Some(if kind == K::PrivateIdentifier {
                            self.builtins.never_type
                        } else if kind == K::NumericLiteral {
                            let literal = self.check_expression(name_node)?;
                            self.get_regular_type_of_literal_type(literal)?
                        } else if kind == K::ComputedPropertyName {
                            let ty = self.check_computed_property_name(name_node)?;
                            self.get_regular_type_of_literal_type(ty)?
                        } else {
                            let text = view.node_text(name_node)?.into_js_string();
                            self.get_string_literal_type(text)?
                        });
                    }
                }
                if ty.is_none() && !name.as_bytes().starts_with(b"\xfe@") {
                    ty = Some(self.get_string_literal_type(name)?);
                }
            }
        }
        let ty = ty.unwrap_or(self.builtins.never_type);
        Ok(if self.types.flags(ty)? & include != 0 {
            ty
        } else {
            self.builtins.never_type
        })
    }

    // port: tsc/internal/checker/checker.go:Checker.getIndexInfosOfType
    pub(crate) fn index_infos_of_type(&mut self, ty: TypeId) -> Result<Vec<IndexInfoId>, Error> {
        let ty = self.reduced_apparent_type(ty)?;
        if self.types.flags(ty)? & tf::STRUCTURED_TYPE == 0 {
            return Ok(Vec::new());
        }
        self.resolve_type_members(ty)?;
        Ok(self
            .types
            .structured(ty)?
            .index_infos
            .as_deref()
            .unwrap_or_default()
            .to_vec())
    }
}

impl CheckerState {
    // port: tsc/internal/checker/checker.go:Checker.getTypeFromIndexedAccessTypeNode
    pub(crate) fn source_indexed_access_type(
        &mut self,
        node: ts_arena::NodeId,
    ) -> Result<TypeId, Error> {
        let read = self.ast(node)?.node(node)?;
        let data = read
            .data_source()
            .as_indexed_access_type_node()
            .ok_or(Error::MissingLink("indexed type"))?;
        let object = data
            .object_type()
            .ok_or(Error::MissingLink("indexed object"))?;
        let index = data.index_type().ok_or(Error::MissingLink("indexed key"))?;
        let object = self.get_type_from_type_node(object)?;
        let index = self.get_type_from_type_node(index)?;
        let alias = self
            .alias_for_type_node(node)?
            .map(|alias| self.types.push_alias(alias))
            .transpose()?;
        self.get_indexed_access_type(object, index, 0, Some(node), alias)
    }

    // port: tsc/internal/checker/checker.go:Checker.getIndexedAccessTypeEx
    pub(crate) fn get_indexed_access_type(
        &mut self,
        object: TypeId,
        index: TypeId,
        flags: crate::AccessFlags,
        node: Option<ts_arena::NodeId>,
        alias: Option<crate::AliasId>,
    ) -> Result<TypeId, Error> {
        let result = self.indexed_access_or_undefined(object, index, flags, node, alias)?;
        Ok(result.unwrap_or(if node.is_some() {
            self.builtins.error_type
        } else {
            self.builtins.unknown_type
        }))
    }

    // port: tsc/internal/checker/checker.go:Checker.getIndexedAccessTypeOrUndefined
    pub(crate) fn indexed_access_or_undefined(
        &mut self,
        object: TypeId,
        mut index: TypeId,
        mut flags: crate::AccessFlags,
        node: Option<ts_arena::NodeId>,
        alias: Option<crate::AliasId>,
    ) -> Result<Option<TypeId>, Error> {
        use crate::access_flags as af;
        if object == self.builtins.wildcard_type || index == self.builtins.wildcard_type {
            return Ok(Some(self.builtins.wildcard_type));
        }
        let object = self.get_reduced_type(object)?;
        if self.string_index_signature_only(object)?
            && self.types.flags(index)? & tf::NULLABLE == 0
            && self.type_assignable_to_kind(index, tf::STRING | tf::NUMBER)?
        {
            index = self.builtins.string_type;
        }
        if self
            .program
            .as_ref()
            .is_some_and(|program| program.host.options().no_unchecked_indexed_access.is_true())
            && flags & af::EXPRESSION_POSITION != 0
        {
            flags |= af::INCLUDE_UNDEFINED;
        }
        if self.should_defer_indexed_access(object, index, node)? {
            if self.types.flags(object)? & tf::ANY_OR_UNKNOWN != 0 {
                return Ok(Some(object));
            }
            let parts = self.alias_key_parts(alias)?;
            let mut key = crate::key::KeyBuilder::new();
            key.write_type(object);
            key.write_type(index);
            key.write_u32(flags);
            key.write_alias(parts.as_ref().map(|(symbol, types)| (*symbol, &types[..])));
            let key = key.finish();
            if let Some(&ty) = self.types.caches.indexed_access_types.get(&key) {
                return Ok(Some(ty));
            }
            let ty = self.types.new_type(
                tf::INDEXED_ACCESS,
                of::NONE,
                crate::types::Payload::IndexedAccess(crate::types::IndexedAccessData {
                    resolved_base_constraint: None,
                    object_type: object,
                    index_type: index,
                    access_flags: flags & af::PERSISTENT,
                }),
            )?;
            self.types.get_mut(ty)?.alias = alias;
            self.types.caches.indexed_access_types.insert(key, ty);
            return Ok(Some(ty));
        }
        let apparent = self.reduced_apparent_type(object)?;
        if self.types.flags(index)? & tf::UNION != 0 && self.types.flags(index)? & tf::BOOLEAN == 0
        {
            let indexes = self.types.union(index)?.types.clone();
            let mut results = Vec::new();
            let mut missing = false;
            for &part in indexes.iter() {
                match self.property_for_index(
                    object,
                    apparent,
                    part,
                    index,
                    node,
                    flags
                        | if missing {
                            af::SUPPRESS_NO_IMPLICIT_ANY_ERROR
                        } else {
                            0
                        },
                )? {
                    Some(result) => results.push(result),
                    None if node.is_none() => return Ok(None),
                    None => missing = true,
                }
            }
            if missing {
                return Ok(None);
            }
            return if flags & af::WRITING != 0 {
                self.get_intersection_type_ex(&results, 0, alias).map(Some)
            } else {
                self.get_union_type_ex(&results, crate::UnionReduction::Literal, alias, None)
                    .map(Some)
            };
        }
        self.property_for_index(
            object,
            apparent,
            index,
            index,
            node,
            flags | af::CACHE_SYMBOL | af::REPORT_DEPRECATED,
        )
    }

    // port: tsc/internal/checker/checker.go:Checker.shouldDeferIndexedAccessType
    fn should_defer_indexed_access(
        &mut self,
        object: TypeId,
        index: TypeId,
        node: Option<ts_arena::NodeId>,
    ) -> Result<bool, Error> {
        if self.is_generic_index_type(index)? {
            return Ok(true);
        }
        let fixed_index = self.is_tuple_type(object)?
            && self.index_less_than(index, self.total_fixed_elements(object)?)?;
        if let Some(node) = node {
            if self.ast(node)?.node(node)?.kind() != K::IndexedAccessType {
                return Ok(self.is_generic_tuple_type(object)? && !fixed_index);
            }
        }
        Ok(
            self.get_generic_object_flags(object)? & of::IS_GENERIC_OBJECT_TYPE != 0
                && !fixed_index
                || self.is_generic_reducible_type(object)?,
        )
    }

    // port: tsc/internal/checker/checker.go:getTotalFixedElementCount
    pub(crate) fn total_fixed_elements(&self, ty: TypeId) -> Result<usize, Error> {
        let tuple = self.types.tuple(self.types.target(ty)?)?;
        Ok(tuple.fixed_length as usize
            + tuple
                .element_infos
                .iter()
                .rev()
                .take_while(|info| info.flags & crate::element_flags::FIXED != 0)
                .count())
    }

    // port: tsc/internal/checker/checker.go:indexTypeLessThan
    fn index_less_than(&self, ty: TypeId, limit: usize) -> Result<bool, Error> {
        if self.types.flags(ty)? & tf::UNION != 0 {
            for &part in self.types.types_of(ty)? {
                if !self.index_less_than(part, limit)? {
                    return Ok(false);
                }
            }
            return Ok(true);
        }
        let Some(name) = self.index_property_name(ty)? else {
            return Ok(false);
        };
        Ok(numeric_name(name.as_bytes())
            .is_some_and(|number| number >= 0.0 && number < limit as f64))
    }

    // port: tsc/internal/checker/utilities.go:getPropertyNameFromType
    pub(crate) fn index_property_name(&self, ty: TypeId) -> Result<Option<JsString>, Error> {
        if self.types.flags(ty)? & tf::STRING_OR_NUMBER_LITERAL != 0 {
            return Ok(Some(match &self.types.literal(ty)?.value {
                crate::LiteralValue::String(text) => text.clone(),
                crate::LiteralValue::Number(value) => {
                    JsString::from_bytes(value.to_string().into_bytes())
                }
                _ => return Err(Error::MissingLink("property name literal")),
            }));
        }
        if self.types.flags(ty)? & tf::UNIQUE_ES_SYMBOL != 0 {
            return Ok(Some(self.types.unique_symbol(ty)?.name.clone()));
        }
        Ok(None)
    }

    // port: tsc/internal/checker/checker.go:Checker.isStringIndexSignatureOnlyTypeWorker
    fn string_index_signature_only(&mut self, ty: TypeId) -> Result<bool, Error> {
        let record = *self.types.get(ty)?;
        if record.flags & tf::OBJECT != 0
            && !self.is_generic_mapped_type(ty)?
            && self.get_properties_of_type(ty)?.is_empty()
        {
            let indexes = self.index_infos_of_type(ty)?;
            return Ok(indexes.len() == 1
                && self.signatures.index_info(indexes[0])?.key_type == self.builtins.string_type);
        }
        if record.flags & tf::UNION_OR_INTERSECTION != 0 {
            let parts = self.types.compound_types(ty)?.clone();
            for &part in parts.iter() {
                if !self.string_index_signature_only(part)? {
                    return Ok(false);
                }
            }
            return Ok(true);
        }
        Ok(false)
    }

    // port: tsc/internal/checker/checker.go:Checker.isTypeAssignableToKindEx
    pub(crate) fn type_assignable_to_kind(
        &mut self,
        ty: TypeId,
        flags: crate::TypeFlags,
    ) -> Result<bool, Error> {
        if self.types.flags(ty)? & flags != 0 {
            return Ok(true);
        }
        for (flag, target) in [
            (tf::NUMBER_LIKE, self.builtins.number_type),
            (tf::BIG_INT_LIKE, self.builtins.bigint_type),
            (tf::STRING_LIKE, self.builtins.string_type),
            (tf::BOOLEAN_LIKE, self.builtins.boolean_type),
            (tf::VOID, self.builtins.void_type),
            (tf::NEVER, self.builtins.never_type),
            (tf::NULL, self.builtins.null_type),
            (tf::UNDEFINED, self.builtins.undefined_type),
            (tf::ES_SYMBOL, self.builtins.es_symbol_type),
            (tf::NON_PRIMITIVE, self.builtins.non_primitive_type),
        ] {
            if flags & flag != 0 && self.source_type_assignable(ty, target, &mut Vec::new())? {
                return Ok(true);
            }
        }
        Ok(false)
    }

    // port: tsc/internal/checker/checker.go:Checker.isApplicableIndexType
    pub(crate) fn applicable_index_type(
        &mut self,
        source: TypeId,
        target: TypeId,
    ) -> Result<bool, Error> {
        if self.source_type_assignable(source, target, &mut Vec::new())? {
            return Ok(true);
        }
        if target == self.builtins.string_type
            && self.source_type_assignable(source, self.builtins.number_type, &mut Vec::new())?
        {
            return Ok(true);
        }
        Ok(target == self.builtins.number_type
            && (source == self.builtins.numeric_string_type
                || self.types.flags(source)? & tf::STRING_LITERAL != 0
                    && self
                        .index_property_name(source)?
                        .is_some_and(|name| numeric_name(name.as_bytes()).is_some())))
    }

    // port: tsc/internal/checker/checker.go:Checker.findApplicableIndexInfo
    pub(crate) fn applicable_index_info(
        &mut self,
        object: TypeId,
        key: TypeId,
    ) -> Result<Option<IndexInfoId>, Error> {
        let mut string_index = None;
        let mut applicable = Vec::new();
        for index in self.index_infos_of_type(object)? {
            let info = self.signatures.index_info(index)?;
            if info.key_type == self.builtins.string_type {
                string_index = Some(index);
            } else if self.applicable_index_type(key, info.key_type)? {
                applicable.push(index);
            }
        }
        match applicable.as_slice() {
            [] => Ok(
                if self.applicable_index_type(key, self.builtins.string_type)? {
                    string_index
                } else {
                    None
                },
            ),
            [index] => Ok(Some(*index)),
            _ => {
                let mut types = Vec::new();
                let mut readonly = true;
                for index in applicable {
                    let info = self.signatures.index_info(index)?;
                    types.push(info.value_type);
                    readonly &= info.is_readonly;
                }
                let value = self.get_intersection_type(&types)?;
                self.signatures
                    .new_index_info(self.builtins.unknown_type, value, readonly, None, None)
                    .map(Some)
            }
        }
    }

    // port: tsc/internal/checker/checker.go:Checker.getPropertyTypeForIndexType
    fn property_for_index(
        &mut self,
        original_object: TypeId,
        object: TypeId,
        index: TypeId,
        full_index: TypeId,
        node: Option<ts_arena::NodeId>,
        flags: crate::AccessFlags,
    ) -> Result<Option<TypeId>, Error> {
        use crate::access_flags as af;
        let expression = match node {
            Some(node) if self.ast(node)?.node(node)?.kind() == K::ElementAccessExpression => {
                Some(node)
            }
            _ => None,
        };
        let name = if node
            .map(|node| {
                self.ast(node)?
                    .node(node)
                    .map(|read| read.kind() == K::PrivateIdentifier)
                    .map_err(Error::from)
            })
            .transpose()?
            .unwrap_or(false)
        {
            None
        } else if let Some(name) = self.index_property_name(index)? {
            Some(name)
        } else if let Some(node) = node {
            if ts_ast::utilities::is_property_name(&self.ast(node)?.node(node)?) {
                let name = self.index_property_name_node(node)?;
                (name.as_bytes() != ts_ast::internal_symbol_names::MISSING).then_some(name)
            } else {
                None
            }
        } else {
            None
        };
        if let Some(name) = &name {
            if flags & af::CONTEXTUAL != 0 {
                return Ok(Some(
                    self.type_of_property_of_contextual_type(object, name.as_bytes())?
                        .unwrap_or(self.builtins.any_type),
                ));
            }
            if let Some(symbol) = self.constituent_property(object, name.as_bytes(), false)? {
                if let Some(expression) = expression {
                    let left = self
                        .ast(expression)?
                        .node(expression)?
                        .expression()
                        .ok_or(Error::MissingLink("indexed receiver"))?;
                    self.mark_access_property_referenced(symbol, expression, left)?;
                    let assignment = self.assignment_target_kind(expression)?;
                    if self.assignment_to_readonly_property(expression, symbol, assignment)? {
                        let name = self.symbol_to_string(symbol)?;
                        self.error_at(
                            self.index_access_node(node)?,
                            ts_diagnostics::Cannot_assign_to_0_because_it_is_a_read_only_property,
                            vec![name],
                        )?;
                        return Ok(None);
                    }
                    if flags & af::CACHE_SYMBOL != 0 {
                        *self.query.resolved_symbols.get_or_default(expression) = Some(symbol);
                    }
                    if self.this_property_access_in_constructor(expression, symbol)? {
                        return Ok(Some(self.builtins.auto_type));
                    }
                }
                let value = if flags & af::WRITING != 0 {
                    self.write_type_of_symbol(symbol)?
                } else {
                    self.get_type_of_symbol(symbol)?
                };
                if let Some(expression) = expression {
                    if self.assignment_target_kind(expression)?
                        != crate::flow_assignments::AssignmentKind::Definite
                    {
                        return self
                            .flow_type_of_this_reference(expression, value)
                            .map(Some);
                    }
                } else if node
                    .map(|node| {
                        self.ast(node)?
                            .node(node)
                            .map(|read| read.kind() == K::IndexedAccessType)
                            .map_err(Error::from)
                    })
                    .transpose()?
                    .unwrap_or(false)
                    && self.type_contains_missing(value)?
                {
                    return self
                        .get_union_type(&[value, self.builtins.undefined_type])
                        .map(Some);
                }
                return Ok(Some(value));
            }
            if let Some(index) = numeric_name(name.as_bytes()) {
                let parts = if self.types.flags(object)? & tf::UNION != 0 {
                    self.types.types_of(object)?.to_vec()
                } else {
                    vec![object]
                };
                let mut tuples = true;
                for &part in &parts {
                    tuples &= self.is_tuple_type(part)?;
                }
                if tuples {
                    if let Some(value) = self.tuple_index_outside_start(
                        object,
                        &parts,
                        index,
                        name.clone(),
                        node,
                        flags,
                    )? {
                        return Ok(Some(value));
                    }
                }
            }
        }
        if self.types.flags(index)? & tf::NULLABLE == 0
            && self.type_assignable_to_kind(
                index,
                tf::STRING_LIKE | tf::NUMBER_LIKE | tf::ES_SYMBOL_LIKE,
            )?
        {
            if self.types.flags(object)? & (tf::ANY | tf::NEVER) != 0 {
                return Ok(Some(object));
            }
            let applicable = self.applicable_index_info(object, index)?;
            let applicable = match applicable {
                Some(info) => Some(info),
                None => self.index_infos_of_type(object)?.into_iter().find(|&id| {
                    self.signatures
                        .index_info(id)
                        .is_ok_and(|info| info.key_type == self.builtins.string_type)
                }),
            };
            if let Some(info) = applicable {
                let info = self.signatures.index_info(info)?.clone();
                if flags & af::NO_INDEX_SIGNATURES != 0
                    && info.key_type != self.builtins.number_type
                {
                    if let Some(expression) = expression {
                        let display = self
                            .type_to_string(original_object, crate::type_display::DEFAULT_FLAGS)?;
                        if flags & af::WRITING != 0 {
                            self.error_at(Some(expression),ts_diagnostics::Type_0_is_generic_and_can_only_be_indexed_for_reading,vec![display])?;
                        } else {
                            let index =
                                self.type_to_string(index, crate::type_display::DEFAULT_FLAGS)?;
                            self.error_at(
                                Some(expression),
                                ts_diagnostics::Type_0_cannot_be_used_to_index_type_1,
                                vec![index, display],
                            )?;
                        }
                    }
                    return Ok(None);
                }
                if node.is_some()
                    && info.key_type == self.builtins.string_type
                    && !self.type_assignable_to_kind(index, tf::STRING | tf::NUMBER)?
                {
                    let text = self.type_to_string(index, crate::type_display::DEFAULT_FLAGS)?;
                    self.error_at(
                        self.index_access_node(node)?,
                        ts_diagnostics::Type_0_cannot_be_used_as_an_index_type,
                        vec![text],
                    )?;
                    return if flags & af::INCLUDE_UNDEFINED != 0 {
                        self.get_union_type(&[info.value_type, self.builtins.missing_type])
                            .map(Some)
                    } else {
                        Ok(Some(info.value_type))
                    };
                }
                self.error_writing_readonly_index(Some(info.clone()), object, expression)?;
                let enum_self_index = if let (Some(object_symbol), Some(index_symbol)) = (
                    self.types.get(object)?.symbol,
                    self.types.get(index)?.symbol,
                ) {
                    self.symbol(object_symbol)?.flags() & ts_ast::symbol_flags::ENUM != 0
                        && self.types.flags(index)? & tf::ENUM_LITERAL != 0
                        && self.parent_of_symbol(index_symbol)? == Some(object_symbol)
                } else {
                    false
                };
                return if flags & af::INCLUDE_UNDEFINED != 0 && !enum_self_index {
                    self.get_union_type(&[info.value_type, self.builtins.missing_type])
                        .map(Some)
                } else {
                    Ok(Some(info.value_type))
                };
            }
            if self.types.flags(index)? & tf::NEVER != 0 {
                return Ok(Some(self.builtins.never_type));
            }
        }
        if let Some(expression) = expression {
            let constant_enum = self
                .types
                .get(object)?
                .symbol
                .map(|symbol| {
                    self.symbol(symbol)
                        .map(|read| read.flags() & ts_ast::symbol_flags::CONST_ENUM != 0)
                })
                .transpose()?
                .unwrap_or(false);
            if !constant_enum
                && self.types.flags(index)? & tf::NULLABLE == 0
                && self.type_assignable_to_kind(
                    index,
                    tf::STRING_LIKE | tf::NUMBER_LIKE | tf::ES_SYMBOL_LIKE,
                )?
            {
                return self.missing_indexed_property(
                    expression,
                    object,
                    index,
                    full_index,
                    name.as_ref(),
                    flags,
                );
            }
        }
        if flags & af::ALLOW_MISSING != 0
            && self.types.get(object)?.object_flags & of::OBJECT_LITERAL != 0
        {
            return Ok(Some(self.builtins.undefined_type));
        }
        if let Some(node) = self.index_access_node(node)? {
            let object_text = self.type_to_string(object, crate::type_display::DEFAULT_FLAGS)?;
            let index_text = self.type_to_string(index, crate::type_display::DEFAULT_FLAGS)?;
            if self.ast(node)?.node(node)?.kind() != K::BigIntLiteral
                && self.types.flags(index)? & tf::STRING_OR_NUMBER_LITERAL != 0
            {
                self.error_at(
                    Some(node),
                    ts_diagnostics::Property_0_does_not_exist_on_type_1,
                    vec![
                        self.index_property_name(index)?
                            .ok_or(Error::MissingLink("literal index name"))?,
                        object_text,
                    ],
                )?;
            } else if self.types.flags(index)? & (tf::STRING | tf::NUMBER) != 0 {
                self.error_at(
                    Some(node),
                    ts_diagnostics::Type_0_has_no_matching_index_signature_for_type_1,
                    vec![object_text, index_text],
                )?;
            } else {
                let index_text = if self.ast(node)?.node(node)?.kind() == K::BigIntLiteral {
                    JsString::from_bytes(b"bigint".to_vec())
                } else {
                    index_text
                };
                self.error_at(
                    Some(node),
                    ts_diagnostics::Type_0_cannot_be_used_as_an_index_type,
                    vec![index_text],
                )?;
            }
        }
        Ok((self.types.flags(index)? & tf::ANY != 0).then_some(index))
    }

    pub(crate) fn index_access_node(
        &self,
        node: Option<ts_arena::NodeId>,
    ) -> Result<Option<ts_arena::NodeId>, Error> {
        node.map(|node| {
            let read = self.ast(node)?.node(node)?;
            match read.kind().known() {
                Some(K::ElementAccessExpression) => read
                    .data_source()
                    .as_element_access_expression()
                    .and_then(|data| data.argument_expression())
                    .ok_or(Error::MissingLink("element access index")),
                Some(K::IndexedAccessType) => read
                    .data_source()
                    .as_indexed_access_type_node()
                    .and_then(|data| data.index_type())
                    .ok_or(Error::MissingLink("indexed type index")),
                Some(K::ComputedPropertyName) => read
                    .expression()
                    .ok_or(Error::MissingLink("computed index expression")),
                _ => Ok(node),
            }
        })
        .transpose()
    }

    // Upstream NewChecker's containsMissingType closure (checker.go:1259).
    pub(crate) fn type_contains_missing(&self, ty: TypeId) -> Result<bool, Error> {
        Ok(ty == self.builtins.missing_type
            || self.types.flags(ty)? & tf::UNION != 0
                && self.types.types_of(ty)?.first() == Some(&self.builtins.missing_type))
    }

    // port: tsc/internal/checker/checker.go:Checker.getPropertyTypeForIndexType
    fn tuple_index_outside_start(
        &mut self,
        ty: TypeId,
        parts: &[TypeId],
        index: f64,
        name: JsString,
        node: Option<ts_arena::NodeId>,
        flags: crate::AccessFlags,
    ) -> Result<Option<TypeId>, Error> {
        let mut fixed = true;
        for &part in parts {
            fixed &= self.types.tuple(self.types.target(part)?)?.combined_flags
                & crate::element_flags::VARIABLE
                == 0;
        }
        if node.is_some() && fixed && flags & crate::access_flags::ALLOW_MISSING == 0 {
            let error_node = self.index_access_node(node)?;
            if self.is_tuple_type(ty)? {
                if index < 0.0 {
                    self.error_at(
                        error_node,
                        ts_diagnostics::A_tuple_type_cannot_be_indexed_with_a_negative_value,
                        vec![],
                    )?;
                    return Ok(Some(self.builtins.undefined_type));
                }
                let display = self.type_to_string(ty, crate::type_display::DEFAULT_FLAGS)?;
                let count = self
                    .types
                    .tuple(self.types.target(ty)?)?
                    .element_infos
                    .len();
                self.error_at(
                    error_node,
                    ts_diagnostics::Tuple_type_0_of_length_1_has_no_element_at_index_2,
                    vec![
                        display,
                        JsString::from_bytes(count.to_string().into_bytes()),
                        name,
                    ],
                )?;
            } else {
                let display = self.type_to_string(ty, crate::type_display::DEFAULT_FLAGS)?;
                self.error_at(
                    error_node,
                    ts_diagnostics::Property_0_does_not_exist_on_type_1,
                    vec![name, display],
                )?;
            }
        }
        if index < 0.0 {
            return Ok(None);
        }
        let mut numeric_info = None;
        for id in self.index_infos_of_type(ty)? {
            let info = self.signatures.index_info(id)?;
            if info.key_type == self.builtins.number_type {
                numeric_info = Some(info.clone());
                break;
            }
        }
        let expression = match node {
            Some(node) if self.ast(node)?.node(node)?.kind() == K::ElementAccessExpression => {
                Some(node)
            }
            _ => None,
        };
        self.error_writing_readonly_index(numeric_info, ty, expression)?;
        let mut values = Vec::new();
        for &part in parts {
            values.push(self.tuple_element_outside_start(part, index, flags)?);
        }
        self.get_union_type(&values).map(Some)
    }

    // port: tsc/internal/checker/checker.go:Checker.getTupleElementTypeOutOfStartCount
    fn tuple_element_outside_start(
        &mut self,
        ty: TypeId,
        index: f64,
        flags: crate::AccessFlags,
    ) -> Result<TypeId, Error> {
        use crate::element_flags as ef;
        let data = self.types.tuple(self.types.target(ty)?)?;
        let infos = data.element_infos.clone();
        let start = data.fixed_length as usize;
        let elements = self.element_types(ty)?;
        if start == elements.len() {
            return Ok(self.builtins.undefined_type);
        }
        let mut types = Vec::new();
        for position in start..elements.len() {
            let ty = if infos[position].flags & ef::VARIADIC != 0 {
                self.get_indexed_access_type(
                    elements[position],
                    self.builtins.number_type,
                    0,
                    None,
                    None,
                )?
            } else {
                elements[position]
            };
            types.push(ty);
        }
        if flags & crate::access_flags::INCLUDE_UNDEFINED != 0
            && index >= self.total_fixed_elements(ty)? as f64
        {
            types.push(self.builtins.missing_type);
        }
        self.get_union_type(&types)
    }

    // port: tsc/internal/checker/checker.go:Checker.checkIndexedAccessType
    // port: tsc/internal/checker/checker.go:Checker.checkIndexedAccessIndexType
    pub(crate) fn check_indexed_access_type(
        &mut self,
        node: ts_arena::NodeId,
    ) -> Result<(), Error> {
        for child in self.source_children(node)? {
            self.check_source_element(child)?;
        }
        let ty = self.get_type_from_type_node(node)?;
        self.check_indexed_value_type(ty, node)?;
        Ok(())
    }
}

// port: tsc/internal/checker/utilities.go:isNumericLiteralName
pub(crate) fn numeric_name(name: &[u8]) -> Option<f64> {
    let value = ts_jsnum::from_string(name);
    (value.to_string().as_bytes() == name).then_some(value.value())
}
