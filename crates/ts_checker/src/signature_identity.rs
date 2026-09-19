//! One signature-identity algorithm with the caller-selected type comparer.
use crate::{ternary as tr, CheckerState, Error, SignatureId, Ternary, TypeId};

impl CheckerState {
    // port: tsc/internal/checker/relater.go:Checker.compareSignaturesIdentical
    pub(crate) fn compare_signatures_identical(
        &mut self,
        mut source: SignatureId,
        target: SignatureId,
        partial: bool,
        ignore_this: bool,
        ignore_return: bool,
        compare: &mut impl FnMut(&mut CheckerState, TypeId, TypeId) -> Result<Ternary, Error>,
    ) -> Result<Ternary, Error> {
        if source == target {
            return Ok(tr::TRUE);
        }
        let source_count = self.parameter_count(source)?;
        let target_count = self.parameter_count(target)?;
        let source_min = self.min_argument_count(source)?;
        let target_min = self.min_argument_count(target)?;
        let source_rest = self.effective_rest_parameter(source)?;
        let target_rest = self.effective_rest_parameter(target)?;
        if !(source_count == target_count && source_min == target_min && source_rest == target_rest
            || partial && source_min <= target_min)
        {
            return Ok(tr::FALSE);
        }
        let source_parameters = self
            .signatures
            .get(source)?
            .type_parameters
            .clone()
            .unwrap_or_else(|| [].into());
        let target_parameters = self
            .signatures
            .get(target)?
            .type_parameters
            .clone()
            .unwrap_or_else(|| [].into());
        if source_parameters.len() != target_parameters.len() {
            return Ok(tr::FALSE);
        }
        if !target_parameters.is_empty() {
            let mapper = self.new_type_mapper(&source_parameters, &target_parameters)?;
            for (&s, &t) in source_parameters.iter().zip(target_parameters.iter()) {
                if s == t {
                    continue;
                }
                let sc = self
                    .constraint_of_type_parameter(s)?
                    .unwrap_or(self.builtins.unknown_type);
                let sc = self.instantiate_type(sc, Some(mapper))?;
                let tc = self
                    .constraint_of_type_parameter(t)?
                    .unwrap_or(self.builtins.unknown_type);
                if compare(self, sc, tc)? == tr::FALSE {
                    return Ok(tr::FALSE);
                }
                let sd = self.default_or_unknown(s)?;
                let sd = self.instantiate_type(sd, Some(mapper))?;
                let td = self.default_or_unknown(t)?;
                if compare(self, sd, td)? == tr::FALSE {
                    return Ok(tr::FALSE);
                }
            }
            source = self.instantiate_signature_ex(source, mapper, true)?;
        }
        let mut result = tr::TRUE;
        if !ignore_this {
            if let (Some(s), Some(t)) = (
                self.signatures.get(source)?.this_parameter,
                self.signatures.get(target)?.this_parameter,
            ) {
                let s = self.get_type_of_symbol(s)?;
                let t = self.get_type_of_symbol(t)?;
                result &= compare(self, s, t)?;
            }
        }
        if result == tr::FALSE {
            return Ok(result);
        }
        for position in 0..self.parameter_count(target)? {
            let s = self
                .parameter_type_at(source, position)?
                .unwrap_or(self.builtins.any_type);
            let t = self
                .parameter_type_at(target, position)?
                .unwrap_or(self.builtins.any_type);
            result &= compare(self, t, s)?;
            if result == tr::FALSE {
                return Ok(result);
            }
        }
        if !ignore_return {
            let s = self.type_predicate_of_signature(source)?;
            let t = self.type_predicate_of_signature(target)?;
            match (s, t) {
                (Some(s), Some(t)) => {
                    let s = self.signatures.predicate(s)?.clone();
                    let t = self.signatures.predicate(t)?.clone();
                    if s.kind != t.kind || s.parameter_index != t.parameter_index {
                        return Ok(tr::FALSE);
                    }
                    result &= match (s.t, t.t) {
                        (Some(s), Some(t)) => compare(self, s, t)?,
                        (None, None) => tr::TRUE,
                        _ => tr::FALSE,
                    };
                }
                (None, None) => {
                    let s = self.return_type_of_signature(source)?;
                    let t = self.return_type_of_signature(target)?;
                    result &= compare(self, s, t)?;
                }
                _ => return Ok(tr::FALSE),
            }
        }
        Ok(result)
    }
}
