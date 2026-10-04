# L2 server and conversion checks

L2 connects the generated protocol to production project sessions and compiler
diagnostics. `tsrust --lsp --stdio` and the private version-3 endpoint use the
same runtime. Later service methods return a named `-32601` error; their pinned
capabilities remain declared so the protocol surface does not drift during the
port. This is not a complete editor server yet.

## Reproduce

Put the Go version in `data/s04/toolchains.toml` on PATH, then run:

```sh
cargo test -p tsr_ls -p tsr_lsp -p tsr_project -p tsr_testhost --lib
cargo test -p tsrust --bin tsrust lsp::tests
cargo build -p tsrust -p tsr_testhost --bin tsrust --bin phase5_testserver
python3 tools/phase5/lsp/check.py
python3 tools/phase5/lsp/interop.py
python3 tools/phase5/lsp/review_cases.py
```

The native watcher test requires ordinary host filesystem notification access.
It runs on fast-recursive backends (FSEvents on the development Mac); Linux
uses the fake-backend tests because the pinned LSP fallback excludes its slower
recursive backend. CI's native job includes `tsr_lsp` on Linux and macOS.

`check.py` compiles an ephemeral Go overlay. The only changes to existing Go
code are the `lsptestutil.NewLSPClient` transport selection and the narrow
`Server` interface for `InitComplete`/inferred options. The carried client starts
`phase5_testserver`, serves the existing test filesystem callbacks, and uses
the original generated request types, message router and assertions.

It runs these four unchanged pinned tests against Go and Rust:

- `TestInitializeCodeActionKinds`
- `TestProjectInfoConfiguredProject`
- `TestProjectInfoInferredProject`
- `TestProgressNotificationsEndToEnd`

`options_sync_test.go` adds one client contract, also run unchanged against
both: inferred options produce TS7006, an edit with an annotation clears it,
and disabling `noImplicitAny` restores the native TS7044 suggestion. Native
progress delay is passed through the private descriptor, including zero.

`interop.py` builds the pinned Go command and runs a small ordinary client
against Go, `tsrust --lsp`, and the private endpoint on identical files. It
compares complete response objects and array order for initialize, pull
diagnostics, project-info, incremental non-ASCII edits, validation off/on and
shutdown, separately in UTF-8 and UTF-16. It also requires a clean process exit.
Asynchronous logs/watch/progress messages are serviced but are not part of
that response comparison; the dedicated tests cover their contracts.

`review_cases.py` checks the #93 regressions against that pinned executable:
full typed error responses, cancellation while queued behind configuration,
early exit, unknown notifications, the unknown-file diagnostic fallback,
stable/unstable config precedence and resetting the config, irrelevant watch
events and one coalesced refresh. Both Rust entry points run the same requests.
The native commands also compare complete malformed-JSON responses and 19 CLI
flag cases (usage/errors, prefixes, separators and native-width integer bounds).
The private endpoint retains ADR 0019's strict malformed-frame policy.
Rust tests separately cover capability absence, known/deleted directories,
ordinary edits cancelling pending refreshes, and shutdown invalidating queued
delivery. Duplicate-ID and post-shutdown refusals are intentional safeguards,
documented with owner approval in the L2 plan record.

No canonical upstream file is changed. These scripts build only into `target/`
and temporary directories; they do not produce archives, rewrite expectation
files, or claim semantic corpus passes.

## Direct-test assignment

| Pinned area | Rust checks |
| --- | --- |
| `dynamic_queue_test.go` FIFO and cancellation | `dynamic_queue::tests`: canceled put/get preserve contents; blocked readers/writers wake |
| `progress_test.go` delay, refcounts and labels | `progress::tests`: pure state transitions plus the bounded actor, fire-and-forget create and cancellation |
| `server_test.go` outgoing queue, shutdown, serialization failure | `tests`, `rpc_client::tests`, private endpoint tests: the reader remains available during reverse calls; cancellation and shutdown settle waiting work; bad serialization fails one request |
| Server recovery and logger internals | `recovery::tests`, `logger::tests`: subsequent work survives a panic; telemetry is opt-in and redacts unknown frames; filtering and stderr fallback |
| `stack_sanitizer_test.go` | All three committed native sanitizer baselines plus unknown-frame redaction |
| `server_contentmapper_internal_test.go` parsing | Contribution identity, options, duplicate extension casing, manifest/extension/cwd/compiler-option validation. Installation, mapper tracing and execution remain L6 |
| `lspwatcher_test.go` fake backend | Registration/removal, overflow/kind filtering, missing ancestor promotion, atomic creation race, termination and recreation, synthetic depth, stale callbacks after ID reuse, close during blocked registration and late-subscription disposal |
| `lspwatcher_test.go` real backend | Missing directory followed by creation and descendant change, on a fast-recursive native backend |
| `lsconv/converters_test.go` | Invalid bytes, UTF-8/UTF-16, CRLF/U+2028, feature ranges, canonical/supplemental source identity and complete source-file projection expansion. URI tests remain in L0 and position algorithms retain S04 coverage |
| `ls/diagnostics.go` and diagnostic conversion | Mapped compiler versus mapper-origin ranges, synthesized aggregate severity/related information, localization, message chains, tags, Visual Studio codes and style severities |
| Native command lifecycle | Flag spelling, parent disappearance, shutdown/exit through real processes |

Development results on 2026-10-04: the five Go client tests pass on both
runtimes; both Rust entry points match the Go response sequence in both
encodings. The direct tests, targeted clippy with warnings denied, ledger/marker
validation and package policy pass. The real FSEvents test passed outside the
desktop sandbox, which denies starting its stream. CI results are separate.

## Remaining Phase 5 boundaries

L3–L6 own hover, completion, navigation, edits/formatting, code actions,
cross-project/API services, mapper execution and ATA. The full L0 fourslash
transport/supervisor and semantic acceptance run are still pending; these
focused server tests do not add entries to `status/parity/lsp.json`. Native
pprof is Phase 7. Rust backtraces follow the sanitizer's unknown-frame policy
and are empty in telemetry; local error logs retain the diagnostic backtrace.
