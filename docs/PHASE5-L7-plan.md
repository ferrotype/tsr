# Phase 5 L7: acceptance path, residuals, replay and latency

Status: **implementation started** (2026-10-06); the owner approved section 2 items 1–3 on 2026-10-06; the other acceptance
decisions remain open.
Checkpoint L7 of the [Phase 5 plan](PHASE5-plan.md). Planning reference: `main`
at `ef456f65` (L0 to L6 merged, 0.3.0 released). Work is on
`codex/phase5-l7`. Upstream remains Corsa
`1f70213d4922b434345f639b441681e470c7cfc1`.

Written by Claude (Fable) with Codex (Astra): Claude drafted it, Astra
supplied the implementation state and reviewed the draft. The corpus counts
were checked by both against the pin unless a row says otherwise.

2026-10-06 update: this retains the proposed plan drafted against `3dbc11d9`
and corrects verified stale facts after L6 merged as `ef456f65`. Current agent
allocation follows the user's authorization. On 2026-10-06 the owner explicitly
approved section 2 items 1–3: count tests the native Go run executes, keep pinned
skips visible, retain only owner-approved differences at closure, and give
initial failing entries shared-cause reasons with no approval. This is approval
of that policy, not of specific residuals; N is not yet measured. Other decisions
and unmeasured estimates remain proposals.

## 1. What L7 has to deliver

The Phase 5 plan gives L7 five items: fix the remaining fourslash failures,
add replay fixtures and a Go-against-Rust replay comparison, add the `lsp`
latency workload, and apply PLAN's no-regression rule. Its exit is section 5's
count, passing replay and the latency result.

L7 is larger than that text, because the acceptance path L0 was to build was
not complete at the original planning reference. The table records that initial
audit; implementation is now in progress:

| L0 item | State |
| --- | --- |
| The carried client patch for fourslash, `tools/phase5/harness/` (item 3) | Absent. `tools/phase5/lsp/rust_client.go` (126 lines) and `check.py` replace `lsptestutil.NewLSPClient` for five `internal/lsp` tests, one fresh server process per test |
| The batch path in `scripts/parity.py` (item 3) | Absent. `SUITES` has `compiler`, `compiler-concurrent`, `transpile` and `tsc`; every variant is one process |
| `docs/PHASE5-tests.md`, the route of every pinned test (item 4) | Absent. The assignments that exist are the tables in `tools/phase5/lsp/README.md` and `tools/phase5/project/README.md` |
| `fourslash` and `lsp` suites in CI (item 7) | Absent |
| The first full run and its accepted failing set (item 8) | Never run. `status/parity/fourslash.json` and `lsp.json` do not exist |

So no fourslash test has run against the Rust server. What is known about
parity comes from the bounded native comparisons of L2 to L5
(`tools/phase5/lsp/*.py`, some 20,000 matching responses over hand-written
cases) and from the direct unit tests. Those are development checks; the L2 to
L5 records all say they carry no corpus credit.

What does exist and is reused: the version-3 private endpoint
`phase5_testserver` over the production `tsr_lsp` server, with the reset
barrier and the retained parse cache (L1); the `test/projectState` projection
and its ten-state check through the original Go writer
(`tools/phase5/project/`); the callback file system and options staging; an
ordinary stdio client that drives the pinned Go command, `tsrust --lsp` and the
private endpoint on the same files (`tools/phase5/lsp/interop.py`).

### The pinned corpus, measured

| Fact | Value | Source |
| --- | ---: | --- |
| Test functions under `internal/fourslash/tests` | 4,547 | `func Test…(t *testing.T)`; the plan's 4,548 also counts `TestMain` |
| Unconditional skips at the pin | 386 | 385 `t.Skip("Known failing fourslash test")` and one bare `t.Skip()`, each the first statement of its function |
| Conditional skips | about 31 more | `harnessutil.SkipUnsupportedCompilerOptions` (`harnessutil.go:1236`) skips at run time on UMD/System modules, node10/classic resolution, `esModuleInterop=false`, `allowSyntheticDefaultImports=false`, `baseUrl`, an ES5 target and `alwaysStrict=false`. Astra's scan of the test directives finds 31 tests with such options, none among the 386; the Go-mode run of L7.1 is the authority |
| Tests that mark themselves Strada | 162 | `MarkTestAsStradaServer` |
| State-baseline tests | 26 functions in seven files | `@stateBaseline: true`: the six `state*_test.go` files and `getEditsForFileRenameWithSolutionConfigFile_test.go` |
| Files whose tests open sessions inside nested parallel sub-tests | 6 | `t.Run` with `t.Parallel()`, each sub-test calling `NewFourslash` (three code-lens files, `organizeImports_exportLeadingComment`, `statedeclarationmaps`, `statefindallrefs`) |
| `@tsc` prebuild tests | 8 functions in 2 files | `tsctests.GetFileMapWithBuild` |
| Content-mapper tests | 55 functions | the `contentMapper*_test.go` files; they pass a spawner and `RunExternalCode` |
| Committed baselines | 1,749 | `testdata/baselines/reference/fourslash` |
| Calls on the Go server object | 3 | `SetCompilerOptionsForInferredProjects` (`lsptestutil/lspclient.go:324`), `Server.InitComplete()` (`fourslash.go:394`), `Server.Session().Snapshot()` (`statebaseline.go:247`). The existing overlay covers the first two |

Baselines are not Go sub-tests: `baseline.Run` writes the local file and
reports a mismatch as an error of the enclosing test. The adapter needs its
own per-baseline result (L7.1).

The skip count matters for the gate. Section 5 of the Phase 5 plan sets
`N = 4548` and keeps every unresolved skip in `F`, with `len(F) <= 22`. With at
least 386 pinned skips the original rule cannot be met by any implementation.
Owner decision 1 below replaces its denominator; the source estimate is not a
measured Go execution count.

## 2. Decisions for the owner

1. **The fourslash denominator — approved 2026-10-06.** The pin itself does not run 386 of its
   tests, so there is no Go outcome to match for them. Approved: count the
   tests the pin executes. `N` is the number of test functions that do not
   skip when the same test binary runs against the Go server (about 4,130:
   4,547 less 386 less the 31 conditional skips), and `F` is the set of those
   that do not pass against Rust; `len(F) <= floor(0.005 * N)`, which is 20
   at that `N`. The pinned
   skips stay visible as `skip` rows with the pin's reason, and a skip the Go
   run does not have is a failure. The alternative, `N = 4547` with skips
   outside `F`, allows 22.
2. **Approved entries in `lsp.json` — approved 2026-10-06.** Section 5 says `lsp.json` is empty at
   closure, but the L4 record carries an owner-approved replay difference
   (config-root replacement, 2026-10-05) "into `lsp.json`" as a raw non-match.
   Approved: `lsp.json` holds only owner-approved entries at closure.
3. **Wording of the first accepted sets — approved 2026-10-06.** `parity.py accept` creates both
   expectation files with, at first, a large failing set. Approved: the
   implementing agent writes each `reason` as a shared-cause label from the
   triage (L7.3), leaves `approved` empty, and the owner words the reasons of
   the final retained entries only.
4. **The latency fixture.** Recommended: the TypeScript package the pin
   itself carries, `upstream/packages/typescript` (beside `upstream/tsc`, not
   inside it; `src` is 108 files and 35,292 lines, strict, composite,
   `module: node16`), copied from the submodule by the capture with a small
   checked-in overlay for its one missing dependency (`@types/node`). It is
   fixed by the pin and is real code. Astra's review looked for it under
   `upstream/tsc` and did not find it, so this recommendation has not had its
   second check. The alternatives are a vendored snapshot of an external
   package with its provenance, or a hand-written multi-project fixture,
   which is easier to reason about and too small to measure references or
   rename meaningfully.
5. **Who records the latency run.** The quiet-host captures are the owner's.
   Recommended: the agents run sample captures (three pairs) while building
   the workload; the twenty-pair run of record is the owner's, on the host
   class `thresholds.toml` is authorized for.
6. **The branch and pull request split:** whether L7 is one
   branch or three pull requests (harness, replay and latency, residuals).
   The user-approved current agent allocation is in section 5.
   Recommended: three, because the first two touch no production crate and
   can merge while residual work continues.
7. **Profiling and API commands.** `custom/runGC` and the heap and CPU profile
   commands answer `-32601`, and `tsrust --lsp -pprofDir` is refused; the L2
   record names native pprof a Phase 7 boundary. Recommended: they do not
   block Phase 5 closure, and a suite case that observes one keeps its actual
   outcome. `custom/initializeAPISession`'s wire handshake belongs to the Phase 6
   API per the accepted L6 record; L6 supplies its retained project/symbol/type
   primitives. Its observed outcome remains visible in any suite case.
8. **CI runner class and shard count** for the two new suites, once L7.2 has
   a measurement.

## 3. Work items

The seven work items are numbered L7.1 to L7.7. L7.1 to L7.3 are the L0 debt
and gate everything else in the fourslash track. L7.5 and L7.6 are independent
of that harness track now that L6 has merged.

### L7.1 The fourslash transport patch

Home: `tools/phase5/harness/`. The existing `rust_client.go` and the overlay
construction in `check.py` move there, so `internal/lsp` and
`internal/fourslash` share one patch; `check.py` keeps running its five tests
through it.

1. **Overlay, by anchored replacement.** Three pinned files are replaced in a
   `go test -overlay`, each by a patch that fails when its anchor does not
   occur exactly once:
   - `internal/testutil/lsptestutil/lspclient.go`: transport selection, and
     `LSPClient.Server` as a narrow interface for `InitComplete`, option
     staging and the state projection;
   - `internal/fourslash/statebaseline.go`: line 247 reads the
     `test/projectState` projection through the adapter already checked by
     `tools/phase5/project/check.py`, keeping the writer's identity
     comparisons (lines 262 and 403);
   - `internal/testutil/baseline/baseline.go`: a reporting hook in `Run` and
     `writeComparison` (not in `Track`, which only lists touched paths after
     `TestMain`) that emits the baseline's path and whether it matched, is new,
     was deleted or could not be written, before control returns to the test.

   `fourslash.go` needs no patch while the interface keeps the call shape of
   `InitComplete()`. No file under `internal/fourslash/tests` and no assertion
   is touched; a test asserts that.
2. **Worker session.** One `phase5_testserver` per Go test process. A session
   belongs to one `NewFourslash` call, which in six files is a nested parallel
   sub-test, not the top-level test; no top-level function opens a session
   before `t.Parallel()`. The caller attaches when `NewLSPClient` runs and
   releases on close with `test/reset`;
   a lease mutex makes a second concurrent attach a harness failure rather
   than a shared session. A lost connection or a reset that misses its
   deadline fails the active test, kills the server and starts a clean one
   for the next test. The fourslash package's Go `parseCache` is ignored in
   Rust mode; the server's retained cache is the only one.
3. **Seams.** Inferred-project options before `initialize`
   (`SetCompilerOptionsForInferredProjects`), the `InitComplete` barrier, the
   callback file system with each test's case sensitivity and symlinks,
   `handleServerRequest` (the `workspace/configuration` replies), the mapper
   spawner and `RunExternalCode` over S11's stream tunnel. `@tsc` prebuilds
   stay Go fixture setup.
4. **Witnesses.**
   - *The patch is neutral.* The patched binary with Rust mode off passes
     every test that the unpatched pin passes. This run also yields the pinned
     skip set of decision 1.
   - *Go cannot answer for Rust.* In Rust mode `lsp.NewServer` is unreachable
     (the overlay panics on that path).
   - *A wrong answer fails.* A fault-injection mode of the carried client
     (harness code, not the server) alters one hover response, one completion
     response and one baseline's actual text; each selected test must fail.
   - *Sessions are isolated.* Two tests in one worker cannot see each other's
     files, options, encoding or late callbacks (L1's reset witnesses, now
     through the fourslash client).

### L7.2 The supervisor, the suites and the count

1. **Adapter.** `list` enumerates `fourslash/<TestName>` from the compiled
   test binary (`-test.list`, which names top-level tests only); the batch
   command takes a file of ids and streams result rows as tests finish;
   `run --id` is a batch of one. There is a row for the test's own outcome,
   one per Go sub-test (known only from the event stream) and one per
   reported baseline. Every row carries its parent test explicitly, taken
   from the first segment of the Go event name; sub-test and baseline names
   are encoded in the row id, and nothing infers a parent by counting
   slashes.
2. **Supervision** as section 2.3 of the Phase 5 plan specifies: read the test
   events (`run`, `pause`, `cont`, `pass`, `fail`, `skip`), keep a deadline for
   the active test, publish only finished tests, and on a crash or a deadline
   fail the active test and restart with the tests not yet started. Kill the
   Go process, the server and their children as one group. A start that
   identifies no test is a harness failure; two in a row stop the batch.
3. **`parity.py`.** An optional batch runner per suite, a prepare step (the Go
   test binary and the release server are built once, outside any deadline),
   and the skip rule of decision 1. `compiler` and `tsc` keep their path. The
   `lsp` suite uses one fresh process per test.
4. **The count.** `check fourslash` prints `N`, `len(F)` and the percentage
   from the merged results. Tests of the count: several failures in one test,
   the limit and the limit plus one, an approved failure, a missing result, a
   skip.
5. **Script tests** against a fake test binary that emits scripted events
   (fast, in `scripts/tests`), and one real twenty-test batch compared with
   twenty single runs.
6. **Bring-up in four small sets** before any full run: the 26 state-baseline
   tests, the 55 mapper tests, the 8 `@tsc` tests, and about fifty tests from
   each of the L3, L4 and L5 families. Each set exercises a different seam,
   and a harness fault found here costs minutes.
7. **Measure, then shard.** Time one worker on those sets before choosing the
   CI shard count. Astra's planning estimate is 60 to 120 minutes for one
   unsharded worker and at least 16 shards on GitHub's runners; nothing is
   measured yet, and the ten-minute figure of earlier drafts is not assumed.
   CI builds the binaries once, as the `runner` job does for the compiler
   suites, and adds `fourslash` and `lsp` to the parity matrix. Go is
   installed for those jobs only.

### L7.3 The first full run and its triage

1. Run the whole suite once on the development host, record build time and
   test time separately, and accept the exact failing set (decision 3).
2. Run the client-driven `internal/lsp` tests the same way into `lsp.json`:
   `server_completion`, `server_contentmapper`, `server_progress`,
   `server_projectinfo`, `server_projectreference_updates`,
   `server_semantictokens`. The three `server_test.go` tests access Go server
   internals directly; their route is a Rust observation port, not the carried
   LSP client.
3. Group the failures by cause: harness, crash or deadline, unimplemented
   handler, shared behavior difference, baseline bytes by writer, single
   cases. The grouping is a throwaway script over `results.ndjson`; its output
   is a table in the L7 record (`docs/PHASE5-L7.md`) and the `reason` of each
   entry, not a second register.
4. Write `docs/PHASE5-tests.md` from the test binaries' own lists: every
   pinned test of `project`, `lsp`, `ls` and their subpackages with its route
   (a suite id, or the Rust test that ports it). Tests still unported are
   listed as work, not as passes. L6 already added project content-mapper and
   ATA observation ports and the carried
   `TestSetContentMapperContributionsBeforeDidOpen` check. Its cross-project
   development scenarios cover ancestor/config changes, but do not establish
   the pinned `TestReferencesAfterAncestorProjectConfigDeletion1` outcome.
   Exact routes and residual gaps remain this audit's responsibility.

### L7.4 Residual fixes

Order: failures the harness causes; then crashes, panics and deadlines, since
each hides other failures and costs a worker restart; then shared causes by
the number of tests they fail; then baseline differences by writer; then
single cases.

For each cause: reproduce with `parity.py run fourslash --id …`, read the
pinned Go for the behavior (not memory of the TypeScript implementation), fix
it in the production crate with a unit regression, and run `accept` so the
expectation file is exact for the commit. CI runs the full suite; no local
full run per fix. A difference that will be retained is written up for the
owner with the native behavior, the Rust behavior and the reason; the owner
approves it in the entry.

Differences the owner has already approved are carried in, not re-argued: the
L2 lifecycle choices (duplicate in-flight ids rejected; requests after
`shutdown` answered `InvalidRequest`) and the L4 config-root replacement. Each
still shows as its actual outcome wherever a suite case observes it.

What the first run is expected to show. These are Astra's estimates from
implementing and reviewing L2 to L5 and from L6's scope, not measurements:

| Cause | Exposure | Depends on |
| --- | --- | --- |
| State projection fidelity (`Session().Snapshot()` through `test/projectState`) | 26 tests | L7.1; the ten-state check covers a fraction of what these tests read |
| Mapper lifecycle, execution and streams | 55 tests | L6 execution/installation is merged; full mapper corpus remains unrun |
| `@tsc` prebuilt outputs and project references | 8 tests | L6, and the fixture setup staying in Go |
| Cross-project search, project-reference redirection, workspace discovery | dozens, inside references (362), rename (148), definitions (225 + 69 + 38) and auto-imports (231) | L6 |
| Code-fix long tail | inside the 270 code-fix tests; L5 compared bounded samples only | L7.4 |
| Completion long tail | inside the 1,111 completion tests; L4 compared about 740 responses per encoding | L7.4 |
| Handlers that answer `-32601` by name: `custom/runGC`, the heap and CPU profile commands, `custom/initializeAPISession` | any test that sends them | Phase 6 API for the wire handshake; decision 7 for profiling/GC. `custom/setContentMapperContributions` is implemented by merged L6 |

One documented difference has no approval yet and may surface as an ordering
difference: scheduler waits are cancellation-aware in Rust and not in Go, which
can change later slot and type allocation order (`crates/tsr_project/README.md`).

### L7.5 Replay

The pinned driver, verified: a replay file is newline-delimited JSON. The
first line names two placeholders (`@PROJECT_ROOT@` and `@PROJECT_ROOT_URI@`
by default); every other line is `{kind, method, params}` with `kind`
`request` or `notification`. `TestReplay` substitutes the project directory,
assigns its own request ids, sends each message in order, waits for each
response and fails only on an error response. It compares nothing, serves the
real file system, and installs with real `npm`. `-simple` and `-superSimple`
reduce a session; they are for crash triage and are not used here.

1. **Fixtures** under `tools/phase5/replay/fixtures/`, each 20 to 80 files:
   `references` (three
   TypeScript projects with `composite` references and declaration maps),
   `checkjs` (JavaScript with `checkJs`, JSDoc types and a CommonJS
   dependency), and `monorepo` (workspace packages linked through
   `node_modules` symlinks, a package with `exports` conditions, and local
   `@types`). Each holds every file a session reads, including dependencies;
   automatic type acquisition is off or served from the fixture.
2. **Sessions** in the pin's format, written as scripted scenarios (open,
   edit, completion and resolve, hover, definition, references, rename, code
   action, organize imports, formatting, a config change, a watched-file
   change, close) and expanded to the replay file by a small generator, so a
   session is reviewable and no private editor recording is needed. An
   owner-supplied recording drops in as another file.
3. **The driver** `tools/phase5/replay/replay.py`, built on `interop.py`'s
   client: copy the fixture to a scratch directory, substitute the
   placeholders, send the session to one server over stdio, and write a
   transcript with the paths turned back into placeholders. The transcript has
   each request's `result` or `error`, correlated by position in the session,
   and each server notification and server request in its order per stream.
   The pinned server serves diagnostics both ways (`server.go:1583` and 1851
   for pull, 724 and 1728 for push): a pull response is compared as a
   response, and pushed diagnostics as one stream per document, with no order
   required across documents.
4. **Comparison.** `compare` runs the pinned Go command and `tsrust --lsp` and
   compares the transcripts exactly: arrays in order, absent against null,
   text and ranges. Nothing is dropped because it differs. What cannot be
   equal by construction is correlated or excluded by name in the driver,
   with its reason: root paths and URIs (placeholders), client request ids,
   the server's own request ids (`tsN`, `server.go:1053`), API pipe paths
   (`server.go:2350`) and log or telemetry timestamps. Work-done progress is
   gated by a timer (`progress.go:118`), so whether it appears depends on
   timing: replay sessions do not advertise it, and progress keeps its pinned
   client test in the `lsp` suite. `record` writes the Go transcript as
   the session's expected file; ordinary CI runs Rust against it as replay
   variants of the `lsp` suite. A session also runs once in UTF-16.
5. **Witness.** Changing one response in an expected file fails the variant.

### L7.6 Latency

1. **Capture** `tools/phase5/perf/capture.py` drives the pinned Go command and
   a release `tsrust --lsp --stdio` on the same local fixture copy (decision
   4), with no test-host callbacks. A repetition is one fresh process per
   runtime; the order of the two runtimes alternates between repetitions.
2. **Scenarios**, each a fixed document, version and position written in the
   capture's scenario file:
   - *first diagnostics*: from the write of `textDocument/didOpen`, after the
     `initialize` exchange has completed, to the first complete diagnostics
     for that document; fresh process, cold cache.
   - *completion*: after a fixed one-character edit, from the write of the
     request to the full response.
   - *hover*, *references*, *rename*: request write to response, at fixed
     positions, after the same warm-up on both runtimes (the first
     diagnostics above and one hover elsewhere).
   Every measured response is also compared between the runtimes; a pair that
   disagrees is recorded, fails the workload as a correctness mismatch and
   is left out of the latency statistics.
3. **Record.** Twenty pairs; raw samples, host, revision and the per-scenario
   ratio of medians with its spread go to `status/perf/lsp/` through
   `perf.py record lsp`. `thresholds.toml` gains `[lsp]` with the five ratios
   at 1.0, and `perf.yml` gains the `lsp` workload, needing only the two
   binaries.
4. **Reading the result.** A scenario passes when its ratio is at or below
   1.0. When the interval around a ratio includes 1.0 the scenario is
   inconclusive: extend the samples once, to forty pairs, and report it to the
   owner if it stays so. A measured regression is profiled and fixed, or
   goes to the owner as a separate decision. Results are reported against Go
   first, each scenario with its ratio.
5. **Where to look first** if a scenario is slow (Astra's suspicion, not a
   profile): the snapshot and project clone before work is dispatched
   (`tsr_lsp/src/runtime.rs`), checker acquisition per semantic request
   (`language_features.rs`), the auto-import index synchronization after a
   completion, and per-request protocol conversion.

### L7.7 The L7 record

`docs/PHASE5-L7.md`: the harness and its witnesses, the first-run table and
the table at exit, the retained entries with their approvals, the replay
fixtures, the latency run, and what L8 inherits (unported unit tests from
L7.3, ledger homes).

## 4. Order

```text
L6 merge ──┬─> L7.1 ─> L7.2 ─> L7.3 ─> L7.4 ───────────┬─> L7.7
           ├─> L7.5 (fixtures, driver) ─> replay in CI ─┤
           └─> L7.6 (capture, samples) ─> run of record ┘
```

L7.1 and L7.2 touch only `tools/`, `scripts/` and CI, so they could start before
L6 merged. L7.4 starts from L6's merged state (`ef456f65`). The latency run of record is
taken last, on the commit that meets the correctness count.

## 5. Who does what

The user authorized Sol agents for noncoding and less-critical coding support.
The root agent owns integration, shared harness contracts, production residual
fixes, validation and the final acceptance report. Support agents have disjoint
file ownership; the table is the current allocation, not a dispatch to a separate
Claude session. The branch-versus-PR decision in section 2 remains open.

| Track | Work | Current allocation |
| --- | --- | --- |
| Acceptance | L7.1, L7.2, L7.3: `tools/phase5/harness/`, `scripts/parity.py`, CI, the first run and its triage | Root integration; Sol support for the routing inventory/documentation |
| Replay and latency tooling | L7.5, L7.6: `tools/phase5/replay/`, `tools/phase5/perf/`, `scripts/perf.py`, `perf.yml` | Root integration; scoped Sol support for tooling |
| Residuals | L7.4: fixes in `tsr_ls`, `tsr_lsp`, `tsr_project`, `tsr_autoimport` and the compiler crates | Root after triage; any delegated fixes receive explicit crate/module ownership |

The root reviews delegated changes before integration. No separate Claude
session or additional pull request is implied by this allocation.

## 6. Exit

- `status/parity/fourslash.json` exists, CI recomputes it, and the count of
  decision 1 holds with every retained entry approved.
- `lsp.json` satisfies decision 2; the replay variants pass.
- The harness witnesses of L7.1 and the script tests of L7.2 pass.
- The latency run is recorded and no scenario regresses against Go without an
  owner decision.
- `docs/PHASE5-tests.md` and `docs/PHASE5-L7.md` are written.

## 7. Risks

| Risk | Answer |
| --- | --- |
| The first run fails far more than the bounded comparisons suggest | Triage by cause before fixing anything; the harness witnesses separate harness failures from Rust behavior |
| A crash or hang early in a batch hides the rest | Restart with the unstarted tests; crashes are fixed first |
| Batch reuse changes outcomes | The twenty-test single-against-batch comparison, repeated on any test whose outcome differs between a batch and `--id` |
| Replay notifications arrive in a different order on the two runtimes | Order is compared per stream, not across streams; anything excluded is named with its reason |
| Latency noise on a small fixture | A fixture large enough for millisecond-scale requests, alternating order, and the inconclusive rule |
| Retained differences exceed the limit | Reported to the owner with the list as soon as it is known; the limit is not edited |
