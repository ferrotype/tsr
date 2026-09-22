# F5b review disposition

Review date: 2026-09-23. The reviewed implementation and evidence tranche is
`019ec1c`. The changes below are a new review increment: a scoped test passing
is not a fresh family capture or a completed F5b gate. Historical observations
remain retained; changed requests, operation claims and source inputs cannot
inherit their acceptance.

## Production and witness corrections

| Finding | Disposition |
| --- | --- |
| I02 | Confirmed. Checker literal formatting/truthiness and the evaluator now use the same borrowed `PrimitiveValue` operations. The checker no longer supplies a separate implementation behind shared-copy coverage. |
| I03 | Confirmed. Outer-expression classification, JSDoc-assertion detection and skipping now live in shared AST utilities. Evaluator and declaration transformer call them; constructed-node tests cover masks, operand selection and the JSDoc exclusion. |
| I05 | Confirmed. Binder and scanner position assertions now preserve the pinned Go debug payload; the scanner out-of-range regression checks the actual payload. |
| I30 | Confirmed. Retry helpers now cover Unix metadata/readlink and the affected OS stat/remove paths, including macOS symlink evaluation. A focused test distinguishes EINTR retry from other errors and already-wrapped errors. This is not a new Linux-native measurement. |
| I04, I35 | Confirmed. Evaluator descriptions no longer claim absent increment/decrement, assignment or synthesized outer-node inputs. The unterminated-literal description now says the unfinished template consumes the later regex/comment-looking bytes. No coverage is inferred for those unexecuted branches. |
| I36 | Confirmed. The navigation rescan test now constructs the containing JSX/non-JSX node and executes production classification before scanning, rather than supplying the fixture boolean directly. |
| I37 | Partly confirmed. The directives row cannot witness default settings overwritten by setup, nor a callback it never invokes. Those credits are removed. A new default-state request observes `NewScanner` before configuration and trivia skipping after `Reset`. `SetOnError` already has a separate valid `state/reset` witness: callback reinstallation affects the later invalid-byte diagnostic. |
| I31 | Confirmed. The native writer probe now accepts a locale. Twelve native/Rust traces compare German, Japanese and unsupported-locale fallback for nested chains, table headings, summary branches and plain/colored status lines. All twelve match and are included in the reviewed config-family capture recorded below. |
| I21 | Confirmed. The new paths request inserts `x*z` before `x*`, opposite lexical order, with equal prefix lengths and a separate exact-match control. Native and Rust choose the same file. Deliberately sorting the request changes the Rust observation, so the witness detects the claimed defect. |
| I23 | Confirmed. The snapshot-retention regression remains useful but did not exercise cached/tracking composition. B2-3 below adds the actual live OS → cached → tracking → program rebuild witness on both runtimes. Watch scheduling remains outside the claim. |
| I40 | Confirmed. `scripts/phase1_navigation_rescan.py` reproduces the five pinned private-operation observations without editing upstream. Its README gives the command; `--write` explicitly refreshes observations/provenance and the derived Rust TSV. The reproduced operation results match all five retained rows. |

## Harness and evidence corrections

| Finding | Disposition |
| --- | --- |
| I07 | Confirmed. Recorded outcomes now bind the exact request, operation claims, result and authenticated capture identity. Changing any binding leaves the historical result visible but unavailable for current coverage/preparation. |
| I08 | Confirmed. Request serialization preserves object insertion order at every depth; canonical sorting is reserved for observation metadata. The native paths negative control above demonstrates why this matters. |
| I09 | Confirmed. Syntax replay now rejects a changed source closure with a typed stale-capture result instead of returning a passing comparison with a warning. |
| I10 | Confirmed. A `later_step` preparation exemption does not remove an operation from Phase 1. Unresolved leaves/config transfers now appear in gap accounting; the current audit identifies 92 such entries. |
| I13 | Confirmed. Config and syntax no longer depend on unrelated installed-package/transport integration health or recomputing live whole-workspace classification. Their consumed audit/runner inputs are included in the producer closure and ledger. The inventory audit remains a separate check. |
| I14 | Confirmed. Rust-side witnesses now have explicit execution routes to metrics. The five contracts previously pointing only to `workspace` compilation get an exact named-test receipt, `rust-witnesses`; its named tests passed in the reviewed receipt; compilation alone still cannot satisfy the metric. |
| I15 | Confirmed. The 142 matchFiles observations contribute to both config baseline parity and filesystem completion. |
| I16 | Confirmed. Ordinary in-crate Markdown is excluded from behavioral capture inputs. Literal embedded Markdown and packages with arbitrary build-script inputs remain conservative inputs; archive receipts deliberately include package README files. The regression uses actual temporary package files and changes both prose and an embedded asset. |
| I18 | Extended. Reports now retain current request-bound results, historical raw outcomes, approved pairs, per-case cause classifications, expected contracts and missing-operation identities. Case reproduction selects the exact case; operation reproduction uses `phase1_coverage.py explain --operation ...`. A result classification is not a claim that every future semantic difference has already been diagnosed. |
| I19, I25, I43 | Confirmed. Source-stale family captures, integration receipts and auto-detected platform captures are unavailable contributions; independent current results survive. Malformed or altered artifacts still fail authentication. Staleness never becomes a pass. |
| I20 | Contributor checks reject empty direct-case and Rust-witness metric routes. The full-program and integration inventories now have separate nonempty denominators, source identities and named replay validators; no whole-program result is attributed to every leaf operation. Empty integration references remain pending, and completion requires all seven witnesses plus transport, generation and Rust-witness receipts. |
| I22 | Confirmed. Integration inputs now include `xtask`, `.gitmodules`, special API codecs, scanner table generation, the testhost manifest and program requests, alongside published-package/toolchain inputs. Literal source includes are followed rather than assuming a Rust source-file glob captures their data. |
| I26 | Confirmed. Ordinary CI uses the structural harness check; it does not fail simply because an unrelated edit makes the saved live classification stale. `inventory --check` still checks classification, and correctness producers still enforce their actual capture freshness. |

## Items that are not closed by these edits

- **I17:** the [destination audit](PHASE1-F5b-destinations.md) is complete as a
  scope review. The accepted plan supports 205 later-phase destinations;
  23 ordinary project-reference loading/configuration operations remain
  pending Phase 1 work. Build scheduling does not exempt ordinary reference
  loading. No new owner approval or blanket exemption is inferred.
- **I24:** metadata prefetch and the shell-loop correction exist. Local test
  results do not establish that the remote CI job has completed successfully.
  CI confirmation and accurate PR validation wording remain separate work.
- **I38:** the reviewed family and full-syntax captures now have repository
  archives, listed below. They replace the earlier target-only archival gap.
  Seven refreshed executable integration receipts are also archived.
  None of these archives waives operation coverage or host requirements.
- **I39:** port homes now include the EINTR helper and scanner binder helpers;
  syntax inventory references have been refreshed to the recorded E1/binder
  artifacts as stale where appropriate. The program helper pass now executes
  all 28 named tests, including the composed live-filesystem boundary witness.
  P1A/P1B remain incomplete: the current report has 1,365 pending
  entries—947 missing witnesses, 303 unverified implementation mappings,
  92 unresolved later-step transfers and 23 ordinary loader/configuration
  operations. The original 1,250 count is a historical checkpoint.
- **I41:** the Linux step now emits an authenticated platform summary, including
  unavailable outcomes. A real observed mismatch should still make CI red.
  A macOS run cannot supply the missing Linux result; upload alone is not a
  full filesystem-family acceptance record.

## Validation observed for this review

The bounded native/Rust checks observed 12/12 localized writer traces, 1/1
scanner-default trace and 1/1 source-order paths trace. Sorting the paths input
made the negative control differ. The native navigation reproduction matched
5/5 rows. The live-filesystem/program, constructed-node navigation and reversed
paths Rust regressions passed. Scoped clippy passed for the two harnesses and
the compiler/module/navigation test targets. Shared outer-expression tests,
scanner panic-payload checks and the EINTR unit checks passed separately.

The final gating review ran 83 focused tests with 87 subtests, followed by 16
family rejection tests with 15 subtests. The script suite passed **901 tests**
with **1 skipped** and **1,425 subtests**. These are local results, not a claim
of fully green remote CI, a Linux host observation or a new instrumentation run.

## Reviewed captures and remaining completion work

The final family captures are under
`target/phase1-f5b-review-20260923-03`. Each capture was replayed against the
reviewed source inputs and recorded with request, operation-claim and capture
identity bindings. Raw approved differences remain differences:

| Family | Matches | Approved differences | Other outcomes |
| --- | ---: | ---: | --- |
| Leaves | 229 | 1 | none |
| Filesystem | 355 | 3 | 1 Linux-only native-unavailable case on this host |
| Config | 498 | 1 | none |
| Syntax utilities | 1,123 | 0 | none |
| Historical pilot, ungated | 2 | 0 | 4 historical not-implemented observations |

The full program-syntax capture at
`target/phase1-f5b-review-full-20260923/full` matches **15,152/15,152** executable
native rows. Its derived smoke selection matches **908/908**; that selection is
not an independent full run or an additional acceptance denominator.

Both captures are archived in the repository:

- [`f5b-reviewed-families.tar.gz`](../data/phase1/captures/f5b-reviewed-families.tar.gz)
  (5.4 MB).
- [`f5b-reviewed-syntax-full.tar.gz`](../data/phase1/captures/f5b-reviewed-syntax-full.tar.gz)
  (18 MB).

All seven executable integration receipts passed on these inputs and are retained in
[`f5b-reviewed-integration-program-refresh.tar.gz`](../data/phase1/captures/f5b-reviewed-integration-program-refresh.tar.gz)
(about 1 MB), with their execution logs and registry. The preceding receipt
archive is also retained. The `program`, `foundations`, `config` and `syntax` producers now have valid
recorded results on these inputs. Foundations reports integration and Rust
witnesses complete; config reports all direct cases complete. The family
matches do not discharge the 1,365 remaining
operation entries, the Linux-only observation, or ownership instrumentation.
No performance benchmark is implied or required by these correctness captures.


### Program producer recovery and a measured loader difference

The refreshed program run completed 10,728 loader observations and 10,728
option-verification observations before its helper stage rejected old native
manifest metadata. Re-running the three native producers established identical
semver, package JSON and config observation bytes; only their manifests changed.
All 12 helper source checks and 28 named Rust helper tests then passed.

Helpers now run before the costly capture stages. Complete stages may be reused
only after authenticating their raw artifacts, exact ordered requests and pin,
checking current source inputs, rebuilding the current probe executables and
requiring identical binary hashes, then reconstructing every row and metric
with the existing comparators. This extra build check covers assets and Cargo
configuration absent from the older stage fingerprints. Missing/stale stages
recapture; corrupted stages fail. Twenty-five focused tests and 36 subtests
cover these boundaries, including preservation of genuinely failing results.

The loader result is **10,727/10,728**, not a pass; option verification is
**10,728/10,728**. The one loader difference is
`conformance/declarationEmit/typesVersionsDeclarationEmit.multiFileBackReferenceToSelf.ts#configuration=0`.
Its graph matches, but Rust omits two package-field resolution trace messages.
`tsr_module::Resolver::directory` removes a candidate's trailing separator before
comparing it with a cached package directory whose spelling retains it. The
pinned resolver keeps the candidate spelling at that comparison. This is a
pre-existing module-resolution defect, outside the reviewed production edits;
it was retained as a named failure, not an approved difference. The follow-up
below fixes it; the quoted full-run result remains historical.


Aggregate replay also exposed two receipt-format defects after the child
commands exited successfully. Localized integration output sorted the nested
Rust renderer request, invalidating its exact byte digest on replay; both
stdout and the saved comparison now preserve that order. The binder witness
filtered on the source filename instead of Rust's module path and ran zero
tests. Its corrected filter executes the five exact container tests; the
complete Rust-witness receipt now observes 15 tests across five contracts.
The six native localized envelopes pass both stdout and artifact round trips.
The final receipt set is checked through the aggregate validator before metric
recording; a successful child exit alone is not accepted. Earlier rejected
receipts and failed producer records remain preserved for diagnosis.

## B1-3 and B2-3 — owner decisions, 2026-09-23

**B1-3:** retain one independently authenticated capture per host. All filesystem
requests now declare `hosts`; `host_note` preserves the former prose. `any` and
`posix` require both supported CI hosts, Linux and Darwin. This is not evidence
for additional operating systems. The two native realpath implementations
remain Linux-only and Darwin-only respectively.

Comparison derives `not_applicable` only when native reports
`native_unavailable` and the declared hosts exclude the capture's GOOS. It keeps
the native reason and Rust outcome, and removes that row from the local parity
denominator. Native unavailability on an applicable host remains a failure;
harness failures are never excluded. Provenance GOOS must agree with every
hashed native probe response. Reports from different hosts cannot be folded by
`join`.

`filesystem_prepared` and completion require an authenticated acceptable result
for every `(case, required GOOS)` pair. Existing owner-approved differences
still require the exact native/Rust pair. Their request hashes were migrated
only after checking that removing the host metadata yields byte-identical
requests; their observations and exception scopes did not change. Neither a
Darwin result nor a prose note substitutes for the Linux observation. A missing
or stale host remains pending without discarding independent current families.

CI captures each runner's complete applicable inventory in
`target/phase1-platform/{linux,darwin}` and preserves the artifacts separately.
Replay accepts either that exact selection or a whole-family capture. Supply a
base with `--capture filesystem=DIR` and the other host with
`--host-capture OTHER_DIR` to `scripts/phase1_producers.py foundations` (or
`config`). `scripts/phase1_producers.py host --capture filesystem=DIR` verifies
and records one host without claiming multi-host preparation.

**B2-3:** `filesystem/composed/live-program-rebuild` follows the composition at
pinned `execute/watcher.go:431–490`. Native uses the real `watchCompilerHost` and
source-file cache from a same-package overlay. Rust uses production `OsFs`,
`CachedFs`, `TrackingFs`, `FileCache` and `Program::load_live`. The new explicit
live-host entry point creates fresh resolution caches; `Program::load` keeps its
snapshot-host requirement. Loaded bound files remain immutable, while the live
entry point documents that later host-dependent operations can observe changes.

The witness reads a physical config, seeds wildcard directories and the config
path in `SeenFiles`, builds, disables and clears metadata caches, physically
creates/deletes/edits files, then rebuilds with fresh cached/tracking wrappers.
Both observations include the new files/text/syntactic diagnostics, retained
first-file text/symbols after both programs are dropped, and the rebuild's full
tracking set including misses and wildcard directories. Paths are normalized
relative to `<root>`/`<realroot>`; no machine temp spelling enters the comparison.
An unchanged source must be reused, and the edited source must be replaced.
The observation does not claim watcher scheduling, incremental updates, emit,
or semantic diagnostics.

These changes do not close the operation-witness backlog. The owner explicitly
left stale family evidence and local-versus-CI confirmation for phase-end
refresh; neither is disguised as current by this increment.


## `typesVersions` trailing-directory follow-up

The resolver now compares the original candidate with the package directory
using pinned `ComparePaths` semantics, before trimming the separator for child
path arithmetic. A `../` import previously skipped `typings`/`types` lookup
because `/pkg` did not equal the cached `/pkg/`. This could also resolve the
wrong file when the declared `types` entry differs from `index`.

Three new pinned-Go module-trace fixtures cover cold and warm package caches
with distinct `entry`/`index` targets, plus the directory-import path without
`typesVersions`. The existing trace test failed before the fix on the first
new callback and passes after it. All 29 operations / 361 callbacks and all
13 module tests pass; scoped Clippy and formatting pass. An independent
read-only review found no further issues.

A fresh native/Rust run of the exact conformance variant named above now
matches every loader observation, including all 68 trace entries (Rust
previously emitted 66). The bounded comparison is preserved at
`target/f5b-typesversions-loader/comparison.json`. No full 10,728-row capture
was rerun or relabeled as passing; that producer refresh remains phase-end
work. Existing filesystem evidence likewise retains its original source
identity after this resolver change.
