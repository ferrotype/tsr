//! Type instantiation and its active-mapper cache. Depth and cache state are
//! restored on every Result path. A panic retires the checker operation's owner.

use crate::{
    object_flags as of, type_flags as tf, AliasId, CacheKey, CheckerState, Error, MapperId, TypeId,
    TypeList,
};
use tsr_arena::{NodeId, SymbolId};
use tsr_ast::{check_flags as cf, symbol_flags as sf, SyntaxKind as K};

#[derive(Default)]
pub(crate) struct InstantiationState {
    #[cfg(feature = "recursion-probe")]
    pub(crate) probe: Option<crate::handles::c2_probe::InstantiationProbe>,
    pub(crate) mappers: Vec<crate::mapper::Mapper>,
    // Inactive slots retain only an empty map's allocation, as the pin's
    // activeTypeMappersCaches slice does beyond its length. They must never
    // participate in mapper lookup or keep types reachable.
    active: Vec<(MapperId, crate::types::Map<CacheKey, TypeId>)>,
    active_len: usize,
    pub(crate) depth: u32,
    pub(crate) count: u64,
    pub(crate) total_count: u64,
    pub(crate) reliability_mappers: [Option<MapperId>; 2],
    pub(crate) unique_literal_mapper: Option<MapperId>,
}

impl InstantiationState {
    // port: tsc/internal/checker/checker.go:Checker.pushActiveMapper
    fn push_active_mapper(&mut self, mapper: MapperId) -> usize {
        let index = self.active_len;
        if let Some((id, cache)) = self.active.get_mut(index) {
            debug_assert!(cache.is_empty());
            *id = mapper;
        } else {
            self.active.push((mapper, crate::types::Map::default()));
        }
        self.active_len += 1;
        index
    }

    // port: tsc/internal/checker/checker.go:Checker.popActiveMapper
    fn pop_active_mapper(&mut self) {
        self.active_len -= 1;
        self.active[self.active_len].1.clear();
    }
}

impl CheckerState {
    // port: tsc/internal/checker/checker.go:Checker.couldContainTypeVariablesWorker
    pub(crate) fn could_contain_type_variables(&mut self, ty: TypeId) -> Result<bool, Error> {
        let record = *self.types.get(ty)?;
        if record.flags & tf::STRUCTURED_OR_INSTANTIABLE == 0 {
            return Ok(false);
        }
        if record.object_flags & of::COULD_CONTAIN_TYPE_VARIABLES_COMPUTED != 0 {
            return Ok(record.object_flags & of::COULD_CONTAIN_TYPE_VARIABLES != 0);
        }
        let mut result = record.flags & tf::INSTANTIABLE != 0;
        if !result && !self.is_non_generic_top_level_type(ty)? {
            if record.flags & tf::OBJECT != 0 {
                if record.object_flags & of::REFERENCE != 0 {
                    result = self.types.type_reference(ty)?.node.is_some();
                    if !result {
                        for &argument in self.get_type_arguments(ty)?.iter() {
                            if self.could_contain_type_variables(argument)? {
                                result = true;
                                break;
                            }
                        }
                    }
                }
                if record.object_flags & of::ANONYMOUS != 0 {
                    if let Some(symbol) = record.symbol {
                        result |= self.symbol(symbol)?.flags()
                            & (sf::FUNCTION
                                | sf::METHOD
                                | sf::CLASS
                                | sf::TYPE_LITERAL
                                | sf::OBJECT_LITERAL)
                            != 0
                            && !self.symbol_declarations(symbol)?.is_empty();
                    }
                }
                result |= record.object_flags
                    & (of::MAPPED
                        | of::REVERSE_MAPPED
                        | of::OBJECT_REST_TYPE
                        | of::INSTANTIATION_EXPRESSION_TYPE)
                    != 0;
            } else if record.flags & tf::UNION_OR_INTERSECTION != 0
                && record.flags & tf::ENUM_LITERAL == 0
            {
                let types = self.types.compound_types(ty)?.clone();
                for &ty in types.iter() {
                    if self.could_contain_type_variables(ty)? {
                        result = true;
                        break;
                    }
                }
            }
        }
        self.types.get_mut(ty)?.object_flags |= of::COULD_CONTAIN_TYPE_VARIABLES_COMPUTED
            | if result {
                of::COULD_CONTAIN_TYPE_VARIABLES
            } else {
                0
            };
        Ok(result)
    }

    // port: tsc/internal/checker/checker.go:Checker.isNonGenericTopLevelType
    fn is_non_generic_top_level_type(&self, ty: TypeId) -> Result<bool, Error> {
        let Some(alias) = self.types.alias_of(ty)? else {
            return Ok(false);
        };
        if !alias.type_arguments.is_empty() {
            return Ok(false);
        }
        for declaration in self.symbol_declarations(alias.symbol)?.iter().flatten() {
            let read = self.node(declaration)?;
            if !matches!(
                read.kind().known(),
                Some(K::TypeAliasDeclaration | K::JSTypeAliasDeclaration)
            ) {
                continue;
            }
            let mut parent = read.parent();
            while let Some(node) = parent {
                let read = self.node(node)?;
                match read.kind().known() {
                    Some(K::SourceFile) => return Ok(true),
                    Some(K::ModuleDeclaration) => parent = read.parent(),
                    _ => return Ok(false),
                }
            }
        }
        Ok(false)
    }

    // port: tsc/internal/checker/checker.go:Checker.instantiateType
    pub(crate) fn instantiate_type(
        &mut self,
        ty: TypeId,
        mapper: Option<MapperId>,
    ) -> Result<TypeId, Error> {
        self.instantiate_type_with_alias(ty, mapper, None)
    }

    // port: tsc/internal/checker/checker.go:Checker.instantiateTypeWithAlias
    pub(crate) fn instantiate_type_with_alias(
        &mut self,
        ty: TypeId,
        mapper: Option<MapperId>,
        alias: Option<AliasId>,
    ) -> Result<TypeId, Error> {
        let Some(mapper) = mapper else { return Ok(ty) };
        let mut contains = self.could_contain_type_variables(ty)?;
        if !contains {
            if let Some(alias) = self.types.alias_of(ty)?.cloned() {
                for &argument in alias.type_arguments.iter() {
                    if self.could_contain_type_variables(argument)? {
                        contains = true;
                        break;
                    }
                }
            }
        }
        if !contains {
            return Ok(ty);
        }
        if self.instantiation.depth == 100 || self.instantiation.count >= 5_000_000 {
            #[cfg(feature = "recursion-probe")]
            if let Some(probe) = &mut self.instantiation.probe {
                probe.depth_limit_hits += usize::from(self.instantiation.depth == 100);
                probe.count_limit_hits += usize::from(self.instantiation.count >= 5_000_000);
            }
            let (depth, count) = (self.instantiation.depth, self.instantiation.count);
            self.trace_instant(
                crate::trace::TracePhase::CheckTypes,
                "instantiateType_DepthLimit",
                || {
                    crate::trace::args([
                        ("typeId", crate::trace::int(ty.get())),
                        ("instantiationDepth", crate::trace::int(depth)),
                        ("instantiationCount", crate::trace::int(count)),
                    ])
                },
            );
            self.error_at(
                self.current_node,
                tsr_diagnostics::Type_instantiation_is_excessively_deep_and_possibly_infinite,
                vec![],
            )?;
            return Ok(self.builtins.error_type);
        }
        let mut key = crate::key::KeyBuilder::new();
        key.write_type(ty);
        let alias_parts = self.alias_key_parts(alias)?;
        key.write_alias(
            alias_parts
                .as_ref()
                .map(|(symbol, arguments)| (*symbol, &arguments[..])),
        );
        let key = key.finish();
        let active_index = self.instantiation.active[..self.instantiation.active_len]
            .iter()
            .rposition(|(id, _)| *id == mapper);
        let index = active_index.unwrap_or_else(|| self.instantiation.push_active_mapper(mapper));
        if let Some(&cached) = self.instantiation.active[index].1.get(&key) {
            return Ok(cached);
        }
        self.instantiation.depth += 1;
        self.instantiation.count += 1;
        self.instantiation.total_count += 1;
        let result = stacker::maybe_grow(128 * 1024, 2 * 1024 * 1024, || {
            #[cfg(feature = "recursion-probe")]
            if let Some(probe) = &mut self.instantiation.probe {
                probe.maximum_depth = probe.maximum_depth.max(self.instantiation.depth);
                probe.maximum_remaining_stack = probe
                    .maximum_remaining_stack
                    .max(stacker::remaining_stack().expect("instantiation contract stack bounds"));
            }
            self.instantiate_type_worker(ty, mapper, alias)
        });
        self.instantiation.depth -= 1;
        if active_index.is_none() {
            self.instantiation.pop_active_mapper();
        } else if let Ok(result) = result {
            self.instantiation.active[index].1.insert(key, result);
        }
        result
    }

    // port: tsc/internal/checker/checker.go:Checker.clearActiveMapperCaches
    pub(crate) fn clear_active_mapper_caches(&mut self) {
        for (_, cache) in &mut self.instantiation.active[..self.instantiation.active_len] {
            cache.clear();
        }
    }

    pub(crate) fn instantiate_types(
        &mut self,
        types: &TypeList,
        mapper: Option<MapperId>,
    ) -> Result<TypeList, Error> {
        // instantiateList in the pin calls the mapper in source order, but
        // allocates only once an element actually changes. Most lists survive
        // instantiation unchanged and can keep their shared backing.
        for (index, &ty) in types.iter().enumerate() {
            let mapped = self.instantiate_type(ty, mapper)?;
            if mapped != ty {
                let mut result = Vec::with_capacity(types.len());
                result.extend_from_slice(&types[..index]);
                result.push(mapped);
                for &rest in &types[index + 1..] {
                    result.push(self.instantiate_type(rest, mapper)?);
                }
                return Ok(result.into());
            }
        }
        Ok(types.clone())
    }

    // port: tsc/internal/checker/checker.go:Checker.instantiateTypeAlias
    pub(crate) fn instantiate_type_alias(
        &mut self,
        ty: TypeId,
        mapper: MapperId,
    ) -> Result<Option<AliasId>, Error> {
        let Some(alias) = self.types.alias_of(ty)?.cloned() else {
            return Ok(None);
        };
        let arguments = self.instantiate_types(&alias.type_arguments, Some(mapper))?;
        if arguments == alias.type_arguments {
            return Ok(self.types.get(ty)?.alias);
        }
        self.types
            .push_alias(crate::TypeAlias {
                symbol: alias.symbol,
                type_arguments: arguments,
            })
            .map(Some)
    }

    // port: tsc/internal/checker/checker.go:Checker.instantiateTypeWorker
    fn instantiate_type_worker(
        &mut self,
        ty: TypeId,
        mapper: MapperId,
        alias: Option<AliasId>,
    ) -> Result<TypeId, Error> {
        let record = *self.types.get(ty)?;
        if record.flags & tf::TYPE_PARAMETER != 0 {
            return self.map_type_parameter(ty, mapper);
        }
        if record.flags & tf::OBJECT != 0 {
            if record.object_flags & of::REFERENCE != 0
                && self.types.type_reference(ty)?.node.is_none()
            {
                let arguments = self.get_type_arguments(ty)?;
                let instantiated = self.instantiate_types(&arguments, Some(mapper))?;
                return if instantiated == arguments {
                    Ok(ty)
                } else {
                    self.create_normalized_type_reference(self.types.target(ty)?, &instantiated)
                };
            }
            if record.object_flags & (of::REFERENCE | of::ANONYMOUS | of::MAPPED) != 0 {
                if record.object_flags & of::REVERSE_MAPPED != 0 {
                    return self.instantiate_reverse_mapped(ty, mapper);
                }
                return self.get_object_type_instantiation(ty, mapper, alias);
            }
            return Ok(ty);
        }
        if record.flags & tf::UNION_OR_INTERSECTION != 0 {
            let source = if record.flags & tf::UNION != 0 {
                match self.types.union(ty)?.origin {
                    Some(origin) if self.types.flags(origin)? & tf::UNION_OR_INTERSECTION != 0 => {
                        origin
                    }
                    _ => ty,
                }
            } else {
                ty
            };
            let types = self.types.compound_types(source)?.clone();
            let instantiated = self.instantiate_types(&types, Some(mapper))?;
            let new_symbol = self.alias_key_parts(alias)?.map(|(symbol, _)| symbol);
            let old_symbol = self
                .alias_key_parts(record.alias)?
                .map(|(symbol, _)| symbol);
            if instantiated == types && new_symbol == old_symbol {
                return Ok(ty);
            }
            let alias = match alias {
                Some(alias) => Some(alias),
                None => self.instantiate_type_alias(ty, mapper)?,
            };
            return if self.types.flags(source)? & tf::INTERSECTION != 0 {
                self.get_intersection_type_ex(&instantiated, 0, alias)
            } else {
                self.get_union_type_ex(&instantiated, crate::UnionReduction::Literal, alias, None)
            };
        }
        if record.flags & tf::TEMPLATE_LITERAL != 0 {
            let template = self.types.template_literal(ty)?;
            let texts = template.texts.clone();
            let types = template.types.clone();
            let types = self.instantiate_types(&types, Some(mapper))?;
            return self.get_template_literal_type(&texts, &types);
        }

        if record.flags & tf::STRING_MAPPING != 0 {
            let target = self.instantiate_type(self.types.target(ty)?, Some(mapper))?;
            return self.get_string_mapping_type(
                record
                    .symbol
                    .ok_or(Error::MissingLink("string mapping symbol"))?,
                target,
            );
        }
        if record.flags & tf::INDEX != 0 {
            let data = *self.types.index_type(ty)?;
            let target = self.instantiate_type(data.target, Some(mapper))?;
            return self.get_index_type(target, data.index_flags);
        }
        if record.flags & tf::INDEXED_ACCESS != 0 {
            let data = *self.types.indexed_access(ty)?;
            let object = self.instantiate_type(data.object_type, Some(mapper))?;
            let index = self.instantiate_type(data.index_type, Some(mapper))?;
            let alias = match alias {
                Some(alias) => Some(alias),
                None => self.instantiate_type_alias(ty, mapper)?,
            };
            return self.get_indexed_access_type(object, index, data.access_flags, None, alias);
        }
        if record.flags & tf::SUBSTITUTION != 0 {
            return self.instantiate_substitution(ty, mapper);
        }
        if record.flags & tf::CONDITIONAL != 0 {
            let mapper = self.combine_type_mappers(self.types.conditional(ty)?.mapper, mapper)?;
            return self.conditional_instantiation(ty, mapper, false, alias);
        }
        Ok(ty)
    }

    // port: tsc/internal/checker/checker.go:Checker.instantiateSymbol
    pub(crate) fn instantiate_symbol(
        &mut self,
        symbol: SymbolId,
        mapper: MapperId,
    ) -> Result<SymbolId, Error> {
        // Go's valueSymbolLinks.Get assigns this lazy comparison identity even
        // when the symbol can be returned without instantiation.
        let links = self
            .value_symbol_links
            .try_get(self.value_symbol_key(symbol)?)
            .copied()
            .unwrap_or_default();
        if self.mapper_maps_this_only(mapper)? && self.is_thisless_symbol(symbol)? {
            return Ok(symbol);
        }
        if let Some(ty) = links.resolved_type {
            if !self.could_contain_type_variables(ty)? {
                if self.symbol(symbol)?.flags() & sf::SET_ACCESSOR == 0 {
                    return Ok(symbol);
                }
                // If we're a setter, check writeType.
                if let Some(write) = links.write_type {
                    if !self.could_contain_type_variables(write)? {
                        return Ok(symbol);
                    }
                }
            }
        }
        let (symbol, mapper) = if self.symbol(symbol)?.check_flags() & cf::INSTANTIATED != 0 {
            (
                links
                    .target
                    .ok_or(Error::MissingLink("instantiated symbol target"))?,
                self.combine_type_mappers(links.mapper, mapper)?,
            )
        } else {
            (symbol, mapper)
        };
        let read = self.symbol(symbol)?;
        let flags = read.flags();
        let check_flags = cf::INSTANTIATED
            | read.check_flags()
                & (cf::READONLY | cf::LATE | cf::OPTIONAL_PARAMETER | cf::REST_PARAMETER);
        let name = read.name_to_owned();
        let parent = read.parent();
        let value_declaration = read.value_declaration();
        let declarations = read.declarations();
        let result = self.new_symbol_ex(flags, name, check_flags)?;
        let write = self.symbol_mut(result)?;
        write.parent = parent;
        write.value_declaration = value_declaration;
        write.declarations = declarations;
        let result_links = self
            .value_symbol_links
            .get_or_default(self.value_symbol_key(result)?);
        result_links.target = Some(symbol);
        result_links.mapper = Some(mapper);
        result_links.name_type = links.name_type;
        Ok(result)
    }

    fn is_thisless_symbol(&self, symbol: SymbolId) -> Result<bool, Error> {
        let declarations = self.symbol_declarations(symbol)?;
        if declarations.len() != 1 {
            return Ok(false);
        }
        let Some(declaration) = declarations.first().flatten() else {
            return Ok(false);
        };
        let read = self.node(declaration)?;
        match read.kind().known() {
            Some(K::Parameter | K::PropertyDeclaration | K::PropertySignature) => {
                self.is_thisless_variable(declaration)
            }
            Some(
                K::MethodDeclaration
                | K::MethodSignature
                | K::Constructor
                | K::GetAccessor
                | K::SetAccessor,
            ) => {
                if read.kind() != K::Constructor {
                    let Some(annotation) = read.type_node() else {
                        return Ok(false);
                    };
                    if !self.is_thisless_type_node(annotation)? {
                        return Ok(false);
                    }
                }
                for parameter in self.source_list(declaration, read.parameter_list())? {
                    if !self.is_thisless_variable(parameter)? {
                        return Ok(false);
                    }
                }
                for parameter in self.source_list(declaration, read.type_parameter_list())? {
                    let read = self.node(parameter)?;
                    let constraint = read
                        .data_source()
                        .as_type_parameter_declaration()
                        .ok_or(Error::MissingLink("type parameter declaration"))?
                        .constraint();
                    if let Some(constraint) = constraint {
                        if !self.is_thisless_type_node(constraint)? {
                            return Ok(false);
                        }
                    }
                }
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    // port: tsc/internal/checker/checker.go:isThislessVariableLikeDeclaration
    fn is_thisless_variable(&self, node: NodeId) -> Result<bool, Error> {
        let read = self.node(node)?;
        match read.type_node() {
            Some(annotation) => self.is_thisless_type_node(annotation),
            None => Ok(read.initializer().is_none()),
        }
    }

    // port: tsc/internal/checker/checker.go:isThislessType
    fn is_thisless_type_node(&self, node: NodeId) -> Result<bool, Error> {
        let read = self.node(node)?;
        match read.kind().known() {
            Some(
                K::AnyKeyword
                | K::UnknownKeyword
                | K::StringKeyword
                | K::NumberKeyword
                | K::BigIntKeyword
                | K::BooleanKeyword
                | K::SymbolKeyword
                | K::ObjectKeyword
                | K::VoidKeyword
                | K::UndefinedKeyword
                | K::NeverKeyword
                | K::LiteralType,
            ) => Ok(true),
            Some(K::ArrayType) => {
                let element = read
                    .data_source()
                    .as_array_type_node()
                    .and_then(|data| data.element_type())
                    .ok_or(Error::MissingLink("array element"))?;
                self.is_thisless_type_node(element)
            }
            Some(K::TypeReference) => {
                for argument in self.source_list(node, read.type_argument_list())? {
                    if !self.is_thisless_type_node(argument)? {
                        return Ok(false);
                    }
                }
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    // port: tsc/internal/checker/checker.go:Checker.getObjectTypeInstantiation
    pub(crate) fn get_object_type_instantiation(
        &mut self,
        ty: TypeId,
        mapper: MapperId,
        alias: Option<AliasId>,
    ) -> Result<TypeId, Error> {
        let record = *self.types.get(ty)?;
        let deferred = record.object_flags & of::REFERENCE != 0;
        let instantiation_expression = record.object_flags & of::INSTANTIATION_EXPRESSION_TYPE != 0;
        let declaration = if deferred {
            self.types
                .type_reference(ty)?
                .node
                .ok_or(Error::MissingLink("deferred reference node"))?
        } else if instantiation_expression {
            self.types
                .instantiation_expression(ty)?
                .node
                .ok_or(Error::MissingLink("instantiation expression node"))?
        } else {
            let symbol = record
                .symbol
                .ok_or(Error::MissingLink("instantiated object symbol"))?;
            self.symbol_declarations(symbol)?
                .first()
                .flatten()
                .ok_or(Error::MissingLink("instantiated object declaration"))?
        };
        let target = if deferred {
            self.query
                .type_nodes
                .try_get(declaration)
                .copied()
                .flatten()
                .ok_or(Error::MissingLink("deferred source type"))?
        } else if record.object_flags & of::INSTANTIATED != 0 {
            self.types
                .object(ty)?
                .target
                .ok_or(Error::MissingLink("instantiated object target"))?
        } else {
            ty
        };
        let parameters = if let Some(Some(parameters)) =
            self.query.outer_type_parameters.try_get(declaration)
        {
            parameters.clone()
        } else {
            let outer = self.get_outer_type_parameters(declaration, true)?;
            let mut parameters = Vec::new();
            let has_alias_arguments = self
                .types
                .alias_of(target)?
                .is_some_and(|alias| !alias.type_arguments.is_empty());
            for &parameter in outer.iter() {
                let mut referenced = has_alias_arguments;
                if !referenced {
                    if deferred || instantiation_expression {
                        referenced =
                            self.type_parameter_possibly_referenced(parameter, declaration)?;
                    } else if let Some(symbol) = record.symbol {
                        if self.symbol(symbol)?.flags() & (sf::METHOD | sf::TYPE_LITERAL) != 0 {
                            for declaration in self
                                .symbol_declarations(symbol)?
                                .to_vec()
                                .into_iter()
                                .flatten()
                            {
                                if self
                                    .type_parameter_possibly_referenced(parameter, declaration)?
                                {
                                    referenced = true;
                                    break;
                                }
                            }
                        } else {
                            referenced = true;
                        }
                    }
                }
                if referenced {
                    parameters.push(parameter);
                }
            }
            let parameters: TypeList = parameters.into();
            *self.query.outer_type_parameters.get_or_default(declaration) =
                Some(parameters.clone());
            parameters
        };
        if parameters.is_empty() {
            return Ok(ty);
        }
        let existing_mapper = self.types.object(ty)?.mapper;
        let mut arguments = Vec::with_capacity(parameters.len());
        for &parameter in parameters.iter() {
            let first = match existing_mapper {
                Some(mapper) => self.map_type_parameter(parameter, mapper)?,
                None => parameter,
            };
            arguments.push(if first == parameter {
                self.map_type_parameter(parameter, mapper)?
            } else {
                self.instantiate_type(first, Some(mapper))?
            });
        }
        let alias = match alias {
            Some(alias) => Some(alias),
            None => self.instantiate_type_alias(ty, mapper)?,
        };
        let key = self.object_instantiation_key(
            &arguments,
            alias,
            record.object_flags & of::SINGLE_SIGNATURE_TYPE != 0,
        )?;
        if self.types.object(target)?.instantiations.is_none() {
            let initial_key =
                self.object_instantiation_key(&parameters, self.types.get(target)?.alias, false)?;
            let mut cache = crate::types::Map::default();
            cache.insert(initial_key, target);
            self.types.object_mut(target)?.instantiations = Some(Box::new(cache));
        }
        if let Some(&cached) = self
            .types
            .object(target)?
            .instantiations
            .as_ref()
            .and_then(|cache| cache.get(&key))
        {
            return Ok(cached);
        }
        let mut new_mapper = self.new_type_mapper(&parameters, &arguments)?;
        if self.types.object_flags(target)? & of::SINGLE_SIGNATURE_TYPE != 0 {
            new_mapper = self.combine_type_mappers(Some(new_mapper), mapper)?;
        }
        let result = if self.types.object_flags(target)? & of::REFERENCE != 0 {
            self.create_deferred_type_reference(
                self.types.target(ty)?,
                declaration,
                Some(new_mapper),
                alias,
            )?
        } else if self.types.object_flags(target)? & of::MAPPED != 0 {
            self.instantiate_mapped_type(target, new_mapper, alias)?
        } else {
            let target_record = *self.types.get(target)?;
            let result = self.new_object_type(
                target_record.object_flags
                    & !(of::COULD_CONTAIN_TYPE_VARIABLES_COMPUTED
                        | of::COULD_CONTAIN_TYPE_VARIABLES)
                    | of::INSTANTIATED,
                target_record.symbol,
            )?;
            if target_record.object_flags & of::INSTANTIATION_EXPRESSION_TYPE != 0 {
                self.types.instantiation_expression_mut(result)?.node =
                    self.types.instantiation_expression(target)?.node;
            }
            self.types.get_mut(result)?.alias = alias;
            if let Some(alias) = self.types.alias_of(result)?.cloned() {
                let propagating = self.get_propagating_flags_of_types(&alias.type_arguments, 0)?;
                self.types.get_mut(result)?.object_flags |= propagating;
            }
            let object = self.types.object_mut(result)?;
            object.target = Some(target);
            object.mapper = Some(new_mapper);
            result
        };
        self.types
            .object_mut(target)?
            .instantiations
            .as_mut()
            .expect("cache initialized above")
            .insert(key, result);
        let mut contains = false;
        for &argument in &arguments {
            if self.could_contain_type_variables(argument)? {
                contains = true;
                break;
            }
        }
        if self.types.object_flags(result)? & of::COULD_CONTAIN_TYPE_VARIABLES_COMPUTED == 0 {
            self.types.get_mut(result)?.object_flags |= of::COULD_CONTAIN_TYPE_VARIABLES_COMPUTED
                | if contains {
                    of::COULD_CONTAIN_TYPE_VARIABLES
                } else {
                    0
                };
        }
        Ok(result)
    }

    pub(crate) fn object_instantiation_key(
        &self,
        arguments: &[TypeId],
        alias: Option<AliasId>,
        single_signature: bool,
    ) -> Result<CacheKey, Error> {
        let mut key = crate::key::KeyBuilder::new();
        key.write_types(arguments);
        let parts = self.alias_key_parts(alias)?;
        key.write_alias(
            parts
                .as_ref()
                .map(|(symbol, arguments)| (*symbol, &arguments[..])),
        );
        if single_signature {
            key.write_byte(b'!');
        }
        Ok(key.finish())
    }

    // port: tsc/internal/checker/checker.go:Checker.isTypeParameterPossiblyReferenced
    pub(crate) fn type_parameter_possibly_referenced(
        &mut self,
        parameter: TypeId,
        node: tsr_arena::NodeId,
    ) -> Result<bool, Error> {
        let Some(symbol) = self.types.get(parameter)?.symbol else {
            return Ok(true);
        };
        let declarations = self.symbol_declarations(symbol)?;
        if declarations.len() != 1 {
            return Ok(true);
        }
        let declaration = declarations
            .first()
            .flatten()
            .ok_or(Error::MissingLink("type parameter declaration"))?;
        let container = self.node(declaration)?.parent();
        let mut ancestor = Some(node);
        while ancestor != container {
            let Some(current) = ancestor else {
                return Ok(true);
            };
            let read = self.node(current)?;
            if read.kind() == K::Block {
                return Ok(true);
            }
            if let Some(conditional) = read.data_source().as_conditional_type_node() {
                if let Some(extends) = conditional.extends_type() {
                    if self.contains_type_parameter_reference(parameter, extends)? {
                        return Ok(true);
                    }
                }
            }
            ancestor = self.node(current)?.parent();
        }
        self.contains_type_parameter_reference(parameter, node)
    }

    fn contains_type_parameter_reference(
        &mut self,
        parameter: TypeId,
        node: tsr_arena::NodeId,
    ) -> Result<bool, Error> {
        let is_this = self.types.type_parameter(parameter)?.is_this_type;
        let read = self.node(node)?;
        match read.kind().known() {
            Some(K::ThisType) => return Ok(is_this),
            Some(K::TypeReference) if !is_this && read.type_argument_list().is_none() => {
                let symbol = self.type_reference_symbol(node, false)?;
                if symbol == self.types.get(parameter)?.symbol {
                    return Ok(true);
                }
            }
            Some(K::TypeQuery) => {
                let view = self.ast(node)?;
                let entity_name = read
                    .data_source()
                    .as_type_query_node()
                    .and_then(|data| data.expr_name())
                    .ok_or(Error::MissingLink("type query name"))?;
                let first = tsr_ast::utilities_middle::get_first_identifier(view, entity_name)?;
                let type_arguments = read.type_argument_list();
                let is_this = view.node(first)?.kind() == K::Identifier
                    && view.node_text(first)?.as_bytes() == b"this";
                if !is_this {
                    let first_symbol = self.resolved_value_symbol(first)?;
                    let symbol = self
                        .types
                        .get(parameter)?
                        .symbol
                        .ok_or(Error::MissingLink("type parameter symbol"))?;
                    // There is exactly one declaration, otherwise `containsReference` is not called
                    let declaration = self
                        .symbol_declarations(symbol)?
                        .first()
                        .flatten()
                        .ok_or(Error::MissingLink("type parameter declaration"))?;
                    let declaration_read = self.node(declaration)?;
                    let scope = if declaration_read.kind() == K::TypeParameter {
                        // Type parameter is a regular type parameter, e.g. foo<T>
                        declaration_read.parent()
                    } else if is_this {
                        Some(declaration)
                    } else if self.types.type_parameter(parameter)?.is_this_type {
                        // Type parameter is the this type, and its declaration is the class declaration.
                        Some(declaration)
                    } else {
                        None
                    };
                    if let Some(scope) = scope {
                        for candidate in self
                            .symbol_declarations(first_symbol)?
                            .to_vec()
                            .into_iter()
                            .flatten()
                        {
                            if tsr_ast::utilities::is_node_descendant_of(
                                self.ast(candidate)?,
                                Some(candidate),
                                Some(scope),
                            )? {
                                return Ok(true);
                            }
                        }
                        for argument in self.source_list(node, type_arguments)? {
                            if self.contains_type_parameter_reference(parameter, argument)? {
                                return Ok(true);
                            }
                        }
                        return Ok(false);
                    }
                }
                return Ok(true);
            }
            Some(K::MethodDeclaration | K::MethodSignature)
                if read.type_node().is_none() && read.body().is_some() =>
            {
                return Ok(true)
            }
            _ => {}
        }
        for child in self.source_children(node)? {
            if self.contains_type_parameter_reference(parameter, child)? {
                return Ok(true);
            }
        }
        Ok(false)
    }
}

#[cfg(any(test, feature = "storage-pilot"))]
impl InstantiationState {
    pub(crate) fn census(&self, census: &mut crate::census::Census) {
        census.vec_capacity("mappers", &self.mappers, self.mappers.capacity());
        for mapper in &self.mappers {
            match mapper {
                crate::mapper::Mapper::DeferredArguments { sources, .. }
                | crate::mapper::Mapper::ArrayToSingle { sources, .. } => {
                    census.list("type_lists", sources);
                }
                crate::mapper::Mapper::Array { sources, targets } => {
                    census.list("type_lists", sources);
                    census.list("type_lists", targets);
                }
                _ => {}
            }
        }
        census.vec_capacity("mappers", &self.active, self.active.capacity());
        for (_, map) in &self.active {
            census.key_map("type_caches", map);
        }
    }
}

#[cfg(any(test, feature = "storage-pilot"))]
impl InstantiationState {
    pub(crate) fn census_active_mappers(&self) -> impl Iterator<Item = MapperId> + '_ {
        self.active[..self.active_len].iter().map(|(id, _)| *id)
    }
    pub(crate) fn census_active_types(&self) -> impl Iterator<Item = TypeId> + '_ {
        self.active[..self.active_len]
            .iter()
            .flat_map(|(_, cache)| cache.values().copied())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn active_mapper_hits_and_clearing_preserve_instantiation_counters() {
        let (_counters, _generation, _identity, owner) = crate::tests::owner();
        let mut operation = owner.operation().unwrap();
        let state = operation.state_mut();
        let parameter = state.new_type_parameter(None).unwrap();
        let number = state.builtins.number_type;
        let mapper = state
            .alloc_mapper(crate::mapper::Mapper::Simple {
                source: parameter,
                target: number,
            })
            .unwrap();

        state.instantiation.push_active_mapper(mapper);
        for _ in 0..2 {
            assert_eq!(
                state.instantiate_type(parameter, Some(mapper)).unwrap(),
                number
            );
            assert_eq!(state.instantiation.active_len, 1);
            assert_eq!(state.instantiation.depth, 0);
            assert_eq!(state.instantiation.count, 1);
            assert_eq!(state.instantiation.total_count, 1);
        }

        state.clear_active_mapper_caches();
        assert_eq!(
            state.instantiate_type(parameter, Some(mapper)).unwrap(),
            number
        );
        assert_eq!(state.instantiation.count, 2);
        assert_eq!(state.instantiation.total_count, 2);
        state.instantiation.pop_active_mapper();

        // The same mapper's inactive slot must neither supply a stale answer
        // nor look active and retain a new answer after either invocation.
        for expected_count in 3..=4 {
            assert_eq!(
                state.instantiate_type(parameter, Some(mapper)).unwrap(),
                number
            );
            assert_eq!(state.instantiation.active_len, 0);
            assert_eq!(state.instantiation.depth, 0);
            assert_eq!(state.instantiation.count, expected_count);
            assert_eq!(state.instantiation.total_count, expected_count);
            assert!(state.instantiation.active[0].1.is_empty());
        }
    }

    #[test]
    fn inactive_mapper_slots_reuse_capacity_without_reusing_results() {
        let (_counters, _generation, _identity, owner) = crate::tests::owner();
        let mut operation = owner.operation().unwrap();
        let state = operation.state_mut();
        let a = state
            .alloc_mapper(crate::mapper::Mapper::Permissive)
            .unwrap();
        let b = state
            .alloc_mapper(crate::mapper::Mapper::Restrictive)
            .unwrap();
        let key = || {
            let mut builder = crate::key::KeyBuilder::new();
            builder.write_type(state.builtins.number_type);
            builder.finish()
        };
        let key = key();
        let instantiation = &mut state.instantiation;
        assert_eq!(instantiation.push_active_mapper(a), 0);
        instantiation.active[0]
            .1
            .insert(key.clone(), state.builtins.number_type);
        assert_eq!(instantiation.push_active_mapper(b), 1);
        instantiation.active[1]
            .1
            .insert(key.clone(), state.builtins.string_type);
        let capacity = instantiation.active[1].1.capacity();
        instantiation.pop_active_mapper();
        assert_eq!(instantiation.active[1].1.capacity(), capacity);
        assert_eq!(
            instantiation.census_active_mappers().collect::<Vec<_>>(),
            [a]
        );
        assert_eq!(
            instantiation.census_active_types().collect::<Vec<_>>(),
            [state.builtins.number_type]
        );
        assert_eq!(instantiation.push_active_mapper(a), 1);
        assert!(instantiation.active[1].1.is_empty());
        assert_eq!(
            instantiation.active[0].1.get(&key),
            Some(&state.builtins.number_type)
        );
        instantiation.pop_active_mapper();
        instantiation.pop_active_mapper();
        assert_eq!(instantiation.active_len, 0);
        assert!(instantiation.census_active_mappers().next().is_none());
        assert!(instantiation.census_active_types().next().is_none());
    }

    #[test]
    fn failed_instantiation_clears_the_active_slot_before_reuse() {
        let (_counters, _generation, _identity, owner) = crate::tests::owner();
        let mut operation = owner.operation().unwrap();
        let state = operation.state_mut();
        let parameter = state.new_type_parameter(None).unwrap();
        let invalid_mapper = MapperId::next(0, 100).unwrap();
        assert!(state
            .instantiate_type(parameter, Some(invalid_mapper))
            .is_err());
        assert_eq!(state.instantiation.active_len, 0);
        assert_eq!(state.instantiation.depth, 0);
        assert!(state
            .instantiation
            .active
            .iter()
            .all(|(_, cache)| cache.is_empty()));
        let mapper = state
            .alloc_mapper(crate::mapper::Mapper::Simple {
                source: parameter,
                target: state.builtins.number_type,
            })
            .unwrap();
        assert_eq!(
            state.instantiate_type(parameter, Some(mapper)).unwrap(),
            state.builtins.number_type
        );
        assert_eq!(state.instantiation.active_len, 0);
    }

    #[test]
    fn instantiate_lists_keep_unchanged_backing_and_preserve_changed_order() {
        let (_counters, _generation, _identity, owner) = crate::tests::owner();
        let mut operation = owner.operation().unwrap();
        let state = operation.state_mut();
        let parameter = state.new_type_parameter(None).unwrap();
        let number = state.builtins.number_type;
        let string = state.builtins.string_type;
        let mapper = state
            .alloc_mapper(crate::mapper::Mapper::Simple {
                source: parameter,
                target: string,
            })
            .unwrap();
        let unchanged: TypeList = vec![number, string].into();
        let result = state.instantiate_types(&unchanged, Some(mapper)).unwrap();
        assert!(std::sync::Arc::ptr_eq(&unchanged, &result));
        let changed: TypeList = vec![number, parameter, number, parameter].into();
        let result = state.instantiate_types(&changed, Some(mapper)).unwrap();
        assert_eq!(&*result, &[number, string, number, string]);
        assert_eq!(&*changed, &[number, parameter, number, parameter]);
        assert_eq!(state.instantiation.total_count, 2);
    }
}
