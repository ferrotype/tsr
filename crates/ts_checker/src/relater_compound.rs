//! Ordered union/intersection decomposition shared by all relation modes.
use crate::{
    object_flags as of,
    relater::{Relater, RelationKind, BOTH, SOURCE, TARGET},
    ternary as tr, type_flags as tf, Error, Ternary, TypeId,
};

impl Relater<'_> {
    // port: tsc/internal/checker/relater.go:Relater.unionOrIntersectionRelatedTo
    pub(crate) fn union_intersection_related(
        &mut self,
        mut source: TypeId,
        target: TypeId,
        intersection: u32,
    ) -> Result<Ternary, Error> {
        let s = self.checker.types.flags(source)?;
        let t = self.checker.types.flags(target)?;
        if s & tf::UNION != 0 {
            if t & tf::UNION != 0 {
                if let Some(origin) = self.checker.types.union(source)?.origin {
                    if self.checker.types.flags(origin)? & tf::INTERSECTION != 0
                        && self.checker.types.get(target)?.alias.is_some()
                        && self.checker.types.types_of(origin)?.contains(&target)
                    {
                        return Ok(tr::TRUE);
                    }
                }
                if let Some(origin) = self.checker.types.union(target)?.origin {
                    if self.checker.types.flags(origin)? & tf::UNION != 0
                        && self.checker.types.get(source)?.alias.is_some()
                        && self.checker.types.types_of(origin)?.contains(&source)
                    {
                        return Ok(tr::TRUE);
                    }
                }
            }
            return self.with_reporting(self.report_errors && s & tf::PRIMITIVE == 0, |this| {
                if this.kind == RelationKind::Comparable {
                    this.some_source_related(source, target, intersection)
                } else {
                    this.each_source_related(source, target, intersection)
                }
            });
        }
        if t & tf::UNION != 0 {
            // Regularizing a fresh object recursively regularizes its property
            // types. That source helper is also needed for excess-property checks.
            let source = self.checker.regular_object_literal_type(source)?;
            return self.with_reporting(
                self.report_errors && s & tf::PRIMITIVE == 0 && t & tf::PRIMITIVE == 0,
                |this| this.some_target_related(source, target, intersection),
            );
        }
        if t & tf::INTERSECTION != 0 {
            let mut result = tr::TRUE;
            let types = self.checker.types.compound_types(target)?.clone();
            for &part in types.iter() {
                result &= self.related(source, part, TARGET, TARGET)?;
                if result == tr::FALSE {
                    break;
                }
            }
            return Ok(result);
        }
        if self.kind == RelationKind::Comparable && t & tf::PRIMITIVE != 0 {
            let types = self.checker.types.compound_types(source)?.clone();
            let mut constraints = Vec::with_capacity(types.len());
            for &part in types.iter() {
                constraints.push(if self.checker.types.flags(part)? & tf::INSTANTIABLE != 0 {
                    self.checker
                        .base_constraint_of_type(part)?
                        .unwrap_or(self.checker.builtins.unknown_type)
                } else {
                    part
                });
            }
            if constraints != types.as_ref() {
                source = self.checker.get_intersection_type(&constraints)?;
                let flags = self.checker.types.flags(source)?;
                if flags & tf::NEVER != 0 {
                    return Ok(tr::FALSE);
                }
                if flags & tf::INTERSECTION == 0 {
                    let result = self.related_with_errors(source, target, SOURCE, 0, false)?;
                    return if result == tr::FALSE {
                        self.related_with_errors(target, source, SOURCE, 0, false)
                    } else {
                        Ok(result)
                    };
                }
            }
        }
        self.with_reporting(false, |this| {
            this.some_source_related(source, target, SOURCE)
        })
    }

    // port: tsc/internal/checker/relater.go:Relater.someTypeRelatedToType
    fn some_source_related(
        &mut self,
        source: TypeId,
        target: TypeId,
        intersection: u32,
    ) -> Result<Ternary, Error> {
        let types = self.checker.types.compound_types(source)?.clone();
        if self.checker.types.flags(source)? & tf::UNION != 0 && types.contains(&target) {
            return Ok(tr::TRUE);
        }
        for (index, &part) in types.iter().enumerate() {
            let result = self.related_with_errors(
                part,
                target,
                SOURCE,
                intersection,
                self.report_errors && index + 1 == types.len(),
            )?;
            if result != tr::FALSE {
                return Ok(result);
            }
        }
        Ok(tr::FALSE)
    }

    // port: tsc/internal/checker/relater.go:Relater.eachTypeRelatedToType
    fn each_source_related(
        &mut self,
        source: TypeId,
        target: TypeId,
        intersection: u32,
    ) -> Result<Ternary, Error> {
        let sources = self.checker.types.compound_types(source)?.clone();
        let mut stripped = target;
        if self.checker.types.flags(target)? & tf::UNION != 0
            && self.checker.types.flags(sources[0])? & tf::UNDEFINED == 0
            && self
                .checker
                .types
                .flags(self.checker.types.types_of(target)?[0])?
                & tf::UNDEFINED
                != 0
        {
            stripped = self.checker.filter_type_flags(target, !tf::UNDEFINED)?;
        }
        let targets = if self.checker.types.flags(stripped)? & tf::UNION != 0 {
            Some(self.checker.types.compound_types(stripped)?.clone())
        } else {
            None
        };
        let mut result = tr::TRUE;
        for (index, &part) in sources.iter().enumerate() {
            if let Some(targets) = &targets {
                if sources.len() >= targets.len() && sources.len() % targets.len() == 0 {
                    let related = self.related_with_errors(
                        part,
                        targets[index % targets.len()],
                        BOTH,
                        intersection,
                        false,
                    )?;
                    if related != tr::FALSE {
                        result &= related;
                        continue;
                    }
                }
            }
            result &= self.related(part, target, SOURCE, intersection)?;
            if result == tr::FALSE {
                break;
            }
        }
        Ok(result)
    }

    // port: tsc/internal/checker/relater.go:Relater.typeRelatedToSomeType
    pub(crate) fn some_target_related(
        &mut self,
        source: TypeId,
        target: TypeId,
        intersection: u32,
    ) -> Result<Ternary, Error> {
        let types = self.checker.types.compound_types(target)?.clone();
        if self.checker.types.flags(target)? & tf::UNION != 0 {
            if types.contains(&source) {
                return Ok(tr::TRUE);
            }
            let flags = self.checker.types.flags(source)?;
            if self.kind != RelationKind::Comparable
                && self.checker.types.get(target)?.object_flags & of::PRIMITIVE_UNION != 0
                && flags & tf::ENUM_LITERAL == 0
                && (flags & (tf::STRING_LITERAL | tf::BOOLEAN_LITERAL | tf::BIG_INT_LITERAL) != 0
                    || matches!(
                        self.kind,
                        RelationKind::Subtype | RelationKind::StrictSubtype
                    ) && flags & tf::NUMBER_LITERAL != 0)
            {
                let literal = self.checker.types.literal(source)?;
                let alternate = if source == literal.regular {
                    literal.fresh
                } else {
                    Some(literal.regular)
                };
                let primitive = if flags & tf::STRING_LITERAL != 0 {
                    Some(self.checker.builtins.string_type)
                } else if flags & tf::NUMBER_LITERAL != 0 {
                    Some(self.checker.builtins.number_type)
                } else if flags & tf::BIG_INT_LITERAL != 0 {
                    Some(self.checker.builtins.bigint_type)
                } else {
                    None
                };
                return Ok(
                    if primitive.is_some_and(|ty| types.contains(&ty))
                        || alternate.is_some_and(|ty| types.contains(&ty))
                    {
                        tr::TRUE
                    } else {
                        tr::FALSE
                    },
                );
            }
            // Discriminant lookup is an optimization with observable cache
            // effects, supplied by the production discriminant cache.
            if let Some(matching) = self.checker.matching_union_constituent(target, source)? {
                let result =
                    self.related_with_errors(source, matching, TARGET, intersection, false)?;
                if result != tr::FALSE {
                    return Ok(result);
                }
            }
        }
        for &part in types.iter() {
            let result = self.related_with_errors(source, part, TARGET, intersection, false)?;
            if result != tr::FALSE {
                return Ok(result);
            }
        }
        if self.report_errors {
            if let Some(best) = self.best_error_target(source, target)? {
                self.related_with_errors(source, best, TARGET, intersection, true)?;
            }
        }
        Ok(tr::FALSE)
    }

    // port: tsc/internal/checker/relater.go:Relater.eachTypeRelatedToSomeType
    pub(crate) fn each_source_some_target(
        &mut self,
        source: TypeId,
        target: TypeId,
    ) -> Result<Ternary, Error> {
        let mut result = tr::TRUE;
        let parts = self.checker.types.compound_types(source)?.clone();
        for &part in parts.iter() {
            result &=
                self.with_reporting(false, |this| this.some_target_related(part, target, 0))?;
            if result == tr::FALSE {
                break;
            }
        }
        Ok(result)
    }
}
