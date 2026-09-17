//! One inference traversal; priority, recursion assumptions and variance are
//! operation state, while candidate lists belong to the retained context.
use crate::{
    inference::priority as p, object_flags as of, type_flags as tf, variance_flags as vf,
    CheckerState, Error, InferenceId, TypeId,
};

pub(crate) struct InferenceRun {
    pub context: InferenceId,
    pub original_target: TypeId,
    pub priority: i32,
    pub inference_priority: i32,
    pub contravariant: bool,
    pub bivariant: bool,
    pub propagation: Option<TypeId>,
    pub visited: crate::types::Map<(TypeId, TypeId), i32>,
    pub source_stack: Vec<TypeId>,
    pub target_stack: Vec<TypeId>,
    pub expanding: u32,
}
impl CheckerState {
    // port: tsc/internal/checker/inference.go:Checker.inferTypes
    pub(crate) fn infer_types(
        &mut self,
        context: InferenceId,
        source: TypeId,
        target: TypeId,
        priority: i32,
        contravariant: bool,
    ) -> Result<(), Error> {
        let mut run = InferenceRun {
            context,
            original_target: target,
            priority,
            inference_priority: p::MAX,
            contravariant,
            bivariant: false,
            propagation: None,
            visited: crate::types::Map::default(),
            source_stack: Vec::new(),
            target_stack: Vec::new(),
            expanding: 0,
        };
        self.infer_from_types(&mut run, source, target)
    }
    pub(crate) fn inference_index(
        &self,
        run: &InferenceRun,
        ty: TypeId,
    ) -> Result<Option<usize>, Error> {
        if self.types.flags(ty)? & tf::TYPE_VARIABLE == 0 {
            return Ok(None);
        }
        Ok(self
            .inference_context(run.context)?
            .inferences
            .iter()
            .position(|i| i.parameter == ty))
    }
    pub(crate) fn infer_from_types(
        &mut self,
        run: &mut InferenceRun,
        source: TypeId,
        target: TypeId,
    ) -> Result<(), Error> {
        stacker::maybe_grow(128 * 1024, 2 * 1024 * 1024, || {
            self.infer_from_types_worker(run, source, target)
        })
    }
    // port: tsc/internal/checker/inference.go:Checker.inferFromTypes
    fn infer_from_types_worker(
        &mut self,
        run: &mut InferenceRun,
        mut source: TypeId,
        mut target: TypeId,
    ) -> Result<(), Error> {
        if !self.could_contain_type_variables(target)? || self.is_no_infer_type(target)? {
            return Ok(());
        }
        if source == self.builtins.wildcard_type || source == self.builtins.blocked_string_type {
            let saved = run.propagation.replace(source);
            let result = self.infer_from_types(run, target, target);
            run.propagation = saved;
            return result;
        }
        if let (Some(a), Some(b)) = (
            self.types.alias_of(source)?.cloned(),
            self.types.alias_of(target)?.cloned(),
        ) {
            if a.symbol == b.symbol {
                if !a.type_arguments.is_empty() || !b.type_arguments.is_empty() {
                    let parameters = self
                        .query
                        .type_aliases
                        .try_get(a.symbol)
                        .and_then(|l| l.parameters.clone())
                        .ok_or(Error::MissingLink("inference alias parameters"))?;
                    let in_js = match self.symbol(a.symbol)?.value_declaration() {
                        Some(node) => {
                            self.node(node)?.flags() & ts_ast::node_flags::JAVA_SCRIPT_FILE != 0
                        }
                        None => false,
                    };
                    let a_types =
                        self.fill_missing_type_arguments(&a.type_arguments, &parameters, in_js)?;
                    let b_types =
                        self.fill_missing_type_arguments(&b.type_arguments, &parameters, in_js)?;
                    let variances = self.alias_variances(a.symbol)?;
                    self.infer_arguments(run, &a_types, &b_types, &variances)?;
                }
                return Ok(());
            }
        }
        if source == target && self.types.flags(source)? & tf::UNION_OR_INTERSECTION != 0 {
            for part in self.types.types_of(source)?.to_vec() {
                self.infer_from_types(run, part, part)?;
            }
            return Ok(());
        }
        if self.types.flags(target)? & tf::UNION != 0 {
            let sources = if self.types.flags(source)? & tf::UNION != 0 {
                self.types.types_of(source)?.to_vec()
            } else {
                vec![source]
            };
            let targets = self.types.types_of(target)?.to_vec();
            let (sources, targets) = self.infer_matching_types(
                run,
                &sources,
                &targets,
                crate::infer_matching::MatchKind::BaseIdentity,
            )?;
            let (sources, targets) = self.infer_matching_types(
                run,
                &sources,
                &targets,
                crate::infer_matching::MatchKind::Close,
            )?;
            if targets.is_empty() {
                return Ok(());
            }
            target = self.get_union_type(&targets)?;
            if sources.is_empty() {
                return self.infer_with_priority(run, source, target, p::NAKED);
            }
            source = self.get_union_type(&sources)?;
        } else if self.types.flags(target)? & tf::INTERSECTION != 0 {
            let targets = self.types.types_of(target)?.to_vec();
            let mut all_objects = true;
            for &part in &targets {
                if self.types.flags(part)? & tf::OBJECT == 0 || self.is_generic_type(part)? {
                    all_objects = false;
                    break;
                }
            }
            if !all_objects && self.types.flags(source)? & tf::UNION == 0 {
                let sources = if self.types.flags(source)? & tf::INTERSECTION != 0 {
                    self.types.types_of(source)?.to_vec()
                } else {
                    vec![source]
                };
                let (sources, targets) = self.infer_matching_types(
                    run,
                    &sources,
                    &targets,
                    crate::infer_matching::MatchKind::Identity,
                )?;
                if sources.is_empty() || targets.is_empty() {
                    return Ok(());
                }
                source = self.get_intersection_type(&sources)?;
                target = self.get_intersection_type(&targets)?;
            }
        }
        if self.types.flags(target)? & (tf::INDEXED_ACCESS | tf::SUBSTITUTION) != 0 {
            if self.is_no_infer_type(target)? {
                return Ok(());
            }
            target = self.actual_type_variable(target)?;
        }
        if self.types.flags(target)? & tf::TYPE_VARIABLE != 0 {
            // SkipDirectInferenceNodes are populated by P4 expression contexts;
            // type-only inference does not create inference-blocked AST nodes.
            if let Some(index) = self.inference_index(run, target)? {
                if self.types.object_flags(source)? & of::NON_INFERRABLE_TYPE != 0
                    || source == self.builtins.non_inferrable_any_type
                {
                    return Ok(());
                }
                if !self.inference_context(run.context)?.inferences[index].fixed {
                    let candidate = run.propagation.unwrap_or(source);
                    if candidate == self.builtins.blocked_string_type {
                        return Ok(());
                    }
                    let info = &mut self.inference_context_mut(run.context)?.inferences[index];
                    if run.priority < info.priority {
                        info.candidates.clear();
                        info.contra_candidates.clear();
                        info.top_level = true;
                        info.priority = run.priority;
                    }
                    let mut changed = false;
                    if run.priority == info.priority {
                        let candidates = if run.contravariant && !run.bivariant {
                            &mut info.contra_candidates
                        } else {
                            &mut info.candidates
                        };
                        if !candidates.contains(&candidate) {
                            candidates.push(candidate);
                            changed = true;
                        }
                    }
                    if changed {
                        self.clear_cached_inferences(run.context)?;
                    }
                    if run.priority & p::RETURN_TYPE == 0
                        && self.types.flags(target)? & tf::TYPE_PARAMETER != 0
                        && self.inference_context(run.context)?.inferences[index].top_level
                        && !self.type_parameter_at_top_level(run.original_target, target, 0)?
                    {
                        self.inference_context_mut(run.context)?.inferences[index].top_level =
                            false;
                        self.clear_cached_inferences(run.context)?;
                    }
                }
                run.inference_priority = run.inference_priority.min(run.priority);
                return Ok(());
            }
            let simplified = self.simplified_type(target, false)?;
            if simplified != target {
                self.infer_from_types(run, source, simplified)?;
            } else if self.types.flags(target)? & tf::INDEXED_ACCESS != 0 {
                let data = *self.types.indexed_access(target)?;
                let index = self.simplified_type(data.index_type, false)?;
                if self.types.flags(index)? & tf::INSTANTIABLE != 0 {
                    let object = self.simplified_type(data.object_type, false)?;
                    if let Some(simplified) =
                        self.distribute_index_over_object(object, index, false)?
                    {
                        if simplified != target {
                            self.infer_from_types(run, source, simplified)?;
                        }
                    }
                }
            }
        }
        let s = *self.types.get(source)?;
        let t = *self.types.get(target)?;
        if s.object_flags & t.object_flags & of::REFERENCE != 0
            && (self.types.target(source)? == self.types.target(target)?
                || self.is_array_type(source)? && self.is_array_type(target)?)
            && !(self.types.type_reference(source)?.node.is_some()
                && self.types.type_reference(target)?.node.is_some())
        {
            let sources = self.get_type_arguments(source)?;
            let targets = self.get_type_arguments(target)?;
            let variances = self.variances_of(self.types.target(source)?)?;
            return self.infer_arguments(run, &sources, &targets, &variances);
        }
        if s.flags & t.flags & tf::INDEX != 0 {
            return self.infer_contravariant(
                run,
                self.types.target(source)?,
                self.types.target(target)?,
            );
        }
        if (self.is_literal_type(source)? || s.flags & tf::STRING != 0) && t.flags & tf::INDEX != 0
        {
            let empty = self.empty_object_from_literal(source)?;
            let saved = run.priority;
            run.priority |= p::LITERAL_KEYOF;
            let result = self.infer_contravariant(run, empty, self.types.target(target)?);
            run.priority = saved;
            return result;
        }
        if s.flags & t.flags & tf::INDEXED_ACCESS != 0 {
            let a = *self.types.indexed_access(source)?;
            let b = *self.types.indexed_access(target)?;
            self.infer_from_types(run, a.object_type, b.object_type)?;
            return self.infer_from_types(run, a.index_type, b.index_type);
        }
        if s.flags & t.flags & tf::STRING_MAPPING != 0 {
            return if s.symbol == t.symbol {
                self.infer_from_types(run, self.types.target(source)?, self.types.target(target)?)
            } else {
                Ok(())
            };
        }
        if s.flags & tf::SUBSTITUTION != 0 {
            self.infer_from_types(run, self.types.substitution(source)?.base, target)?;
            let intersection = self.substitution_intersection(source)?;
            return self.infer_with_priority(run, intersection, target, p::SUBSTITUTE_SOURCE);
        }
        if t.flags & tf::CONDITIONAL != 0 {
            return self.infer_once(run, source, target, Self::infer_to_conditional);
        }
        if t.flags & tf::UNION_OR_INTERSECTION != 0 {
            let targets = self.types.types_of(target)?.to_vec();
            return self.infer_multiple_types(run, source, &targets, t.flags);
        }
        if s.flags & tf::UNION != 0 {
            for part in self.types.types_of(source)?.to_vec() {
                self.infer_from_types(run, part, target)?;
            }
            return Ok(());
        }
        if t.flags & tf::TEMPLATE_LITERAL != 0 {
            return self.infer_to_template(run, source, target);
        }
        source = self.get_reduced_type(source)?;
        if self.is_generic_mapped_type(source)? && self.is_generic_mapped_type(target)? {
            self.infer_once(run, source, target, Self::infer_generic_mapped)?;
        }
        if !(run.priority & p::NO_CONSTRAINTS != 0
            && self.types.flags(source)? & (tf::INTERSECTION | tf::INSTANTIABLE) != 0)
        {
            let apparent = self.apparent_type(source)?;
            if apparent != source
                && self.types.flags(apparent)? & (tf::OBJECT | tf::INTERSECTION) == 0
            {
                return self.infer_from_types(run, apparent, target);
            }
            source = apparent;
        }
        if self.types.flags(source)? & (tf::OBJECT | tf::INTERSECTION) != 0 {
            self.infer_once(run, source, target, Self::infer_object_types)?;
        }
        Ok(())
    }
    pub(crate) fn infer_arguments(
        &mut self,
        run: &mut InferenceRun,
        sources: &[TypeId],
        targets: &[TypeId],
        variances: &[crate::VarianceFlags],
    ) -> Result<(), Error> {
        for (i, (&source, &target)) in sources.iter().zip(targets).enumerate() {
            if variances
                .get(i)
                .is_some_and(|v| v & vf::VARIANCE_MASK == vf::CONTRAVARIANT)
            {
                self.infer_contravariant(run, source, target)?;
            } else {
                self.infer_from_types(run, source, target)?;
            }
        }
        Ok(())
    }
    pub(crate) fn infer_with_priority(
        &mut self,
        run: &mut InferenceRun,
        source: TypeId,
        target: TypeId,
        priority: i32,
    ) -> Result<(), Error> {
        let saved = run.priority;
        run.priority |= priority;
        let result = self.infer_from_types(run, source, target);
        run.priority = saved;
        result
    }
    pub(crate) fn infer_contravariant(
        &mut self,
        run: &mut InferenceRun,
        source: TypeId,
        target: TypeId,
    ) -> Result<(), Error> {
        run.contravariant = !run.contravariant;
        let result = self.infer_from_types(run, source, target);
        run.contravariant = !run.contravariant;
        result
    }
    // port: tsc/internal/checker/inference.go:Checker.invokeOnce
    fn infer_once(
        &mut self,
        run: &mut InferenceRun,
        source: TypeId,
        target: TypeId,
        action: fn(&mut Self, &mut InferenceRun, TypeId, TypeId) -> Result<(), Error>,
    ) -> Result<(), Error> {
        let key = (source, target);
        if let Some(&priority) = run.visited.get(&key) {
            run.inference_priority = run.inference_priority.min(priority);
            return Ok(());
        }
        run.visited.insert(key, p::CIRCULARITY);
        let saved_priority = run.inference_priority;
        run.inference_priority = p::MAX;
        let expanding = run.expanding;
        run.source_stack.push(source);
        run.target_stack.push(target);
        let result = (|| {
            if self.deeply_nested_type(source, &run.source_stack, 2)? {
                run.expanding |= crate::relater::SOURCE;
            }
            if self.deeply_nested_type(target, &run.target_stack, 2)? {
                run.expanding |= crate::relater::TARGET;
            }
            if run.expanding == crate::relater::BOTH {
                run.inference_priority = p::CIRCULARITY;
                Ok(())
            } else {
                action(self, run, source, target)
            }
        })();
        run.source_stack.pop();
        run.target_stack.pop();
        run.expanding = expanding;
        if result.is_ok() {
            run.visited.insert(key, run.inference_priority);
        } else {
            run.visited.remove(&key);
        }
        run.inference_priority = run.inference_priority.min(saved_priority);
        result
    }
}
