//! Reference-owned type substitution and the pinned active-mapper cache.
//!
//! Resolving a generic reference is not itself an instantiation. The counter
//! advances only when `instantiateTypeWorker` runs, including workers which
//! return their input unchanged. A recursive cache hit does not advance it.

use super::{
    missing, of, tf, Cell, Construction, Environment, EnvironmentKey, Error, HashMap, NodeId,
    NodeListId, Rc, RefCell, TypeCell, Weak, K,
};

/// Identity is the allocation, not the substitution list. In particular two
/// equal mapper lists created for separate operations have separate caches.
#[derive(Clone)]
pub(super) struct Mapper(Rc<MapperData>);

struct MapperData {
    environment: Environment,
    // `this` and tuple-target parameters have no type-parameter declaration.
    // They still map by cell identity, exactly like ordinary TypeMapper sources.
    types: Vec<(Weak<TypeCell>, Weak<TypeCell>)>,
    composite: Option<(Mapper, Mapper)>,
    /// A merged mapper maps the first result through the second; a composite
    /// one instantiates it (`TypeMapKindMerged` against `TypeMapKindComposite`).
    merged: bool,
    functional: Option<FunctionalMapper>,
}

#[derive(Clone, Copy)]
enum FunctionalMapper {
    Permissive,
    Restrictive,
}

impl Mapper {
    pub(super) fn new(mut environment: Environment) -> Self {
        environment.1 = None;
        let types = environment
            .2
            .iter()
            .map(|(source, target)| {
                // Environment source cells belong to the graph which outlives its
                // mapper; retain their weak handles and validate on the read path.
                (source.clone(), target.clone())
            })
            .collect();
        Self(Rc::new(MapperData {
            environment,
            types,
            composite: None,
            merged: false,
            functional: None,
        }))
    }

    pub(super) fn with_types(
        environment: Environment,
        sources: &[Rc<TypeCell>],
        targets: &[Rc<TypeCell>],
    ) -> Result<Self, Error> {
        if sources.len() != targets.len() {
            return missing("mapper source/target arity");
        }
        let mut mapper = Self::new(environment);
        Rc::get_mut(&mut mapper.0)
            .expect("fresh mapper")
            .types
            .extend(
                sources
                    .iter()
                    .zip(targets)
                    .map(|(source, target)| (Rc::downgrade(source), Rc::downgrade(target))),
            );
        Ok(mapper)
    }

    pub(super) fn synthetic_key(&self) -> Result<Vec<(u32, u32)>, Error> {
        if self.0.composite.is_some() {
            return missing("composite mapper must resolve its outer arguments before caching");
        }
        if let Some(functional) = self.0.functional {
            // Zero is not a graph type ID. Reserve it for the two native
            // functional mapper identities in syntax/signature cache keys.
            return Ok(vec![(
                0,
                match functional {
                    FunctionalMapper::Permissive => 1,
                    FunctionalMapper::Restrictive => 2,
                },
            )]);
        }
        self.0
            .types
            .iter()
            .map(|(source, target)| {
                Ok((
                    source.upgrade().ok_or(Error::Released)?.id(),
                    target.upgrade().ok_or(Error::Released)?.id(),
                ))
            })
            .collect()
    }

    pub(super) fn compose(first: &Self, second: &Self) -> Self {
        let mut environment = second.0.environment.clone();
        environment.1 = None;
        Self(Rc::new(MapperData {
            environment,
            types: Vec::new(),
            composite: Some((first.clone(), second.clone())),
            merged: false,
            functional: None,
        }))
    }

    // port: tsc/internal/checker/mapper.go:appendTypeMapping
    pub(super) fn append(
        mapper: &Self,
        source: &Rc<TypeCell>,
        target: &Rc<TypeCell>,
    ) -> Result<Self, Error> {
        let second = Self::with_types(
            Environment::default(),
            std::slice::from_ref(source),
            std::slice::from_ref(target),
        )?;
        let mut environment = mapper.0.environment.clone();
        environment.1 = None;
        Ok(Self(Rc::new(MapperData {
            environment,
            types: Vec::new(),
            composite: Some((mapper.clone(), second)),
            merged: true,
            functional: None,
        })))
    }

    // port: tsc/internal/checker/mapper.go:prependTypeMapping
    pub(super) fn prepend(
        source: &Rc<TypeCell>,
        target: &Rc<TypeCell>,
        mapper: &Self,
    ) -> Result<Self, Error> {
        let first = Self::with_types(
            Environment::default(),
            std::slice::from_ref(source),
            std::slice::from_ref(target),
        )?;
        let mut environment = mapper.0.environment.clone();
        environment.1 = None;
        Ok(Self(Rc::new(MapperData {
            environment,
            types: Vec::new(),
            composite: Some((first, mapper.clone())),
            merged: true,
            functional: None,
        })))
    }

    pub(super) fn permissive() -> Self {
        Self::functional(FunctionalMapper::Permissive)
    }
    pub(super) fn restrictive() -> Self {
        Self::functional(FunctionalMapper::Restrictive)
    }
    fn functional(functional: FunctionalMapper) -> Self {
        let mut mapper = Self::new(Environment::default());
        Rc::get_mut(&mut mapper.0).expect("fresh mapper").functional = Some(functional);
        mapper
    }

    fn same(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.0, &other.0)
    }
}

#[derive(Clone)]
pub(super) struct Alias {
    pub(super) declaration: NodeId,
    pub(super) name: Rc<str>,
    pub(super) arguments: Vec<Weak<TypeCell>>,
}

impl Alias {
    fn key(&self) -> Result<AliasKey, Error> {
        Ok(AliasKey {
            declaration: self.declaration,
            arguments: self
                .arguments
                .iter()
                .map(|ty| ty.upgrade().map(|ty| ty.id()).ok_or(Error::Released))
                .collect::<Result<_, _>>()?,
        })
    }
}

/// Syntax provenance is attached to the canonical cell, before it escapes.
/// `environment` contains the canonical outer parameters in source order.
#[derive(Clone)]
pub(super) struct TypeSource {
    pub(super) node: NodeId,
    pub(super) environment: Environment,
    pub(super) alias: Option<Alias>,
}

/// An alias declaration, its type argument ids and the alias it is named by.
type AliasInstanceKey = (NodeId, Vec<u32>, Option<AliasKey>);

#[derive(Clone, PartialEq, Eq, Hash)]
struct AliasKey {
    declaration: NodeId,
    arguments: Vec<u32>,
}

#[derive(Clone, PartialEq, Eq, Hash)]
struct CacheKey {
    source: u32,
    alias: Option<AliasKey>,
}

#[derive(Clone, PartialEq, Eq, Hash)]
struct SourceKey {
    source: u32,
    arguments: EnvironmentKey,
    alias: Option<AliasKey>,
}

struct Frame {
    mapper: Option<Mapper>,
    types: HashMap<CacheKey, Weak<TypeCell>>,
}

#[derive(Default)]
pub(super) struct State {
    frames: RefCell<Vec<Frame>>,
    active: Cell<usize>,
    depth: Cell<usize>,
    statement_count: Cell<usize>,
    contains_variables: RefCell<HashMap<u32, bool>>,
    sources: RefCell<HashMap<u32, TypeSource>>,
    parameters: RefCell<HashMap<u32, NodeId>>,
    /// The other declarations of a merged class or interface type parameter:
    /// an environment may be keyed by any of them.
    merged_parameters: RefCell<HashMap<u32, Vec<NodeId>>>,
    objects: RefCell<HashMap<SourceKey, Weak<TypeCell>>>,
    object_targets: RefCell<HashMap<u32, Weak<TypeCell>>>,
    aliases: RefCell<HashMap<AliasInstanceKey, Weak<TypeCell>>>,
}

struct Invocation<'a> {
    state: &'a State,
    index: usize,
    pushed: bool,
    old_depth: usize,
}

impl Drop for Invocation<'_> {
    fn drop(&mut self) {
        self.state.depth.set(self.old_depth);
        if self.pushed {
            let mut frames = self.state.frames.borrow_mut();
            frames[self.index].types.clear();
            frames[self.index].mapper = None;
            self.state.active.set(self.index);
        }
    }
}

impl State {
    fn active_mapper(&self, mapper: &Mapper) -> Option<usize> {
        self.frames.borrow()[..self.active.get()]
            .iter()
            .rposition(|frame| {
                frame
                    .mapper
                    .as_ref()
                    .is_some_and(|active| active.same(mapper))
            })
    }

    fn enter(&self, mapper: &Mapper, index: Option<usize>) -> Invocation<'_> {
        let pushed = index.is_none();
        let index = index.unwrap_or_else(|| {
            let index = self.active.get();
            let mut frames = self.frames.borrow_mut();
            if index == frames.len() {
                frames.push(Frame {
                    mapper: None,
                    types: HashMap::new(),
                });
            }
            frames[index].mapper = Some(mapper.clone());
            debug_assert!(frames[index].types.is_empty());
            self.active.set(index + 1);
            index
        });
        let old_depth = self.depth.replace(self.depth.get() + 1);
        Invocation {
            state: self,
            index,
            pushed,
            old_depth,
        }
    }
}

impl Construction {
    // port: tsc/internal/checker/checker.go:Checker.getTypeAliasInstantiation
    pub(super) fn instantiate_alias_reference(
        self: &Rc<Self>,
        declaration: NodeId,
        source: &Rc<TypeCell>,
        arguments: &[Rc<TypeCell>],
        environment: &Environment,
        alias: Option<Alias>,
    ) -> Result<Rc<TypeCell>, Error> {
        let key = (
            declaration,
            arguments.iter().map(|ty| ty.id()).collect(),
            alias.as_ref().map(Alias::key).transpose()?,
        );
        if let Some(cached) = self.instantiation.aliases.borrow().get(&key) {
            return cached.upgrade().ok_or(Error::Released);
        }
        let parameters = self
            .list(
                declaration,
                self.input.node(declaration)?.type_parameter_list(),
            )?
            .into_iter()
            .map(|parameter| self.type_parameter(parameter))
            .collect::<Result<Vec<_>, _>>()?;
        let result = if alias.is_none() && same_types(arguments, &parameters) {
            source.clone()
        } else {
            self.instantiate(
                source,
                environment.1.as_ref().ok_or(Error::ResolutionFailed)?,
                alias,
            )?
        };
        self.instantiation
            .aliases
            .borrow_mut()
            .insert(key, Rc::downgrade(&result));
        Ok(result)
    }

    pub(super) fn record_type_source(&self, ty: &TypeCell, source: TypeSource) {
        if ty.alias.is_some() {
            if let Some(alias) = &source.alias {
                ty.alias_arguments.get_or_init(|| alias.arguments.clone());
            }
        }
        self.instantiation
            .contains_variables
            .borrow_mut()
            .remove(&ty.id());
        // Interned primitives and already named types may be encountered again
        // from a use site; that use cannot replace their declaration identity.
        self.instantiation
            .sources
            .borrow_mut()
            .entry(ty.id())
            .or_insert(source);
    }

    pub(super) fn record_type_parameter(&self, ty: &TypeCell, declaration: NodeId) {
        self.instantiation
            .parameters
            .borrow_mut()
            .insert(ty.id(), declaration);
    }

    pub(super) fn record_merged_type_parameter(&self, ty: &TypeCell, declaration: NodeId) {
        let mut merged = self.instantiation.merged_parameters.borrow_mut();
        let declarations = merged.entry(ty.id()).or_default();
        if !declarations.contains(&declaration) {
            declarations.push(declaration);
        }
    }

    pub(super) fn reset_instantiation_count(&self) {
        self.instantiation.statement_count.set(0);
    }

    /// `instantiateTypes(typeParameters, newSimpleTypeMapper(source, marker))`.
    pub(super) fn marker_arguments(
        self: &Rc<Self>,
        parameters: &[Rc<TypeCell>],
        source: &Rc<TypeCell>,
        marker: &Rc<TypeCell>,
    ) -> Result<Vec<Rc<TypeCell>>, Error> {
        let mapper = Mapper::with_types(
            Environment::default(),
            std::slice::from_ref(source),
            std::slice::from_ref(marker),
        )?;
        parameters
            .iter()
            .map(|parameter| self.instantiate(parameter, &mapper, None))
            .collect()
    }

    // port: tsc/internal/checker/checker.go:Checker.instantiateTypeWithAlias
    pub(super) fn instantiate(
        self: &Rc<Self>,
        ty: &Rc<TypeCell>,
        mapper: &Mapper,
        alias: Option<Alias>,
    ) -> Result<Rc<TypeCell>, Error> {
        let source_alias = self
            .instantiation
            .sources
            .borrow()
            .get(&ty.id())
            .and_then(|source| source.alias.clone());
        let contains_variables = self.could_contain_type_variables(ty)?;
        // couldContainTypeVariables also holds when the alias type arguments
        // could contain one, so instantiating a generic alias's body is real
        // work even when the body itself is free of type variables.
        let alias_variables = if contains_variables {
            false
        } else {
            let arguments = ty
                .alias_arguments
                .get()
                .cloned()
                .or_else(|| source_alias.as_ref().map(|alias| alias.arguments.clone()))
                .unwrap_or_default();
            let mut contains = false;
            for argument in &arguments {
                if self.could_contain_type_variables(&argument.upgrade().ok_or(Error::Released)?)? {
                    contains = true;
                    break;
                }
            }
            contains
        };
        if !contains_variables && !alias_variables {
            return Ok(ty.clone());
        }
        let state = &self.instantiation;
        if state.depth.get() == 100 || state.statement_count.get() >= 5_000_000 {
            return self.instantiation_limit(ty);
        }
        let key = CacheKey {
            source: ty.id(),
            alias: alias.as_ref().map(Alias::key).transpose()?,
        };
        let index = state.active_mapper(mapper);
        if let Some(index) = index {
            if let Some(cached) = state.frames.borrow()[index].types.get(&key) {
                return cached.upgrade().ok_or(Error::Released);
            }
        }
        let invocation = state.enter(mapper, index);
        self.instantiations.set(self.instantiations.get() + 1);
        state.statement_count.set(state.statement_count.get() + 1);
        let result = self.instantiate_worker(ty, mapper, alias, source_alias)?;
        if !invocation.pushed {
            state.frames.borrow_mut()[invocation.index]
                .types
                .insert(key, Rc::downgrade(&result));
        }
        Ok(result)
    }

    // port: tsc/internal/checker/checker.go:Checker.couldContainTypeVariablesWorker
    pub(super) fn could_contain_type_variables(&self, ty: &Rc<TypeCell>) -> Result<bool, Error> {
        if ty.flags & tf::STRUCTURED_OR_INSTANTIABLE == 0 {
            return Ok(false);
        }
        if let Some(&result) = self.instantiation.contains_variables.borrow().get(&ty.id()) {
            return Ok(result);
        }
        let result = if ty.flags & tf::INSTANTIABLE != 0 {
            true
        } else if self.non_generic_top_level_type(ty)? {
            false
        } else if ty.flags & tf::OBJECT != 0 {
            const REVERSE_MAPPED: u32 = 1 << 10;
            const OBJECT_REST: u32 = 1 << 23;
            const INSTANTIATION_EXPRESSION: u32 = 1 << 24;
            if ty.object_flags
                & (of::MAPPED | REVERSE_MAPPED | OBJECT_REST | INSTANTIATION_EXPRESSION)
                != 0
            {
                true
            } else if ty.object_flags & of::REFERENCE != 0 {
                if ty
                    .reference_shape()
                    .is_some_and(|reference| reference.deferred_node().is_some())
                {
                    self.instantiation
                        .contains_variables
                        .borrow_mut()
                        .insert(ty.id(), true);
                    return Ok(true);
                }
                let arguments = if let Some(reference) = ty.reference_shape() {
                    reference.arguments()?
                } else if let Some(tuple) = ty.tuple_shape() {
                    tuple.elements()?
                } else if let Some(target) = ty.generic_target() {
                    target.parameters()?
                }
                // A non-generic interface may still be a reference because it
                // has an implicit this type; its argument vector is empty.
                else if ty.symbol.is_some() {
                    Vec::new()
                } else {
                    return missing("reference type-variable containment without arguments");
                };
                self.some_type_variables(&arguments)?
            } else if ty.object_flags & of::ANONYMOUS != 0 {
                // A synthetic empty object has no declaration and is not a
                // potential generic object merely because it is anonymous.
                let source = self.instantiation.sources.borrow().get(&ty.id()).cloned();
                if ty.symbol.is_none() {
                    false
                } else if let Some(source) = source {
                    matches!(
                        self.input.node(source.node)?.kind().known(),
                        Some(
                            K::TypeLiteral
                                | K::FunctionType
                                | K::ConstructorType
                                | K::FunctionDeclaration
                                | K::FunctionExpression
                                | K::ArrowFunction
                                | K::MethodDeclaration
                                | K::MethodSignature
                                | K::ClassDeclaration
                                | K::ClassExpression
                                | K::ObjectLiteralExpression
                        )
                    )
                } else {
                    false
                }
            } else {
                false
            }
        } else if ty.flags & tf::UNION_OR_INTERSECTION != 0 && ty.flags & tf::ENUM_LITERAL == 0 {
            self.some_type_variables(&ty.types()?)?
        } else {
            false
        };
        self.instantiation
            .contains_variables
            .borrow_mut()
            .insert(ty.id(), result);
        Ok(result)
    }

    fn some_type_variables(&self, types: &[Rc<TypeCell>]) -> Result<bool, Error> {
        for ty in types {
            if self.could_contain_type_variables(ty)? {
                return Ok(true);
            }
        }
        Ok(false)
    }

    // port: tsc/internal/checker/checker.go:Checker.isNonGenericTopLevelType
    fn non_generic_top_level_type(&self, ty: &TypeCell) -> Result<bool, Error> {
        let alias = self
            .instantiation
            .sources
            .borrow()
            .get(&ty.id())
            .and_then(|source| source.alias.clone());
        let Some(alias) = alias else {
            return Ok(false);
        };
        if !alias.arguments.is_empty() {
            return Ok(false);
        }
        let declaration = self.input.node(alias.declaration)?;
        if !matches!(
            declaration.kind().known(),
            Some(K::TypeAliasDeclaration | K::JSTypeAliasDeclaration)
        ) {
            return Ok(false);
        }
        let mut parent = declaration.parent();
        while let Some(node) = parent {
            let read = self.input.node(node)?;
            match read.kind().known() {
                Some(K::SourceFile) => return Ok(true),
                Some(K::ModuleDeclaration) => parent = read.parent(),
                _ => return Ok(false),
            }
        }
        Ok(false)
    }

    pub(super) fn map_type(
        self: &Rc<Self>,
        ty: &Rc<TypeCell>,
        mapper: &Mapper,
    ) -> Result<Rc<TypeCell>, Error> {
        if let Some(functional) = mapper.0.functional {
            if ty.flags & tf::TYPE_PARAMETER == 0 {
                return Ok(ty.clone());
            }
            return match functional {
                FunctionalMapper::Permissive => self
                    .initialization
                    .named
                    .borrow()
                    .get("wildcardType")
                    .cloned()
                    .ok_or(Error::ResolutionFailed),
                FunctionalMapper::Restrictive => self.restrictive_type_parameter(ty),
            };
        }
        if let Some((first, second)) = &mapper.0.composite {
            let mapped = self.map_type(ty, first)?;
            return if mapper.0.merged || Rc::ptr_eq(ty, &mapped) {
                self.map_type(&mapped, second)
            } else {
                self.instantiate(&mapped, second, None)
            };
        }
        for (source, mapped) in mapper.0.types.iter().rev() {
            if Rc::ptr_eq(&source.upgrade().ok_or(Error::Released)?, ty) {
                return mapped.upgrade().ok_or(Error::Released);
            }
        }
        let parameter = self
            .instantiation
            .parameters
            .borrow()
            .get(&ty.id())
            .copied();
        if let Some(parameter) = parameter {
            if let Some(mapped) = mapper.0.environment.get(parameter)? {
                return Ok(mapped);
            }
            let merged = self
                .instantiation
                .merged_parameters
                .borrow()
                .get(&ty.id())
                .cloned()
                .unwrap_or_default();
            for declaration in merged {
                if let Some(mapped) = mapper.0.environment.get(declaration)? {
                    return Ok(mapped);
                }
            }
        }
        Ok(ty.clone())
    }

    fn instantiate_types(
        self: &Rc<Self>,
        types: &[Rc<TypeCell>],
        mapper: &Mapper,
    ) -> Result<Vec<Rc<TypeCell>>, Error> {
        types
            .iter()
            .map(|ty| self.instantiate(ty, mapper, None))
            .collect()
    }

    pub(super) fn instantiate_alias(
        self: &Rc<Self>,
        alias: Option<Alias>,
        mapper: &Mapper,
    ) -> Result<Option<Alias>, Error> {
        alias
            .map(|mut alias| {
                alias.arguments = alias
                    .arguments
                    .iter()
                    .map(|argument| {
                        let argument = argument.upgrade().ok_or(Error::Released)?;
                        Ok(Rc::downgrade(&self.instantiate(&argument, mapper, None)?))
                    })
                    .collect::<Result<_, Error>>()?;
                Ok(alias)
            })
            .transpose()
    }

    // port: tsc/internal/checker/checker.go:Checker.instantiateTypeWorker
    fn instantiate_worker(
        self: &Rc<Self>,
        ty: &Rc<TypeCell>,
        mapper: &Mapper,
        alias: Option<Alias>,
        source_alias: Option<Alias>,
    ) -> Result<Rc<TypeCell>, Error> {
        if ty.flags & tf::TYPE_PARAMETER != 0 {
            return self.map_type(ty, mapper);
        }
        if ty.flags & tf::OBJECT != 0 {
            if ty.object_flags & of::MAPPED != 0 {
                return self.instantiate_mapped(ty, mapper, alias);
            }
            if let Some(reference) = ty.reference_shape() {
                if reference.deferred_node().is_some() {
                    return self.instantiate_source_type(ty, mapper, alias, source_alias);
                }
                let arguments = reference.arguments()?;
                let mapped = self.instantiate_types(&arguments, mapper)?;
                if same_types(&mapped, &arguments) {
                    return Ok(ty.clone());
                }
                let target = reference.target()?;
                let generic = target.generic_target().ok_or_else(|| {
                    Error::Unsupported("reference target has no instantiator".into())
                })?;
                return generic.instantiate(&self.checker, &target, &mapped);
            }
            if ty.object_flags & (of::REFERENCE | of::ANONYMOUS | of::MAPPED) != 0 {
                return self.instantiate_source_type(ty, mapper, alias, source_alias);
            }
            return Ok(ty.clone());
        }
        if ty.flags & tf::UNION_OR_INTERSECTION != 0 {
            let types = ty.types()?;
            let mapped = self.instantiate_types(&types, mapper)?;
            if same_types(&mapped, &types)
                && alias.as_ref().map(|a| a.declaration)
                    == source_alias.as_ref().map(|a| a.declaration)
            {
                return Ok(ty.clone());
            }
            let alias = if alias.is_some() {
                alias
            } else {
                self.instantiate_alias(source_alias, mapper)?
            };
            return self.instantiated_compound(ty, mapped, alias);
        }
        if ty.flags & tf::INDEX != 0 {
            let source = self
                .instantiation
                .sources
                .borrow()
                .get(&ty.id())
                .cloned()
                .ok_or_else(|| Error::Unsupported("index type has no source".into()))?;
            let target = ty
                .types()?
                .into_iter()
                .next()
                .ok_or(Error::ResolutionFailed)?;
            return self.keyof(source.node, self.instantiate(&target, mapper, None)?);
        }
        if ty.flags & tf::INDEXED_ACCESS != 0 {
            let types = ty.types()?;
            if types.len() != 2 {
                return missing("indexed access operands");
            }
            // The result alias has a separate argument instantiation even when
            // a property lookup returns an already existing cell.
            let alias = if alias.is_some() {
                alias
            } else {
                self.instantiate_alias(source_alias, mapper)?
            };
            return self.indexed_access_with_alias(
                self.instantiate(&types[0], mapper, None)?,
                self.instantiate(&types[1], mapper, None)?,
                alias,
            );
        }
        if ty.flags & tf::TEMPLATE_LITERAL != 0 {
            let template = ty.template_parts().ok_or(Error::ResolutionFailed)?;
            return self.checker.graph.template_literal(
                &template.texts,
                &self.instantiate_types(&template.types()?, mapper)?,
            );
        }
        if ty.flags & tf::CONDITIONAL != 0 {
            return self.instantiate_conditional(ty, mapper, alias);
        }
        if ty.flags & (tf::STRING_MAPPING | tf::SUBSTITUTION) != 0 {
            return missing("string mapping/substitution instantiation");
        }
        Ok(ty.clone())
    }

    fn instantiated_compound(
        self: &Rc<Self>,
        source: &Rc<TypeCell>,
        types: Vec<Rc<TypeCell>>,
        alias: Option<Alias>,
    ) -> Result<Rc<TypeCell>, Error> {
        let kind = source.flags & tf::UNION_OR_INTERSECTION;
        if kind == tf::INTERSECTION {
            // getIntersectionType's absorbing reductions: `any` and `never`
            // absorb the intersection, `unknown` drops out, duplicates collapse
            // and one remaining constituent is the result. A result that needs
            // a new intersection type (with its ordering, primitive disjointness
            // and union distribution rules) stays outside the reference.
            let mut flat = Vec::new();
            for ty in types {
                if ty.flags & tf::INTERSECTION != 0 {
                    flat.extend(ty.types()?);
                } else {
                    flat.push(ty);
                }
            }
            if let Some(never) = flat.iter().find(|ty| ty.flags & tf::NEVER != 0) {
                return Ok(never.clone());
            }
            if let Some(any) = flat.iter().find(|ty| ty.flags & tf::ANY != 0) {
                return Ok(any.clone());
            }
            let mut set: Vec<Rc<TypeCell>> = Vec::new();
            for ty in flat {
                if ty.flags & tf::UNKNOWN == 0 && !set.iter().any(|seen| Rc::ptr_eq(seen, &ty)) {
                    set.push(ty);
                }
            }
            if set.len() == 1 {
                return Ok(set.remove(0));
            }
            return self.intersection(&set, source.name.clone(), alias);
        }
        let Some(alias) = alias else {
            return self.checker.graph.union(&types);
        };
        let key = alias.key()?;
        let result = self.checker.graph.union_named_arguments(
            &types,
            self.identity(alias.declaration),
            &alias.name,
            &key.arguments,
        )?;
        if result.flags & tf::UNION == 0 {
            return Ok(result);
        }
        let source = self
            .instantiation
            .sources
            .borrow()
            .get(&source.id())
            .cloned();
        if let Some(mut source) = source {
            source.alias = Some(alias);
            self.record_type_source(&result, source);
        }
        Ok(result)
    }

    pub(super) fn instantiate_source_type(
        self: &Rc<Self>,
        ty: &Rc<TypeCell>,
        mapper: &Mapper,
        alias: Option<Alias>,
        source_alias: Option<Alias>,
    ) -> Result<Rc<TypeCell>, Error> {
        let source = self
            .instantiation
            .sources
            .borrow()
            .get(&ty.id())
            .cloned()
            .ok_or_else(|| {
                Error::Unsupported("instantiable type has no syntax provenance".into())
            })?;
        let outer = self.outer_instantiation_environment(ty, &source)?;
        if outer.0.is_empty() && outer.2.is_empty() {
            return Ok(ty.clone());
        }
        let alias = if alias.is_some() {
            alias
        } else {
            self.instantiate_alias(source_alias, mapper)?
        };
        let mut environment = outer.clone();
        for (_, value) in &mut environment.0 {
            let original = value.upgrade().ok_or(Error::Released)?;
            // A canonical parameter is mapped directly. A previously mapped
            // value needs recursive substitution (CompositeTypeMapper.Map).
            let mapped = if original.flags & tf::TYPE_PARAMETER != 0 {
                self.map_type(&original, mapper)?
            } else {
                self.instantiate(&original, mapper, None)?
            };
            *value = Rc::downgrade(&mapped);
        }
        for (_, value) in &mut environment.2 {
            let original = value.upgrade().ok_or(Error::Released)?;
            let mapped = if original.flags & tf::TYPE_PARAMETER != 0 {
                self.map_type(&original, mapper)?
            } else {
                self.instantiate(&original, mapper, None)?
            };
            *value = Rc::downgrade(&mapped);
        }
        environment.1 = Some(Mapper::new(environment.clone()));
        let target = self
            .instantiation
            .object_targets
            .borrow()
            .get(&ty.id())
            .map(|target| target.upgrade().ok_or(Error::Released))
            .transpose()?
            .unwrap_or_else(|| ty.clone());
        let target_source = self
            .instantiation
            .sources
            .borrow()
            .get(&target.id())
            .cloned()
            .ok_or(Error::ResolutionFailed)?;
        let target_outer = self.outer_instantiation_environment(&target, &target_source)?;
        let original_key = SourceKey {
            source: target.id(),
            arguments: target_outer.key()?,
            alias: target_source.alias.as_ref().map(Alias::key).transpose()?,
        };
        let key = SourceKey {
            source: target.id(),
            arguments: environment.key()?,
            alias: alias.as_ref().map(Alias::key).transpose()?,
        };
        if key == original_key {
            return Ok(target);
        }
        if let Some(result) = self.instantiation.objects.borrow().get(&key) {
            return result.upgrade().ok_or(Error::Released);
        }
        let result =
            self.instantiate_source(&target, &target_source, environment.clone(), alias.clone())?;
        self.instantiation
            .objects
            .borrow_mut()
            .insert(key, Rc::downgrade(&result));
        if !Rc::ptr_eq(&result, &target) {
            self.instantiation
                .object_targets
                .borrow_mut()
                .insert(result.id(), Rc::downgrade(&target));
        }
        let mut contains = environment
            .0
            .iter()
            .map(|(_, ty)| ty.upgrade().ok_or(Error::Released))
            .collect::<Result<Vec<_>, _>>()?;
        contains.extend(
            environment
                .2
                .iter()
                .map(|(_, ty)| ty.upgrade().ok_or(Error::Released))
                .collect::<Result<Vec<_>, _>>()?,
        );
        if result.flags & tf::OBJECT != 0
            && result.object_flags & (of::MAPPED | of::ANONYMOUS | of::REFERENCE) != 0
        {
            let contains = self.some_type_variables(&contains)?;
            self.instantiation
                .contains_variables
                .borrow_mut()
                .insert(result.id(), contains);
        }
        Ok(result)
    }

    fn outer_instantiation_environment(
        &self,
        ty: &TypeCell,
        source: &TypeSource,
    ) -> Result<Environment, Error> {
        let mut outer = source.environment.clone();
        if source
            .alias
            .as_ref()
            .is_some_and(|alias| !alias.arguments.is_empty())
        {
            return Ok(outer);
        }
        let kind = self.input.node(source.node)?.kind();
        if ty.flags & tf::OBJECT != 0
            && (ty.object_flags & of::REFERENCE != 0
                // `SymbolFlagsTypeLiteral` is what the binder gives a type
                // literal, a function or constructor type and a mapped type;
                // `SymbolFlagsMethod` covers both method forms.
                || matches!(
                    kind.known(),
                    Some(
                        K::TypeLiteral
                            | K::FunctionType
                            | K::ConstructorType
                            | K::MappedType
                            | K::MethodDeclaration
                            | K::MethodSignature
                    )
                ))
        {
            let mut retained = Vec::with_capacity(outer.0.len());
            for (parameter, value) in outer.0 {
                if self.parameter_possibly_referenced(parameter, source.node)? {
                    retained.push((parameter, value));
                }
            }
            outer.0 = retained;
            let mut retained = Vec::with_capacity(outer.2.len());
            for (parameter, value) in outer.2 {
                let parameter_cell = parameter.upgrade().ok_or(Error::Released)?;
                let declaration = self
                    .instantiation
                    .parameters
                    .borrow()
                    .get(&parameter_cell.id())
                    .copied();
                let keep = match declaration {
                    Some(declaration) => {
                        self.parameter_possibly_referenced(declaration, source.node)?
                    }
                    None => true,
                };
                if keep {
                    retained.push((parameter, value));
                }
            }
            outer.2 = retained;
        }
        Ok(outer)
    }

    // port: tsc/internal/checker/checker.go:Checker.isTypeParameterPossiblyReferenced
    pub(super) fn parameter_possibly_referenced(
        &self,
        parameter: NodeId,
        node: NodeId,
    ) -> Result<bool, Error> {
        let parameter_read = self.input.node(parameter)?;
        let is_this = matches!(
            parameter_read.kind().known(),
            Some(K::InterfaceDeclaration | K::ClassDeclaration | K::ClassExpression)
        );
        let container = if is_this {
            Some(parameter)
        } else {
            parameter_read.parent()
        };
        // A type parameter whose symbol does not have exactly one declaration
        // is always possibly referenced. Class and interface type parameters
        // (and their `this` type) live in the merged symbol's members, so every
        // merged declaration contributes one.
        let owner = container.filter(|&owner| {
            self.input.node(owner).is_ok_and(|read| {
                matches!(
                    read.kind().known(),
                    Some(K::InterfaceDeclaration | K::ClassDeclaration)
                )
            })
        });
        if let Some(owner) = owner {
            if let Some(name) = self.input.node(owner)?.name() {
                if let Some(group) = self.input.resolve_type_name(name)? {
                    let mut count = 0;
                    for declaration in self.type_declaration_nodes(&group)? {
                        if is_this {
                            count += 1;
                            continue;
                        }
                        let read = self.input.node(declaration)?;
                        for candidate in self.list(declaration, read.type_parameter_list())? {
                            if self.text(candidate_name(self, candidate)?)?
                                == self.text(candidate_name(self, parameter)?)?
                            {
                                count += 1;
                            }
                        }
                    }
                    if count != 1 {
                        return Ok(true);
                    }
                }
            }
        }
        let mut current = Some(node);
        while current != container {
            let Some(current_node) = current else {
                return Ok(true);
            };
            let read = self.input.node(current_node)?;
            if read.kind() == K::Block {
                return Ok(true);
            }
            if let Some(conditional) = read.data_source().as_conditional_type_node() {
                if let Some(extends) = conditional.extends_type() {
                    if self.contains_parameter_reference(parameter, extends)? {
                        return Ok(true);
                    }
                }
            }
            current = read.parent();
        }
        self.contains_parameter_reference(parameter, node)
    }

    fn contains_parameter_reference(&self, parameter: NodeId, root: NodeId) -> Result<bool, Error> {
        let _ = candidate_name;
        let is_this = matches!(
            self.input.node(parameter)?.kind().known(),
            Some(K::InterfaceDeclaration | K::ClassDeclaration | K::ClassExpression)
        );
        let mut pending = vec![root];
        while let Some(node) = pending.pop() {
            let read = self.input.node(node)?;
            match read.kind().known() {
                Some(K::ThisType) if is_this => return Ok(true),
                Some(K::TypeReference)
                    if !is_this && self.list(node, read.type_argument_list())?.is_empty() =>
                {
                    let name = read
                        .data_source()
                        .as_type_reference_node()
                        .and_then(|data| data.type_name())
                        .ok_or(Error::ResolutionFailed)?;
                    if let Some(symbol) = self.input.resolve_type_name(name)? {
                        if self.declarations(&symbol)?.contains(&parameter) {
                            return Ok(true);
                        }
                    }
                }
                Some(K::TypeQuery) => {
                    let mut first = read
                        .data_source()
                        .as_type_query_node()
                        .and_then(|data| data.expr_name())
                        .ok_or(Error::ResolutionFailed)?;
                    loop {
                        let first_read = self.input.node(first)?;
                        let left = first_read
                            .data_source()
                            .as_qualified_name()
                            .and_then(|data| data.left());
                        if let Some(left) = left {
                            first = left;
                        } else {
                            break;
                        }
                    }
                    if self.input.node(first)?.kind() == K::ThisKeyword
                        || self.text(first)? == b"this"
                    {
                        return Ok(true);
                    }
                    let scope = if is_this {
                        parameter
                    } else {
                        self.input
                            .node(parameter)?
                            .parent()
                            .ok_or(Error::ResolutionFailed)?
                    };
                    let Some(symbol) = self.input.resolve_value_name(first)? else {
                        return Ok(true);
                    };
                    for declaration in self.declarations(&symbol)? {
                        let mut current = Some(declaration);
                        while let Some(node) = current {
                            if node == scope {
                                return Ok(true);
                            }
                            current = self.input.node(node)?.parent();
                        }
                    }
                    pending.extend(self.list(node, read.type_argument_list())?);
                    continue;
                }
                Some(K::MethodDeclaration | K::MethodSignature) => {
                    let return_type = read.type_node();
                    if return_type.is_none() && read.body().is_some() {
                        return Ok(true);
                    }
                    pending.extend(self.list(node, read.type_parameter_list())?);
                    pending.extend(self.list(node, read.parameter_list())?);
                    pending.extend(return_type);
                    continue;
                }
                _ => {}
            }
            let mut children = SyntaxChildren::default();
            let _ = read.for_each_child(&mut children);
            pending.extend(children.nodes);
            for list in children.lists {
                pending.extend(self.list(node, Some(list))?);
            }
            for slice in children.slices {
                pending.extend(self.input.nodes(node, slice)?.iter().flatten());
            }
        }
        Ok(false)
    }
}

#[derive(Default)]
struct SyntaxChildren {
    nodes: Vec<NodeId>,
    lists: Vec<NodeListId>,
    slices: Vec<tsr_ast::NodeSlice>,
}

impl tsr_ast::ChildVisitor for SyntaxChildren {
    fn visit_node(&mut self, node: NodeId) -> std::ops::ControlFlow<()> {
        self.nodes.push(node);
        std::ops::ControlFlow::Continue(())
    }
    fn visit_list(&mut self, list: NodeListId) -> std::ops::ControlFlow<()> {
        self.lists.push(list);
        std::ops::ControlFlow::Continue(())
    }
    fn visit_node_slice(&mut self, slice: tsr_ast::NodeSlice) -> std::ops::ControlFlow<()> {
        self.slices.push(slice);
        std::ops::ControlFlow::Continue(())
    }
}

fn same_types(left: &[Rc<TypeCell>], right: &[Rc<TypeCell>]) -> bool {
    left.len() == right.len()
        && left
            .iter()
            .zip(right)
            .all(|(left, right)| Rc::ptr_eq(left, right))
}

/// The name node of a type parameter declaration.
fn candidate_name(state: &Construction, parameter: NodeId) -> Result<NodeId, Error> {
    state
        .input
        .node(parameter)?
        .name()
        .ok_or(Error::ResolutionFailed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bound::BoundChecker;
    use crate::bound_input::BoundInput;

    fn owner(source: &str) -> (BoundChecker, NodeId) {
        let file = tsr_binder::bind_parsed_file(tsr_parser::parse_source_file(
            tsr_jsstring::SourceText::from_loaded_bytes(source.as_bytes()),
            tsr_core::ScriptKind::TS,
            tsr_ast::SourceFileParseOptions {
                file_name: tsr_jsstring::JsString::from_bytes(b"/fixture.ts".as_slice()),
                ..Default::default()
            },
        ))
        .unwrap();
        let source_file = file.source();
        let input = BoundInput::new(
            vec![file],
            crate::bound_input::BoundInputOptions::default(),
            Vec::new(),
        )
        .unwrap();
        (BoundChecker::new(input).unwrap(), source_file)
    }

    #[test]
    fn workers_count_substitution_and_recursive_cache_hits_separately() {
        let (owner, _) = owner("type A = number;");
        let state = &owner.state;
        let parameter = state.checker.graph.type_parameter("synthetic", None);
        let number = state.builtin(tf::NUMBER).unwrap();
        let mapper = Mapper::with_types(
            Environment::default(),
            std::slice::from_ref(&parameter),
            std::slice::from_ref(&number),
        )
        .unwrap();
        let before = state.instantiations.get();
        assert!(Rc::ptr_eq(
            &state.instantiate(&number, &mapper, None).unwrap(),
            &number
        ));
        assert_eq!(state.instantiations.get(), before);
        for expected in [1, 2] {
            assert!(Rc::ptr_eq(
                &state.instantiate(&parameter, &mapper, None).unwrap(),
                &number
            ));
            assert_eq!(state.instantiations.get(), before + expected);
        }
        let _outer = state.instantiation.enter(&mapper, None);
        for _ in 0..2 {
            assert!(Rc::ptr_eq(
                &state.instantiate(&parameter, &mapper, None).unwrap(),
                &number
            ));
            assert_eq!(state.instantiations.get(), before + 3);
        }
    }

    #[test]
    fn mapper_cache_uses_identity_and_discards_top_level_entries() {
        let state = State::default();
        let first = Mapper::new(Environment::default());
        let same_values = Mapper::new(Environment::default());
        let graph = crate::Graph::new();
        let ty = graph.primitive(tf::NUMBER, "number");
        let key = CacheKey {
            source: ty.id(),
            alias: None,
        };
        let invocation = state.enter(&first, None);
        state.frames.borrow_mut()[invocation.index]
            .types
            .insert(key.clone(), Rc::downgrade(&ty));
        assert_eq!(state.active_mapper(&first), Some(0));
        assert_eq!(state.active_mapper(&first.clone()), Some(0));
        assert_eq!(state.active_mapper(&same_values), None);
        {
            let nested = state.enter(&first, Some(0));
            assert_eq!(state.depth.get(), 2);
            assert!(!nested.pushed);
        }
        assert_eq!(state.depth.get(), 1);
        assert!(state.frames.borrow()[0].types.contains_key(&key));
        drop(invocation);
        assert_eq!(state.depth.get(), 0);
        assert_eq!(state.active.get(), 0);
        assert!(state.frames.borrow()[0].types.is_empty());
        assert!(state.frames.borrow()[0].mapper.is_none());
    }

    #[test]
    fn mapper_frames_restore_after_unwinding() {
        let state = State::default();
        let mapper = Mapper::new(Environment::default());
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _frame = state.enter(&mapper, None);
            panic!("source instantiation failed");
        }));
        assert!(result.is_err());
        assert_eq!(state.depth.get(), 0);
        assert_eq!(state.active.get(), 0);
        assert!(state.frames.borrow()[0].mapper.is_none());
    }

    #[test]
    fn source_literal_creation_matches_frozen_native_lookup_deltas() {
        // These deltas come from supplemental-observations.json.xz, not from a
        // second Rust implementation: native before_lookup is 85 and the first
        // action starts at 94, 94 and 98 respectively. Missing libraries change
        // this test's initial count but not the literal/union constructor work.
        for (source, created) in [
            ("type A = 'a' | 'b' | 0 | 1 | true; type B = string | number | boolean;", 9),
            ("type A = '\\uD800' | '\\u{1F600}' | '\\0' | '\\uFEFF'; type B = string;", 9),
            ("type A = -0 | 1e21 | 0.0000001 | -42 | 9007199254740993n; type B = number | bigint;", 13),
        ] {
            let (owner, source_file) = owner(source);
            let before = owner.checker().graph.len();
            for name in [b"A", b"B"] {
                let declaration = owner.input().declaration_by_name(source_file, name).unwrap().unwrap();
                owner.declared_type(declaration).unwrap();
            }
            assert_eq!(owner.checker().graph.len() - before, created, "{source}");
        }
    }
}
