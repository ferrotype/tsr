//! Key type construction used by native relation elaboration and index checks.

use super::*;

impl Relater<'_> {
    // port: tsc/internal/checker/checker.go:Checker.getLiteralTypeFromProperty
    pub(super) fn property_key(&self, property: &Member) -> Result<Rc<TypeCell>, Error> {
        if let Some(key) = &property.name_type {
            key.resolve()
        } else {
            Ok(self.graph().string_literal(property.name.as_bytes()))
        }
    }

    // port: tsc/internal/checker/checker.go:Checker.getIndexTypeEx
    pub(super) fn relation_key_type(&mut self, ty: &Rc<TypeCell>) -> Result<Rc<TypeCell>, Error> {
        if let Some(key) = self.checker.property_keys.borrow().get(&ty.id()) {
            return key.upgrade().ok_or(Error::Released);
        }
        let result = if ty.flags & flags::UNION != 0 {
            let mut result = None;
            for part in ty.types()? {
                let key = self.relation_key_type(&part)?;
                result = Some(if let Some(previous) = result {
                    self.key_intersection(&previous, &key)?
                } else {
                    key
                });
            }
            result.ok_or(Error::ResolutionFailed)?
        } else if ty.flags & flags::INTERSECTION != 0 {
            let keys = ty
                .types()?
                .iter()
                .map(|part| self.relation_key_type(part))
                .collect::<Result<Vec<_>, _>>()?;
            self.graph().union(&keys)?
        } else if ty.flags & flags::UNKNOWN != 0 {
            self.checker.intrinsic(flags::NEVER)?
        } else if ty.flags & (flags::ANY | flags::NEVER) != 0 {
            self.graph().union(&[
                self.checker.intrinsic(flags::STRING)?,
                self.checker.intrinsic(flags::NUMBER)?,
                self.checker.intrinsic(flags::ES_SYMBOL)?,
            ])?
        } else {
            if ty.flags & flags::INSTANTIABLE != 0 {
                return unsupported("deferred generic keyof in relation elaboration");
            }
            // getLiteralTypeFromProperties retains an origin index even if its
            // resulting union simplifies to a single key. This is a real
            // semantic type, allocated at this demand point.
            let origin = if ty.object_flags
                & (object_flags::CLASS | object_flags::INTERFACE | object_flags::REFERENCE)
                != 0
                || ty.alias.is_some()
            {
                Some(self.graph().allocate_full(
                    flags::INDEX,
                    0,
                    Rc::from("keyof"),
                    None,
                    None,
                    None,
                    false,
                    vec![Rc::downgrade(ty)],
                    false,
                    None,
                ))
            } else {
                None
            };
            let mut keys = self
                .properties_of_type(ty)?
                .iter()
                .map(|property| self.property_key(property))
                .collect::<Result<Vec<_>, _>>()?;
            for index in self.index_infos_of_type(ty)? {
                let key = index.key()?;
                if key.flags & flags::STRING != 0 {
                    keys.push(self.checker.intrinsic(flags::NUMBER)?);
                }
                keys.push(key);
            }
            self.graph().union_with_origin(&keys, origin.as_ref())?
        };
        self.checker
            .property_keys
            .borrow_mut()
            .insert(ty.id(), Rc::downgrade(&result));
        Ok(result)
    }

    /// The primitive/literal intersection normalization reached by keyof
    /// overlap. It retains ordinary cached union identities and fails visibly
    /// for generic intersections requiring constraint evaluation.
    pub(super) fn key_intersection(
        &self,
        left: &Rc<TypeCell>,
        right: &Rc<TypeCell>,
    ) -> Result<Rc<TypeCell>, Error> {
        if Rc::ptr_eq(left, right) {
            return Ok(left.clone());
        }
        if left.flags & (flags::ANY | flags::UNKNOWN) != 0 {
            return Ok(right.clone());
        }
        if right.flags & (flags::ANY | flags::UNKNOWN) != 0 {
            return Ok(left.clone());
        }
        if left.flags & flags::INDEX != 0 || right.flags & flags::INDEX != 0 {
            return unsupported("generic keyof intersection");
        }
        let a = if left.flags & flags::UNION != 0 {
            left.types()?
        } else {
            vec![left.clone()]
        };
        let b = if right.flags & flags::UNION != 0 {
            right.types()?
        } else {
            vec![right.clone()]
        };
        let mut common = Vec::new();
        for a in &a {
            for b in &b {
                let value = if Rc::ptr_eq(a, b) {
                    Some(a.clone())
                } else if a.flags & flags::STRING != 0 && b.flags & flags::STRING_LITERAL != 0
                    || a.flags & flags::NUMBER != 0 && b.flags & flags::NUMBER_LITERAL != 0
                    || a.flags & flags::ES_SYMBOL != 0 && b.flags & flags::UNIQUE_ES_SYMBOL != 0
                {
                    Some(b.clone())
                } else if b.flags & flags::STRING != 0 && a.flags & flags::STRING_LITERAL != 0
                    || b.flags & flags::NUMBER != 0 && a.flags & flags::NUMBER_LITERAL != 0
                    || b.flags & flags::ES_SYMBOL != 0 && a.flags & flags::UNIQUE_ES_SYMBOL != 0
                {
                    Some(a.clone())
                } else {
                    None
                };
                if let Some(value) = value {
                    common.push(value)
                }
            }
        }
        self.graph().union(&common)
    }
}
