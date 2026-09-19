//! Tuple element comparison from pinned `relater.go:propertiesRelatedTo`.
//! The source frontend supplies normalized elements; array/rest library members
//! remain lazy in the cell's ordinary structure resolver.

use super::{
    flags, unsupported, Error, Rc, Relater, Ternary, TypeCell, TypeLink, FALSE, RECURSION_BOTH,
    TRUE,
};

pub mod element_flags {
    pub const REQUIRED: u8 = 1;
    pub const OPTIONAL: u8 = 2;
    pub const REST: u8 = 4;
    pub const VARIADIC: u8 = 8;
    pub const VARIABLE: u8 = REST | VARIADIC;
    pub const NON_REST: u8 = REQUIRED | OPTIONAL | VARIADIC;
    pub const FIXED: u8 = REQUIRED | OPTIONAL;
}

#[derive(Debug)]
pub struct TupleShape {
    elements: Vec<TypeLink>,
    pub element_flags: Vec<u8>,
    pub readonly: bool,
    pub min_length: usize,
    pub combined_flags: u8,
}

impl TupleShape {
    // port: tsc/internal/checker/checker.go:getTotalFixedElementCount
    /// The initial required or optional elements plus the fixed elements that
    /// trail the last variable one.
    pub(crate) fn total_fixed_elements(&self) -> usize {
        let fixed_length = self
            .element_flags
            .iter()
            .take_while(|flags| *flags & element_flags::FIXED != 0)
            .count();
        let trailing = self
            .element_flags
            .iter()
            .rev()
            .take_while(|flags| *flags & element_flags::FIXED != 0)
            .count();
        // The two runs can only overlap in a tuple with no variable element,
        // which the pin never asks about: it consults this count for generic
        // tuples, and those always carry a variadic or rest element.
        fixed_length + trailing
    }

    pub fn elements(&self) -> Result<Vec<Rc<TypeCell>>, Error> {
        self.elements.iter().map(TypeLink::resolve).collect()
    }
}

#[derive(Debug)]
pub struct ArrayElement {
    element: TypeLink,
    pub readonly: bool,
}

impl ArrayElement {
    pub fn element(&self) -> Result<Rc<TypeCell>, Error> {
        self.element.resolve()
    }
}

impl TypeCell {
    /// Attach normalized tuple arguments once, before exposing the cell.
    /// A rest element carries its element type; a variadic element carries its
    /// array/tuple type, exactly as `TypeReference.resolvedTypeArguments` does.
    pub fn set_tuple_shape(
        &self,
        elements: &[Rc<TypeCell>],
        flags: &[u8],
        readonly: bool,
    ) -> Result<(), Error> {
        self.set_lazy_tuple_shape(
            elements.iter().map(|t| Rc::downgrade(t).into()).collect(),
            flags,
            readonly,
        )
    }

    pub(crate) fn set_lazy_tuple_shape(
        &self,
        elements: Vec<TypeLink>,
        flags: &[u8],
        readonly: bool,
    ) -> Result<(), Error> {
        if self.flags & flags::OBJECT == 0
            || elements.len() != flags.len()
            || flags.iter().any(|flag| {
                !matches!(
                    *flag,
                    element_flags::REQUIRED
                        | element_flags::OPTIONAL
                        | element_flags::REST
                        | element_flags::VARIADIC
                )
            })
            || self.array_element.get().is_some()
        {
            return unsupported("invalid tuple metadata");
        }
        self.tuple_shape
            .set(TupleShape {
                elements,
                element_flags: flags.to_vec(),
                readonly,
                min_length: flags
                    .iter()
                    .filter(|f| **f & (element_flags::REQUIRED | element_flags::VARIADIC) != 0)
                    .count(),
                combined_flags: flags.iter().fold(0, |acc, flag| acc | flag),
            })
            .map_err(|_| Error::Unsupported(Rc::from("tuple metadata already set")))
    }

    pub fn set_array_element(&self, element: &Rc<TypeCell>, readonly: bool) -> Result<(), Error> {
        self.set_lazy_array_element(Rc::downgrade(element).into(), readonly)
    }

    pub(crate) fn set_lazy_array_element(
        &self,
        element: TypeLink,
        readonly: bool,
    ) -> Result<(), Error> {
        if self.flags & flags::OBJECT == 0 || self.tuple_shape.get().is_some() {
            return unsupported("invalid array metadata");
        }
        self.array_element
            .set(ArrayElement { element, readonly })
            .map_err(|_| Error::Unsupported(Rc::from("array metadata already set")))
    }

    pub fn tuple_shape(&self) -> Option<&TupleShape> {
        self.tuple_shape.get()
    }
    pub fn array_element(&self) -> Option<&ArrayElement> {
        self.array_element.get()
    }
    pub fn is_array_or_tuple(&self) -> bool {
        self.tuple_shape.get().is_some() || self.array_element.get().is_some()
    }
    pub fn is_readonly_array_or_tuple(&self) -> bool {
        self.tuple_shape.get().is_some_and(|t| t.readonly)
            || self.array_element.get().is_some_and(|t| t.readonly)
    }
}

impl Relater<'_> {
    /// `propertiesRelatedTo`'s tuple-target branch. `None` means continue the
    /// ordinary member comparison, including tuple targets with fixed members.
    pub(super) fn tuple_properties_related_to(
        &mut self,
        source: &Rc<TypeCell>,
        target: &Rc<TypeCell>,
        report_errors: bool,
        intersection_state: u8,
    ) -> Result<Option<Ternary>, Error> {
        use element_flags as ef;
        let Some(target_shape) = target.tuple_shape() else {
            return Ok(None);
        };
        let source_shape = source.tuple_shape();
        let source_array = source.array_element();
        if source_shape.is_none() && source_array.is_none() {
            return Ok((target_shape.combined_flags & ef::VARIABLE != 0).then_some(FALSE));
        }
        if !target_shape.readonly && source.is_readonly_array_or_tuple() {
            return Ok(Some(FALSE));
        }
        let source_types = if let Some(shape) = source_shape {
            shape.elements()?
        } else {
            vec![source_array.expect("array shape checked").element()?]
        };
        let target_types = target_shape.elements()?;
        let source_arity = source_types.len();
        let target_arity = target_types.len();
        let source_rest = source_shape.is_none_or(|s| s.combined_flags & ef::REST != 0);
        let target_rest = target_shape.combined_flags & ef::REST != 0;
        let target_variable = target_shape.combined_flags & ef::VARIABLE != 0;
        let source_min = source_shape.map_or(0, |s| s.min_length);
        let target_min = target_shape.min_length;
        if !source_rest && source_arity < target_min {
            self.report(
                report_errors,
                tsr_diagnostics::Source_has_0_element_s_but_target_requires_1,
                vec![source_arity.to_string(), target_min.to_string()],
            );
            return Ok(Some(FALSE));
        }
        if !target_variable && target_arity < source_min {
            self.report(
                report_errors,
                tsr_diagnostics::Source_has_0_element_s_but_target_allows_only_1,
                vec![source_min.to_string(), target_arity.to_string()],
            );
            return Ok(Some(FALSE));
        }
        if !target_variable && (source_rest || target_arity < source_arity) {
            let (message, arg) = if source_min < target_min {
                (
                    &tsr_diagnostics::Target_requires_0_element_s_but_source_may_have_fewer,
                    target_min,
                )
            } else {
                (
                    &tsr_diagnostics::Target_allows_only_0_element_s_but_source_may_have_more,
                    target_arity,
                )
            };
            self.report(report_errors, message, vec![arg.to_string()]);
            return Ok(Some(FALSE));
        }
        let target_start = target_shape
            .element_flags
            .iter()
            .take_while(|f| **f & ef::NON_REST != 0)
            .count();
        let target_end = target_shape
            .element_flags
            .iter()
            .rev()
            .take_while(|f| **f & ef::NON_REST != 0)
            .count();
        let mut result = TRUE;
        for (source_position, source_type) in source_types.iter().enumerate() {
            let source_flags = source_shape.map_or(ef::REST, |s| s.element_flags[source_position]);
            let from_end = source_arity - 1 - source_position;
            let target_position = if target_rest && source_position >= target_start {
                target_arity - 1 - from_end.min(target_end)
            } else {
                if source_position >= target_arity {
                    self.report(
                        report_errors,
                        tsr_diagnostics::Target_allows_only_0_element_s_but_source_may_have_more,
                        vec![target_arity.to_string()],
                    );
                    return Ok(Some(FALSE));
                }
                source_position
            };
            let target_flags = target_shape.element_flags[target_position];
            if target_flags & ef::VARIADIC != 0 && source_flags & ef::VARIADIC == 0 {
                self.report(report_errors, tsr_diagnostics::Source_provides_no_match_for_variadic_element_at_position_0_in_target, vec![target_position.to_string()]);
                return Ok(Some(FALSE));
            }
            if source_flags & ef::VARIADIC != 0 && target_flags & ef::VARIABLE == 0 {
                self.report(report_errors, tsr_diagnostics::Variadic_element_at_position_0_in_source_does_not_match_element_at_position_1_in_target, vec![source_position.to_string(), target_position.to_string()]);
                return Ok(Some(FALSE));
            }
            if target_flags & ef::REQUIRED != 0 && source_flags & ef::REQUIRED == 0 {
                self.report(report_errors, tsr_diagnostics::Source_provides_no_match_for_required_element_at_position_0_in_target, vec![target_position.to_string()]);
                return Ok(Some(FALSE));
            }
            // With strictNullChecks and exactOptionalPropertyTypes disabled,
            // removeMissingType is identity: missingType is undefinedType and
            // the operation removes it only in exact-optional mode.
            let target_type = &target_types[target_position];
            let related = if source_flags & ef::VARIADIC != 0 && target_flags & ef::REST != 0 {
                // A variadic tuple element denotes an array/tuple as a whole.
                // Relate its numeric index type to the rest's element type.
                Self::variadic_to_rest()?
            } else {
                self.is_related_to_ex(
                    source_type,
                    target_type,
                    RECURSION_BOTH,
                    report_errors,
                    intersection_state,
                )?
            };
            if related == FALSE {
                if target_arity > 1 || source_arity > 1 {
                    if target_rest
                        && source_position >= target_start
                        && from_end >= target_end
                        && target_start as isize != source_arity as isize - target_end as isize - 1
                    {
                        self.report(report_errors, tsr_diagnostics::Type_at_positions_0_through_1_in_source_is_not_compatible_with_type_at_position_2_in_target, vec![target_start.to_string(), (source_arity as isize - target_end as isize - 1).to_string(), target_position.to_string()]);
                    } else {
                        self.report(report_errors, tsr_diagnostics::Type_at_position_0_in_source_is_not_compatible_with_type_at_position_1_in_target, vec![source_position.to_string(), target_position.to_string()]);
                    }
                }
                return Ok(Some(FALSE));
            }
            result &= related;
        }
        Ok(Some(result))
    }

    fn variadic_to_rest() -> Result<Ternary, Error> {
        // Creating the target array is semantic work in the pinned algorithm.
        // The bound frontend must provide the array factory before this path is
        // enabled; decomposing here would hide that work and its cache entries.
        unsupported("variadic tuple-to-rest array construction")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::object_flags;
    use crate::Checker;
    use crate::Graph;
    use crate::Member;
    use crate::Mode;
    use crate::Resolver;
    use crate::Structure;
    use std::cell::Cell;

    #[test]
    fn deferred_arguments_are_shared_by_shape_and_reference() {
        let graph = Rc::new(Graph::new());
        let target = graph.object("target", Vec::new());
        let instance = graph.object("A", Vec::new());
        let calls = Rc::new(Cell::new(0));
        let weak = Rc::downgrade(&graph);
        let observed = calls.clone();
        let link = TypeLink::lazy(move || {
            observed.set(observed.get() + 1);
            Ok(weak.upgrade().ok_or(Error::Released)?.string_literal(b"x"))
        });
        instance
            .set_lazy_tuple_shape(vec![link.clone()], &[element_flags::REQUIRED], false)
            .unwrap();
        instance
            .set_lazy_reference_shape(&target, vec![link])
            .unwrap();
        assert_eq!(calls.get(), 0);
        assert_eq!(instance.tuple_shape().unwrap().min_length, 1);
        assert_eq!(
            calls.get(),
            0,
            "shape access does not resolve element types"
        );
        let from_shape = instance.tuple_shape().unwrap().elements().unwrap();
        let from_reference = instance.reference_shape().unwrap().arguments().unwrap();
        assert!(Rc::ptr_eq(&from_shape[0], &from_reference[0]));
        assert_eq!(calls.get(), 1);
    }

    fn tuple(
        graph: &Graph,
        name: &str,
        elements: &[Rc<TypeCell>],
        flags: &[u8],
        readonly: bool,
    ) -> Rc<TypeCell> {
        let elements: Vec<_> = elements.iter().map(Rc::downgrade).collect();
        let shape_elements: Vec<_> = elements.iter().map(|e| e.upgrade().unwrap()).collect();
        let flags_owned = flags.to_vec();
        let resolver: Resolver = Box::new(move |_, _| {
            let members = elements
                .iter()
                .zip(&flags_owned)
                .enumerate()
                .take_while(|(_, (_, flag))| **flag & element_flags::VARIABLE == 0)
                .map(|(index, (ty, flag))| Member {
                    name_type: None,
                    name: Rc::from(index.to_string()),
                    optional: *flag == element_flags::OPTIONAL,
                    readonly,
                    class_member: true,
                    r#type: ty.clone().into(),
                })
                .collect();
            Ok(Structure {
                members,
                ..Structure::default()
            })
        });
        let cell = graph.allocate_full(
            flags::OBJECT,
            object_flags::REFERENCE,
            Rc::from(name),
            None,
            None,
            None,
            false,
            vec![],
            false,
            Some(resolver),
        );
        cell.set_tuple_shape(&shape_elements, flags, readonly)
            .unwrap();
        cell
    }

    #[test]
    fn optional_rest_tuple_is_assignable_to_readonly_wider_rest_and_reverse_reports_readonly() {
        let checker = Checker::new();
        let graph = &checker.graph;
        let string = graph.primitive(flags::STRING, "string");
        let number = graph.primitive(flags::NUMBER, "number");
        let boolean = graph.primitive(flags::BOOLEAN, "boolean");
        let undefined = graph.primitive(flags::UNDEFINED, "undefined");
        checker.register_intrinsics(&[
            string.clone(),
            number.clone(),
            boolean.clone(),
            undefined.clone(),
        ]);
        let optional_number = graph.union(&[number.clone(), undefined.clone()]).unwrap();
        let rest = graph.union(&[number, boolean.clone(), undefined]).unwrap();
        let a = tuple(
            graph,
            "A",
            &[string.clone(), optional_number, boolean],
            &[
                element_flags::REQUIRED,
                element_flags::OPTIONAL,
                element_flags::REST,
            ],
            false,
        );
        let b = tuple(
            graph,
            "readonly [string, ...(number | boolean | undefined)[]]",
            &[string, rest],
            &[element_flags::REQUIRED, element_flags::REST],
            true,
        );
        for mode in [
            Mode::Assignable,
            Mode::Subtype,
            Mode::StrictSubtype,
            Mode::Comparable,
        ] {
            assert!(checker.check_type_related_to(&a, &b, mode, true).unwrap().1);
            assert!(!checker.check_type_related_to(&b, &a, mode, true).unwrap().1);
            assert_eq!(
                checker
                    .structured_diagnostics()
                    .last()
                    .unwrap()
                    .message
                    .code,
                4104
            );
        }
        assert!(
            !checker
                .check_type_related_to(&a, &b, Mode::Identity, false)
                .unwrap()
                .1
        );
    }

    #[test]
    fn tuple_metadata_is_single_assignment_and_edges_are_weak() {
        let retained;
        {
            let graph = Graph::new();
            let number = graph.primitive(flags::NUMBER, "number");
            retained = tuple(
                &graph,
                "[number]",
                std::slice::from_ref(&number),
                &[element_flags::REQUIRED],
                false,
            );
            assert!(retained
                .set_tuple_shape(&[number], &[element_flags::REQUIRED], false)
                .is_err());
        }
        assert!(matches!(
            retained.tuple_shape().unwrap().elements(),
            Err(Error::Released)
        ));
    }
}
