use crate::{
    element_flags as ef, infer_types::InferenceRun, inference::priority as p, object_flags as of,
    type_flags as tf, CheckerState, Error, TypeId,
};
use tsr_ast::{check_flags as cf, symbol_flags as sf};

impl CheckerState {
    // port: tsc/internal/checker/inference.go:Checker.inferFromGenericMappedTypes
    pub(crate) fn infer_generic_mapped(
        &mut self,
        run: &mut InferenceRun,
        source: TypeId,
        target: TypeId,
    ) -> Result<(), Error> {
        let s = self.mapped_constraint(source)?;
        let t = self.mapped_constraint(target)?;
        self.infer_from_types(run, s, t)?;
        let s = self.mapped_template(source)?;
        let t = self.mapped_template(target)?;
        self.infer_from_types(run, s, t)?;
        if let (Some(s), Some(t)) = (self.mapped_name(source)?, self.mapped_name(target)?) {
            self.infer_from_types(run, s, t)?;
        }
        Ok(())
    }

    // port: tsc/internal/checker/inference.go:Checker.inferFromObjectTypes
    pub(crate) fn infer_object_types(
        &mut self,
        run: &mut InferenceRun,
        source: TypeId,
        target: TypeId,
    ) -> Result<(), Error> {
        if self.types.object_flags(source)? & self.types.object_flags(target)? & of::REFERENCE != 0
            && (self.types.target(source)? == self.types.target(target)?
                || self.is_array_type(source)? && self.is_array_type(target)?)
        {
            let s = self.get_type_arguments(source)?;
            let t = self.get_type_arguments(target)?;
            let variances = self.variances_of(self.types.target(source)?)?;
            return self.infer_arguments(run, &s, &t, &variances);
        }
        if self.is_generic_mapped_type(source)? && self.is_generic_mapped_type(target)? {
            self.infer_generic_mapped(run, source, target)?;
        }
        if self.types.object_flags(target)? & of::MAPPED != 0 && self.mapped_name(target)?.is_none()
        {
            let constraint = self.mapped_constraint(target)?;
            if self.infer_to_mapped(run, source, target, constraint)? {
                return Ok(());
            }
        }
        if self.types_definitely_unrelated(source, target)? {
            return Ok(());
        }
        if self.is_array_type(source)? || self.is_tuple_type(source)? {
            if self.is_tuple_type(target)? {
                return self.infer_tuple_types(run, source, target);
            }
            if self.is_array_type(target)? {
                return self.infer_index_types(run, source, target);
            }
        }
        // getPropertiesOfObjectType does not synthesize intersection members.
        let properties = if self.types.flags(target)? & tf::OBJECT != 0 {
            self.resolve_type_members(target)?;
            self.types
                .structured(target)?
                .properties
                .as_deref()
                .unwrap_or_default()
                .to_vec()
        } else {
            Vec::new()
        };
        for target_property in properties {
            let name = self.symbol(target_property)?.name_to_owned();
            if let Some(source_property) =
                self.constituent_property(source, name.as_bytes(), false)?
            {
                let s = self.get_type_of_symbol(source_property)?;
                let s = self.remove_missing_type(
                    s,
                    self.symbol(source_property)?.flags() & sf::OPTIONAL != 0,
                )?;
                let t = self.get_type_of_symbol(target_property)?;
                let t = self.remove_missing_type(
                    t,
                    self.symbol(target_property)?.flags() & sf::OPTIONAL != 0,
                )?;
                self.infer_from_types(run, s, t)?;
            }
        }
        self.infer_signatures(run, source, target, false)?;
        self.infer_signatures(run, source, target, true)?;
        self.infer_index_types(run, source, target)
    }

    // port: tsc/internal/checker/inference.go:Checker.typesDefinitelyUnrelated
    pub(crate) fn types_definitely_unrelated(
        &mut self,
        source: TypeId,
        target: TypeId,
    ) -> Result<bool, Error> {
        if self.is_tuple_type(source)? && self.is_tuple_type(target)? {
            let s = self.types.tuple(self.types.target(source)?)?;
            let t = self.types.tuple(self.types.target(target)?)?;
            return Ok(
                t.combined_flags & ef::VARIADIC == 0 && t.min_length > s.min_length
                    || t.combined_flags & ef::VARIABLE == 0
                        && (s.combined_flags & ef::VARIABLE != 0
                            || t.fixed_length < s.fixed_length),
            );
        }
        Ok(self
            .unmatched_property(source, target, false, true)?
            .is_some()
            && self
                .unmatched_property(target, source, false, false)?
                .is_some())
    }

    // port: tsc/internal/checker/relater.go:Checker.getUnmatchedPropertiesWorker
    pub(crate) fn unmatched_property(
        &mut self,
        source: TypeId,
        target: TypeId,
        require_optional: bool,
        discriminants: bool,
    ) -> Result<Option<tsr_arena::SymbolId>, Error> {
        for property in self.get_properties_of_type(target)? {
            let read = self.symbol(property)?;
            let name = read.name_to_owned();
            if self.property_modifiers(property)? & tsr_ast::modifier_flags::STATIC != 0 {
                if let Some(node) = read.value_declaration() {
                    if let Some(name) = self.node(node)?.name() {
                        if self.node(name)?.kind() == tsr_ast::SyntaxKind::PrivateIdentifier {
                            continue;
                        }
                    }
                }
            }
            if require_optional
                || read.flags() & sf::OPTIONAL == 0 && read.check_flags() & cf::PARTIAL == 0
            {
                let Some(other) = self.constituent_property(source, name.as_bytes(), false)? else {
                    return Ok(Some(property));
                };
                if discriminants {
                    let target_type = self.get_type_of_symbol(property)?;
                    if self.types.flags(target_type)? & tf::UNIT != 0 {
                        let source_type = self.get_type_of_symbol(other)?;
                        if self.types.flags(source_type)? & tf::ANY == 0
                            && self.get_regular_type_of_literal_type(source_type)?
                                != self.get_regular_type_of_literal_type(target_type)?
                        {
                            return Ok(Some(property));
                        }
                    }
                }
            }
        }
        Ok(None)
    }

    // port: tsc/internal/checker/inference.go:Checker.inferFromIndexTypes
    fn infer_index_types(
        &mut self,
        run: &mut InferenceRun,
        source: TypeId,
        target: TypeId,
    ) -> Result<(), Error> {
        let priority = if self.types.object_flags(source)?
            & self.types.object_flags(target)?
            & of::MAPPED
            != 0
        {
            p::HOMOMORPHIC
        } else {
            p::NONE
        };
        let indexes = self.index_infos_of_type(target)?;
        if self.inferable_index(source)? {
            for &index in &indexes {
                let target_info = self.signatures.index_info(index)?.clone();
                let mut types = Vec::new();
                for property in self.get_properties_of_type(source)? {
                    let key = self.literal_type_from_property(
                        property,
                        tf::STRING_OR_NUMBER_LITERAL_OR_UNIQUE,
                    )?;
                    if self.applicable_index_type(key, target_info.key_type)? {
                        let mut ty = self.get_type_of_symbol(property)?;
                        if self.symbol(property)?.flags() & sf::OPTIONAL != 0 {
                            ty = self.remove_missing_or_undefined(ty)?;
                        }
                        types.push(ty);
                    }
                }
                for index in self.index_infos_of_type(source)? {
                    let info = self.signatures.index_info(index)?.clone();
                    if self.applicable_index_type(info.key_type, target_info.key_type)? {
                        types.push(info.value_type);
                    }
                }
                if !types.is_empty() {
                    let source = self.get_union_type(&types)?;
                    self.infer_with_priority(run, source, target_info.value_type, priority)?;
                }
            }
        }
        for index in indexes {
            let target_info = self.signatures.index_info(index)?.clone();
            if let Some(index) = self.applicable_index_info(source, target_info.key_type)? {
                self.infer_with_priority(
                    run,
                    self.signatures.index_info(index)?.value_type,
                    target_info.value_type,
                    priority,
                )?;
            }
        }
        Ok(())
    }

    // port: tsc/internal/checker/inference.go:Checker.inferToMappedType
    fn infer_to_mapped(
        &mut self,
        run: &mut InferenceRun,
        source: TypeId,
        target: TypeId,
        constraint: TypeId,
    ) -> Result<bool, Error> {
        let flags = self.types.flags(constraint)?;
        if flags & tf::UNION_OR_INTERSECTION != 0 {
            let mut result = false;
            for part in self.types.types_of(constraint)?.to_vec() {
                result |= self.infer_to_mapped(run, source, target, part)?;
            }
            return Ok(result);
        }
        if flags & tf::INDEX != 0 {
            if let Some(index) = self.inference_index(run, self.types.target(constraint)?)? {
                if !self.inference_context(run.context)?.inferences[index].fixed {
                    if let Some(inferred) =
                        self.infer_homomorphic_type(source, target, constraint)?
                    {
                        let parameter =
                            self.inference_context(run.context)?.inferences[index].parameter;
                        let priority =
                            if self.types.object_flags(source)? & of::NON_INFERRABLE_TYPE != 0 {
                                p::PARTIAL_HOMOMORPHIC
                            } else {
                                p::HOMOMORPHIC
                            };
                        self.infer_with_priority(run, inferred, parameter, priority)?;
                    }
                }
            }
            return Ok(true);
        }
        if flags & tf::TYPE_PARAMETER != 0 {
            let flags = if self.bindings.pattern_for_type.contains_key(&source) {
                crate::indexes::NO_INDEX_SIGNATURES
            } else {
                0
            };
            let index = self.get_index_type(source, flags)?;
            self.infer_with_priority(run, index, constraint, p::MAPPED_CONSTRAINT)?;
            if let Some(extended) = self.constraint_of_type_parameter(constraint)? {
                if self.infer_to_mapped(run, source, target, extended)? {
                    return Ok(true);
                }
            }
            let mut types = Vec::new();
            for property in self.get_properties_of_type(source)? {
                types.push(self.get_type_of_symbol(property)?);
            }
            for index in self.index_infos_of_type(source)? {
                types.push(if index == self.builtins.enum_number_index_info {
                    self.builtins.never_type
                } else {
                    self.signatures.index_info(index)?.value_type
                });
            }
            let source = self.get_union_type(&types)?;
            let target = self.mapped_template(target)?;
            self.infer_from_types(run, source, target)?;
            return Ok(true);
        }
        Ok(false)
    }
}

#[cfg(test)]
mod native_contract_tests {
    use super::*;
    use crate::{mapper::Mapper, CheckerOptions, CheckerOwner};
    use serde_json::json;
    use tsr_arena::{CheckerIdentity, Counters, Generation};
    use tsr_ast::{FactoryMethods, JsString, RuntimeFactory};

    #[test]
    fn native_inference_mapper_contract() {
        let expected: serde_json::Value = serde_json::from_slice(include_bytes!(
            "../tests/fixtures/c2_inference_mapper/observations.json"
        ))
        .unwrap();
        let expected = &expected["observation"];
        let counters = Counters::new();
        let generation = Generation::new(&counters);
        let identity = CheckerIdentity::new(generation, &counters);
        let owner = std::sync::Arc::new(
            CheckerOwner::new(identity, &counters, CheckerOptions::default()).unwrap(),
        );
        let mut operation = owner.operation().unwrap();
        let state = operation.state_mut();
        // The native request uses noLib. Program initialization installs this
        // sentinel when Array/ReadonlyArray declarations are absent.
        state
            .query
            .global_types
            .insert("Array", state.builtins.empty_generic_type);
        state
            .query
            .global_types
            .insert("ReadonlyArray", state.builtins.empty_generic_type);
        let parameter = state.new_type_parameter(None).unwrap();
        let this = state.new_type_parameter(None).unwrap();
        state.types.type_parameter_mut(this).unwrap().is_this_type = true;
        let string = state
            .factory
            .new_keyword_type_node(tsr_ast::SyntaxKind::StringKeyword.into());
        let nodes = state.factory.alloc_nodes(vec![Some(string)]);
        let arguments = state
            .factory
            .alloc_list(tsr_core::TextRange::new(-1, -1), nodes);
        let node = state.factory.new_type_reference_node(None, Some(arguments));
        let any = state.builtins.any_type;
        let mappers = [
            state.new_array_to_single_type_mapper(&[], any).unwrap(),
            state
                .new_array_to_single_type_mapper(&[parameter], any)
                .unwrap(),
            state.new_array_to_single_type_mapper(&[this], any).unwrap(),
            state
                .new_array_to_single_type_mapper(&[parameter, this], any)
                .unwrap(),
            state.new_type_mapper(&[parameter], &[any]).unwrap(),
            state
                .new_type_mapper(&[parameter, this], &[any, any])
                .unwrap(),
            state
                .alloc_mapper(Mapper::DeferredArguments {
                    node,
                    sources: [this].as_slice().into(),
                })
                .unwrap(),
        ];
        let mut observed = Vec::new();
        for &mapper in &mappers {
            let mut mapped = Vec::new();
            for ty in [parameter, this, state.builtins.string_type] {
                let ty = state.map_type_parameter(ty, mapper).unwrap();
                mapped.push(if ty == parameter {
                    "parameter"
                } else if ty == this {
                    "this"
                } else if ty == any {
                    "any"
                } else if ty == state.builtins.string_type {
                    "string"
                } else {
                    panic!("unexpected mapped type")
                });
            }
            let compare: Vec<_> = mappers
                .iter()
                .map(|&other| {
                    match state
                        .compare_type_mappers(Some(mapper), Some(other))
                        .unwrap()
                    {
                        std::cmp::Ordering::Less => -1,
                        std::cmp::Ordering::Equal => 0,
                        std::cmp::Ordering::Greater => 1,
                    }
                })
                .collect();
            observed.push(json!({"mapped":mapped, "maps_this_only":state.mapper_maps_this_only(mapper).unwrap(), "compare":compare}));
        }
        assert_eq!(json!(observed), expected["mappers"]);

        let mut observed = Vec::new();
        for scenario in ["ordinary-index", "pattern-index", "enum-index"] {
            let key = state.new_type_parameter(None).unwrap();
            let value = state.new_type_parameter(None).unwrap();
            let target = state.new_object_type(of::MAPPED, None).unwrap();
            state.types.mapped_mut(target).unwrap().template_type = Some(value);
            let mut members = tsr_ast::SymbolTable::default();
            let index = if scenario == "enum-index" {
                state.builtins.enum_number_index_info
            } else {
                let prop = state
                    .new_symbol(sf::PROPERTY, JsString::from_bytes(b"x".as_slice()))
                    .unwrap();
                state.value_symbol_links.get_or_default(prop).resolved_type =
                    Some(state.builtins.string_type);
                members.insert(JsString::from_bytes(b"x".as_slice()), Some(prop));
                state
                    .signatures
                    .new_index_info(
                        state.builtins.string_type,
                        state.builtins.string_type,
                        false,
                        None,
                        None,
                    )
                    .unwrap()
            };
            let members = state.alloc_symbol_table(members);
            let source = state
                .new_anonymous_type(None, Some(members), &[], &[], &[index])
                .unwrap();
            if scenario == "pattern-index" {
                state.bindings.pattern_for_type.insert(source, node);
            }
            let context = state.new_inference_context(&[key, value], None, 0).unwrap();
            let mut run = InferenceRun {
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
            };
            state
                .infer_to_mapped(&mut run, source, target, key)
                .unwrap();
            let infos = state.inference_context(context).unwrap().inferences.clone();
            let mut inferred = Vec::new();
            for info in infos {
                let ty = if !info.candidates.is_empty() {
                    state
                        .get_union_type_ex(
                            &info.candidates,
                            crate::UnionReduction::Subtype,
                            None,
                            None,
                        )
                        .unwrap()
                } else if !info.contra_candidates.is_empty() {
                    state
                        .get_intersection_type(&info.contra_candidates)
                        .unwrap()
                } else {
                    panic!("missing inference")
                };
                inferred.push(
                    String::from_utf8(state.type_to_string(ty, 0).unwrap().as_bytes().to_vec())
                        .unwrap(),
                );
            }
            observed.push(json!({"id":scenario,"inferred":inferred}));
        }
        assert_eq!(json!(observed), expected["mapped_inference"]);

        let u = state.new_type_parameter(None).unwrap();
        let source = state
            .create_tuple_type_ex(
                &[parameter],
                &[crate::TupleElementInfo {
                    flags: ef::VARIADIC,
                    labeled_declaration: None,
                }],
                false,
            )
            .unwrap();
        let length = state
            .new_symbol(sf::PROPERTY, JsString::from_bytes(b"length".as_slice()))
            .unwrap();
        state
            .value_symbol_links
            .get_or_default(length)
            .resolved_type = Some(u);
        let mut members = tsr_ast::SymbolTable::default();
        members.insert(JsString::from_bytes(b"length".as_slice()), Some(length));
        let members = state.alloc_symbol_table(members);
        let object = state
            .new_anonymous_type(None, Some(members), &[], &[], &[])
            .unwrap();
        state.types.get_mut(object).unwrap().object_flags |=
            of::COULD_CONTAIN_TYPE_VARIABLES | of::COULD_CONTAIN_TYPE_VARIABLES_COMPUTED;
        let target = state.get_intersection_type(&[source, object]).unwrap();
        let context = state.new_inference_context(&[u], None, 0).unwrap();
        state
            .infer_types(context, source, target, p::NONE, false)
            .unwrap();
        let candidates = state.inference_context(context).unwrap().inferences[0]
            .candidates
            .clone();
        let mut observed = json!({
            "generic_tuple": state.is_generic_tuple_type(source).unwrap(),
            "candidate_count": candidates.len()
        });
        if !candidates.is_empty() {
            let inferred = state
                .get_union_type_ex(&candidates, crate::UnionReduction::Subtype, None, None)
                .unwrap();
            observed["inferred"] = json!(String::from_utf8(
                state
                    .type_to_string(inferred, 0)
                    .unwrap()
                    .as_bytes()
                    .to_vec()
            )
            .unwrap());
        }
        assert_eq!(observed, expected["tuple_intersection"]);

        let signature = state
            .signatures
            .new_signature(
                0,
                None,
                Some([parameter].as_slice().into()),
                None,
                None,
                Some(parameter),
                None,
                0,
            )
            .unwrap();
        state.permissive_instantiation(parameter).unwrap();
        state.restrictive_instantiation(parameter).unwrap();
        let mut observed = Vec::new();
        for mapper in [
            state.conditional.permissive_mapper.unwrap(),
            state.conditional.restrictive_mapper.unwrap(),
        ] {
            let instantiated = state.instantiate_signature(signature, mapper).unwrap();
            let read = state.signatures.get(instantiated).unwrap();
            let parameters = read.type_parameters.as_deref().unwrap_or_default().len();
            let cached = read.resolved_return_type.is_some();
            let result = state.return_type_of_signature(instantiated).unwrap();
            observed.push(
                json!({"type_parameters":parameters,"return_cached_before":cached,
                "return_is_wildcard": result == state.builtins.wildcard_type}),
            );
        }
        assert_eq!(json!(observed), expected["generic_signatures"]);

        let mut observed = Vec::new();
        for (name, present) in [
            ("NoDefault", false),
            ("UnknownDefault", true),
            ("CircularDefault", true),
        ] {
            let name_node = state
                .factory
                .new_identifier(JsString::from_bytes(b"T".as_slice()));
            let default = present.then(|| {
                if name == "UnknownDefault" {
                    state
                        .factory
                        .new_keyword_type_node(tsr_ast::SyntaxKind::UnknownKeyword.into())
                } else {
                    state
                        .factory
                        .new_identifier(JsString::from_bytes(b"T".as_slice()))
                }
            });
            let declaration = state.factory.new_type_parameter_declaration(
                None,
                Some(name_node),
                None,
                None,
                default,
            );
            let symbol = state
                .new_symbol(sf::TYPE_PARAMETER, JsString::from_bytes(b"T".as_slice()))
                .unwrap();
            let declarations = state.declarations.alloc_one(Some(declaration)).unwrap();
            state.symbol_mut(symbol).unwrap().declarations = declarations;
            let parameter = state.new_type_parameter(Some(symbol)).unwrap();
            let before = state
                .types
                .type_parameter(parameter)
                .unwrap()
                .resolved_default_type
                .is_some();
            let syntactic = state.inference_parameter_has_default(parameter).unwrap();
            let after = state
                .types
                .type_parameter(parameter)
                .unwrap()
                .resolved_default_type
                .is_some();
            observed.push(json!({"name":name,"syntactic":syntactic,"cached_before":before,"cached_after":after}));
        }
        assert_eq!(json!(observed), expected["defaults"]);
    }
}
