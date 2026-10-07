# L7 implementation record

Status: **complete** (2026-10-07). The owner closed L7 and moved all remaining
performance work and acceptance to Phase 7. Correctness and replay pass;
the latency results below remain unchanged, including all five misses.

Work started on `codex/phase5-l7` after L6 merged. The owner approved the
acceptance rules in [the plan](PHASE5-L7-plan.md#2-decisions-for-the-owner) on
2026-10-06. No new individual difference is approved by this record.

## Harness bring-up

The carried overlay builds the pinned fourslash test package without replacing
any test case or expected baseline. It shares the transport with the L2 client
checks, uses the retained private Rust server, and renders project state through
the original native writer. The new supervisor consumes actual `test2json`
events, restarts a crashed worker, bounds active-test deadlines, and emits
explicit test/subtest/baseline rows. `parity.py` supports these batch suites
without changing the compiler/transpile/tsc execution paths.

Local checks performed during bring-up:

- `TestAutoCloseFragment` and `TestAutoCloseTag` pass in one retained Rust worker.
- `TestNavigationBarJsDoc`, `TestHoverOptionalMembers` and
  `TestContentMapperCompletions` pass against Go and Rust.
- `TestImplementationsAcrossProjects` passes against Go and fails against Rust:
  the actual state baseline differs in project/config retention after a
  cross-project search. The projection is not normalized to hide it.
- The hover and completion controls `TestQuickCatchInfo` and
  `TestCompletionListAfterFunction` pass. Injecting one wrong response makes
  their unchanged Go assertions fail. Corrupting a navigation baseline also
  fails. Making the baseline output directory a regular file produces an
  explicit baseline write-error row, including when actual bytes match.
- A native package failure writing its tracking file, after all individual
  tests passed, was rejected as a harness failure. The runner creates the
  tracking directory before execution.

Commands and ownership are in [the harness README](../tools/phase5/harness/README.md).
Raw bring-up output is disposable development output under
`target/phase5/harness/`; it is not an acceptance archive or an approval registry.
The completed 222-test seam/feature sample has 209 native-executed tests and
13 native skips. Rust passes 153 and fails 56 of those 209 parents; no native
reference failed. The 137 failing result rows count only once per parent.
The slowest parent took 0.4 seconds locally in the release build. These are
functional bring-up timings, not latency evidence. There is no full-suite
acceptance claim yet. CI routing follows measured runtime checks.

A twenty-test mixed batch agrees with twenty single runs on both runtimes,
including exact baseline bytes and nested parallel tests. All 13 routed LSP
client tests pass against both runtimes; their initial expectation file is
recorded. This does not include replay or the direct internal-test ports.

Two full-run attempts exposed supervisor handling gaps: very long ordinary
assertion lines, and `test2json` synthesizing a named terminal failure before
EOF on a process panic. Both were corrected and regression-tested against the
saved event streams. Incomplete attempts have no final result metadata and are
not accepted. An underlying empty-directory callback from root import-path
completion is the first production fix identified by those runs.


## Initial complete fourslash run and triage

The first complete run is `target/phase5/harness/full-03`, recorded on
2026-10-06. Its compiled roster contains 4,534 tests on Darwin arm64: Go's
implicit GOOS=js filename rule excludes 13 of the 4,547 source functions.
These platform exclusions are distinct from runtime skips. The native and Rust
runs use the same compiled test binary. Every native-executed parent passed;
the skip set below comes from actual native terminal events, not the option scan.

| Initial result | Count |
| --- | ---: |
| Compiled top-level tests | 4,534 |
| Native runtime skips | 417 |
| Native executed tests, N | 4,117 |
| Native failed parents | 0 |
| Rust passing native-executed parents | 3,842 |
| Rust failing native-executed parents, F | 275 |
| Rust failing result rows, including nested tests and baselines | 391 |
| Distinct-parent semantic pass percentage | 93.320% |
| Closure limit, floor(0.005 × N) | 20 |
| Approved retained failing entries | 0 |

`meta.json` records 09:54:53–09:55:44 UTC: 51 seconds of combined native/Rust
functional execution with four workers, a 45-second active-test deadline and
18 logical CPUs. Preparation/builds precede this interval. The slowest parent
was `TestDeleteModifierBeforeVarStatement1` at 3.4 seconds. These functional
harness timings do not measure editor request latency or establish a quiet-host
performance result.

The first run produced 391 failing rows; the current exact set in
`status/parity/fourslash.json` is updated by the follow-up runs below.
Every reason is an observed error group or an explicitly named symptom; detail
fields retain assertion excerpts or native-versus-Rust baseline byte differences.
No entry receives an approval. The intermediate expectation makes CI exact; this initial 275-parent result
exceeded the closure limit by 255. This is the
initial run, not Phase 5 closure or an assertion that all failures share 25 fixes.

The table is a disjoint assignment of each failed parent to its first actionable
observed group. A parent may have several other failures; all rows remain in the
expectation. `*-symptom` groups identify what differed, without diagnosing a
shared implementation cause. Baselines were compared against the pinned reference
bytes and the run's isolated Rust output, rather than grouped only by test names.

| Observed group / symptom | Failed parents |
| --- | ---: |
| completion-items-or-fields-symptom | 89 |
| completion-context-eligibility-symptom | 24 |
| project-state-retention-baseline-symptom | 24 |
| navigation-or-display-baseline-symptom | 17 |
| import-action-selection-or-edit-symptom | 15 |
| completion-commit-characters-symptom | 11 |
| completion-code-action-edit-symptom | 10 |
| code-fix-edit-or-availability-symptom | 9 |
| auto-import-baseline-symptom | 9 |
| document-symbol-access-name-panic | 9 |
| completion-sort-priority-symptom | 7 |
| completion-filter-text-symptom | 7 |
| class-member-snippet-action-missing-symptom | 6 |
| other-assertion-symptom | 5 |
| private-project-queue-overflow | 5 |
| signature-help-ast-owner-error | 5 |
| completion-checker-operation-error | 4 |
| mapper-formatting-edit-symptom | 4 |
| private-session-lease-conflict | 4 |
| import-equals-specifier-panic | 3 |
| folding-range-symptom | 2 |
| host-root-callback-panic | 2 |
| hover-qualified-name-panic | 2 |
| code-action-template-text-panic | 1 |
| hover-result-symptom | 1 |
| Total | 275 |

Fix harness/host errors and panics first. Four parents fail the private lease's
`concurrent private sessions` check even with `-test.parallel=1`; five fail
`too many project operations` and then `private router ended before reset`.
The logs establish these failures but do not yet distinguish a leaked lease,
cleanup race, or excessive session admission. Two package-import wildcard tests
panic inside the Go filesystem callback while constructing a sub-filesystem for
`/`. The root-path fix `045bd11d` passes four bounded native comparisons; the two
wildcard cases still require a fix and remain in this initial failing set.

The production panic/error groups have concrete common messages:

- Nine document-symbol parents hit `GetElementOrPropertyAccessName`'s unhandled
  case, including `TestNavigationBarFunctionPrototype` and anonymous-expression
  cases.
- Three import actions hit `Node.ModuleSpecifier: KindImportEqualsDeclaration`.
- Two hover cases hit `Node.Text: *ast.QualifiedName`; one import code action hits
  `Node.Text: *ast.TemplateExpression`.
- Five signature-help parents report `Ast(WrongOwner)`; four completion parents
  report unsupported checker operations such as `Unexpected node kind in
  isValidPropertyAccess`. These are observed failure mechanisms, not approvals.

The 24 state-baseline parents expose missing or different `RetainingProjects` /
`RetainingOpenFiles` edges, configuration retention and extra inferred-project
transitions. For example, the call-hierarchy/code-lens baselines omit the solution
container's retaining-project edges; declaration-map searches add or retire
inferred projects differently. The bytes establish a project-state discrepancy,
but do not yet distinguish projection fidelity from actual session-state behavior.

Completion triage can start with the smaller repeated field symptoms: eleven
parents differ in default commit characters, seven in sort priority (`11` versus
`10` appears repeatedly), and seven in filter text. The 24 eligibility cases
return nil where a list is expected or return a list where nil is expected.
The six missing `ClassMemberSnippet/` actions and the remaining item/field cases
need separate reproductions before claiming common fixes.

The auto-import baselines show distinct issues: omitted import semicolons in the
exclude-pattern and exclude-regex tests, splitting one combined import into two
in the symlinked-monorepo case, extra candidate edits, missing augmentation edits
and changed default-import casing. Import action excerpts separately show type
specifier ordering and different module paths. Four mapper-formatting cases leave
supplemental verbatim ranges unformatted or change the wrong projection. Navigation
baselines add or omit reference/rename ranges and supplemental source targets;
one inlay baseline adds empty display parts. These byte-level symptoms are not
collapsed into an unverified single auto-import or navigation cause.

## Replay bring-up

Three self-contained fixtures exercise references across projects, check-JS and
a small monorepo. Sessions run ordinary stdio LSP, retain raw frames and actual
sent requests, and compare both UTF encodings. Root/correlation normalization
is named explicitly; diagnostic ordering is preserved per document and arrays
are never sorted. Supported error-only logging prevents timing logs at source.
Malformed frames, duplicate JSON keys, unexpected responses and booleans in
place of numeric protocol values fail the harness.

Repeated native captures agree for each fixture. The initial check-JS comparison exposed two separate effects. A native probe
waiting after open publishes 22 config diagnostics, then 32 after checking;
the earlier first-count difference was scheduling, not omitted global errors.
The replay now waits for each named config publication before advancing, keeping
every notification and byte comparison. Validation policy is also retained in
Rust snapshots so a disabled transition clears once, as in the pin. All three
fixtures now match in both UTF encodings, including the diagnostic streams.
See the [replay README](../tools/phase5/replay/README.md).

## First residual-fix batches

The unchanged native suite was rerun after each release build:

| Run | Rust failing parents | Failing rows | Fixed parents since previous | Regressions |
| --- | ---: | ---: | ---: | ---: |
| `full-03` | 275 | 391 | — | — |
| `full-04` | 249 | 354 | 26 | 0 |
| `full-05` | 207 | 275 | 42 | 0 |
| `full-06` | 180 | 243 | 27 | 0 |
| `full-07` | 152 | 183 | 28 | 0 |
| `full-08` | 152 | 183 | 0 | 0 |

N remains 4,117 with 417 native skips and no native failures. At `full-08` the raw
failure set is 183 rows over 152 parents, with zero approvals; Phase 5's limit
remains 20 parents. The 13-test LSP client suite still passes in full (`lsp-03`). `full-08` also
includes the reviewed constraint-completion guard: callable type-literal members
cannot acquire object-literal method-body snippets or their sort priority.
The final batch passes 102 LS unit tests, 20 auto-import tests, the 39 LSP tests
(the native FSEvents case rerun outside the sandbox), mapped compiler ownership
checks, testhost regressions, affected-crate clippy and marker validation.
Thirty-one focused Python tests pass, including the exact-marker regression.

Implemented corrections include overlapping fourslash sessions and notification
backpressure in the private adapter; root package-path resolution; guarded AST
access for document symbols, qualified JSDoc links, malformed import actions and
import-equals; import-kind ordering; static-property and optional-JSX completion
fields; dotted-namespace and incomplete-dot completion contexts; index-signature
and string-overload commit characters; cross-source signature display ownership;
and referenced config/inferred-resource retention. Focused regressions exercise
these mechanisms independently of the unchanged native suite.

The CI bundle builds the two native assertion binaries and private Rust server
once, then runs four fourslash shards and one LSP shard. It relocates the native
repository root explicitly so source-path discovery does not retain the build
checkout. Hosted CI has passed all four fourslash shards, the LSP shard and their parity
join at `b031d767`. The unrelated script-test failure on moved historical Rust
line anchors is corrected by validating exact operation identities, files and
marker multiplicities; no historical capture is regenerated for line movement.

The next production batch resolves supplemental sources through the retained
program, including separately owned mapper outputs; all 55 content-mapper tests
pass in `full-07`. Completion payloads retain their supplemental index through
resolve. Mapped auto-imports advance past generated headers, map every edit
exactly and eagerly remove unsafe candidates. Shared indentation handling
preserves original mapped indentation. Generic type-argument constraints and
binding patterns now use their native completion-property rules, exported local
symbols retain both declaration and export meanings, typesVersions wildcards do
not introduce duplicate separators, imports keep printer semicolons unless
explicitly removed, expando symbols support computed names, and malformed
finally blocks retain their zero-width folding span.

The state-baseline adapter now observes the published snapshot without flushing
pending edits or closes, while the original Go writer uses its own immediate
open-file map. The existing explicit flush endpoint retains its previous
behavior. This removes observation-induced state differences without changing
production scheduling. Focused tests cover the non-mutating observation.

## Completion, import and project follow-up

`target/phase5/harness/full-09` reduces failures from 152 to **108 parents**
(134 failing rows), fixing 44 previously failing parents with no regression.
All 4,117 native-executed tests still complete; the 417 native skips and closure
limit of 20 remain unchanged. No residual receives approval. The separate
LSP client suite passes all 13 tests in `lsp-04`.

This batch restores explicit import-completion `insertText`, value properties
of namespaces merged with classes/objects, public hash-named property edits,
and runtime members of `typeof import`. CommonJS object exports now contribute
the exact property forms indexed by Go; forced module detection no longer
masquerades as ESM syntax when selecting `require`. Import insertion groups
package names before relative names and preserves non-header leading comments.
The native suite verifies the affected CommonJS, import, quote-style and
import-type cases, including the already-existing controls.

Implement-interface generation excludes constructor parameter declarations,
preserves signature-child ranges, and carries synthetic elision comments into
private printer fragments. All four previously failing implement-interface
tests pass. Global workspace-symbol queries load referenced projects; ordinary
resource lookups retain solution configurations until the native cleanup point.
JSDoc rename inspects the reparsed import declaration. Focused tests cover the
ownership, scope and cleanup distinctions; 115 LS unit tests, 26 auto-import
tests and affected-crate clippy pass.

CI now routes all three bounded replay fixtures in both encodings through the
LSP shard. Ordinary native/Rust CLIs are built once and shared as an artifact;
raw replay transcripts and mismatches are uploaded even after failure. Local
execution through this CI helper passes all six fixture/encoding combinations
against the ordinary Go and Rust CLIs in `target/phase5/replay/current`.
Direct checkerpool-test routing is audited separately: equivalent timeout/capacity
parameter choices do not create artificial gaps, while missing independent
assertions remain named. The source audit is not an execution result.

## Final batch before the requested pause

Auto-import ranking now retains the pinned module-specifier provenance and
export-star origin through candidate selection, including barrel-cycle and
Node-core style preferences. Only the original protocol fields are serialized.
Completion blockers include regexp flags and template text, object-type member
containers follow the native syntax rules, and constructor parameter modifiers
remain available without treating declaration names as expression slots.
Class snippets keep the name fallback for unsupported declaration shapes,
retain decorators, and identify signature import edits as snippet actions.
Constructor references follow `extends`, not `implements`; UMD namespace
aliases follow the native merged-symbol condition.

The combined LS suite passes 124 tests, auto-import passes 31, and scheduler
passes 18. Seven new scheduler tests supply the previously missing independent
category, capacity, timer, cancellation and discard assertions. The direct-test
inventory records their exact observations and Rust API correspondence; it does
not turn source inspection into native execution credit. Affected-crate clippy,
seven inventory/replay script tests, formatting and `cargo xtask validate` pass.

`target/phase5/harness/full-11` finishes this batch at **77 failing parents / 97
failing rows**, down from 108 / 134 in `full-09`: 31 formerly failing parents
now pass and no previously passing parent fails. An intermediate run caught
optional-label filter text and constructor-at-EOF regressions; both were fixed
and given focused tests before the final comparison. The original native
denominator remains 4,117 executed tests with 417 skips. The exact residual set
is in `status/parity/fourslash.json`; no difference was approved.

The latency capture and `perf.py lsp` adapter are implemented and tested with
fake framed servers. They do not constitute a real performance result. Fixture
approval, a decision about the first-diagnostics metric (the pin only pushes
unversioned config diagnostics), threshold registration and the quiet-host run
remain outstanding.

## Closing batch

`target/phase5/harness/full-12` finishes at **19 failing parents / 32 failing
rows**, down from 77 / 97 in `full-11`: 58 formerly failing parents now pass and
no previously passing parent fails. N remains 4,117 with 417 native skips, so
the closure limit of 20 is met. No residual is approved. The LSP client suite
passes all 13 tests in `lsp-05`, and the compiler suite shows no difference
from its expectation file.

Import edits now follow the pin's specifier ordering: named-import insertion
detects case sensitivity and type-only placement with the native comparers and
binary insertion, module insertion compares raw specifiers (internal
import-equals included), and a top-of-file import skips a line break only after
a shebang. The auto-import index extracts module-augmentation exports under the
augmented module and resolves them from their own declarations. Package exports
use realpath module identities. A referenced project's declaration output is
extracted from its source, with entrypoint specifiers still keyed by the
original file. Program files reached through a `node_modules` symlink outside
the project directory belong to package indexes. Re-exports of ambient modules
get the native second pass. UMD import fixes build their `export=` export from
the symbol. Anonymous default exports take the target file's spelling.
Auto-import specifiers skip `node_modules` paths and keep the
realpath-relative candidate.

Completion now ports the pin's type-only and blocker rules for JS value
locations, unclosed type arguments, type queries, `with` statements, import
attributes, `import { type | }`, generator members, `as const` property
termination and the `assert` keyword. String literal completion walks
parentheses, drops private members and used `case` values, and uses
type-argument property constraints, which also supply literal completions.
Computed symbol members offer their first accessible name. Named imports alias
keyword exports, and class member snippets require the native location. Path
completions hide dot-files, and optional-chain filter text keeps `?.`. JSX
attributes quote `string & {}` unions. UMD globals yield to auto-imports in
module files. Untitled files get no exhaustive-case imports. The runtime no
longer unwraps a missing server on `didChangeConfiguration`.

Workspace clippy with warnings denied, formatting, `cargo xtask validate`, and
the tsr_ls (124), tsr_autoimport (32), tsr_lsp (40), tsr_checker (77) and
tsr_testhost (37) unit tests pass. A prepared harness must be rebuilt after
`tools/phase5/harness` changes: a build that predated the last harness commits
produced 53 false failures against the current server.

The 19 residual parents are eight completion cases (destructuring definition
locations, an unclosed index signature, two JSDoc type expressions and four
auto-import completion lists) and eleven navigation/project-state cases
(declaration maps, six find-all-references state baselines, transitive
export references, implementation search through a triple-slash reference,
interactive template-literal inlay hints and a CSS import rename).

## Correctness completion, 2026-10-07

`target/phase5/harness/full-15` passes **all 4,117 native-executed parents**:
5,922 passing result rows, zero failures and 417 visible native skips. The
compiled roster remains 4,534 tests. All 19 residual parents from `full-12`
now pass, including their individual baselines. Intermediate runs caught a
UMD reference regression and four delayed-project state regressions; focused
tests and the final full comparison verify their fixes. Both expectation
files are empty failing sets: `parity.py check fourslash` and `check lsp`
pass, with all 13 LSP client tests passing in `lsp-13`. No residual approval
is needed. This measured N belongs to the native run, not the source audit;
another platform or skip set requires its own native execution.

The production fixes preserve independent project and package export indexes,
local-name shadowing and JSX import usage; binding-pattern/JSDoc completion
contexts; default-export and internal import-alias references; range-only
implementation references; template-literal inlay parts; and the original file
move through cross-project CSS rename. Project cleanup retains config ownership
until file close, follows the nearest-config ancestor chain, and preserves
delayed ancestors. Opening a project precedes cleanup of existing overlays,
so a package already included in a configured program no longer survives in a
second inferred program. Dirty unrelated programs remain deferred until a
request needs them. Inferred-project resource identities are not parsed as
config filenames.

Focused verification passes 139 LS, 43 LSP, 150 project and 13 CLI unit tests
(the existing real-npm integration test remains opt-in). Affected-crate clippy
with warnings denied, formatting, `cargo xtask validate` and the focused
inventory/replay/latency/perf Python tests pass. The flaky npm cancellation
test now waits for child-process readiness before canceling, with bounded
deadlines; its focused stress run passes 30/30. Queue cancellation has its
missing unavailable-state/subsequent-item regression. The direct-test routing
inventory still names 93 WORK rows for L8; these are not counted as executed
native tests or silently certified by the fourslash results.

Replay now retains the six normalized native response transcripts in
`tools/phase5/replay/expected/`. Each new Go and Rust run must match its
committed transcript as well as the other runtime. A changed expected response
fails even when the live servers agree; missing or malformed expectations fail
too. Fresh ordinary Go and Rust CLI builds pass all three fixtures in both
UTF-8 and UTF-16 in `target/phase5/replay/l7-final`: all six live comparisons
and all twelve runtime-to-expectation comparisons pass. No comparison field
is dropped.

The latency tooling supports an explicit pull-diagnostics metric, fixed response
deadlines and a 20-to-40 extension that preserves and validates the original
samples and artifacts. The dispatch workflow is wired, and the five thresholds
are the plan's existing 1.0 values. The concrete TypeScript fixture and fixed
queries are prepared for review; dependency archives were checked against the
pinned lock. This is preparation, not a performance result.

## Latency smoke, 2026-10-07

The authorized TypeScript/pull scenario passes all three fresh-process smoke
pairs in `target/phase5/latency/smoke-03`. Both runtimes return 386 enum
completion items, 1,066 reference locations, a `Node` interface hover, four
local import-alias rename edits and an empty full source diagnostic report.
All remaining correlated traffic also matches. These three pairs are excluded
from the run of record.

Two harness corrections preceded this smoke. Shutdown must omit `params`:
the pin rejects explicit `null`. Completion wire order is native-process
dependent, so the latency comparator now stably orders items by
`(sortText or label, label)`, as the pin's editor contract and fourslash sorting
require. It preserves all fields, duplicates and equal-key order; raw frames
remain unchanged, and other arrays stay ordered. The rejected `smoke-01` and
`smoke-02` artifacts remain available. Regression tests reject changed payloads,
sort keys, item counts, tie order and changes to other response arrays.

## Latency measurement, 2026-10-07

The authorized capture at `7dfe0eaf` completed twenty alternating Go/Rust pairs
in `target/phase5/latency/capture-01`, using the ordinary release CLIs from
`target/phase5/replay/l7-final-binaries`. All twenty pairs passed the full
response and traffic comparison. No pair was replaced or discarded. The
three smoke pairs are separate and contribute no samples here.

The immutable [performance record](../status/perf/lsp/2026-10-07T05-06-35.795361+00-00-7dfe0eaf.json)
contains the binary/fixture/scenario identities, individual samples and
bootstrap statistics. Times below are medians in milliseconds; ratios are
Rust/Go, and intervals are 95% bootstrap intervals for that ratio.

| Scenario | Go ms | Rust ms | Rust/Go | 95% interval | Gate |
| --- | ---: | ---: | ---: | --- | --- |
| First diagnostics (open through full pull) | 21.682 | 110.835 | 5.112 | 4.943–5.301 | Miss |
| Completion after edit | 1.506 | 4.309 | 2.861 | 2.758–3.038 | Miss |
| Hover | 0.278 | 0.576 | 2.074 | 1.899–2.389 | Miss |
| References | 9.355 | 16.906 | 1.807 | 1.733–1.887 | Miss |
| Local import-alias rename | 0.204 | 0.391 | 1.916 | 1.750–2.067 | Miss |

Every interval is wholly above the unchanged 1.0 threshold, so the plan's
twenty-to-forty extension is not triggered. This is a measured regression,
not an inconclusive pass. No performance exception is approved.

Host: macOS arm64, 18 schedulable CPUs, `GOGC=100`, `GOTOOLCHAIN=local`,
`GOWORK=off`. No builds or other agent tests ran during the batch. Background
applications remained active: the one-minute load average was 4.32 before and
4.57 after; this was not an otherwise idle host. Host notes and raw frames
remain in `target/phase5/latency/capture-01-host` and the capture directory.
Sub-millisecond hover/rename ratios are especially sensitive to scheduling;
the result should not be generalized to other projects or hardware.

The measurement driver, focused latency/perf tests (43),
`cargo xtask validate` and `git diff --check` pass. CI passed on the production
correctness commit `8fed54f1`, including all fourslash, LSP, compiler,
concurrent compiler, transpile and native CLI jobs. No production Rust changed
for this capture.

## Closure and Phase 7 performance work

Owner decision, 2026-10-07: "you can close this one we can move all performance
related stuff to Phase 7." L7 is complete on its correctness, replay and
recorded measurement. The decision moves the performance acceptance boundary;
it does not turn the five measured misses into passes or relax their 1.0
targets. No further benchmark is needed for L7 or Phase 5 closure.

[Phase 7](../PLAN.md#phase-7-hardening-webassembly-embedding-cut-over) owns
profiling, optimization, further paired captures and performance acceptance
for all five scenarios. The first profiling target is the open/load/check
path: it accounts for the largest absolute gap, 89.2 ms. These end-to-end
timings do not yet attribute that gap to project loading, checker construction,
checking or protocol work. The record and raw artifacts above are the starting
baseline; the same fixture, response comparison and host limitations apply.

L8 remains the Phase 5 closure checkpoint, including the 93 direct-test WORK
assignments, port-completion review and documentation. Those correctness and
coverage tasks have not moved to Phase 7.

The closure also corrects a CI assertion that required every committed
performance record to pass and only accepted whole-second `Z` timestamps.
Recorded misses and precise UTC capture timestamps are valid; incomplete
ratios/intervals still fail validation, and `perf.py check` still returns 1
for a measured miss. All 45 focused latency/performance tests pass.
