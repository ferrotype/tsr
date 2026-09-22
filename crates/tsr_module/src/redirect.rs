//! A borrowed project-reference view and an exclusive resolution options scope.
use crate::{ParsedPatterns, Resolver};
use std::sync::{Arc, OnceLock};
use tsr_core::CompilerOptions;

/// Rust value view of the pinned ResolvedProjectReference interface. An absent
/// options value retains the base options but still participates in cache keys.
#[derive(Clone, Copy)]
pub struct ResolvedProjectReference<'a> {
    pub config_name: &'a [u8],
    pub compiler_options: Option<&'a CompilerOptions>,
}
/// port: tsc/internal/module/resolver.go:GetCompilerOptionsWithRedirect
pub fn compiler_options_with_redirect<'a>(
    base: &'a CompilerOptions,
    reference: Option<ResolvedProjectReference<'a>>,
) -> &'a CompilerOptions {
    reference.and_then(|r| r.compiler_options).unwrap_or(base)
}

// A redirect cannot mutate the caller's owned options or another resolver.
// Restoring in Drop covers both returned errors and native panic unwinding.
struct OptionsScope<'a> {
    resolver: &'a mut Resolver,
    saved: Arc<CompilerOptions>,
    saved_base: Option<Arc<CompilerOptions>>,
    patterns: OnceLock<ParsedPatterns>,
}
impl Drop for OptionsScope<'_> {
    fn drop(&mut self) {
        self.resolver.options = self.saved.clone();
        self.resolver.operation_base_options = self.saved_base.take();
        self.resolver.option_patterns = std::mem::take(&mut self.patterns);
    }
}
impl Resolver {
    pub(super) fn with_redirect<T>(
        &mut self,
        reference: Option<ResolvedProjectReference<'_>>,
        action: impl FnOnce(&mut Self) -> T,
    ) -> T {
        let Some(options) = reference.and_then(|r| r.compiler_options) else {
            return action(self);
        };
        let saved = std::mem::replace(&mut self.options, Arc::new(options.clone()));
        let saved_base = self.operation_base_options.replace(saved.clone());
        let patterns = std::mem::take(&mut self.option_patterns);
        let scope = OptionsScope {
            resolver: self,
            saved,
            saved_base,
            patterns,
        };
        action(scope.resolver)
    }
    /// port: tsc/internal/module/resolver.go:tracer.traceResolutionUsingProjectReference
    pub(super) fn trace_redirect(&mut self, reference: Option<ResolvedProjectReference<'_>>) {
        if let Some(reference) = reference.filter(|r| r.compiler_options.is_some()) {
            crate::trace::trace!(
                self,
                tsr_diagnostics::Using_compiler_options_of_project_reference_redirect_0,
                reference.config_name
            );
        }
    }
}
