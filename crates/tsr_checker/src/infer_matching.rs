use crate::{
    infer_types::InferenceRun, inference::priority as p, object_flags as of, type_flags as tf,
    CheckerState, Error, RelationKind, TypeId,
};

#[derive(Clone, Copy)]
pub(crate) enum MatchKind {
    BaseIdentity,
    Close,
    Identity,
}

impl CheckerState {
    // port: tsc/internal/checker/inference.go:Checker.isTypeOrBaseIdenticalTo
    // port: tsc/internal/checker/inference.go:Checker.isTypeCloselyMatchedBy
    fn inference_types_match(
        &mut self,
        source: TypeId,
        target: TypeId,
        kind: MatchKind,
    ) -> Result<bool, Error> {
        let s = *self.types.get(source)?;
        let t = *self.types.get(target)?;
        match kind {
            MatchKind::Identity => self.is_type_related_to(source, target, RelationKind::Identity),
            MatchKind::BaseIdentity => {
                if target == self.builtins.missing_type {
                    return Ok(source == target);
                }
                Ok(
                    self.is_type_related_to(source, target, RelationKind::Identity)?
                        || t.flags & tf::STRING != 0 && s.flags & tf::STRING_LITERAL != 0
                        || t.flags & tf::NUMBER != 0 && s.flags & tf::NUMBER_LITERAL != 0,
                )
            }
            MatchKind::Close => {
                if s.flags & t.flags & tf::OBJECT != 0 && s.symbol.is_some() && s.symbol == t.symbol
                {
                    return Ok(true);
                }
                Ok(
                    match (self.types.alias_of(source)?, self.types.alias_of(target)?) {
                        (Some(s), Some(t)) => !s.type_arguments.is_empty() && s.symbol == t.symbol,
                        _ => false,
                    },
                )
            }
        }
    }

    // port: tsc/internal/checker/inference.go:Checker.inferFromMatchingTypes
    pub(crate) fn infer_matching_types(
        &mut self,
        run: &mut InferenceRun,
        sources: &[TypeId],
        targets: &[TypeId],
        kind: MatchKind,
    ) -> Result<(Vec<TypeId>, Vec<TypeId>), Error> {
        let sort = matches!(kind, MatchKind::Close);
        let mut matched_sources = Vec::new();
        let mut matched_targets = Vec::new();
        for &target in targets {
            for &source in sources {
                if self.inference_types_match(source, target, kind)? {
                    if !sort {
                        self.infer_from_types(run, source, target)?;
                    }
                    if !matched_sources.contains(&source) {
                        matched_sources.push(source);
                    }
                    if !matched_targets.contains(&target) {
                        matched_targets.push(target);
                    }
                }
            }
        }
        if sort {
            // Fallible insertion sort keeps the source comparator's tie-break;
            // these are union constituents, not the whole type store.
            let mut depths = Vec::with_capacity(matched_targets.len());
            for &target in &matched_targets {
                depths.push(self.inference_type_depth(target, 3)?);
            }
            for i in 1..matched_targets.len() {
                let mut j = i;
                while j > 0
                    && (depths[j] > depths[j - 1]
                        || depths[j] == depths[j - 1]
                            && self
                                .compare_types(matched_targets[j], matched_targets[j - 1])?
                                .is_lt())
                {
                    matched_targets.swap(j, j - 1);
                    depths.swap(j, j - 1);
                    j -= 1;
                }
            }
            for &target in &matched_targets {
                for &source in &matched_sources {
                    if self.inference_types_match(source, target, kind)? {
                        self.infer_from_types(run, source, target)?;
                    }
                }
            }
        }
        Ok((
            sources
                .iter()
                .copied()
                .filter(|t| !matched_sources.contains(t))
                .collect(),
            targets
                .iter()
                .copied()
                .filter(|t| !matched_targets.contains(t))
                .collect(),
        ))
    }

    // port: tsc/internal/checker/inference.go:getTypeDepth
    fn inference_type_depth(&mut self, ty: TypeId, maximum: usize) -> Result<usize, Error> {
        if maximum == 0 {
            return Ok(0);
        }
        let (types, increment) = if let Some(alias) = self
            .types
            .alias_of(ty)?
            .filter(|a| !a.type_arguments.is_empty())
        {
            (alias.type_arguments.clone(), 1)
        } else if self.types.object_flags(ty)? & of::REFERENCE != 0 {
            (self.get_type_arguments(ty)?, 1)
        } else if self.types.flags(ty)? & tf::UNION_OR_INTERSECTION != 0 {
            (self.types.types_of(ty)?.to_vec().into(), 0)
        } else {
            return Ok(0);
        };
        if types.is_empty() {
            return Ok(0);
        }
        let mut depth = 0;
        for &part in types.iter() {
            depth = depth.max(self.inference_type_depth(part, maximum - increment)?);
        }
        Ok(depth + increment)
    }

    // port: tsc/internal/checker/inference.go:Checker.inferToMultipleTypes
    pub(crate) fn infer_multiple_types(
        &mut self,
        run: &mut InferenceRun,
        source: TypeId,
        targets: &[TypeId],
        flags: crate::TypeFlags,
    ) -> Result<(), Error> {
        let mut variable_count = 0;
        if flags & tf::UNION != 0 {
            let sources = if self.types.flags(source)? & tf::UNION != 0 {
                self.types.types_of(source)?.to_vec()
            } else {
                vec![source]
            };
            let mut matched = vec![false; sources.len()];
            let mut circular = false;
            let mut naked = None;
            for &target in targets {
                if self.inference_index(run, target)?.is_some() {
                    naked = Some(target);
                    variable_count += 1;
                } else {
                    for (i, &part) in sources.iter().enumerate() {
                        let saved = run.inference_priority;
                        run.inference_priority = p::MAX;
                        let result = self.infer_from_types(run, part, target);
                        matched[i] |= run.inference_priority == run.priority;
                        circular |= run.inference_priority == p::CIRCULARITY;
                        run.inference_priority = run.inference_priority.min(saved);
                        result?;
                    }
                }
            }
            if variable_count == 0 {
                let mut variable = None;
                for &target in targets {
                    if self.types.flags(target)? & tf::INTERSECTION == 0 {
                        return Ok(());
                    }
                    let mut found = None;
                    for &part in self.types.types_of(target)? {
                        if self.inference_index(run, part)?.is_some() {
                            found = Some(part);
                            break;
                        }
                    }
                    if found.is_none() || variable.is_some() && variable != found {
                        return Ok(());
                    }
                    variable = found;
                }
                if let Some(variable) = variable {
                    self.infer_with_priority(run, source, variable, p::NAKED)?;
                }
                return Ok(());
            }
            if variable_count == 1 && !circular {
                let unmatched: Vec<_> = sources
                    .into_iter()
                    .zip(matched)
                    .filter_map(|(s, matched)| (!matched).then_some(s))
                    .collect();
                if !unmatched.is_empty() {
                    let source = self.get_union_type(&unmatched)?;
                    return self.infer_from_types(
                        run,
                        source,
                        naked.expect("one naked inference variable"),
                    );
                }
            }
        } else {
            for &target in targets {
                if self.inference_index(run, target)?.is_some() {
                    variable_count += 1;
                } else {
                    self.infer_from_types(run, source, target)?;
                }
            }
        }
        if flags & tf::INTERSECTION != 0 && variable_count == 1
            || flags & tf::INTERSECTION == 0 && variable_count > 0
        {
            for &target in targets {
                if self.inference_index(run, target)?.is_some() {
                    self.infer_with_priority(run, source, target, p::NAKED)?;
                }
            }
        }
        Ok(())
    }

    // port: tsc/internal/checker/inference.go:Checker.inferToConditionalType
    pub(crate) fn infer_to_conditional(
        &mut self,
        run: &mut InferenceRun,
        source: TypeId,
        target: TypeId,
    ) -> Result<(), Error> {
        if self.types.flags(source)? & tf::CONDITIONAL != 0 {
            let s = *self.types.conditional(source)?;
            let t = *self.types.conditional(target)?;
            self.infer_from_types(run, s.check_type, t.check_type)?;
            self.infer_from_types(run, s.extends_type, t.extends_type)?;
            let s = self.conditional_true_type(source, false)?;
            let t = self.conditional_true_type(target, false)?;
            self.infer_from_types(run, s, t)?;
            let s = self.conditional_false_type(source)?;
            let t = self.conditional_false_type(target)?;
            self.infer_from_types(run, s, t)
        } else {
            let targets = [
                self.conditional_true_type(target, false)?,
                self.conditional_false_type(target)?,
            ];
            let saved = run.priority;
            if run.contravariant {
                run.priority |= p::CONTRAVARIANT_CONDITIONAL;
            }
            let result =
                self.infer_multiple_types(run, source, &targets, self.types.flags(target)?);
            run.priority = saved;
            result
        }
    }
}
