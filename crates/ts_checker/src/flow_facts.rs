//! Type fact filtering shared by expressions and control-flow narrowing.

use crate::{
    object_flags as of, type_facts as f, type_flags as tf, CheckerState, Error, LiteralValue,
    RelationKind, TypeId,
};

impl CheckerState {
    // port: tsc/internal/checker/checker.go:Checker.GetNonNullableType
    pub(crate) fn non_nullable_type(&mut self, ty: TypeId) -> Result<TypeId, Error> {
        if self.options.strict_null_checks {
            self.adjusted_type_with_facts(ty, f::NE_UNDEFINED_OR_NULL)
        } else {
            Ok(ty)
        }
    }
    // port: tsc/internal/checker/checker.go:Checker.getTypeFacts
    pub(crate) fn type_facts(&mut self, ty: TypeId, mask: u32) -> Result<u32, Error> {
        Ok(self.type_facts_worker(ty, mask)? & mask)
    }

    // port: tsc/internal/checker/checker.go:Checker.getTypeFactsWorker
    fn type_facts_worker(&mut self, mut ty: TypeId, mask: u32) -> Result<u32, Error> {
        if self.types.flags(ty)? & (tf::INTERSECTION | tf::INSTANTIABLE) != 0 {
            ty = self
                .base_constraint_of_type(ty)?
                .unwrap_or(self.builtins.unknown_type);
        }
        let flags = self.types.flags(ty)?;
        let strict = self.options.strict_null_checks;
        let facts = if flags & (tf::STRING | tf::STRING_MAPPING) != 0 {
            if strict {
                f::STRING_STRICT_FACTS
            } else {
                f::STRING_FACTS
            }
        } else if flags & (tf::STRING_LITERAL | tf::TEMPLATE_LITERAL) != 0 {
            let empty = flags & tf::STRING_LITERAL != 0
                && matches!(&self.types.literal(ty)?.value, LiteralValue::String(value) if value.is_empty());
            match (strict, empty) {
                (true, true) => f::EMPTY_STRING_STRICT_FACTS,
                (true, false) => f::NON_EMPTY_STRING_STRICT_FACTS,
                (false, true) => f::EMPTY_STRING_FACTS,
                (false, false) => f::NON_EMPTY_STRING_FACTS,
            }
        } else if flags & (tf::NUMBER | tf::ENUM) != 0 {
            if strict {
                f::NUMBER_STRICT_FACTS
            } else {
                f::NUMBER_FACTS
            }
        } else if flags & tf::NUMBER_LITERAL != 0 {
            let zero = matches!(&self.types.literal(ty)?.value, LiteralValue::Number(value) if value.value() == 0.0);
            match (strict, zero) {
                (true, true) => f::ZERO_NUMBER_STRICT_FACTS,
                (true, false) => f::NON_ZERO_NUMBER_STRICT_FACTS,
                (false, true) => f::ZERO_NUMBER_FACTS,
                (false, false) => f::NON_ZERO_NUMBER_FACTS,
            }
        } else if flags & tf::BIG_INT != 0 {
            if strict {
                f::BIG_INT_STRICT_FACTS
            } else {
                f::BIG_INT_FACTS
            }
        } else if flags & tf::BIG_INT_LITERAL != 0 {
            let zero = matches!(&self.types.literal(ty)?.value, LiteralValue::BigInt(value) if *value == ts_jsnum::PseudoBigInt::default());
            match (strict, zero) {
                (true, true) => f::ZERO_BIG_INT_STRICT_FACTS,
                (true, false) => f::NON_ZERO_BIG_INT_STRICT_FACTS,
                (false, true) => f::ZERO_BIG_INT_FACTS,
                (false, false) => f::NON_ZERO_BIG_INT_FACTS,
            }
        } else if flags & tf::BOOLEAN != 0 {
            if strict {
                f::BOOLEAN_STRICT_FACTS
            } else {
                f::BOOLEAN_FACTS
            }
        } else if flags & tf::BOOLEAN_LIKE != 0 {
            match (
                strict,
                ty == self.builtins.false_type || ty == self.builtins.regular_false_type,
            ) {
                (true, true) => f::FALSE_STRICT_FACTS,
                (true, false) => f::TRUE_STRICT_FACTS,
                (false, true) => f::FALSE_FACTS,
                (false, false) => f::TRUE_FACTS,
            }
        } else if flags & tf::OBJECT != 0 {
            let possible = if strict {
                f::EMPTY_OBJECT_STRICT_FACTS | f::FUNCTION_STRICT_FACTS | f::OBJECT_STRICT_FACTS
            } else {
                f::EMPTY_OBJECT_FACTS | f::FUNCTION_FACTS | f::OBJECT_FACTS
            };
            if mask & possible == 0 {
                return Ok(0);
            }
            if self.types.get(ty)?.object_flags & of::ANONYMOUS != 0
                && self.empty_object_type(ty)?
            {
                if strict {
                    f::EMPTY_OBJECT_STRICT_FACTS
                } else {
                    f::EMPTY_OBJECT_FACTS
                }
            } else if self.is_function_object_type(ty)? {
                if strict {
                    f::FUNCTION_STRICT_FACTS
                } else {
                    f::FUNCTION_FACTS
                }
            } else if strict {
                f::OBJECT_STRICT_FACTS
            } else {
                f::OBJECT_FACTS
            }
        } else if flags & tf::VOID != 0 {
            f::VOID_FACTS
        } else if flags & tf::UNDEFINED != 0 {
            f::UNDEFINED_FACTS
        } else if flags & tf::NULL != 0 {
            f::NULL_FACTS
        } else if flags & tf::ES_SYMBOL_LIKE != 0 {
            if strict {
                f::SYMBOL_STRICT_FACTS
            } else {
                f::SYMBOL_FACTS
            }
        } else if flags & tf::NON_PRIMITIVE != 0 {
            if strict {
                f::OBJECT_STRICT_FACTS
            } else {
                f::OBJECT_FACTS
            }
        } else if flags & tf::NEVER != 0 {
            0
        } else if flags & tf::UNION != 0 {
            let mut facts = 0;
            for &part in self.types.compound_types(ty)?.clone().iter() {
                facts |= self.type_facts_worker(part, mask)?;
            }
            facts
        } else if flags & tf::INTERSECTION != 0 {
            let ignore_objects = self.maybe_type_of_kind(ty, tf::PRIMITIVE)?;
            let (mut ored, mut anded) = (0, f::ALL);
            for &part in self.types.compound_types(ty)?.clone().iter() {
                if !(ignore_objects && self.types.flags(part)? & tf::OBJECT != 0) {
                    let facts = self.type_facts_worker(part, mask)?;
                    ored |= facts;
                    anded &= facts;
                }
            }
            ored & f::OR_FACTS_MASK | anded & f::AND_FACTS_MASK
        } else {
            f::UNKNOWN_FACTS
        };
        Ok(facts)
    }

    // port: tsc/internal/checker/checker.go:Checker.isFunctionObjectType
    pub(crate) fn is_function_object_type(&mut self, ty: TypeId) -> Result<bool, Error> {
        if self.types.get(ty)?.object_flags & of::EVOLVING_ARRAY != 0 {
            return Ok(false);
        }
        self.resolve_type_members(ty)?;
        let members = self.types.structured(ty)?;
        if members.signatures.as_ref().is_some_and(|s| !s.is_empty()) {
            return Ok(true);
        }
        let bind = members
            .members
            .map(|m| self.table(m).map(|m| m.get(b"bind").flatten().is_some()))
            .transpose()?
            .unwrap_or(false);
        if !bind {
            return Ok(false);
        }
        let function = self
            .query
            .global_types
            .get("Function")
            .copied()
            .ok_or(Error::MissingLink("global Function"))?;
        self.is_type_related_to(ty, function, RelationKind::Subtype)
    }

    // port: tsc/internal/checker/checker.go:Checker.getTypeWithFacts
    pub(crate) fn type_with_facts(&mut self, ty: TypeId, facts: u32) -> Result<TypeId, Error> {
        self.filter_type(ty, &mut |state, part| {
            Ok(state.type_facts(part, facts)? != 0)
        })
    }

    // port: tsc/internal/checker/checker.go:Checker.getAdjustedTypeWithFacts
    pub(crate) fn adjusted_type_with_facts(
        &mut self,
        ty: TypeId,
        facts: u32,
    ) -> Result<TypeId, Error> {
        let strict = self.options.strict_null_checks;
        let ty = if strict && self.types.flags(ty)? & tf::UNKNOWN != 0 {
            self.builtins.unknown_union_type
        } else {
            ty
        };
        let mut reduced = self.type_with_facts(ty, facts)?;
        if reduced == self.builtins.unknown_union_type {
            reduced = self.builtins.unknown_type;
        }
        if strict {
            match facts {
                f::NE_UNDEFINED => {
                    return self.remove_nullable_by_intersection(
                        reduced,
                        f::EQ_UNDEFINED,
                        f::EQ_NULL,
                        f::IS_NULL,
                        self.builtins.null_type,
                    )
                }
                f::NE_NULL => {
                    return self.remove_nullable_by_intersection(
                        reduced,
                        f::EQ_NULL,
                        f::EQ_UNDEFINED,
                        f::IS_UNDEFINED,
                        self.builtins.undefined_type,
                    )
                }
                f::NE_UNDEFINED_OR_NULL | f::TRUTHY => {
                    return self
                        .map_type(reduced, &mut |state, part| {
                            Ok(Some(
                                if state.type_facts(part, f::EQ_UNDEFINED_OR_NULL)? != 0 {
                                    state.global_non_nullable_type(part)?
                                } else {
                                    part
                                },
                            ))
                        })?
                        .ok_or(Error::MissingLink("adjusted type"));
                }
                _ => {}
            }
        }
        Ok(reduced)
    }

    // port: tsc/internal/checker/checker.go:Checker.removeNullableByIntersection
    fn remove_nullable_by_intersection(
        &mut self,
        ty: TypeId,
        target: u32,
        other: u32,
        includes: u32,
        other_type: TypeId,
    ) -> Result<TypeId, Error> {
        let facts = self.type_facts(
            ty,
            f::EQ_UNDEFINED | f::EQ_NULL | f::IS_UNDEFINED | f::IS_NULL,
        )?;
        if facts & target == 0 {
            return Ok(ty);
        }
        let empty_other = self.get_union_type(&[self.builtins.empty_object_type, other_type])?;
        self.map_type(ty, &mut |state, part| {
            Ok(Some(if state.type_facts(part, target)? != 0 {
                let other = if facts & includes == 0 && state.type_facts(part, other)? != 0 {
                    empty_other
                } else {
                    state.builtins.empty_object_type
                };
                state.get_intersection_type(&[part, other])?
            } else {
                part
            }))
        })?
        .ok_or(Error::MissingLink("non-nullable intersection"))
    }

    // port: tsc/internal/checker/checker.go:Checker.getGlobalNonNullableTypeInstantiation
    fn global_non_nullable_type(&mut self, ty: TypeId) -> Result<TypeId, Error> {
        if let Some(symbol) = self.lookup_symbol(
            self.builtins.globals,
            b"NonNullable",
            ts_ast::symbol_flags::TYPE_ALIAS,
        )? {
            // getGlobalTypeAliasResolver only accepts aliases with the expected arity.
            let declared = self.get_declared_type_of_symbol(symbol)?;
            let parameters = self
                .query
                .type_aliases
                .try_get(symbol)
                .and_then(|links| links.parameters.clone())
                .unwrap_or_default();
            if parameters.len() == 1 {
                return self.type_alias_instantiation(symbol, declared, &parameters, &[ty], None);
            }
        }
        self.get_intersection_type(&[ty, self.builtins.empty_object_type])
    }
}
