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

N remains 4,117 with 417 native skips and no native failures. The current raw
failure set is 275 rows over 207 parents, with zero approvals; Phase 5's limit
remains 20 parents. The 13-test LSP client suite still passes in full.

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
checkout. This wiring is locally tested; hosted CI is still to run.

The latency capture and `perf.py lsp` adapter are implemented and tested with
fake framed servers. They do not constitute a real performance result. Fixture
approval, a decision about the first-diagnostics metric (the pin only pushes
unversioned config diagnostics), threshold registration and the quiet-host run
remain outstanding.

## Still to complete

The initial full fourslash run and exact failing set are recorded above.
Residual fixes, final approved retained sets, direct-test routing, CI validation,
replay CI routing and the latency workload remain in progress. The measured N above
belongs to that compiled native run; a new platform, binary or skip set requires
its own native execution rather than reusing the source count.
