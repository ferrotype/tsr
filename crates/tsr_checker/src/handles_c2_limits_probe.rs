//! Diagnostic-only schedules over source types, paired with pinned Go calls.
//! Counters and thresholds are never changed by these observers.
use super::Operation;
use crate::Error;
use serde_json::{json, Value};
use tsr_arena::NodeId;

#[derive(Default)]
pub(crate) struct LimitsProbe {
    pub subtype_estimates: Vec<(usize, usize)>,
    pub base_depths: Vec<usize>,
    pub conditional_depths: Vec<u32>,
}

impl Operation<'_> {
    pub fn c2_begin_limits_probe(&mut self) {
        self.state_mut().c2_limits_probe = Some(LimitsProbe::default());
    }
    pub fn c2_take_limits_probe(&mut self) -> Value {
        let state = self.state_mut();
        let p = state.c2_limits_probe.take().unwrap();
        json!({"subtype_estimates":p.subtype_estimates,"base_depths":p.base_depths,"conditional_depths":p.conditional_depths,"restored_conditional_depth":state.conditional_constraint_depth})
    }
    pub fn c2_instantiation_count(&self) -> u64 {
        self.state().instantiation.count
    }
    pub fn c2_base_constraint_contract(&mut self, node: NodeId) -> Result<Value, Error> {
        let state = self.state_mut();
        let ty = state.get_type_at_location(node)?;
        let constraint = state.base_constraint_of_type(ty)?;
        let display = constraint.map(|t| state.type_to_string(t, 0)).transpose()?;
        Ok(json!({"constraint":display.map(|s|String::from_utf8(s.as_bytes().to_vec()).unwrap())}))
    }

    pub fn c2_variance_contract(&mut self, node: NodeId) -> Result<Value, Error> {
        let state = self.state_mut();
        let symbol = state
            .get_symbol_at_location(node)?
            .ok_or(Error::MissingLink("variance contract declaration"))?;
        let ty = state.get_declared_type_of_symbol(symbol)?;
        state.variance.contract_cycles = Some([0; 2]);
        let is_alias = state.symbol(symbol)?.flags() & tsr_ast::symbol_flags::TYPE_ALIAS != 0;
        let result = if is_alias {
            state.alias_variances(symbol)
        } else {
            state.variances_of(ty)
        };
        let counts = state.variance.contract_cycles.take().unwrap();
        let values = result?;
        let repeated = if is_alias {
            state.alias_variances(symbol)?
        } else {
            state.variances_of(ty)?
        };
        Ok(
            json!({"flags":values,"cached_flags":repeated,"cycles":counts[0],"restarts":counts[1],"restored_stack":state.variance.stack.len(),"restored_reliability":state.variance.reliability}),
        )
    }
    /// Calls the real instantiation entry repeatedly, without resetting or seeding
    /// its counter. A later public type query performs its ordinary recovery reset.
    pub fn c2_instantiation_count_contract(&mut self, node: NodeId) -> Result<Value, Error> {
        let state = self.state_mut();
        let ty = state.get_type_at_location(node)?;
        let sig = state.signatures_of_type(ty, false)?[0];
        let parameter = state.signatures.get(sig)?.type_parameters.as_ref().unwrap()[0];
        let mapper = state.new_type_mapper(&[parameter], &[state.builtins.string_type])?;
        let start = state.instantiation.count;
        let total = state.instantiation.total_count;
        let saved = state.current_node.replace(node);
        let result = (|| {
            let mut completed = 0u64;
            let mut error = None;
            for _ in 0..=5_000_000 {
                let value = state.instantiate_type(parameter, Some(mapper))?;
                if value == state.builtins.error_type {
                    error = Some(value);
                    break;
                }
                if value != state.builtins.string_type {
                    return Err(Error::MissingLink("count contract substituted parameter"));
                }
                completed += 1;
            }
            Ok(
                json!({"starting_count":start,"completed":completed,"count":state.instantiation.count,
                "total_delta":state.instantiation.total_count-total,"error_type":error.is_some(),
                "restored_depth":state.instantiation.depth}),
            )
        })();
        state.current_node = saved;
        result
    }

    pub fn c2_subtype_limit_contract(&mut self, node: NodeId) -> Result<Value, Error> {
        let state = self.state_mut();
        let ty = state.get_type_at_location(node)?;
        let parts = state.types.types_of(ty)?.to_vec();
        let saved = state.current_node.replace(node);
        let result = state.get_union_type_ex(&parts, crate::UnionReduction::Subtype, None, None);
        state.current_node = saved;
        let result = result?;
        Ok(
            json!({"constituents":parts.len(),"error_type":result==state.builtins.error_type,
            "result_constituents":state.types.types_of(result)?.len()}),
        )
    }

    pub fn c2_nested_contract(
        &mut self,
        node: NodeId,
        stack: &[NodeId],
        max: usize,
    ) -> Result<bool, Error> {
        let state = self.state_mut();
        let ty = state.get_type_at_location(node)?;
        let stack = stack
            .iter()
            .map(|&n| state.get_type_at_location(n))
            .collect::<Result<Vec<_>, _>>()?;
        state.deeply_nested_type(ty, &stack, max)
    }
}
