//! Signature arity and parameter-position access from pinned `relater.go`.

use super::*;

impl Signature {
    // port: tsc/internal/checker/relater.go:Checker.isTopSignature
    pub(super) fn is_top_signature(&self) -> Result<bool, Error> {
        if self.type_parameters != 0 || self.parameters.len() != 1 || !self.has_rest_parameter {
            return Ok(false);
        }
        if self
            .this_type()?
            .is_some_and(|ty| ty.flags & flags::ANY == 0)
        {
            return Ok(false);
        }
        let parameter = self.parameter(0)?.ok_or(Error::ResolutionFailed)?;
        let rest = match parameter.array_element() {
            Some(array) => array.element()?,
            None => parameter,
        };
        Ok(rest.flags & (flags::ANY | flags::NEVER) != 0
            && self.return_type()?.flags & (flags::ANY | flags::UNKNOWN) != 0)
    }

    fn rest_type(&self) -> Result<Option<Rc<TypeCell>>, Error> {
        if !self.has_rest_parameter {
            return Ok(None);
        }
        self.parameter(
            self.parameters
                .len()
                .checked_sub(1)
                .ok_or(Error::ResolutionFailed)?,
        )
    }

    // port: tsc/internal/checker/relater.go:Checker.getParameterCount
    pub(super) fn effective_parameter_count(&self) -> Result<usize, Error> {
        let length = self.parameters.len();
        if let Some(rest) = self.rest_type()? {
            if let Some(tuple) = rest.tuple_shape() {
                let fixed = tuple
                    .element_flags
                    .iter()
                    .position(|flags| flags & element_flags::VARIABLE != 0)
                    .unwrap_or(tuple.element_flags.len());
                return Ok(length + fixed
                    - usize::from(tuple.combined_flags & element_flags::VARIABLE == 0));
            }
        }
        Ok(length)
    }

    // port: tsc/internal/checker/relater.go:Checker.hasEffectiveRestParameter
    pub(super) fn has_effective_rest_parameter(&self) -> Result<bool, Error> {
        Ok(self.rest_type()?.is_some_and(|rest| {
            rest.tuple_shape()
                .is_none_or(|tuple| tuple.combined_flags & element_flags::VARIABLE != 0)
        }))
    }

    pub(super) fn has_non_array_rest_type(&self) -> Result<bool, Error> {
        let Some(rest) = self.rest_type()? else {
            return Ok(false);
        };
        if let Some(tuple) = rest.tuple_shape() {
            return Ok(tuple.combined_flags & element_flags::VARIABLE != 0);
        }
        Ok(rest.flags & flags::ANY == 0 && rest.array_element().is_none())
    }
}

impl Relater<'_> {
    // port: tsc/internal/checker/relater.go:Checker.tryGetTypeAtPosition
    pub(super) fn try_signature_type_at_position(
        &self,
        signature: &Signature,
        position: usize,
    ) -> Result<Option<Rc<TypeCell>>, Error> {
        let fixed = signature.parameters.len() - usize::from(signature.has_rest_parameter);
        if position < fixed {
            return signature.parameter(position);
        }
        let Some(rest) = signature.rest_type()? else {
            return Ok(None);
        };
        let index = position - fixed;
        if let Some(tuple) = rest.tuple_shape() {
            if tuple.combined_flags & element_flags::VARIABLE == 0
                && index >= tuple.element_flags.len()
            {
                return Ok(None);
            }
        }
        // The native indexed access constructs the numeric literal even for an
        // array/any index. Its interning is real checker work, not a counter.
        self.graph().intern_literal(
            flags::NUMBER_LITERAL,
            LiteralValue::Number((index as f64).to_bits()),
            &index.to_string(),
        );
        if rest.flags & flags::ANY != 0 {
            return Ok(Some(rest));
        }
        if let Some(array) = rest.array_element() {
            return array.element().map(Some);
        }
        if let Some(tuple) = rest.tuple_shape() {
            let elements = tuple.elements()?;
            if let Some(ty) = elements.get(index) {
                if tuple.element_flags[index] & element_flags::VARIADIC != 0 {
                    return unsupported("signature variadic tuple indexed access");
                }
                return Ok(Some(ty.clone()));
            }
            if let Some(last) = tuple.element_flags.last() {
                if last & element_flags::REST != 0 {
                    return Ok(elements.last().cloned());
                }
            }
        }
        unsupported("signature rest indexed access")
    }

    // port: tsc/internal/checker/relater.go:Checker.getMinArgumentCountEx
    pub(super) fn min_argument_count(&self, signature: &Signature) -> Result<usize, Error> {
        let mut count = signature.min_argument_count;
        if let Some(rest) = signature.rest_type()? {
            if let Some(tuple) = rest.tuple_shape() {
                let required = tuple
                    .element_flags
                    .iter()
                    .position(|flags| flags & element_flags::REQUIRED == 0)
                    .unwrap_or(tuple.element_flags.len());
                if required > 0 {
                    count = signature.parameters.len() - 1 + required;
                }
            }
        }
        while count > 0 {
            let Some(ty) = self.try_signature_type_at_position(signature, count - 1)? else {
                break;
            };
            let contains_void = if ty.flags & flags::UNION != 0 {
                ty.types()?.iter().any(|part| part.flags & flags::VOID != 0)
            } else {
                ty.flags & flags::VOID != 0
            };
            if !contains_void {
                break;
            }
            count -= 1;
        }
        Ok(count)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn signature(parameters: &[Rc<TypeCell>], rest: bool, return_type: &Rc<TypeCell>) -> Signature {
        Signature {
            parameters: parameters
                .iter()
                .map(|ty| Rc::downgrade(ty).into())
                .collect(),
            parameter_names: (0..parameters.len())
                .map(|i| Rc::from(format!("p{i}")))
                .collect(),
            min_argument_count: parameters.len() - usize::from(rest),
            has_rest_parameter: rest,
            type_parameters: 0,
            generic: None,
            this_type: None,
            return_type: Rc::downgrade(return_type).into(),
            bivariant_parameters: false,
            is_abstract: false,
            is_construct: false,
        }
    }

    #[test]
    fn top_signatures_short_circuit_before_rest_indexing() {
        let graph = Graph::new();
        let any = graph.primitive(flags::ANY, "any");
        let unknown = graph.primitive(flags::UNKNOWN, "unknown");
        let string = graph.primitive(flags::STRING, "string");
        let array = graph.object("any[]", vec![]);
        array.set_array_element(&any, false).unwrap();
        assert!(signature(&[any.clone()], true, &any)
            .is_top_signature()
            .unwrap());
        assert!(signature(&[array], true, &unknown)
            .is_top_signature()
            .unwrap());
        assert!(!signature(&[string], true, &any).is_top_signature().unwrap());
        assert!(!signature(&[any.clone()], false, &any)
            .is_top_signature()
            .unwrap());
    }

    #[test]
    fn fixed_tuple_rest_expands_arity_without_becoming_variadic() {
        let graph = Graph::new();
        let string = graph.primitive(flags::STRING, "string");
        let tuple = graph.object("[string,string?]", vec![]);
        tuple
            .set_tuple_shape(
                &[string.clone(), string.clone()],
                &[element_flags::REQUIRED, element_flags::OPTIONAL],
                false,
            )
            .unwrap();
        let signature = signature(&[string.clone(), tuple], true, &string);
        assert_eq!(signature.effective_parameter_count().unwrap(), 3);
        assert!(!signature.has_effective_rest_parameter().unwrap());
    }
}
