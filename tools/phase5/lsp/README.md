# Language server development checks (L2 and L3)

L2 connects the generated protocol to production project sessions and compiler
diagnostics. `tsrust --lsp --stdio` and the private version-3 endpoint use the
same runtime. Unported L4–L6 service methods return a named `-32601` error; their pinned
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

## L3 read-only features

The ordinary server and private endpoint dispatch through the same production
language service. This increment implements hover (Markdown, plaintext and
Visual Studio classified text), references and implementations, highlights,
definitions/type definitions/source definitions, document/workspace symbols,
signature help, inlay hints, semantic tokens, call hierarchy, selection ranges,
folding, code lenses and linked editing. Pull diagnostics and suggestions keep
the L2 compiler pipeline and now run alongside the semantic feature probes.

Navigation retains the requesting program and checker lease. Source definitions
use a private `NoDtsResolution` resolver for implementation files and forwarding
exports. The syntactic path never acquires a checker; semantic resolution is a
scoped callback. Declaration maps follow original positions through external,
inline and chained maps, including files outside the loaded program. Unreadable
maps fall back and map cycles terminate. No navigation target is inserted into
the published program. L3 also fixes checker API queries on original JSDoc nodes
to resolve their bound reparsed nodes, and reuses a containing configured project
for an unopened dependency instead of creating an inferred project.

The bounded scripts below compare complete native response objects, preserving
array order and absent/null distinctions. They run against the same temporary
files with UTF-8 and UTF-16 negotiation. They use the Go executable built by
`interop.py`; run that first if `target/phase5/go-lsp` is absent or the pin changed.
No renderer is duplicated and no output is accepted by normalization.

```sh
cargo build -p tsrust --bin tsrust
python3 tools/phase5/lsp/read_only.py
python3 tools/phase5/lsp/signature_help.py
python3 tools/phase5/lsp/inlay_hints.py
python3 tools/phase5/lsp/references.py
python3 tools/phase5/lsp/call_hierarchy.py
python3 tools/phase5/lsp/code_lens.py
python3 tools/phase5/lsp/source_definition.py
python3 tools/phase5/lsp/read_only_edges.py
```

Development comparisons on 2026-10-04:

| Script | Matching responses | Scope |
| --- | ---: | --- |
| `read_only.py` | 3,608 | Hover, definition/type definition, symbols, tokens, folding, linked editing and selection ranges; hierarchical/flat symbols, line-only folding, text/link capabilities and edits |
| `signature_help.py` | 5,152 | Cursor boundaries, type arguments, overloads, rest/spread tuples, tagged templates, JSX, classified text and nullable active parameter |
| `inlay_hints.py` | 280, plus 14 refresh counts | All parameter/type/enum/return preferences, name suppression, locations, quote style, enabling/disabling/resetting preferences |
| `references.py` | 6,220 | Cross-file aliases/re-exports, inheritance, implementations, destructuring/contextual properties, labels/keywords, Visual Studio references, location links and edits |
| `call_hierarchy.py` | 646 | Prepare/incoming/outgoing, cross-file overloads, methods/accessors, anonymous functions, class initializers/static blocks and JSX |
| `code_lens.py` | 180 | Reference/implementation lens admission, resolution, commands/counts and preference updates |
| `source_definition.py` | 724 | Package implementation entry points, forwarded/default/type-only imports, normal definition fallback, malformed/inline/chained declaration maps and source-definition preference |
| `read_only_edges.py` | 1,218 | CommonJS exports, JSDoc typedef/property names, JSX-runtime/tslib implicit imports, unopened dependencies, deprecation/unused diagnostics |

The direct native selection-depth and implementation-worklist regressions are
ported into `tsr_ls::tests`. Other regressions cover cancellation, escaped owner
identities, classified type-reference handles, code fences and string-name
ranges, inlay declaration links, signature printer context, declaration-map
ownership/cycles, JSDoc symbol/type identity and the no-checker source-definition
fast path. A session test protects configured-project selection for an unopened
dependency. The L2 diagnostic/coordinate tests remain in `converters::tests`.
The relevant preference parsing/reset and refresh behavior is exercised through
the ordinary wire; editing/auto-import preferences and the full preference
roundtrip belong to their L4/L5 consumers.

## Remaining Phase 5 boundaries

L4 owns completions and auto-imports. L5 owns rename, edits/formatting and code
fixes. L6 owns project-tree loading and searching across referenced projects,
workspace discovery beyond loaded projects, reverse declaration-map lookup into
other project programs, content-mapper execution/installation, ATA and the API
bridge. L3 searches and workspace symbols currently use the loaded program(s);
these are concrete L6 project-host dependencies, not completed cross-project
acceptance. Source maps from a declaration target to an original file are
implemented in L3.

The full L0 fourslash transport/supervisor and semantic acceptance run are still
pending for L7. These development comparisons and direct tests confer no
`status/parity/lsp.json` corpus credit. Native pprof is Phase 7. Rust backtraces
follow the sanitizer's unknown-frame policy and are empty in telemetry; local
error logs retain the diagnostic backtrace.
