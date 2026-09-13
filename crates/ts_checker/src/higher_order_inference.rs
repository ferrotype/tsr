//! Generic function expressions may contribute their own type parameters to an
//! outer generic call's result. Adoption is allowed only for disjoint inference
//! candidates, and keeps the native fixing and second-pass schedule.
use crate::{
    inference::priority, type_flags as tf, CheckerState, Error, InferenceId, SignatureId, TypeId,
};
use ts_arena::NodeId;
use ts_ast::{symbol_flags as sf, JsString};

impl CheckerState {
    // port: tsc/internal/checker/checker.go:Checker.getSingleSignature
    pub(crate) fn single_kind_signature(
        &mut self,
        ty: TypeId,
        construct: bool,
        allow_members: bool,
    ) -> Result<Option<SignatureId>, Error> {
        if self.types.flags(ty)? & tf::OBJECT == 0 {
            return Ok(None);
        }
        self.resolve_type_members(ty)?;
        let data = self.types.structured(ty)?;
        if !allow_members
            && (data.properties.as_ref().is_some_and(|p| !p.is_empty())
                || data.index_infos.as_ref().is_some_and(|p| !p.is_empty()))
        {
            return Ok(None);
        }
        let signatures = data.signatures.as_deref().unwrap_or_default();
        Ok(
            (signatures.len() == 1 && (data.call_signature_count == 0) == construct)
                .then(|| signatures[0]),
        )
    }
    // port: tsc/internal/checker/checker.go:Checker.getSingleCallOrConstructSignature
    fn single_call_or_construct_signature(
        &mut self,
        ty: TypeId,
    ) -> Result<Option<SignatureId>, Error> {
        if let Some(signature) = self.single_kind_signature(ty, false, false)? {
            return Ok(Some(signature));
        }
        self.single_kind_signature(ty, true, false)
    }
    // port: tsc/internal/checker/checker.go:Checker.instantiateTypeWithSingleGenericCallSignature
    pub(crate) fn instantiate_single_generic_function(
        &mut self,
        node: NodeId,
        ty: TypeId,
        mode: u32,
    ) -> Result<TypeId, Error> {
        if mode & (2 | 8) == 0 {
            return Ok(ty);
        }
        let call = self.single_kind_signature(ty, false, true)?;
        let construct = self.single_kind_signature(ty, true, true)?;
        let Some(signature) = call.or(construct) else {
            return Ok(ty);
        };
        let parameters = self
            .signatures
            .get(signature)?
            .type_parameters
            .clone()
            .unwrap_or_else(|| [].into());
        if parameters.is_empty() {
            return Ok(ty);
        }
        let Some(contextual) = self.apparent_contextual_expression_type_ex(node, 2)? else {
            return Ok(ty);
        };
        let contextual = self.non_nullable_type(contextual)?;
        let Some(contextual) = self.single_kind_signature(contextual, call.is_none(), false)?
        else {
            return Ok(ty);
        };
        if self
            .signatures
            .get(contextual)?
            .type_parameters
            .as_ref()
            .is_some_and(|p| !p.is_empty())
        {
            return Ok(ty);
        }
        let context = self.call_inference_at_node(node)?;
        if mode & 8 != 0 {
            if mode & 2 != 0 {
                let context = context.ok_or(Error::MissingLink("skipped generic inference"))?;
                self.inference_context_mut(context)?.flags |= 4;
            }
            return Ok(self.builtins.any_function_type);
        }
        let context = context.ok_or(Error::MissingLink("generic expression inference context"))?;
        let returned = if let Some(outer) = self.inference_context(context)?.signature {
            let returned = self.return_type_of_signature(outer)?;
            self.single_call_or_construct_signature(returned)?
        } else {
            None
        };
        if let Some(returned) = returned {
            if self
                .signatures
                .get(returned)?
                .type_parameters
                .as_ref()
                .is_none_or(|p| p.is_empty())
                && self
                    .inference_context(context)?
                    .inferences
                    .iter()
                    .any(|info| info.candidates.is_empty() && info.contra_candidates.is_empty())
            {
                let unique = self.unique_inferred_type_parameters(context, &parameters)?;
                let instantiated = self.signature_instantiation(signature, &unique, false)?;
                let outer_parameters = self
                    .inference_context(context)?
                    .inferences
                    .iter()
                    .map(|info| info.parameter)
                    .collect::<Vec<_>>();
                let tentative = self.new_inference_context(&outer_parameters, None, 0)?;
                self.for_signature_parameters(
                    instantiated,
                    contextual,
                    &mut |checker, source, target| {
                        checker.infer_types(tentative, source, target, priority::NONE, true)
                    },
                )?;
                if self
                    .inference_context(tentative)?
                    .inferences
                    .iter()
                    .any(|info| !info.candidates.is_empty() || !info.contra_candidates.is_empty())
                {
                    self.for_signature_returns(
                        instantiated,
                        contextual,
                        &mut |checker, source, target| {
                            checker.infer_types(tentative, source, target, priority::NONE, false)
                        },
                    )?;
                    let overlap = self
                        .inference_context(context)?
                        .inferences
                        .iter()
                        .zip(&self.inference_context(tentative)?.inferences)
                        .any(|(a, b)| {
                            (!a.candidates.is_empty() || !a.contra_candidates.is_empty())
                                && (!b.candidates.is_empty() || !b.contra_candidates.is_empty())
                        });
                    if !overlap {
                        let candidates = self.inference_context(tentative)?.inferences.clone();
                        for (target, source) in self
                            .inference_context_mut(context)?
                            .inferences
                            .iter_mut()
                            .zip(candidates)
                        {
                            if target.candidates.is_empty()
                                && target.contra_candidates.is_empty()
                                && (!source.candidates.is_empty()
                                    || !source.contra_candidates.is_empty())
                            {
                                *target = source;
                            }
                        }
                        self.inference_context_mut(context)?
                            .inferred_type_parameters
                            .extend(unique);
                        return self.isolated_signature_type(instantiated);
                    }
                }
            }
        }
        let signature =
            self.contextual_signature_instantiation(signature, contextual, Some(context), None)?;
        self.isolated_signature_type(signature)
    }
    // port: tsc/internal/checker/checker.go:Checker.getSignatureInstantiation
    pub(crate) fn call_signature_instantiation(
        &mut self,
        signature: SignatureId,
        arguments: &[TypeId],
        javascript: bool,
        context: Option<InferenceId>,
    ) -> Result<SignatureId, Error> {
        let instantiated = self.signature_instantiation(signature, arguments, javascript)?;
        let Some(context) = context else {
            return Ok(instantiated);
        };
        let parameters = self
            .inference_context(context)?
            .inferred_type_parameters
            .clone();
        if parameters.is_empty() {
            return Ok(instantiated);
        }
        let returned = self.return_type_of_signature(instantiated)?;
        let Some(returned) = self.single_call_or_construct_signature(returned)? else {
            return Ok(instantiated);
        };
        let new_returned = self.clone_signature(returned)?;
        self.signatures.get_mut(new_returned)?.type_parameters = Some(parameters.into());
        let new_type = self.isolated_signature_type(new_returned)?;
        self.types.object_mut(new_type)?.mapper = self.signatures.get(instantiated)?.mapper;
        let result = self.clone_signature(instantiated)?;
        self.signatures.get_mut(result)?.resolved_return_type = Some(new_type);
        Ok(result)
    }
    // port: tsc/internal/checker/checker.go:Checker.getUniqueTypeParameters
    fn unique_inferred_type_parameters(
        &mut self,
        context: InferenceId,
        parameters: &[TypeId],
    ) -> Result<Vec<TypeId>, Error> {
        let mut old = Vec::new();
        let mut new = Vec::new();
        let mut result = Vec::with_capacity(parameters.len());
        let inferred = self
            .inference_context(context)?
            .inferred_type_parameters
            .clone();
        for &parameter in parameters {
            let symbol = self
                .types
                .get(parameter)?
                .symbol
                .ok_or(Error::MissingLink("inferred parameter symbol"))?;
            let name = self.symbol(symbol)?.name_to_owned();
            if self.type_parameter_list_has_name(&inferred, name.as_bytes())?
                || self.type_parameter_list_has_name(&result, name.as_bytes())?
            {
                let mut base = name.as_bytes();
                while base.len() > 1 && base.last().is_some_and(u8::is_ascii_digit) {
                    base = &base[..base.len() - 1];
                }
                let mut suffix = 1u64;
                let name = loop {
                    let mut candidate = base.to_vec();
                    candidate.extend_from_slice(suffix.to_string().as_bytes());
                    if !self.type_parameter_list_has_name(&inferred, &candidate)?
                        && !self.type_parameter_list_has_name(&result, &candidate)?
                    {
                        break JsString::from_bytes(candidate);
                    }
                    suffix = suffix.checked_add(1).ok_or(Error::IdExhausted)?;
                };
                let symbol = self.new_symbol(sf::TYPE_PARAMETER, name)?;
                let renamed = self.new_type_parameter(Some(symbol))?;
                self.types.type_parameter_mut(renamed)?.target = Some(parameter);
                old.push(parameter);
                new.push(renamed);
                result.push(renamed);
            } else {
                result.push(parameter);
            }
        }
        if !new.is_empty() {
            let mapper = self.new_type_mapper(&old, &new)?;
            for parameter in new {
                self.types.type_parameter_mut(parameter)?.mapper = Some(mapper);
            }
        }
        Ok(result)
    }
    fn type_parameter_list_has_name(
        &self,
        parameters: &[TypeId],
        name: &[u8],
    ) -> Result<bool, Error> {
        for &parameter in parameters {
            let symbol = self
                .types
                .get(parameter)?
                .symbol
                .ok_or(Error::MissingLink("type parameter symbol"))?;
            if self.symbol(symbol)?.name_bytes() == name {
                return Ok(true);
            }
        }
        Ok(false)
    }
}
