use std::borrow::Cow;

use crate::{
    infer_types::InferenceRun, inference::priority as p, object_flags as of, type_flags as tf,
    CheckerState, Error, RelationKind, TypeId,
};

type UnmatchedTypes<'s, 't> = (Cow<'s, [TypeId]>, Cow<'t, [TypeId]>);

#[derive(Clone, Copy)]
pub(crate) enum MatchKind {
    BaseIdentity,
    Close,
    Identity,
}

/// Keep the pin's first-match order while avoiding a linear membership scan
/// for every pair in dense unions. Small matches keep the existing vector-only
/// path; the set is allocated only after that vector reaches the threshold.
#[derive(Default)]
struct MatchedTypes {
    ordered: Vec<TypeId>,
    membership: Option<crate::types::Set<TypeId>>,
}
impl MatchedTypes {
    const LINEAR_LIMIT: usize = 16;

    fn insert(&mut self, ty: TypeId) {
        if self.membership.is_none() && self.ordered.len() < Self::LINEAR_LIMIT {
            if !self.ordered.contains(&ty) {
                self.ordered.push(ty);
            }
            return;
        }
        let membership = self
            .membership
            .get_or_insert_with(|| self.ordered.iter().copied().collect());
        if membership.insert(ty) {
            self.ordered.push(ty);
        }
    }

    fn contains(&self, ty: TypeId) -> bool {
        match &self.membership {
            Some(membership) => membership.contains(&ty),
            None => self.ordered.contains(&ty),
        }
    }

    fn remainder<'a>(&self, types: &'a [TypeId]) -> Cow<'a, [TypeId]> {
        if self.ordered.is_empty() {
            Cow::Borrowed(types)
        } else {
            Cow::Owned(
                types
                    .iter()
                    .copied()
                    .filter(|&ty| !self.contains(ty))
                    .collect(),
            )
        }
    }
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
    pub(crate) fn infer_matching_types<'s, 't>(
        &mut self,
        run: &mut InferenceRun,
        sources: &'s [TypeId],
        targets: &'t [TypeId],
        kind: MatchKind,
    ) -> Result<UnmatchedTypes<'s, 't>, Error> {
        let sort = matches!(kind, MatchKind::Close);
        let mut matched_sources = MatchedTypes::default();
        let mut matched_targets = MatchedTypes::default();
        for &target in targets {
            for &source in sources {
                if self.inference_types_match(source, target, kind)? {
                    if !sort {
                        self.infer_from_types(run, source, target)?;
                    }
                    matched_sources.insert(source);
                    matched_targets.insert(target);
                }
            }
        }
        if sort {
            // Fallible insertion sort keeps the source comparator's tie-break;
            // these are union constituents, not the whole type store.
            let mut depths = Vec::with_capacity(matched_targets.ordered.len());
            for &target in &matched_targets.ordered {
                depths.push(self.inference_type_depth(target, 3)?);
            }
            for i in 1..matched_targets.ordered.len() {
                let mut j = i;
                while j > 0
                    && (depths[j] > depths[j - 1]
                        || depths[j] == depths[j - 1]
                            && self
                                .compare_types(
                                    matched_targets.ordered[j],
                                    matched_targets.ordered[j - 1],
                                )?
                                .is_lt())
                {
                    matched_targets.ordered.swap(j, j - 1);
                    depths.swap(j, j - 1);
                    j -= 1;
                }
            }
            for &target in &matched_targets.ordered {
                for &source in &matched_sources.ordered {
                    if self.inference_types_match(source, target, kind)? {
                        self.infer_from_types(run, source, target)?;
                    }
                }
            }
        }
        Ok((
            matched_sources.remainder(sources),
            matched_targets.remainder(targets),
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

#[cfg(test)]
mod tests {
    use super::*;
    use tsr_ast::{symbol_flags as sf, JsString};

    // Frozen pre-optimization Rust algorithm, used as a differential model.
    // This checks traversal and inference effects, not independent Go parity.
    impl CheckerState {
        fn baseline_matching_types(
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
    }

    fn run(context: crate::InferenceId, target: TypeId) -> InferenceRun {
        InferenceRun {
            context,
            original_target: target,
            priority: p::NONE,
            inference_priority: p::MAX,
            contravariant: false,
            bivariant: false,
            propagation: None,
            visited: crate::types::Map::default(),
            source_stack: Vec::new(),
            target_stack: Vec::new(),
            expanding: 0,
        }
    }

    fn object(state: &mut CheckerState, symbol: tsr_arena::SymbolId, value: TypeId) -> TypeId {
        let name = JsString::from_bytes(b"p".as_slice());
        let property = state.new_symbol(sf::PROPERTY, name.clone()).unwrap();
        state.value_symbol_links.probe_entry(property).resolved_type = Some(value);
        let mut table = tsr_ast::SymbolTable::default();
        table.insert(name, Some(property));
        let table = state.alloc_symbol_table(table);
        let ty = state
            .new_anonymous_type(Some(symbol), Some(table), &[], &[], &[])
            .unwrap();
        state.types.get_mut(ty).unwrap().object_flags |=
            of::COULD_CONTAIN_TYPE_VARIABLES | of::COULD_CONTAIN_TYPE_VARIABLES_COMPUTED;
        ty
    }

    fn compare(
        state: &mut CheckerState,
        parameters: &[TypeId],
        sources: &[TypeId],
        targets: &[TypeId],
        kind: MatchKind,
    ) -> Result<(), Error> {
        let target = targets
            .first()
            .copied()
            .unwrap_or(state.builtins.unknown_type);
        let expected_context = state.new_inference_context(parameters, None, 0).unwrap();
        let actual_context = state.new_inference_context(parameters, None, 0).unwrap();
        let mut expected_run = run(expected_context, target);
        let mut actual_run = run(actual_context, target);
        let expected = state.baseline_matching_types(&mut expected_run, sources, targets, kind);
        let actual = state
            .infer_matching_types(&mut actual_run, sources, targets, kind)
            .map(|(s, t)| (s.into_owned(), t.into_owned()));
        assert_eq!(actual, expected);
        assert_eq!(
            actual_run.inference_priority,
            expected_run.inference_priority
        );
        let expected_inferences = &state
            .inference_context(expected_context)
            .unwrap()
            .inferences;
        let actual = &state.inference_context(actual_context).unwrap().inferences;
        for (a, e) in actual.iter().zip(expected_inferences) {
            assert_eq!(a.candidates, e.candidates);
            assert_eq!(a.contra_candidates, e.contra_candidates);
            assert_eq!(
                (a.priority, a.top_level, a.inferred, a.fixed),
                (e.priority, e.top_level, e.inferred, e.fixed)
            );
        }
        expected.map(|_| ())
    }

    #[test]
    fn matching_preserves_sparse_dense_duplicate_and_failure_inference_order() {
        let (_counters, _generation, _identity, owner) = crate::tests::owner();
        let mut operation = owner.operation().unwrap();
        let state = operation.state_mut();
        state
            .query
            .global_types
            .insert("Array", state.builtins.empty_generic_type);
        state
            .query
            .global_types
            .insert("ReadonlyArray", state.builtins.empty_generic_type);
        let symbol = state
            .new_symbol(sf::TYPE_LITERAL, JsString::from_bytes(b"Box".as_slice()))
            .unwrap();
        let mut parameters = Vec::new();
        let mut sources = Vec::new();
        let mut targets = Vec::new();
        for index in 0..24 {
            let parameter = state.new_type_parameter(None).unwrap();
            parameters.push(parameter);
            let value = state
                .get_string_literal_type(JsString::from_bytes(index.to_string().as_bytes()))
                .unwrap();
            sources.push(object(state, symbol, value));
            targets.push(object(state, symbol, parameter));
        }
        let number = state.builtins.number_type;
        let string = state.builtins.string_type;
        let boolean = state.builtins.boolean_type;
        let sparse_sources = [sources[0], number, sources[0], string];
        let sparse_targets = [targets[1], targets[0], targets[1], number, boolean];
        for kind in [
            MatchKind::Identity,
            MatchKind::BaseIdentity,
            MatchKind::Close,
        ] {
            compare(state, &parameters, &[], &sparse_targets, kind).unwrap();
            compare(state, &parameters, &sparse_sources, &sparse_targets, kind).unwrap();
            compare(state, &parameters, &targets[..2], &targets[..2], kind).unwrap();
        }
        sources.reverse();
        sources.extend_from_within(0..4);
        targets.reverse();
        targets.extend_from_within(0..4);
        compare(state, &parameters, &sources, &targets, MatchKind::Close).unwrap();
        let invalid = TypeId::next(0, state.types.len() + 100).unwrap();
        for kind in [MatchKind::Identity, MatchKind::Close] {
            assert!(compare(
                state,
                &parameters,
                &[sources[0], invalid],
                &targets[..2],
                kind,
            )
            .is_err());
        }
    }

    #[test]
    fn no_matches_borrow_both_input_lists() {
        let (_counters, _generation, _identity, owner) = crate::tests::owner();
        let mut operation = owner.operation().unwrap();
        let state = operation.state_mut();
        let sources = [state.builtins.string_type];
        let targets = [state.builtins.number_type];
        let context = state.new_inference_context(&[], None, 0).unwrap();
        let mut run = run(context, targets[0]);
        let (s, t) = state
            .infer_matching_types(&mut run, &sources, &targets, MatchKind::Close)
            .unwrap();
        assert!(matches!(s, Cow::Borrowed(_)));
        assert!(matches!(t, Cow::Borrowed(_)));
        assert_eq!(s.as_ptr(), sources.as_ptr());
        assert_eq!(t.as_ptr(), targets.as_ptr());
    }
}
