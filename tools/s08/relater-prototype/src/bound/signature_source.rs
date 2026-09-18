//! Lazy instantiated symbols and signatures. Each instantiated signature owns
//! its mapper and links to its target; merely resolving a method table does not
//! resolve the method's parameters or return type.

use super::{
    instantiate, missing, of, tf, Construction, Environment, Error, IndexInfo, Rc, Signature,
    Structure, TypeCell, TypeLink,
};

/// `links.target` and `links.mapper` of an instantiated symbol.
struct SymbolOrigin {
    root: TypeLink,
    mapper: instantiate::Mapper,
}

impl Construction {
    pub(super) fn attach_signature_factory(
        self: &Rc<Self>,
        signature: &mut Signature,
        parameters: &[Rc<TypeCell>],
    ) {
        if parameters.is_empty() {
            return;
        }
        let source = signature.clone();
        let parameters_owned = parameters.iter().map(Rc::downgrade).collect::<Vec<_>>();
        let weak = Rc::downgrade(self);
        signature.generic = Some(crate::GenericSignature::new(
            parameters,
            move |checker, arguments| {
                let state = weak.upgrade().ok_or(Error::Released)?;
                if !std::ptr::eq(checker, &raw const state.checker) {
                    return missing("signature used by another checker");
                }
                let parameters = parameters_owned
                    .iter()
                    .map(|ty| ty.upgrade().ok_or(Error::Released))
                    .collect::<Result<Vec<_>, _>>()?;
                let mapper = instantiate::Mapper::with_types(
                    Environment::default(),
                    &parameters,
                    arguments,
                )?;
                state.instantiate_signature(&source, &mapper, true)
            },
        ));
    }

    /// The return type of an instantiated signature is the target's return
    /// type instantiated with the signature's mapper, one step at a time
    /// (`getReturnTypeOfSignature`), unlike symbols, which return to their root.
    fn instantiated_link(
        self: &Rc<Self>,
        source: TypeLink,
        mapper: instantiate::Mapper,
    ) -> TypeLink {
        let weak = Rc::downgrade(self);
        TypeLink::lazy(move || {
            let state = weak.upgrade().ok_or(Error::Released)?;
            state.instantiate(&source.resolve()?, &mapper, None)
        })
    }

    // port: tsc/internal/checker/checker.go:Checker.instantiateSymbol
    fn instantiated_symbol_link(
        self: &Rc<Self>,
        source: TypeLink,
        mapper: instantiate::Mapper,
    ) -> Result<TypeLink, Error> {
        // A symbol whose type is resolved and cannot be affected by
        // instantiation is returned as is.
        if let Some(resolved) = source.resolved()? {
            if !self.could_contain_type_variables(&resolved)? {
                return Ok(source);
            }
        }
        // An instantiation of an instantiated symbol goes back to the original
        // target with the combined mapper, preserving original identities.
        let (root, mapper) = match source.origin::<SymbolOrigin>() {
            Some(origin) => (
                origin.root.clone(),
                instantiate::Mapper::compose(&origin.mapper, &mapper),
            ),
            None => (source, mapper),
        };
        let origin = Rc::new(SymbolOrigin {
            root: root.clone(),
            mapper: mapper.clone(),
        });
        let weak = Rc::downgrade(self);
        Ok(TypeLink::lazy_with_origin(
            move || {
                let state = weak.upgrade().ok_or(Error::Released)?;
                state.instantiate(&root.resolve()?, &mapper, None)
            },
            origin,
        ))
    }

    // port: tsc/internal/checker/checker.go:Checker.instantiateSignatureEx
    fn instantiate_signature(
        self: &Rc<Self>,
        source: &Signature,
        outer_mapper: &instantiate::Mapper,
        erase: bool,
    ) -> Result<Signature, Error> {
        let mut mapper = outer_mapper.clone();
        let mut parameters = Vec::new();
        if !erase {
            if let Some(generic) = &source.generic {
                let original = generic.parameters()?;
                for parameter in &original {
                    parameters.push(
                        self.checker
                            .graph
                            .primitive(tf::TYPE_PARAMETER, &parameter.name),
                    );
                }
                let local = instantiate::Mapper::with_types(
                    Environment::default(),
                    &original,
                    &parameters,
                )?;
                mapper = instantiate::Mapper::compose(&local, outer_mapper);
                for (original, fresh) in original.iter().zip(&parameters) {
                    let weak_original = Rc::downgrade(original);
                    let weak_state = Rc::downgrade(self);
                    let mapper = mapper.clone();
                    let constraint = TypeLink::lazy(move || {
                        let original = weak_original.upgrade().ok_or(Error::Released)?;
                        let state = weak_state.upgrade().ok_or(Error::Released)?;
                        let constraint = original
                            .type_parameter_shape()
                            .map(crate::TypeParameterShape::constraint)
                            .transpose()?
                            .flatten();
                        match constraint {
                            Some(constraint) => state.instantiate(&constraint, &mapper, None),
                            None => state
                                .initialization
                                .named
                                .borrow()
                                .get("noConstraintType")
                                .cloned()
                                .ok_or(Error::ResolutionFailed),
                        }
                    });
                    // A missing constraint remains absent. The current source
                    // shape exposes this without constructing a constraint type.
                    let has_constraint = original
                        .type_parameter_shape()
                        .is_some_and(crate::TypeParameterShape::has_declared_constraint);
                    fresh.set_lazy_type_parameter(
                        has_constraint.then_some(constraint),
                        Some(original),
                        0,
                    )?;
                }
            }
        }
        self.signatures.set(self.signatures.get() + 1);
        let mut result = source.clone();
        result.parameters = source
            .parameters
            .iter()
            .cloned()
            .map(|link| self.instantiated_symbol_link(link, mapper.clone()))
            .collect::<Result<_, _>>()?;
        result.this_type = source
            .this_type
            .clone()
            .map(|link| self.instantiated_symbol_link(link, mapper.clone()))
            .transpose()?;
        result.return_type = self.instantiated_link(source.return_type.clone(), mapper);
        result.type_parameters = parameters.len();
        result.generic = None;
        result.target = Some(Rc::new(source.clone()));
        self.attach_signature_factory(&mut result, &parameters);
        Ok(result)
    }

    // Fallible like every other constructor here, so call sites stay uniform.
    #[allow(clippy::unnecessary_wraps)]
    // port: tsc/internal/checker/checker.go:Checker.resolveAnonymousTypeMembers
    pub(super) fn instantiated_anonymous(
        self: &Rc<Self>,
        target: &Rc<TypeCell>,
        mapper: instantiate::Mapper,
        name: Rc<str>,
        alias: Option<u64>,
    ) -> Result<Rc<TypeCell>, Error> {
        let symbol = target.symbol;
        let target = Rc::downgrade(target);
        let weak = Rc::downgrade(self);
        let resolver = Box::new(move |_: &crate::Graph, _: &TypeCell| {
            let state = weak.upgrade().ok_or(Error::Released)?;
            let target = target.upgrade().ok_or(Error::Released)?;
            let source = target.structure(&state.checker.graph)?;
            let mut result = Structure::default();
            for member in &source.members {
                let mut member = member.clone();
                member.r#type = state.instantiated_symbol_link(member.r#type, mapper.clone())?;
                result.members.push(member);
            }
            for signature in &source.call_signatures {
                result
                    .call_signatures
                    .push(state.instantiate_signature(signature, &mapper, false)?);
            }
            for signature in &source.construct_signatures {
                result
                    .construct_signatures
                    .push(state.instantiate_signature(signature, &mapper, false)?);
            }
            for index in &source.index_infos {
                result.index_infos.push(IndexInfo {
                    key: index.key.clone(),
                    value: Rc::downgrade(&state.instantiate(&index.value()?, &mapper, None)?),
                    readonly: index.readonly,
                });
            }
            Ok(result)
        });
        Ok(self.checker.graph.allocate_full(
            tf::OBJECT,
            of::ANONYMOUS | of::INSTANTIATED,
            name,
            symbol,
            alias,
            None,
            false,
            Vec::new(),
            true,
            Some(resolver),
        ))
    }
}
