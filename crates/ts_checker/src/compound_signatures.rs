//! Union/intersection signatures use the same type comparer as ordinary
//! signature identity. Composite return types and predicates stay lazy.
use crate::{
    object_flags as of, signature_flags as sg, signatures::CompositeSignature, ternary as tr,
    type_flags as tf, CheckerState, Error, MapperId, RelationKind, SignatureId, TypeId,
    TypePredicateId,
};
use ts_arena::SymbolId;
use ts_ast::{check_flags as cf, symbol_flags as sf, JsString};

impl CheckerState {
    // port: tsc/internal/checker/checker.go:Checker.resolveUnionTypeMembers
    // port: tsc/internal/checker/checker.go:Checker.resolveIntersectionTypeMembers
    pub(crate) fn resolve_compound_type_members(&mut self, ty: TypeId) -> Result<(), Error> {
        let parts = self.types.compound_types(ty)?.clone();
        if self.types.flags(ty)? & tf::UNION != 0 {
            let mut lists = Vec::with_capacity(parts.len());
            for &part in parts.iter() {
                lists.push(if self.query.global_types.get("Function") == Some(&part) {
                    vec![self.builtins.unknown_signature]
                } else {
                    self.signatures_of_type(part, false)?
                });
            }
            let mut calls = self.union_signatures(&lists)?;
            if calls.is_empty() {
                calls = self.array_member_call_signatures(ty)?;
            }
            lists.clear();
            for &part in parts.iter() {
                lists.push(self.signatures_of_type(part, true)?);
            }
            let constructs = self.union_signatures(&lists)?;
            let indexes = self.union_index_infos(&parts)?;
            return self.set_structured_type_members(ty, None, &calls, &constructs, &indexes);
        }
        let (mixins, mixin_count) = self.find_mixins(&parts)?;
        let mut calls = Vec::new();
        let mut constructs = Vec::new();
        let mut indexes = Vec::new();
        for (i, &part) in parts.iter().enumerate() {
            if !mixins[i] {
                let mut signatures = self.signatures_of_type(part, true)?;
                if mixin_count != 0 {
                    for signature in &mut signatures {
                        let cloned = self.clone_signature(*signature)?;
                        let returned = self.return_type_of_signature(*signature)?;
                        let mut returns = Vec::new();
                        for (j, &other) in parts.iter().enumerate() {
                            if i == j {
                                returns.push(returned);
                            } else if mixins[j] {
                                let sig = self.signatures_of_type(other, true)?[0];
                                returns.push(self.return_type_of_signature(sig)?);
                            }
                        }
                        let returned = self.get_intersection_type(&returns)?;
                        self.signatures.get_mut(cloned)?.resolved_return_type = Some(returned);
                        *signature = cloned;
                    }
                }
                self.append_signatures(&mut constructs, &signatures)?;
            }
            let signatures = self.signatures_of_type(part, false)?;
            self.append_signatures(&mut calls, &signatures)?;
            for index in self.index_infos_of_type(part)? {
                self.append_index_info(&mut indexes, index, false)?;
            }
        }
        self.set_structured_type_members(ty, None, &calls, &constructs, &indexes)
    }

    // port: tsc/internal/checker/checker.go:Checker.cloneSignature
    pub(crate) fn clone_signature(&mut self, signature: SignatureId) -> Result<SignatureId, Error> {
        let s = self.signatures.get(signature)?.clone();
        let id = self.signatures.new_signature(
            s.flags & sg::PROPAGATING_FLAGS,
            s.declaration,
            s.type_parameters,
            s.this_parameter,
            s.parameters,
            None,
            None,
            s.min_argument_count,
        )?;
        let result = self.signatures.get_mut(id)?;
        result.target = s.target;
        result.mapper = s.mapper;
        result.composite = s.composite;
        Ok(id)
    }

    // port: tsc/internal/checker/relater.go:Checker.findMatchingSignature
    fn matching_signature(
        &mut self,
        signatures: &[SignatureId],
        signature: SignatureId,
        partial: bool,
        ignore_return: bool,
    ) -> Result<Option<SignatureId>, Error> {
        let relation = if partial {
            RelationKind::Subtype
        } else {
            RelationKind::Identity
        };
        for &other in signatures {
            if self.compare_signatures_identical(
                other,
                signature,
                partial,
                false,
                ignore_return,
                &mut |state, source, target| {
                    state
                        .is_type_related_to(source, target, relation)
                        .map(|yes| if yes { tr::TRUE } else { tr::FALSE })
                },
            )? != tr::FALSE
            {
                return Ok(Some(other));
            }
        }
        Ok(None)
    }

    // port: tsc/internal/checker/relater.go:Checker.findMatchingSignatures
    fn matching_signatures(
        &mut self,
        lists: &[Vec<SignatureId>],
        signature: SignatureId,
        list_index: usize,
    ) -> Result<Vec<SignatureId>, Error> {
        if self
            .signatures
            .get(signature)?
            .type_parameters
            .as_ref()
            .is_some_and(|p| !p.is_empty())
        {
            if list_index != 0 {
                return Ok(Vec::new());
            }
            for list in &lists[1..] {
                if self
                    .matching_signature(list, signature, false, false)?
                    .is_none()
                {
                    return Ok(Vec::new());
                }
            }
            return Ok(vec![signature]);
        }
        let mut result = Vec::new();
        for (i, list) in lists.iter().enumerate() {
            let found = if i == list_index {
                Some(signature)
            } else {
                match self.matching_signature(list, signature, false, true)? {
                    Some(found) => Some(found),
                    None => self.matching_signature(list, signature, true, true)?,
                }
            };
            let Some(found) = found else {
                return Ok(Vec::new());
            };
            if !result.contains(&found) {
                result.push(found);
            }
        }
        Ok(result)
    }

    // port: tsc/internal/checker/checker.go:Checker.getUnionSignatures
    fn union_signatures(&mut self, lists: &[Vec<SignatureId>]) -> Result<Vec<SignatureId>, Error> {
        let mut result = Vec::new();
        let mut over_one = 0;
        let mut master = 0;
        for (i, list) in lists.iter().enumerate() {
            if list.is_empty() {
                return Ok(Vec::new());
            }
            if list.len() > 1 {
                over_one += 1;
                master = i;
            }
            for &signature in list {
                if !result.is_empty()
                    && self
                        .matching_signature(&result, signature, false, true)?
                        .is_some()
                {
                    continue;
                }
                let matches = self.matching_signatures(lists, signature, i)?;
                if matches.is_empty() {
                    continue;
                }
                let mut combined = signature;
                if matches.len() > 1 {
                    let mut first_this = None;
                    let mut this_types = Vec::new();
                    for &item in &matches {
                        if let Some(symbol) = self.signatures.get(item)?.this_parameter {
                            first_this.get_or_insert(symbol);
                            this_types.push(self.get_type_of_symbol(symbol)?);
                        }
                    }
                    let this = if let Some(symbol) = first_this {
                        let ty = self.get_intersection_type(&this_types)?;
                        Some(self.create_symbol_with_type(symbol, ty)?)
                    } else {
                        None
                    };
                    combined = self.clone_signature(signature)?;
                    let sig = self.signatures.get_mut(combined)?;
                    sig.composite = Some(CompositeSignature {
                        is_union: true,
                        signatures: matches.into(),
                    });
                    sig.target = None;
                    sig.mapper = None;
                    sig.this_parameter = this;
                }
                result.push(combined);
            }
        }
        if result.is_empty() && over_one <= 1 && !lists.is_empty() {
            result.clone_from(&lists[master]);
            for (i, list) in lists.iter().enumerate() {
                if i == master {
                    continue;
                }
                let signature = list[0];
                let parameters = self
                    .signatures
                    .get(signature)?
                    .type_parameters
                    .clone()
                    .unwrap_or_else(|| [].into());
                if !parameters.is_empty() {
                    for &other in &result {
                        let other = self
                            .signatures
                            .get(other)?
                            .type_parameters
                            .clone()
                            .unwrap_or_else(|| [].into());
                        if !other.is_empty()
                            && !self.signature_type_parameters_identical(&parameters, &other)?
                        {
                            return Ok(Vec::new());
                        }
                    }
                }
                for item in &mut result {
                    *item = self.combine_member_signatures(*item, signature, true)?;
                }
            }
        }
        Ok(result)
    }

    // port: tsc/internal/checker/relater.go:Checker.compareTypeParametersIdentical
    pub(crate) fn signature_type_parameters_identical(
        &mut self,
        source: &[TypeId],
        target: &[TypeId],
    ) -> Result<bool, Error> {
        if source.len() != target.len() {
            return Ok(false);
        }
        let mapper = self.new_type_mapper(target, source)?;
        for (&s, &t) in source.iter().zip(target) {
            if s == t {
                continue;
            }
            let sc = self
                .constraint_of_type_parameter(s)?
                .unwrap_or(self.builtins.unknown_type);
            let tc = self
                .constraint_of_type_parameter(t)?
                .unwrap_or(self.builtins.unknown_type);
            let tc = self.instantiate_type(tc, Some(mapper))?;
            if !self.is_type_related_to(sc, tc, RelationKind::Identity)? {
                return Ok(false);
            }
        }
        Ok(true)
    }

    // port: tsc/internal/checker/checker.go:Checker.combineUnionOrIntersectionMemberSignatures
    pub(crate) fn combine_member_signatures(
        &mut self,
        left: SignatureId,
        right: SignatureId,
        union: bool,
    ) -> Result<SignatureId, Error> {
        let l = self.signatures.get(left)?.clone();
        let r = self.signatures.get(right)?.clone();
        let lp = l.type_parameters.as_deref().unwrap_or_default();
        let rp = r.type_parameters.as_deref().unwrap_or_default();
        let parameters = if lp.is_empty() {
            r.type_parameters.clone()
        } else {
            l.type_parameters.clone()
        };
        let mapper = if !lp.is_empty() && !rp.is_empty() {
            Some(self.new_type_mapper(rp, lp)?)
        } else {
            None
        };
        let params = self.combine_signature_parameters(left, right, mapper, union)?;
        let mut flags = (l.flags | r.flags) & (sg::PROPAGATING_FLAGS & !sg::HAS_REST_PARAMETER);
        if let Some(&last) = params.last() {
            if self.symbol(last)?.check_flags() & cf::REST_PARAMETER != 0 {
                flags |= sg::HAS_REST_PARAMETER;
            }
        }
        let this = match (l.this_parameter, r.this_parameter) {
            (None, other) | (other, None) => other,
            (Some(l), Some(r)) => {
                let lt = self.get_type_of_symbol(l)?;
                let rt = self.get_type_of_symbol(r)?;
                let rt = self.instantiate_type(rt, mapper)?;
                let ty = if union {
                    self.get_intersection_type(&[lt, rt])?
                } else {
                    self.get_union_type(&[lt, rt])?
                };
                Some(self.create_symbol_with_type(l, ty)?)
            }
        };
        let result = self.signatures.new_signature(
            flags,
            l.declaration,
            parameters,
            this,
            (!params.is_empty()).then(|| params.into()),
            None,
            None,
            l.min_argument_count.max(r.min_argument_count),
        )?;
        let mut parts = if let Some(composite) = &l.composite {
            if composite.is_union {
                composite.signatures.to_vec()
            } else {
                vec![left]
            }
        } else {
            vec![left]
        };
        parts.push(right);
        let old_mapper = if l.composite.as_ref().is_some_and(|c| c.is_union == union) {
            l.mapper
        } else {
            None
        };
        let mapper = match mapper {
            Some(mapper) => Some(self.combine_type_mappers(old_mapper, mapper)?),
            None => old_mapper,
        };
        let s = self.signatures.get_mut(result)?;
        s.composite = Some(CompositeSignature {
            is_union: union,
            signatures: parts.into(),
        });
        s.mapper = mapper;
        Ok(result)
    }

    // port: tsc/internal/checker/checker.go:Checker.combineUnionOrIntersectionParameters
    fn combine_signature_parameters(
        &mut self,
        left: SignatureId,
        right: SignatureId,
        mapper: Option<MapperId>,
        union: bool,
    ) -> Result<Vec<SymbolId>, Error> {
        let lc = self.parameter_count(left)?;
        let rc = self.parameter_count(right)?;
        let (count, longest, shorter) = if lc >= rc {
            (lc, left, right)
        } else {
            (rc, right, left)
        };
        let has_rest =
            self.effective_rest_parameter(left)? || self.effective_rest_parameter(right)?;
        let extra_rest = has_rest && !self.effective_rest_parameter(longest)?;
        let mut params = Vec::with_capacity(count + usize::from(extra_rest));
        for i in 0..count {
            let mut lt = self
                .parameter_type_at(longest, i)?
                .ok_or(Error::MissingLink("composite parameter"))?;
            if longest == right {
                lt = self.instantiate_type(lt, mapper)?;
            }
            let mut st = self
                .parameter_type_at(shorter, i)?
                .unwrap_or(self.builtins.unknown_type);
            if shorter == right {
                st = self.instantiate_type(st, mapper)?;
            }
            let ty = if union {
                self.get_intersection_type(&[lt, st])?
            } else {
                self.get_union_type(&[lt, st])?
            };
            let rest = has_rest && !extra_rest && i + 1 == count;
            let optional =
                i >= self.min_argument_count(longest)? && i >= self.min_argument_count(shorter)?;
            let ln = if i < lc {
                self.parameter_name_at(left, i)?
            } else {
                JsString::default()
            };
            let rn = if i < rc {
                self.parameter_name_at(right, i)?
            } else {
                JsString::default()
            };
            let name = if ln == rn || rn.is_empty() {
                ln
            } else if ln.is_empty() {
                rn
            } else {
                JsString::default()
            };
            let name = if name.is_empty() {
                JsString::from_bytes(format!("arg{i}").into_bytes())
            } else {
                name
            };
            let flags = if rest {
                cf::REST_PARAMETER
            } else if optional {
                cf::OPTIONAL_PARAMETER
            } else {
                0
            };
            let symbol = self.new_symbol_ex(
                sf::FUNCTION_SCOPED_VARIABLE | if optional && !rest { sf::OPTIONAL } else { 0 },
                name,
                flags,
            )?;
            let ty = if rest {
                self.create_array_type(ty, false)?
            } else {
                ty
            };
            self.value_symbol_links.get_or_default(symbol).resolved_type = Some(ty);
            params.push(symbol);
        }
        if extra_rest {
            let symbol = self.new_symbol_ex(
                sf::FUNCTION_SCOPED_VARIABLE,
                JsString::from_bytes(b"args".as_slice()),
                cf::REST_PARAMETER,
            )?;
            let ty = self
                .parameter_type_at(shorter, count)?
                .unwrap_or(self.builtins.any_type);
            let mut ty = self.create_array_type(ty, false)?;
            if shorter == right {
                ty = self.instantiate_type(ty, mapper)?;
            }
            self.value_symbol_links.get_or_default(symbol).resolved_type = Some(ty);
            params.push(symbol);
        }
        Ok(params)
    }

    // port: tsc/internal/checker/checker.go:Checker.appendSignatures
    fn append_signatures(
        &mut self,
        result: &mut Vec<SignatureId>,
        incoming: &[SignatureId],
    ) -> Result<(), Error> {
        for &signature in incoming {
            if self
                .matching_signature(result, signature, false, false)?
                .is_none()
            {
                result.push(signature);
            }
        }
        Ok(())
    }

    // port: tsc/internal/checker/checker.go:Checker.findMixins
    fn find_mixins(&mut self, types: &[TypeId]) -> Result<(Vec<bool>, usize), Error> {
        let mut flags = Vec::with_capacity(types.len());
        let mut constructors = 0;
        let mut mixins = 0;
        for &ty in types {
            let mixin = self.is_mixin_constructor(ty)?;
            flags.push(mixin);
        }
        for (&ty, &mixin) in types.iter().zip(&flags) {
            constructors += usize::from(!self.signatures_of_type(ty, true)?.is_empty());
            mixins += usize::from(mixin);
        }
        if constructors > 0 && constructors == mixins {
            let first = flags
                .iter()
                .position(|&flag| flag)
                .ok_or(Error::MissingLink("mixin count"))?;
            flags[first] = false;
            mixins -= 1;
        }
        Ok((flags, mixins))
    }

    // port: tsc/internal/checker/checker.go:Checker.isMixinConstructorType
    fn is_mixin_constructor(&mut self, ty: TypeId) -> Result<bool, Error> {
        let signatures = self.signatures_of_type(ty, true)?;
        if signatures.len() != 1 {
            return Ok(false);
        }
        let signature = self.signatures.get(signatures[0])?;
        if signature
            .type_parameters
            .as_ref()
            .is_some_and(|p| !p.is_empty())
            || signature.parameters.as_deref().unwrap_or_default().len() != 1
            || signature.flags & sg::HAS_REST_PARAMETER == 0
        {
            return Ok(false);
        }
        let parameter = signature
            .parameters
            .as_ref()
            .ok_or(Error::MissingLink("mixin parameter"))?[0];
        let ty = self.type_of_parameter(parameter)?;
        Ok(self.types.flags(ty)? & tf::ANY != 0
            || self.is_array_type(ty)?
                && self.get_type_arguments(ty)?.first() == Some(&self.builtins.any_type))
    }

    // port: tsc/internal/checker/relater.go:Checker.getUnionOrIntersectionTypePredicate
    pub(crate) fn compound_type_predicate(
        &mut self,
        signatures: &[SignatureId],
        union: bool,
    ) -> Result<Option<TypePredicateId>, Error> {
        let mut last: Option<crate::signatures::TypePredicate> = None;
        let mut types = Vec::new();
        for &signature in signatures {
            if let Some(predicate) = self.type_predicate_of_signature(signature)? {
                let predicate = self.signatures.predicate(predicate)?.clone();
                if !matches!(
                    predicate.kind,
                    crate::TypePredicateKind::This | crate::TypePredicateKind::Identifier
                ) || last.as_ref().is_some_and(|last| {
                    last.kind != predicate.kind || last.parameter_index != predicate.parameter_index
                }) {
                    return Ok(None);
                }
                types.push(
                    predicate
                        .t
                        .ok_or(Error::MissingLink("composite predicate type"))?,
                );
                last = Some(predicate);
            } else {
                if !union {
                    return Ok(None);
                }
                let returned = self.return_type_of_signature(signature)?;
                if returned != self.builtins.false_type
                    && returned != self.builtins.regular_false_type
                {
                    return Ok(None);
                }
            }
        }
        let Some(mut predicate) = last else {
            return Ok(None);
        };
        predicate.t = Some(if union {
            self.get_union_type(&types)?
        } else {
            self.get_intersection_type(&types)?
        });
        self.signatures.new_type_predicate(predicate).map(Some)
    }

    // port: tsc/internal/checker/checker.go:Checker.getArrayMemberCallSignatures
    fn array_member_call_signatures(&mut self, ty: TypeId) -> Result<Vec<SignatureId>, Error> {
        let parts = self.types.compound_types(ty)?.clone();
        let array = self.array_target(false)?;
        let readonly = self.array_target(true)?;
        let array_symbol = self.types.get(array)?.symbol;
        let readonly_symbol = self.types.get(readonly)?.symbol;
        let (Some(array_symbol), Some(readonly_symbol)) = (array_symbol, readonly_symbol) else {
            return Ok(Vec::new());
        };
        let mut name = None;
        let mut arguments = Vec::new();
        let mut any_readonly = false;
        for &part in parts.iter() {
            let r = *self.types.get(part)?;
            if r.object_flags & of::INSTANTIATED == 0 {
                return Ok(Vec::new());
            }
            let Some(symbol) = r.symbol else {
                return Ok(Vec::new());
            };
            let Some(parent) = self.symbol(symbol)?.parent() else {
                return Ok(Vec::new());
            };
            let parent = self.get_merged_symbol(parent);
            let is_readonly = parent == self.get_merged_symbol(readonly_symbol);
            if !is_readonly && parent != self.get_merged_symbol(array_symbol) {
                return Ok(Vec::new());
            }
            let member_name = self.symbol(symbol)?.name_to_owned();
            if name.as_ref().is_some_and(|name| *name != member_name) {
                return Ok(Vec::new());
            }
            name.get_or_insert(member_name);
            any_readonly |= is_readonly;
        }
        for &part in parts.iter() {
            let symbol = self
                .types
                .get(part)?
                .symbol
                .ok_or(Error::MissingLink("array member symbol"))?;
            let parent = self
                .symbol(symbol)?
                .parent()
                .ok_or(Error::MissingLink("array member parent"))?;
            let target =
                if self.get_merged_symbol(parent) == self.get_merged_symbol(readonly_symbol) {
                    readonly
                } else {
                    array
                };
            let parameter = self
                .types
                .interface(target)?
                .all_type_parameters
                .as_ref()
                .and_then(|p| p.first())
                .copied()
                .ok_or(Error::MissingLink("array type parameter"))?;
            let mapper = self
                .types
                .object(part)?
                .mapper
                .ok_or(Error::MissingLink("array member mapper"))?;
            arguments.push(self.map_type_parameter(parameter, mapper)?);
        }
        let argument = self.get_union_type(&arguments)?;
        let array = self.create_array_type(argument, any_readonly)?;
        let name = name.ok_or(Error::MissingLink("array member name"))?;
        let property = self
            .constituent_property(array, name.as_bytes(), false)?
            .ok_or(Error::MissingLink("array member property"))?;
        let value = self.get_type_of_symbol(property)?;
        self.signatures_of_type(value, false)
    }
}
