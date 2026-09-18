//! Generic reference variance and signature instantiation. Factories resolve
//! bound declarations in the frontend; relation callbacks always stay here.

use super::*;

type InstantiateType =
    Box<dyn Fn(&Checker, &Rc<TypeCell>, &[Rc<TypeCell>]) -> Result<Rc<TypeCell>, Error>>;
type InstantiateSignature = Box<dyn Fn(&Checker, &[Rc<TypeCell>]) -> Result<Signature, Error>>;

#[derive(Debug)]
pub struct ReferenceShape {
    target: Weak<TypeCell>,
    arguments: Vec<TypeLink>,
    deferred_node: Option<ts_arena::NodeId>,
}
impl ReferenceShape {
    pub fn target(&self) -> Result<Rc<TypeCell>, Error> {
        self.target.upgrade().ok_or(Error::Released)
    }
    pub fn arguments(&self) -> Result<Vec<Rc<TypeCell>>, Error> {
        self.arguments.iter().map(TypeLink::resolve).collect()
    }
    pub(crate) fn deferred_node(&self) -> Option<ts_arena::NodeId> {
        self.deferred_node
    }
}

pub struct GenericTarget {
    parameters: Vec<Weak<TypeCell>>,
    instantiate: InstantiateType,
    variances: RefCell<Option<Vec<u8>>>,
    failed: Cell<bool>,
}
impl std::fmt::Debug for GenericTarget {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GenericTarget")
            .field("parameter_count", &self.parameters.len())
            .field("variances", &self.variances.borrow())
            .finish_non_exhaustive()
    }
}
impl GenericTarget {
    pub fn parameters(&self) -> Result<Vec<Rc<TypeCell>>, Error> {
        self.parameters
            .iter()
            .map(|t| t.upgrade().ok_or(Error::Released))
            .collect()
    }
    pub fn instantiate(
        &self,
        checker: &Checker,
        target: &Rc<TypeCell>,
        arguments: &[Rc<TypeCell>],
    ) -> Result<Rc<TypeCell>, Error> {
        if !checker.graph.owns(target) || arguments.iter().any(|t| !checker.graph.owns(t)) {
            return unsupported("generic factory owner mismatch");
        }
        (self.instantiate)(checker, target, arguments)
    }
}

#[derive(Debug)]
pub struct TypeParameterShape {
    constraint: Option<TypeLink>,
    original: Option<Weak<TypeCell>>,
    /// `in` = 1, `out` = 2; zero means infer variance from marker relations.
    pub modifiers: u8,
}
impl TypeParameterShape {
    pub(crate) fn has_declared_constraint(&self) -> bool {
        self.constraint.is_some()
    }
    pub fn constraint(&self) -> Result<Option<Rc<TypeCell>>, Error> {
        self.constraint.as_ref().map(TypeLink::resolve).transpose()
    }
    pub fn original(&self) -> Result<Option<Rc<TypeCell>>, Error> {
        self.original
            .as_ref()
            .map(|t| t.upgrade().ok_or(Error::Released))
            .transpose()
    }
}

impl Graph {
    pub(super) fn owns(&self, cell: &Rc<TypeCell>) -> bool {
        self.types
            .borrow()
            .get(cell.id as usize - 1)
            .is_some_and(|owned| Rc::ptr_eq(owned, cell))
    }
    pub fn type_parameter(&self, name: &str, constraint: Option<&Rc<TypeCell>>) -> Rc<TypeCell> {
        let cell = self.primitive(flags::TYPE_PARAMETER, name);
        cell.set_type_parameter(constraint, None, 0)
            .expect("new type parameter metadata");
        cell
    }
}

impl TypeCell {
    pub fn set_reference_shape(
        &self,
        target: &Rc<TypeCell>,
        arguments: &[Rc<TypeCell>],
    ) -> Result<(), Error> {
        self.set_lazy_reference_shape(
            target,
            arguments.iter().map(|t| Rc::downgrade(t).into()).collect(),
        )
    }
    pub(crate) fn set_lazy_reference_shape(
        &self,
        target: &Rc<TypeCell>,
        arguments: Vec<TypeLink>,
    ) -> Result<(), Error> {
        self.set_reference_shape_worker(target, arguments, None)
    }
    pub(crate) fn set_deferred_reference_shape(
        &self,
        target: &Rc<TypeCell>,
        arguments: Vec<TypeLink>,
        node: ts_arena::NodeId,
    ) -> Result<(), Error> {
        self.set_reference_shape_worker(target, arguments, Some(node))
    }
    fn set_reference_shape_worker(
        &self,
        target: &Rc<TypeCell>,
        arguments: Vec<TypeLink>,
        deferred_node: Option<ts_arena::NodeId>,
    ) -> Result<(), Error> {
        self.reference_shape
            .set(ReferenceShape {
                target: Rc::downgrade(target),
                arguments,
                deferred_node,
            })
            .map_err(|_| Error::Unsupported(Rc::from("reference metadata already set")))
    }
    pub fn reference_shape(&self) -> Option<&ReferenceShape> {
        self.reference_shape.get()
    }
    pub fn set_generic_target(
        &self,
        parameters: &[Rc<TypeCell>],
        instantiate: impl Fn(&Checker, &Rc<TypeCell>, &[Rc<TypeCell>]) -> Result<Rc<TypeCell>, Error>
            + 'static,
    ) -> Result<(), Error> {
        self.generic_target
            .set(GenericTarget {
                parameters: parameters.iter().map(Rc::downgrade).collect(),
                instantiate: Box::new(instantiate),
                variances: RefCell::new(None),
                failed: Cell::new(false),
            })
            .map_err(|_| Error::Unsupported(Rc::from("generic target metadata already set")))
    }
    pub fn generic_target(&self) -> Option<&GenericTarget> {
        self.generic_target.get()
    }
    pub fn set_type_parameter(
        &self,
        constraint: Option<&Rc<TypeCell>>,
        original: Option<&Rc<TypeCell>>,
        modifiers: u8,
    ) -> Result<(), Error> {
        self.type_parameter
            .set(TypeParameterShape {
                constraint: constraint.map(|t| Rc::downgrade(t).into()),
                original: original.map(Rc::downgrade),
                modifiers,
            })
            .map_err(|_| Error::Unsupported(Rc::from("type parameter metadata already set")))
    }
    pub(crate) fn set_lazy_type_parameter(
        &self,
        constraint: Option<TypeLink>,
        original: Option<&Rc<TypeCell>>,
        modifiers: u8,
    ) -> Result<(), Error> {
        self.type_parameter
            .set(TypeParameterShape {
                constraint,
                original: original.map(Rc::downgrade),
                modifiers,
            })
            .map_err(|_| Error::Unsupported(Rc::from("type parameter metadata already set")))
    }
    pub fn type_parameter_shape(&self) -> Option<&TypeParameterShape> {
        self.type_parameter.get()
    }
}

pub struct GenericSignature {
    parameters: Vec<Weak<TypeCell>>,
    instantiate: InstantiateSignature,
    instantiations: RefCell<HashMap<Vec<u32>, Signature>>,
}
impl std::fmt::Debug for GenericSignature {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GenericSignature")
            .field("parameter_count", &self.parameters.len())
            .finish_non_exhaustive()
    }
}
impl GenericSignature {
    pub fn new(
        parameters: &[Rc<TypeCell>],
        instantiate: impl Fn(&Checker, &[Rc<TypeCell>]) -> Result<Signature, Error> + 'static,
    ) -> Rc<Self> {
        Rc::new(Self {
            parameters: parameters.iter().map(Rc::downgrade).collect(),
            instantiate: Box::new(instantiate),
            instantiations: RefCell::new(HashMap::new()),
        })
    }
    pub fn parameters(&self) -> Result<Vec<Rc<TypeCell>>, Error> {
        self.parameters
            .iter()
            .map(|t| t.upgrade().ok_or(Error::Released))
            .collect()
    }
    pub fn instantiate(
        &self,
        checker: &Checker,
        arguments: &[Rc<TypeCell>],
    ) -> Result<Signature, Error> {
        if arguments.len() != self.parameters.len()
            || arguments.iter().any(|t| !checker.graph.owns(t))
        {
            return unsupported("signature instantiation arguments/owner mismatch");
        }
        let key = arguments.iter().map(|t| t.id).collect::<Vec<_>>();
        if let Some(signature) = self.instantiations.borrow().get(&key) {
            return Ok(signature.clone());
        }
        let result = (self.instantiate)(checker, arguments)?;
        if result.generic.is_some() {
            return unsupported("instantiated signature retains generic factory");
        }
        self.instantiations.borrow_mut().insert(key, result.clone());
        Ok(result)
    }
    fn erased(&self, checker: &Checker) -> Result<Signature, Error> {
        self.instantiate(
            checker,
            &vec![checker.intrinsic(flags::ANY)?; self.parameters.len()],
        )
    }
    fn canonical(&self, checker: &Checker) -> Result<Signature, Error> {
        let mut arguments = Vec::new();
        for parameter in self.parameters()? {
            let original = parameter
                .type_parameter_shape()
                .map(TypeParameterShape::original)
                .transpose()?
                .flatten();
            let unconstrained_original = if let Some(original) = &original {
                original
                    .type_parameter_shape()
                    .map(TypeParameterShape::constraint)
                    .transpose()?
                    .flatten()
                    .is_none()
            } else {
                false
            };
            arguments.push(if unconstrained_original {
                original.expect("original checked")
            } else {
                parameter
            });
        }
        self.instantiate(checker, &arguments)
    }
}

pub(super) struct VarianceMarkers {
    super_type: Weak<TypeCell>,
    sub_type: Weak<TypeCell>,
    other_type: Weak<TypeCell>,
}

impl Checker {
    pub fn register_variance_markers(
        &self,
        super_type: &Rc<TypeCell>,
        sub_type: &Rc<TypeCell>,
        other_type: &Rc<TypeCell>,
    ) -> Result<(), Error> {
        if [super_type, sub_type, other_type]
            .iter()
            .any(|t| !self.graph.owns(t))
        {
            return unsupported("variance marker owner mismatch");
        }
        self.variance_markers
            .set(VarianceMarkers {
                super_type: Rc::downgrade(super_type),
                sub_type: Rc::downgrade(sub_type),
                other_type: Rc::downgrade(other_type),
            })
            .map_err(|_| Error::Unsupported(Rc::from("variance markers already set")))
    }

    // port: tsc/internal/checker/relater.go:Checker.getVariancesWorker
    pub(crate) fn variances(&self, target: &Rc<TypeCell>) -> Result<Vec<u8>, Error> {
        let data = target
            .generic_target()
            .ok_or_else(|| Error::Unsupported(Rc::from("reference target metadata missing")))?;
        if data.failed.get() {
            return Err(Error::ResolutionFailed);
        }
        if let Some(variances) = data.variances.borrow().as_ref() {
            return Ok(variances.clone());
        }
        if self
            .variance_stack
            .borrow()
            .iter()
            .any(|t| t.upgrade().is_some_and(|t| Rc::ptr_eq(&t, target)))
        {
            *data.variances.borrow_mut() = Some(Vec::new());
            return Ok(Vec::new());
        }
        let index = self.variance_stack.borrow().len();
        self.variance_stack.borrow_mut().push(Rc::downgrade(target));
        struct Guard<'a> {
            checker: &'a Checker,
            data: &'a GenericTarget,
            index: usize,
            completed: bool,
        }
        impl Drop for Guard<'_> {
            fn drop(&mut self) {
                self.checker
                    .variance_stack
                    .borrow_mut()
                    .truncate(self.index);
                if !self.completed {
                    self.data.failed.set(true);
                }
            }
        }
        let mut guard = Guard {
            checker: self,
            data,
            index,
            completed: false,
        };
        let parameters = data.parameters()?;
        let mut variances = Vec::with_capacity(parameters.len());
        for (index, parameter) in parameters.iter().enumerate() {
            let modifiers = parameter.type_parameter_shape().map_or(0, |p| p.modifiers);
            let variance = if modifiers != 0 {
                match modifiers {
                    1 => 2,
                    2 => 1,
                    _ => 0,
                }
            } else {
                let markers = self.variance_markers.get().ok_or_else(|| {
                    Error::Unsupported(Rc::from("variance markers not initialized"))
                })?;
                let mut arguments = parameters.clone();
                arguments[index] = markers.super_type.upgrade().ok_or(Error::Released)?;
                let with_super = data.instantiate(self, target, &arguments)?;
                with_super.marker_instantiation.set(true);
                arguments[index] = markers.sub_type.upgrade().ok_or(Error::Released)?;
                let with_sub = data.instantiate(self, target, &arguments)?;
                with_sub.marker_instantiation.set(true);
                let mut result = u8::from(self.is_type_assignable_to(&with_sub, &with_super)?)
                    | (u8::from(self.is_type_assignable_to(&with_super, &with_sub)?) << 1);
                if result == 3 {
                    arguments[index] = markers.other_type.upgrade().ok_or(Error::Released)?;
                    let other = data.instantiate(self, target, &arguments)?;
                    other.marker_instantiation.set(true);
                    if self.is_type_assignable_to(&other, &with_super)? {
                        result = 4;
                    }
                }
                result
            };
            variances.push(variance);
        }
        *data.variances.borrow_mut() = Some(variances.clone());
        guard.completed = true;
        Ok(variances)
    }
}

impl Relater<'_> {
    pub(super) fn instantiate_identical_signature(
        &mut self,
        source: &Signature,
        target: &Signature,
    ) -> Result<Option<Signature>, Error> {
        let source_generic = source.generic.as_ref().ok_or_else(|| {
            Error::Unsupported(Rc::from("generic source signature metadata missing"))
        })?;
        let target_generic = target.generic.as_ref().ok_or_else(|| {
            Error::Unsupported(Rc::from("generic target signature metadata missing"))
        })?;
        let source_params = source_generic.parameters()?;
        let target_params = target_generic.parameters()?;
        for (source, target) in source_params.iter().zip(&target_params) {
            if Rc::ptr_eq(source, target) {
                continue;
            }
            let source_constraint = source
                .type_parameter_shape()
                .map(TypeParameterShape::constraint)
                .transpose()?
                .flatten();
            let target_constraint = target
                .type_parameter_shape()
                .map(TypeParameterShape::constraint)
                .transpose()?
                .flatten();
            match (source_constraint, target_constraint) {
                (None, None) => {}
                (Some(source), Some(target))
                    if source.flags & flags::INSTANTIABLE == 0
                        && target.flags & flags::INSTANTIABLE == 0 =>
                {
                    if self.is_related_to_simple(&source, &target)? == FALSE {
                        return Ok(None);
                    }
                }
                (Some(_), Some(_)) => {
                    return unsupported("generic signature constraint substitution")
                }
                _ => return Ok(None),
            }
        }
        Ok(Some(
            source_generic.instantiate(self.checker, &target_params)?,
        ))
    }

    pub(super) fn reference_arguments_related(
        &mut self,
        source: &Rc<TypeCell>,
        target: &Rc<TypeCell>,
        report_errors: bool,
        intersection_state: u8,
    ) -> Result<Option<Ternary>, Error> {
        let (Some(source_ref), Some(target_ref)) =
            (source.reference_shape(), target.reference_shape())
        else {
            return Ok(None);
        };
        let source_target = source_ref.target()?;
        if !Rc::ptr_eq(&source_target, &target_ref.target()?)
            || source.tuple_shape().is_some()
            || source.marker_instantiation.get()
            || target.marker_instantiation.get()
        {
            return Ok(None);
        }
        // `getVariances` returns the known covariant vector for the two global
        // array targets; it never resolves their members to measure variance.
        let variances = if source.array_element().is_some() && target.array_element().is_some() {
            vec![1]
        } else {
            self.checker.variances(&source_target)?
        };
        if variances.is_empty() {
            return Ok(Some(UNKNOWN));
        }
        let result = self.type_arguments_related(
            &source_ref.arguments()?,
            &target_ref.arguments()?,
            &variances,
            report_errors,
            intersection_state,
        )?;
        if result != FALSE {
            return Ok(Some(result));
        }
        // Covariant void arguments allow the native structural fallback.
        if target_ref
            .arguments()?
            .iter()
            .zip(&variances)
            .any(|(arg, variance)| *variance == 1 && arg.flags & flags::VOID != 0)
        {
            return Ok(None);
        }
        // Invariance failures must elaborate structurally when reporting. A
        // success from a depth-limited structural pass still cannot override
        // the variance failure; callers retain that decision separately.
        if report_errors && variances.contains(&0) {
            return unsupported("invariant reference structural error elaboration");
        }
        Ok(Some(FALSE))
    }

    fn type_arguments_related(
        &mut self,
        sources: &[Rc<TypeCell>],
        targets: &[Rc<TypeCell>],
        variances: &[u8],
        report_errors: bool,
        intersection_state: u8,
    ) -> Result<Ternary, Error> {
        if sources.len() != targets.len() && self.mode == Mode::Identity {
            return Ok(FALSE);
        }
        let mut result = TRUE;
        for (index, (source, target)) in sources.iter().zip(targets).enumerate() {
            let variance = variances.get(index).copied().unwrap_or(1);
            let related = match variance {
                4 => TRUE,
                1 => self.is_related_to_ex(
                    source,
                    target,
                    RECURSION_BOTH,
                    report_errors,
                    intersection_state,
                )?,
                2 => self.is_related_to_ex(
                    target,
                    source,
                    RECURSION_BOTH,
                    report_errors,
                    intersection_state,
                )?,
                3 => {
                    let reverse = self.is_related_to_ex(
                        target,
                        source,
                        RECURSION_BOTH,
                        false,
                        intersection_state,
                    )?;
                    if reverse != FALSE {
                        reverse
                    } else {
                        self.is_related_to_ex(
                            source,
                            target,
                            RECURSION_BOTH,
                            report_errors,
                            intersection_state,
                        )?
                    }
                }
                0 => {
                    let forward = self.is_related_to_ex(
                        source,
                        target,
                        RECURSION_BOTH,
                        report_errors,
                        intersection_state,
                    )?;
                    if forward == FALSE {
                        FALSE
                    } else {
                        forward
                            & self.is_related_to_ex(
                                target,
                                source,
                                RECURSION_BOTH,
                                report_errors,
                                intersection_state,
                            )?
                    }
                }
                _ => return unsupported("unmeasurable or unreliable variance"),
            };
            if related == FALSE {
                return Ok(FALSE);
            }
            result &= related;
        }
        Ok(result)
    }

    pub(super) fn erased_signature(&self, signature: &Signature) -> Result<Signature, Error> {
        if signature.type_parameters == 0 {
            return Ok(signature.clone());
        }
        signature
            .generic
            .as_ref()
            .ok_or_else(|| Error::Unsupported(Rc::from("generic signature factory missing")))?
            .erased(self.checker)
    }

    pub(super) fn instantiate_signature_in_context(
        &mut self,
        source: &Signature,
        target: &Signature,
    ) -> Result<(Signature, Signature), Error> {
        let source_generic = source
            .generic
            .as_ref()
            .ok_or_else(|| Error::Unsupported(Rc::from("generic signature factory missing")))?;
        let target = if let Some(generic) = &target.generic {
            generic.canonical(self.checker)?
        } else {
            target.clone()
        };
        // Inference uses the actual source/target structure, never declaration
        // strings. The recursive candidate collector covers direct parameters,
        // reference arguments and callback signatures used by the bound slice.
        let parameters = source_generic.parameters()?;
        let mut candidates = vec![Vec::<Rc<TypeCell>>::new(); parameters.len()];
        for index in 0..source.parameter_count().min(target.parameter_count()) {
            self.infer_signature_type(
                &target.parameter(index)?.ok_or(Error::Released)?,
                &source.parameter(index)?.ok_or(Error::Released)?,
                &parameters,
                &mut candidates,
            )?;
        }
        self.infer_signature_type(
            &target.return_type()?,
            &source.return_type()?,
            &parameters,
            &mut candidates,
        )?;
        let mut arguments = Vec::new();
        for (parameter, candidates) in parameters.iter().zip(candidates) {
            arguments.push(if candidates.is_empty() {
                parameter
                    .type_parameter_shape()
                    .map(TypeParameterShape::constraint)
                    .transpose()?
                    .flatten()
                    .unwrap_or(self.checker.intrinsic(flags::UNKNOWN)?)
            } else if candidates.len() == 1 {
                candidates[0].clone()
            } else {
                return unsupported("generic signature inference candidate priority/variance");
            });
        }
        Ok((
            source_generic.instantiate(self.checker, &arguments)?,
            target,
        ))
    }

    fn infer_signature_type(
        &mut self,
        source: &Rc<TypeCell>,
        target: &Rc<TypeCell>,
        parameters: &[Rc<TypeCell>],
        candidates: &mut [Vec<Rc<TypeCell>>],
    ) -> Result<(), Error> {
        if let Some(index) = parameters.iter().position(|p| Rc::ptr_eq(p, target)) {
            if !candidates[index].iter().any(|c| Rc::ptr_eq(c, source)) {
                candidates[index].push(source.clone());
            }
            return Ok(());
        }
        if let (Some(s), Some(t)) = (source.reference_shape(), target.reference_shape()) {
            if Rc::ptr_eq(&s.target()?, &t.target()?) {
                for (s, t) in s.arguments()?.iter().zip(t.arguments()?) {
                    self.infer_signature_type(s, &t, parameters, candidates)?;
                }
                return Ok(());
            }
        }
        if source.flags & flags::OBJECT != 0 && target.flags & flags::OBJECT != 0 {
            if !target.members(self.graph())?.is_empty() {
                return unsupported("generic signature inference through object properties");
            }
            let sources = self.signatures_of_type(source, false)?;
            let targets = self.signatures_of_type(target, false)?;
            for (s, t) in sources.iter().zip(&targets) {
                for index in 0..s.parameter_count().min(t.parameter_count()) {
                    self.infer_signature_type(
                        &s.parameter(index)?.ok_or(Error::Released)?,
                        &t.parameter(index)?.ok_or(Error::Released)?,
                        parameters,
                        candidates,
                    )?;
                }
                self.infer_signature_type(
                    &s.return_type()?,
                    &t.return_type()?,
                    parameters,
                    candidates,
                )?;
            }
        } else if target.flags & flags::UNION_OR_INTERSECTION != 0 {
            return unsupported("generic signature inference through compound types");
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn variance_is_measured_by_nested_relations_and_reused() {
        let checker = Checker::new();
        let graph = &checker.graph;
        let string = graph.primitive(flags::STRING, "string");
        let any = graph.primitive(flags::ANY, "any");
        checker.register_intrinsics(&[string.clone(), any]);
        let parameter = graph.type_parameter("T", None);
        let super_type = graph.type_parameter("markerSuper", None);
        let sub_type = graph.type_parameter("markerSub", Some(&super_type));
        let other_type = graph.type_parameter("markerOther", None);
        checker
            .register_variance_markers(&super_type, &sub_type, &other_type)
            .unwrap();
        let target = graph.object("Box", vec![]);
        let instances = RefCell::new(HashMap::<u32, Weak<TypeCell>>::new());
        target
            .set_generic_target(&[parameter], move |checker, target, args| {
                if let Some(existing) = instances
                    .borrow()
                    .get(&args[0].id())
                    .and_then(Weak::upgrade)
                {
                    return Ok(existing);
                }
                let cell = checker
                    .graph
                    .object("Box", vec![("value", false, Rc::downgrade(&args[0]))]);
                cell.set_reference_shape(target, args)?;
                instances
                    .borrow_mut()
                    .insert(args[0].id(), Rc::downgrade(&cell));
                Ok(cell)
            })
            .unwrap();
        let narrow = graph.string_literal(b"x");
        let source = target
            .generic_target()
            .unwrap()
            .instantiate(&checker, &target, &[narrow])
            .unwrap();
        let destination = target
            .generic_target()
            .unwrap()
            .instantiate(&checker, &target, &[string])
            .unwrap();
        assert!(
            checker
                .check_type_related_to(&source, &destination, Mode::Assignable, false)
                .unwrap()
                .1
        );
        assert_eq!(checker.take_observed(), [TRUE, FALSE, TRUE]);
        assert!(
            !checker
                .check_type_related_to(&destination, &source, Mode::Assignable, false)
                .unwrap()
                .1
        );
        assert_eq!(checker.take_observed(), [FALSE]);
    }

    #[test]
    fn generic_signature_instantiation_is_cached_and_cannot_create_strong_type_cycles() {
        let checker = Checker::new();
        let parameter = checker.graph.type_parameter("T", None);
        let any = checker.graph.primitive(flags::ANY, "any");
        checker.register_intrinsics(std::slice::from_ref(&any));
        let calls = Rc::new(Cell::new(0));
        let observed_calls = calls.clone();
        let generic = GenericSignature::new(&[parameter], move |_, args| {
            observed_calls.set(observed_calls.get() + 1);
            Ok(Signature {
                parameters: vec![Rc::downgrade(&args[0]).into()],
                parameter_names: vec![Rc::from("x")],
                min_argument_count: 1,
                has_rest_parameter: false,
                type_parameters: 0,
                generic: None,
                this_type: None,
                return_type: Rc::downgrade(&args[0]).into(),
                bivariant_parameters: false,
                is_abstract: false,
                is_construct: false,
            })
        });
        let first = generic.erased(&checker).unwrap();
        let second = generic.erased(&checker).unwrap();
        assert_eq!(calls.get(), 1);
        assert!(Rc::ptr_eq(
            &first.return_type().unwrap(),
            &second.return_type().unwrap()
        ));
        drop(any);
        drop(checker);
        assert_eq!(first.return_type().unwrap_err(), Error::Released);
    }
}
