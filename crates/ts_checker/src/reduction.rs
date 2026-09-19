//! Lazy reduction of intersections with conflicting discriminant properties.
//! A failed discovery must not leave a completed-reduction flag behind.

use crate::{object_flags as of, type_flags as tf, CheckerState, Error, TypeId};
use ts_arena::SymbolId;
use ts_ast::{check_flags as cf, symbol_flags as sf};

impl CheckerState {
    // port: tsc/internal/checker/checker.go:Checker.getReducedType
    pub(crate) fn get_reduced_type(&mut self, ty: TypeId) -> Result<TypeId, Error> {
        let record = *self.types.get(ty)?;
        if record.flags & tf::UNION != 0 && record.object_flags & of::CONTAINS_INTERSECTIONS != 0 {
            if let Some(reduced) = self.types.union(ty)?.resolved_reduced_type {
                return Ok(reduced);
            }
            let reduced = self.get_reduced_union_type(ty)?;
            self.types.union_mut(ty)?.resolved_reduced_type = Some(reduced);
            return Ok(reduced);
        }
        if record.flags & tf::INTERSECTION != 0 {
            if record.object_flags & of::IS_NEVER_INTERSECTION_COMPUTED == 0 {
                // Recursive member reads see the original type during this
                // operation, matching Go's early computed-bit assignment.
                self.types.get_mut(ty)?.object_flags |= of::IS_NEVER_INTERSECTION_COMPUTED;
                let reduced = (|| {
                    let properties = self.get_properties_of_union_or_intersection_type(ty)?;
                    for prop in properties {
                        if self.is_never_reduced_property(prop)? {
                            return Ok(true);
                        }
                    }
                    Ok(false)
                })();
                match reduced {
                    Ok(true) => self.types.get_mut(ty)?.object_flags |= of::IS_NEVER_INTERSECTION,
                    Ok(false) => {}
                    Err(error) => {
                        self.types.get_mut(ty)?.object_flags &= !of::IS_NEVER_INTERSECTION_COMPUTED;
                        return Err(error);
                    }
                }
            }
            if self.types.get(ty)?.object_flags & of::IS_NEVER_INTERSECTION != 0 {
                return Ok(self.builtins.never_type);
            }
        }
        Ok(ty)
    }

    // port: tsc/internal/checker/checker.go:Checker.getReducedUnionType
    fn get_reduced_union_type(&mut self, ty: TypeId) -> Result<TypeId, Error> {
        let types = self.types.union(ty)?.types.clone();
        let mut reduced = Vec::with_capacity(types.len());
        for &ty in types.iter() {
            reduced.push(self.get_reduced_type(ty)?);
        }
        if reduced.as_slice() == types.as_ref() {
            return Ok(ty);
        }
        let result = self.get_union_type(&reduced)?;
        if self.types.flags(result)? & tf::UNION != 0 {
            self.types.union_mut(result)?.resolved_reduced_type = Some(result);
        }
        Ok(result)
    }

    // port: tsc/internal/checker/checker.go:Checker.isDiscriminantWithNeverType
    pub(crate) fn is_discriminant_with_never_type(
        &mut self,
        prop: SymbolId,
    ) -> Result<bool, Error> {
        let symbol = self.symbol(prop)?;
        if symbol.flags() & sf::OPTIONAL != 0
            || symbol.check_flags() & (cf::NON_UNIFORM_AND_LITERAL | cf::HAS_NEVER_TYPE)
                != cf::NON_UNIFORM_AND_LITERAL
        {
            return Ok(false);
        }
        let ty = self.get_type_of_symbol(prop)?;
        Ok(self.types.flags(ty)? & tf::NEVER != 0)
    }

    // port: tsc/internal/checker/checker.go:Checker.isNeverReducedProperty
    pub(crate) fn is_never_reduced_property(&mut self, prop: SymbolId) -> Result<bool, Error> {
        Ok(self.is_discriminant_with_never_type(prop)?
            || self.is_conflicting_private_property(prop)?)
    }

    // port: tsc/internal/checker/checker.go:isConflictingPrivateProperty
    pub(crate) fn is_conflicting_private_property(&self, prop: SymbolId) -> Result<bool, Error> {
        // Return true for a synthetic property with multiple declarations, at least one of which is private.
        let symbol = self.symbol(prop)?;
        Ok(
            symbol.value_declaration().is_none()
                && symbol.check_flags() & cf::CONTAINS_PRIVATE != 0,
        )
    }
}
