//! The typed request context of `core/context.go`: request id and checker
//! lifetime, without Go's `context.Context`.
//!
//! Ports of `tsc/internal/core/context.go`, witnessed by the `concurrency` group of the Phase 1
//! operation tables (`docs/PHASE1-mutation-witnesses.md`, section 9).

/// Go's `CheckerLifetime`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum CheckerLifetime {
    #[default]
    Temporary,
    Diagnostics,
    Api,
}

/// The values `core/context.go` stores in a `context.Context`: a request id
/// and a checker lifetime, each absent until set. Go derives a new context
/// with `context.WithValue`; the setters here update a context the caller
/// owns (a clone of the parent where the parent must stay unchanged).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RequestContext {
    request_id: Option<Vec<u8>>,
    checker_lifetime: Option<CheckerLifetime>,
}

impl RequestContext {
    /// port: tsc/internal/core/context.go:WithRequestID
    pub fn with_request_id(&mut self, id: &[u8]) {
        self.request_id = Some(id.to_vec());
    }

    /// The empty id when none is set.
    /// port: tsc/internal/core/context.go:GetRequestID
    pub fn request_id(&self) -> &[u8] {
        self.request_id.as_deref().unwrap_or_default()
    }

    /// port: tsc/internal/core/context.go:WithCheckerLifetime
    pub fn with_checker_lifetime(&mut self, lifetime: CheckerLifetime) {
        self.checker_lifetime = Some(lifetime);
    }

    /// `Temporary` when none is set.
    /// port: tsc/internal/core/context.go:GetCheckerLifetime
    pub fn checker_lifetime(&self) -> CheckerLifetime {
        self.checker_lifetime.unwrap_or(CheckerLifetime::Temporary)
    }
}
