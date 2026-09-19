# S07 implementation review record

The implementation follows the amended plan on PR #11. The original Claude
Fable 5.1 plan review and its disposition remain in
[the plan](S07-implementation-plan.md#claude-fable-51-review-disposition).
The implementation review used separate readers for ownership, program/config
boundaries, source selection, and benchmark evidence. These reviews do not
replace measured parity or the sprint's acceptance thresholds.

## Findings and fixes

| Finding | Resolution and counterexample |
| --- | --- |
| Equal file/node/symbol counts did not establish benchmark input identity. | Both native children digest their actual preloaded bytes, names, paths and parse options before measurement. A same-size source edit and an input reorder must change that digest; every sample is checked against frozen recipes. |
| Guessing Cargo's output path could copy a stale executable when caller configuration redirected the build. | Consume the exact successful Cargo compiler artifact, validate its target/features/profile, copy it into the measurement cache, and record its digest. The regression builds with an alternate output directory and a stale default-path executable. |
| A configuration fingerprint did not prove the claimed release profile. | Enforce the release profile and configured package overrides explicitly. Reject compiler overrides that cannot be neutralized. The native regression starts with abort, no LTO, and conflicting optimization/codegen settings. |
| Allocator preflight could use a configured cross target or runner. | Build a native example artifact with the declared profile, then execute it directly. Debug/release checks also run on the exact MSRV. |
| Persisted Go settings could select emulation or a nondefault collector; mimalloc environment options could alter memory behavior. | Preserve registry/cache settings, disable persisted compiler configuration, select the native Go host, and normalize experiment/tuning and mimalloc overrides. Tests supply conflicting settings and preserve private-registry values. |
| Instrumentation overhead existed in raw samples but was omitted from summaries. | Report separate normal/instrumented Rust time and RSS medians, including their possibly different sample counts. These informational ratios do not alter E5/E6. |
| The measurement consumer checked provenance keys without checking all values. | Validate compiler pins, host shape, fixed measurement domains, revision and digest types, raw sample identity, and recomputed aggregates. |
| The accepted plan's resolver and graph-contract metrics were absent. | Derive both from the complete named helper and graph/protocol obligations and require them in S07 and native CI. Omitted, replaced, duplicate and failed obligations cannot pass by count alone. |
| Focused configuration regressions were outside the integrated helper gate. | Register the 25 direct Go config fixtures, their native test target and nonwriting source check alongside the full variant comparison. |
| Workspace JSON feature unification changed the tracker’s canonical ledger hash. | Use an explicit ordered outer map. The preexisting Python-compatible non-ASCII hash regression passes with all workspace features enabled. |
| The binder observer generator used a different Rust edition from the workspace. | Read the edition from the workspace manifest, then verify generated output and ordinary workspace formatting agree. |
| New local path dependencies omitted the version required by cargo-deny's wildcard policy. | Declare their existing 0.1.0 package version; no dependency or policy exception is added. |
| Operation-inventory and binder-depth progress contaminated their containing producers' JSON output. | Send progress to stderr and exercise the helpers as embedded calls. The tracker rejected the contaminated E2/binder captures; their artifacts remain archived. Fresh captures are required after correcting the output boundary. |
| Repeated package-JSON source checks rejected equivalent Go diagnostics. | The pinned JSON dependency deliberately chooses `json: cannot ` or `json: unable to ` per process. Qualify only this prefix in a failed parse's top-level error; preserve every remaining byte and reject changed message bodies, unknown prefixes and source/manifest drift. The helper report retains both raw diagnostic sets, affected paths and hashes. |

Ordinary bound reads and retention also needed to select the target logical
source's completed overlay. Mapped siblings and sources sharing one arena now
route independently; retained nodes keep their actual source owner. Separate
regressions reject unbound siblings, orphans, parent cycles and attempts to stage
another logical source's headers. These are ownership contracts, distinct from
the factory-copy behavior below, and run in the exact E3 inventory.

The final ownership review identified bound-source cloning losing published
flags/CommonJS metadata. Factory copies now select the completed binding of
the imported logical source; parsed views stay parsed and copies remain unbound.
A real parser/binder regression matches 17 observations from unchanged Go. A
separate multiple-source regression is included in E3's exact ownership inventory.

The public program constructor also silently accepted unsupported
project-reference and external-mapper execution. It now rejects these requests
before building a resolver or touching the file cache. Three parsed-config
regressions exercise the errors, empty lists and disabled-mapper diagnostics;
all are included in the program helper inventory.

## Evidence interpretation

The first CI run exposed a missing build input in all four MSRV jobs:
`tsr_bundled` embeds the pinned upstream copyright notice and 108 library files,
but the MSRV checkout did not initialize submodules. The shared MSRV job now
checks out `upstream`, as the other build jobs already did. The local MSRV result
had used an initialized checkout and therefore did not exercise this failure.
A fresh local checkout reproduced the missing-file errors before submodule
initialization. After checking out the exact gitlink, Rust 1.96.0 passed
`cargo check --workspace --all-targets --all-features --locked`. An independent
asset audit confirmed that all 109 embedded upstream files are committed inputs;
no generation step or nested submodule is required.

The frozen source subset is selected independently of Rust results. Its operation
inventory records static calls, named function references, dynamic/interface
boundaries and package initialization dependencies separately; it is not a
function-parity percentage. The function ledger leaves partially implemented
source files in progress.

Binder traceability is 188 of 195 handwritten functions (96.4%). The seven
unmapped functions were inspected separately: the two `ActiveLabel` target
methods have direct Rust closure equivalents; `getBinder`/`putBinder` use Go
pooling replaced by scoped Rust construction/drop; three private helpers have
no callers in the pinned binder package. The shared signed-numeric AST helper
is implemented. No missing reachable binder behavior was found in this set;
absence of binder pool reuse is an implementation difference included in the
workload measurements, but its contribution has not been isolated.

E3's S07 ownership groups do not complete the entire E3 experiment. The subset
freeze does not establish checker parity. Benchmark graph parity is required for
both worker modes before timing/allocation evidence is accepted. A completed
measurement above a threshold remains a failed gate, even when correctness
comparisons pass. S07 measures only E5's parse/bind criteria; checker per-type
footprint remains unmeasured. See [S07 evidence](S07.md) for current results.

## CPU diagnosis follow-up

The [CPU report](S07-cpu-profile.md) retains twelve profiles and eighteen fresh
controls on the unchanged compiler/benchmark sources. All thirty invocations
match the frozen input digest and work counters. AST lookup paths account for
38.9%/36.4% of sampled Rust binding CPU at one/eight workers; final binding
validation accounts for 2.6%/2.4%. These are path costs, not promised savings.

Independent review corrected capture provenance and cleanup, preserved control
binary hashes, separated elapsed worker timers from CPU, and retained unknown
sample mass. DWARF inline display can hide a physical wrapper name, so Rust
analysis verifies the executable identity and resolves physical address ranges
while preserving displayed source metadata. Go analysis keeps unlabelled GC
and labelled runtime work in its denominator. Neither inclusive callback time
nor the sampled `madvise` location is reported as lock waiting or GC by itself.

Analyzer countertests and an independent real-trace accounting review pass.
Reproducible targeted exports are archived with provenance; native traces stay
local. Production code and E5/E6 evidence remain unchanged, and S07-4 stays open.


## Comparative memory diagnosis

The [memory report](S07-memory-profile.md) records twelve paired native diagnostic
runs and six calibrated Rust source-scope runs. All eighteen match the frozen
work and leave compiler sources and acceptance evidence unchanged. Retained
censuses, exact runtime counters, Go sampled allocation stacks and OS RSS/VM
snapshots have separate denominators; Go's parser-allocated binding fields are
explicitly distinguished from Rust's later side tables.

Independent reviews fixed stale-stage/configuration provenance, permissive
completion records, missing scope-artifact validation, and a cold macOS mutex
allocation that contaminated enclosing source scopes. The fixed-request probe
then matched exact nested and eight-thread counts. The Go observer caches
reflection field paths after its original inspection cost was measured;
inspection still runs after retained snapshots and its retirement RSS effects
remain explicit.

The final accounting review reconciled category totals, 79.24% approximate
allocation-origin share, map/slot capacities and instrumentation perturbation
against all captured records. Validation passes 29 Python tests, three Go
census tests, four Rust census tests and the release allocation-scope probe.
The archive preserves all 470 raw/provenance/analysis files and exact staged
patches with hashes. Independent replay reproduces all eighteen run summaries
and all ninety native Go exports exactly. S07-4 and the existing E5/E6 failures
remain open.
