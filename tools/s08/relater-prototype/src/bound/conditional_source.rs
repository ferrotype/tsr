//! Conditional roots retain canonical types and compose real instantiation maps.
//! Tail recursion is evaluated by the pinned loop, independently of Rust's stack.

use super::instantiate::{Alias, Mapper};
use super::{
    missing, of, tf, Construction, Environment, Error, HashMap, NodeId, Rc, RefCell, TypeCell,
    Weak, K,
};
use crate::tuples::element_flags as ef;

/// One `infer` parameter's candidates, kept apart by the variance of the
/// position each was found in (`InferenceInfo.candidates`/`contraCandidates`).
#[derive(Clone, Default)]
struct Candidates {
    covariant: Vec<Rc<TypeCell>>,
    contra: Vec<Rc<TypeCell>>,
}

/// `InferenceState.contravariant` and `.bivariant`.
#[derive(Clone, Copy, Default)]
struct Variance {
    contravariant: bool,
    bivariant: bool,
}

struct Root {
    node: NodeId,
    check: Weak<TypeCell>,
    extends: Weak<TypeCell>,
    outer: Environment,
    infer: Vec<(NodeId, Weak<TypeCell>)>,
    distributive: bool,
    alias: Option<Alias>,
}
impl Root {
    fn parameters(&self) -> Result<Vec<Rc<TypeCell>>, Error> {
        self.outer
            .0
            .iter()
            .map(|(_, ty)| ty)
            .chain(self.outer.2.iter().map(|(ty, _)| ty))
            .map(|ty| ty.upgrade().ok_or(Error::Released))
            .collect()
    }
    fn has_parameters(&self) -> bool {
        !self.outer.0.is_empty() || !self.outer.2.is_empty()
    }
}
#[derive(Clone)]
struct Instance {
    root: Rc<Root>,
    mapper: Option<Mapper>,
}
#[derive(Clone, PartialEq, Eq, Hash)]
struct Key {
    root: NodeId,
    arguments: Vec<u32>,
    alias: Option<(NodeId, Vec<u32>)>,
}

#[derive(Default)]
pub(super) struct State {
    roots: RefCell<HashMap<NodeId, Rc<Root>>>,
    instances: RefCell<HashMap<u32, Instance>>,
    cache: RefCell<HashMap<Key, Weak<TypeCell>>>,
    permissive: RefCell<HashMap<u32, Weak<TypeCell>>>,
    restrictive: RefCell<HashMap<u32, Weak<TypeCell>>>,
    restrictive_parameters: RefCell<HashMap<u32, Weak<TypeCell>>>,
    permissive_mapper: std::cell::OnceCell<Mapper>,
    restrictive_mapper: std::cell::OnceCell<Mapper>,
}

impl Construction {
    // port: tsc/internal/checker/checker.go:Checker.getTypeFromConditionalTypeNode
    pub(super) fn conditional(
        self: &Rc<Self>,
        node: NodeId,
        env: &Environment,
        name: Option<Rc<str>>,
    ) -> Result<Rc<TypeCell>, Error> {
        let existing = self.conditional.roots.borrow().get(&node).cloned();
        let root = if let Some(root) = existing {
            root
        } else {
            let read = self.input.node(node)?;
            let data = read.data_source();
            let data = data
                .as_conditional_type_node()
                .ok_or(Error::ResolutionFailed)?;
            let check = self.type_node(
                data.check_type().ok_or(Error::ResolutionFailed)?,
                &Environment::default(),
                None,
            )?;
            let extends = self.type_node(
                data.extends_type().ok_or(Error::ResolutionFailed)?,
                &Environment::default(),
                None,
            )?;
            let alias = self.alias_metadata(node, env, name)?;
            let mut outer = self.outer_environment(node)?;
            if alias
                .as_ref()
                .is_none_or(|alias| alias.arguments.is_empty())
            {
                let mut referenced = Vec::new();
                for (parameter, ty) in outer.0 {
                    if self.parameter_possibly_referenced(parameter, node)? {
                        referenced.push((parameter, ty));
                    }
                }
                outer.0 = referenced;
            }
            let mut infer = Vec::new();
            for declaration in self.infer_type_parameters(node)? {
                infer.push((
                    declaration,
                    Rc::downgrade(&self.type_parameter(declaration)?),
                ));
            }
            let root = Rc::new(Root {
                node,
                check: Rc::downgrade(&check),
                extends: Rc::downgrade(&extends),
                outer,
                infer,
                distributive: check.flags & tf::TYPE_PARAMETER != 0,
                alias,
            });
            self.conditional
                .roots
                .borrow_mut()
                .insert(node, root.clone());
            root
        };
        let result = self.evaluate_conditional(root.clone(), None, None)?;
        if root.has_parameters() {
            let arguments = root.parameters()?.iter().map(|ty| ty.id()).collect();
            self.conditional.cache.borrow_mut().insert(
                Key {
                    root: node,
                    arguments,
                    alias: None,
                },
                Rc::downgrade(&result),
            );
        }
        Ok(result)
    }

    // port: tsc/internal/checker/checker.go:Checker.getConditionalTypeInstantiation
    pub(super) fn instantiate_conditional(
        self: &Rc<Self>,
        ty: &Rc<TypeCell>,
        mapper: &Mapper,
        alias: Option<Alias>,
    ) -> Result<Rc<TypeCell>, Error> {
        let instance = self
            .conditional
            .instances
            .borrow()
            .get(&ty.id())
            .cloned()
            .ok_or_else(|| Error::Unsupported("conditional root metadata missing".into()))?;
        let composed = instance
            .mapper
            .as_ref()
            .map(|first| Mapper::compose(first, mapper));
        let mapper = composed.as_ref().unwrap_or(mapper);
        let root = instance.root;
        if !root.has_parameters() {
            return Ok(ty.clone());
        }
        let arguments = root
            .parameters()?
            .iter()
            .map(|ty| self.map_type(ty, mapper))
            .collect::<Result<Vec<_>, _>>()?;
        let key = Key {
            root: root.node,
            arguments: arguments.iter().map(|ty| ty.id()).collect(),
            alias: alias
                .as_ref()
                .map(|alias| {
                    Ok::<_, Error>((
                        alias.declaration,
                        alias
                            .arguments
                            .iter()
                            .map(|ty| ty.upgrade().map(|ty| ty.id()).ok_or(Error::Released))
                            .collect::<Result<_, Error>>()?,
                    ))
                })
                .transpose()?,
        };
        if let Some(cached) = self.conditional.cache.borrow().get(&key) {
            return cached.upgrade().ok_or(Error::Released);
        }
        let mapper = Self::conditional_mapper(&root, &arguments)?;
        let check = root.check.upgrade().ok_or(Error::Released)?;
        let distribution = if root.distributive {
            Some(self.map_type(&check, &mapper)?)
        } else {
            None
        };
        let result = if let Some(distribution) = distribution
            .filter(|ty| !Rc::ptr_eq(ty, &check) && ty.flags & (tf::UNION | tf::NEVER) != 0)
        {
            let mut results = Vec::new();
            if distribution.flags & tf::NEVER == 0 {
                for part in distribution.types()? {
                    let part_mapper = Mapper::with_types(
                        Environment::default(),
                        std::slice::from_ref(&check),
                        &[part],
                    )?;
                    let mapper = Mapper::compose(&part_mapper, &mapper);
                    results.push(self.evaluate_conditional(root.clone(), Some(mapper), None)?);
                }
            }
            if let Some(alias) = &alias {
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
                )?
            } else {
                self.checker.graph.union(&results)?
            }
        } else {
            self.evaluate_conditional(root, Some(mapper), alias)?
        };
        self.conditional
            .cache
            .borrow_mut()
            .insert(key, Rc::downgrade(&result));
        Ok(result)
    }

    fn conditional_mapper(root: &Root, arguments: &[Rc<TypeCell>]) -> Result<Mapper, Error> {
        let parameters = root.parameters()?;
        let mut environment = Environment::default();
        for ((node, _), argument) in root.outer.0.iter().zip(arguments) {
            environment.0.push((*node, Rc::downgrade(argument)));
        }
        for ((parameter, _), argument) in root.outer.2.iter().zip(&arguments[root.outer.0.len()..])
        {
            environment
                .2
                .push((parameter.clone(), Rc::downgrade(argument)));
        }
        Mapper::with_types(environment, &parameters, arguments)
    }

    fn instantiate_optional(
        self: &Rc<Self>,
        ty: &Rc<TypeCell>,
        mapper: Option<&Mapper>,
    ) -> Result<Rc<TypeCell>, Error> {
        if let Some(mapper) = mapper {
            self.instantiate(ty, mapper, None)
        } else {
            Ok(ty.clone())
        }
    }

    // port: tsc/internal/checker/checker.go:Checker.getConditionalType
    fn evaluate_conditional(
        self: &Rc<Self>,
        mut root: Rc<Root>,
        mut mapper: Option<Mapper>,
        mut alias: Option<Alias>,
    ) -> Result<Rc<TypeCell>, Error> {
        let mut tail_count = 0;
        let mut extra = Vec::new();
        let result = loop {
            if tail_count == 1000 {
                break self
                    .instantiation_limit(root.check.upgrade().ok_or(Error::Released)?.as_ref())?;
            }
            let check = self.instantiate_optional(
                &root.check.upgrade().ok_or(Error::Released)?,
                mapper.as_ref(),
            )?;
            let extends = self.instantiate_optional(
                &root.extends.upgrade().ok_or(Error::Released)?,
                mapper.as_ref(),
            )?;
            let error = self
                .initialization
                .named
                .borrow()
                .get("errorType")
                .cloned()
                .ok_or(Error::ResolutionFailed)?;
            let wildcard = self
                .initialization
                .named
                .borrow()
                .get("wildcardType")
                .cloned()
                .ok_or(Error::ResolutionFailed)?;
            if Rc::ptr_eq(&check, &error) || Rc::ptr_eq(&extends, &error) {
                break error;
            }
            if Rc::ptr_eq(&check, &wildcard) || Rc::ptr_eq(&extends, &wildcard) {
                break wildcard;
            }
            let read = self.input.node(root.node)?;
            let data = read.data_source();
            let data = data
                .as_conditional_type_node()
                .ok_or(Error::ResolutionFailed)?;
            let check_node = data.check_type().ok_or(Error::ResolutionFailed)?;
            let extends_node = data.extends_type().ok_or(Error::ResolutionFailed)?;
            let check_tuples = self
                .simple_tuple_arity(check_node)?
                .zip(self.simple_tuple_arity(extends_node)?)
                .is_some_and(|(a, b)| a == b);
            let check_deferred = Self::conditional_deferred(&check, check_tuples)?;
            let combined = if root.infer.is_empty() {
                None
            } else {
                let parameters = root
                    .infer
                    .iter()
                    .map(|(_, ty)| ty.upgrade().ok_or(Error::Released))
                    .collect::<Result<Vec<_>, _>>()?;
                let mut candidates = vec![Candidates::default(); parameters.len()];
                if !check_deferred {
                    self.infer_conditional_types(
                        root.node,
                        &check,
                        &extends,
                        &parameters,
                        &mut candidates,
                        Variance::default(),
                    )?;
                }
                let mut inferred = Vec::new();
                for (parameter, candidates) in parameters.iter().zip(candidates) {
                    // getTypeFromInference: covariant candidates first, then a
                    // contravariant intersection, then the constraint.
                    let contra = candidates.covariant.is_empty() && !candidates.contra.is_empty();
                    let candidates = if contra {
                        candidates.contra
                    } else {
                        candidates.covariant
                    };
                    let ty = if candidates.len() == 1 {
                        candidates[0].clone()
                    } else if contra {
                        self.intersection(&candidates, "intersection".into(), None)?
                    } else if candidates.is_empty() {
                        parameter
                            .type_parameter_shape()
                            .map(crate::TypeParameterShape::constraint)
                            .transpose()?
                            .flatten()
                            .unwrap_or(self.builtin(tf::UNKNOWN)?)
                    } else {
                        return missing("conditional inference candidate priority/variance");
                    };
                    inferred.push(ty);
                }
                let mut environment = Environment::default();
                for ((node, _), ty) in root.infer.iter().zip(&inferred) {
                    environment.0.push((*node, Rc::downgrade(ty)));
                }
                let inference = Mapper::with_types(environment, &parameters, &inferred)?;
                Some(if let Some(mapper) = &mapper {
                    Mapper::compose(&inference, mapper)
                } else {
                    inference
                })
            };
            let inferred_extends = if let Some(combined) = &combined {
                self.instantiate(
                    &root.extends.upgrade().ok_or(Error::Released)?,
                    combined,
                    None,
                )?
            } else {
                extends
            };
            if !check_deferred && !Self::conditional_deferred(&inferred_extends, check_tuples)? {
                let top = inferred_extends.flags & tf::ANY_OR_UNKNOWN != 0;
                if !top
                    && (check.flags & tf::ANY != 0
                        || !self.checker.is_type_assignable_to(
                            &self.permissive_instantiation(&check)?,
                            &self.permissive_instantiation(&inferred_extends)?,
                        )?)
                {
                    if check.flags & tf::ANY != 0 {
                        let true_type = self.type_node(
                            data.true_type().ok_or(Error::ResolutionFailed)?,
                            &Environment::default(),
                            None,
                        )?;
                        extra.push(self.instantiate_optional(
                            &true_type,
                            combined.as_ref().or(mapper.as_ref()),
                        )?);
                    }
                    let false_type = self.type_node(
                        data.false_type().ok_or(Error::ResolutionFailed)?,
                        &Environment::default(),
                        None,
                    )?;
                    if let Some(next) = self
                        .conditional
                        .instances
                        .borrow()
                        .get(&false_type.id())
                        .cloned()
                    {
                        let parent = self.input.node(next.root.node)?.parent();
                        let next_check = next.root.check.upgrade().ok_or(Error::Released)?;
                        if parent == Some(root.node)
                            && (!next.root.distributive
                                || Rc::ptr_eq(
                                    &next_check,
                                    &root.check.upgrade().ok_or(Error::Released)?,
                                ))
                        {
                            root = next.root;
                            continue;
                        }
                    }
                    if let Some((next, next_mapper)) =
                        self.conditional_tail(&false_type, mapper.as_ref())?
                    {
                        root = next;
                        mapper = Some(next_mapper);
                        alias = None;
                        if root.alias.is_some() {
                            tail_count += 1;
                        }
                        continue;
                    }
                    break self.instantiate_optional(&false_type, mapper.as_ref())?;
                }
                if top
                    || self.checker.is_type_assignable_to(
                        &self.restrictive_instantiation(&check)?,
                        &self.restrictive_instantiation(&inferred_extends)?,
                    )?
                {
                    let true_type = self.type_node(
                        data.true_type().ok_or(Error::ResolutionFailed)?,
                        &Environment::default(),
                        None,
                    )?;
                    let true_mapper = combined.as_ref().or(mapper.as_ref());
                    if let Some((next, next_mapper)) =
                        self.conditional_tail(&true_type, true_mapper)?
                    {
                        root = next;
                        mapper = Some(next_mapper);
                        alias = None;
                        if root.alias.is_some() {
                            tail_count += 1;
                        }
                        continue;
                    }
                    break self.instantiate_optional(&true_type, true_mapper)?;
                }
            }
            // newConditionalType instantiates the root's check and extends
            // types afresh (the extends type without the inferences), and only
            // then is the alias instantiated.
            let check = self.instantiate_optional(
                &root.check.upgrade().ok_or(Error::Released)?,
                mapper.as_ref(),
            )?;
            let deferred_extends = self.instantiate_optional(
                &root.extends.upgrade().ok_or(Error::Released)?,
                mapper.as_ref(),
            )?;
            let alias = if alias.is_some() {
                alias
            } else if let Some(mapper) = &mapper {
                self.instantiate_alias(root.alias.clone(), mapper)?
            } else {
                root.alias.clone()
            };
            let name = alias
                .as_ref()
                .map_or_else(|| Rc::from("conditional"), |alias| alias.name.clone());
            let cell = self.checker.graph.allocate_full(
                tf::CONDITIONAL,
                0,
                name,
                None,
                alias.as_ref().map(|alias| self.identity(alias.declaration)),
                None,
                false,
                vec![Rc::downgrade(&check), Rc::downgrade(&deferred_extends)],
                false,
                None,
            );
            self.conditional.instances.borrow_mut().insert(
                cell.id(),
                Instance {
                    root: root.clone(),
                    mapper,
                },
            );
            break cell;
        };
        if extra.is_empty() {
            Ok(result)
        } else {
            extra.push(result);
            self.checker.graph.union(&extra)
        }
    }

    // port: tsc/internal/checker/checker.go:Checker.getTailRecursionRoot
    fn conditional_tail(
        self: &Rc<Self>,
        ty: &Rc<TypeCell>,
        mapper: Option<&Mapper>,
    ) -> Result<Option<(Rc<Root>, Mapper)>, Error> {
        let Some(mapper) = mapper else {
            return Ok(None);
        };
        let instance = self.conditional.instances.borrow().get(&ty.id()).cloned();
        let Some(instance) = instance else {
            return Ok(None);
        };
        if !instance.root.has_parameters() {
            return Ok(None);
        }
        let combined = if let Some(original) = instance.mapper {
            Mapper::compose(&original, mapper)
        } else {
            mapper.clone()
        };
        let arguments = instance
            .root
            .parameters()?
            .iter()
            .map(|ty| self.map_type(ty, &combined))
            .collect::<Result<Vec<_>, _>>()?;
        let next = Self::conditional_mapper(&instance.root, &arguments)?;
        if instance.root.distributive {
            let original = instance.root.check.upgrade().ok_or(Error::Released)?;
            let check = self.map_type(&original, &next)?;
            if !Rc::ptr_eq(&check, &original) && check.flags & (tf::UNION | tf::NEVER) != 0 {
                return Ok(None);
            }
        }
        Ok(Some((instance.root, next)))
    }

    fn conditional_deferred(ty: &Rc<TypeCell>, check_tuples: bool) -> Result<bool, Error> {
        if ty.flags & tf::INSTANTIABLE_NON_PRIMITIVE != 0 {
            return Ok(true);
        }
        if ty.object_flags & of::MAPPED != 0 {
            return missing("conditional generic mapped constraint");
        }
        if let Some(tuple) = ty.tuple_shape() {
            if tuple.combined_flags & crate::element_flags::VARIADIC != 0 {
                return Ok(true);
            }
            if check_tuples {
                for element in tuple.elements()? {
                    if Self::conditional_deferred(&element, false)? {
                        return Ok(true);
                    }
                }
            }
        }
        Ok(false)
    }

    fn simple_tuple_arity(&self, mut node: NodeId) -> Result<Option<usize>, Error> {
        while self.input.node(node)?.kind() == K::ParenthesizedType {
            node = self
                .input
                .node(node)?
                .type_node()
                .ok_or(Error::ResolutionFailed)?;
        }
        let read = self.input.node(node)?;
        let Some(data) = read.data_source().as_tuple_type_node() else {
            return Ok(None);
        };
        let elements = self.list(node, data.elements())?;
        if elements.is_empty() {
            return Ok(None);
        }
        for element in &elements {
            let read = self.input.node(*element)?;
            if matches!(read.kind().known(), Some(K::OptionalType | K::RestType))
                || read
                    .data_source()
                    .as_named_tuple_member()
                    .is_some_and(|data| {
                        data.question_token().is_some() || data.dot_dot_dot_token().is_some()
                    })
            {
                return Ok(None);
            }
        }
        Ok(Some(elements.len()))
    }

    fn permissive_instantiation(self: &Rc<Self>, ty: &Rc<TypeCell>) -> Result<Rc<TypeCell>, Error> {
        if ty.flags & (tf::PRIMITIVE | tf::ANY_OR_UNKNOWN | tf::NEVER) != 0 {
            return Ok(ty.clone());
        }
        if let Some(cached) = self.conditional.permissive.borrow().get(&ty.id()) {
            return cached.upgrade().ok_or(Error::Released);
        }
        let mapper = self
            .conditional
            .permissive_mapper
            .get_or_init(Mapper::permissive);
        let result = self.instantiate(ty, mapper, None)?;
        self.conditional
            .permissive
            .borrow_mut()
            .insert(ty.id(), Rc::downgrade(&result));
        Ok(result)
    }

    fn restrictive_instantiation(
        self: &Rc<Self>,
        ty: &Rc<TypeCell>,
    ) -> Result<Rc<TypeCell>, Error> {
        if ty.flags & (tf::PRIMITIVE | tf::ANY_OR_UNKNOWN | tf::NEVER) != 0 {
            return Ok(ty.clone());
        }
        if let Some(cached) = self.conditional.restrictive.borrow().get(&ty.id()) {
            return cached.upgrade().ok_or(Error::Released);
        }
        let mapper = self
            .conditional
            .restrictive_mapper
            .get_or_init(Mapper::restrictive);
        let result = self.instantiate(ty, mapper, None)?;
        self.conditional
            .restrictive
            .borrow_mut()
            .insert(ty.id(), Rc::downgrade(&result));
        self.conditional
            .restrictive
            .borrow_mut()
            .insert(result.id(), Rc::downgrade(&result));
        Ok(result)
    }

    pub(super) fn restrictive_type_parameter(
        self: &Rc<Self>,
        ty: &Rc<TypeCell>,
    ) -> Result<Rc<TypeCell>, Error> {
        if ty
            .type_parameter_shape()
            .map(crate::TypeParameterShape::constraint)
            .transpose()?
            .flatten()
            .is_none()
        {
            return Ok(ty.clone());
        }
        if let Some(cached) = self
            .conditional
            .restrictive_parameters
            .borrow()
            .get(&ty.id())
        {
            return cached.upgrade().ok_or(Error::Released);
        }
        let result = self.checker.graph.type_parameter(ty.name(), None);
        self.conditional
            .restrictive_parameters
            .borrow_mut()
            .insert(ty.id(), Rc::downgrade(&result));
        Ok(result)
    }

    fn infer_conditional_types(
        self: &Rc<Self>,
        location: NodeId,
        source: &Rc<TypeCell>,
        target: &Rc<TypeCell>,
        parameters: &[Rc<TypeCell>],
        candidates: &mut [Candidates],
        variance: Variance,
    ) -> Result<(), Error> {
        if let Some(index) = parameters
            .iter()
            .position(|parameter| Rc::ptr_eq(parameter, target))
        {
            // A candidate found in a purely contravariant position is kept
            // apart, as upstream's `contraCandidates` are.
            let candidates = if variance.contravariant && !variance.bivariant {
                &mut candidates[index].contra
            } else {
                &mut candidates[index].covariant
            };
            if !candidates
                .iter()
                .any(|candidate| Rc::ptr_eq(candidate, source))
            {
                candidates.push(source.clone());
            }
            return Ok(());
        }
        if let (Some(source_ref), Some(target_ref)) =
            (source.reference_shape(), target.reference_shape())
        {
            let root = source_ref.target()?;
            if Rc::ptr_eq(&root, &target_ref.target()?) {
                let variances = if source.array_element().is_some() {
                    vec![1]
                } else {
                    self.checker.variances(&root)?
                };
                for (index, (source, target)) in source_ref
                    .arguments()?
                    .iter()
                    .zip(target_ref.arguments()?)
                    .enumerate()
                {
                    if variances.get(index).is_some_and(|variance| *variance == 2) {
                        return missing("conditional contravariant inference candidates");
                    }
                    self.infer_conditional_types(
                        location, source, &target, parameters, candidates, variance,
                    )?;
                }
                return Ok(());
            }
        }
        if source.flags & tf::OBJECT != 0 && target.flags & tf::OBJECT != 0 {
            let source = source.structure(&self.checker.graph)?;
            let target = target.structure(&self.checker.graph)?;
            for property in &target.members {
                if let Some(source) = source
                    .members
                    .iter()
                    .find(|member| member.name == property.name)
                {
                    self.infer_conditional_types(
                        location,
                        &source.r#type()?,
                        &property.r#type()?,
                        parameters,
                        candidates,
                        variance,
                    )?;
                }
            }
            // port: tsc/internal/checker/inference.go:Checker.inferFromSignatures
            let count = source
                .call_signatures
                .len()
                .min(target.call_signatures.len());
            let source_signatures = &source.call_signatures[source.call_signatures.len() - count..];
            let target_signatures = &target.call_signatures[target.call_signatures.len() - count..];
            for (source, target) in source_signatures.iter().zip(target_signatures) {
                self.infer_from_signature(
                    location, source, target, parameters, candidates, variance,
                )?;
            }
        }
        Ok(())
    }

    // port: tsc/internal/checker/inference.go:Checker.inferFromSignature
    /// Parameters are contravariant positions; the reference has no
    /// contra-candidate model, so a target parameter that could hold one of
    /// the inferred parameters is refused by name. The types are resolved and
    /// the source's rest tuple is built regardless, as upstream does.
    fn infer_from_signature(
        self: &Rc<Self>,
        location: NodeId,
        source: &crate::Signature,
        target: &crate::Signature,
        parameters: &[Rc<TypeCell>],
        candidates: &mut [Candidates],
        variance: Variance,
    ) -> Result<(), Error> {
        if source.generic.is_some() || target.generic.is_some() {
            return missing("conditional inference between generic signatures");
        }
        // Once inference descends into a bivariant signature it stays bivariant.
        let parameter_variance = Variance {
            contravariant: !variance.contravariant,
            bivariant: variance.bivariant || target.bivariant_parameters,
        };
        if source.this_type.is_some() && target.this_type.is_some() {
            return missing("conditional inference of this types");
        }
        // applyToParameterTypes
        let source_count = source.parameters.len();
        let target_count = target.parameters.len();
        let target_non_rest = target_count - usize::from(target.has_rest_parameter);
        let paired = if source.has_rest_parameter {
            target_non_rest
        } else {
            source_count.min(target_non_rest)
        };
        for index in 0..paired {
            if source.has_rest_parameter && index + 1 >= source_count {
                return missing("conditional inference from a source rest parameter");
            }
            let source_type = source.parameters[index].resolve()?;
            let target_type = target.parameters[index].resolve()?;
            // A conditional's inferences are always made with strict function
            // types, so a parameter position always flips variance.
            self.infer_conditional_types(
                location,
                &source_type,
                &target_type,
                parameters,
                candidates,
                parameter_variance,
            )?;
        }
        if target.has_rest_parameter {
            let source_rest = self.rest_type_at_position(location, source, paired)?;
            let rest = target.parameters[target_count - 1].resolve()?;
            // getEffectiveRestType: `any` stands for `any[]` and holds no
            // type variable, so it collects no candidate.
            if rest.flags & tf::ANY == 0 {
                self.infer_conditional_types(
                    location,
                    &source_rest,
                    &rest,
                    parameters,
                    candidates,
                    parameter_variance,
                )?;
            }
        }
        // applyToReturnTypes keeps the current variance.
        self.infer_conditional_types(
            location,
            &source.return_type.resolve()?,
            &target.return_type.resolve()?,
            parameters,
            candidates,
            variance,
        )
    }

    // port: tsc/internal/checker/checker.go:Checker.getRestTypeAtPosition
    fn rest_type_at_position(
        self: &Rc<Self>,
        location: NodeId,
        source: &crate::Signature,
        position: usize,
    ) -> Result<Rc<TypeCell>, Error> {
        if source.has_rest_parameter {
            return missing("rest type of a signature with a rest parameter");
        }
        // getMinArgumentCount: trailing parameters that accept void are optional.
        let mut minimum = source.min_argument_count;
        while minimum > 0 {
            let ty = source.parameters[minimum - 1].resolve()?;
            let accepts_void = if ty.flags & tf::UNION != 0 {
                ty.types()?.iter().any(|part| part.flags & tf::VOID != 0)
            } else {
                ty.flags & tf::VOID != 0
            };
            if !accepts_void {
                break;
            }
            minimum -= 1;
        }
        let mut types = Vec::new();
        let mut infos = Vec::new();
        for index in position..source.parameters.len() {
            types.push(source.parameters[index].resolve()?);
            infos.push((
                if index < minimum {
                    ef::REQUIRED
                } else {
                    ef::OPTIONAL
                },
                source.parameter_declarations.get(index).copied().flatten(),
            ));
        }
        self.create_tuple_type(location, &types, infos, false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bound::BoundChecker;
    use crate::bound_input::BoundInput;
    use crate::bound_input::BoundInputOptions;

    fn fixture(text: &[u8]) -> (BoundChecker, NodeId) {
        let file = ts_binder::bind_parsed_file(ts_parser::parse_source_file(
            ts_jsstring::SourceText::from_loaded_bytes(text),
            ts_core::ScriptKind::TS,
            ts_ast::SourceFileParseOptions {
                file_name: ts_ast::JsString::from_bytes(b"/conditional.ts".as_slice()),
                ..Default::default()
            },
        ))
        .unwrap();
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
        (BoundChecker::new(input).unwrap(), source)
    }
    fn named(checker: &BoundChecker, source: NodeId, name: &[u8]) -> Rc<TypeCell> {
        checker
            .declared_type(
                checker
                    .input()
                    .declaration_by_name(source, name)
                    .unwrap()
                    .unwrap(),
            )
            .unwrap()
    }

    #[test]
    fn distributive_conditionals_intern_branches_and_repeat_without_work() {
        let (owner,source) = fixture(b"type Pick<T> = T extends string ? 'yes' : 'no'; type A = Pick<'x' | 1>; type B = 'yes' | 'no'; type C = Pick<'x' | 1>;");
        let a = named(&owner, source, b"A");
        let b = named(&owner, source, b"B");
        assert!(owner.checker().is_type_assignable_to(&a, &b).unwrap());
        let before = owner.checker().graph.len();
        let repeated = named(&owner, source, b"A");
        assert!(Rc::ptr_eq(&a, &repeated));
        assert_eq!(owner.checker().graph.len(), before);
        // A second alias preserves its own alias identity; only constituent
        // literals and the evaluated conditional branches are shared.
        let c = named(&owner, source, b"C");
        assert!(!Rc::ptr_eq(&a, &c));
        assert!(a
            .types()
            .unwrap()
            .iter()
            .zip(c.types().unwrap())
            .all(|(a, c)| Rc::ptr_eq(a, &c)));
        assert_eq!(owner.checker().graph.len(), before + 1);
    }

    #[test]
    fn conditional_tail_constructs_real_tuple_arguments() {
        let (owner,source) = fixture(b"interface Array<T> { length: number; [n:number]: T } interface ReadonlyArray<T> { readonly length:number; readonly[n:number]:T } type Build<N extends number,T extends unknown[] = []> = T['length'] extends N ? T : Build<N,[...T,unknown]>; type A = Build<8>;");
        let a = named(&owner, source, b"A");
        let tuple = a.tuple_shape().unwrap();
        assert_eq!(tuple.elements().unwrap().len(), 8);
        assert!(tuple
            .elements()
            .unwrap()
            .iter()
            .all(|ty| ty.flags() == tf::UNKNOWN));
        assert!(owner.checker().structured_diagnostics().is_empty());
    }
}
