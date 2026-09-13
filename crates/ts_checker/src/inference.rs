//! Inference contexts are retained by ID because their fixing/non-fixing
//! mappers can outlive the immediate inference pass. Candidate lists and resolved
//! inferences are distinct: collecting a candidate invalidates unfixed caches.
use crate::{
    type_flags as tf, CheckerState, Error, InferenceId, MapperId, RelationKind, SignatureId, TypeId,
};

pub(crate) mod priority {
    pub const NONE: i32 = 0;
    pub const NAKED: i32 = 1 << 0;
    pub const SPECULATIVE_TUPLE: i32 = 1 << 1;
    pub const SUBSTITUTE_SOURCE: i32 = 1 << 2;
    pub const HOMOMORPHIC: i32 = 1 << 3;
    pub const PARTIAL_HOMOMORPHIC: i32 = 1 << 4;
    pub const MAPPED_CONSTRAINT: i32 = 1 << 5;
    pub const CONTRAVARIANT_CONDITIONAL: i32 = 1 << 6;
    pub const RETURN_TYPE: i32 = 1 << 7;
    pub const LITERAL_KEYOF: i32 = 1 << 8;
    pub const NO_CONSTRAINTS: i32 = 1 << 9;
    pub const ALWAYS_STRICT: i32 = 1 << 10;
    pub const MAX: i32 = 1 << 11;
    pub const CIRCULARITY: i32 = -1;
    pub const COMBINATION: i32 = RETURN_TYPE | MAPPED_CONSTRAINT | LITERAL_KEYOF;
}
pub(crate) const NO_DEFAULT: u32 = 1;
pub(crate) const ANY_DEFAULT: u32 = 2;

#[derive(Clone)]
pub(crate) struct InferenceInfo {
    pub parameter: TypeId,
    pub candidates: Vec<TypeId>,
    pub contra_candidates: Vec<TypeId>,
    pub inferred: Option<TypeId>,
    pub priority: i32,
    pub top_level: bool,
    pub fixed: bool,
    pub implied_arity: Option<usize>,
}
pub(crate) struct InferenceContext {
    pub inferences: Vec<InferenceInfo>,
    pub signature: Option<SignatureId>,
    pub flags: u32,
    pub mapper: MapperId,
    pub non_fixing_mapper: MapperId,
    pub return_mapper: Option<MapperId>,
    pub outer_return_mapper: Option<MapperId>,
    pub intra_expression_sites: Vec<(ts_arena::NodeId, TypeId)>,
    pub inferred_type_parameters: Vec<TypeId>,
    pub comparer: Option<crate::RelationFrameId>,
}
#[derive(Default)]
pub(crate) struct InferenceStore {
    pub contexts: Vec<InferenceContext>,
    pub reverse: crate::infer_reverse::ReverseInference,
}

impl CheckerState {
    pub(crate) fn inference_context(&self, id: InferenceId) -> Result<&InferenceContext, Error> {
        id.index(0)
            .and_then(|i| self.inference.contexts.get(i))
            .ok_or(Error::Arena(ts_arena::Error::InvalidSlot))
    }
    pub(crate) fn inference_context_mut(
        &mut self,
        id: InferenceId,
    ) -> Result<&mut InferenceContext, Error> {
        id.index(0)
            .and_then(|i| self.inference.contexts.get_mut(i))
            .ok_or(Error::Arena(ts_arena::Error::InvalidSlot))
    }
    // port: tsc/internal/checker/inference.go:Checker.newInferenceContext
    pub(crate) fn new_inference_context(
        &mut self,
        parameters: &[TypeId],
        signature: Option<SignatureId>,
        flags: u32,
    ) -> Result<InferenceId, Error> {
        let id = InferenceId::next(0, self.inference.contexts.len())?;
        let mapper = self.alloc_mapper(crate::mapper::Mapper::Inference {
            context: id,
            fixing: true,
        })?;
        let non_fixing_mapper = self.alloc_mapper(crate::mapper::Mapper::Inference {
            context: id,
            fixing: false,
        })?;
        let inferences = parameters
            .iter()
            .map(|&parameter| InferenceInfo {
                parameter,
                candidates: Vec::new(),
                contra_candidates: Vec::new(),
                inferred: None,
                priority: priority::MAX,
                top_level: true,
                fixed: false,
                implied_arity: None,
            })
            .collect();
        self.inference.contexts.push(InferenceContext {
            inferences,
            signature,
            flags,
            mapper,
            non_fixing_mapper,
            return_mapper: None,
            outer_return_mapper: None,
            intra_expression_sites: Vec::new(),
            inferred_type_parameters: Vec::new(),
            comparer: None,
        });
        Ok(id)
    }

    // port: tsc/internal/checker/inference.go:Checker.cloneInferenceContext
    // port: tsc/internal/checker/inference.go:Checker.cloneInferredPartOfContext
    pub(crate) fn clone_call_inference_context(
        &mut self,
        source: InferenceId,
        flags: u32,
        inferred_only: bool,
    ) -> Result<Option<InferenceId>, Error> {
        let source = self.inference_context(source)?;
        let inferences: Vec<_> = source
            .inferences
            .iter()
            .filter(|info| {
                !inferred_only || !info.candidates.is_empty() || !info.contra_candidates.is_empty()
            })
            .cloned()
            .collect();
        if inferred_only && inferences.is_empty() {
            return Ok(None);
        }
        let parameters: Vec<_> = inferences.iter().map(|info| info.parameter).collect();
        let signature = source.signature;
        let flags = source.flags | flags;
        let comparer = source.comparer;
        let result = self.new_inference_context(&parameters, signature, flags)?;
        self.inference_context_mut(result)?.inferences = inferences;
        if let Some(comparer) = comparer {
            self.set_inference_comparer(result, comparer)?;
        }
        Ok(Some(result))
    }

    // port: tsc/internal/checker/inference.go:Checker.createOuterReturnMapper
    pub(crate) fn call_outer_return_mapper(
        &mut self,
        context: InferenceId,
    ) -> Result<MapperId, Error> {
        if let Some(mapper) = self.inference_context(context)?.outer_return_mapper {
            return Ok(mapper);
        }
        let cloned = self
            .clone_call_inference_context(context, 0, false)?
            .ok_or(Error::MissingLink("outer inference clone"))?;
        let mut mapper = self.inference_context(cloned)?.mapper;
        if let Some(first) = self.inference_context(context)?.return_mapper {
            mapper = self.alloc_mapper(crate::mapper::Mapper::Merged {
                first,
                second: mapper,
            })?;
        }
        self.inference_context_mut(context)?.outer_return_mapper = Some(mapper);
        Ok(mapper)
    }

    pub(crate) fn set_inference_comparer(
        &mut self,
        id: InferenceId,
        frame: crate::RelationFrameId,
    ) -> Result<(), Error> {
        self.retain_relation_frame(frame)?;
        self.inference_context_mut(id)?.comparer = Some(frame);
        Ok(())
    }
    fn compare_inference_constraint(
        &mut self,
        id: InferenceId,
        source: TypeId,
        target: TypeId,
    ) -> Result<bool, Error> {
        match self.inference_context(id)?.comparer {
            Some(frame) => self
                .compare_in_relation_frame(frame, source, target)
                .map(|result| result != crate::ternary::FALSE),
            None => self.is_type_related_to(source, target, RelationKind::Assignable),
        }
    }
    // port: tsc/internal/checker/inference.go:clearCachedInferences
    pub(crate) fn clear_cached_inferences(&mut self, id: InferenceId) -> Result<(), Error> {
        for inference in &mut self.inference_context_mut(id)?.inferences {
            if !inference.fixed {
                inference.inferred = None;
            }
        }
        Ok(())
    }
    // port: tsc/internal/checker/inference.go:Checker.addIntraExpressionInferenceSite
    pub(crate) fn add_intra_expression_inference_site(
        &mut self,
        context: InferenceId,
        node: ts_arena::NodeId,
        ty: TypeId,
    ) -> Result<(), Error> {
        self.inference_context_mut(context)?
            .intra_expression_sites
            .push((node, ty));
        Ok(())
    }
    // port: tsc/internal/checker/inference.go:Checker.inferFromIntraExpressionSites
    fn infer_from_intra_expression_sites(&mut self, context: InferenceId) -> Result<(), Error> {
        let sites = self
            .inference_context(context)?
            .intra_expression_sites
            .clone();
        for (node, ty) in sites {
            let contextual =
                if self.ast(node)?.node(node)?.kind() == ts_ast::SyntaxKind::MethodDeclaration {
                    self.contextual_property_type_with_flags(node, 2)?
                } else {
                    self.contextual_expression_type_ex(node, 2)?
                };
            if let Some(contextual) = contextual {
                self.infer_types(context, ty, contextual, priority::NONE, false)?;
            }
        }
        self.inference_context_mut(context)?
            .intra_expression_sites
            .clear();
        Ok(())
    }
    // port: tsc/internal/checker/mapper.go:InferenceTypeMapper.Map
    pub(crate) fn map_inference_type(
        &mut self,
        id: InferenceId,
        ty: TypeId,
        fixing: bool,
    ) -> Result<TypeId, Error> {
        let Some(index) = self
            .inference_context(id)?
            .inferences
            .iter()
            .position(|i| i.parameter == ty)
        else {
            return Ok(ty);
        };
        if fixing && !self.inference_context(id)?.inferences[index].fixed {
            self.infer_from_intra_expression_sites(id)?;
            self.clear_cached_inferences(id)?;
            self.inference_context_mut(id)?.inferences[index].fixed = true;
        }
        self.inferred_type(id, index)
    }
    // port: tsc/internal/checker/inference.go:Checker.getInferredType
    pub(crate) fn inferred_type(&mut self, id: InferenceId, index: usize) -> Result<TypeId, Error> {
        let context = self.inference_context(id)?;
        let info = context
            .inferences
            .get(index)
            .ok_or(Error::MissingLink("inference index"))?
            .clone();
        if let Some(inferred) = info.inferred {
            return Ok(inferred);
        }
        if info.parameter == self.builtins.error_type {
            return Ok(info.parameter);
        }
        let flags = context.flags;
        let mapper = context.non_fixing_mapper;
        let (mut inferred, fallback) = if let Some(signature) = context.signature {
            self.signature_inference_candidates(id, index, &info, signature)?
        } else {
            (
                if !info.candidates.is_empty() {
                    Some(self.get_union_type_ex(
                        &info.candidates,
                        crate::UnionReduction::Subtype,
                        None,
                        None,
                    )?)
                } else if !info.contra_candidates.is_empty() {
                    Some(self.get_intersection_type(&info.contra_candidates)?)
                } else {
                    None
                },
                None,
            )
        };
        let default = if flags & ANY_DEFAULT != 0 {
            self.builtins.any_type
        } else {
            self.builtins.unknown_type
        };
        self.inference_context_mut(id)?.inferences[index].inferred =
            Some(inferred.unwrap_or(default));
        let result = (|| {
            if let Some(constraint) = self.constraint_of_type_parameter(info.parameter)? {
                let constraint = self.instantiate_type(constraint, Some(mapper))?;
                if let Some(ty) = inferred {
                    let with_this = self.get_type_with_this_argument(constraint, ty, false)?;
                    if !self.compare_inference_constraint(id, ty, with_this)? {
                        inferred = None;
                        if info.priority == priority::RETURN_TYPE {
                            let filtered = self.map_type(ty, &mut |checker, part| {
                                Ok(Some(
                                    if checker.compare_inference_constraint(id, part, with_this)? {
                                        part
                                    } else {
                                        checker.builtins.never_type
                                    },
                                ))
                            })?;
                            if let Some(filtered) = filtered {
                                if self.types.flags(filtered)? & tf::NEVER == 0 {
                                    inferred = Some(filtered);
                                }
                            }
                        }
                    }
                }
                if inferred.is_none() {
                    if let Some(fallback) = fallback {
                        let with_this =
                            self.get_type_with_this_argument(constraint, fallback, false)?;
                        if self.compare_inference_constraint(id, fallback, with_this)? {
                            inferred = Some(fallback);
                        }
                    }
                }
                self.inference_context_mut(id)?.inferences[index].inferred =
                    Some(inferred.unwrap_or(constraint));
            }
            self.clear_active_mapper_caches();
            Ok(self.inference_context(id)?.inferences[index]
                .inferred
                .expect("provisional inference installed above"))
        })();
        if result.is_err() {
            self.inference_context_mut(id)?.inferences[index].inferred = None;
        }
        result
    }
    // port: tsc/internal/checker/inference.go:Checker.isTypeParameterAtTopLevel
    pub(crate) fn type_parameter_at_top_level(
        &mut self,
        ty: TypeId,
        parameter: TypeId,
        depth: usize,
    ) -> Result<bool, Error> {
        if ty == parameter {
            return Ok(true);
        }
        let flags = self.types.flags(ty)?;
        if flags & tf::UNION_OR_INTERSECTION != 0 {
            for part in self.types.types_of(ty)?.to_vec() {
                if self.type_parameter_at_top_level(part, parameter, depth)? {
                    return Ok(true);
                }
            }
        }
        if depth < 3 && flags & tf::CONDITIONAL != 0 {
            let yes = self.conditional_true_type(ty, false)?;
            if self.type_parameter_at_top_level(yes, parameter, depth + 1)? {
                return Ok(true);
            }
            let no = self.conditional_false_type(ty)?;
            return self.type_parameter_at_top_level(no, parameter, depth + 1);
        }
        Ok(false)
    }
}
