//! Discriminant-property caches and the bounded cartesian-product relation.
use crate::{
    object_flags as of,
    relater::{Relater, RelationKind, BOTH},
    ternary as tr, type_flags as tf, CheckerState, Error, Ternary, TypeId, UnionReduction,
};
use ts_arena::SymbolId;
use ts_ast::{check_flags as cf, JsString};

impl CheckerState {
    // port: tsc/internal/checker/relater.go:Checker.findDiscriminantProperties
    fn discriminant_properties(
        &mut self,
        source: TypeId,
        target: TypeId,
    ) -> Result<Vec<SymbolId>, Error> {
        let mut result = Vec::new();
        for property in self.get_properties_of_type(source)? {
            let name = self.symbol(property)?.name_to_owned();
            if self.discriminant_property(target, name.as_bytes())? {
                result.push(property);
            }
        }
        Ok(result)
    }

    // port: tsc/internal/checker/relater.go:Checker.isDiscriminantProperty
    pub(crate) fn discriminant_property(&mut self, ty: TypeId, name: &[u8]) -> Result<bool, Error> {
        if self.types.flags(ty)? & tf::UNION == 0 {
            return Ok(false);
        }
        let Some(property) = self.constituent_property(ty, name, false)? else {
            return Ok(false);
        };
        let flags = self.symbol(property)?.check_flags();
        if flags & cf::SYNTHETIC_PROPERTY == 0 {
            return Ok(false);
        }
        if flags & cf::IS_DISCRIMINANT_COMPUTED == 0 {
            let mut computed = cf::IS_DISCRIMINANT_COMPUTED;
            if flags & cf::NON_UNIFORM_AND_LITERAL == cf::NON_UNIFORM_AND_LITERAL {
                let ty = self.get_type_of_symbol(property)?;
                if self.get_generic_object_flags(ty)? & of::IS_GENERIC_TYPE == 0 {
                    computed |= cf::IS_DISCRIMINANT;
                }
            }
            self.symbol_mut(property)?.check_flags |= computed;
        }
        Ok(self.symbol(property)?.check_flags() & cf::IS_DISCRIMINANT != 0)
    }

    pub(crate) fn property_or_index_type(
        &mut self,
        ty: TypeId,
        name: &[u8],
    ) -> Result<Option<TypeId>, Error> {
        if let Some(property) = self.constituent_property(ty, name, false)? {
            return self.get_type_of_symbol(property).map(Some);
        }
        let key = self.get_string_literal_type(JsString::from_bytes(name))?;
        if let Some(info) = self.applicable_index_info(ty, key)? {
            return self
                .add_type_optionality(self.signatures.index_info(info)?.value_type, true, true)
                .map(Some);
        }
        Ok(None)
    }
}

impl Relater<'_> {
    // port: tsc/internal/checker/relater.go:Checker.findMatchingDiscriminantType
    pub(crate) fn matching_discriminant_type(
        &mut self,
        source: TypeId,
        target: TypeId,
    ) -> Result<Option<TypeId>, Error> {
        if self.checker.types.flags(target)? & tf::UNION == 0
            || self.checker.types.flags(source)? & (tf::OBJECT | tf::INTERSECTION) == 0
        {
            return Ok(None);
        }
        if let Some(matching) = self.checker.matching_union_constituent(target, source)? {
            return Ok(Some(matching));
        }
        let properties = self.checker.discriminant_properties(source, target)?;
        if properties.is_empty() {
            return Ok(None);
        }
        let types = self.checker.types.compound_types(target)?.clone();
        let mut include = Vec::with_capacity(types.len());
        for &ty in types.iter() {
            let reduced = self.checker.get_reduced_type(ty)?;
            include.push(
                if self.checker.types.flags(ty)? & tf::PRIMITIVE == 0
                    && self.checker.types.flags(reduced)? & tf::NEVER == 0
                {
                    tr::TRUE
                } else {
                    tr::FALSE
                },
            );
        }
        for property in properties {
            let name = self.checker.symbol(property)?.name_to_owned();
            let mut matched = false;
            for (i, &target) in types.iter().enumerate() {
                if include[i] == tr::FALSE {
                    continue;
                }
                if let Some(target_type) = self
                    .checker
                    .property_or_index_type(target, name.as_bytes())?
                {
                    let source_type = self.checker.get_type_of_symbol(property)?;
                    let parts = if self.checker.types.flags(source_type)? & tf::UNION != 0 {
                        self.checker.types.compound_types(source_type)?.clone()
                    } else {
                        vec![source_type].into()
                    };
                    let mut found = false;
                    for &part in parts.iter() {
                        if self.related(part, target_type, BOTH, 0)? != tr::FALSE {
                            found = true;
                            break;
                        }
                    }
                    if found {
                        matched = true;
                    } else {
                        include[i] = tr::MAYBE;
                    }
                }
            }
            for value in &mut include {
                if *value == tr::MAYBE {
                    *value = if matched { tr::FALSE } else { tr::TRUE };
                }
            }
        }
        if include.contains(&tr::FALSE) {
            let types = types
                .iter()
                .zip(include)
                .filter_map(|(&ty, include)| (include == tr::TRUE).then_some(ty))
                .collect::<Vec<_>>();
            let filtered =
                self.checker
                    .get_union_type_ex(&types, UnionReduction::None, None, None)?;
            if filtered != target && self.checker.types.flags(filtered)? & tf::NEVER == 0 {
                return Ok(Some(filtered));
            }
        }
        Ok(None)
    }

    // port: tsc/internal/checker/relater.go:Relater.typeRelatedToDiscriminatedType
    pub(crate) fn discriminated_related(
        &mut self,
        source: TypeId,
        target: TypeId,
    ) -> Result<Ternary, Error> {
        let properties = self.checker.discriminant_properties(source, target)?;
        if properties.is_empty() {
            return Ok(tr::FALSE);
        }
        let mut count = 1usize;
        for &property in &properties {
            let ty = self.checker.non_missing_symbol_type(property)?;
            count *= if self.checker.types.flags(ty)? & tf::UNION != 0 {
                self.checker.types.types_of(ty)?.len()
            } else {
                1
            };
            if count == 0 || count > 25 {
                return Ok(tr::FALSE);
            }
        }
        let mut discriminants = Vec::new();
        let mut excluded = Vec::new();
        for &property in &properties {
            let ty = self.checker.non_missing_symbol_type(property)?;
            discriminants.push(if self.checker.types.flags(ty)? & tf::UNION != 0 {
                self.checker.types.compound_types(ty)?.clone()
            } else {
                vec![ty].into()
            });
            excluded.push(self.checker.symbol(property)?.name_to_owned());
        }
        let targets = self.checker.types.compound_types(target)?.clone();
        let mut matching = Vec::new();
        let mut combination = vec![self.checker.builtins.never_type; properties.len()];
        for index in 0..count {
            let mut n = index;
            for (i, types) in discriminants.iter().enumerate().rev() {
                combination[i] = types[n % types.len()];
                n /= types.len();
            }
            let mut found = false;
            'targets: for &target in targets.iter() {
                for (i, &property) in properties.iter().enumerate() {
                    let Some(other) =
                        self.checker
                            .constituent_property(target, excluded[i].as_bytes(), false)?
                    else {
                        continue 'targets;
                    };
                    if property == other {
                        continue;
                    }
                    let skip_optional = self.checker.options.strict_null_checks
                        || self.kind == RelationKind::Comparable;
                    if self.property_related_as(
                        source,
                        target,
                        property,
                        other,
                        0,
                        Some(combination[i]),
                        skip_optional,
                    )? == tr::FALSE
                    {
                        continue 'targets;
                    }
                }
                if !matching.contains(&target) {
                    matching.push(target);
                }
                found = true;
            }
            if !found {
                return Ok(tr::FALSE);
            }
        }
        let mut result = tr::TRUE;
        for target in matching {
            result &= self.properties_related_except(source, target, false, 0, &excluded)?;
            if result != tr::FALSE {
                result &= self.signatures_related(source, target, false, 0)?;
            }
            if result != tr::FALSE {
                result &= self.signatures_related(source, target, true, 0)?;
            }
            if result != tr::FALSE
                && !(self.checker.is_tuple_type(source)? && self.checker.is_tuple_type(target)?)
            {
                result &= self.indexes_related(source, target, false, 0)?;
            }
            if result == tr::FALSE {
                break;
            }
        }
        Ok(result)
    }
}
