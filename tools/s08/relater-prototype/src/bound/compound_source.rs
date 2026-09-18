//! Array and tuple construction from source annotations and real library targets.

use super::instantiate::{Alias, Mapper};
use super::{
    missing, of, tf, Construction, Environment, Error, LiteralValue, Member, NodeId, Rc, Structure,
    TypeCell, TypeLink, K,
};
use crate::element_flags as ef;

#[derive(Clone, PartialEq, Eq, Hash)]
pub(super) struct TupleTargetKey {
    infos: Vec<(u8, Option<NodeId>)>,
    readonly: bool,
}

impl Construction {
    // Constructor-style entry: callers hand over handles they have just built.
    #[allow(clippy::needless_pass_by_value)]
    pub(super) fn array(
        self: &Rc<Self>,
        _location: NodeId,
        element: Rc<TypeCell>,
        readonly: bool,
    ) -> Result<Rc<TypeCell>, Error> {
        let name: &[u8] = if readonly { b"ReadonlyArray" } else { b"Array" };
        let group = self
            .input
            .resolve_global(name, ts_ast::symbol_flags::TYPE)?
            .ok_or_else(|| Error::Unsupported("global array declaration missing".into()))?;
        let target = self.declared(&group)?;
        let array = self.instantiate_interface(&group, &target, std::slice::from_ref(&element))?;
        if array.array_element().is_none() {
            array.set_array_element(&element, readonly)?;
        }
        Ok(array)
    }

    // port: tsc/internal/checker/checker.go:Checker.getArrayElementTypeNode
    fn array_element_node(&self, node: NodeId) -> Result<Option<NodeId>, Error> {
        let read = self.input.node(node)?;
        match read.kind().known() {
            Some(K::ParenthesizedType) => {
                self.array_element_node(read.type_node().ok_or(Error::ResolutionFailed)?)
            }
            Some(K::ArrayType) => Ok(read
                .data_source()
                .as_array_type_node()
                .and_then(|data| data.element_type())),
            Some(K::TupleType) => {
                let elements = self.list(
                    node,
                    read.data_source()
                        .as_tuple_type_node()
                        .and_then(|data| data.elements()),
                )?;
                if let [element] = elements.as_slice() {
                    let read = self.input.node(*element)?;
                    let rest = read.kind() == K::RestType
                        || read
                            .data_source()
                            .as_named_tuple_member()
                            .is_some_and(|data| data.dot_dot_dot_token().is_some());
                    if rest {
                        return self
                            .array_element_node(read.type_node().ok_or(Error::ResolutionFailed)?);
                    }
                }
                Ok(None)
            }
            _ => Ok(None),
        }
    }

    fn tuple_info(&self, element: NodeId) -> Result<(NodeId, u8, Option<NodeId>), Error> {
        let read = self.input.node(element)?;
        let (operand, optional, rest, label) =
            if let Some(data) = read.data_source().as_named_tuple_member() {
                (
                    data.r#type().ok_or(Error::ResolutionFailed)?,
                    data.question_token().is_some(),
                    data.dot_dot_dot_token().is_some(),
                    Some(element),
                )
            } else if read.kind() == K::OptionalType {
                (
                    read.type_node().ok_or(Error::ResolutionFailed)?,
                    true,
                    false,
                    None,
                )
            } else if read.kind() == K::RestType {
                (
                    read.type_node().ok_or(Error::ResolutionFailed)?,
                    false,
                    true,
                    None,
                )
            } else {
                (element, false, false, None)
            };
        // Go gives a question token precedence in a named member, even when
        // malformed source also carries a rest token.
        let flag = if optional {
            ef::OPTIONAL
        } else if rest {
            if self.array_element_node(operand)?.is_some() {
                ef::REST
            } else {
                ef::VARIADIC
            }
        } else {
            ef::REQUIRED
        };
        let operand = if rest {
            self.array_element_node(operand)?.unwrap_or(operand)
        } else {
            operand
        };
        Ok((operand, flag, label))
    }

    fn tuple_argument(
        self: &Rc<Self>,
        operand: NodeId,
        flag: u8,
        env: &Environment,
    ) -> Result<Rc<TypeCell>, Error> {
        let ty = self.type_node(operand, env, None)?;
        if flag == ef::OPTIONAL && self.input.options().strict_null_checks {
            self.checker
                .graph
                .union(&[ty, self.builtin(tf::UNDEFINED)?])
        } else {
            Ok(ty)
        }
    }

    // port: tsc/internal/checker/checker.go:Checker.getTypeFromArrayOrTupleTypeNode
    pub(super) fn tuple(
        self: &Rc<Self>,
        node: NodeId,
        env: &Environment,
        readonly: bool,
        name: Option<Rc<str>>,
    ) -> Result<Rc<TypeCell>, Error> {
        let alias = self.alias_metadata(node, env, name)?;
        self.tuple_with_alias(node, env, readonly, alias)
    }

    pub(super) fn tuple_with_alias(
        self: &Rc<Self>,
        node: NodeId,
        env: &Environment,
        readonly: bool,
        alias: Option<Alias>,
    ) -> Result<Rc<TypeCell>, Error> {
        let read = self.input.node(node)?;
        let list = read
            .data_source()
            .as_tuple_type_node()
            .and_then(|data| data.elements());
        let info = self
            .list(node, list)?
            .into_iter()
            .map(|element| self.tuple_info(element))
            .collect::<Result<Vec<_>, _>>()?;
        if let [(operand, ef::REST, _)] = info.as_slice() {
            return self.array(node, self.type_node(*operand, env, None)?, readonly);
        }
        let key = TupleTargetKey {
            infos: info
                .iter()
                .map(|(_, flag, label)| (*flag, *label))
                .collect(),
            readonly,
        };
        let target = self.tuple_target(node, key.clone())?;
        if info.is_empty() {
            return Ok(target);
        }
        let deferred = self.is_deferred_reference(node, false)?
            && info.iter().all(|(_, flag, _)| *flag != ef::VARIADIC);
        if deferred {
            let links = info
                .iter()
                .map(|(operand, flag, _)| {
                    let weak = Rc::downgrade(self);
                    let environment = env.clone();
                    let (operand, flag) = (*operand, *flag);
                    TypeLink::lazy(move || {
                        weak.upgrade().ok_or(Error::Released)?.tuple_argument(
                            operand,
                            flag,
                            &environment,
                        )
                    })
                })
                .collect::<Vec<_>>();
            return self.tuple_instance(node, &target, links, &key, alias, false);
        }
        let arguments = info
            .into_iter()
            .map(|(operand, flag, _)| self.tuple_argument(operand, flag, env))
            .collect::<Result<Vec<_>, _>>()?;
        self.normalize_tuple(node, &target, &key, &arguments, None)
    }

    // Constructor-style entry: callers hand over handles they have just built.
    #[allow(clippy::needless_pass_by_value)]
    // port: tsc/internal/checker/checker.go:Checker.createNormalizedTupleTypeEx
    fn normalize_tuple(
        self: &Rc<Self>,
        node: NodeId,
        target: &Rc<TypeCell>,
        key: &TupleTargetKey,
        arguments: &[Rc<TypeCell>],
        alias: Option<Rc<str>>,
    ) -> Result<Rc<TypeCell>, Error> {
        let readonly = key.readonly;
        if arguments.len() != key.infos.len() {
            return missing("tuple instantiation argument count");
        }
        for (index, (argument, (flag, _))) in arguments.iter().zip(&key.infos).enumerate() {
            if *flag == ef::VARIADIC && argument.flags & (tf::UNION | tf::NEVER) != 0 {
                if argument.flags & tf::NEVER != 0 {
                    return self.builtin(tf::NEVER);
                }
                let mut tuples = Vec::new();
                for constituent in argument.types()? {
                    let mut arguments = arguments.to_vec();
                    arguments[index] = constituent;
                    tuples.push(self.normalize_tuple(
                        node,
                        target,
                        key,
                        &arguments,
                        alias.clone(),
                    )?);
                }
                return self.checker.graph.union(&tuples);
            }
        }
        let mut elements = Vec::new();
        let mut normalized = Vec::new();
        for (ty, &(flag, label)) in arguments.iter().zip(&key.infos) {
            if flag == ef::VARIADIC {
                if let Some(tuple) = ty.tuple_shape() {
                    let spread = tuple.elements()?;
                    if spread.len() + elements.len() >= 10_000 {
                        return missing("tuple representation size limit");
                    }
                    elements.extend(spread);
                    let source_target = ty
                        .reference_shape()
                        .map(crate::ReferenceShape::target)
                        .transpose()?;
                    let source_key = source_target.and_then(|target| {
                        self.tuple_targets
                            .borrow()
                            .iter()
                            .find(|(_, cell)| Rc::ptr_eq(cell, &target))
                            .map(|(key, _)| key.clone())
                    });
                    if let Some(key) = source_key {
                        normalized.extend(key.infos);
                    } else {
                        normalized.extend(tuple.element_flags.iter().map(|flag| (*flag, None)));
                    }
                    continue;
                }
                if ty.array_element().is_some() {
                    // `getIndexTypeOfType(t, numberType)`: the rest element is
                    // the array's resolved numeric index type, which resolves
                    // the reference's members here, not at first use.
                    let structure = ty.structure(&self.checker.graph)?;
                    let mut element = None;
                    for info in &structure.index_infos {
                        if info.key()?.flags & tf::NUMBER != 0 {
                            element = Some(info.value()?);
                        }
                    }
                    elements.push(element.ok_or(Error::ResolutionFailed)?);
                    normalized.push((ef::REST, label));
                    continue;
                }
                if ty.flags & tf::ANY != 0 {
                    elements.push(ty.clone());
                    normalized.push((ef::REST, label));
                    continue;
                }
                if ty.flags & tf::INSTANTIABLE_NON_PRIMITIVE == 0 {
                    return missing(
                        "tuple variadic normalization requires array-like indexed access",
                    );
                }
            }
            let ty = if flag == ef::OPTIONAL && self.input.options().strict_null_checks {
                self.checker
                    .graph
                    .union(&[ty.clone(), self.builtin(tf::UNDEFINED)?])?
            } else {
                ty.clone()
            };
            elements.push(ty);
            normalized.push((flag, label));
        }
        // TupleNormalizer makes optional slots before a later required slot
        // required without removing undefined from their already-resolved type.
        if let Some(last_required) = normalized
            .iter()
            .rposition(|(flag, _)| *flag == ef::REQUIRED)
        {
            for (flag, _) in &mut normalized[..last_required] {
                if *flag == ef::OPTIONAL {
                    *flag = ef::REQUIRED;
                }
            }
        }
        let first_rest = normalized.iter().position(|(flag, _)| *flag == ef::REST);
        let last_optional_or_rest = normalized
            .iter()
            .rposition(|(flag, _)| *flag & (ef::OPTIONAL | ef::REST) != 0);
        if let (Some(first), Some(last)) = (first_rest, last_optional_or_rest) {
            if first < last {
                if normalized[first..=last]
                    .iter()
                    .any(|(flag, _)| *flag == ef::VARIADIC)
                {
                    return missing("tuple normalization indexed access of variadic slot");
                }
                elements[first] = self.checker.graph.union(&elements[first..=last])?;
                elements.drain(first + 1..=last);
                normalized.drain(first + 1..=last);
            }
        }
        if let [(ef::REST, _)] = normalized.as_slice() {
            return self.array(node, elements.remove(0), readonly);
        }
        let normalized_key = TupleTargetKey {
            infos: normalized,
            readonly,
        };
        let normalized_target = if &normalized_key == key {
            target.clone()
        } else {
            self.tuple_target(node, normalized_key.clone())?
        };
        if elements.is_empty() {
            return Ok(normalized_target);
        }
        let links = elements.iter().map(|ty| Rc::downgrade(ty).into()).collect();
        self.tuple_instance(node, &normalized_target, links, &normalized_key, None, true)
    }

    // port: tsc/internal/checker/checker.go:Checker.createTupleTargetType
    // port: tsc/internal/checker/checker.go:Checker.createTupleTypeEx
    pub(super) fn create_tuple_type(
        self: &Rc<Self>,
        location: NodeId,
        elements: &[Rc<TypeCell>],
        infos: Vec<(u8, Option<NodeId>)>,
        readonly: bool,
    ) -> Result<Rc<TypeCell>, Error> {
        let key = TupleTargetKey { infos, readonly };
        let target = self.tuple_target(location, key.clone())?;
        if elements.is_empty() {
            return Ok(target);
        }
        self.normalize_tuple(location, &target, &key, elements, None)
    }

    fn tuple_target(
        self: &Rc<Self>,
        location: NodeId,
        key: TupleTargetKey,
    ) -> Result<Rc<TypeCell>, Error> {
        if let Some(target) = self.tuple_targets.borrow().get(&key).cloned() {
            return Ok(target);
        }
        let flags = key.infos.iter().map(|(flag, _)| *flag).collect::<Vec<_>>();
        let parameters = flags
            .iter()
            .map(|_| self.checker.graph.type_parameter("", None))
            .collect::<Vec<_>>();
        // Length literals are declared members created with the target, before
        // the target's base and type arguments are ever requested.
        let length = self.tuple_length(&flags)?;
        let weak = Rc::downgrade(self);
        let length = Rc::downgrade(&length);
        let target = self.checker.graph.allocate_full(
            tf::OBJECT,
            of::REFERENCE | of::TUPLE,
            Rc::from("tuple"),
            None,
            None,
            None,
            false,
            Vec::new(),
            false,
            Some(Box::new(move |_, cell| {
                weak.upgrade().ok_or(Error::Released)?.tuple_members(
                    location,
                    cell,
                    length.upgrade().ok_or(Error::Released)?,
                )
            })),
        );
        target.set_tuple_shape(&parameters, &flags, key.readonly)?;
        target.set_reference_shape(&target, &parameters)?;
        let weak = Rc::downgrade(self);
        let target_key = key.clone();
        target.set_generic_target(&parameters, move |checker, target, arguments| {
            let state = weak.upgrade().ok_or(Error::Released)?;
            if !std::ptr::eq(checker, &raw const state.checker) {
                return missing("tuple factory used by another checker");
            }
            state.normalize_tuple(location, target, &target_key, arguments, None)
        })?;
        let weak = Rc::downgrade(self);
        target.set_marker_arguments(move |parameters, source, marker| {
            weak.upgrade()
                .ok_or(Error::Released)?
                .marker_arguments(parameters, source, marker)
        })?;
        let this = self.checker.graph.type_parameter("this", Some(&target));
        self.interface_this.borrow_mut().insert(target.id(), this);
        let weak = Rc::downgrade(self);
        let parameter_links = parameters.iter().map(Rc::downgrade).collect::<Vec<_>>();
        let readonly = key.readonly;
        let element_flags = flags.clone();
        // port: tsc/internal/checker/checker.go:Checker.getTupleBaseType
        let base = TypeLink::lazy(move || {
            let state = weak.upgrade().ok_or(Error::Released)?;
            let number = state.builtin(tf::NUMBER)?;
            let mut elements = Vec::with_capacity(parameter_links.len());
            for (parameter, flag) in parameter_links.iter().zip(&element_flags) {
                let parameter = parameter.upgrade().ok_or(Error::Released)?;
                // A variadic element contributes what it spreads: `T[number]`.
                elements.push(if flag & ef::VARIADIC != 0 {
                    state.indexed_access(parameter, number.clone())?
                } else {
                    parameter
                });
            }
            let element = state.checker.graph.union(&elements)?;
            state.array(location, element, readonly)
        });
        self.tuple_bases.borrow_mut().insert(target.id(), base);
        self.tuple_instances.borrow_mut().insert(
            (target.id(), parameters.iter().map(|ty| ty.id()).collect()),
            target.clone(),
        );
        self.tuple_targets.borrow_mut().insert(key, target.clone());
        Ok(target)
    }

    fn tuple_length(&self, flags: &[u8]) -> Result<Rc<TypeCell>, Error> {
        if flags.iter().any(|flag| *flag & ef::VARIABLE != 0) {
            return self.builtin(tf::NUMBER);
        }
        let min = flags
            .iter()
            .filter(|flag| **flag & (ef::REQUIRED | ef::VARIADIC) != 0)
            .count();
        let lengths = (min..=flags.len())
            .map(|length| {
                self.checker.graph.intern_literal(
                    tf::NUMBER_LITERAL,
                    LiteralValue::Number((length as f64).to_bits()),
                    &length.to_string(),
                )
            })
            .collect::<Vec<_>>();
        self.checker.graph.union(&lengths)
    }

    // Constructor-style entry: callers hand over handles they have just built.
    #[allow(clippy::needless_pass_by_value)]
    fn tuple_instance(
        self: &Rc<Self>,
        location: NodeId,
        target: &Rc<TypeCell>,
        elements: Vec<TypeLink>,
        key: &TupleTargetKey,
        alias: Option<Alias>,
        intern: bool,
    ) -> Result<Rc<TypeCell>, Error> {
        let resolved = if intern {
            Some(
                elements
                    .iter()
                    .map(TypeLink::resolve)
                    .collect::<Result<Vec<_>, _>>()?,
            )
        } else {
            None
        };
        let cache_key = resolved.as_ref().map(|types| {
            (
                target.id(),
                types.iter().map(|ty| ty.id()).collect::<Vec<_>>(),
            )
        });
        if let Some(key) = &cache_key {
            if let Some(instance) = self.tuple_instances.borrow().get(key).cloned() {
                return Ok(instance);
            }
        }
        let flags = key.infos.iter().map(|(flag, _)| *flag).collect::<Vec<_>>();
        let length = self.tuple_length(&flags)?;
        let weak = Rc::downgrade(self);
        let length = Rc::downgrade(&length);
        let name = alias
            .as_ref()
            .map_or_else(|| Rc::from("tuple"), |alias| alias.name.clone());
        let tuple = self.checker.graph.allocate_full(
            tf::OBJECT,
            of::REFERENCE,
            name,
            None,
            alias.as_ref().map(|alias| self.identity(alias.declaration)),
            None,
            false,
            Vec::new(),
            false,
            Some(Box::new(move |_, cell| {
                weak.upgrade().ok_or(Error::Released)?.tuple_members(
                    location,
                    cell,
                    length.upgrade().ok_or(Error::Released)?,
                )
            })),
        );
        tuple.set_lazy_tuple_shape(elements.clone(), &flags, key.readonly)?;
        if intern {
            tuple.set_lazy_reference_shape(target, elements)?;
        } else {
            tuple.set_deferred_reference_shape(target, elements, location)?;
        }
        if let Some(key) = cache_key {
            self.tuple_instances.borrow_mut().insert(key, tuple.clone());
        }
        Ok(tuple)
    }

    // Constructor-style entry: callers hand over handles they have just built.
    #[allow(clippy::needless_pass_by_value)]
    // port: tsc/internal/checker/checker.go:Checker.resolveObjectTypeMembers
    fn tuple_members(
        self: &Rc<Self>,
        _location: NodeId,
        cell: &TypeCell,
        length: Rc<TypeCell>,
    ) -> Result<Structure, Error> {
        let shape = cell.tuple_shape().ok_or(Error::ResolutionFailed)?;
        let elements = shape.elements()?;
        let target = cell
            .reference_shape()
            .ok_or(Error::ResolutionFailed)?
            .target()?;
        let base = self
            .tuple_bases
            .borrow()
            .get(&target.id())
            .cloned()
            .ok_or(Error::ResolutionFailed)?;
        let declared_base = base.resolve()?;
        // resolveTypeReferenceMembers pads the type arguments with the type
        // itself as `this`, for the tuple target too (the empty tuple `[]` is
        // its own target): its base is the array type with that this argument.
        let mapper = {
            let mut sources = target
                .tuple_shape()
                .ok_or(Error::ResolutionFailed)?
                .elements()?;
            sources.push(
                self.interface_this
                    .borrow()
                    .get(&target.id())
                    .cloned()
                    .ok_or(Error::ResolutionFailed)?,
            );
            let mut arguments = elements.clone();
            arguments.push(
                self.checker
                    .graph
                    .types
                    .borrow()
                    .get(cell.id() as usize - 1)
                    .cloned()
                    .ok_or(Error::Released)?,
            );
            Some(Mapper::with_types(
                Environment::default(),
                &sources,
                &arguments,
            )?)
        };
        let array = if let Some(mapper) = &mapper {
            let base = self.instantiate(&declared_base, mapper, None)?;
            let reference = base.reference_shape().ok_or(Error::ResolutionFailed)?;
            let array_target = reference.target()?;
            let arguments = reference.arguments()?;
            let name: &[u8] = if shape.readonly {
                b"ReadonlyArray"
            } else {
                b"Array"
            };
            let group = self
                .input
                .resolve_global(name, ts_ast::symbol_flags::TYPE)?
                .ok_or(Error::ResolutionFailed)?;
            let this = self
                .checker
                .graph
                .types
                .borrow()
                .get(cell.id() as usize - 1)
                .cloned()
                .ok_or(Error::Released)?;
            self.instantiate_interface_with_this(&group, &array_target, &arguments, &this)?
        } else {
            declared_base
        };
        let mut structure = array.structure(&self.checker.graph)?.clone();
        let mut members = Vec::new();
        let declared_elements = target
            .tuple_shape()
            .ok_or(Error::ResolutionFailed)?
            .elements()?;
        for (index, (ty, flag)) in declared_elements
            .iter()
            .zip(&shape.element_flags)
            .enumerate()
        {
            if flag & ef::VARIABLE != 0 {
                break;
            }
            let r#type = if let Some(mapper) = &mapper {
                let mapper = mapper.clone();
                let weak = Rc::downgrade(self);
                let ty = Rc::downgrade(ty);
                TypeLink::lazy(move || {
                    weak.upgrade().ok_or(Error::Released)?.instantiate(
                        &ty.upgrade().ok_or(Error::Released)?,
                        &mapper,
                        None,
                    )
                })
            } else {
                Rc::downgrade(ty).into()
            };
            members.push(Member {
                name_type: None,
                name: index.to_string().into(),
                optional: *flag == ef::OPTIONAL,
                readonly: shape.readonly,
                class_member: true,
                r#type,
            });
        }
        members.push(Member {
            name_type: None,
            name: Rc::from("length"),
            optional: false,
            readonly: shape.readonly,
            class_member: true,
            r#type: Rc::downgrade(&length).into(),
        });
        members.extend(
            structure
                .members
                .into_iter()
                .filter(|member| &*member.name != "length"),
        );
        structure.members = members;
        Ok(structure)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bound::BoundChecker;
    use crate::bound_input::BoundInput;
    use crate::bound_input::BoundInputOptions;
    use ts_ast::SourceFileParseOptions;
    use ts_core::ScriptKind;
    use ts_jsstring::{JsString, SourceText};

    #[test]
    fn aliased_tuple_creates_target_but_defers_element_resolution() {
        let file = ts_binder::bind_parsed_file(ts_parser::parse_source_file(
            SourceText::from_loaded_bytes(b"interface Array<T> { length: number; [n: number]: T } interface ReadonlyArray<T> { readonly length: number; readonly [n: number]: T } type A = [string, number?, ...boolean[]]; type B = [string, number?, ...boolean[]];".as_slice()),
            ScriptKind::TS,
            SourceFileParseOptions { file_name: JsString::from_bytes(b"/tuple.ts".as_slice()), ..Default::default() },
        )).unwrap();
        let source = file.source();
        let input = BoundInput::new(
            vec![file],
            BoundInputOptions {
                strict_null_checks: true,
                ..Default::default()
            },
            Vec::new(),
        )
        .unwrap();
        let checker = BoundChecker::new(input).unwrap();
        let declaration = |name| {
            checker
                .input()
                .declaration_by_name(source, name)
                .unwrap()
                .unwrap()
        };
        let before = checker.checker().graph.len();
        let a = checker.declared_type(declaration(b"A")).unwrap();
        assert_eq!(
            checker.checker().graph.len() - before,
            6,
            "three slot parameters, the target and this parameter, and one deferred reference"
        );
        let after_a = checker.checker().graph.len();
        let b = checker.declared_type(declaration(b"B")).unwrap();
        assert_eq!(
            checker.checker().graph.len() - after_a,
            1,
            "equivalent unlabelled tuples share the target"
        );
        assert!(Rc::ptr_eq(
            &a.reference_shape().unwrap().target().unwrap(),
            &b.reference_shape().unwrap().target().unwrap()
        ));
        let before_arguments = checker.checker().graph.len();
        let arguments = a.tuple_shape().unwrap().elements().unwrap();
        assert_eq!(arguments.len(), 3);
        assert_eq!(arguments[0].flags(), tf::STRING);
        assert_ne!(arguments[1].flags() & tf::UNION, 0);
        assert_ne!(arguments[2].flags() & tf::BOOLEAN, 0);
        assert_eq!(
            checker.checker().graph.len() - before_arguments,
            1,
            "only the optional union is new; the rest array syntax creates no Array reference"
        );
        assert!(Rc::ptr_eq(
            &a,
            &checker.declared_type(declaration(b"A")).unwrap()
        ));
    }
}
