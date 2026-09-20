# S07-bis: selected local binder migration completion

Status of this v1 freeze: implementation, source review, scoped correctness validation and the
combined performance screen are complete. The screen qualifies for review;
the full binder producer passes, but E3 rejects stale declared test inventories.
This version is not promoted. This records
`bindings.rs`, `declarations.rs`, `expressions.rs` and `statements.rs`, with their
dispatch/container/flow adapters, under the
[local binding plan](S07-bis-local-bind-plan.md). “Completion” names that slice,
not an entirely facade-free binder or completed S07-bis gates. CP1 remained the
accepted performance control at this checkpoint. The subsequent
[shared-text/inventory repair](S07-bis-local-text-repair.md) has its own complete
evidence and is now the experimental control; this v1 result is unchanged.

## Implementation and shared contracts

Expression/statement binding now carries `BindingNode`, `BindingList` and
resolved `BindingEdges` through operand evaluation, optional/logical branches,
loops, switch clauses, labels and recursive binding. The typed payload adapter
copies only the selected scalar fields and non-owning links needed across
mutation. It does not materialize an owned payload. Narrow flag/flow writes
remain separate from immutable syntax edges, and flags are reread after binding
where the algorithm requires live state.

Declaration/property/function binding carries the same syntax handles together
with `BindingSymbol` and `BindingTable`. New symbols/tables are minted locally;
reads and narrow mutations address their stable arenas by local slot. Ordinary
node symbol/locals reads and symbol/local-symbol/locals writes use generated
u32 fields directly; they do not reconstruct a public ID and pass it through the
general encoder. Checked variants remain available for exceptional references. State rows and table
entries still contain public links: following those links imports them into the
scope or retains the checked variant. This does not claim removal of every
reference codec or owner check from symbol processing.

The semantic inventories have one source rather than separate checked/local
algorithms:

| Inventory | Source and adapters |
| --- | --- |
| Node getters, kind-driven failure rules, shape-driven modifiers/body/function fields, locals-container shapes | `tsr_ast::node_semantics`, expanded by `NodeAccess` and `BindRead`; storage adapters select borrowed checked or local typed rows. |
| Concrete shape names, tags, declaration-name capability and payload/child access | The existing AST schema and `xtask::gen::ast_local_read`; generated local reads include the shape metadata needed by the shared rules. |
| Parentheses/partially-emitted wrappers, entity/dotted names, left-hand-side classification, optional-chain and logical/coalescing predicates | `tsr_ast::syntax_helpers`, used by checked `AstView` and scoped `LocalBind`; only storage reads and error types differ. |
| Fixed kind predicates and container classification | Shared utility predicates and the existing container-rule inventory; dynamic reads occur only for rules that need them. |

Mixed checked/local identities compare their canonical public identities;
local/local comparisons use their scoped slots. An invalid or not-yet-allocated
compatibility link is not silently promoted into a valid local reference.
Optional table results preserve absent keys versus present nil entries. Scoped
symbol/locals writes use the existing binding-storage rules, including an
explicit empty binding, promotion of a flow-only record, unsupported payloads,
field clearing and the precedence of materialized compatibility records.
Whole-arena replacement and unrestricted syntax mutation remain unavailable
inside the callback. Entry also verifies that the core's stored binding
namespaces match the current symbol/table/flow arenas. A checked caller can
replace arenas between callbacks; re-entry must reject that mismatch before
minting local handles, even when replacement slots have identical numbers.

## Review and remaining boundaries

Independent source review of the expression/statement/flow/dispatch integration
found no actionable regression in selected-field snapshots, nil/list ordering,
initializer early exits, conditional and try/finally flow restoration, binary
continuations or live error flags. The review checked behavior against the prior
implementation; separate Go execution is recorded below. Independent state review identified the arena-replacement
re-entry case above and six changed malformed-payload panic messages. The entry
check and regression now cover rejection, preserved checked IDs, escaped writes,
clearing and restored-namespace re-entry. Selected payload observations accept
the original call-site panic message; malformed infer-parent/export fixtures
exercise both backends. Final validation results are recorded below only after
their runs finish.

Explicit general boundaries remain: modifier and async/ambient queries,
assigned/dynamic/declaration-name helpers, diagnostic construction and source
metadata, unmigrated module/expando/resolver operations, declaration backing,
lazy or foreign nodes, and public flow/symbol/table links. Identifier text may
be borrowed locally before producing an owned `JsString`; other text forms keep
the checked text operation. Locals-container capability has a shared actual-shape
inventory (29 shapes), distinct from the 27 shapes with an inline locals field.
Unsupported state encodings retain their general storage path. Reaching a local scope for
every file does not establish that every operation inside it stays local.

## Cost and measurement ledger

The [single untimed native profile](S07-bis-local-bind-migration-cpu.md) covers
the preceding migration binary, not this later source. Its remaining
`AstView::node`/`NodeRead::data` samples primarily identify shared helpers and
validation; it provides no measured speedup for this completion slice. Its raw
trace, failed setup attempts and exact binary/source identity are archived.

The selected earlier binary stores 24-byte, 8-byte-aligned `Step` continuation
values; `Exit` is also 24 bytes. The current definitions retain that layout
structure. Final-artifact confirmation, peak continuation capacity and any
resulting request traffic remain unmeasured. Local/checked adapter enums and
selected-field tuples can enlarge live stack observations; local text conversion
can allocate. Borrowing typed rows alone does not establish a memory saving.

The [allocation attribution](S07-bis-allocation-traffic.md) still describes its
own frozen binary: 333.733 MB of traffic, including 108.289 MB unclassified.
This slice chooses no new backing policy and does not spend an assumed saving
from that report. All new or changed costs belong in the combined candidate's
allocation/RSS and parse/bind assessment.

| Obligation for this completion slice | Result |
| --- | --- |
| Affected debug/release/doctest, generator drift, formatting, Clippy and MSRV checks | **Pass.** Both modes: 30 arena, 158 AST, 45 binder and 63 tooling library tests, 29 AST integration tests and 37 doctests. All 22 generator outputs and pinned client bytes match. Final Clippy denies warnings; Rust 1.96 and formatting pass. Final check source inventories match. |
| Relevant strict-provenance Miri and ASan paths, including aliases/state presence and deep recursion | **Pass (scoped).** Miri: 3 mutable arena-scope and 27 AST local-binding tests. ASan: 27 AST and 20 binder tests, including deep binary chains. After lint cleanup, the 3 AST state cases were rechecked under Miri and all 20 selected binder cases under ASan. This is not a full ownership-producer or Darwin LeakSanitizer claim. |
| Full binder producer | **Pass.** 22,343 requests across 12,829 primary rows, 18 supplemental requests at parity 1.0, 18 helper tests and 32 protocol checks; graph contracts, resolver, depth and reached-bind obligations pass. Evidence `565fc34b…`; raw failure stream empty. |
| Full ownership producer | **Failed evidence.** All executed tests pass, but exact inventories are stale in every mode: storage 24 declared / 29 actual, publication 14 / 27, proof 3 / 9. Evidence `6d9ecbd8…` and raw logs are retained. |
| Frozen-source canonical Go/Rust graphs and local-path selection, one/eight workers | **Pass.** All 13,094 graphs match in each mode. Raw equality is 13,094 / 13,039 files; the existing canonical protocol validates the 55 eight-worker name-counter differences. Separate normal-binary probes enter the local scope and bind in place for every file, with zero checked/fallback files. |
| One complete combined screen against CP1: both CPU modes, allocation and RSS | **Pass, eligible for review.** Eight warmups and all 56 scheduled samples, verified from raw outputs; numbers below. No sample extension. |
| Parse-versus-bind elapsed attribution for that complete combination | **Complete, diagnostic only.** One consuming run from an adapter bound to the same production source: parse 2.230553404 s; bind/publication/validation 1.639655704 s. |
| Retain/reject decision and final Go-relative acceptance | **Retain as a measured candidate; do not promote this version.** The screen improves every distance against CP1, but E3 is false. Repair the inventories/filter and introduced text copy in a separately identified revision. All final Go-relative gates remain open. |

A component's CPU regression is not a hard veto on retaining a memory experiment
while its named dependent CPU work is integrated. Judge the actual combination
in one screen, using all gate distances and the existing noise/tradeoff rules.
Do not add separately measured savings or use a weighted score to waive a final
gate. If the fixed screen is inconclusive or regressing, retain that result and
make the next decision explicitly; do not extend it until a preferred answer
appears.

Initial checks are retained. Generator checking first hit the sandboxed default
Go cache; rerunning with the existing workspace cache passed. Clippy found
explicit-default test style, nested compatibility option states and ordinary
style issues. Compatibility selection now shares the existing `ScopedValue`
enum across flow, symbol and table adapters; actual table results retain nested
options because missing keys and present nil entries differ. One incomplete
enum conversion failed compilation and was corrected before the passing final
debug/release/lint/MSRV run. The final cleanup instrumentation has unchanged
before/after inventories. The complete earlier scoped run and its source
inventory remain recorded separately.

## Complete combined measurement

The normal executable is
`e03c7f8e8b5c554218cedb742709b47ae01acb2f11a340da24507e417bd7394e`;
the separate allocation executable is
`d7fae0ac93b1219d362c5ace9f1ad952a7fd4887a2c21d367949872e4acb43a7`.
Candidate manifest `f9be0d88…` binds 288 production source/configuration/protocol
files with fingerprint `2a381566…`. The pre-commit working snapshot is identical
to the production files committed in `7eae473`; hashes, not the recorded earlier
HEAD, identify the measured code. Fresh final and intermediate Cargo directories
were used. Both binaries use Rust 1.97.1 and the unchanged native release profile;
the allocation preflight passed before freezing.

The fixed screen report is
`b9d5cef9fb50504ab5c6aa8edeb1bdc8f76c79b8183edee22a56bce8f89f3337`.
All rows retain the frozen 13,094 files, 19,593,488 nodes, 2,459,867 symbols,
423 parse diagnostics and 5,250 bind diagnostics, with loaded-input digest
`d4ff2ad3b0d590babb596522a7105d961dbb3c4f2ebc987488d88b7ebd8e0c88`.

| Metric | Workers | Same-screen CP1 median | Complete candidate median | Candidate/control | Upper 95% timing ratio |
| --- | ---: | ---: | ---: | ---: | ---: |
| Wall, seconds | 1 | 4.546591458 | 3.908289208 | 0.859609 | 0.880953 |
| Wall, seconds | 8 | 1.062971209 | 0.890372083 | 0.837626 | 0.854378 |
| Allocation, decimal GB | 1 | 4.642569843 | 2.302069849 | 0.495861 | — |
| Allocation, decimal GB | 8 | 4.642571536 | 2.302071412 | 0.495861 | — |
| Peak RSS, decimal GB | 1 | 4.506714112 | 2.316779520 | 0.514073 | — |
| Peak RSS, decimal GB | 8 | 4.509220864 | 2.320007168 | 0.514503 | — |

One-worker wall improves by 638.302 ms (14.04%); eight-worker wall improves by
172.599 ms (16.24%). Timing relative MAD is 0.231% / 0.823% for CP1/candidate
at one worker and 0.497% / 0.955% at eight. Every noise/nonregression condition
passes. Allocation falls by 2.3405 GB and peak RSS by about 2.19 GB versus CP1.
This is the complete compact/local combination; it does not isolate the local
migration's CPU effect from earlier compact components or add their old results.

The new allocation total is about **57.9 MB above the preceding compact capture**
(2.244 GB). RSS is approximately unchanged from that capture. This is a recorded
request-cost increase, not a current per-family attribution: the new enums,
continuation buffers, owned text conversions and other temporaries remain part
of the measured combination. Its cause needs evidence; the earlier 333.733 MB
traffic split is not a census of this binary. Do not carry its old 209 MB request
deficit forward or claim that the CPU migration was free in memory.

A subsequent source audit finds a concrete introduced allocation:
`Binder::target_text` converts local identifier bytes with
`JsString::from_bytes`. The former checked operation retained a source slice or
cloned the exceptional text backing through `TextPool::owned`. Ordinary
declaration names reach the new conversion; name interning/table insertion
usually discards that temporary after canonicalizing the bytes. Wider `Step`
and active-label buffers are another unpriced request mechanism. This identifies
an avoidable copy, not its share of the observed 57.9 MB. The prepared repair
forwards explicit owned-text access through the local storage context while
preserving existing kind/shape guards and checked fallbacks. It is a changed
candidate and must not overwrite this screen or claim an unmeasured saving.

### Remaining distance to historical Go limits

The pinned original Go report supplies arithmetic context only. It is not a
fresh paired Go capture; no current Go-relative confidence bound is inferred.

| Metric | Workers | Candidate/old Go median | Required ratio | Candidate above old budget |
| --- | ---: | ---: | ---: | ---: |
| Wall | 1 | 1.330424 | ≤1.0 | 970.662 ms |
| Wall | 8 | 1.404455 | ≤1.0 | 256.409 ms |
| Allocation | 1 | 0.791779 | ≤0.7 | 266.844 MB |
| Allocation | 8 | 0.791569 | ≤0.7 | 266.305 MB |
| Peak RSS | 1 | 0.734171 | ≤0.7 | 107.831 MB |
| Peak RSS | 8 | 0.732509 | ≤0.7 | 102.962 MB |

All four distances improve against the same-screen CP1 control, and all remain
positive. Final acceptance still needs fresh Go-relative evidence, both CPU
bounds and the independent allocation/RSS gates.

### Phase attribution and limitations

One separately built timer adapter uses those same 288 production files,
12 explicitly recorded diagnostic support files, the unchanged normal mimalloc
policy and the existing `layout-profile` feature. It records exactly one
one-worker consuming invocation, with no warmup, repetition or shape traversal.
Its selected executable is `67925625…`; build manifest `05003a00…` and capture
report `1ccf6326…` bind its full identities. An independent preparation review
checked timer boundaries, retained roots, dependencies and artifact selection.

| Domain | Elapsed seconds |
| --- | ---: |
| Parse, including completion | 2.230553404 |
| Bind, publication and final validation | 1.639655704 |
| Sum of per-file phase timers | 3.870209108 |
| Diagnostic pipeline | 3.883679834 |
| Queue/dispatch and other time outside those phase timers | 0.013470726 |

Per-file elapsed timers include timer overhead and descheduling. Preload and
post-endpoint root destruction are excluded; roots remain retained through the
endpoints. The adapter proves in-place consumption, while the separate exact
normal-binary probes above prove local-scope selection. This is neither a
sampled self-cost ranking nor a paired phase speedup. Parsing is the larger
remaining phase here, and binding is still substantial; neither is assumed
capable of closing the complete CPU gap alone.

Offline replay verifies the screen from every stored row and recomputes the
phase arithmetic and historical distances. Initial distance postprocessing
incorrectly expected bootstrap fields on memory metrics and failed; it was
corrected to emit null for those fields. Raw screen evidence was unchanged,
and no workload was repeated.

## Broader ownership evidence failure and disposition

The full binder producer passes, including the published compatibility path and
repeated binding. The full E3 invocation finishes with outer exit 0 and false
ownership metrics; the wrapper correctly records a failure. All 29 executed
test summaries report zero failed tests. That does not make the producer pass:
its declared exact outcome sets disagree with the actual runs in all four modes.

The AST storage inventory omits five tests, and its loader still requires the
old `storage_tests::storage_*` naming convention. The validation-proof inventory
omits six tests. The publication group omits three genuine `bind_tests` tests;
in addition, Cargo's substring filter `bind_tests::` also selects ten
`local_bind_tests` tests. That accidental selection must not be presented as a
deliberate local-scope inventory. Correct the manifests and filtering explicitly,
keep exact-set rejection, and declare the intended local-scope coverage.

The complete failed E3 record is
`6d9ecbd86be80198dada7038c5655df6d338a7a40426592796cde3ab92c3a933`;
the passing binder record is
`565fc34baed6c5e2f263e35aa89d77eb2347c821505a71ae6f0de32b0ef6a135`.
Previous pointers and raw captures were preserved first. One sandboxed process
inspection failed before any producer invocation; that setup failure is also
retained. Neither producer was automatically retried. CP1 remains the accepted
control while the text-ownership repair and inventory correction proceed.
