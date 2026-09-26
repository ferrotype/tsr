//! Exact native state schedule for a completed intermediate constraint frame.
//! Only the test feature exposes this setup; production cycle checks are reused.
use super::Operation;
use crate::{Error, TypeSystemEntity, TypeSystemPropertyName};
use serde_json::{json, Value};
use tsr_arena::NodeId;

impl Operation<'_> {
    pub fn c2_mapped_property_cycle_contract(&mut self, node: NodeId) -> Result<Value, Error> {
        let state = self.state_mut();
        let ty = state.get_type_at_location(node)?;
        let property = state
            .constituent_property(ty, b"a", false)?
            .ok_or(Error::MissingLink("contract mapped property"))?;
        if state.symbol(property)?.check_flags() & tsr_ast::check_flags::MAPPED == 0
            || state
                .value_symbol_links
                .peek(property)
                .is_some_and(|links| links.resolved_type.is_some())
        {
            return Err(Error::MissingLink(
                "unresolved source mapped property required",
            ));
        }
        let depth = state.resolution.depth();
        let parameter = state.new_type_parameter(None)?;
        if !state.push_type_resolution(
            TypeSystemEntity::Symbol(property),
            TypeSystemPropertyName::Type,
        ) {
            return Err(Error::MissingLink("contract mapped resolution push"));
        }
        let without = state.is_circular_mapped_property(property)?;
        if !state.push_type_resolution(
            TypeSystemEntity::Type(parameter),
            TypeSystemPropertyName::ResolvedBaseConstraint,
        ) {
            state.resolution.pop();
            return Err(Error::MissingLink("contract constraint resolution push"));
        }
        let observed = (|| {
            let unfinished = state.is_circular_mapped_property(property)?;
            state
                .types
                .type_parameter_mut(parameter)?
                .resolved_base_constraint = Some(state.builtins.unknown_type);
            let completed = state.is_circular_mapped_property(property)?;
            Ok::<_, Error>((unfinished, completed))
        })();
        let first = state.resolution.pop();
        let after = state.is_circular_mapped_property(property);
        let second = state.resolution.pop();
        let (unfinished, completed) = observed?;
        Ok(
            json!({"without_intermediate":without,"unfinished_intermediate":unfinished,
            "completed_intermediate":completed,"after_pop":after?,"pops":[first,second],
            "restored":state.resolution.depth()==depth}),
        )
    }
}

impl Operation<'_> {
    pub fn c2_type_parameter_helper_contract(
        &mut self,
        capabilities: &[NodeId],
        parameter_nodes: &[NodeId],
        outer: NodeId,
        alias: NodeId,
        instance: NodeId,
    ) -> Result<Value, Error> {
        let state = self.state_mut();
        let mut can_get = Vec::new();
        for &node in capabilities {
            let symbol = state
                .get_symbol_of_declaration(node)?
                .ok_or(Error::MissingLink("capability symbol"))?;
            can_get.push(state.can_get_type_parameters_of_class_or_interface(symbol)?);
        }
        let mut parameters = Vec::new();
        let mut unconstrained = Vec::new();
        for &node in parameter_nodes {
            let symbol = state
                .get_symbol_of_declaration(node)?
                .ok_or(Error::MissingLink("parameter symbol"))?;
            let parameter = state.get_declared_type_of_symbol(symbol)?;
            parameters.push(parameter);
            unconstrained.push(state.is_unconstrained_type_parameter(parameter)?);
        }
        for index in [0, 2] {
            let clone = state.new_type_parameter(None)?;
            state.types.type_parameter_mut(clone)?.target = Some(parameters[index]);
            unconstrained.push(state.is_unconstrained_type_parameter(clone)?);
        }
        let anonymous = state.new_type_parameter(None)?;
        unconstrained.push(state.is_unconstrained_type_parameter(anonymous)?);
        let before = state.calls.inference_contexts.len();
        let first = state.new_inference_context(&[parameters[0], parameters[2]], None, 0)?;
        let second = state.new_inference_context(&[parameters[1], parameters[0]], None, 0)?;
        state.calls.inference_contexts.extend([
            (outer, Some(first)),
            (outer, None),
            (outer, Some(second)),
        ]);
        let outer_parameters = state.outer_inference_type_parameters();
        state.calls.inference_contexts.truncate(before);
        let names = outer_parameters?
            .iter()
            .map(|&ty| {
                let symbol = state
                    .types
                    .get(ty)?
                    .symbol
                    .ok_or(Error::MissingLink("outer parameter symbol"))?;
                Ok::<_, Error>(
                    String::from_utf8_lossy(state.symbol(symbol)?.name_bytes()).into_owned(),
                )
            })
            .collect::<Result<Vec<_>, _>>()?;
        let outer_symbol = state
            .get_symbol_of_declaration(outer)?
            .ok_or(Error::MissingLink("outer symbol"))?;
        let alias_symbol = state
            .get_symbol_of_declaration(alias)?
            .ok_or(Error::MissingLink("alias symbol"))?;
        let instance_type = state.get_type_at_location(instance)?;
        let property = state
            .constituent_property(instance_type, b"item", false)?
            .ok_or(Error::MissingLink("instantiated property"))?;
        let outer_type = state.get_declared_type_of_symbol(outer_symbol)?;
        let original = state
            .constituent_property(outer_type, b"item", false)?
            .ok_or(Error::MissingLink("original property"))?;
        let mut qualified = Vec::new();
        for chain in [
            [outer_symbol, property],
            [alias_symbol, property],
            [outer_symbol, original],
        ] {
            let mut builder = crate::node_builder::NodeBuilder::new(
                state,
                tsr_nodebuilder::flags::WRITE_TYPE_PARAMETERS_IN_QUALIFIED_NAME,
            );
            qualified.push(builder.c2_qualified_parameter_contract(&chain)?);
        }
        Ok(
            json!({"capabilities":can_get,"unconstrained":unconstrained,"outer":names,
            "restored":state.calls.inference_contexts.len()==before,"qualified":qualified}),
        )
    }
}
