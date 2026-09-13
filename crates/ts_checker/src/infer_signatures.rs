use crate::{
    infer_types::InferenceRun, inference::priority as p, signature_flags as sg, CheckerState,
    Error, SignatureId, TypeId,
};
use ts_ast::SyntaxKind as K;

impl CheckerState {
    // port: tsc/internal/checker/checker.go:Checker.getTypeParametersForMapper
    fn signature_parameters_for_mapper(
        &mut self,
        signature: SignatureId,
    ) -> Result<crate::TypeList, Error> {
        let mut parameters = self
            .signatures
            .get(signature)?
            .type_parameters
            .as_deref()
            .unwrap_or_default()
            .to_vec();
        for parameter in &mut parameters {
            *parameter =
                self.instantiate_type(*parameter, self.types.type_parameter(*parameter)?.mapper)?;
        }
        Ok(parameters.into())
    }

    // port: tsc/internal/checker/checker.go:Checker.getSignatureInstantiation
    pub(crate) fn signature_instantiation(
        &mut self,
        signature: SignatureId,
        arguments: &[TypeId],
        javascript: bool,
    ) -> Result<SignatureId, Error> {
        let parameters = self
            .signatures
            .get(signature)?
            .type_parameters
            .clone()
            .unwrap_or_else(|| [].into());
        let arguments = self.fill_missing_type_arguments(arguments, &parameters, javascript)?;
        let key = (signature, crate::key::type_list_key(&arguments));
        if let Some(&cached) = self.signatures.instantiations.get(&key) {
            return Ok(cached);
        }
        let result = self.create_signature_instantiation(signature, &arguments)?;
        self.signatures.instantiations.insert(key, result);
        Ok(result)
    }

    // port: tsc/internal/checker/checker.go:Checker.createSignatureInstantiation
    pub(crate) fn create_signature_instantiation(
        &mut self,
        signature: SignatureId,
        arguments: &[TypeId],
    ) -> Result<SignatureId, Error> {
        let parameters = self.signature_parameters_for_mapper(signature)?;
        let mapper = self.new_type_mapper(&parameters, arguments)?;
        self.instantiate_signature_ex(signature, mapper, true)
    }

    // port: tsc/internal/checker/checker.go:Checker.instantiateSignatureInContextOf
    pub(crate) fn contextual_signature_instantiation(
        &mut self,
        signature: SignatureId,
        contextual: SignatureId,
        outer: Option<crate::InferenceId>,
        comparer: Option<crate::RelationFrameId>,
    ) -> Result<SignatureId, Error> {
        let parameters = self.signature_parameters_for_mapper(signature)?;
        let context = self.new_inference_context(&parameters, Some(signature), 0)?;
        if let Some(frame) = comparer {
            self.set_inference_comparer(context, frame)?;
        }
        let rest = self.effective_rest_type(contextual)?;
        let source = if let Some(outer) = outer {
            let use_non_fixing = match rest {
                Some(rest) => self.types.flags(rest)? & crate::type_flags::TYPE_PARAMETER != 0,
                None => false,
            };
            let mapper = if use_non_fixing {
                self.inference_context(outer)?.non_fixing_mapper
            } else {
                self.inference_context(outer)?.mapper
            };
            self.instantiate_signature(contextual, mapper)?
        } else {
            contextual
        };
        self.for_signature_parameters(source, signature, &mut |checker, s, t| {
            checker.infer_types(context, s, t, p::NONE, false)
        })?;
        if outer.is_none() {
            self.for_signature_returns(contextual, signature, &mut |checker, s, t| {
                checker.infer_types(context, s, t, p::RETURN_TYPE, false)
            })?;
        }
        let mut arguments = Vec::with_capacity(parameters.len());
        for index in 0..parameters.len() {
            arguments.push(self.inferred_type(context, index)?);
        }
        let javascript = match self.signatures.get(contextual)?.declaration {
            Some(node) => {
                self.ast(node)?.node(node)?.flags() & ts_ast::node_flags::JAVA_SCRIPT_FILE != 0
            }
            None => false,
        };
        self.signature_instantiation(signature, &arguments, javascript)
    }

    // port: tsc/internal/checker/inference.go:Checker.inferFromSignatures
    pub(crate) fn infer_signatures(
        &mut self,
        run: &mut InferenceRun,
        source: TypeId,
        target: TypeId,
        construct: bool,
    ) -> Result<(), Error> {
        let sources = self.signatures_of_type(source, construct)?;
        if sources.is_empty() {
            return Ok(());
        }
        let targets = self.signatures_of_type(target, construct)?;
        for (i, &target) in targets.iter().enumerate() {
            let source = sources[(sources.len() + i).saturating_sub(targets.len())];
            let source = self.base_signature(source)?;
            let target = self.erased_signature(target)?;
            self.infer_signature(run, source, target)?;
        }
        Ok(())
    }

    // port: tsc/internal/checker/inference.go:Checker.inferFromSignature
    fn infer_signature(
        &mut self,
        run: &mut InferenceRun,
        source: SignatureId,
        target: SignatureId,
    ) -> Result<(), Error> {
        if self.signatures.get(source)?.flags & sg::IS_NON_INFERRABLE == 0 {
            let saved = run.bivariant;
            let kind = match self.signatures.get(target)?.declaration {
                Some(node) => self.ast(node)?.node(node)?.kind().known(),
                None => None,
            };
            run.bivariant |= matches!(
                kind,
                Some(K::MethodDeclaration | K::MethodSignature | K::Constructor)
            );
            let result = self.infer_signature_parameters(run, source, target, true);
            run.bivariant = saved;
            result?;
        }
        self.infer_signature_return(run, source, target)
    }

    // port: tsc/internal/checker/inference.go:Checker.applyToParameterTypes
    pub(crate) fn infer_signature_parameters(
        &mut self,
        run: &mut InferenceRun,
        source: SignatureId,
        target: SignatureId,
        strict_contravariant: bool,
    ) -> Result<(), Error> {
        self.for_signature_parameters(source, target, &mut |checker, s, t| {
            checker.infer_signature_parameter(run, s, t, strict_contravariant)
        })
    }

    pub(crate) fn for_signature_parameters<F>(
        &mut self,
        source: SignatureId,
        target: SignatureId,
        callback: &mut F,
    ) -> Result<(), Error>
    where
        F: FnMut(&mut Self, TypeId, TypeId) -> Result<(), Error>,
    {
        let source_count = self.parameter_count(source)?;
        let target_count = self.parameter_count(target)?;
        let source_rest = self.effective_rest_type(source)?;
        let target_rest = self.effective_rest_type(target)?;
        let target_non_rest = target_count - usize::from(target_rest.is_some());
        let count = if source_rest.is_none() {
            source_count.min(target_non_rest)
        } else {
            target_non_rest
        };
        if let (Some(s), Some(t)) = (
            self.signatures.get(source)?.this_parameter,
            self.signatures.get(target)?.this_parameter,
        ) {
            let s = self.get_type_of_symbol(s)?;
            let t = self.get_type_of_symbol(t)?;
            callback(self, s, t)?;
        }
        for position in 0..count {
            let s = self
                .parameter_type_at(source, position)?
                .unwrap_or(self.builtins.any_type);
            let t = self
                .parameter_type_at(target, position)?
                .unwrap_or(self.builtins.any_type);
            callback(self, s, t)?;
        }
        if let Some(target_rest) = target_rest {
            let readonly = self.is_const_type_variable(target_rest, 0)?
                && !self.some_mutable_array_like(target_rest)?;
            let source_rest = self.rest_type_at_ex(source, count, readonly)?;
            callback(self, source_rest, target_rest)?;
        }
        Ok(())
    }
    fn infer_signature_parameter(
        &mut self,
        run: &mut InferenceRun,
        source: TypeId,
        target: TypeId,
        strict_contravariant: bool,
    ) -> Result<(), Error> {
        if strict_contravariant
            && (self.strict_function_types() || run.priority & p::ALWAYS_STRICT != 0)
        {
            self.infer_contravariant(run, source, target)
        } else {
            self.infer_from_types(run, source, target)
        }
    }

    // port: tsc/internal/checker/inference.go:Checker.applyToReturnTypes
    pub(crate) fn infer_signature_return(
        &mut self,
        run: &mut InferenceRun,
        source: SignatureId,
        target: SignatureId,
    ) -> Result<(), Error> {
        self.for_signature_returns(source, target, &mut |checker, s, t| {
            checker.infer_from_types(run, s, t)
        })
    }

    pub(crate) fn for_signature_returns<F>(
        &mut self,
        source: SignatureId,
        target: SignatureId,
        callback: &mut F,
    ) -> Result<(), Error>
    where
        F: FnMut(&mut Self, TypeId, TypeId) -> Result<(), Error>,
    {
        if let Some(target_predicate) = self.type_predicate_of_signature(target)? {
            if let Some(source_predicate) = self.type_predicate_of_signature(source)? {
                let s = self.signatures.predicate(source_predicate)?;
                let t = self.signatures.predicate(target_predicate)?;
                if s.kind == t.kind && s.parameter_index == t.parameter_index {
                    if let (Some(s), Some(t)) = (s.t, t.t) {
                        return callback(self, s, t);
                    }
                }
            }
        }
        let target_type = self.return_type_of_signature(target)?;
        if self.could_contain_type_variables(target_type)? {
            let source_type = self.return_type_of_signature(source)?;
            callback(self, source_type, target_type)?;
        }
        Ok(())
    }

    // port: tsc/internal/checker/checker.go:Checker.getBaseSignature
    pub(crate) fn base_signature(&mut self, signature: SignatureId) -> Result<SignatureId, Error> {
        let sig = self.signatures.get(signature)?;
        let Some(parameters) = sig.type_parameters.clone().filter(|p| !p.is_empty()) else {
            return Ok(signature);
        };
        if let Some(base) = sig.base {
            return Ok(base);
        }
        let mut constraints = Vec::with_capacity(parameters.len());
        for &parameter in parameters.iter() {
            constraints.push(
                self.constraint_of_type_parameter(parameter)?
                    .unwrap_or(self.builtins.unknown_type),
            );
        }
        let mapper = self.new_type_mapper(&parameters, &constraints)?;
        let mut types = Vec::with_capacity(parameters.len());
        for &parameter in parameters.iter() {
            types.push(self.instantiate_type(parameter, Some(mapper))?);
        }
        for _ in 1..parameters.len() {
            for ty in &mut types {
                *ty = self.instantiate_type(*ty, Some(mapper))?;
            }
        }
        let eraser =
            self.new_type_mapper(&parameters, &vec![self.builtins.any_type; parameters.len()])?;
        for ty in &mut types {
            *ty = self.instantiate_type(*ty, Some(eraser))?;
        }
        let mapper = self.new_type_mapper(&parameters, &types)?;
        let result = self.instantiate_signature_ex(signature, mapper, true)?;
        self.signatures.get_mut(signature)?.base = Some(result);
        Ok(result)
    }
}
