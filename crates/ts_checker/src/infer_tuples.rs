use crate::{
    element_flags as ef, infer_types::InferenceRun, inference::priority as p, CheckerState, Error,
    TypeId,
};

impl CheckerState {
    // port: tsc/internal/checker/inference.go:Checker.inferFromObjectTypes
    pub(crate) fn infer_tuple_types(
        &mut self,
        run: &mut InferenceRun,
        source: TypeId,
        target: TypeId,
    ) -> Result<(), Error> {
        let source_tuple = self.is_tuple_type(source)?;
        let sources = self.get_type_arguments(source)?;
        let targets = self.get_type_arguments(target)?;
        let target_data = self.types.tuple(self.types.target(target)?)?;
        let target_fixed = target_data.fixed_length;
        let target_flags = target_data.combined_flags;
        let infos = target_data.element_infos.clone();
        let source_data = if source_tuple {
            let s = self.types.tuple(self.types.target(source)?)?;
            Some((s.element_infos.clone(), s.fixed_length))
        } else {
            None
        };
        if let Some(s) = &source_data {
            if sources.len() == targets.len()
                && s.0
                    .iter()
                    .zip(infos.iter())
                    .all(|(a, b)| a.flags & ef::VARIABLE == b.flags & ef::VARIABLE)
            {
                for (&s, &t) in sources.iter().zip(targets.iter()) {
                    self.infer_from_types(run, s, t)?;
                }
                return Ok(());
            }
        }
        let (start, end) = match &source_data {
            Some(s) => (
                s.1.min(target_fixed) as usize,
                if target_flags & ef::VARIABLE != 0 {
                    s.0.iter()
                        .rev()
                        .take_while(|i| i.flags & ef::FIXED != 0)
                        .count()
                        .min(
                            infos
                                .iter()
                                .rev()
                                .take_while(|i| i.flags & ef::FIXED != 0)
                                .count(),
                        )
                } else {
                    0
                },
            ),
            None => (0, 0),
        };
        for i in 0..start {
            self.infer_from_types(run, sources[i], targets[i])?;
        }
        if !source_tuple
            || sources.len() - start - end == 1
                && source_data.as_ref().expect("source tuple").0[start].flags & ef::REST != 0
        {
            let rest = sources[start];
            for i in start..targets.len() - end {
                let s = if infos[i].flags & ef::VARIADIC != 0 {
                    self.create_array_type(rest, false)?
                } else {
                    rest
                };
                self.infer_from_types(run, s, targets[i])?;
            }
        } else {
            let middle = targets.len() - start - end;
            if middle == 2 {
                let a = infos[start].flags;
                let b = infos[start + 1].flags;
                if a & b & ef::VARIADIC != 0 {
                    if let Some(index) = self.inference_index(run, targets[start])? {
                        if let Some(arity) =
                            self.inference_context(run.context)?.inferences[index].implied_arity
                        {
                            // The source uses signed slice boundaries. A prefix
                            // beyond the fixed part yields its rest array/[];
                            // a negative end skip is clamped to zero.
                            let s = self.slice_tuple(
                                source,
                                start,
                                (end + sources.len()) as isize - arity as isize,
                            )?;
                            self.infer_from_types(run, s, targets[start])?;
                            let s = self.slice_tuple(source, start + arity, end as isize)?;
                            self.infer_from_types(run, s, targets[start + 1])?;
                        }
                    }
                } else if a & ef::VARIADIC != 0 && b & ef::REST != 0 {
                    if let Some(arity) = self.inferred_fixed_tuple_arity(run, targets[start])? {
                        let s = self.slice_tuple(
                            source,
                            start,
                            sources.len() as isize - (start + arity) as isize,
                        )?;
                        self.infer_from_types(run, s, targets[start])?;
                        if let Some(s) =
                            self.tuple_slice_element_type(source, start + arity, end, false)?
                        {
                            self.infer_from_types(run, s, targets[start + 1])?;
                        }
                    }
                } else if a & ef::REST != 0 && b & ef::VARIADIC != 0 {
                    if let Some(arity) = self.inferred_fixed_tuple_arity(run, targets[start + 1])? {
                        let end_index = sources.len()
                            - infos
                                .iter()
                                .rev()
                                .take_while(|i| i.flags & ef::FIXED != 0)
                                .count();
                        if let Some(start_index) =
                            end_index.checked_sub(arity).filter(|i| *i >= start)
                        {
                            let s_infos = &source_data.as_ref().expect("source tuple").0;
                            let trailing = self.create_tuple_type_ex(
                                &sources[start_index..end_index],
                                &s_infos[start_index..end_index],
                                false,
                            )?;
                            if let Some(s) =
                                self.tuple_slice_element_type(source, start, end + arity, false)?
                            {
                                self.infer_from_types(run, s, targets[start])?;
                            }
                            self.infer_from_types(run, trailing, targets[start + 1])?;
                        }
                    }
                }
            } else if middle == 1 && infos[start].flags & ef::VARIADIC != 0 {
                let priority = if infos[targets.len() - 1].flags & ef::OPTIONAL != 0 {
                    p::SPECULATIVE_TUPLE
                } else {
                    p::NONE
                };
                let source = self.slice_tuple(source, start, end as isize)?;
                self.infer_with_priority(run, source, targets[start], priority)?;
            } else if middle == 1 && infos[start].flags & ef::REST != 0 {
                if let Some(source) = self.tuple_slice_element_type(source, start, end, false)? {
                    self.infer_from_types(run, source, targets[start])?;
                }
            }
        }
        for i in 0..end {
            self.infer_from_types(
                run,
                sources[sources.len() - i - 1],
                targets[targets.len() - i - 1],
            )?;
        }
        Ok(())
    }

    fn inferred_fixed_tuple_arity(
        &mut self,
        run: &InferenceRun,
        ty: TypeId,
    ) -> Result<Option<usize>, Error> {
        let Some(index) = self.inference_index(run, ty)? else {
            return Ok(None);
        };
        let parameter = self.inference_context(run.context)?.inferences[index].parameter;
        let Some(constraint) = self.base_constraint_of_type(parameter)? else {
            return Ok(None);
        };
        if !self.is_tuple_type(constraint)? {
            return Ok(None);
        }
        let data = self.types.tuple(self.types.target(constraint)?)?;
        Ok((data.combined_flags & ef::VARIABLE == 0).then_some(data.fixed_length as usize))
    }

    // port: tsc/internal/checker/relater.go:Checker.sliceTupleType
    pub(crate) fn slice_tuple(
        &mut self,
        ty: TypeId,
        start: usize,
        end_skip: isize,
    ) -> Result<TypeId, Error> {
        let data = self.types.tuple(self.types.target(ty)?)?;
        let fixed = data.fixed_length as usize;
        let infos = data.element_infos.clone();
        if start > fixed {
            if let Some(rest) = self.tuple_slice_element_type(ty, fixed, 0, false)? {
                return self.create_array_type(rest, false);
            }
            return self.create_tuple_type(&[]);
        }
        let arguments = self.get_type_arguments(ty)?;
        let end = arguments.len().saturating_sub(end_skip.max(0) as usize);
        if start >= end {
            return self.create_tuple_type(&[]);
        }
        self.create_tuple_type_ex(&arguments[start..end], &infos[start..end], false)
    }
}
