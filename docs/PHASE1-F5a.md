# F5a: integration checks and coverage review

F5a's runnable integration and tracker infrastructure is implemented on top of
F4b (#51). **Phase A is not complete.** The report keeps the remaining operation
attribution, platform and phase-boundary work visible; it does not reinterpret a
passing corpus as an individual witness for every function it might execute.
This is the scheduled coverage-review point before F5b.

## Report and remaining work

The complete operation → request/action → native observation → Rust driver →
comparison → producer → sprint join is committed as
`data/phase1/coverage-report.json.gz` (about 270 KB compressed). Rebuild or verify:

```sh
python3 scripts/phase1_coverage.py report --output target/phase1-coverage.json
python3 scripts/phase1_coverage.py check --output data/phase1/coverage-report.json.gz
python3 scripts/phase1_producers.py check
```

The report has all 4,795 operation IDs, 1,164 case IDs, and exactly 309 baseline
output paths. The 142 matchFiles outputs have one owner and are consumed by both
filesystem and config checks without creating a second denominator. Six old
pilot cases remain historical preparation evidence, not production acceptance.

| Pending classification | Count | Work needed |
| --- | ---: | --- |
| Operation witness missing | 2,490 | Link an exact native/Rust action or reviewed equivalent mechanism; a file-level metric does not suffice. |
| Implementation unverified | 315 | Inspect the pinned operation and actual Rust home; failure of name matching does not establish an absent API. |
| Compiler destination ambiguous | 44 | Resolve the proposed phase 4–6 placement against the accepted syntactic boundary; no scope waiver has been made. |
| Total | 2,849 | Exact IDs, homes, owners, dependencies and representative cases are in the report. |

The family preparation reports retain 2,803 syntax and two filesystem operation
entries. The 44 ambiguous compiler entries are additional: their old roster
exemption was never a reviewed Phase 1 scope decision. Another 186 compiler
operations now have explicit destinations supported by the plan's checker,
emit, project-reference and mapper exclusions. Every original operation ID stays
in the report. The 18 earlier generator/platform exclusions also remain visible.

Examples of the attribution distinction are
`AccessorDeclarationBase.computeSubtreeFacts` (a known Rust home, no exact
witness) and `AccessorDeclarationBase.IsAccessorDeclaration` (name-derived
uncertainty, not a reproduced missing operation). These are coverage work before
new production code is inferred to be necessary.

The recorded family comparisons are 229/230 leaves, 355/359 filesystem,
485/486 config and 83/83 syntax utilities. These are historical results, not
freshness claims. Three raw `different` rows have owner-approved ownership
qualifications: compiler-option clones, retained filesystem entry values and
immutable package `Parseable`. The new `approved-differences.json` binds each
qualification to the pin, exact request and complete native/Rust observation
pair. Extra field, identity, ordering, value or JSON-type differences fail.
The raw comparisons are preserved. The two Go dynamic-type filesystem
mismatches and the Linux-only observation remain unqualified.

## Integration witnesses

`data/phase1/integration.json` names seven runnable witnesses: localized config
diagnostics, ordered package and config resolution, retained program snapshots,
live cached filesystems, installed generated assets, and real compiler syntax
diagnostics. Case witnesses reuse existing identities; additional Rust tests
have exact names. Source existence establishes preparation, never execution.

The localized-config fixture has run. It parses a config containing an unknown
option with locale `de-DE`. The leaf diagnostic catalog returns German, while
the production diagnostic writer returns English. The comparison uses the
pinned Go German catalog as explicitly labeled data authority; it is not
presented as execution of a Go baseline renderer. F5b must propagate locale to
the writer and the baseline adapter, then add a localized config envelope
comparison. Existing English baseline equality does not discharge this gap.

All 89 S11 cases are mapped to continuing obligations, with the five actual Go
mapper stream recordings retained. No configuration callback, plugin method
proxy, blocked synchronous filesystem bridge, parse-cache injection or semantic
fourslash coverage is claimed. ADR 0019's strict Unicode wire limitation stays
explicit. Existing valid full captures can be reused through their validators;
this preparation does not launch a new transport or benchmark campaign.

The `gen` producer now wraps its existing command and additionally executes the
locale table generator's exact `--check`, exposing `run.gen.locale_complete`.
Existing AST/diagnostic/API metrics and untouched-client checks remain intact.
The wrapper does not treat the mere presence of locale files as success. The
pinned Node/npm bootstrap remains `target/s03-runtime/node_modules/`.

## Producers and receipt commands

Three grouped adapters are registered: `foundations`, `config`, `syntax`.
Their default capture roots are `target/phase1-acceptance/{leaves,filesystem,
config,syntax,program}`; `program` is the complete syntax corpus capture.
They only replay, never launch a corpus or compile. For existing directories:

```sh
python3 scripts/phase1_producers.py foundations \
  --capture leaves=PATH --capture filesystem=PATH \
  --capture config=PATH --capture syntax=PATH
python3 scripts/phase1_producers.py config --capture config=PATH --capture filesystem=PATH
python3 scripts/phase1_producers.py syntax --capture syntax=PATH --capture program=PATH
```

Missing captures remain unavailable (config/syntax test rows are skipped, not
passed). Explicit stale, partial, corrupt or malformed captures are rejected.
A source-stable replay writes a content-addressed report under
`target/phase1-producers/`; its digest and contributing capture identities are
printed to stderr, which xtask retains in the evidence record. Raw reports and
captures must remain available alongside recorded evidence.

Non-case integration witnesses can be executed individually, with their fixed
commands and before/after source fingerprints:

```sh
python3 scripts/phase1_producers.py observe --witness localized-config-diagnostics
python3 scripts/phase1_producers.py observe --witness retained-program-snapshot
# The returned receipt can be passed, without editing it, to:
python3 scripts/phase1_producers.py foundations --receipt PATH_TO_RECEIPT
```

Successful source-stable execution records its receipt path in
`target/phase1-acceptance/integration-receipts.json`; the normal foundations
producer replays that registry. Explicit `--receipt` arguments replace it for
a scoped development replay. Old receipt files are preserved.

`transport`, `generation` and `installed-generated-assets` are also named
executable witnesses. They are explicit commands, not automatic side effects of
`check`. Case witnesses use normal family captures. Receipt replay validates
exact commands, current inputs, output hashes and the actual named test/required
result inventory. Missing receipts stay pending; an exit code alone cannot pass.
Failed or source-changing executions retain stdout/stderr for diagnosis.

For tracker recording, place the chosen valid family captures at the default
roots (a local symlink is sufficient), then:

```sh
cargo xtask run foundations
cargo xtask run config
cargo xtask run syntax
cargo xtask status
cargo xtask check P1A
cargo xtask check P1B
```

P1A consumes only preparation metrics. P1B separately requires every family,
309 config results, the full syntax/parser/binder comparisons, generation,
transport and existing quality/safety checks. Neither currently passes. P1A's
integration readiness also requires the complete coverage review, so fixing the
syntax witness list cannot accidentally hide the 44 unresolved scope decisions.
No timing metric, relaxed threshold, new target or reopened S12 gate was added.

Grouped freshness covers the union of contributing family dependencies,
requests, native helpers and manifests. Because the scope classifier scans Rust
names and port markers across the workspace, grouped records also fingerprint
all Rust source files. Individual captures still use their own smaller input
sets. Unrelated documentation changes do not stale these code-only captures.
Scope classification now derives the unmapped set from the pinned inventory,
PORTS and markers; generated status views are not producer inputs.

## Review queue

1. Finish the exact operation attribution and inspect name-inferred gaps before
   proposing more ports. Retain the complete list; do not grant blanket corpus
   credit.
2. Resolve the 44 compiler placement ambiguities against the accepted plan.
3. Keep the two filesystem dynamic-type differences and Linux-only case visible;
   they are not covered by the three ownership approvals.
4. Implement locale propagation and the missing installed locale/message
   consumer coverage in F5b; execute the other prepared integration witnesses.
5. Reuse captures accepted by the normal validators and refresh only invalid
   relevant correctness inputs for closure. No benchmark refresh is required.

## Validation of this increment

- 902 repository script tests pass (one existing platform skip); focused Phase 1
  tests were repeated after the final producer wiring changes.
- The new Rust fixture builds and runs; clippy with warnings denied is clean.
- Formatting, package policy and tracker validation pass.
- The real generator wrapper passes: no drift, client equality, and locale-table
  regeneration. The initial sandbox-denied Go cache read is retained as a failed
  receipt; the normal-cache retry passed.
- The current syntax utility capture has 83/83 matches. The new grouped producer
  replay consumes it and records the German diagnostic integration difference.
  Missing family captures and unresolved coverage remain false/pending.

The full F4 parser, binder and syntax observations remain preserved. This step
adds a test-only compiler example, which changes the conservative capture
closure; it does not silently relabel an older source-bound capture as current.
No full parser/binder/checker capture, timing benchmark or ownership campaign was
started by F5a.

`data/phase1/captures/f5a-integration.tar.gz` retains the complete 83-case utility
capture, the exact grouped report and the two measured integration receipts
(about 1.3 MB compressed). It contains no binary or full corpus. The report's
identity matches the foundations evidence stderr; the archived raw comparisons
can be replayed normally after extraction. The final focused Phase 1 suite has
146 passing tests.
