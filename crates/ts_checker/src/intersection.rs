//! Ordered intersection construction. Unlike union construction, constituent
//! order is part of the identity and eventual overload precedence.

use crate::key::KeyBuilder;
use crate::types::{IntersectionData, UnionOrIntersectionMembers};
use crate::{
    object_flags as of, type_flags as tf, AliasId, CheckerState, Error, Payload, TypeId,
    UnionReduction,
};
use std::collections::HashSet;
use std::sync::Arc;
use ts_ast::symbol_flags as sf;

pub(crate) const NO_SUPERTYPE_REDUCTION: u32 = 1;
pub(crate) const NO_CONSTRAINT_REDUCTION: u32 = 2;

impl CheckerState {
    // port: tsc/internal/checker/checker.go:Checker.getIntersectionType
    pub(crate) fn get_intersection_type(&mut self, types: &[TypeId]) -> Result<TypeId, Error> {
        self.get_intersection_type_ex(types, 0, None)
    }

    // port: tsc/internal/checker/checker.go:Checker.getIntersectionTypeEx
    pub(crate) fn get_intersection_type_ex(
        &mut self,
        types: &[TypeId],
        flags: u32,
        alias: Option<AliasId>,
    ) -> Result<TypeId, Error> {
        let mut set = Vec::with_capacity(types.len());
        let mut seen = HashSet::new();
        let includes = self.add_types_to_intersection(&mut set, &mut seen, 0, types)?;
        if includes & tf::NEVER != 0 {
            return Ok(if set.contains(&self.builtins.silent_never_type) {
                self.builtins.silent_never_type
            } else {
                self.builtins.never_type
            });
        }
        if self.options.strict_null_checks
            && includes & tf::NULLABLE != 0
            && includes & (tf::OBJECT | tf::NON_PRIMITIVE | tf::INCLUDES_EMPTY_OBJECT) != 0
            || [
                tf::NON_PRIMITIVE,
                tf::STRING_LIKE,
                tf::NUMBER_LIKE,
                tf::BIG_INT_LIKE,
                tf::ES_SYMBOL_LIKE,
                tf::VOID_LIKE,
            ]
            .into_iter()
            .any(|domain| {
                includes & domain != 0 && includes & (tf::DISJOINT_DOMAINS & !domain) != 0
            })
        {
            return Ok(self.builtins.never_type);
        }
        if includes & (tf::TEMPLATE_LITERAL | tf::STRING_MAPPING) != 0
            && includes & tf::STRING_LITERAL != 0
        {
            return Err(Error::Unsupported("extractRedundantTemplateLiterals"));
        }
        if includes & tf::ANY != 0 {
            return Ok(if includes & tf::INCLUDES_WILDCARD != 0 {
                self.builtins.wildcard_type
            } else if includes & tf::INCLUDES_ERROR != 0 {
                self.builtins.error_type
            } else {
                self.builtins.any_type
            });
        }
        if !self.options.strict_null_checks && includes & tf::NULLABLE != 0 {
            return Ok(if includes & tf::INCLUDES_EMPTY_OBJECT != 0 {
                self.builtins.never_type
            } else if includes & tf::UNDEFINED != 0 {
                self.builtins.undefined_type
            } else {
                self.builtins.null_type
            });
        }
        if flags & NO_SUPERTYPE_REDUCTION == 0 {
            self.remove_redundant_supertypes(&mut set, includes)?;
        }
        if includes & tf::INCLUDES_MISSING_TYPE != 0 {
            let index = set
                .iter()
                .position(|&t| t == self.builtins.undefined_type)
                .ok_or(Error::MissingLink(
                    "intersection missing/undefined representative",
                ))?;
            set[index] = self.builtins.missing_type;
        }
        if set.is_empty() {
            return Ok(self.builtins.unknown_type);
        }
        if set.len() == 1 {
            return Ok(set[0]);
        }
        let mut object_flags = of::NONE;
        if set.len() == 2 && flags & NO_CONSTRAINT_REDUCTION == 0 {
            let variable_index = usize::from(self.types.flags(set[0])? & tf::TYPE_VARIABLE == 0);
            let variable = set[variable_index];
            let primitive = set[1 - variable_index];
            if self.types.flags(variable)? & tf::TYPE_VARIABLE != 0
                && (self.types.flags(primitive)? & (tf::PRIMITIVE | tf::NON_PRIMITIVE) != 0
                    && !self.is_generic_string_like_type(primitive)?
                    || includes & tf::INCLUDES_EMPTY_OBJECT != 0)
            {
                if let Some(constraint) = self.base_constraint_of_type(variable)? {
                    let parts = if self.types.flags(constraint)? & tf::UNION != 0 {
                        self.types.compound_types(constraint)?.clone()
                    } else {
                        vec![constraint].into()
                    };
                    let mut valid = true;
                    for &part in parts.iter() {
                        valid &= self.types.flags(part)? & (tf::PRIMITIVE | tf::NON_PRIMITIVE) != 0
                            || self.is_empty_anonymous_object_type(part)?;
                    }
                    if valid {
                        if self.is_type_related_to(
                            constraint,
                            primitive,
                            crate::RelationKind::StrictSubtype,
                        )? {
                            return Ok(variable);
                        }
                        let mut some_subtype = false;
                        if self.types.flags(constraint)? & tf::UNION != 0 {
                            for &part in parts.iter() {
                                if self.is_type_related_to(
                                    part,
                                    primitive,
                                    crate::RelationKind::StrictSubtype,
                                )? {
                                    some_subtype = true;
                                    break;
                                }
                            }
                        }
                        if !some_subtype
                            && !self.is_type_related_to(
                                primitive,
                                constraint,
                                crate::RelationKind::StrictSubtype,
                            )?
                        {
                            return Ok(self.builtins.never_type);
                        }
                        object_flags = of::IS_CONSTRAINED_TYPE_VARIABLE;
                    }
                }
            }
        }
        let key = self.intersection_key(&set, flags, alias)?;
        if let Some(&result) = self.types.caches.intersection_types.get(&key) {
            return Ok(result);
        }
        let result = if includes & tf::UNION != 0 {
            if self.intersect_unions_of_primitive_types(&mut set)? {
                self.get_intersection_type_ex(&set, flags, alias)?
            } else if self.all_unions_contain_nullable(&set, tf::UNDEFINED)? {
                let mut undefined = self.builtins.undefined_type;
                for &ty in &set {
                    if self
                        .types
                        .types_of(ty)?
                        .contains(&self.builtins.missing_type)
                    {
                        undefined = self.builtins.missing_type;
                    }
                }
                self.intersect_without_nullable(&set, tf::UNDEFINED, undefined, flags, alias)?
            } else if self.all_unions_contain_nullable(&set, tf::NULL)? {
                self.intersect_without_nullable(
                    &set,
                    tf::NULL,
                    self.builtins.null_type,
                    flags,
                    alias,
                )?
            } else if set.len() >= 3 && types.len() > 2 {
                let middle = set.len() / 2;
                let left = self.get_intersection_type_ex(&set[..middle], flags, None)?;
                let right = self.get_intersection_type_ex(&set[middle..], flags, None)?;
                self.get_intersection_type_ex(&[left, right], flags, alias)?
            } else {
                self.check_cross_product_union(&set)?;
                let constituents = self.get_cross_product_intersections(&set, flags)?;
                let mut has_intersection = false;
                for &ty in &constituents {
                    has_intersection |= self.types.flags(ty)? & tf::INTERSECTION != 0;
                }
                let origin = if has_intersection
                    && self.constituent_count_of_types(&constituents)?
                        > self.constituent_count_of_types(&set)?
                {
                    Some(self.new_intersection_type(of::NONE, &set)?)
                } else {
                    None
                };
                self.get_union_type_ex(&constituents, UnionReduction::Literal, alias, origin)?
            }
        } else {
            let propagating = self.get_propagating_flags_of_types(types, tf::NULLABLE)?;
            let result = self.new_intersection_type(object_flags | propagating, &set)?;
            self.types.get_mut(result)?.alias = alias;
            result
        };
        self.types.caches.intersection_types.insert(key, result);
        Ok(result)
    }

    // port: tsc/internal/checker/checker.go:Checker.newIntersectionType
    fn new_intersection_type(&mut self, flags: u32, types: &[TypeId]) -> Result<TypeId, Error> {
        self.types.new_type(
            tf::INTERSECTION,
            flags,
            Payload::Intersection(IntersectionData {
                common: UnionOrIntersectionMembers::default(),
                types: Arc::from(types),
                resolved_apparent_type: None,
                unique_literal_filled_instantiation: None,
            }),
        )
    }

    // port: tsc/internal/checker/checker.go:getIntersectionKey
    fn intersection_key(
        &self,
        types: &[TypeId],
        flags: u32,
        alias: Option<AliasId>,
    ) -> Result<crate::CacheKey, Error> {
        let mut key = KeyBuilder::new();
        key.write_types(types);
        if flags & NO_CONSTRAINT_REDUCTION != 0 {
            key.write_byte(b'*');
        } else {
            let parts = self.alias_key_parts(alias)?;
            key.write_alias(parts.as_ref().map(|(symbol, args)| (*symbol, &args[..])));
        }
        Ok(key.finish())
    }

    // port: tsc/internal/checker/checker.go:Checker.addTypesToIntersection
    fn add_types_to_intersection(
        &mut self,
        set: &mut Vec<TypeId>,
        seen: &mut HashSet<TypeId>,
        mut includes: u32,
        types: &[TypeId],
    ) -> Result<u32, Error> {
        for &ty in types {
            let ty = self.get_regular_type_of_literal_type(ty)?;
            includes = self.add_type_to_intersection(set, seen, includes, ty)?;
        }
        Ok(includes)
    }

    // port: tsc/internal/checker/checker.go:Checker.addTypeToIntersection
    fn add_type_to_intersection(
        &mut self,
        set: &mut Vec<TypeId>,
        seen: &mut HashSet<TypeId>,
        mut includes: u32,
        mut ty: TypeId,
    ) -> Result<u32, Error> {
        let flags = self.types.flags(ty)?;
        if flags & tf::INTERSECTION != 0 {
            let types = self.types.intersection(ty)?.types.clone();
            return self.add_types_to_intersection(set, seen, includes, &types);
        }
        if self.is_empty_anonymous_object_type(ty)? {
            if includes & tf::INCLUDES_EMPTY_OBJECT == 0 {
                includes |= tf::INCLUDES_EMPTY_OBJECT;
                if seen.insert(ty) {
                    set.push(ty);
                }
            }
        } else {
            if flags & tf::ANY_OR_UNKNOWN != 0 {
                if ty == self.builtins.wildcard_type {
                    includes |= tf::INCLUDES_WILDCARD;
                }
                if self.is_error_type(ty)? {
                    includes |= tf::INCLUDES_ERROR;
                }
            } else if self.options.strict_null_checks || flags & tf::NULLABLE == 0 {
                if ty == self.builtins.missing_type {
                    includes |= tf::INCLUDES_MISSING_TYPE;
                    ty = self.builtins.undefined_type;
                }
                if seen.insert(ty) {
                    if self.types.flags(ty)? & tf::UNIT != 0 && includes & tf::UNIT != 0 {
                        includes |= tf::NON_PRIMITIVE;
                    }
                    set.push(ty);
                }
            }
            includes |= flags & tf::INCLUDES_MASK;
        }
        Ok(includes)
    }

    // port: tsc/internal/checker/checker.go:Checker.removeRedundantSupertypes
    fn remove_redundant_supertypes(
        &mut self,
        types: &mut Vec<TypeId>,
        includes: u32,
    ) -> Result<(), Error> {
        for i in (0..types.len()).rev() {
            let ty = types[i];
            let flags = self.types.flags(ty)?;
            if flags & tf::STRING != 0
                && includes & (tf::STRING_LITERAL | tf::TEMPLATE_LITERAL | tf::STRING_MAPPING) != 0
                || flags & tf::NUMBER != 0 && includes & tf::NUMBER_LITERAL != 0
                || flags & tf::BIG_INT != 0 && includes & tf::BIG_INT_LITERAL != 0
                || flags & tf::ES_SYMBOL != 0 && includes & tf::UNIQUE_ES_SYMBOL != 0
                || flags & tf::VOID != 0 && includes & tf::UNDEFINED != 0
                || includes & tf::DEFINITELY_NON_NULLABLE != 0
                    && self.is_empty_anonymous_object_type(ty)?
            {
                types.remove(i);
            }
        }
        Ok(())
    }

    // port: tsc/internal/checker/checker.go:Checker.IsEmptyAnonymousObjectType
    pub(crate) fn is_empty_anonymous_object_type(&mut self, ty: TypeId) -> Result<bool, Error> {
        let record = *self.types.get(ty)?;
        if record.object_flags & of::ANONYMOUS == 0 {
            return Ok(false);
        }
        if record.object_flags & of::MEMBERS_RESOLVED != 0 {
            let members = self.types.structured(ty)?;
            if ty != self.builtins.any_function_type
                && members.properties.as_ref().is_none_or(|v| v.is_empty())
                && members.signatures.as_ref().is_none_or(|v| v.is_empty())
                && members.index_infos.as_ref().is_none_or(|v| v.is_empty())
            {
                return Ok(true);
            }
        }
        if let Some(symbol) = record.symbol {
            if self.symbol(symbol)?.flags() & sf::TYPE_LITERAL != 0 {
                // getMembersOfSymbol resolves late-bound members first.
                return Ok(match self.members_of_symbol(symbol)? {
                    Some(table) => self.table(table)?.is_empty(),
                    None => true,
                });
            }
        }
        Ok(false)
    }

    // port: tsc/internal/checker/checker.go:Checker.intersectUnionsOfPrimitiveTypes
    fn intersect_unions_of_primitive_types(
        &mut self,
        types: &mut Vec<TypeId>,
    ) -> Result<bool, Error> {
        let mut indices = Vec::new();
        for (i, &ty) in types.iter().enumerate() {
            if self.types.get(ty)?.object_flags & of::PRIMITIVE_UNION != 0 {
                indices.push(i);
            }
        }
        if indices.len() < 2 {
            return Ok(false);
        }
        let unions: Vec<_> = indices.iter().map(|&i| types[i]).collect();
        let mut checked = HashSet::new();
        let mut result = Vec::new();
        for &union in &unions {
            let constituents = self.types.union(union)?.types.clone();
            for &ty in constituents.iter() {
                if checked.insert(ty) && self.each_union_contains(&unions, ty)? {
                    if ty == self.builtins.undefined_type
                        && result.first() == Some(&self.builtins.missing_type)
                    {
                        continue;
                    }
                    if ty == self.builtins.missing_type
                        && result.first() == Some(&self.builtins.undefined_type)
                    {
                        result[0] = ty;
                        continue;
                    }
                    self.insert_type(&mut result, ty)?;
                }
            }
        }
        let reduced =
            self.get_union_type_from_sorted_list(result, of::PRIMITIVE_UNION, None, None)?;
        types[indices[0]] = reduced;
        for &index in indices[1..].iter().rev() {
            types.remove(index);
        }
        Ok(true)
    }

    // port: tsc/internal/checker/checker.go:Checker.eachUnionContains
    fn each_union_contains(&self, unions: &[TypeId], ty: TypeId) -> Result<bool, Error> {
        for &union in unions {
            if !self.union_contains_type(union, ty)? {
                return Ok(false);
            }
        }
        Ok(true)
    }

    // port: tsc/internal/checker/checker.go:Checker.unionContainsType
    fn union_contains_type(&self, union: TypeId, ty: TypeId) -> Result<bool, Error> {
        let types = self.types.types_of(union)?;
        if self.contains_type(types, ty)? {
            return Ok(true);
        }
        if ty == self.builtins.missing_type {
            return self.contains_type(types, self.builtins.undefined_type);
        }
        if ty == self.builtins.undefined_type {
            return self.contains_type(types, self.builtins.missing_type);
        }
        let flags = self.types.flags(ty)?;
        let primitive = if flags & tf::STRING_LITERAL != 0 {
            Some(self.builtins.string_type)
        } else if flags & (tf::ENUM | tf::NUMBER_LITERAL) != 0 {
            Some(self.builtins.number_type)
        } else if flags & tf::BIG_INT_LITERAL != 0 {
            Some(self.builtins.bigint_type)
        } else if flags & tf::UNIQUE_ES_SYMBOL != 0 {
            Some(self.builtins.es_symbol_type)
        } else {
            None
        };
        match primitive {
            Some(ty) => self.contains_type(types, ty),
            None => Ok(false),
        }
    }

    fn all_unions_contain_nullable(&self, types: &[TypeId], nullable: u32) -> Result<bool, Error> {
        for &ty in types {
            if self.types.flags(ty)? & tf::UNION == 0 {
                return Ok(false);
            }
            let parts = self.types.types_of(ty)?;
            let mut found = false;
            for &part in parts.iter().take(if nullable == tf::NULL { 2 } else { 1 }) {
                found |= self.types.flags(part)? & nullable != 0;
            }
            if !found {
                return Ok(false);
            }
        }
        Ok(true)
    }

    fn intersect_without_nullable(
        &mut self,
        types: &[TypeId],
        nullable: u32,
        preserved: TypeId,
        flags: u32,
        alias: Option<AliasId>,
    ) -> Result<TypeId, Error> {
        let mut filtered = Vec::with_capacity(types.len());
        for &ty in types {
            filtered.push(self.filter_type(ty, &mut |state, t| {
                Ok(state.types.flags(t)? & nullable == 0)
            })?);
        }
        let intersection = self.get_intersection_type_ex(&filtered, flags, None)?;
        self.get_union_type_ex(
            &[intersection, preserved],
            UnionReduction::Literal,
            alias,
            None,
        )
    }

    // port: tsc/internal/checker/checker.go:Checker.getCrossProductIntersections
    fn get_cross_product_intersections(
        &mut self,
        types: &[TypeId],
        flags: u32,
    ) -> Result<Vec<TypeId>, Error> {
        let count = self.get_cross_product_union_size(types)?;
        let mut result = Vec::new();
        for i in 0..count {
            let mut constituents = types.to_vec();
            let mut n = i;
            for j in (0..types.len()).rev() {
                if self.types.flags(types[j])? & tf::UNION != 0 {
                    let parts = self.types.types_of(types[j])?;
                    constituents[j] = parts[n % parts.len()];
                    n /= parts.len();
                }
            }
            let ty = self.get_intersection_type_ex(&constituents, flags, None)?;
            if self.types.flags(ty)? & tf::NEVER == 0 {
                result.push(ty);
            }
        }
        Ok(result)
    }

    // port: tsc/internal/checker/checker.go:getConstituentCountOfTypes
    fn constituent_count_of_types(&self, types: &[TypeId]) -> Result<usize, Error> {
        let mut count = 0;
        for &ty in types {
            count += self.constituent_count(ty)?;
        }
        Ok(count)
    }

    // port: tsc/internal/checker/checker.go:getConstituentCount
    fn constituent_count(&self, ty: TypeId) -> Result<usize, Error> {
        let record = self.types.get(ty)?;
        if record.flags & tf::UNION_OR_INTERSECTION == 0 || record.alias.is_some() {
            return Ok(1);
        }
        if record.flags & tf::UNION != 0 {
            if let Some(origin) = self.types.union(ty)?.origin {
                return self.constituent_count(origin);
            }
        }
        self.constituent_count_of_types(self.types.types_of(ty)?)
    }
}
