use crate::{
    relater::{Relater, RelationKind, BOTH, TARGET},
    ternary as tr, type_flags as tf, Error, Ternary, TypeId,
};

impl Relater<'_> {
    // port: tsc/internal/checker/relater.go:Relater.mappedTypeRelatedTo
    pub(crate) fn mapped_related(
        &mut self,
        source: TypeId,
        target: TypeId,
    ) -> Result<Ternary, Error> {
        let modifiers_related = if self.kind == RelationKind::Comparable {
            true
        } else if self.kind == RelationKind::Identity {
            self.checker.mapped_modifiers(source)? == self.checker.mapped_modifiers(target)?
        } else {
            self.checker.combined_mapped_optionality(source)?
                <= self.checker.combined_mapped_optionality(target)?
        };
        if !modifiers_related {
            return Ok(tr::FALSE);
        }
        let target_constraint = self.checker.mapped_constraint(target)?;
        let source_constraint = self.checker.mapped_constraint(source)?;
        let unmeasurable = self.checker.combined_mapped_optionality(source)? < 0;
        let source_constraint = self
            .checker
            .report_unreliable_markers(source_constraint, unmeasurable)?;
        let mut result = self.related(target_constraint, source_constraint, BOTH, 0)?;
        if result == tr::FALSE {
            return Ok(result);
        }
        let source_parameter = self.checker.mapped_parameter(source)?;
        let target_parameter = self.checker.mapped_parameter(target)?;
        let mapper = self
            .checker
            .new_type_mapper(&[source_parameter], &[target_parameter])?;
        let source_name = self
            .checker
            .mapped_name(source)?
            .map(|ty| self.checker.instantiate_type(ty, Some(mapper)))
            .transpose()?;
        let target_name = self
            .checker
            .mapped_name(target)?
            .map(|ty| self.checker.instantiate_type(ty, Some(mapper)))
            .transpose()?;
        if source_name != target_name {
            return Ok(tr::FALSE);
        }
        let source_template = self.checker.mapped_template(source)?;
        let source_template = self
            .checker
            .instantiate_type(source_template, Some(mapper))?;
        let target_template = self.checker.mapped_template(target)?;
        result &= self.related(source_template, target_template, BOTH, 0)?;
        Ok(result)
    }

    // port: tsc/internal/checker/relater.go:Relater.structuredTypeRelatedToWorker
    pub(crate) fn generic_mapped_target(
        &mut self,
        source: TypeId,
        target: TypeId,
    ) -> Result<Ternary, Error> {
        let name = self.checker.mapped_name(target)?;
        let template = self.checker.mapped_template(target)?;
        let modifiers = self.checker.mapped_modifiers(target)?;
        if modifiers & crate::mapped::EXCLUDE_OPTIONAL != 0 {
            return Ok(tr::FALSE);
        }
        let parameter = self.checker.mapped_parameter(target)?;
        if name.is_none() && self.checker.types.flags(template)? & tf::INDEXED_ACCESS != 0 {
            let data = self.checker.types.indexed_access(template)?;
            if data.object_type == source && data.index_type == parameter {
                return Ok(tr::TRUE);
            }
        }
        if self.checker.is_generic_mapped_type(source)? {
            return Ok(tr::FALSE);
        }
        let target_keys = match name {
            Some(name) => name,
            None => self.checker.mapped_constraint(target)?,
        };
        let source_keys = self
            .checker
            .get_index_type(source, crate::indexes::NO_INDEX_SIGNATURES)?;
        let optional = modifiers & crate::mapped::INCLUDE_OPTIONAL != 0;
        let filtered = if optional {
            Some(
                self.checker
                    .get_intersection_type(&[target_keys, source_keys])?,
            )
        } else {
            None
        };
        let applicable = match filtered {
            Some(filtered) => self.checker.types.flags(filtered)? & tf::NEVER == 0,
            None => self.related(target_keys, source_keys, BOTH, 0)? != tr::FALSE,
        };
        if !applicable {
            return Ok(tr::FALSE);
        }
        let template = self.checker.mapped_template(target)?;
        let non_null = self.checker.filter_type_flags(template, !tf::NULLABLE)?;
        if name.is_none()
            && self.checker.types.flags(non_null)? & tf::INDEXED_ACCESS != 0
            && self.checker.types.indexed_access(non_null)?.index_type == parameter
        {
            return self.related(
                source,
                self.checker.types.indexed_access(non_null)?.object_type,
                TARGET,
                0,
            );
        }
        let index = if name.is_some() {
            filtered.unwrap_or(target_keys)
        } else if let Some(filtered) = filtered {
            self.checker.get_intersection_type(&[filtered, parameter])?
        } else {
            parameter
        };
        let indexed = self
            .checker
            .get_indexed_access_type(source, index, 0, None, None)?;
        self.related(indexed, template, BOTH, 0)
    }
}
