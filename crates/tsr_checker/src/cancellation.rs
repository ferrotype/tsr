//! Cancellation (docs/PHASE2-C6-plan.md, C6.5).
//!
//! The pin's `checkSourceFile` takes a `context.Context`, keeps it for the
//! duration of the check and polls it per statement (`checkSourceElements`),
//! per deferred node (`checkDeferredNodes`), per property of a contextual
//! deprecation check and before the unused-identifier passes. A check that
//! observes the cancellation still finishes its bookkeeping (the file is
//! marked checked), sets `wasCanceled`, which never clears, and returns no
//! diagnostics. From then on the checker refuses diagnostics and node
//! building (`checkNotCanceled`): the pin panics with `Checker was previously
//! cancelled`. Here the refusal is [`Error::PreviouslyCanceled`], so it never
//! unwinds through the operation and a canceled checker is poisoned without
//! its generation being retired; cancellation and panic retirement stay
//! distinct contracts.
use crate::{CheckerState, Error, Operation};
use tsr_arena::NodeId;
use tsr_core::CancellationToken;

impl CheckerState {
    // port: tsc/internal/checker/utilities.go:Checker.isCanceled
    pub(crate) fn is_canceled(&self) -> bool {
        self.cancellation
            .as_ref()
            .is_some_and(CancellationToken::is_canceled)
    }

    // port: tsc/internal/checker/utilities.go:Checker.checkNotCanceled
    pub(crate) fn check_not_canceled(&self) -> Result<(), Error> {
        if self.was_canceled {
            return Err(Error::PreviouslyCanceled);
        }
        Ok(())
    }

    // port: tsc/internal/checker/checker.go:Checker.checkSourceElements
    pub(crate) fn check_source_elements(
        &mut self,
        nodes: impl IntoIterator<Item = NodeId>,
    ) -> Result<(), Error> {
        for node in nodes {
            if self.is_canceled() {
                break;
            }
            self.check_source_element(node)?;
        }
        Ok(())
    }

    // port: tsc/internal/checker/checker.go:Checker.checkSourceFile
    /// `checkSourceFile(ctx, sourceFile, checkUnused)`: the token is the
    /// context for the duration of the check; a canceled check leaves the
    /// checker canceled.
    pub(crate) fn check_source_file_in(
        &mut self,
        source: NodeId,
        check_unused: bool,
        token: Option<&CancellationToken>,
    ) -> Result<(), Error> {
        self.cancellation = token.cloned();
        let result = self.check_source_file_ex(source, check_unused);
        if self.is_canceled() {
            self.was_canceled = true;
        }
        self.cancellation = None;
        result
    }

    // port: tsc/internal/checker/checker.go:Checker.getDiagnostics
    pub(crate) fn diagnostics_in(
        &mut self,
        source: NodeId,
        token: Option<&CancellationToken>,
        suggestions: bool,
    ) -> Result<Vec<tsr_ast::Diagnostic>, Error> {
        self.check_not_canceled()?;
        let options = self.program()?.host.options();
        let check_unused = suggestions
            || options.no_unused_locals == tsr_core::Tristate::TRUE
            || options.no_unused_parameters == tsr_core::Tristate::TRUE;
        self.check_source_file_in(source, check_unused, token)?;
        if self.was_canceled {
            return Ok(Vec::new());
        }
        let collected = if suggestions {
            self.suggestions_for_file(Some(source))?
        } else {
            self.diagnostics_for_file(Some(source))?
        };
        Ok(collected.into_iter().cloned().collect())
    }
}

impl Operation<'_> {
    /// `GetSemanticDiagnostics(ctx, file)` with a cancellable context: no
    /// diagnostics when the check was canceled, and
    /// [`Error::PreviouslyCanceled`] once the checker has been.
    pub fn semantic_diagnostics_cancellable(
        &mut self,
        source: NodeId,
        token: &CancellationToken,
    ) -> Result<Vec<tsr_ast::Diagnostic>, Error> {
        let result = self.state_mut().diagnostics_in(source, Some(token), false);
        self.publish_canceled();
        result
    }

    /// `GetSuggestionDiagnostics(ctx, file)` with a cancellable context.
    pub fn suggestion_diagnostics_cancellable(
        &mut self,
        source: NodeId,
        token: &CancellationToken,
    ) -> Result<Vec<tsr_ast::Diagnostic>, Error> {
        let result = self.state_mut().diagnostics_in(source, Some(token), true);
        self.publish_canceled();
        result
    }

    /// Contract-only: the diagnostics recorded for `source` so far, read
    /// without checking the file and past the canceled-checker refusal, so a
    /// contract can observe which passes a canceled check ran.
    #[cfg(feature = "recursion-probe")]
    pub fn recorded_diagnostics_probe(
        &mut self,
        source: NodeId,
    ) -> Result<Vec<tsr_ast::Diagnostic>, Error> {
        Ok(self
            .state_mut()
            .diagnostics_for_file(Some(source))?
            .into_iter()
            .cloned()
            .collect())
    }

    fn publish_canceled(&self) {
        if self.state().was_canceled {
            self.owner().mark_canceled();
        }
    }
}
