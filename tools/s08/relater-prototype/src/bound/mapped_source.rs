//! Reference-owned mapped members and indexed access constructors.

use super::{
    instantiate, missing, of, tf, Construction, Environment, Error, HashMap, LiteralValue, Member,
    NodeId, Rc, RefCell, Structure, TypeCell, TypeLink, Weak, K,
};

#[derive(Default)]
pub(super) struct State {
    mapped: RefCell<HashMap<u32, Mapped>>,
    indexes: RefCell<HashMap<u32, Weak<TypeCell>>>,
    indexed: RefCell<HashMap<(u32, u32), Weak<TypeCell>>>,
}

#[derive(Clone)]
struct Mapped {
    node: NodeId,
    environment: Environment,
    constraint: TypeLink,
}

impl Construction {
    // port: tsc/internal/checker/checker.go:Checker.getTypeFromMappedTypeNode
    pub(super) fn mapped(
        self: &Rc<Self>,
        node: NodeId,
        env: &Environment,
        alias: Option<Rc<str>>,
    ) -> Result<Rc<TypeCell>, Error> {
        let alias = self.alias_metadata(node, env, alias)?;
        self.mapped_with_alias(node, env, alias)
    }

    // Constructor-style entry: callers hand over handles they have just built.
    #[allow(clippy::needless_pass_by_value)]
    pub(super) fn mapped_with_alias(
        self: &Rc<Self>,
        node: NodeId,
        env: &Environment,
        alias: Option<instantiate::Alias>,
    ) -> Result<Rc<TypeCell>, Error> {
        let identity = alias.as_ref().map(|alias| self.identity(alias.declaration));
        let name = alias
            .as_ref()
            .map_or_else(|| Rc::from("__mapped"), |alias| alias.name.clone());
        let weak = Rc::downgrade(self);
        let resolver = Box::new(move |_: &crate::Graph, cell: &TypeCell| {
            weak.upgrade().ok_or(Error::Released)?.mapped_members(cell)
        });
        let result = self.checker.graph.allocate_full(
            tf::OBJECT,
            of::MAPPED | if env.1.is_some() { of::INSTANTIATED } else { 0 },
            name,
            Some(self.identity(node)),
            identity,
            None,
            false,
            Vec::new(),
            false,
            Some(resolver),
        );
        let read = self.input.node(node)?;
        let data = read.data_source();
        let parameter_node = data
            .as_mapped_type_node()
            .and_then(|data| data.type_parameter())
            .ok_or(Error::ResolutionFailed)?;
        let original = self.type_parameter(parameter_node)?;
        let mut environment = env.clone();
        let parameter = if env.1.is_some() {
            // instantiateAnonymousType clones the iteration parameter. Its
            // constraint is substituted only when members demand it.
            let parameter = self.checker.graph.allocate_full(
                tf::TYPE_PARAMETER,
                0,
                original.name.clone(),
                None,
                None,
                None,
                false,
                Vec::new(),
                false,
                None,
            );
            environment
                .0
                .push((parameter_node, Rc::downgrade(&parameter)));
            environment.1 = Some(instantiate::Mapper::new(environment.clone()));
            let weak = Rc::downgrade(self);
            let source = Rc::downgrade(&original);
            let mapper = environment.1.clone().ok_or(Error::ResolutionFailed)?;
            let constraint = TypeLink::lazy(move || {
                let state = weak.upgrade().ok_or(Error::Released)?;
                let source = source.upgrade().ok_or(Error::Released)?;
                let constraint = source
                    .type_parameter_shape()
                    .ok_or(Error::ResolutionFailed)?
                    .constraint()?
                    .ok_or(Error::ResolutionFailed)?;
                state.instantiate(&constraint, &mapper, None)
            });
            parameter.set_lazy_type_parameter(Some(constraint), Some(&original), 0)?;
            self.record_type_parameter(&parameter, parameter_node);
            parameter
        } else {
            original
        };
        let parameter = Rc::downgrade(&parameter);
        let constraint = TypeLink::lazy(move || {
            parameter
                .upgrade()
                .ok_or(Error::Released)?
                .type_parameter_shape()
                .ok_or(Error::ResolutionFailed)?
                .constraint()?
                .ok_or(Error::ResolutionFailed)
        });
        self.mapped_state.mapped.borrow_mut().insert(
            result.id(),
            Mapped {
                node,
                environment,
                constraint: constraint.clone(),
            },
        );
        if env.1.is_none() {
            constraint.resolve()?;
        }
        Ok(result)
    }

    // port: tsc/internal/checker/checker.go:Checker.instantiateMappedType
    pub(super) fn instantiate_mapped(
        self: &Rc<Self>,
        ty: &Rc<TypeCell>,
        mapper: &instantiate::Mapper,
        alias: Option<instantiate::Alias>,
    ) -> Result<Rc<TypeCell>, Error> {
        let info = self
            .mapped_state
            .mapped
            .borrow()
            .get(&ty.id())
            .cloned()
            .ok_or(Error::ResolutionFailed)?;
        let constraint = info.constraint.resolve()?;
        if constraint.flags & tf::INDEX != 0 {
            let variable = constraint
                .types()?
                .into_iter()
                .next()
                .ok_or(Error::ResolutionFailed)?;
            if variable.flags & tf::TYPE_PARAMETER != 0 {
                let mapped = self.instantiate(&variable, mapper, None)?;
                if !Rc::ptr_eq(&variable, &mapped) {
                    if mapped.flags
                        & (tf::ANY_OR_UNKNOWN
                            | (tf::INSTANTIABLE & !tf::INSTANTIABLE_PRIMITIVE)
                            | tf::OBJECT
                            | tf::INTERSECTION
                            | tf::UNION)
                        == 0
                    {
                        return Ok(mapped);
                    }
                    if mapped.flags & (tf::UNION | tf::INTERSECTION) != 0
                        || mapped.is_array_or_tuple()
                    {
                        return missing("homomorphic mapped union/array/tuple instantiation");
                    }
                    // instantiateConstituent: the mapped type is instantiated
                    // with the variable's mapping prepended, a distinct mapper
                    // whose alias arguments are instantiated afresh.
                    let source_alias = self.alias_metadata(info.node, &info.environment, None)?;
                    let prepended = instantiate::Mapper::prepend(&variable, &mapped, mapper)?;
                    return self.instantiate_source_type(ty, &prepended, None, source_alias);
                }
            }
        }
        let instantiated_constraint = self.instantiate(&constraint, mapper, None)?;
        if self
            .initialization
            .named
            .borrow()
            .get("wildcardType")
            .is_some_and(|wildcard| Rc::ptr_eq(wildcard, &instantiated_constraint))
        {
            return Ok(instantiated_constraint);
        }
        let source_alias = self.alias_metadata(info.node, &info.environment, None)?;
        self.instantiate_source_type(ty, mapper, alias, source_alias)
    }

    // port: tsc/internal/checker/checker.go:Checker.resolveMappedTypeMembers
    fn mapped_members(self: &Rc<Self>, cell: &TypeCell) -> Result<Structure, Error> {
        let info = self
            .mapped_state
            .mapped
            .borrow()
            .get(&cell.id())
            .cloned()
            .ok_or(Error::ResolutionFailed)?;
        let node = info.node;
        let env = &info.environment;
        let read = self.input.node(node)?;
        let data = read.data_source();
        let data = data.as_mapped_type_node().ok_or(Error::ResolutionFailed)?;
        let parameter = data.type_parameter().ok_or(Error::ResolutionFailed)?;
        let parameter_read = self.input.node(parameter)?;
        let parameter_data = parameter_read.data_source();
        let constraint_node = parameter_data
            .as_type_parameter_declaration()
            .and_then(|data| data.constraint())
            .ok_or(Error::ResolutionFailed)?;
        let constraint_type = info.constraint.resolve()?;
        let template_node = data.r#type().ok_or(Error::ResolutionFailed)?;
        // These are canonical types. The template is constructed now, while
        // each property's substitution remains inside its lazy type link.
        let name_type = data
            .name_type()
            .map(|node| self.type_node(node, &Environment::default(), None))
            .transpose()?;
        let template = self.type_node(template_node, &Environment::default(), None)?;
        // getTemplateTypeFromMappedType: a `?` (or `+?`) modifier makes the
        // template itself optional, once, so every property instantiates the
        // union `X | undefined` rather than gaining `undefined` afterwards.
        let includes_optional = match data.question_token() {
            Some(token) => self.input.node(token)?.kind() != K::MinusToken,
            None => false,
        };
        let template = if includes_optional && self.input.options().strict_null_checks {
            let undefined = self.builtin(tf::UNDEFINED)?;
            let already = Rc::ptr_eq(&template, &undefined)
                || template.flags & tf::UNION != 0
                    && template
                        .types()?
                        .first()
                        .is_some_and(|first| Rc::ptr_eq(first, &undefined));
            if already {
                template
            } else {
                self.checker.graph.union(&[template, undefined])?
            }
        } else {
            template
        };
        // port: tsc/internal/checker/checker.go:Checker.getModifiersTypeFromMappedType
        let keyof_operand = self
            .input
            .node(constraint_node)?
            .data_source()
            .as_type_operator_node()
            .filter(|operator| operator.operator() == K::KeyOfKeyword)
            .map(|operator| operator.r#type().ok_or(Error::ResolutionFailed))
            .transpose()?;
        let modifier_type = if let Some(operand) = keyof_operand {
            // A `keyof T` constraint declaration: the modifiers type is T.
            Some(self.type_node(operand, env, None)?)
        } else {
            // Otherwise the declared constraint, through a type parameter's own
            // constraint: `P in K` with `K extends keyof T` also has modifiers
            // type T. Anything else has none (`unknown` upstream).
            let declared = self
                .type_parameter(parameter)?
                .type_parameter_shape()
                .ok_or(Error::ResolutionFailed)?
                .constraint()?;
            let extended = match declared {
                Some(constraint) if constraint.flags & tf::TYPE_PARAMETER != 0 => constraint
                    .type_parameter_shape()
                    .map(crate::TypeParameterShape::constraint)
                    .transpose()?
                    .flatten(),
                other => other,
            };
            match extended {
                Some(index) if index.flags & tf::INDEX != 0 => {
                    let target = index
                        .types()?
                        .into_iter()
                        .next()
                        .ok_or(Error::ResolutionFailed)?;
                    Some(match &env.1 {
                        Some(mapper) => self.instantiate(&target, mapper, None)?,
                        None => target,
                    })
                }
                _ => None,
            }
        };
        // Only a `keyof T` constraint declaration takes its keys from the
        // modifiers type; otherwise they come from the constraint itself.
        let keys =
            if let Some(modifiers) = modifier_type.as_ref().filter(|_| keyof_operand.is_some()) {
                let structure = modifiers.structure(&self.checker.graph)?;
                let mut keys = structure
                    .members
                    .iter()
                    .map(|member| self.mapped_property_key(member))
                    .collect::<Result<Vec<_>, _>>()?;
                for index in &structure.index_infos {
                    keys.push(index.key()?);
                }
                keys
            } else if constraint_type.flags & tf::UNION != 0 {
                constraint_type.types()?
            } else {
                vec![constraint_type]
            };
        let mut structure = Structure::default();
        for key in keys {
            if key.flags & tf::NEVER != 0 {
                continue;
            }
            // getTypeOfMappedSymbol: `appendTypeMapping(mappedType.mapper,
            // typeParameter, key)`. An instantiated mapped type's mapper is the
            // composite of its cloned iteration parameter and the outer mapper,
            // so the clone is instantiated through the outer mapper on the way.
            let mapper = if env.1.is_some() {
                let original = self.type_parameter(parameter)?;
                let fresh = env.get(parameter)?.ok_or(Error::ResolutionFailed)?;
                let mut outer = env.clone();
                outer.0.retain(|(node, _)| *node != parameter);
                let clone_mapper = instantiate::Mapper::with_types(
                    Environment::default(),
                    std::slice::from_ref(&original),
                    std::slice::from_ref(&fresh),
                )?;
                let mapped_mapper =
                    instantiate::Mapper::compose(&clone_mapper, &instantiate::Mapper::new(outer));
                instantiate::Mapper::append(&mapped_mapper, &fresh, &key)?
            } else {
                let mut environment = env.clone();
                environment.0.push((parameter, Rc::downgrade(&key)));
                instantiate::Mapper::new(environment)
            };
            let names = if let Some(name_type) = &name_type {
                self.instantiate(name_type, &mapper, None)?
            } else {
                key.clone()
            };
            let names = if names.flags & tf::UNION != 0 {
                names.types()?
            } else {
                vec![names]
            };
            for name_type in names {
                let name = match name_type.literal.as_ref() {
                    Some(LiteralValue::String(bytes)) => String::from_utf8(bytes.clone())
                        .map_err(|_| Error::Unsupported("non-UTF8 mapped property name".into()))?,
                    Some(LiteralValue::Number(bits)) => {
                        ts_jsnum::Number::new(f64::from_bits(*bits)).to_string()
                    }
                    _ => return missing("mapped nonliteral key/index signature"),
                };
                if structure
                    .members
                    .iter()
                    .any(|member| member.name.as_ref() == name)
                {
                    return missing("mapped remapping merges multiple property keys");
                }
                let source_property = if let Some(modifiers) = &modifier_type {
                    modifiers
                        .member(&self.checker.graph, &name)?
                        .map(|member| (member.optional, member.readonly))
                } else {
                    None
                };
                let (mut optional, mut readonly) = source_property.unwrap_or_default();
                if let Some(token) = data.question_token() {
                    optional = self.input.node(token)?.kind() != K::MinusToken;
                }
                if let Some(token) = data.readonly_token() {
                    readonly = self.input.node(token)?.kind() != K::MinusToken;
                }
                let strip_optional =
                    !optional && source_property.is_some_and(|property| property.0);
                let weak = Rc::downgrade(self);
                let template = Rc::downgrade(&template);
                let mapper = mapper.clone();
                let r#type = TypeLink::lazy(move || {
                    let state = weak.upgrade().ok_or(Error::Released)?;
                    let mut ty = state.instantiate(
                        &template.upgrade().ok_or(Error::Released)?,
                        &mapper,
                        None,
                    )?;
                    if state.input.options().strict_null_checks {
                        // An optional property gains `undefined` only when its
                        // type cannot already be undefined or void.
                        let maybe_undefined = if ty.flags & tf::UNION != 0 {
                            ty.types()?
                                .iter()
                                .any(|part| part.flags & (tf::UNDEFINED | tf::VOID) != 0)
                        } else {
                            ty.flags & (tf::UNDEFINED | tf::VOID) != 0
                        };
                        if optional && !maybe_undefined {
                            ty = state
                                .checker
                                .graph
                                .union(&[ty, state.builtin(tf::UNDEFINED)?])?;
                        } else if strip_optional {
                            if ty.flags & tf::UNION != 0 {
                                let types = ty
                                    .types()?
                                    .into_iter()
                                    .filter(|ty| ty.flags & tf::UNDEFINED == 0)
                                    .collect::<Vec<_>>();
                                ty = state.checker.graph.union(&types)?;
                            } else if ty.flags & tf::UNDEFINED != 0 {
                                ty = state.builtin(tf::NEVER)?;
                            }
                        }
                    }
                    Ok(ty)
                });
                structure.members.push(Member {
                    name_type: None,
                    name: name.into(),
                    optional,
                    readonly,
                    class_member: true,
                    r#type,
                });
            }
        }
        Ok(structure)
    }

    fn mapped_property_key(&self, member: &Member) -> Result<Rc<TypeCell>, Error> {
        if let Some(key) = &member.name_type {
            key.resolve()
        } else {
            Ok(self.checker.graph.string_literal(member.name.as_bytes()))
        }
    }

    // Constructor-style entry: callers hand over handles they have just built.
    #[allow(clippy::needless_pass_by_value)]
    // port: tsc/internal/checker/checker.go:Checker.getIndexType
    pub(super) fn keyof(
        self: &Rc<Self>,
        _: NodeId,
        operand: Rc<TypeCell>,
    ) -> Result<Rc<TypeCell>, Error> {
        if operand.flags & (tf::TYPE_PARAMETER | tf::INDEXED_ACCESS | tf::CONDITIONAL) != 0 {
            if let Some(cached) = self.mapped_state.indexes.borrow().get(&operand.id()) {
                return cached.upgrade().ok_or(Error::Released);
            }
            let result = self.checker.graph.allocate_full(
                tf::INDEX,
                0,
                "keyof".into(),
                None,
                None,
                None,
                false,
                vec![Rc::downgrade(&operand)],
                false,
                None,
            );
            self.mapped_state
                .indexes
                .borrow_mut()
                .insert(operand.id(), Rc::downgrade(&result));
            return Ok(result);
        }
        // getIndexTypeEx: `keyof unknown` is never, and `keyof any` and
        // `keyof never` are the shared `string | number | symbol`.
        if operand.flags & tf::UNKNOWN != 0 {
            return self.builtin(tf::NEVER);
        }
        if operand.flags & (tf::ANY | tf::NEVER) != 0 {
            return self.initialization.named("stringNumberSymbolType");
        }
        if operand.flags & tf::OBJECT == 0 {
            return missing("keyof nonobject");
        }
        let structure = operand.structure(&self.checker.graph)?;
        let mut keys = structure
            .members
            .iter()
            .map(|member| self.mapped_property_key(member))
            .collect::<Result<Vec<_>, _>>()?;
        for index in &structure.index_infos {
            keys.push(index.key()?);
        }
        self.checker.graph.union(&keys)
    }

    // Constructor-style entry: callers hand over handles they have just built.
    #[allow(clippy::needless_pass_by_value)]
    // port: tsc/internal/checker/checker.go:Checker.getIndexedAccessType
    pub(super) fn indexed_access(
        self: &Rc<Self>,
        object: Rc<TypeCell>,
        index: Rc<TypeCell>,
    ) -> Result<Rc<TypeCell>, Error> {
        self.indexed_access_with_alias(object, index, None)
    }

    /// `alias` names the union a distributed access builds, as
    /// `getUnionTypeEx(propTypes, UnionReductionLiteral, alias, nil)` does.
    #[allow(clippy::needless_pass_by_value)]
    pub(super) fn indexed_access_with_alias(
        self: &Rc<Self>,
        object: Rc<TypeCell>,
        index: Rc<TypeCell>,
        alias: Option<instantiate::Alias>,
    ) -> Result<Rc<TypeCell>, Error> {
        if object.flags & (tf::TYPE_PARAMETER | tf::INDEXED_ACCESS | tf::CONDITIONAL) != 0
            || index.flags & (tf::TYPE_PARAMETER | tf::INDEX) != 0
        {
            let key = (object.id(), index.id());
            if let Some(cached) = self.mapped_state.indexed.borrow().get(&key) {
                return cached.upgrade().ok_or(Error::Released);
            }
            let result = self.checker.graph.allocate_full(
                tf::INDEXED_ACCESS,
                0,
                "indexed access".into(),
                None,
                None,
                None,
                false,
                vec![Rc::downgrade(&object), Rc::downgrade(&index)],
                false,
                None,
            );
            self.mapped_state
                .indexed
                .borrow_mut()
                .insert(key, Rc::downgrade(&result));
            return Ok(result);
        }
        // A generic (variadic) tuple defers every access that is not a fixed
        // element index. isStringIndexSignatureOnlyType has resolved the
        // object's members by then, so they are resolved here first.
        if let Some(tuple) = object.tuple_shape() {
            if tuple
                .element_flags
                .iter()
                .any(|flag| flag & crate::element_flags::VARIADIC != 0)
            {
                object.structure(&self.checker.graph)?;
                let fixed_index = match index.literal.as_ref() {
                    Some(LiteralValue::Number(bits)) => {
                        let fixed = tuple
                            .element_flags
                            .iter()
                            .take_while(|flag| *flag & crate::element_flags::VARIABLE == 0)
                            .count();
                        let value = f64::from_bits(*bits);
                        value >= 0.0 && value < fixed as f64
                    }
                    _ => false,
                };
                if !fixed_index {
                    let key = (object.id(), index.id());
                    if let Some(cached) = self.mapped_state.indexed.borrow().get(&key) {
                        return cached.upgrade().ok_or(Error::Released);
                    }
                    let result = self.checker.graph.allocate_full(
                        tf::INDEXED_ACCESS,
                        0,
                        "indexed access".into(),
                        None,
                        None,
                        None,
                        false,
                        vec![Rc::downgrade(&object), Rc::downgrade(&index)],
                        false,
                        None,
                    );
                    self.mapped_state
                        .indexed
                        .borrow_mut()
                        .insert(key, Rc::downgrade(&result));
                    return Ok(result);
                }
            }
        }
        if index.flags & tf::UNION != 0 && index.flags & tf::BOOLEAN == 0 {
            let results = index
                .types()?
                .into_iter()
                .map(|index| self.indexed_access(object.clone(), index))
                .collect::<Result<Vec<_>, _>>()?;
            return match alias {
                Some(alias) => {
                    let arguments = alias
                        .arguments
                        .iter()
                        .map(|ty| ty.upgrade().map(|ty| ty.id()).ok_or(Error::Released))
                        .collect::<Result<Vec<_>, _>>()?;
                    self.checker.graph.union_named_arguments(
                        &results,
                        self.identity(alias.declaration),
                        &alias.name,
                        &arguments,
                    )
                }
                None => self.checker.graph.union(&results),
            };
        }
        let name = match index.literal.as_ref() {
            Some(LiteralValue::String(bytes)) => String::from_utf8(bytes.clone())
                .map_err(|_| Error::Unsupported("non-UTF8 indexed property".into()))?,
            Some(LiteralValue::Number(bits)) => {
                ts_jsnum::Number::new(f64::from_bits(*bits)).to_string()
            }
            _ => {
                if let Some(array) = object.array_element() {
                    if index.flags & tf::NUMBER != 0 {
                        return array.element();
                    }
                }
                return missing("indexed access key");
            }
        };
        if let Some(property) = object.member(&self.checker.graph, &name)? {
            return property.r#type();
        }
        missing(&format!("indexed property {name}"))
    }
}
