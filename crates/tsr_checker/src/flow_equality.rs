//! Equality narrowing preserves union origin, uniform enums and literal families.
use crate::{object_flags as of, type_facts as f, type_flags as tf, CheckerState, Error, TypeId};
use ts_arena::NodeId;
use ts_ast::SyntaxKind as K;

impl CheckerState {
    // port: tsc/internal/checker/flow.go:Checker.narrowTypeByEquality
    pub(crate) fn narrow_equality(
        &mut self,
        ty: TypeId,
        operator: ts_ast::NodeKind,
        value: NodeId,
        mut assume: bool,
    ) -> Result<TypeId, Error> {
        if self.types.flags(ty)? & tf::ANY != 0 {
            return Ok(ty);
        }
        if matches!(
            operator.known(),
            Some(K::ExclamationEqualsToken | K::ExclamationEqualsEqualsToken)
        ) {
            assume = !assume;
        }
        let value_type = self.get_type_of_expression(value)?;
        let double = matches!(
            operator.known(),
            Some(K::EqualsEqualsToken | K::ExclamationEqualsToken)
        );
        let value_flags = self.types.flags(value_type)?;
        if value_flags & tf::NULLABLE != 0 {
            if !self.options.strict_null_checks {
                return Ok(ty);
            }
            let (yes, no) = if double {
                (f::EQ_UNDEFINED_OR_NULL, f::NE_UNDEFINED_OR_NULL)
            } else if value_flags & tf::NULL != 0 {
                (f::EQ_NULL, f::NE_NULL)
            } else {
                (f::EQ_UNDEFINED, f::NE_UNDEFINED)
            };
            return self.adjusted_type_with_facts(ty, if assume { yes } else { no });
        }
        if assume {
            let parts = if self.types.flags(ty)? & tf::UNION != 0 {
                self.types.compound_types(ty)?.to_vec()
            } else {
                vec![ty]
            };
            let mut empty = false;
            if !double {
                for part in parts {
                    empty |= self.is_empty_anonymous_object_type(part)?;
                }
                if self.types.flags(ty)? & tf::UNKNOWN != 0 || empty {
                    if value_flags & (tf::PRIMITIVE | tf::NON_PRIMITIVE) != 0
                        || self.is_empty_anonymous_object_type(value_type)?
                    {
                        return Ok(value_type);
                    }
                    if value_flags & tf::OBJECT != 0 {
                        return Ok(self.builtins.non_primitive_type);
                    }
                }
                if value_flags & tf::PRIMITIVE != 0 && self.uniform_union_type(ty)? {
                    let regular = self.get_regular_type_of_literal_type(value_type)?;
                    if self.flow_union_contains(ty, regular)? {
                        return Ok(regular);
                    }
                }
            }
            let filtered = self.filter_type(ty, &mut |state, part| {
                Ok(state.types_comparable(part, value_type)?
                    || double
                        && state.types.flags(part)?
                            & (tf::NUMBER | tf::STRING | tf::BOOLEAN_LITERAL)
                            != 0
                        && value_flags & (tf::NUMBER | tf::STRING | tf::BOOLEAN) != 0)
            })?;
            return self.replace_primitives_with_literals(filtered, value_type);
        }
        if value_flags & tf::UNIT != 0 {
            if self.uniform_union_type(ty)? {
                let regular = self.get_regular_type_of_literal_type(value_type)?;
                let filtered = self.flow_remove_type(ty, regular)?;
                if filtered != ty {
                    return Ok(filtered);
                }
            }
            return self.filter_type(ty, &mut |state, part| {
                let base = state.base_constraint_of_type(part)?.unwrap_or(part);
                let unit = if state.types.flags(base)? & tf::INTERSECTION != 0 {
                    state
                        .types
                        .compound_types(base)?
                        .iter()
                        .try_fold(false, |has, &t| {
                            Ok::<_, Error>(has || state.types.flags(t)? & tf::UNIT != 0)
                        })?
                } else {
                    state.types.flags(base)? & tf::UNIT != 0
                };
                Ok(!(unit && state.types_comparable(part, value_type)?))
            });
        }
        Ok(ty)
    }

    // port: tsc/internal/checker/checker.go:Checker.isUniformUnionType
    // port: tsc/internal/checker/checker.go:Checker.computeIsUniformUnionType
    pub(crate) fn uniform_union_type(&mut self, ty: TypeId) -> Result<bool, Error> {
        let flags = self.types.get(ty)?.object_flags;
        if flags & of::PRIMITIVE_UNION == 0 {
            return Ok(false);
        }
        if flags & of::IS_UNIFORM_ENUM_COMPUTED != 0 {
            return Ok(flags & of::IS_UNIFORM_ENUM != 0);
        }
        let mut enum_symbol = None;
        let mut literal = false;
        let mut uniform = true;
        for part in self.types.compound_types(ty)?.to_vec() {
            let flags = self.types.flags(part)?;
            if flags & tf::ENUM_LIKE != 0 {
                if literal {
                    uniform = false;
                    break;
                }
                let symbol = self
                    .types
                    .get(part)?
                    .symbol
                    .ok_or(Error::MissingLink("enum literal symbol"))?;
                let parent = self.parent_of_symbol(symbol)?;
                if enum_symbol.is_none() {
                    enum_symbol = parent;
                } else if enum_symbol != parent {
                    uniform = false;
                    break;
                }
            } else if flags & tf::STRING_OR_NUMBER_LITERAL != 0 {
                if enum_symbol.is_some() {
                    uniform = false;
                    break;
                }
                literal = true;
            }
        }
        self.types.get_mut(ty)?.object_flags |=
            of::IS_UNIFORM_ENUM_COMPUTED | if uniform { of::IS_UNIFORM_ENUM } else { 0 };
        Ok(uniform)
    }

    // port: tsc/internal/checker/checker.go:Checker.unionContainsType
    pub(crate) fn flow_union_contains(&self, union: TypeId, value: TypeId) -> Result<bool, Error> {
        let parts = self.types.compound_types(union)?;
        if self.contains_type(parts, value)? {
            return Ok(true);
        }
        if value == self.builtins.missing_type {
            return self.contains_type(parts, self.builtins.undefined_type);
        }
        if value == self.builtins.undefined_type {
            return self.contains_type(parts, self.builtins.missing_type);
        }
        let flags = self.types.flags(value)?;
        let primitive = if flags & tf::STRING_LITERAL != 0 {
            Some(self.builtins.string_type)
        } else if flags & (tf::ENUM | tf::NUMBER_LITERAL) != 0 {
            Some(self.builtins.number_type)
        } else if flags & tf::BIG_INT_LITERAL != 0 {
            Some(self.builtins.bigint_type)
        } else {
            None
        };
        match primitive {
            Some(ty) => self.contains_type(parts, ty),
            None => Ok(false),
        }
    }

    // port: tsc/internal/checker/checker.go:Checker.removeType
    pub(crate) fn flow_remove_type(&mut self, ty: TypeId, target: TypeId) -> Result<TypeId, Error> {
        if self.types.flags(ty)? & tf::UNION == 0 {
            return Ok(if ty == target {
                self.builtins.never_type
            } else {
                ty
            });
        }
        if let Some(origin) = self.types.union(ty)?.origin {
            if self.types.flags(origin)? & tf::UNION != 0
                && self.contains_type(self.types.compound_types(origin)?, target)?
            {
                return self.filter_type(ty, &mut |_, part| Ok(part != target));
            }
        }
        let parts = self.types.compound_types(ty)?.to_vec();
        if !self.contains_type(&parts, target)? {
            return Ok(ty);
        }
        let parts = parts.into_iter().filter(|&part| part != target).collect();
        let flags =
            self.types.get(ty)?.object_flags & (of::PRIMITIVE_UNION | of::CONTAINS_INTERSECTIONS);
        self.get_union_type_from_sorted_list(parts, flags, None, None)
    }

    // port: tsc/internal/checker/flow.go:Checker.replacePrimitivesWithLiterals
    pub(crate) fn replace_primitives_with_literals(
        &mut self,
        primitives: TypeId,
        literals: TypeId,
    ) -> Result<TypeId, Error> {
        if !self.maybe_type_of_kind(
            primitives,
            tf::STRING | tf::TEMPLATE_LITERAL | tf::NUMBER | tf::BIG_INT,
        )? || !self.maybe_type_of_kind(
            literals,
            tf::STRING_LITERAL
                | tf::TEMPLATE_LITERAL
                | tf::STRING_MAPPING
                | tf::NUMBER_LITERAL
                | tf::BIG_INT_LITERAL,
        )? {
            return Ok(primitives);
        }
        self.map_type(primitives, &mut |state, part| {
            let flags = state.types.flags(part)?;
            let extract = if flags & tf::STRING != 0 {
                tf::STRING | tf::STRING_LITERAL | tf::TEMPLATE_LITERAL | tf::STRING_MAPPING
            } else if state.is_pattern_literal_type(part)?
                && !state.maybe_type_of_kind(
                    literals,
                    tf::STRING | tf::TEMPLATE_LITERAL | tf::STRING_MAPPING,
                )?
            {
                tf::STRING_LITERAL
            } else if flags & tf::NUMBER != 0 {
                tf::NUMBER | tf::NUMBER_LITERAL
            } else if flags & tf::BIG_INT != 0 {
                tf::BIG_INT | tf::BIG_INT_LITERAL
            } else {
                return Ok(Some(part));
            };
            state
                .filter_type(literals, &mut |s, t| Ok(s.types.flags(t)? & extract != 0))
                .map(Some)
        })?
        .ok_or(Error::MissingLink("primitive literal replacement"))
    }
}
