use crate::relater::{Relater, BOTH};
use crate::{
    ternary as tr, type_flags as tf, variance_flags as vf, Error, RelationKind, Ternary, TypeId,
    VarianceFlags,
};

#[derive(Default)]
pub(crate) struct VarianceCheck {
    pub failed: bool,
    pub original_chain: Option<Vec<crate::relation_errors::ErrorEntry>>,
}
impl Relater<'_> {
    // port: tsc/internal/checker/relater.go:Relater.structuredTypeRelatedToWorker
    /// `None` requests structural comparison. A failed invariant check still
    /// forces False after that comparison, using its more specific error.
    pub(crate) fn relate_variances(
        &mut self,
        sources: &[TypeId],
        targets: &[TypeId],
        variances: &[VarianceFlags],
        intersection: u32,
        saved: &crate::relation_errors::RelationErrors,
        state: &mut VarianceCheck,
    ) -> Result<Option<Ternary>, Error> {
        let result = self.type_arguments_related(sources, targets, variances, intersection)?;
        if result != tr::FALSE {
            return Ok(Some(result));
        }
        if variances
            .iter()
            .any(|v| v & vf::ALLOWS_STRUCTURAL_FALLBACK != 0)
        {
            state.original_chain = None;
            self.errors = saved.clone();
            return Ok(None);
        }
        let mut allow_structural = false;
        for (&target, &variance) in targets.iter().zip(variances) {
            allow_structural |= variance & vf::VARIANCE_MASK == vf::COVARIANT
                && self.checker.types.flags(target)? & tf::VOID != 0;
        }
        state.failed = !allow_structural;
        if !variances.is_empty() && !allow_structural {
            if !(self.report_errors
                && variances
                    .iter()
                    .any(|v| v & vf::VARIANCE_MASK == vf::INVARIANT))
            {
                return Ok(Some(tr::FALSE));
            }
            state.original_chain = Some(self.errors.chain.clone());
            self.errors = saved.clone();
        }
        Ok(None)
    }
    // port: tsc/internal/checker/relater.go:Relater.typeArgumentsRelatedTo
    fn type_arguments_related(
        &mut self,
        sources: &[TypeId],
        targets: &[TypeId],
        variances: &[VarianceFlags],
        intersection: u32,
    ) -> Result<Ternary, Error> {
        if sources.len() != targets.len() && self.kind == RelationKind::Identity {
            return Ok(tr::FALSE);
        }
        let mut result = tr::TRUE;
        for (index, (&source, &target)) in sources.iter().zip(targets).enumerate() {
            let flags = variances.get(index).copied().unwrap_or(vf::COVARIANT);
            let variance = flags & vf::VARIANCE_MASK;
            if variance == vf::INDEPENDENT {
                continue;
            }
            let related = if flags & vf::UNMEASURABLE != 0 {
                if self.kind == RelationKind::Identity {
                    self.related_with_errors(source, target, BOTH, 0, false)?
                } else if self
                    .checker
                    .is_type_related_to(source, target, RelationKind::Identity)?
                {
                    tr::TRUE
                } else {
                    tr::FALSE
                }
            } else {
                if !self.checker.variance.stack.is_empty() && flags & vf::UNRELIABLE != 0 {
                    self.checker.report_unreliable_markers(source, false)?;
                }
                match variance {
                    vf::COVARIANT => self.related(source, target, BOTH, intersection)?,
                    vf::CONTRAVARIANT => self.related(target, source, BOTH, intersection)?,
                    vf::BIVARIANT => {
                        let r = self.related_with_errors(target, source, BOTH, 0, false)?;
                        if r == tr::FALSE {
                            self.related(source, target, BOTH, intersection)?
                        } else {
                            r
                        }
                    }
                    _ => {
                        let r = self.related(source, target, BOTH, intersection)?;
                        if r == tr::FALSE {
                            r
                        } else {
                            r & self.related(target, source, BOTH, intersection)?
                        }
                    }
                }
            };
            if related == tr::FALSE {
                return Ok(tr::FALSE);
            }
            result &= related;
        }
        Ok(result)
    }
}
