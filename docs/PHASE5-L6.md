# L6 — multi-project sessions, ATA and mapper lifecycle

Implementation branch: `codex/phase5-l6`, based on the merged L5 and 0.3.0
release. The authority is the pinned Go tree at
`1f70213d4922b434345f639b441681e470c7cfc1`. The implementation and focused
checks below are complete. Full fourslash/replay acceptance remains L7.

## Implementation sequence

1. Complete the compiler's editor project-reference host. Resolver existence
   checks must see declaration outputs backed by source files, while automatic
   type discovery still reads the original filesystem. Preserve composite,
   source-redirection disabling, symlink and case-sensitivity behavior.
2. Extend snapshot requests to load project trees, update named projects and
   discover configured projects without manufacturing inferred projects. API
   project/file opens have separate reference counts from editor overlays.
   Retained snapshots keep their programs after a later close or rebuild.
3. Route references, implementations, rename, VS references and incoming calls
   across projects. Start with the default and containing projects, follow
   original declaration positions, then load relevant reference trees. Preserve
   default-project result preference and the pin's per-response deduplication.
   Release the checker before loading a new snapshot. File moves load all trees,
   including the fallback for clients without `workspace/willRenameFiles`.
4. Complete the `project/api.go` and `ls/api.go` bridge. Rust symbol/type results
   retain their checker generation; a foreign generation cannot consume them.
5. Port ATA discovery, filename/package inference, package-name validation,
   registry/version selection, batching and installer caching. Integrate jobs
   with the session's current demand, reject obsolete results, watch acquired
   typing files, and rebuild when the installed set changes. Native npm is
   injected; tests use a deterministic mock and do not use the network.
6. Connect configured and contributed mapper projects to the production mapper
   host, shared mapped parse cache, session watches and dynamic registrations.
   The native process spawner and private S11 byte-stream adapter feed that same
   host. Snapshot leases own projects/bundles; client disconnect and session close
   cancel the host and close streams. A request cancellation does not independently
   cancel a mapper transform: the pin calls it with the host's session context.
7. Review integration and run focused Rust tests/clippy, targeted native project
   and mapper comparisons, format and marker validation. Preserve L7's distinct
   full fourslash replay and performance work; do not fabricate acceptance from
   a development runner or change existing parity approvals.

## Focused checks

- `tsr_compiler` editor reference-host tests: absent emitted `.d.ts`, directory
  faking, symlinks, disabled source redirects and retained source inputs.
- `tsr_project` API/resource tests: reference-counted opens, failed-open snapshot,
  old snapshot retention, unopened sibling discovery and disabled child loading.
- `tsr_ls` retained API handles and native/source/generated definition positions.
- `tsr_lsp` response joins: duplicate locations, rename ranges/resource moves,
  VS definition IDs, cancellation and project-tree routing.
- `tools/phase5/lsp/projects.py`: fresh ordinary Go/Rust LSP processes over one
  live three-project solution; both UTF-8 and UTF-16. No timed performance claim.
- ATA direct and session tests: mocked installs, existing/failed/cancelled
  packages, settings precedence, stale-result rejection and lifecycle completion.
- Mapper host and S11 tests: configured/contributed transforms, dynamic config,
  locale/cache identity, watched files, cancellation/disconnect and final disposal.

## Boundaries carried from earlier checkpoints

The approved L4 config-root replacement difference remains unchanged. Native
mapper calls use the session context; per-request cancellation is not a new
unapproved transport behavior. Full retained fourslash `@tsc`, mapper and
multi-project replay still belongs to L7's complete runner; L6 adds targeted
native-client comparisons and direct unit ports.

## Results (2026-10-05)

- Compiler, module, project, language-service, LSP and private-host unit tests
  pass. Native FSEvents coverage was run with host filesystem access; the
  sandbox-only run cannot start its stream. Checker work is released before
  loading another project and API results retain their owning checker generation.
- The live project runner matches pinned Go on references, rename,
  implementations, incoming calls, `willRenameFiles` and module-rename fallback:
  six fresh-server scenarios in UTF-8 and UTF-16. Only unordered inter-project
  collections are normalized; edit arrays and each result's fields are retained.
  The runner explicitly disables ATA, so these comparisons cannot install npm
  packages. Implementation-only searches retain empty local definition groups;
  an empty serialized result cannot hide another project's definition links.
- Ten snapshot states match the original Go state writer byte for byte, in both
  a fresh process and two retained-cache test repetitions. This includes delayed
  ancestor projects, which appear without parsing their configurations yet.
- The pinned `TestSetContentMapperContributionsBeforeDidOpen` passes against
  Go and the Rust private endpoint. Its unchanged assertions cover transforms,
  extension/watch registrations, removal and fallback. Rust mapper tests cover
  mixed watch batches, manifest changes, locale/cache identity and bundle reuse.
  An escaped supplemental file retains both bundle owners; releasing it returns
  the tracked owner count to zero. Stream reset/disconnect, late traffic, credit
  waiters and malformed spawn responses are tested separately.
- ATA discovery, validation, batching and Session tests cover active pinned
  discovery/ATA scenarios with a deterministic npm mock. Native executor tests
  verify stdout handling and process-group cancellation using local shell
  processes. The real npm test is ignored unless explicitly requested. The pin's
  skipped local `@types` deduplication case is not claimed as a passing port.
- Seven targeted compiler variants pass all 67 subtests: three `typesVersions`
  cases, mapped transform/supplemental module/supplemental globals, and multiple
  automatic type roots. These `parity.py run --id` results are development
  checks, not a replacement full-suite expectation or evidence archive.

The initial L6 review found and fixed response-union combination, implementation
search group retention, alias preferences during expansion, incoming-call order,
late cancellation, delayed-tree traversal, and ATA preference precedence. There
is no new parity exception or second approval registry. Existing owner-approved
L2 lifecycle safeguards and the L4 config-root difference are unchanged.

## Remaining Phase 5 work

L7 owns full fourslash/state replay, any failures those broader cases expose,
and latency/performance measurements. L8 owns the final Phase 5 audit and
cut-over. The custom API-session wire handshake belongs to the Phase 6 API;
L6 supplies the retained project/symbol/type primitives it consumes. Go runtime
profiling methods remain explicit unsupported methods rather than fabricated
Rust profile responses.
