//! The checker-pool interface both pools implement (docs/PHASE2-C6-plan.md,
//! C6.3): the compiler's (`tsr_compiler::CompilerCheckerPool`, the pin's
//! `compiler/checkerpool.go`) and the project system's (`tsr_project`, the
//! pin's `project/checkerpool.go`).
use crate::{Error, Operation};
use tsr_arena::NodeId;

/// The lifetime a request asks a pool for, which the pin's context carries
/// (`core.CheckerLifetime`). The compiler pool ignores it; the project pool
/// picks its diagnostics, query or API checker by it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CheckerLifetime {
    /// A request-scoped query checker (`CheckerLifetimeTemporary`).
    #[default]
    Temporary,
    Diagnostics,
    /// The persistent checker of the API (`CheckerLifetimeAPI`).
    Api,
}

/// What the pin's context carries into a program's checker requests: the
/// lifetime a pool serves them with (`core.GetCheckerLifetime(ctx)`) and their
/// cancellation (the context `checkSourceFile` polls). The default is the
/// harness's `context.Background()`: a temporary checker, never canceled.
#[derive(Clone, Debug, Default)]
pub struct CheckerRequest {
    pub lifetime: CheckerLifetime,
    pub cancellation: Option<tsr_core::CancellationToken>,
}

/// The pin's `CheckerPool` interface: `GetChecker(ctx, file)` returns a
/// checker held exclusively by the caller and a release the caller calls
/// once; with a file, the pool may use it as an affinity hint. Here the hold
/// is the checker's [`Operation`] and it is scoped: `with_checker` runs
/// `task` holding it and releases it once, when `task` returns or unwinds.
pub trait CheckerPool: Send + Sync {
    fn with_checker(
        &self,
        lifetime: CheckerLifetime,
        file: Option<NodeId>,
        task: &mut dyn FnMut(&mut Operation<'_>) -> Result<(), Error>,
    ) -> Result<(), Error>;
}
