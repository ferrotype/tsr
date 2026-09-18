//! Reference-owned generic targets, argument maps and instantiated objects.

use super::{
    instantiate, missing, of, tf, Cell, Construction, Environment, Error, NodeId, Rc, Structure,
    SymbolGroup, TypeCell, TypeLink, Weak, K,
};
use std::cell::OnceCell;
use std::collections::HashSet;

impl Construction {
    // port: tsc/internal/checker/checker.go:Checker.isThislessInterface
    pub(super) fn interface_has_this(
        self: &Rc<Self>,
        declarations: &[NodeId],
    ) -> Result<bool, Error> {
        self.interface_has_this_worker(declarations, &mut HashSet::new())
    }

    fn interface_has_this_worker(
        self: &Rc<Self>,
        declarations: &[NodeId],
        visiting: &mut HashSet<NodeId>,
    ) -> Result<bool, Error> {
        for &declaration in declarations {
            let read = self.input.node(declaration)?;
            if read.kind() != K::InterfaceDeclaration {
                continue;
            }
            if read.flags() & ts_ast::node_flags::CONTAINS_THIS != 0
                || !self
                    .list(declaration, read.type_parameter_list())?
                    .is_empty()
            {
                return Ok(true);
            }
            if !visiting.insert(declaration) {
                return missing("interface recursively references itself as a base type");
            }
            for base in self.interface_base_nodes(declaration)? {
                let node = self.input.node(base)?;
                let name = node
                    .data_source()
                    .as_expression_with_type_arguments()
                    .and_then(|data| data.expression())
                    .or_else(|| {
                        node.data_source()
                            .as_type_reference_node()
                            .and_then(|data| data.type_name())
                    })
                    .ok_or(Error::ResolutionFailed)?;
                if !matches!(
                    self.input.node(name)?.kind().known(),
                    Some(K::Identifier | K::QualifiedName | K::PropertyAccessExpression)
                ) {
                    continue;
                }
                let Some(group) = self.input.resolve_type_name(name)? else {
                    return Ok(true);
                };
                if !group.symbols.iter().any(|&symbol| {
                    self.input
                        .symbol(symbol)
                        .is_ok_and(|symbol| symbol.flags() & ts_ast::symbol_flags::INTERFACE != 0)
                }) {
                    return Ok(true);
                }
                if self.interface_has_this_worker(&self.declarations(&group)?, visiting)? {
                    return Ok(true);
                }
            }
            visiting.remove(&declaration);
        }
        Ok(false)
    }

    fn interface_base_nodes(&self, declaration: NodeId) -> Result<Vec<NodeId>, Error> {
        let read = self.input.node(declaration)?;
        let Some(data) = read.data_source().as_interface_declaration() else {
            return Ok(Vec::new());
        };
        let mut bases = Vec::new();
        for clause in self.list(declaration, data.heritage_clauses())? {
            let node = self.input.node(clause)?;
            let data = node.data_source();
            let data = data.as_heritage_clause().ok_or(Error::ResolutionFailed)?;
            if data.token() == K::ExtendsKeyword {
                bases.extend(self.list(clause, data.types())?);
            }
        }
        Ok(bases)
    }

    pub(super) fn heritage_type(
        self: &Rc<Self>,
        node: NodeId,
        env: &Environment,
    ) -> Result<Rc<TypeCell>, Error> {
        let read = self.input.node(node)?;
        let data = read.data_source();
        let name = data
            .as_expression_with_type_arguments()
            .and_then(|data| data.expression())
            .or_else(|| {
                data.as_type_reference_node()
                    .and_then(|data| data.type_name())
            })
            .ok_or(Error::ResolutionFailed)?;
        let group = self
            .input
            .resolve_type_name(name)?
            .ok_or_else(|| Error::Unsupported("unresolved interface base".into()))?;
        let arguments = self
            .list(node, read.type_argument_list())?
            .into_iter()
            .map(|argument| self.type_node(argument, env, None))
            .collect::<Result<Vec<_>, _>>()?;
        self.type_reference(&group, &arguments, env)
    }

    // port: tsc/internal/checker/checker.go:Checker.resolveObjectTypeMembers
    // The caller keeps declared members first, then adds only unshadowed base
    // properties and index keys. Signatures retain declaration/base order.
    pub(super) fn inherited_members(
        self: &Rc<Self>,
        declarations: &[NodeId],
        env: &Environment,
    ) -> Result<Vec<Structure>, Error> {
        let mut inherited = Vec::new();
        let mut interface_name = None;
        for &declaration in declarations {
            let read = self.input.node(declaration)?;
            if read.kind() == K::InterfaceDeclaration {
                interface_name = Some(read.name().ok_or(Error::ResolutionFailed)?);
                break;
            }
        }
        let this_argument = interface_name
            .map(|name| {
                let group = self
                    .input
                    .resolve_type_name(name)?
                    .ok_or(Error::ResolutionFailed)?;
                let target = self.declared(&group)?;
                let marker = self.interface_this.borrow().get(&target.id()).cloned();
                marker
                    .map(|marker| match &env.1 {
                        Some(mapper) => self.map_type(&marker, mapper),
                        None => Ok(marker),
                    })
                    .transpose()
            })
            .transpose()?
            .flatten();
        for &declaration in declarations {
            for base in self.interface_base_nodes(declaration)? {
                let mut ty = self.heritage_type(base, env)?;
                if let Some(this) = &this_argument {
                    let (target, arguments) = match ty.reference_shape() {
                        Some(reference) => (reference.target()?, reference.arguments()?),
                        None => (ty.clone(), Vec::new()),
                    };
                    if self.interface_this.borrow().contains_key(&target.id()) {
                        let symbols = self
                            .declared_types
                            .borrow()
                            .iter()
                            .find(|(_, declared)| Rc::ptr_eq(declared, &target))
                            .map(|(symbols, _)| symbols.clone())
                            .ok_or(Error::ResolutionFailed)?;
                        let count = target
                            .generic_target()
                            .map(|generic| generic.parameters().map(|parameters| parameters.len()))
                            .transpose()?
                            .unwrap_or(0);
                        ty = self.instantiate_interface_with_this(
                            &SymbolGroup { symbols },
                            &target,
                            arguments.get(..count).ok_or(Error::ResolutionFailed)?,
                            this,
                        )?;
                    }
                }
                self.inherited_structure(&ty, &mut inherited)?;
            }
        }
        Ok(inherited)
    }

    fn inherited_structure(
        &self,
        ty: &Rc<TypeCell>,
        inherited: &mut Vec<Structure>,
    ) -> Result<(), Error> {
        if ty.flags & tf::ANY != 0 {
            let index = self
                .initialization
                .index_infos
                .get(1)
                .ok_or(Error::ResolutionFailed)?
                .clone();
            inherited.push(Structure {
                index_infos: vec![index],
                ..Structure::default()
            });
            return Ok(());
        }
        if ty.flags & tf::INTERSECTION != 0 {
            for part in ty.types()? {
                self.inherited_structure(&part, inherited)?;
            }
            return Ok(());
        }
        if ty.flags & tf::OBJECT == 0 {
            return missing("interface base must have statically known object members");
        }
        // TypeCell's resolution state detects a recursive base before this can
        // borrow unfinished members. Its failure propagates; no empty base is
        // substituted and no RefCell borrow survives another resolution.
        inherited.push(ty.structure(&self.checker.graph)?.clone());
        Ok(())
    }

    // port: tsc/internal/checker/checker.go:Checker.getDeclaredTypeOfTypeParameter
    /// The declaration that stands for a class or interface type parameter.
    /// Those parameters are members of the merged symbol, so the same-named
    /// parameter of every merged declaration is one symbol and one type; the
    /// first declaration's node represents it.
    fn canonical_type_parameter(&self, node: NodeId) -> Result<NodeId, Error> {
        let read = self.input.node(node)?;
        let Some(owner) = read.parent() else {
            return Ok(node);
        };
        let owner_read = self.input.node(owner)?;
        if !matches!(
            owner_read.kind().known(),
            Some(K::InterfaceDeclaration | K::ClassDeclaration)
        ) {
            return Ok(node);
        }
        let (Some(owner_name), Some(name)) = (owner_read.name(), read.name()) else {
            return Ok(node);
        };
        let Some(group) = self.input.resolve_type_name(owner_name)? else {
            return Ok(node);
        };
        let name = self.text(name)?;
        for declaration in self.type_declaration_nodes(&group)? {
            let declaration_read = self.input.node(declaration)?;
            for candidate in self.list(declaration, declaration_read.type_parameter_list())? {
                if let Some(candidate_name) = self.input.node(candidate)?.name() {
                    if self.text(candidate_name)? == name {
                        return Ok(candidate);
                    }
                }
            }
        }
        Ok(node)
    }

    pub(super) fn type_parameter(self: &Rc<Self>, node: NodeId) -> Result<Rc<TypeCell>, Error> {
        let key = (node, Default::default());
        if let Some(ty) = self.node_types.borrow().get(&key).cloned() {
            return Ok(ty);
        }
        let canonical = self.canonical_type_parameter(node)?;
        if canonical != node {
            let ty = self.type_parameter(canonical)?;
            self.record_merged_type_parameter(&ty, node);
            self.node_types.borrow_mut().insert(key, ty.clone());
            return Ok(ty);
        }
        let read = self.input.node(node)?;
        let name = self.name(
            read.name()
                .ok_or_else(|| Error::Unsupported("type parameter name".into()))?,
        )?;
        let ty = self.checker.graph.allocate_full(
            tf::TYPE_PARAMETER,
            0,
            name,
            None,
            None,
            None,
            false,
            Vec::new(),
            false,
            None,
        );
        self.node_types.borrow_mut().insert(key, ty.clone());
        let constraint = read
            .data_source()
            .as_type_parameter_declaration()
            .and_then(|data| data.constraint());
        let constraint = constraint.map(|node| {
            let weak = Rc::downgrade(self);
            TypeLink::lazy(move || {
                weak.upgrade().ok_or(Error::Released)?.type_node(
                    node,
                    &Environment::default(),
                    None,
                )
            })
        });
        ty.set_lazy_type_parameter(constraint, None, 0)?;
        self.record_type_parameter(&ty, node);
        self.reject_inferred_constraint(node)?;
        Ok(ty)
    }

    // port: tsc/internal/checker/checker.go:Checker.getInferredTypeParameterConstraint
    /// An `infer P` declaration in certain positions gains an implicit
    /// constraint: `unknown[]` in a rest position, `string` in a template span,
    /// `string | number | symbol` as a mapped type's parameter, and the
    /// referenced parameter's constraint inside a type reference. The reference
    /// does not model those, so it refuses where one would be produced rather
    /// than resolving the parameter unconstrained.
    fn reject_inferred_constraint(self: &Rc<Self>, node: NodeId) -> Result<(), Error> {
        let read = self.input.node(node)?;
        let Some(infer) = read.parent() else {
            return Ok(());
        };
        if self.input.node(infer)?.kind() != K::InferType {
            return Ok(());
        }
        let mut child = infer;
        let mut parent = self.input.node(infer)?.parent();
        while let Some(node) = parent.filter(|&node| {
            self.input
                .node(node)
                .is_ok_and(|read| read.kind() == K::ParenthesizedType)
        }) {
            child = node;
            parent = self.input.node(node)?.parent();
        }
        let Some(parent) = parent else {
            return Ok(());
        };
        let parent_read = self.input.node(parent)?;
        let rest = match parent_read.kind().known() {
            Some(K::Parameter) => parent_read
                .data_source()
                .as_parameter_declaration()
                .and_then(|data| data.dot_dot_dot_token())
                .is_some(),
            Some(K::RestType) => true,
            Some(K::NamedTupleMember) => parent_read
                .data_source()
                .as_named_tuple_member()
                .and_then(|data| data.dot_dot_dot_token())
                .is_some(),
            _ => false,
        };
        if rest {
            return missing(
                "implicit unknown[] constraint of an infer parameter in a rest position",
            );
        }
        if matches!(
            parent_read.kind().known(),
            Some(K::TemplateLiteralTypeSpan | K::MappedType)
        ) || parent_read.kind() == K::TypeParameter
        {
            return missing(
                "implicit constraint of an infer parameter in a template or mapped type",
            );
        }
        if parent_read.kind() == K::TypeReference {
            // Only a referenced parameter that declares a constraint produces
            // one here; `Promise<infer U>` and the like do not.
            let name = parent_read
                .data_source()
                .as_type_reference_node()
                .and_then(|data| data.type_name())
                .ok_or(Error::ResolutionFailed)?;
            let Some(group) = self.input.resolve_type_name(name)? else {
                return Ok(());
            };
            let declarations = self.type_declaration_nodes(&group)?;
            let Some(&declaration) = declarations.first() else {
                return Ok(());
            };
            let parameters = self.list(
                declaration,
                self.input.node(declaration)?.type_parameter_list(),
            )?;
            let arguments = self.list(parent, parent_read.type_argument_list())?;
            if let Some(index) = arguments.iter().position(|&argument| argument == child) {
                if let Some(&parameter) = parameters.get(index) {
                    let declared = self
                        .input
                        .node(parameter)?
                        .data_source()
                        .as_type_parameter_declaration()
                        .and_then(|data| data.constraint());
                    if declared.is_some() {
                        return missing(
                            "implicit constraint of an infer parameter from a referenced parameter",
                        );
                    }
                }
            }
        }
        Ok(())
    }

    pub(super) fn parameters(
        self: &Rc<Self>,
        declaration: NodeId,
        outer: &Environment,
    ) -> Result<(Environment, Vec<Rc<TypeCell>>), Error> {
        let nodes = self.list(
            declaration,
            self.input.node(declaration)?.type_parameter_list(),
        )?;
        let mut env = outer.clone();
        let mut parameters = Vec::with_capacity(nodes.len());
        let mut originals = Vec::new();
        for node in nodes {
            let original = self.type_parameter(node)?;
            let ty = if outer.1.is_some() {
                let fresh = self.checker.graph.allocate_full(
                    tf::TYPE_PARAMETER,
                    0,
                    original.name.clone(),
                    original.symbol,
                    None,
                    None,
                    false,
                    Vec::new(),
                    false,
                    None,
                );
                self.record_type_parameter(&fresh, node);
                originals.push((node, original));
                fresh
            } else {
                original
            };
            env.0.push((node, Rc::downgrade(&ty)));
            parameters.push(ty);
        }
        if outer.1.is_some() {
            let mapper = instantiate::Mapper::new(env.clone());
            env.1 = Some(mapper.clone());
            for (fresh, (node, original)) in parameters.iter().zip(originals) {
                let state = Rc::downgrade(self);
                let original_link = Rc::downgrade(&original);
                let mapper = mapper.clone();
                let has_constraint = self
                    .input
                    .node(node)?
                    .data_source()
                    .as_type_parameter_declaration()
                    .and_then(|data| data.constraint())
                    .is_some();
                let constraint = has_constraint.then(|| {
                    TypeLink::lazy(move || {
                        let state = state.upgrade().ok_or(Error::Released)?;
                        let original = original_link.upgrade().ok_or(Error::Released)?;
                        match original
                            .type_parameter_shape()
                            .map(super::super::generics::TypeParameterShape::constraint)
                            .transpose()?
                            .flatten()
                        {
                            Some(constraint) => state.instantiate(&constraint, &mapper, None),
                            None => missing("declared generic constraint is unavailable"),
                        }
                    })
                });
                let modifiers = original
                    .type_parameter_shape()
                    .map_or(0, |shape| shape.modifiers);
                fresh.set_lazy_type_parameter(constraint, Some(&original), modifiers)?;
            }
        }
        Ok((env, parameters))
    }

    fn arguments_environment(
        self: &Rc<Self>,
        declaration: NodeId,
        arguments: &[Rc<TypeCell>],
        outer: &Environment,
    ) -> Result<Environment, Error> {
        let nodes = self.list(
            declaration,
            self.input.node(declaration)?.type_parameter_list(),
        )?;
        if arguments.len() > nodes.len() {
            return missing("too many type arguments");
        }
        let mut env = outer.clone();
        for (index, node) in nodes.into_iter().enumerate() {
            let argument = if let Some(argument) = arguments.get(index) {
                argument.clone()
            } else if let Some(default) = self
                .input
                .node(node)?
                .data_source()
                .as_type_parameter_declaration()
                .and_then(|data| data.default_type())
            {
                self.type_node(default, &env, None)?
            } else {
                return missing("missing required type argument");
            };
            env.0.push((node, Rc::downgrade(&argument)));
        }
        env.1 = Some(instantiate::Mapper::new(env.clone()));
        Ok(env)
    }

    // port: tsc/internal/checker/checker.go:Checker.getTypeFromTypeReference
    pub(super) fn source_reference(
        self: &Rc<Self>,
        node: NodeId,
        outer: &Environment,
        alias: Option<Rc<str>>,
    ) -> Result<Rc<TypeCell>, Error> {
        let alias = self.alias_metadata(node, outer, alias)?;
        self.source_reference_with_alias(node, outer, alias)
    }

    pub(super) fn source_reference_with_alias(
        self: &Rc<Self>,
        node: NodeId,
        outer: &Environment,
        source_alias: Option<instantiate::Alias>,
    ) -> Result<Rc<TypeCell>, Error> {
        let read = self.input.node(node)?;
        let name = read
            .data_source()
            .as_type_reference_node()
            .and_then(|data| data.type_name())
            .ok_or(Error::ResolutionFailed)?;
        let group = self
            .input
            .resolve_type_name(name)?
            .ok_or_else(|| Error::Unsupported("unresolved type reference".into()))?;
        let declarations = self.type_declaration_nodes(&group)?;
        let first = *declarations.first().ok_or(Error::ResolutionFailed)?;
        let argument_nodes = self.list(node, read.type_argument_list())?;
        let declaration = self.input.node(first)?;
        if declaration.kind() == K::TypeParameter {
            if !argument_nodes.is_empty() {
                return missing("type arguments on a type parameter");
            }
            return outer
                .get(first)?
                .map_or_else(|| self.type_parameter(first), Ok);
        }
        let parameters = self.list(first, declaration.type_parameter_list())?;
        if argument_nodes.len() > parameters.len() {
            return missing("too many type arguments");
        }
        for &parameter in &parameters[argument_nodes.len()..] {
            if self
                .input
                .node(parameter)?
                .data_source()
                .as_type_parameter_declaration()
                .and_then(|data| data.default_type())
                .is_none()
            {
                return missing("missing required type argument");
            }
        }
        if declaration.kind() == K::InterfaceDeclaration
            && !parameters.is_empty()
            && self.is_deferred_reference(node, argument_nodes.len() != parameters.len())?
        {
            let target = self.declared(&group)?;
            let declarations = declarations
                .into_iter()
                .filter(|&node| {
                    self.input
                        .node(node)
                        .is_ok_and(|read| read.kind() == K::InterfaceDeclaration)
                })
                .collect();
            // The syntax reference is a real cell before its arguments are
            // requested. Only weak argument edges survive the first lookup.
            let state = Rc::downgrade(self);
            let env = outer.clone();
            let values = Rc::new(OnceCell::<Result<Vec<Weak<TypeCell>>, Error>>::new());
            let resolving = Rc::new(Cell::new(false));
            let arguments = (0..parameters.len())
                .map(|index| {
                    let state = state.clone();
                    let env = env.clone();
                    let parameters = parameters.clone();
                    let argument_nodes = argument_nodes.clone();
                    let values = values.clone();
                    let resolving = resolving.clone();
                    TypeLink::lazy(move || {
                        if values.get().is_none() {
                            if resolving.replace(true) {
                                return missing("circular deferred type arguments");
                            }
                            struct Reset<'a>(&'a Cell<bool>);
                            impl Drop for Reset<'_> {
                                fn drop(&mut self) {
                                    self.0.set(false);
                                }
                            }
                            let _reset = Reset(&resolving);
                            let result = (|| {
                                let state = state.upgrade().ok_or(Error::Released)?;
                                let arguments = argument_nodes
                                    .iter()
                                    .map(|&argument| state.type_node(argument, &env, None))
                                    .collect::<Result<Vec<_>, _>>()?;
                                let complete =
                                    state.arguments_environment(first, &arguments, &env)?;
                                parameters
                                    .iter()
                                    .map(|&parameter| {
                                        complete
                                            .get(parameter)?
                                            .map(|ty| Rc::downgrade(&ty))
                                            .ok_or(Error::ResolutionFailed)
                                    })
                                    .collect::<Result<Vec<_>, Error>>()
                            })();
                            values.set(result).map_err(|_| Error::ResolutionFailed)?;
                        }
                        values
                            .get()
                            .ok_or(Error::ResolutionFailed)?
                            .as_ref()
                            .map_err(Clone::clone)?
                            .get(index)
                            .ok_or(Error::ResolutionFailed)?
                            .upgrade()
                            .ok_or(Error::Released)
                    })
                })
                .collect::<Vec<_>>();
            let instance = self.object_with_alias(
                declarations,
                outer.clone(),
                source_alias
                    .as_ref()
                    .map_or_else(|| target.name.clone(), |alias| alias.name.clone()),
                of::REFERENCE,
                source_alias
                    .as_ref()
                    .map(|alias| self.identity(alias.declaration)),
            )?;
            if let Some(readonly) = self.global_array_kind(&target) {
                instance.set_lazy_array_element(
                    arguments.first().cloned().ok_or(Error::ResolutionFailed)?,
                    readonly,
                )?;
            }
            instance.set_deferred_reference_shape(&target, arguments, node)?;
            return Ok(instance);
        }
        let declared = self.declared(&group)?;
        let arguments = argument_nodes
            .into_iter()
            .map(|argument| self.type_node(argument, outer, None))
            .collect::<Result<Vec<_>, _>>()?;
        if !parameters.is_empty()
            && matches!(
                declaration.kind().known(),
                Some(K::TypeAliasDeclaration | K::JSTypeAliasDeclaration)
            )
        {
            let mut requested_alias = source_alias;
            if let Some(candidate) = &requested_alias {
                if !self.local_type_alias(first)? && self.local_type_alias(candidate.declaration)? {
                    requested_alias = None;
                }
            }
            let environment = self.arguments_environment(first, &arguments, outer)?;
            return self.instantiate_alias_reference(
                first,
                &declared,
                &arguments,
                &environment,
                requested_alias,
            );
        }
        self.type_reference(&group, &arguments, outer)
    }

    fn local_type_alias(&self, declaration: NodeId) -> Result<bool, Error> {
        let mut parent = self.input.node(declaration)?.parent();
        while let Some(node) = parent {
            let read = self.input.node(node)?;
            if matches!(
                read.kind().known(),
                Some(
                    K::FunctionDeclaration
                        | K::FunctionExpression
                        | K::ArrowFunction
                        | K::MethodDeclaration
                        | K::GetAccessor
                        | K::SetAccessor
                        | K::Constructor
                )
            ) {
                return Ok(true);
            }
            parent = read.parent();
        }
        Ok(false)
    }

    // port: tsc/internal/checker/checker.go:Checker.isDeferredTypeReferenceNode
    pub(super) fn is_deferred_reference(
        &self,
        node: NodeId,
        has_default_arguments: bool,
    ) -> Result<bool, Error> {
        let mut parent = self.input.node(node)?.parent();
        while let Some(current) = parent {
            let read = self.input.node(current)?;
            if read.kind() == K::ParenthesizedType
                || read
                    .data_source()
                    .as_type_operator_node()
                    .is_some_and(|operator| operator.operator() == K::ReadonlyKeyword)
            {
                parent = read.parent();
            } else {
                break;
            }
        }
        if parent.is_some_and(|parent| {
            self.input.node(parent).is_ok_and(|read| {
                matches!(
                    read.kind().known(),
                    Some(K::TypeAliasDeclaration | K::JSTypeAliasDeclaration)
                )
            })
        }) {
            return Ok(true);
        }
        let mut current = node;
        loop {
            let Some(parent) = self.input.node(current)?.parent() else {
                return Ok(false);
            };
            let read = self.input.node(parent)?;
            match read.kind().known() {
                Some(K::TypeAliasDeclaration | K::JSTypeAliasDeclaration) => break,
                Some(
                    K::ParenthesizedType
                    | K::NamedTupleMember
                    | K::TypeReference
                    | K::UnionType
                    | K::IntersectionType
                    | K::IndexedAccessType
                    | K::ConditionalType
                    | K::TypeOperator
                    | K::ArrayType
                    | K::TupleType,
                ) => {
                    current = parent;
                }
                _ => return Ok(false),
            }
        }
        let read = self.input.node(node)?;
        let operands = match read.kind().known() {
            Some(K::TypeReference) => {
                if has_default_arguments {
                    return Ok(true);
                }
                self.list(node, read.type_argument_list())?
            }
            Some(K::ArrayType) => vec![read
                .data_source()
                .as_array_type_node()
                .and_then(|data| data.element_type())
                .ok_or(Error::ResolutionFailed)?],
            Some(K::TupleType) => self.list(
                node,
                read.data_source()
                    .as_tuple_type_node()
                    .and_then(|data| data.elements()),
            )?,
            _ => return missing("deferred reference syntax"),
        };
        for operand in operands {
            if self.may_resolve_type_alias(operand)? {
                return Ok(true);
            }
        }
        Ok(false)
    }

    // port: tsc/internal/checker/checker.go:Checker.mayResolveTypeAlias
    fn may_resolve_type_alias(&self, node: NodeId) -> Result<bool, Error> {
        let read = self.input.node(node)?;
        let data = read.data_source();
        let operands = match read.kind().known() {
            Some(K::TypeReference) => {
                let name = data
                    .as_type_reference_node()
                    .and_then(|data| data.type_name())
                    .ok_or(Error::ResolutionFailed)?;
                let group = self
                    .input
                    .resolve_type_name(name)?
                    .ok_or_else(|| Error::Unsupported("unresolved type reference".into()))?;
                return Ok(group.symbols.iter().any(|&symbol| {
                    self.input
                        .symbol(symbol)
                        .is_ok_and(|symbol| symbol.flags() & ts_ast::symbol_flags::TYPE_ALIAS != 0)
                }));
            }
            Some(K::TypeQuery) => return Ok(true),
            Some(K::TypeOperator)
                if data
                    .as_type_operator_node()
                    .is_some_and(|data| data.operator() == K::UniqueKeyword) =>
            {
                return Ok(false)
            }
            Some(
                K::TypeOperator | K::ParenthesizedType | K::OptionalType | K::NamedTupleMember,
            ) => {
                vec![read.type_node().ok_or(Error::ResolutionFailed)?]
            }
            Some(K::RestType) => {
                let operand = read.type_node().ok_or(Error::ResolutionFailed)?;
                let inner = self.input.node(operand)?;
                if inner.kind() != K::ArrayType {
                    return Ok(true);
                }
                vec![inner
                    .data_source()
                    .as_array_type_node()
                    .and_then(|data| data.element_type())
                    .ok_or(Error::ResolutionFailed)?]
            }
            Some(K::UnionType) => self.list(
                node,
                data.as_union_type_node().and_then(|data| data.types()),
            )?,
            Some(K::IntersectionType) => self.list(
                node,
                data.as_intersection_type_node()
                    .and_then(|data| data.types()),
            )?,
            Some(K::IndexedAccessType) => {
                let data = data
                    .as_indexed_access_type_node()
                    .ok_or(Error::ResolutionFailed)?;
                vec![
                    data.object_type().ok_or(Error::ResolutionFailed)?,
                    data.index_type().ok_or(Error::ResolutionFailed)?,
                ]
            }
            Some(K::ConditionalType) => {
                let data = data
                    .as_conditional_type_node()
                    .ok_or(Error::ResolutionFailed)?;
                vec![
                    data.check_type().ok_or(Error::ResolutionFailed)?,
                    data.extends_type().ok_or(Error::ResolutionFailed)?,
                    data.true_type().ok_or(Error::ResolutionFailed)?,
                    data.false_type().ok_or(Error::ResolutionFailed)?,
                ]
            }
            _ => return Ok(false),
        };
        for operand in operands {
            if self.may_resolve_type_alias(operand)? {
                return Ok(true);
            }
        }
        Ok(false)
    }

    pub(super) fn type_reference(
        self: &Rc<Self>,
        group: &SymbolGroup,
        arguments: &[Rc<TypeCell>],
        outer: &Environment,
    ) -> Result<Rc<TypeCell>, Error> {
        let declarations = self.type_declaration_nodes(group)?;
        let first = *declarations.first().ok_or(Error::ResolutionFailed)?;
        let read = self.input.node(first)?;
        if read.kind() == K::TypeParameter {
            if !arguments.is_empty() {
                return missing("type arguments on a type parameter");
            }
            return outer
                .get(first)?
                .map_or_else(|| self.type_parameter(first), Ok);
        }
        let parameters = self.list(first, read.type_parameter_list())?;
        if parameters.is_empty() {
            if !arguments.is_empty() {
                return missing("type arguments on non-generic declaration");
            }
            return self.declared(group);
        }
        let env = self.arguments_environment(first, arguments, outer)?;
        let key = (first, env.key()?);
        if let Some(ty) = self.instantiated_types.borrow().get(&key).cloned() {
            return Ok(ty);
        }
        let ty = if read.kind() == K::InterfaceDeclaration {
            let target = self.declared(group)?;
            self.instantiate_interface(group, &target, arguments)?
        } else if matches!(
            read.kind().known(),
            Some(K::TypeAliasDeclaration | K::JSTypeAliasDeclaration)
        ) {
            let target = self.declared(group)?;
            self.instantiate_alias_reference(first, &target, arguments, &env, None)?
        } else {
            return missing("generic declaration kind");
        };
        self.instantiated_types.borrow_mut().insert(key, ty.clone());
        Ok(ty)
    }

    // port: tsc/internal/checker/checker.go:Checker.createTypeReference
    pub(super) fn instantiate_interface(
        self: &Rc<Self>,
        group: &SymbolGroup,
        target: &Rc<TypeCell>,
        arguments: &[Rc<TypeCell>],
    ) -> Result<Rc<TypeCell>, Error> {
        self.instantiate_interface_worker(group, target, arguments, None)
    }

    pub(super) fn instantiate_interface_with_this(
        self: &Rc<Self>,
        group: &SymbolGroup,
        target: &Rc<TypeCell>,
        arguments: &[Rc<TypeCell>],
        this_argument: &Rc<TypeCell>,
    ) -> Result<Rc<TypeCell>, Error> {
        self.instantiate_interface_worker(group, target, arguments, Some(this_argument))
    }

    fn instantiate_interface_worker(
        self: &Rc<Self>,
        group: &SymbolGroup,
        target: &Rc<TypeCell>,
        arguments: &[Rc<TypeCell>],
        this_argument: Option<&Rc<TypeCell>>,
    ) -> Result<Rc<TypeCell>, Error> {
        let declarations = self
            .type_declaration_nodes(group)?
            .into_iter()
            .filter(|&node| {
                self.input
                    .node(node)
                    .is_ok_and(|read| read.kind() == K::InterfaceDeclaration)
            })
            .collect::<Vec<_>>();
        let first = *declarations.first().ok_or(Error::ResolutionFailed)?;
        let mut env = self.arguments_environment(first, arguments, &Environment::default())?;
        let parameter_nodes = self.list(first, self.input.node(first)?.type_parameter_list())?;
        let mut resolved_arguments = parameter_nodes
            .iter()
            .map(|&parameter| env.get(parameter)?.ok_or(Error::ResolutionFailed))
            .collect::<Result<Vec<_>, _>>()?;
        if this_argument.is_none() {
            if let Some(generic) = target.generic_target() {
                let parameters = generic.parameters()?;
                if parameters.len() == resolved_arguments.len()
                    && parameters
                        .iter()
                        .zip(&resolved_arguments)
                        .all(|(parameter, argument)| Rc::ptr_eq(parameter, argument))
                {
                    return Ok(target.clone());
                }
            }
        }
        for &declaration in declarations.iter().skip(1) {
            for (parameter, argument) in self
                .list(
                    declaration,
                    self.input.node(declaration)?.type_parameter_list(),
                )?
                .into_iter()
                .zip(&resolved_arguments)
            {
                env.0.push((parameter, Rc::downgrade(argument)));
            }
        }
        env.1 = Some(instantiate::Mapper::new(env.clone()));
        if let Some(this_argument) = this_argument {
            if !self.interface_this.borrow().contains_key(&target.id()) {
                return missing("explicit this argument on a thisless interface");
            }
            resolved_arguments.push(this_argument.clone());
        }
        let key = (
            target.id(),
            resolved_arguments
                .iter()
                .map(|argument| argument.id())
                .collect(),
        );
        if let Some(ty) = self.interface_instances.borrow().get(&key).cloned() {
            return Ok(ty);
        }
        let name = format!(
            "{}<{}>",
            target.name(),
            resolved_arguments
                .iter()
                .map(|ty| ty.name())
                .collect::<Vec<_>>()
                .join(", ")
        );
        let instance = self.object(declarations, env, name.into(), of::REFERENCE)?;
        instance.set_reference_shape(target, &resolved_arguments)?;
        if let Some(readonly) = self.global_array_kind(target) {
            let element = resolved_arguments.first().ok_or(Error::ResolutionFailed)?;
            instance.set_array_element(element, readonly)?;
        }
        self.interface_instances
            .borrow_mut()
            .insert(key, instance.clone());
        // Array recognition follows the resolved global target's identity, not
        // a spelling at a use site (a local interface named Array may shadow it).
        Ok(instance)
    }

    fn global_array_kind(&self, target: &Rc<TypeCell>) -> Option<bool> {
        let globals = self.initialization.named.borrow();
        if globals
            .get("Array")
            .is_some_and(|array| Rc::ptr_eq(array, target))
        {
            Some(false)
        } else if globals
            .get("ReadonlyArray")
            .is_some_and(|array| Rc::ptr_eq(array, target))
        {
            Some(true)
        } else {
            None
        }
    }

    pub(super) fn this_type(self: &Rc<Self>, node: NodeId) -> Result<Rc<TypeCell>, Error> {
        let mut current = node;
        while let Some(parent) = self.input.node(current)?.parent() {
            let read = self.input.node(parent)?;
            if read.kind() == K::InterfaceDeclaration {
                let group = self
                    .input
                    .resolve_type_name(read.name().ok_or(Error::ResolutionFailed)?)?
                    .ok_or(Error::ResolutionFailed)?;
                let target = self.declared(&group)?;
                return self
                    .interface_this
                    .borrow()
                    .get(&target.id())
                    .cloned()
                    .ok_or_else(|| Error::Unsupported("this type on thisless interface".into()));
            }
            if matches!(
                read.kind().known(),
                Some(K::ClassDeclaration | K::ClassExpression)
            ) {
                return missing("class this-type construction");
            }
            current = parent;
        }
        missing("this type outside a class or interface")
    }

    pub(super) fn reference_member_environment(
        &self,
        cell: &TypeCell,
        outer: &Environment,
    ) -> Result<Environment, Error> {
        let Some(reference) = cell.reference_shape() else {
            return Ok(outer.clone());
        };
        let target = reference.target()?;
        if target.id() == cell.id() {
            return Ok(outer.clone());
        }
        let arguments = reference.arguments()?;
        let parameter_count = match target.generic_target() {
            Some(generic) => generic.parameters()?.len(),
            None => 0,
        };
        let this = if arguments.len() == parameter_count + 1 {
            arguments.last().cloned().ok_or(Error::ResolutionFailed)?
        } else if arguments.len() == parameter_count {
            self.checker
                .graph
                .types
                .borrow()
                .get(cell.id() as usize - 1)
                .cloned()
                .ok_or(Error::Released)?
        } else {
            return missing("interface reference type-argument arity");
        };
        let mut env = outer.clone();
        let symbols = self
            .declared_types
            .borrow()
            .iter()
            .find(|(_, declared)| Rc::ptr_eq(declared, &target))
            .map(|(symbols, _)| symbols.clone())
            .ok_or_else(|| {
                Error::Unsupported("interface reference target has no declaration".into())
            })?;
        for declaration in self.type_declaration_nodes(&SymbolGroup { symbols })? {
            let read = self.input.node(declaration)?;
            if read.kind() != K::InterfaceDeclaration {
                continue;
            }
            let nodes = self.list(declaration, read.type_parameter_list())?;
            if nodes.len() != parameter_count {
                return missing("merged interface type-parameter arity");
            }
            for (parameter, argument) in nodes.into_iter().zip(&arguments) {
                env.0.push((parameter, Rc::downgrade(argument)));
            }
        }
        env.1 = Some(
            match self.interface_this.borrow().get(&target.id()).cloned() {
                Some(marker) => instantiate::Mapper::with_types(env.clone(), &[marker], &[this])?,
                None => instantiate::Mapper::new(env.clone()),
            },
        );
        Ok(env)
    }

    pub(super) fn type_declaration_nodes(&self, group: &SymbolGroup) -> Result<Vec<NodeId>, Error> {
        let mut declarations = Vec::new();
        for node in self.declarations(group)? {
            if matches!(
                self.input.node(node)?.kind().known(),
                Some(
                    K::InterfaceDeclaration
                        | K::ClassDeclaration
                        | K::TypeAliasDeclaration
                        | K::JSTypeAliasDeclaration
                        | K::TypeParameter
                )
            ) {
                declarations.push(node);
            }
        }
        Ok(declarations)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bound::BoundChecker;
    use crate::bound_input::BoundInput;
    use crate::bound_input::BoundInputOptions;
    use crate::LiteralValue;

    fn fixture(text: &[u8]) -> (BoundChecker, NodeId) {
        let file = ts_binder::bind_parsed_file(ts_parser::parse_source_file(
            ts_jsstring::SourceText::from_loaded_bytes(text),
            ts_core::ScriptKind::TS,
            ts_ast::SourceFileParseOptions {
                file_name: ts_ast::JsString::from_bytes(b"/generic.ts".as_slice()),
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

    fn named(owner: &BoundChecker, source: NodeId, name: &[u8]) -> Rc<TypeCell> {
        let node = owner
            .input()
            .declaration_by_name(source, name)
            .unwrap()
            .unwrap();
        owner.declared_type(node).unwrap()
    }

    #[test]
    fn aliased_interface_arguments_remain_unconstructed_until_requested() {
        let (owner, source) =
            fixture(b"interface Box<T> { value: T } type A = Box<'x'>; type B = Box<string>;");
        let before = owner.checker().graph.len();
        let a = named(&owner, source, b"A");
        let b = named(&owner, source, b"B");
        // One target, its declared parameter and this marker, and two syntax
        // references. The string literal's regular/fresh pair stays deferred.
        assert_eq!(owner.checker().graph.len() - before, 5);
        assert_eq!(a.resolutions(), 0);
        assert_eq!(b.resolutions(), 0);
        let arguments = a.reference_shape().unwrap().arguments().unwrap();
        assert_eq!(arguments[0].literal, Some(LiteralValue::String("x".into())));
        assert_eq!(owner.checker().graph.len() - before, 7);
        assert!(Rc::ptr_eq(
            &arguments[0],
            &a.reference_shape().unwrap().arguments().unwrap()[0]
        ));
    }

    #[test]
    fn deferred_recursive_alias_argument_uses_the_published_reference() {
        let (owner, source) = fixture(b"interface Box<T> { value: T } type A = Box<A>;");
        let a = named(&owner, source, b"A");
        let arguments = a.reference_shape().unwrap().arguments().unwrap();
        assert!(Rc::ptr_eq(&arguments[0], &a));
        let member = &a.members(&owner.checker().graph).unwrap()[0];
        assert!(Rc::ptr_eq(&member.r#type().unwrap(), &a));
        let state = Rc::downgrade(&owner.state);
        drop(owner);
        assert!(state.upgrade().is_none());
    }

    #[test]
    fn inherited_polymorphic_this_refers_to_the_derived_instance() {
        let (owner, source) = fixture(
            b"interface Base<T> { self: this; value: T } interface Derived<T> extends Base<T> { own: T } type A = Derived<number>;",
        );
        let a = named(&owner, source, b"A");
        let members = a.members(&owner.checker().graph).unwrap();
        let this = members
            .iter()
            .find(|member| member.name.as_ref() == "self")
            .unwrap()
            .r#type()
            .unwrap();
        assert!(Rc::ptr_eq(&this, &a));
        let value = members
            .iter()
            .find(|member| member.name.as_ref() == "value")
            .unwrap()
            .r#type()
            .unwrap();
        assert_eq!(value.flags, tf::NUMBER);
    }
}
