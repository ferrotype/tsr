use crate::{
    element_flags as ef, infer_types::InferenceRun, inference::priority as p, object_flags as of,
    type_flags as tf, CheckerState, Error, TypeId,
};
use ts_ast::{check_flags as cf, symbol_flags as sf};

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
    ) -> Result<Option<ts_arena::SymbolId>, Error> {
        for property in self.get_properties_of_type(target)? {
            let read = self.symbol(property)?;
            let name = read.name_to_owned();
            if self.property_modifiers(property)? & ts_ast::modifier_flags::STATIC != 0 {
                if let Some(node) = read.value_declaration() {
                    if let Some(name) = self.node(node)?.name() {
                        if self.node(name)?.kind() == ts_ast::SyntaxKind::PrivateIdentifier {
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
            let index = self.get_index_type(source, 0)?;
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
                types.push(self.signatures.index_info(index)?.value_type);
            }
            let source = self.get_union_type(&types)?;
            let target = self.mapped_template(target)?;
            self.infer_from_types(run, source, target)?;
            return Ok(true);
        }
        Ok(false)
    }
}
