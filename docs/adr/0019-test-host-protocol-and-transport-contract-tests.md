# ADR 0019: Test-host protocol and transport contract tests

Status: Proposed (2026-09-05)
Plan: section 8 (oracle seams) and section 14, step 6
Sprint: S11

## Context

The Go fourslash harness injects an in-memory filesystem, symlinks and configurable case sensitivity, a parse cache and plugin spawners, inferred-project options and an initialization signal. Semantic fourslash coverage belongs to Phase 5, but the transport that carries those injections has to be designed before the language service exists, so that Phase 5 connects a tested endpoint instead of inventing one.

## Decision

Proposed for owner review: use the versioned `ts_testhost --stdio` endpoint and
the contract in [S11](../S11.md). Valid messages use Content-Length JSON-RPC
framing. A bounded, single-threaded router continues servicing cancellation,
progress and unrelated responses while reverse callbacks are outstanding.

Use pinned Go callbackFS semantics for the five read operations, including the
null/delegate versus missing/empty distinctions. Fallback is restricted to an
explicitly injected immutable filesystem. Accept strict Unicode JSON strings;
do not silently replace arbitrary source bytes. Initialization and inferred
options use a configuration callback as an explicit completion barrier. Reject
locally-known size failures before dispatch. Configuration errors guarantee no
applied change (rollback must finish before the error). After cancellation, a
late success or an unrecognized result retires the session instead of resuming
with old local state. A malformed configuration acknowledgment also retires it.

Retain opaque JSON numeric tokens without machine-float conversion. Bound
forwarded progress before writing; overflow fails its owning operation while
unrelated work remains live.

Register plugin names/options at initialization and proxy their spawn,
initialize, project, transform and disposal requests over the same connection.
The test client owns plugin processes and project handles; this binary does not
execute external code. Cancellation retires an affected plugin name, and the
client must clean up its resources, including late spawn results.

Carry small access-only Go oracle overlays against the pin rather than fork the
fourslash harness. Compare actual upstream callbackFS observations and actual
framed mapper responses with the Rust subprocess transport. Report new control
and malformed-protocol cases separately from Go observations. These tests do
not certify project integration, mapper semantics in a compiler, or language
service results. The existing Go fixtures remain unchanged.

## Consequences

Until accepted, S11 stays open. Acceptance requires the owner's review of the design note; semantic fourslash assertions remain deferred to Phase 5 and are not counted as coverage by this ADR.

## Evidence

`tsc/internal/api/callbackfs.go`, `tsc/internal/jsonrpc/baseproto.go`,
`tsc/internal/contentmapper/hostimpl.go`, `tsc/internal/fourslash/fourslash.go`,
and `tsc/internal/testutil/contentmappertest` in the pinned checkout.
The frozen inventories are under `data/s11/`, access-only bridges under
`tools/s11/`, and the producer is `cargo xtask run testhost`.
