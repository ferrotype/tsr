//! Diagnostic-only C2 schedules and read-only state snapshots. The matching Go
//! bridge executes the same production operations over the same source types.
use super::{Operation, TypeRef};
use crate::{Error, InferenceId, TypeId};
use serde_json::{json, Value};
use tsr_arena::NodeId;

#[derive(Default)]
pub(crate) struct InstantiationProbe {
    pub maximum_depth: u32,
    pub depth_limit_hits: usize,
    pub count_limit_hits: usize,
    pub maximum_remaining_stack: usize,
}

impl Operation<'_> {
    pub fn c2_context_depth(&self) -> usize {
        self.state().calls.contexts.len()
    }

    pub fn c2_inference_contract(&mut self, node: NodeId) -> Result<Value, Error> {
        let state = self.state_mut();
        let ty = state.get_type_at_location(node)?;
        let signature = state.signatures_of_type(ty, false)?[0];
        let parameter = state
            .signatures
            .get(signature)?
            .type_parameters
            .as_ref()
            .and_then(|p| p.first())
            .copied()
            .ok_or(Error::MissingLink("contract type parameter"))?;
        let context = state.new_inference_context(&[parameter], None, 0)?;
        state.infer_types(context, state.builtins.string_type, parameter, 0, false)?;
        let mut rows = Vec::new();
        fn snapshot(
            state: &mut crate::CheckerState,
            rows: &mut Vec<Value>,
            label: &str,
            context: InferenceId,
            ty: TypeId,
        ) -> Result<(), Error> {
            let info = &state.inference_context(context)?.inferences[0];
            let fixed = info.fixed;
            let candidates = info.candidates.len();
            let text = state.type_to_string(ty, 0)?;
            rows.push(json!({"label":label,"fixed":fixed,"candidates":candidates,"type":std::str::from_utf8(text.as_bytes()).expect("fixture display is UTF-8")}));
            Ok(())
        }
        let mapper = state.inference_context(context)?.non_fixing_mapper;
        let ty = state.instantiate_type(parameter, Some(mapper))?;
        snapshot(state, &mut rows, "nonfixing", context, ty)?;
        let cloned = state
            .clone_call_inference_context(context, 0, false)?
            .ok_or(Error::MissingLink("contract cloned context"))?;
        let mapper = state.inference_context(cloned)?.mapper;
        let ty = state.instantiate_type(parameter, Some(mapper))?;
        snapshot(state, &mut rows, "clone_fixed", cloned, ty)?;
        let mapper = state.inference_context(context)?.non_fixing_mapper;
        let ty = state.instantiate_type(parameter, Some(mapper))?;
        snapshot(state, &mut rows, "original_after_clone", context, ty)?;
        state.infer_types(context, state.builtins.number_type, parameter, 0, false)?;
        let mapper = state.inference_context(context)?.mapper;
        let ty = state.instantiate_type(parameter, Some(mapper))?;
        snapshot(state, &mut rows, "fixed", context, ty)?;
        state.infer_types(context, state.builtins.boolean_type, parameter, 0, false)?;
        let ty = state.instantiate_type(parameter, Some(mapper))?;
        snapshot(state, &mut rows, "later_candidate", context, ty)?;
        Ok(json!(rows))
    }

    pub fn c2_higher_order_contract(&mut self, ty: TypeRef) -> Result<Value, Error> {
        let ty = self.check_type(ty)?;
        let state = self.state_mut();
        let mut rows = Vec::new();
        for signature in state.signatures_of_type(ty, false)? {
            let parameters = state
                .signatures
                .get(signature)?
                .type_parameters
                .clone()
                .unwrap_or_default();
            let mut params = Vec::new();
            for &parameter in parameters.iter() {
                let permissive = state.permissive_instantiation(parameter)?;
                let inference = state.new_inference_context(&[permissive], Some(signature), 0)?;
                let has_default =
                    if state.types.flags(permissive)? & crate::type_flags::TYPE_PARAMETER != 0 {
                        let default = state.resolved_type_parameter_default(permissive)?;
                        default != state.builtins.no_constraint_type
                            && default != state.builtins.circular_constraint_type
                    } else {
                        false
                    };
                let inferred = state.inferred_type(inference, 0)?;
                let text = |s: tsr_ast::JsString| {
                    String::from_utf8(s.as_bytes().to_vec()).expect("fixture display is UTF-8")
                };
                params.push(json!({"type":text(state.type_to_string(parameter,0)?),"permissive":text(state.type_to_string(permissive,0)?),"wildcard":permissive==state.builtins.wildcard_type,"has_default":has_default,"inferred":text(state.type_to_string(inferred,0)?)}));
            }
            rows.push(json!({"parameters":params}));
        }
        Ok(json!(rows))
    }

    pub fn c2_begin_instantiation_probe(&mut self) -> Result<(), Error> {
        let state = self.state_mut();
        if state.instantiation.probe.is_some() {
            return Err(Error::MissingLink("instantiation probe already installed"));
        }
        state.instantiation.probe = Some(InstantiationProbe::default());
        Ok(())
    }

    pub fn c2_take_instantiation_probe(&mut self) -> Result<Value, Error> {
        let state = self.state_mut();
        let probe = state
            .instantiation
            .probe
            .take()
            .ok_or(Error::MissingLink("instantiation probe not installed"))?;
        Ok(
            json!({"maximum_depth":probe.maximum_depth,"depth_limit_hits":probe.depth_limit_hits,"count_limit_hits":probe.count_limit_hits,"maximum_remaining_stack":probe.maximum_remaining_stack,"restored_depth":state.instantiation.depth}),
        )
    }
}
