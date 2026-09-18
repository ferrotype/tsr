# ADR 0008: Checker mutation model

Status: Accepted (2026-09-05)
Plan: section 6, decision 3; section 13, item 7

## Context

The Go checker mutates anything through `*Checker` while holding pointers into its own graph. Rust cannot hold a borrow into an arena across a recursive `&mut self` call.

## Decision

The checker is a single-threaded state machine that takes `&mut self` and never holds a borrow into its arenas across a call: accessors copy small values out, scalar lazily-resolved fields use `Cell`, collections are addressed by id. Type, parameter and type-argument lists are built mutably and published as independently owned immutable handles (`Arc<[TypeId]>`, since the checker is `Send`). Lazy resolution keeps Corsa's `pushTypeResolution` and `popTypeResolution` reentrancy guard. `isRelatedTo`, instantiation, inference and control flow keep Corsa's structure, and keep its order of side effects wherever ADR 0010 lists the output as order-sensitive.

## Consequences

The port ledger depends on Go and Rust having the same shape so upstream diffs transplant. The arena-references-with-interior-mutability alternative (rustc's pattern) is prototyped on the relater core in the spike so that it is a measured fallback, not an assumed one. Rejected: `RefCell` on every table; splitting the checker into pure passes.

The fallback was measured in S08 P7 and loses on every axis. Both implementations reach strict parity with the pinned observations over the 21 frozen relater fixtures in all five modes, so the comparison is identical work. Against the `&mut self` and id design the alternative is 28% slower in the relation interval (bootstrap 95% interval 1.08 to 1.33, seven fresh-process samples per implementation), requests 61% more bytes during relations and retains 2.44 times as much at the declared live-root checkpoint. The memory figures are identical across all seven samples. Per-cell reference counts and the `Weak` edges that keep the graph acyclic are the structural cost. The decision stands and the prototype is not extended further; evidence is the `relater` record in `status/evidence`.

## Evidence

`tsc/internal/checker/checker.go` (`getTypeArguments` and the resolution stack), `tsc/internal/checker/relater.go`.
