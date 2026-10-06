# L7 implementation record

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

## Still to complete

Work is paused at the owner's request after this validated batch. Resume with
the 77 residual parents: completion contexts and fields, auto-import discovery
and type-only selection, and the smaller navigation/project-state tail. The
closure limit is still 20, and any retained failures require owner approval.
Remaining direct-test routes, CI validation and latency decisions/measurement
also remain open. No agents or measurement jobs were left running.
The measured N above
belongs to that compiled native run; a new platform, binary or skip set requires
its own native execution rather than reusing the source count.
