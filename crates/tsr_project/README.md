# tsr_project

Checker ownership, immutable editor/file snapshots and shared parse caches for tsr.

Phase 5 provides configured/inferred sessions, immutable overlays and disk
snapshots, configuration ownership, parse caches, checker scheduling, project
watches and controlled update/idle timers. The transport-facing document layer
lives in `tsr_lsp`. Project-tree requests discover unopened consumers, while
API project/file opens retain their own references independently of editor
buffers. Sessions also own automatic type acquisition and content-mapper hosts;
old snapshots retain programs and mapper leases across updates.

Checker-slot waits are cancellation-aware: a canceled waiter returns without
acquiring or creating a checker. The pinned Go scheduler waits on its semaphore
without observing cancellation, then lets the checker observe it. This deliberate
difference lets abandoned requests leave the wait promptly, but can change which
slots later requests use and their type-allocation order. Request affinity itself
preserves a file's existing checker association, as in Go.

Part of [tsr](https://github.com/ferrotype/tsr), a Rust port of
the TypeScript compiler. This project is under development; the API and supported
compiler behavior are not stable. See the repository status and sprint records
for current coverage.

Requires **Rust 1.96 or newer**.

Licensed under Apache-2.0. See [LICENSE](LICENSE) and [NOTICE](NOTICE) for
license and attribution information.

For application integration, start with
[`tsr_embed`](https://github.com/ferrotype/tsr/tree/main/crates/tsr_embed).
