//! Union type construction (`getUnionType` and its helpers in
//! `tsc/internal/checker/checker.go`).
//!
//! Constituents are sorted and deduplicated by the ported type comparator, so
//! `string | number` and `number | string` intern to one type. Literal reduction
//! is complete. Subtype reduction needs the relater, constrained type variables
//! need base constraints, and template-literal matching needs the string
//! matcher; each is a named failure until its P3 family lands.

use crate::key::KeyBuilder;
use crate::{
    object_flags, type_flags, AliasId, CacheKey, CheckerState, Error, ObjectFlags, Payload,
    TypeFlags, TypeId, TypeList, UnionData, UnionOfUnionKey,
};
use std::sync::Arc;

/// The per-constituent function `mapType` applies; `None` drops a constituent.
pub(crate) type MapTypeFn<'a> =
    &'a mut dyn FnMut(&mut CheckerState, TypeId) -> Result<Option<TypeId>, Error>;

/// `UnionReduction`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(i32)]
pub enum UnionReduction {
    None = 0,
    Literal,
    Subtype,
}

impl CheckerState {
    // port: tsc/internal/checker/checker.go:Checker.getUnionType
    pub(crate) fn get_union_type(&mut self, types: &[TypeId]) -> Result<TypeId, Error> {
        self.get_union_type_ex(types, UnionReduction::Literal, None, None)
    }

    /// Unioning a union with one other type is the common case and has its own
    /// cache keyed by the two ids.
    // port: tsc/internal/checker/checker.go:Checker.getUnionTypeEx
    pub(crate) fn get_union_type_ex(
        &mut self,
        types: &[TypeId],
        union_reduction: UnionReduction,
        alias: Option<AliasId>,
        origin: Option<TypeId>,
    ) -> Result<TypeId, Error> {
        if types.is_empty() {
            return Ok(self.builtins.never_type);
        }
        if types.len() == 1 {
            return Ok(types[0]);
        }
        if types.len() == 2
            && origin.is_none()
            && (self.types.flags(types[0])? & type_flags::UNION != 0
                || self.types.flags(types[1])? & type_flags::UNION != 0)
        {
            let (mut id1, mut id2) = (types[0], types[1]);
            if id1 > id2 {
                std::mem::swap(&mut id1, &mut id2);
            }
            let key = UnionOfUnionKey {
                id1,
                id2,
                reduction: union_reduction,
                alias: self.alias_key(alias)?,
            };
            if let Some(t) = self.types.caches.union_of_union_types.get(&key) {
                return Ok(*t);
            }
            let t = self.get_union_type_worker(types, union_reduction, alias, None)?;
            self.types.caches.union_of_union_types.insert(key, t);
            return Ok(t);
        }
        self.get_union_type_worker(types, union_reduction, alias, origin)
    }

    // port: tsc/internal/checker/checker.go:Checker.getUnionTypeWorker
    fn get_union_type_worker(
        &mut self,
        types: &[TypeId],
        union_reduction: UnionReduction,
        alias: Option<AliasId>,
        mut origin: Option<TypeId>,
    ) -> Result<TypeId, Error> {
        let (mut type_set, includes) = self.add_types_to_union(types)?;
        if union_reduction != UnionReduction::None {
            if includes & type_flags::ANY_OR_UNKNOWN != 0 {
                if includes & type_flags::ANY != 0 {
                    if includes & type_flags::INCLUDES_WILDCARD != 0 {
                        return Ok(self.builtins.wildcard_type);
                    }
                    if includes & type_flags::INCLUDES_ERROR != 0 {
                        return Ok(self.builtins.error_type);
                    }
                    return Ok(self.builtins.any_type);
                }
                return Ok(self.builtins.unknown_type);
            }
            if includes & type_flags::UNDEFINED != 0 {
                // If the type set contains both undefinedType and missingType, remove missingType.
                if type_set.len() >= 2
                    && type_set[0] == self.builtins.undefined_type
                    && type_set[1] == self.builtins.missing_type
                {
                    type_set.remove(1);
                }
            }
            if includes
                & (type_flags::ENUM
                    | type_flags::LITERAL
                    | type_flags::UNIQUE_ES_SYMBOL
                    | type_flags::TEMPLATE_LITERAL
                    | type_flags::STRING_MAPPING)
                != 0
                || includes & type_flags::VOID != 0 && includes & type_flags::UNDEFINED != 0
            {
                type_set = self.remove_redundant_literal_types(
                    type_set,
                    includes,
                    union_reduction == UnionReduction::Subtype,
                )?;
            }
            if includes & type_flags::STRING_LITERAL != 0
                && includes & (type_flags::TEMPLATE_LITERAL | type_flags::STRING_MAPPING) != 0
            {
                type_set = self.remove_matched_string_literals(type_set)?;
            }
            if includes & type_flags::INCLUDES_CONSTRAINED_TYPE_VARIABLE != 0 {
                type_set = self.remove_constrained_variables(type_set)?;
            }
            if union_reduction == UnionReduction::Subtype {
                let Some(reduced) =
                    self.remove_subtypes(type_set, includes & type_flags::OBJECT != 0)?
                else {
                    return Ok(self.builtins.error_type);
                };
                type_set = reduced;
            }
            if type_set.is_empty() {
                if includes & type_flags::NULL != 0 {
                    return Ok(if includes & type_flags::INCLUDES_NON_WIDENING_TYPE != 0 {
                        self.builtins.null_type
                    } else {
                        self.builtins.null_widening_type
                    });
                }
                if includes & type_flags::UNDEFINED != 0 {
                    return Ok(if includes & type_flags::INCLUDES_NON_WIDENING_TYPE != 0 {
                        self.builtins.undefined_type
                    } else {
                        self.builtins.undefined_widening_type
                    });
                }
                return Ok(self.builtins.never_type);
            }
        }
        if origin.is_none() && includes & type_flags::UNION != 0 {
            let named_unions = self.add_named_unions(Vec::new(), types)?;
            let mut reduced_types = Vec::new();
            for t in &type_set {
                let mut in_named = false;
                for u in &named_unions {
                    let constituents = self.types.types_of(*u)?;
                    if self.contains_type(constituents, *t)? {
                        in_named = true;
                        break;
                    }
                }
                if !in_named {
                    reduced_types.push(*t);
                }
            }
            if alias.is_none() && named_unions.len() == 1 && reduced_types.is_empty() {
                return Ok(named_unions[0]);
            }
            // We create a denormalized origin type only when the union was created from one or more named unions
            // (unions with alias symbols or origins) and when there is no overlap between those named unions.
            let mut named_types_count = 0;
            for u in &named_unions {
                named_types_count += self.types.types_of(*u)?.len();
            }
            if named_types_count + reduced_types.len() == type_set.len() {
                for t in &named_unions {
                    self.insert_type(&mut reduced_types, *t)?;
                }
                origin = Some(self.new_union_type(object_flags::NONE, Arc::from(reduced_types))?);
            }
        }
        let object_flags = if includes & type_flags::NOT_PRIMITIVE_UNION != 0 {
            object_flags::NONE
        } else {
            object_flags::PRIMITIVE_UNION
        } | if includes & type_flags::INTERSECTION != 0 {
            object_flags::CONTAINS_INTERSECTIONS
        } else {
            object_flags::NONE
        };
        self.get_union_type_from_sorted_list(type_set, object_flags, alias, origin)
    }

    /// Flattens nested unions, drops `never`, records what the set includes and
    /// returns the constituents sorted and deduplicated.
    // port: tsc/internal/checker/checker.go:Checker.addTypesToUnion
    fn add_types_to_union(
        &mut self,
        source_types: &[TypeId],
    ) -> Result<(Vec<TypeId>, TypeFlags), Error> {
        let mut types: Vec<TypeId> = Vec::with_capacity(source_types.len());
        let mut includes: TypeFlags = 0;
        let add_type = |state: &Self,
                        types: &mut Vec<TypeId>,
                        includes: &mut TypeFlags,
                        t: TypeId|
         -> Result<(), Error> {
            let record = *state.types.get(t)?;
            let flags = record.flags;
            // We ignore 'never' types in unions.
            if flags & type_flags::NEVER != 0 {
                return Ok(());
            }
            *includes |= flags & type_flags::INCLUDES_MASK;
            if flags & type_flags::INSTANTIABLE != 0 {
                *includes |= type_flags::INCLUDES_INSTANTIABLE;
            }
            if flags & type_flags::INTERSECTION != 0
                && record.object_flags & object_flags::IS_CONSTRAINED_TYPE_VARIABLE != 0
            {
                *includes |= type_flags::INCLUDES_CONSTRAINED_TYPE_VARIABLE;
            }
            if t == state.builtins.wildcard_type {
                *includes |= type_flags::INCLUDES_WILDCARD;
            }
            if state.is_error_type(t)? {
                *includes |= type_flags::INCLUDES_ERROR;
            }
            if !state.options.strict_null_checks && flags & type_flags::NULLABLE != 0 {
                if record.object_flags & object_flags::CONTAINS_WIDENING_TYPE == 0 {
                    *includes |= type_flags::INCLUDES_NON_WIDENING_TYPE;
                }
                return Ok(());
            }
            types.push(t);
            Ok(())
        };
        let mut last_type: Option<TypeId> = None;
        for t in source_types {
            if Some(*t) != last_type {
                if self.types.flags(*t)? & type_flags::UNION != 0 {
                    let union = self.types.union(*t)?;
                    if self.types.get(*t)?.alias.is_some() || union.origin.is_some() {
                        includes |= type_flags::UNION;
                    }
                    let constituents = union.types.clone();
                    for s in constituents.iter() {
                        add_type(self, &mut types, &mut includes, *s)?;
                    }
                } else {
                    add_type(self, &mut types, &mut includes, *t)?;
                }
                last_type = Some(*t);
            }
        }
        if types.len() >= 2 {
            // Sort and deduplicate types.
            self.sort_types(&mut types)?;
            let mut unique = 1;
            for index in 1..types.len() {
                if types[index] != types[unique - 1] {
                    types[unique] = types[index];
                    unique += 1;
                }
            }
            types.truncate(unique);
        }
        Ok((types, includes))
    }

    // port: tsc/internal/checker/checker.go:Checker.removeRedundantLiteralTypes
    fn remove_redundant_literal_types(
        &self,
        mut types: Vec<TypeId>,
        includes: TypeFlags,
        reduce_void_undefined: bool,
    ) -> Result<Vec<TypeId>, Error> {
        let mut i = types.len();
        while i > 0 {
            i -= 1;
            let t = types[i];
            let flags = self.types.flags(t)?;
            let remove = flags
                & (type_flags::STRING_LITERAL
                    | type_flags::TEMPLATE_LITERAL
                    | type_flags::STRING_MAPPING)
                != 0
                && includes & type_flags::STRING != 0
                || flags & type_flags::NUMBER_LITERAL != 0 && includes & type_flags::NUMBER != 0
                || flags & type_flags::BIG_INT_LITERAL != 0 && includes & type_flags::BIG_INT != 0
                || flags & type_flags::UNIQUE_ES_SYMBOL != 0
                    && includes & type_flags::ES_SYMBOL != 0
                || reduce_void_undefined
                    && flags & type_flags::UNDEFINED != 0
                    && includes & type_flags::VOID != 0
                || self.is_fresh_literal_type(t)?
                    && self.contains_type(&types, self.types.literal(t)?.regular)?;
            if remove {
                types.remove(i);
            }
        }
        Ok(types)
    }

    /// This function assumes the constituent type list is sorted and deduplicated.
    // port: tsc/internal/checker/checker.go:Checker.getUnionTypeFromSortedList
    pub(crate) fn get_union_type_from_sorted_list(
        &mut self,
        types: Vec<TypeId>,
        precomputed_object_flags: ObjectFlags,
        alias: Option<AliasId>,
        origin: Option<TypeId>,
    ) -> Result<TypeId, Error> {
        if types.is_empty() {
            return Ok(self.builtins.never_type);
        }
        if types.len() == 1 {
            return Ok(types[0]);
        }
        let key = self.union_key(&types, origin, alias)?;
        if let Some(t) = self.types.caches.union_types.get(&key) {
            return Ok(*t);
        }
        let propagating = self.get_propagating_flags_of_types(&types, type_flags::NULLABLE)?;
        let both_boolean_literals = types.len() == 2
            && self.types.flags(types[0])? & type_flags::BOOLEAN_LITERAL != 0
            && self.types.flags(types[1])? & type_flags::BOOLEAN_LITERAL != 0;
        let t = self.new_union_type(precomputed_object_flags | propagating, Arc::from(types))?;
        self.types.union_mut(t)?.origin = origin;
        self.types.get_mut(t)?.alias = alias;
        if both_boolean_literals {
            self.types.get_mut(t)?.flags |= type_flags::BOOLEAN;
        }
        self.types.caches.union_types.insert(key, t);
        Ok(t)
    }

    // port: tsc/internal/checker/checker.go:Checker.newUnionType
    pub(crate) fn new_union_type(
        &mut self,
        object_flags: ObjectFlags,
        types: TypeList,
    ) -> Result<TypeId, Error> {
        self.types.new_type(
            type_flags::UNION,
            object_flags,
            Payload::Union(UnionData {
                common: crate::UnionOrIntersectionMembers::default(),
                types,
                resolved_reduced_type: None,
                regular_type: None,
                origin: None,
                key_property_name: None,
                constituent_map: None,
            }),
        )
    }

    // port: tsc/internal/checker/checker.go:Checker.addNamedUnions
    fn add_named_unions(
        &self,
        mut named_unions: Vec<TypeId>,
        types: &[TypeId],
    ) -> Result<Vec<TypeId>, Error> {
        for t in types {
            if self.types.flags(*t)? & type_flags::UNION != 0 {
                let origin = self.types.union(*t)?.origin;
                let origin_is_union = match origin {
                    Some(origin) => self.types.flags(origin)? & type_flags::UNION != 0,
                    None => false,
                };
                if self.types.get(*t)?.alias.is_some() || origin.is_some() && !origin_is_union {
                    if !named_unions.contains(t) {
                        named_unions.push(*t);
                    }
                } else if let Some(origin) = origin.filter(|_| origin_is_union) {
                    let constituents = self.types.types_of(origin)?.to_vec();
                    named_unions = self.add_named_unions(named_unions, &constituents)?;
                }
            }
        }
        Ok(named_unions)
    }

    /// Propagates the flags that matter at any depth of a containing type: the
    /// presence of `undefined`, `null`, object literals and non-inferrable types.
    // port: tsc/internal/checker/checker.go:Checker.getPropagatingFlagsOfTypes
    pub(crate) fn get_propagating_flags_of_types(
        &self,
        types: &[TypeId],
        exclude_kinds: TypeFlags,
    ) -> Result<ObjectFlags, Error> {
        let mut result = object_flags::NONE;
        for t in types {
            let record = self.types.get(*t)?;
            if record.flags & exclude_kinds == 0 {
                result |= record.object_flags;
            }
        }
        Ok(result & object_flags::PROPAGATING_FLAGS)
    }

    /// The only `any` types with alias symbols are those made for references to
    /// unresolved symbols; they behave like the error type.
    // port: tsc/internal/checker/checker.go:Checker.isErrorType
    pub(crate) fn is_error_type(&self, t: TypeId) -> Result<bool, Error> {
        if t == self.builtins.error_type {
            return Ok(true);
        }
        let record = self.types.get(t)?;
        Ok(record.flags & type_flags::ANY != 0 && record.alias.is_some())
    }

    /// Inserts into a sorted list unless present; returns whether it was added.
    // port: tsc/internal/checker/checker.go:insertType
    pub(crate) fn insert_type(&self, types: &mut Vec<TypeId>, t: TypeId) -> Result<bool, Error> {
        match self.binary_search(types, t)? {
            Ok(_) => Ok(false),
            Err(index) => {
                types.insert(index, t);
                Ok(true)
            }
        }
    }

    // port: tsc/internal/checker/checker.go:containsType
    pub(crate) fn contains_type(&self, types: &[TypeId], t: TypeId) -> Result<bool, Error> {
        Ok(self.binary_search(types, t)?.is_ok())
    }

    /// `slices.BinarySearchFunc` with the fallible comparator.
    fn binary_search(&self, types: &[TypeId], t: TypeId) -> Result<Result<usize, usize>, Error> {
        let (mut low, mut high) = (0usize, types.len());
        while low < high {
            let mid = low + (high - low) / 2;
            match self.compare_types(types[mid], t)? {
                std::cmp::Ordering::Less => low = mid + 1,
                std::cmp::Ordering::Greater => high = mid,
                std::cmp::Ordering::Equal => return Ok(Ok(mid)),
            }
        }
        Ok(Err(low))
    }

    // port: tsc/internal/checker/checker.go:getUnionKey
    pub(crate) fn union_key(
        &self,
        types: &[TypeId],
        origin: Option<TypeId>,
        alias: Option<AliasId>,
    ) -> Result<CacheKey, Error> {
        let mut builder = KeyBuilder::new();
        match origin {
            None => builder.write_types(types),
            Some(origin) => {
                let flags = self.types.flags(origin)?;
                if flags & type_flags::UNION != 0 {
                    builder.write_byte(b'|');
                    builder.write_types(self.types.types_of(origin)?);
                } else if flags & type_flags::INTERSECTION != 0 {
                    builder.write_byte(b'&');
                    builder.write_types(self.types.types_of(origin)?);
                } else if flags & type_flags::INDEX != 0 {
                    // origin type id alone is insufficient, as `keyof x` may resolve to multiple WIP values while `x` is still resolving
                    builder.write_byte(b'#');
                    builder.write_type(origin);
                    builder.write_byte(b'|');
                    builder.write_types(types);
                } else {
                    return Err(Error::Unsupported("getUnionKey: unhandled origin"));
                }
            }
        }
        let alias_parts = self.alias_key_parts(alias)?;
        builder.write_alias(
            alias_parts
                .as_ref()
                .map(|(symbol, args)| (*symbol, &args[..])),
        );
        Ok(builder.finish())
    }

    /// Filters constituents while preserving a denormalized origin when the
    /// removed constituents can be represented there without changing meaning.
    // port: tsc/internal/checker/checker.go:Checker.filterType
    pub(crate) fn filter_type(
        &mut self,
        ty: TypeId,
        predicate: &mut dyn FnMut(&mut Self, TypeId) -> Result<bool, Error>,
    ) -> Result<TypeId, Error> {
        let record = *self.types.get(ty)?;
        if record.flags & type_flags::UNION != 0 {
            let types = self.types.union(ty)?.types.clone();
            let mut filtered = Vec::new();
            for &part in types.iter() {
                if predicate(self, part)? {
                    filtered.push(part);
                }
            }
            if filtered.len() == types.len() {
                return Ok(ty);
            }
            let mut new_origin = None;
            if let Some(origin) = self.types.union(ty)?.origin {
                if self.types.flags(origin)? & type_flags::UNION != 0 {
                    let origin_types = self.types.union(origin)?.types.clone();
                    let mut origin_filtered = Vec::new();
                    for &part in origin_types.iter() {
                        if self.types.flags(part)? & type_flags::UNION != 0
                            || predicate(self, part)?
                        {
                            origin_filtered.push(part);
                        }
                    }
                    if origin_types.len() - origin_filtered.len() == types.len() - filtered.len() {
                        if origin_filtered.len() == 1 {
                            return Ok(origin_filtered[0]);
                        }
                        new_origin =
                            Some(self.new_union_type(object_flags::NONE, origin_filtered.into())?);
                    }
                }
            }
            return self.get_union_type_from_sorted_list(
                filtered,
                record.object_flags
                    & (object_flags::PRIMITIVE_UNION | object_flags::CONTAINS_INTERSECTIONS),
                None,
                new_origin,
            );
        }
        if record.flags & type_flags::NEVER != 0 || predicate(self, ty)? {
            return Ok(ty);
        }
        Ok(self.builtins.never_type)
    }

    /// Applies `f` to each constituent of a union (or to a non-union type) and
    /// unions the results; `None` from `f` drops a constituent.
    // port: tsc/internal/checker/checker.go:Checker.mapType
    pub(crate) fn map_type(
        &mut self,
        t: TypeId,
        f: MapTypeFn<'_>,
    ) -> Result<Option<TypeId>, Error> {
        self.map_type_ex(t, f, false)
    }

    // port: tsc/internal/checker/checker.go:Checker.mapTypeEx
    pub(crate) fn map_type_ex(
        &mut self,
        t: TypeId,
        f: MapTypeFn<'_>,
        no_reductions: bool,
    ) -> Result<Option<TypeId>, Error> {
        let flags = self.types.flags(t)?;
        if flags & type_flags::NEVER != 0 {
            return Ok(Some(t));
        }
        if flags & type_flags::UNION == 0 {
            return f(self, t);
        }
        let union = self.types.union(t)?;
        let mut types = union.types.clone();
        if let Some(origin) = union.origin {
            if self.types.flags(origin)? & type_flags::UNION != 0 {
                types = Arc::from(self.types.types_of(origin)?);
            }
        }
        let mut mapped_types: Vec<TypeId> = Vec::with_capacity(16);
        let mut changed = false;
        for s in types.iter() {
            let mapped = if self.types.flags(*s)? & type_flags::UNION != 0 {
                self.map_type_ex(*s, f, no_reductions)?
            } else {
                f(self, *s)?
            };
            if mapped != Some(*s) {
                changed = true;
            }
            if let Some(mapped) = mapped {
                mapped_types.push(mapped);
            }
        }
        if changed {
            if mapped_types.is_empty() {
                return Ok(None);
            }
            let reduction = if no_reductions {
                UnionReduction::None
            } else {
                UnionReduction::Literal
            };
            return self
                .get_union_type_ex(&mapped_types, reduction, None, None)
                .map(Some);
        }
        Ok(Some(t))
    }
}
