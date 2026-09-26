//! Conditional roots retain declaration identity and share their instantiation
//! cache. Branches are resolved lazily; tail recursion runs in this loop rather
//! than consuming the Rust stack or the ordinary instantiation depth budget.
use crate::{
    type_flags as tf, AliasId, CacheKey, CheckerState, ConditionalRootId, Error, MapperId,
    RelationKind, TypeId, TypeList,
};
use tsr_arena::NodeId;
use tsr_ast::{symbol_flags as sf, SyntaxKind as K};

#[derive(Clone)]
pub(crate) struct ConditionalRoot {
    pub node: NodeId,
    pub check_type: TypeId,
    pub extends_type: TypeId,
    pub distributive: bool,
    pub infer_parameters: TypeList,
    pub outer_parameters: TypeList,
    pub alias: Option<AliasId>,
}
#[derive(Default)]
pub(crate) struct ConditionalState {
    pub roots: Vec<ConditionalRoot>,
    pub instantiations: crate::types::Map<(ConditionalRootId, CacheKey), TypeId>,
    pub permissive: crate::types::Map<TypeId, TypeId>,
    pub restrictive: crate::types::Map<TypeId, TypeId>,
    pub restrictive_parameters: crate::types::Map<TypeId, TypeId>,
    pub permissive_mapper: Option<MapperId>,
    pub restrictive_mapper: Option<MapperId>,
}
impl CheckerState {
    pub(crate) fn conditional_root(
        &self,
        id: ConditionalRootId,
    ) -> Result<&ConditionalRoot, Error> {
        id.index(0)
            .and_then(|i| self.conditional.roots.get(i))
            .ok_or(Error::Arena(tsr_arena::Error::InvalidSlot))
    }
    // port: tsc/internal/checker/checker.go:Checker.getInferTypeParameters
    pub(crate) fn infer_type_parameters(&mut self, node: NodeId) -> Result<TypeList, Error> {
        let locals = self
            .program()?
            .bound(node)?
            .node_binding(node)?
            .and_then(|b| b.locals);
        let mut result = Vec::new();
        if let Some(locals) = locals {
            let symbols = self
                .table(locals)?
                .into_iter()
                .filter_map(|(_, s)| s)
                .collect::<Vec<_>>();
            for symbol in symbols {
                if self.symbol(symbol)?.flags() & sf::TYPE_PARAMETER != 0 {
                    result.push(self.get_declared_type_of_symbol(symbol)?);
                }
            }
        }
        Ok(result.into())
    }
    // port: tsc/internal/checker/checker.go:Checker.getTypeFromConditionalTypeNode
    pub(crate) fn source_conditional_type(&mut self, node: NodeId) -> Result<TypeId, Error> {
        let (check, extends, _, _) = self.conditional_nodes(node)?;
        let check_type = self.get_type_from_type_node(check)?;
        let alias = self
            .alias_for_type_node(node)?
            .map(|alias| self.types.push_alias(alias))
            .transpose()?;
        let all_outer = self.get_outer_type_parameters(node, true)?;
        let mut outer = Vec::new();
        let has_alias_arguments = alias
            .map(|alias| {
                self.types
                    .alias(alias)
                    .map(|a| !a.type_arguments.is_empty())
            })
            .transpose()?
            .unwrap_or(false);
        for &parameter in all_outer.iter() {
            if has_alias_arguments || self.type_parameter_possibly_referenced(parameter, node)? {
                outer.push(parameter);
            }
        }
        let root = ConditionalRoot {
            node,
            check_type,
            extends_type: self.get_type_from_type_node(extends)?,
            distributive: self.types.flags(check_type)? & tf::TYPE_PARAMETER != 0,
            infer_parameters: self.infer_type_parameters(node)?,
            outer_parameters: outer.into(),
            alias,
        };
        let id = ConditionalRootId::next(0, self.conditional.roots.len())?;
        self.conditional.roots.push(root);
        let result = self.get_conditional_type(id, None, false, None)?;
        let parameters = &self.conditional_root(id)?.outer_parameters;
        if !parameters.is_empty() {
            let key = self.conditional_key(parameters, None, false)?;
            self.conditional.instantiations.insert((id, key), result);
        }
        Ok(result)
    }
    fn conditional_nodes(&self, node: NodeId) -> Result<(NodeId, NodeId, NodeId, NodeId), Error> {
        let read = self.node(node)?;
        let data = read
            .data_source()
            .as_conditional_type_node()
            .ok_or(Error::MissingLink("conditional syntax"))?;
        Ok((
            data.check_type()
                .ok_or(Error::MissingLink("conditional check"))?,
            data.extends_type()
                .ok_or(Error::MissingLink("conditional extends"))?,
            data.true_type()
                .ok_or(Error::MissingLink("conditional true"))?,
            data.false_type()
                .ok_or(Error::MissingLink("conditional false"))?,
        ))
    }
    // port: tsc/internal/checker/checker.go:Checker.getTypeFromInferTypeNode
    pub(crate) fn source_infer_type(&mut self, node: NodeId) -> Result<TypeId, Error> {
        let parameter = self
            .ast(node)?
            .node(node)?
            .data_source()
            .as_infer_type_node()
            .and_then(|d| d.type_parameter())
            .ok_or(Error::MissingLink("infer parameter"))?;
        let symbol = self
            .get_symbol_of_declaration(parameter)?
            .ok_or(Error::MissingLink("infer parameter symbol"))?;
        self.get_declared_type_of_type_parameter(symbol)
    }
    // port: tsc/internal/checker/checker.go:Checker.getConditionalType
    pub(crate) fn get_conditional_type(
        &mut self,
        mut root_id: ConditionalRootId,
        mut mapper: Option<MapperId>,
        for_constraint: bool,
        mut alias: Option<AliasId>,
    ) -> Result<TypeId, Error> {
        let mut extra = Vec::new();
        let mut tails = 0;
        let result = loop {
            if tails == 1000 {
                self.error_at(
                    self.current_node,
                    tsr_diagnostics::Type_instantiation_is_excessively_deep_and_possibly_infinite,
                    vec![],
                )?;
                return Ok(self.builtins.error_type);
            }
            let root = self.conditional_root(root_id)?.clone();
            let check_variable = self.actual_type_variable(root.check_type)?;
            let check = self.instantiate_type(check_variable, mapper)?;
            let extends = self.instantiate_type(root.extends_type, mapper)?;
            if check == self.builtins.error_type || extends == self.builtins.error_type {
                return Ok(self.builtins.error_type);
            }
            if check == self.builtins.wildcard_type || extends == self.builtins.wildcard_type {
                return Ok(self.builtins.wildcard_type);
            }
            let (check_node, extends_node, true_node, false_node) =
                self.conditional_nodes(root.node)?;
            let tuples = match (
                self.simple_tuple_arity(check_node)?,
                self.simple_tuple_arity(extends_node)?,
            ) {
                (Some(a), Some(b)) => a == b,
                _ => false,
            };
            let deferred = self.deferred_conditional_type(check, tuples)?;
            let combined = if root.infer_parameters.is_empty() {
                None
            } else {
                let context = self.new_inference_context(&root.infer_parameters, None, 0)?;
                if let Some(mapper) = mapper {
                    let first = self.inference_context(context)?.non_fixing_mapper;
                    let combined = self.combine_type_mappers(Some(first), mapper)?;
                    self.inference_context_mut(context)?.non_fixing_mapper = combined;
                }
                if !deferred {
                    self.infer_types(
                        context,
                        check,
                        extends,
                        crate::inference::priority::NO_CONSTRAINTS
                            | crate::inference::priority::ALWAYS_STRICT,
                        false,
                    )?;
                }
                let first = self.inference_context(context)?.mapper;
                Some(match mapper {
                    Some(mapper) => self.combine_type_mappers(Some(first), mapper)?,
                    None => first,
                })
            };
            let inferred_extends = match combined {
                Some(mapper) => self.instantiate_type(root.extends_type, Some(mapper))?,
                None => extends,
            };
            if !deferred && !self.deferred_conditional_type(inferred_extends, tuples)? {
                let check_any = self.types.flags(check)? & tf::ANY != 0;
                let extends_top = self.types.flags(inferred_extends)? & tf::ANY_OR_UNKNOWN != 0;
                let definitely_false = if extends_top {
                    false
                } else if check_any {
                    true
                } else {
                    let source = self.permissive_instantiation(check)?;
                    let target = self.permissive_instantiation(inferred_extends)?;
                    !self.is_type_related_to(source, target, RelationKind::Assignable)?
                };
                if definitely_false {
                    let mut possible = check_any;
                    if !possible
                        && for_constraint
                        && self.types.flags(inferred_extends)? & tf::NEVER == 0
                    {
                        // someType: a non-union extends type is its own only
                        // constituent.
                        let source = self.permissive_instantiation(inferred_extends)?;
                        let target = self.permissive_instantiation(check)?;
                        for part in self.distributed_types(source)? {
                            if self.is_type_related_to(part, target, RelationKind::Assignable)? {
                                possible = true;
                                break;
                            }
                        }
                    }
                    if possible {
                        let t = self.get_type_from_type_node(true_node)?;
                        extra.push(self.instantiate_type(t, combined.or(mapper))?);
                    }
                    let false_type = self.get_type_from_type_node(false_node)?;
                    if self.types.flags(false_type)? & tf::CONDITIONAL != 0 {
                        let new_root = self.types.conditional(false_type)?.root;
                        let definition = self.conditional_root(new_root)?;
                        if self.node(definition.node)?.parent() == Some(root.node)
                            && (!definition.distributive
                                || definition.check_type == root.check_type)
                        {
                            root_id = new_root;
                            continue;
                        }
                    }
                    if let Some((new_root, new_mapper)) =
                        self.tail_recursion_root(false_type, mapper)?
                    {
                        root_id = new_root;
                        mapper = Some(new_mapper);
                        alias = None;
                        if self.conditional_root(root_id)?.alias.is_some() {
                            tails += 1;
                        }
                        continue;
                    }
                    break self.instantiate_type(false_type, mapper)?;
                }
                let definitely_true = if extends_top {
                    true
                } else {
                    let source = self.restrictive_instantiation(check)?;
                    let target = self.restrictive_instantiation(inferred_extends)?;
                    self.is_type_related_to(source, target, RelationKind::Assignable)?
                };
                if definitely_true {
                    let true_type = self.get_type_from_type_node(true_node)?;
                    let true_mapper = combined.or(mapper);
                    if let Some((new_root, new_mapper)) =
                        self.tail_recursion_root(true_type, true_mapper)?
                    {
                        root_id = new_root;
                        mapper = Some(new_mapper);
                        alias = None;
                        if self.conditional_root(root_id)?.alias.is_some() {
                            tails += 1;
                        }
                        continue;
                    }
                    break self.instantiate_type(true_type, true_mapper)?;
                }
            }
            let check_type = self.instantiate_type(root.check_type, mapper)?;
            let extends_type = self.instantiate_type(root.extends_type, mapper)?;
            let result = self.types.new_type(
                tf::CONDITIONAL,
                0,
                crate::types::Payload::Conditional(crate::types::ConditionalData {
                    root: root_id,
                    check_type,
                    extends_type,
                    mapper,
                    combined_mapper: combined,
                    resolved_base_constraint: None,
                    true_type: None,
                    false_type: None,
                    inferred_true_type: None,
                    default_constraint: None,
                    distributive_constraint: None,
                }),
            )?;
            let alias = if alias.is_some() {
                alias
            } else {
                self.instantiate_alias_record(root.alias, mapper)?
            };
            self.types.get_mut(result)?.alias = alias;
            break result;
        };
        if extra.is_empty() {
            Ok(result)
        } else {
            extra.push(result);
            self.get_union_type(&extra)
        }
    }
    fn instantiate_alias_record(
        &mut self,
        alias: Option<AliasId>,
        mapper: Option<MapperId>,
    ) -> Result<Option<AliasId>, Error> {
        let Some(alias) = alias else { return Ok(None) };
        let mut data = self.types.alias(alias)?.clone();
        data.type_arguments = self.instantiate_types(&data.type_arguments, mapper)?;
        self.types.push_alias(data).map(Some)
    }
    // port: tsc/internal/checker/checker.go:Checker.getConditionalTypeInstantiation
    pub(crate) fn conditional_instantiation(
        &mut self,
        ty: TypeId,
        mapper: MapperId,
        for_constraint: bool,
        alias: Option<AliasId>,
    ) -> Result<TypeId, Error> {
        let id = self.types.conditional(ty)?.root;
        let root = self.conditional_root(id)?.clone();
        if root.outer_parameters.is_empty() {
            return Ok(ty);
        }
        let mut arguments = Vec::new();
        for &parameter in root.outer_parameters.iter() {
            arguments.push(self.map_type_parameter(parameter, mapper)?);
        }
        let key = self.conditional_key(&arguments, alias, for_constraint)?;
        if let Some(&result) = self.conditional.instantiations.get(&(id, key.clone())) {
            return Ok(result);
        }
        let mapper = self.new_type_mapper(&root.outer_parameters, &arguments)?;
        let distribution = if root.distributive {
            let t = self.map_type_parameter(root.check_type, mapper)?;
            Some(self.get_reduced_type(t)?)
        } else {
            None
        };
        let result = if let Some(distribution) = distribution.filter(|t| *t != root.check_type) {
            if self.types.flags(distribution)? & (tf::UNION | tf::NEVER) != 0 {
                let mut mapped = Vec::new();
                if self.types.flags(distribution)? & tf::NEVER == 0 {
                    for part in self.types.types_of(distribution)?.to_vec() {
                        let mapper =
                            self.prepend_type_mapping(root.check_type, part, Some(mapper))?;
                        mapped.push(self.get_conditional_type(
                            id,
                            Some(mapper),
                            for_constraint,
                            None,
                        )?);
                    }
                }
                self.get_union_type_ex(&mapped, crate::UnionReduction::Literal, alias, None)?
            } else {
                self.get_conditional_type(id, Some(mapper), for_constraint, alias)?
            }
        } else {
            self.get_conditional_type(id, Some(mapper), for_constraint, alias)?
        };
        self.conditional.instantiations.insert((id, key), result);
        Ok(result)
    }
    fn conditional_key(
        &self,
        arguments: &[TypeId],
        alias: Option<AliasId>,
        for_constraint: bool,
    ) -> Result<CacheKey, Error> {
        let mut key = crate::key::KeyBuilder::new();
        key.write_types(arguments);
        let alias = self.alias_key_parts(alias)?;
        key.write_alias(alias.as_ref().map(|(symbol, args)| (*symbol, &args[..])));
        if for_constraint {
            key.write_byte(b'!');
        }
        Ok(key.finish())
    }
    // port: tsc/internal/checker/checker.go:Checker.getTailRecursionRoot
    fn tail_recursion_root(
        &mut self,
        ty: TypeId,
        mapper: Option<MapperId>,
    ) -> Result<Option<(ConditionalRootId, MapperId)>, Error> {
        let Some(mapper) = mapper else {
            return Ok(None);
        };
        if self.types.flags(ty)? & tf::CONDITIONAL == 0 {
            return Ok(None);
        }
        let data = *self.types.conditional(ty)?;
        let root = self.conditional_root(data.root)?.clone();
        if root.outer_parameters.is_empty() {
            return Ok(None);
        }
        let mapper = self.combine_type_mappers(data.mapper, mapper)?;
        let mut arguments = Vec::new();
        for &parameter in root.outer_parameters.iter() {
            arguments.push(self.map_type_parameter(parameter, mapper)?);
        }
        let mapper = self.new_type_mapper(&root.outer_parameters, &arguments)?;
        if root.distributive {
            let check = self.map_type_parameter(root.check_type, mapper)?;
            if check != root.check_type && self.types.flags(check)? & (tf::UNION | tf::NEVER) != 0 {
                return Ok(None);
            }
        }
        Ok(Some((data.root, mapper)))
    }
    // port: tsc/internal/checker/relater.go:Checker.isDistributionDependent
    pub(crate) fn distribution_dependent(&mut self, id: ConditionalRootId) -> Result<bool, Error> {
        let root = self.conditional_root(id)?.clone();
        if !root.distributive {
            return Ok(false);
        }
        let (_, _, yes, no) = self.conditional_nodes(root.node)?;
        Ok(
            self.type_parameter_possibly_referenced(root.check_type, yes)?
                || self.type_parameter_possibly_referenced(root.check_type, no)?,
        )
    }
    fn simple_tuple_arity(&self, mut node: NodeId) -> Result<Option<usize>, Error> {
        while self.node(node)?.kind() == K::ParenthesizedType {
            node = self
                .ast(node)?
                .node(node)?
                .type_node()
                .ok_or(Error::MissingLink("type parentheses"))?;
        }
        let read = self.node(node)?;
        if read.kind() != K::TupleType {
            return Ok(None);
        }
        let elements = self.source_list(node, read.element_list())?;
        if elements.is_empty() {
            return Ok(None);
        }
        for &element in &elements {
            if self.tuple_element_info(element)?.flags != crate::element_flags::REQUIRED {
                return Ok(None);
            }
        }
        Ok(Some(elements.len()))
    }
    fn deferred_conditional_type(&mut self, ty: TypeId, tuples: bool) -> Result<bool, Error> {
        if self.is_generic_type(ty)? {
            return Ok(true);
        }
        if tuples && self.is_tuple_type(ty)? {
            for &element in self.element_types(ty)?.iter() {
                if self.is_generic_type(element)? {
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }
    pub(crate) fn permissive_instantiation(&mut self, ty: TypeId) -> Result<TypeId, Error> {
        self.conditional_marker_instantiation(ty, false)
    }
    pub(crate) fn restrictive_instantiation(&mut self, ty: TypeId) -> Result<TypeId, Error> {
        self.conditional_marker_instantiation(ty, true)
    }
    // port: tsc/internal/checker/checker.go:Checker.getPermissiveInstantiation
    // port: tsc/internal/checker/checker.go:Checker.getRestrictiveInstantiation
    fn conditional_marker_instantiation(
        &mut self,
        ty: TypeId,
        restrictive: bool,
    ) -> Result<TypeId, Error> {
        if self.types.flags(ty)? & (tf::PRIMITIVE | tf::ANY_OR_UNKNOWN | tf::NEVER) != 0 {
            return Ok(ty);
        }
        let cache = if restrictive {
            &self.conditional.restrictive
        } else {
            &self.conditional.permissive
        };
        if let Some(&cached) = cache.get(&ty) {
            return Ok(cached);
        }
        let slot = if restrictive {
            self.conditional.restrictive_mapper
        } else {
            self.conditional.permissive_mapper
        };
        let mapper = if let Some(mapper) = slot {
            mapper
        } else {
            let mapper = self.alloc_mapper(if restrictive {
                crate::mapper::Mapper::Restrictive
            } else {
                crate::mapper::Mapper::Permissive
            })?;
            if restrictive {
                self.conditional.restrictive_mapper = Some(mapper);
            } else {
                self.conditional.permissive_mapper = Some(mapper);
            }
            mapper
        };
        let result = self.instantiate_type(ty, Some(mapper))?;
        if restrictive {
            self.conditional.restrictive.insert(ty, result);
            self.conditional.restrictive.insert(result, result);
        } else {
            self.conditional.permissive.insert(ty, result);
        }
        Ok(result)
    }
    // port: tsc/internal/checker/checker.go:Checker.getRestrictiveTypeParameter
    pub(crate) fn restrictive_type_parameter(&mut self, ty: TypeId) -> Result<TypeId, Error> {
        let constraint = self.types.type_parameter(ty)?.constraint;
        if constraint == Some(self.builtins.no_constraint_type)
            || constraint.is_none() && self.constraint_declaration(ty)?.is_none()
        {
            return Ok(ty);
        }
        if let Some(&result) = self.conditional.restrictive_parameters.get(&ty) {
            return Ok(result);
        }
        let result = self.new_type_parameter(self.types.get(ty)?.symbol)?;
        self.types.type_parameter_mut(result)?.constraint = Some(self.builtins.no_constraint_type);
        self.conditional.restrictive_parameters.insert(ty, result);
        Ok(result)
    }
    pub(crate) fn conditional_true_type(
        &mut self,
        ty: TypeId,
        inferred: bool,
    ) -> Result<TypeId, Error> {
        let data = *self.types.conditional(ty)?;
        if let Some(result) = if inferred {
            data.inferred_true_type
        } else {
            data.true_type
        } {
            return Ok(result);
        }
        let node = self
            .conditional_nodes(self.conditional_root(data.root)?.node)?
            .2;
        let source = self.get_type_from_type_node(node)?;
        let result = if inferred && data.combined_mapper.is_none() {
            self.conditional_true_type(ty, false)?
        } else {
            self.instantiate_type(
                source,
                if inferred {
                    data.combined_mapper
                } else {
                    data.mapper
                },
            )?
        };
        let data = self.types.conditional_mut(ty)?;
        if inferred {
            data.inferred_true_type = Some(result);
        } else {
            data.true_type = Some(result);
        }
        Ok(result)
    }
    pub(crate) fn conditional_false_type(&mut self, ty: TypeId) -> Result<TypeId, Error> {
        let data = *self.types.conditional(ty)?;
        if let Some(result) = data.false_type {
            return Ok(result);
        }
        let node = self
            .conditional_nodes(self.conditional_root(data.root)?.node)?
            .3;
        let source = self.get_type_from_type_node(node)?;
        let result = self.instantiate_type(source, data.mapper)?;
        self.types.conditional_mut(ty)?.false_type = Some(result);
        Ok(result)
    }
    // port: tsc/internal/checker/checker.go:Checker.getDefaultConstraintOfConditionalType
    pub(crate) fn default_conditional_constraint(&mut self, ty: TypeId) -> Result<TypeId, Error> {
        if let Some(result) = self.types.conditional(ty)?.default_constraint {
            return Ok(result);
        }
        let yes = self.conditional_true_type(ty, true)?;
        let no = self.conditional_false_type(ty)?;
        let result = if self.types.flags(yes)? & tf::ANY != 0 {
            no
        } else if self.types.flags(no)? & tf::ANY != 0 {
            yes
        } else {
            self.get_union_type(&[yes, no])?
        };
        self.types.conditional_mut(ty)?.default_constraint = Some(result);
        Ok(result)
    }
    // port: tsc/internal/checker/checker.go:Checker.getConstraintOfDistributiveConditionalType
    pub(crate) fn distributive_conditional_constraint(
        &mut self,
        ty: TypeId,
    ) -> Result<Option<TypeId>, Error> {
        let data = *self.types.conditional(ty)?;
        if let Some(result) = data.distributive_constraint {
            return Ok((result != self.builtins.no_constraint_type).then_some(result));
        }
        let root = self.conditional_root(data.root)?.clone();
        if root.distributive && self.conditional.restrictive.get(&ty) != Some(&ty) {
            let simplified = self.simplified_type(data.check_type, false)?;
            let constraint = if simplified == data.check_type {
                self.constraint_of_type(simplified)?
            } else {
                Some(simplified)
            };
            if let Some(constraint) = constraint.filter(|c| *c != data.check_type) {
                let mapper = self.prepend_type_mapping(root.check_type, constraint, data.mapper)?;
                let result = self.conditional_instantiation(ty, mapper, true, None)?;
                if self.types.flags(result)? & tf::NEVER == 0 {
                    self.types.conditional_mut(ty)?.distributive_constraint = Some(result);
                    return Ok(Some(result));
                }
            }
        }
        self.types.conditional_mut(ty)?.distributive_constraint =
            Some(self.builtins.no_constraint_type);
        Ok(None)
    }
    // port: tsc/internal/checker/checker.go:Checker.getConstraintFromConditionalType
    pub(crate) fn constraint_from_conditional(&mut self, ty: TypeId) -> Result<TypeId, Error> {
        match self.distributive_conditional_constraint(ty)? {
            Some(result) => Ok(result),
            None => self.default_conditional_constraint(ty),
        }
    }
    // port: tsc/internal/checker/checker.go:Checker.getSimplifiedConditionalType
    pub(crate) fn simplified_conditional_type(
        &mut self,
        ty: TypeId,
        writing: bool,
    ) -> Result<TypeId, Error> {
        let data = *self.types.conditional(ty)?;
        let yes = self.conditional_true_type(ty, false)?;
        let no = self.conditional_false_type(ty)?;
        let check = self.actual_type_variable(data.check_type)?;
        if self.types.flags(no)? & tf::NEVER != 0 && self.actual_type_variable(yes)? == check {
            let mut assignable = self.types.flags(data.check_type)? & tf::ANY != 0;
            if !assignable {
                let source = self.restrictive_instantiation(data.check_type)?;
                let target = self.restrictive_instantiation(data.extends_type)?;
                assignable = self.is_type_related_to(source, target, RelationKind::Assignable)?;
            }
            if assignable {
                return self.simplified_type(yes, writing);
            }
            if self.intersection_empty(data.check_type, data.extends_type)? {
                return Ok(self.builtins.never_type);
            }
        } else if self.types.flags(yes)? & tf::NEVER != 0 && self.actual_type_variable(no)? == check
        {
            let any = self.types.flags(data.check_type)? & tf::ANY != 0;
            if !any {
                let source = self.restrictive_instantiation(data.check_type)?;
                let target = self.restrictive_instantiation(data.extends_type)?;
                if self.is_type_related_to(source, target, RelationKind::Assignable)? {
                    return Ok(self.builtins.never_type);
                }
            }
            if any || self.intersection_empty(data.check_type, data.extends_type)? {
                return self.simplified_type(no, writing);
            }
        }
        Ok(ty)
    }
    // port: tsc/internal/checker/checker.go:Checker.isIntersectionEmpty
    fn intersection_empty(&mut self, source: TypeId, target: TypeId) -> Result<bool, Error> {
        let intersection = self.get_intersection_type(&[source, target])?;
        let union = self.get_union_type(&[intersection, self.builtins.never_type])?;
        Ok(self.types.flags(union)? & tf::NEVER != 0)
    }
}
