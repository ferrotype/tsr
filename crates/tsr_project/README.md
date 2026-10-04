# tsr_project

Checker ownership, immutable editor/file snapshots and shared parse caches for tsr.

Phase 5 provides configured/inferred sessions, immutable overlays and disk
snapshots, configuration ownership, parse caches, checker scheduling, project
watches and controlled update/idle timers. The transport-facing document layer
lives in `tsr_lsp`; language services and cross-project features follow in the
later Phase 5 steps.

Part of [tsr](https://github.com/ferrotype/tsr), a Rust port of
the TypeScript compiler. This project is under development; the API and supported
compiler behavior are not stable. See the repository status and sprint records
for current coverage.

Requires **Rust 1.96 or newer**.

Licensed under Apache-2.0. See [LICENSE](LICENSE) and [NOTICE](NOTICE) for
license and attribution information.

For application integration, start with
[`tsr_embed`](https://github.com/ferrotype/tsr/tree/main/crates/tsr_embed).
