# ADR 0016: Toolchain and lints

Status: Accepted (2026-09-05)
Plan: section 6, decision 12; section 13, item 14

## Context

Corsa builds without cgo on stable Go with seven custom analyzers. Rust needs a comparable baseline of hygiene without pinning the compiler to a nightly toolchain.

## Decision

Stable Rust with a minimum supported version one behind current stable; `clippy` at pedantic minus an allow-list; `rustfmt`; `cargo nextest`; `lld` or `mold` plus `sccache` in CI; `#![forbid(unsafe_code)]` everywhere except the leaf crates where Corsa uses `unsafe` today (file watching, paths, platform FFI). Release profile: `panic = "unwind"` (required by ADR 0012), fat LTO, one codegen unit, mimalloc. Any compiler-private lint tool has its own pinned toolchain and does not change the compiler's stable-build requirement.

## Consequences

The replacement for upstream's `checkchildren` analyzer uses the compiler-private tool described below; `forbidparentaccess` becomes structural with the declarations transform exempt, three analyzers are Go-specific and moot, and two map to rustc and clippy.

## Evidence

`.golangci.yml` and `tools/customlint/*.go` in the pinned checkout; upstream CI runs with `CGO_ENABLED=0`.

## Child-check analyzer (Phase 7, 2026-10-11)

`tools/phase7/checkchildren` uses `rustc_driver` on the existing instrumentation
nightly, with `rustc-dev`. It has no dependency in published compiler crates.
It resolves the two checker dispatch workers, inspects their direct handlers'
explicit early returns and follows the pin's lexical child-check successor rule.
Typed Rust `Option` absence and `Result::Err` returns handle the port's fallible
API. Expanded returns and indirect child traversal remain outside this analysis.

Eight planted omissions, repaired controls, fake/real constructor identities,
per-return allowance controls and Cargo cache replay are exercised before the
checker. The local checker run covers 104 dispatched methods with zero findings
and six reviewed, reason-bearing allowances. Those describe verified limitations
of the analysis, not permission to omit checks. CI repeats the controls and
analysis; the standalone README records exact scope and reproduction.
