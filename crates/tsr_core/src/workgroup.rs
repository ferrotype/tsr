//! `core.ThrottleGroup` over scoped threads with a bounded permit count.
//!
//! Ports of `tsc/internal/core/workgroup.go`, witnessed by the `concurrency` group of the Phase 1
//! operation tables (`docs/PHASE1-mutation-witnesses.md`, section 9).
