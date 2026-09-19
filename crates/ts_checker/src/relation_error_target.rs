//! Ordered best-match selection for diagnostic elaboration of target unions.
use crate::{object_flags as of, relater::Relater, type_flags as tf, Error, TypeId};
impl Relater<'_> {
    // port: tsc/internal/checker/relater.go:Checker.getBestMatchingType
    pub(crate) fn best_error_target(
        &mut self,
        source: TypeId,
        target: TypeId,
    ) -> Result<Option<TypeId>, Error> {
        if let Some(found) = self.with_reporting(false, |this| {
            this.matching_discriminant_type(source, target)
        })? {
            return Ok(Some(found));
        }
        let targets = self.checker.types.compound_types(target)?.clone();
        let source_flags = self.checker.types.object_flags(source)?;
        if source_flags & (of::REFERENCE | of::ANONYMOUS) != 0 {
            for &target in targets.iter() {
                if self.checker.types.flags(target)? & tf::OBJECT == 0 {
                    continue;
                }
                let overlap = source_flags & self.checker.types.object_flags(target)?;
                if overlap & of::REFERENCE != 0
                    && self.checker.types.target(source)? == self.checker.types.target(target)?
                {
                    return Ok(Some(target));
                }
                if overlap & of::ANONYMOUS != 0 {
                    if let (Some(a), Some(b)) = (
                        self.checker.types.alias_of(source)?,
                        self.checker.types.alias_of(target)?,
                    ) {
                        if a.symbol == b.symbol {
                            return Ok(Some(target));
                        }
                    }
                }
            }
        }
        if source_flags & of::OBJECT_LITERAL != 0 {
            let mut some_array = false;
            for &target in targets.iter() {
                if self.checker.is_array_like_type(target)? {
                    some_array = true;
                    break;
                }
            }
            if some_array {
                for &target in targets.iter() {
                    if !self.checker.is_array_like_type(target)? {
                        return Ok(Some(target));
                    }
                }
            }
        }
        for construct in [false, true] {
            if !self
                .checker
                .signatures_of_type(source, construct)?
                .is_empty()
            {
                for &target in targets.iter() {
                    if !self
                        .checker
                        .signatures_of_type(target, construct)?
                        .is_empty()
                    {
                        return Ok(Some(target));
                    }
                }
            }
        }
        // port: tsc/internal/checker/relater.go:Checker.findMostOverlappyType
        let mut best = None;
        let mut matching_count = 0;
        if self.checker.types.flags(source)? & (tf::PRIMITIVE | tf::INSTANTIABLE_PRIMITIVE) == 0 {
            for &target in targets.iter() {
                if self.checker.types.flags(target)? & (tf::PRIMITIVE | tf::INSTANTIABLE_PRIMITIVE)
                    != 0
                {
                    continue;
                }
                let source_index = self.checker.get_index_type(source, 0)?;
                let target_index = self.checker.get_index_type(target, 0)?;
                let overlap = self
                    .checker
                    .get_intersection_type(&[source_index, target_index])?;
                let flags = self.checker.types.flags(overlap)?;
                if flags & tf::INDEX != 0 {
                    return Ok(Some(target));
                }
                if flags & (tf::UNIT | tf::UNION) != 0 {
                    let mut count = 1;
                    if flags & tf::UNION != 0 {
                        count = 0;
                        for &part in self.checker.types.types_of(overlap)? {
                            if self.checker.types.flags(part)? & tf::UNIT != 0 {
                                count += 1;
                            }
                        }
                    }
                    if count >= matching_count {
                        best = Some(target);
                        matching_count = count;
                    }
                }
            }
        }
        Ok(best)
    }
}
