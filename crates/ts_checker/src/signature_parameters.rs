//! Effective parameter lists expand fixed rest tuples before comparison.
use crate::{
    element_flags as ef, signature_flags as sg, type_flags as tf, CheckerState, Error, SignatureId,
    TypeId,
};
use ts_arena::SymbolId;

impl CheckerState {
    pub(crate) fn strict_function_types(&self) -> bool {
        self.program.as_ref().is_some_and(|program| {
            let options = program.host.options();
            options.strict_option_value(options.strict_function_types)
        })
    }

    // port: tsc/internal/checker/relater.go:Checker.isInstantiatedGenericParameter
    pub(crate) fn instantiated_generic_parameter(
        &mut self,
        signature: SignatureId,
        position: usize,
    ) -> Result<bool, Error> {
        let Some(target) = self.signatures.get(signature)?.target else {
            return Ok(false);
        };
        let Some(ty) = self.parameter_type_at(target, position)? else {
            return Ok(false);
        };
        Ok(self.get_generic_object_flags(ty)? & crate::object_flags::IS_GENERIC_TYPE != 0)
    }

    // port: tsc/internal/checker/checker.go:Checker.getDefaultOrUnknownFromTypeParameter
    pub(crate) fn default_or_unknown(&mut self, parameter: TypeId) -> Result<TypeId, Error> {
        let default = self.resolved_type_parameter_default(parameter)?;
        Ok(
            if default == self.builtins.no_constraint_type
                || default == self.builtins.circular_constraint_type
            {
                self.builtins.unknown_type
            } else {
                default
            },
        )
    }

    // port: tsc/internal/checker/checker.go:Checker.getNonCircularReturnTypeOfSignature
    pub(crate) fn non_circular_return_type(
        &mut self,
        signature: SignatureId,
    ) -> Result<TypeId, Error> {
        if self.resolving_signature_return(signature)? {
            return Ok(self.builtins.any_type);
        }
        self.return_type_of_signature(signature)
    }

    // port: tsc/internal/checker/checker.go:Checker.getSingleCallSignature
    pub(crate) fn non_nullable_single_signature(
        &mut self,
        ty: TypeId,
    ) -> Result<Option<SignatureId>, Error> {
        let ty = self.non_nullable_type(ty)?;
        if self.types.flags(ty)? & tf::OBJECT == 0 {
            return Ok(None);
        }
        self.resolve_type_members(ty)?;
        let data = self.types.structured(ty)?;
        let signatures = data.signatures.as_deref().unwrap_or_default();
        if data
            .properties
            .as_ref()
            .is_none_or(|properties| properties.is_empty())
            && data
                .index_infos
                .as_ref()
                .is_none_or(|indexes| indexes.is_empty())
            && data.call_signature_count == 1
            && signatures.len() == 1
        {
            return Ok(Some(signatures[0]));
        }
        Ok(None)
    }

    // port: tsc/internal/checker/checker.go:Checker.getTypeFactsWorker
    // This caller only requests IsUndefinedOrNull. All non-nullable leaf
    // families (including any/unknown/void) contribute zero to that mask.
    pub(crate) fn nullable_parameter_facts(&mut self, mut ty: TypeId) -> Result<u32, Error> {
        if self.types.flags(ty)? & (tf::INTERSECTION | tf::INSTANTIABLE) != 0 {
            ty = self
                .base_constraint_of_type(ty)?
                .unwrap_or(self.builtins.unknown_type);
        }
        let flags = self.types.flags(ty)?;
        if flags & tf::UNION != 0 {
            let parts = self.types.compound_types(ty)?.clone();
            let mut facts = 0;
            for &part in parts.iter() {
                facts |= self.nullable_parameter_facts(part)?;
            }
            return Ok(facts);
        }
        if flags & tf::INTERSECTION != 0 {
            let parts = self.types.compound_types(ty)?.clone();
            let mut facts = 3;
            for &part in parts.iter() {
                facts &= self.nullable_parameter_facts(part)?;
            }
            return Ok(facts);
        }
        Ok(u32::from(flags & tf::UNDEFINED != 0) | u32::from(flags & tf::NULL != 0) << 1)
    }

    // port: tsc/internal/checker/checker.go:Checker.getTypeOfParameter
    pub(crate) fn type_of_parameter(&mut self, symbol: SymbolId) -> Result<TypeId, Error> {
        let declaration = self.symbol(symbol)?.value_declaration();
        let optional = if let Some(node) = declaration {
            let read = self.ast(node)?.node(node)?;
            read.initializer().is_some() || read.question_token(self.ast(node)?)?.is_some()
        } else {
            false
        };
        let ty = self.get_type_of_symbol(symbol)?;
        self.add_type_optionality(ty, false, optional)
    }

    fn rest_parameter_type(&mut self, signature: SignatureId) -> Result<Option<TypeId>, Error> {
        let sig = self.signatures.get(signature)?;
        if sig.flags & sg::HAS_REST_PARAMETER == 0 {
            return Ok(None);
        }
        let symbol = *sig
            .parameters
            .as_deref()
            .unwrap_or_default()
            .last()
            .ok_or(Error::MissingLink("signature rest parameter"))?;
        self.get_type_of_symbol(symbol).map(Some)
    }

    // port: tsc/internal/checker/relater.go:Checker.getParameterCount
    pub(crate) fn parameter_count(&mut self, signature: SignatureId) -> Result<usize, Error> {
        let count = self
            .signatures
            .get(signature)?
            .parameters
            .as_deref()
            .unwrap_or_default()
            .len();
        if let Some(rest) = self.rest_parameter_type(signature)? {
            if self.is_tuple_type(rest)? {
                let data = self.types.tuple(self.types.target(rest)?)?;
                return Ok(count + data.fixed_length as usize
                    - usize::from(data.combined_flags & ef::VARIABLE == 0));
            }
        }
        Ok(count)
    }

    // port: tsc/internal/checker/relater.go:Checker.getMinArgumentCount
    pub(crate) fn min_argument_count(&mut self, signature: SignatureId) -> Result<usize, Error> {
        self.min_argument_count_ex(signature, 0)
    }

    // port: tsc/internal/checker/relater.go:Checker.getMinArgumentCountEx
    /// Native flags: StrongArityForUntypedJS = 1, VoidIsNonOptional = 2.
    /// The latter bypasses the resolved cache and never evaluates parameter
    /// types merely to decide whether a trailing `void` makes them optional.
    pub(crate) fn min_argument_count_ex(
        &mut self,
        signature: SignatureId,
        flags: u32,
    ) -> Result<usize, Error> {
        let strong_arity = flags & 1 != 0;
        let void_non_optional = flags & 2 != 0;
        let sig = self.signatures.get(signature)?;
        if !void_non_optional && sig.resolved_min_argument_count >= 0 {
            return Ok(sig.resolved_min_argument_count as usize);
        }
        let count = sig.parameters.as_deref().unwrap_or_default().len();
        let mut minimum = None;
        if let Some(rest) = self.rest_parameter_type(signature)? {
            if self.is_tuple_type(rest)? {
                let data = self.types.tuple(self.types.target(rest)?)?;
                let required = data
                    .element_infos
                    .iter()
                    .position(|info| info.flags & ef::REQUIRED == 0)
                    .unwrap_or(data.fixed_length as usize);
                if required > 0 {
                    minimum = Some(count - 1 + required);
                }
            }
        }
        let mut minimum = if let Some(minimum) = minimum {
            minimum
        } else {
            let sig = self.signatures.get(signature)?;
            if !strong_arity && sig.flags & sg::IS_UNTYPED_SIGNATURE_IN_JS_FILE != 0 {
                return Ok(0);
            }
            sig.min_argument_count as usize
        };
        if void_non_optional {
            return Ok(minimum);
        }
        while minimum > 0 {
            let ty = self
                .parameter_type_at(signature, minimum - 1)?
                .unwrap_or(self.builtins.any_type);
            if !self.maybe_type_of_kind(ty, tf::VOID)? {
                break;
            }
            minimum -= 1;
        }
        self.signatures
            .get_mut(signature)?
            .resolved_min_argument_count = minimum as i32;
        Ok(minimum)
    }

    // port: tsc/internal/checker/relater.go:Checker.hasEffectiveRestParameter
    pub(crate) fn effective_rest_parameter(
        &mut self,
        signature: SignatureId,
    ) -> Result<bool, Error> {
        let Some(rest) = self.rest_parameter_type(signature)? else {
            return Ok(false);
        };
        Ok(!self.is_tuple_type(rest)?
            || self.types.tuple(self.types.target(rest)?)?.combined_flags & ef::VARIABLE != 0)
    }

    // port: tsc/internal/checker/relater.go:Checker.tryGetTypeAtPosition
    pub(crate) fn parameter_type_at(
        &mut self,
        signature: SignatureId,
        position: usize,
    ) -> Result<Option<TypeId>, Error> {
        let sig = self.signatures.get(signature)?;
        let rest = sig.flags & sg::HAS_REST_PARAMETER != 0;
        let parameters = sig.parameters.clone().unwrap_or_else(|| [].into());
        let count = parameters.len() - usize::from(rest);
        if position < count {
            return self.type_of_parameter(parameters[position]).map(Some);
        }
        if rest {
            let ty = self.get_type_of_symbol(parameters[count])?;
            let index = position - count;
            if !self.is_tuple_type(ty)?
                || self.types.tuple(self.types.target(ty)?)?.combined_flags & ef::VARIABLE != 0
                || index < self.types.tuple(self.types.target(ty)?)?.fixed_length as usize
            {
                let index = self.get_number_literal_type(ts_jsnum::Number::new(index as f64))?;
                return self
                    .get_indexed_access_type(ty, index, 0, None, None)
                    .map(Some);
            }
        }
        Ok(None)
    }

    // port: tsc/internal/checker/relater.go:Checker.getEffectiveRestType
    pub(crate) fn effective_rest_type(
        &mut self,
        signature: SignatureId,
    ) -> Result<Option<TypeId>, Error> {
        let Some(rest) = self.rest_parameter_type(signature)? else {
            return Ok(None);
        };
        if !self.is_tuple_type(rest)? {
            return Ok(Some(if self.types.flags(rest)? & tf::ANY != 0 {
                *self
                    .query
                    .global_types
                    .get("anyArrayType")
                    .ok_or(Error::MissingLink("any array global"))?
            } else {
                rest
            }));
        }
        let data = self.types.tuple(self.types.target(rest)?)?;
        if data.combined_flags & ef::VARIABLE == 0 {
            return Ok(None);
        }
        let start = data.fixed_length as usize;
        let infos = data.element_infos.clone();
        let args = self.get_type_arguments(rest)?;
        self.create_tuple_type_ex(&args[start..], &infos[start..], false)
            .map(Some)
    }

    // port: tsc/internal/checker/relater.go:Checker.getNonArrayRestType
    pub(crate) fn non_array_rest_type(
        &mut self,
        signature: SignatureId,
    ) -> Result<Option<TypeId>, Error> {
        let Some(rest) = self.effective_rest_type(signature)? else {
            return Ok(None);
        };
        Ok((!self.is_array_type(rest)? && self.types.flags(rest)? & tf::ANY == 0).then_some(rest))
    }

    // port: tsc/internal/checker/relater.go:Checker.getRestTypeAtPosition
    pub(crate) fn rest_type_at(
        &mut self,
        signature: SignatureId,
        position: usize,
    ) -> Result<TypeId, Error> {
        self.rest_type_at_ex(signature, position, false)
    }

    pub(crate) fn rest_type_at_ex(
        &mut self,
        signature: SignatureId,
        position: usize,
        readonly: bool,
    ) -> Result<TypeId, Error> {
        let count = self.parameter_count(signature)?;
        let minimum = self.min_argument_count(signature)?;
        let rest = self.effective_rest_type(signature)?;
        if let Some(rest) = rest {
            if position + 1 >= count {
                if position + 1 == count {
                    return Ok(rest);
                }
                let ty =
                    self.get_indexed_access_type(rest, self.builtins.number_type, 0, None, None)?;
                return self.create_array_type(ty, false);
            }
        }
        let mut types = Vec::new();
        let mut infos = Vec::new();
        for i in position..count {
            let (ty, flags) = if let Some(rest) = rest.filter(|_| i + 1 == count) {
                (rest, ef::VARIADIC)
            } else {
                (
                    self.parameter_type_at(signature, i)?
                        .unwrap_or(self.builtins.any_type),
                    if i < minimum {
                        ef::REQUIRED
                    } else {
                        ef::OPTIONAL
                    },
                )
            };
            types.push(ty);
            let label = self.parameter_label_at(signature, i)?;
            infos.push(crate::types::TupleElementInfo {
                flags,
                labeled_declaration: label,
            });
        }
        self.create_tuple_type_ex(&types, &infos, readonly)
    }

    // port: tsc/internal/checker/relater.go:Checker.getNameableDeclarationAtPosition
    fn parameter_label_at(
        &mut self,
        signature: SignatureId,
        position: usize,
    ) -> Result<Option<ts_arena::NodeId>, Error> {
        let sig = self.signatures.get(signature)?;
        let rest = sig.flags & sg::HAS_REST_PARAMETER != 0;
        let parameters = sig.parameters.clone().unwrap_or_else(|| [].into());
        let count = parameters.len() - usize::from(rest);
        let declaration = if position < count {
            self.symbol(parameters[position])?.value_declaration()
        } else if rest {
            let ty = self.get_type_of_symbol(parameters[count])?;
            if self.is_tuple_type(ty)? {
                return Ok(self
                    .types
                    .tuple(self.types.target(ty)?)?
                    .element_infos
                    .get(position - count)
                    .and_then(|info| info.labeled_declaration));
            }
            self.symbol(parameters[count])?.value_declaration()
        } else {
            None
        };
        let Some(declaration) = declaration else {
            return Ok(None);
        };
        let read = self.ast(declaration)?.node(declaration)?;
        if read.kind() == ts_ast::SyntaxKind::NamedTupleMember {
            return Ok(Some(declaration));
        }
        if read.kind() == ts_ast::SyntaxKind::Parameter {
            if let Some(name) = read.name() {
                if self.ast(name)?.node(name)?.kind() == ts_ast::SyntaxKind::Identifier {
                    return Ok(Some(declaration));
                }
            }
        }
        Ok(None)
    }

    // port: tsc/internal/checker/relater.go:Checker.getRestOrAnyTypeAtPosition
    pub(crate) fn rest_or_any_at(
        &mut self,
        signature: SignatureId,
        position: usize,
    ) -> Result<TypeId, Error> {
        let rest = self.rest_type_at(signature, position)?;
        if self.is_array_type(rest)? {
            let element = self.get_type_arguments(rest)?[0];
            if self.types.flags(element)? & tf::ANY != 0 {
                return Ok(self.builtins.any_type);
            }
        }
        Ok(rest)
    }

    // port: tsc/internal/checker/relater.go:Checker.isTopSignature
    pub(crate) fn top_signature(&mut self, signature: SignatureId) -> Result<bool, Error> {
        let sig = self.signatures.get(signature)?;
        if sig
            .type_parameters
            .as_ref()
            .is_some_and(|params| !params.is_empty())
            || sig.parameters.as_deref().unwrap_or_default().len() != 1
            || sig.flags & sg::HAS_REST_PARAMETER == 0
        {
            return Ok(false);
        }
        let parameter = sig.parameters.as_deref().unwrap_or_default()[0];
        let this = sig.this_parameter;
        if let Some(this) = this {
            let ty = self.type_of_parameter(this)?;
            if self.types.flags(ty)? & tf::ANY == 0 {
                return Ok(false);
            }
        }
        let parameter = self.type_of_parameter(parameter)?;
        let rest = if self.is_array_type(parameter)? {
            self.get_type_arguments(parameter)?[0]
        } else {
            parameter
        };
        if self.types.flags(rest)? & (tf::ANY | tf::NEVER) == 0 {
            return Ok(false);
        }
        let result = self.return_type_of_signature(signature)?;
        Ok(self.types.flags(result)? & tf::ANY_OR_UNKNOWN != 0)
    }

    // port: tsc/internal/checker/checker.go:Checker.getErasedSignature
    pub(crate) fn erased_signature(
        &mut self,
        signature: SignatureId,
    ) -> Result<SignatureId, Error> {
        let sig = self.signatures.get(signature)?;
        let Some(parameters) = sig
            .type_parameters
            .clone()
            .filter(|parameters| !parameters.is_empty())
        else {
            return Ok(signature);
        };
        if let Some(erased) = sig.erased {
            return Ok(erased);
        }
        let mapper =
            self.new_type_mapper(&parameters, &vec![self.builtins.any_type; parameters.len()])?;
        let erased = self.instantiate_signature_ex(signature, mapper, true)?;
        self.signatures.get_mut(signature)?.erased = Some(erased);
        Ok(erased)
    }
}

impl CheckerState {
    // port: tsc/internal/checker/relater.go:Checker.getParameterNameAtPosition
    pub(crate) fn parameter_name_at(
        &mut self,
        signature: SignatureId,
        position: usize,
    ) -> Result<ts_ast::JsString, Error> {
        let sig = self.signatures.get(signature)?;
        let parameters = sig.parameters.clone().unwrap_or_else(|| [].into());
        let count = parameters.len() - usize::from(sig.flags & sg::HAS_REST_PARAMETER != 0);
        if position < count {
            return Ok(self.symbol(parameters[position])?.name_to_owned());
        }
        let rest = *parameters
            .get(count)
            .ok_or(Error::MissingLink("signature rest name"))?;
        let ty = self.get_type_of_symbol(rest)?;
        if !self.is_tuple_type(ty)? {
            return Ok(self.symbol(rest)?.name_to_owned());
        }
        let index = position - count;
        let info = *self
            .types
            .tuple(self.types.target(ty)?)?
            .element_infos
            .get(index)
            .ok_or(Error::MissingLink("tuple parameter label"))?;
        self.tuple_element_label(info, rest, index)
    }

    // port: tsc/internal/checker/relater.go:Checker.getTupleElementLabel
    pub(crate) fn tuple_element_label(
        &self,
        info: crate::TupleElementInfo,
        rest: SymbolId,
        index: usize,
    ) -> Result<ts_ast::JsString, Error> {
        if let Some(node) = info.labeled_declaration {
            let name = self
                .ast(node)?
                .node(node)?
                .name()
                .ok_or(Error::MissingLink("named tuple parameter"))?;
            return Ok(self.ast(name)?.node_text(name)?.into_js_string());
        }
        if let Some(declaration) = self.symbol(rest)?.value_declaration() {
            if self.ast(declaration)?.node(declaration)?.kind() == ts_ast::SyntaxKind::Parameter {
                return self.tuple_label_from_binding(declaration, index, info.flags);
            }
        }
        let mut name = self.symbol(rest)?.name_bytes().to_vec();
        name.extend_from_slice(format!("_{index}").as_bytes());
        Ok(ts_ast::JsString::from_bytes(name))
    }

    // port: tsc/internal/checker/relater.go:Checker.getTupleElementLabelFromBindingElement
    fn tuple_label_from_binding(
        &self,
        node: ts_arena::NodeId,
        index: usize,
        flags: crate::ElementFlags,
    ) -> Result<ts_ast::JsString, Error> {
        use ts_ast::SyntaxKind as K;
        let read = self.ast(node)?.node(node)?;
        let rest = match read.kind().known() {
            Some(K::Parameter) => read
                .data_source()
                .as_parameter_declaration()
                .ok_or(Error::MissingLink("parameter label"))?
                .dot_dot_dot_token()
                .is_some(),
            Some(K::BindingElement) => read
                .data_source()
                .as_binding_element()
                .ok_or(Error::MissingLink("binding label"))?
                .dot_dot_dot_token()
                .is_some(),
            _ => false,
        };
        if let Some(name) = read.name() {
            let name_read = self.ast(name)?.node(name)?;
            if name_read.kind() == K::Identifier {
                let mut text = self.ast(name)?.node_text(name)?.as_bytes().to_vec();
                if rest {
                    if flags & ef::VARIABLE == 0 {
                        text.extend_from_slice(format!("_{index}").as_bytes());
                    }
                } else if flags & ef::FIXED == 0 {
                    text.extend_from_slice(b"_n");
                }
                return Ok(ts_ast::JsString::from_bytes(text));
            }
            if name_read.kind() == K::ArrayBindingPattern && rest {
                let elements = self.source_list(name, name_read.element_list())?;
                let last_rest = if let Some(&last) = elements.last() {
                    let read = self.ast(last)?.node(last)?;
                    read.data_source()
                        .as_binding_element()
                        .is_some_and(|data| data.dot_dot_dot_token().is_some())
                } else {
                    false
                };
                let count = elements.len() - usize::from(last_rest);
                if index < count {
                    if self.ast(elements[index])?.node(elements[index])?.kind() == K::BindingElement
                    {
                        return self.tuple_label_from_binding(elements[index], index, flags);
                    }
                } else if last_rest {
                    return self.tuple_label_from_binding(elements[count], index - count, flags);
                }
            }
        }
        Ok(ts_ast::JsString::from_bytes(
            format!("arg_{index}").into_bytes(),
        ))
    }

    // port: tsc/internal/checker/relater.go:Checker.isResolvingReturnTypeOfSignature
    pub(crate) fn resolving_signature_return(&self, signature: SignatureId) -> Result<bool, Error> {
        let sig = self.signatures.get(signature)?;
        if let Some(composite) = &sig.composite {
            for &part in composite.signatures.iter() {
                if self.resolving_signature_return(part)? {
                    return Ok(true);
                }
            }
        }
        if sig.resolved_return_type.is_some() {
            return Ok(false);
        }
        Ok(self
            .resolution
            .find_resolution_cycle_start_index(
                crate::TypeSystemEntity::Signature(signature),
                crate::TypeSystemPropertyName::ResolvedReturnType,
                |entry| {
                    let crate::TypeSystemEntity::Signature(signature) = entry.target else {
                        return false;
                    };
                    entry.property_name == crate::TypeSystemPropertyName::ResolvedReturnType
                        && self
                            .signatures
                            .get(signature)
                            .is_ok_and(|sig| sig.resolved_return_type.is_some())
                },
            )
            .is_some())
    }
}

impl CheckerState {
    // port: tsc/internal/checker/nodebuilderimpl.go:Checker.getExpandedParameters
    // The display caller requests skipUnionExpanding=true. It expands one
    // concrete tuple, leaving a union rest type as one parameter.
    pub(crate) fn expanded_signature_parameters(
        &mut self,
        signature: SignatureId,
    ) -> Result<Vec<SymbolId>, Error> {
        use ts_ast::{check_flags as cf, symbol_flags as sf, JsString};
        let sig = self.signatures.get(signature)?;
        let parameters = sig.parameters.clone().unwrap_or_else(|| [].into());
        if sig.flags & sg::HAS_REST_PARAMETER == 0 {
            return Ok(parameters.to_vec());
        }
        let rest = *parameters
            .last()
            .ok_or(Error::MissingLink("signature rest parameter"))?;
        let ty = self.get_type_of_symbol(rest)?;
        if !self.is_tuple_type(ty)? {
            return Ok(parameters.to_vec());
        }
        let elements = self.get_type_arguments(ty)?;
        let infos = self
            .types
            .tuple(self.types.target(ty)?)?
            .element_infos
            .clone();
        let mut names = Vec::with_capacity(infos.len());
        for (i, &info) in infos.iter().enumerate() {
            names.push(self.tuple_element_label(info, rest, i)?);
        }
        let mut unique = std::collections::HashSet::new();
        let mut duplicates = Vec::new();
        for (i, name) in names.iter().enumerate() {
            if !unique.insert(name.clone()) {
                duplicates.push(i);
            }
        }
        let mut counters = crate::types::Map::default();
        for i in duplicates {
            let mut counter: usize = counters.get(&names[i]).copied().unwrap_or(1);
            loop {
                let mut bytes = names[i].as_bytes().to_vec();
                bytes.extend_from_slice(format!("_{counter}").as_bytes());
                let name = JsString::from_bytes(bytes);
                if unique.insert(name.clone()) {
                    names[i] = name;
                    break;
                }
                counter += 1;
            }
            // The pinned implementation keys this counter by the new name.
            counters.insert(names[i].clone(), counter + 1);
        }
        let mut result = parameters[..parameters.len() - 1].to_vec();
        for (i, &element) in elements.iter().enumerate() {
            let flags = infos[i].flags;
            let check = if flags & ef::VARIABLE != 0 {
                cf::REST_PARAMETER
            } else if flags & ef::OPTIONAL != 0 {
                cf::OPTIONAL_PARAMETER
            } else {
                0
            };
            let symbol =
                self.new_symbol_ex(sf::FUNCTION_SCOPED_VARIABLE, names[i].clone(), check)?;
            let ty = if flags & ef::REST != 0 {
                self.create_array_type(element, false)?
            } else {
                element
            };
            self.value_symbol_links.get_or_default(symbol).resolved_type = Some(ty);
            result.push(symbol);
        }
        Ok(result)
    }
}
