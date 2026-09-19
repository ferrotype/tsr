//! An embedding session owns one immutable program and one lazy checker.
//!
//! Supply a filesystem explicitly through [`ProgramOptions`]; an in-memory
//! snapshot requires no OS services. Query results that outlive an operation
//! must use the checker's `retain_*` methods. Such results keep their program
//! alive after the session is dropped. [`Session::retire`] instead invalidates
//! subsequent operations, including operations through retained results.

use std::sync::{Arc, OnceLock};
use tsr_arena::{CheckerIdentity, Counters, Generation};
use tsr_checker::{CheckerOwner, Error, Operation};
pub use tsr_compiler::{FileCache, Program, ProgramOptions};

/// Cheap, explicitly retained root for a loaded program and its checker.
/// Initialization occurs on the first query, under the checker's identity
/// permit. Reentry fails before waiting on the initialization cell. Failed
/// initialization is remembered; a panic retires the generation via the permit.
pub struct Session {
    program: Arc<Program>,
    identity: Arc<CheckerIdentity>,
    counters: Counters,
    checker: OnceLock<Result<Arc<CheckerOwner>, Error>>,
}

impl Session {
    /// Load through the supplied host. The caller controls cache reuse and its
    /// lifetime. Its weak entries do not retain otherwise unreferenced files.
    pub fn load(
        options: ProgramOptions,
        cache: &mut FileCache,
        counters: &Counters,
    ) -> Result<Self, tsr_compiler::Error> {
        Ok(Self::from_program(
            Arc::new(Program::load(options, cache, counters)?),
            counters,
        ))
    }

    /// Adopt an already loaded program without copying its graph. A fresh
    /// checker identity prevents results from another session from aliasing.
    pub fn from_program(program: Arc<Program>, counters: &Counters) -> Self {
        Self {
            program,
            identity: CheckerIdentity::new(Generation::new(counters), counters),
            counters: counters.clone(),
            checker: OnceLock::new(),
        }
    }

    pub fn program(&self) -> &Arc<Program> {
        &self.program
    }

    pub fn checker(&self) -> Result<&Arc<CheckerOwner>, Error> {
        if let Some(result) = self.checker.get() {
            return result.as_ref().map_err(|error| *error);
        }
        // Acquire before entering OnceLock: a host callback that reenters the
        // same session receives Reentry instead of waiting on its own cell.
        let _initialization = self.identity.lease()?;
        self.checker
            .get_or_init(|| {
                CheckerOwner::for_program(
                    self.identity.clone(),
                    &self.counters,
                    Arc::new(tsr_compiler::ProgramCheckerHost::new(self.program.clone())),
                )
                .map(Arc::new)
            })
            .as_ref()
            .map_err(|error| *error)
    }

    /// Queries borrow this scope; mutable checker storage never escapes it.
    pub fn operation(&self) -> Result<Operation<'_>, Error> {
        self.checker()?.operation()
    }

    /// Explicit cancellation is distinct from dropping a retention root.
    pub fn retire(&self) {
        self.identity.generation().retire();
    }
}
