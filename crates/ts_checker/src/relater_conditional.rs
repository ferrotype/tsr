use crate::relater::{Relater, BOTH, SOURCE, TARGET};
use crate::{ternary as tr, type_flags as tf, Error, RelationKind, Ternary, TypeId};

impl Relater<'_> {
    pub(crate) fn conditional_identity(
        &mut self,
        source: TypeId,
        target: TypeId,
    ) -> Result<Ternary, Error> {
        let a = *self.checker.types.conditional(source)?;
        let b = *self.checker.types.conditional(target)?;
        if self.checker.conditional_root(a.root)?.distributive
            != self.checker.conditional_root(b.root)?.distributive
        {
            return Ok(tr::FALSE);
        }
        let mut result = self.related(a.check_type, b.check_type, BOTH, 0)?;
        if result != tr::FALSE {
            result &= self.related(a.extends_type, b.extends_type, BOTH, 0)?;
        }
        if result != tr::FALSE {
            let a = self.checker.conditional_true_type(source, false)?;
            let b = self.checker.conditional_true_type(target, false)?;
            result &= self.related(a, b, BOTH, 0)?;
        }
        if result != tr::FALSE {
            let a = self.checker.conditional_false_type(source)?;
            let b = self.checker.conditional_false_type(target)?;
            result &= self.related(a, b, BOTH, 0)?;
        }
        Ok(result)
    }
    pub(crate) fn conditional_target(
        &mut self,
        source: TypeId,
        target: TypeId,
        intersection: u32,
    ) -> Result<Ternary, Error> {
        if self
            .checker
            .deeply_nested_type(target, &self.frame().target_stack, 10)?
        {
            return Ok(tr::MAYBE);
        }
        let data = *self.checker.types.conditional(target)?;
        let root = self.checker.conditional_root(data.root)?.clone();
        if !root.infer_parameters.is_empty()
            || self.checker.distribution_dependent(data.root)?
            || self.checker.types.flags(source)? & tf::CONDITIONAL != 0
                && self.checker.types.conditional(source)?.root == data.root
        {
            return Ok(tr::FALSE);
        }
        let a = self.checker.permissive_instantiation(data.check_type)?;
        let b = self.checker.permissive_instantiation(data.extends_type)?;
        let skip_true = !self
            .checker
            .is_type_related_to(a, b, RelationKind::Assignable)?;
        let skip_false = if skip_true {
            false
        } else {
            let a = self.checker.restrictive_instantiation(data.check_type)?;
            let b = self.checker.restrictive_instantiation(data.extends_type)?;
            self.checker
                .is_type_related_to(a, b, RelationKind::Assignable)?
        };
        let mut result = if skip_true {
            tr::TRUE
        } else {
            let yes = self.checker.conditional_true_type(target, false)?;
            self.related(source, yes, TARGET, intersection)?
        };
        if result != tr::FALSE && !skip_false {
            let no = self.checker.conditional_false_type(target)?;
            result &= self.related(source, no, TARGET, intersection)?;
        }
        Ok(result)
    }
    pub(crate) fn conditional_source(
        &mut self,
        source: TypeId,
        target: TypeId,
    ) -> Result<Ternary, Error> {
        if self
            .checker
            .deeply_nested_type(source, &self.frame().source_stack, 10)?
        {
            return Ok(tr::MAYBE);
        }
        if self.checker.types.flags(target)? & tf::CONDITIONAL != 0 {
            let a = *self.checker.types.conditional(source)?;
            let b = *self.checker.types.conditional(target)?;
            let parameters = self
                .checker
                .conditional_root(a.root)?
                .infer_parameters
                .clone();
            let mut mapper = None;
            let mut extends = a.extends_type;
            if !parameters.is_empty() {
                let context = self.checker.new_inference_context(&parameters, None, 0)?;
                self.checker.set_inference_comparer(context, self.frame)?;
                self.checker.infer_types(
                    context,
                    b.extends_type,
                    extends,
                    crate::inference::priority::NO_CONSTRAINTS
                        | crate::inference::priority::ALWAYS_STRICT,
                    false,
                )?;
                mapper = Some(self.checker.inference_context(context)?.mapper);
                extends = self.checker.instantiate_type(extends, mapper)?;
            }
            if self
                .checker
                .is_type_related_to(extends, b.extends_type, RelationKind::Identity)?
                && (self.related(a.check_type, b.check_type, BOTH, 0)? != tr::FALSE
                    || self.related(b.check_type, a.check_type, BOTH, 0)? != tr::FALSE)
            {
                let yes_a = self.checker.conditional_true_type(source, false)?;
                let yes_a = self.checker.instantiate_type(yes_a, mapper)?;
                let yes_b = self.checker.conditional_true_type(target, false)?;
                let mut result = self.related(yes_a, yes_b, BOTH, 0)?;
                if result != tr::FALSE {
                    let no_a = self.checker.conditional_false_type(source)?;
                    let no_b = self.checker.conditional_false_type(target)?;
                    result &= self.related(no_a, no_b, BOTH, 0)?;
                }
                if result != tr::FALSE {
                    return Ok(result);
                }
            }
        }
        let constraint = self.checker.default_conditional_constraint(source)?;
        let result = self.related(constraint, target, SOURCE, 0)?;
        if result != tr::FALSE {
            return Ok(result);
        }
        if self.checker.types.flags(target)? & tf::CONDITIONAL == 0
            && self.checker.has_non_circular_base_constraint(source)?
        {
            if let Some(constraint) = self.checker.distributive_conditional_constraint(source)? {
                return self.related(constraint, target, SOURCE, 0);
            }
        }
        Ok(tr::FALSE)
    }
}
