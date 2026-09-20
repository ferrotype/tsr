# S07-bis exclusive binding: selected pilot and held general rewrite

Status: **single-source A0-b retained after its checkpoint screen; general
published-file rewrite on hold**.
Date: 2026-09-08. The initial on-hold plan is preserved in commit `4173b89`.
This file keeps its original name so references to Claude's alternative remain
traceable. The [revised primary plan](S07-bis-performance-plan.md) is the execution
authority and supplies the unchanged gates, workload, budgets and validation.

## 1. Decision after Claude's second review

Run **CP0 → A0 → architectural decision**, before building CP2's replacement
columns/locator/patch store or undertaking the general accessor migration.
If A0 passes, proceed with compact per-shape storage and applicable inline
binding fields. If it fails, CP2 is the retained side-table alternative.

The [implementation results](S07-bis-A0.md) now select the revised A0-b pilot.
The first A0 capture remains rejected and archived. CP2 stays on hold; passing
this bounded pilot does not prove the compact-layout budget or final gates.

The initial plan overstated the obstacle by treating a single-source consuming
entry as though it had to replace every published-file contract. The original
`FileCache::acquire` parses, publishes and binds before exposing `ProgramFile`;
loader consumers read from the bound result. The benchmark follows the same
sequence. A new exclusive entry can exploit that actual lifecycle while keeping
existing `bind_source_file(&AstFile)` behavior for already published inputs.

The old mapped-sibling review objected to group-wide initialization. Moving all
files to overlays was the author's broader design choice, not a requirement that
ordinary single-source binding must always follow publication. Keeping existing
code for the general path is less work than replacing that path immediately.
However, the two storage backends must share one binder algorithm and receive
independent coverage; retaining code does not remove that integration obligation.

## 2. Verified claims and bounded corrections

| Question | Code finding and plan decision |
| --- | --- |
| Does production expose a parsed view between parsing and binding? | No such exposure in `tsr_compiler::FileCache::acquire`; use that consuming entry point first, also in the benchmark |
| Is there a shared parse-cache once cell to replace? | No: `acquire` takes `&mut self`, stores weak completed entries and installs nothing on failure. Do not introduce negative caching or a new pending-cell protocol |
| Do published readers prevent A0? | They remain on the existing path. Exclusive input cannot have an escaping borrowed node across consumption; preserve this through compile-fail checks |
| What about post-bind parsed access? | `BoundFile::parsed_file()` currently exposes original parsed state. The new path needs a bound-only result or another explicit API; it must not falsely expose mutated storage as that old parsed snapshot |
| What about mapped/multi-source or transformed inputs? | Classify before mutation and use the existing checked path. Do not equate a single physical arena with one logical source |
| What about panic? | Private failed construction drops; failed cache acquisition still installs no entry. The old shared path retains initiator-panic, waiter-failure and terminal once semantics |
| Is committed lazy mutation already implemented? | No. `StorageTransaction::node_mut` only mutates pending nodes. Owner-exclusive access to committed Arc/OnceLock pages needs a uniqueness proof and implementation, or eligibility fallback before mutation |
| What is the main new borrowing problem? | Binder traversal currently holds parsed node/list borrows across recursive mutable calls. A0 must replace that lifetime assumption without cloning whole nodes/lists or adding a second graph |
| Do all owner checks disappear? | Repeated routing may disappear under a proved local scope; arbitrary raw, foreign/lazy and wrong-source IDs still require validation and safe bounds access |
| Does A0 remove the large field-map allocation? | Initially no: it removes full-node overlay/routing while keeping symbol/flow field maps. Removing those maps belongs to compact inline fields at CP3 |

Source anchors: [cache](../crates/tsr_compiler/src/cache.rs),
[loader](../crates/tsr_compiler/src/loader.rs),
[binder storage and result APIs](../crates/tsr_ast/src/bind_result.rs),
[binder traversal](../crates/tsr_binder/src/containers.rs),
[lazy storage](../crates/tsr_arena/src/lazy.rs).

## 3. Activated A0 prototype

1. Specify a consuming `bind_parsed(ParsedFile)` entry with a checked eligibility
   predicate, bound-result API and failure behavior. Keep current node layout
   and field maps. Preserve the existing published-file entry unchanged.
2. Implement the smallest shared binder storage abstraction that supports direct
   syntax mutation and the old overlay path. Resolve recursive traversal
   borrowing using indexed access, proven split storage or bounded observations.
   Include interface dispatch, generated code size and any replacement traversal
   work in measurement. No per-node/list allocation as a borrowing workaround.
3. Route compiler-cache misses and the benchmark through the same production
   entry. Published, mapped, multi-source, imported/shared-child or committed-lazy
   cases outside the proved exclusive contract take the old path before mutation.
   Record path counts/reasons, including newly created lazy records during bind.
   A fallback that accidentally handles the whole workload is not a successful
   exclusive experiment.
4. Define post-bind views and retained handles without inventing an unavailable
   pristine parsed graph. Audit factory/import/encoder use of the completed file.
   Preserve success reuse, cache retry after failed acquisition and the separate
   shared binding cell's existing repeated-call/panic/reentry behavior.
5. Extend corpus comparison to execute both entries. The current publish-first
   adapter otherwise tests only the old path. Record parsed graphs before
   consumption and compare full bound graphs, identity and repeat-bind results
   afterward. Keep both paths under the applicable debug/release and Miri/ASan
   ownership tests. Exercise eligibility rejection and final disposal explicitly.
6. Update full-workload graph, CPU and memory adapters to use the same entry as
   production. Exclusive order is parse→bind→publish; compatibility order is
   parse→publish→bind. All operations remain inside the same combined per-file
   interval, with unchanged preload/worker boundaries, queues and endpoint roots.
   Record the actual phase order, path counts and binary/source provenance.
7. Measure the full frozen workload at one/eight workers against the unchanged
   control. Require the primary plan's targeted and whole-pipeline screening
   exits. Include publication revalidation and field-map traffic still present.
   One-second binding is a later storage milestone, not an A0 promise.

Exit: keep the exclusive entry and proceed to compact inline fields, activate
CP2, or reject/revise the prototype for a named measured reason. Make that
decision immediately. A0 is not delayed until CP2 or final acceptance.

## 4. Shared layout work after successful A0

The large representation migration is common to both approaches. Implement the
borrowed facade and compact-parent headers, separate concrete payload shapes
from syntax kinds, and pool source-relative/cooked text with correct retention.
Store binding fields only in applicable shapes and remove their temporary maps.
Keep the 780 MB syntax / 100 MB binding sub-budget inside the combined 880 MB
physical-allocation budget; inline fields have real cost.

Model cumulative requests at CP0 and implement necessary list/buffer/growth
changes with the layout. Current 923 MB freed/superseded requests cannot be
left unexplained until CP6. Compact symbols/flows, prove duplicate validation
removal, and tune measured hashing/capacity work under the primary plan's
checkpoints. Apply phase continuation exits immediately after their relevant
storage/validation steps. All final gates and statistical rules remain unchanged.

## 5. General exclusive publication remains on hold

This is the materially different alternative still held: replacing the existing
published-unbound and mapped-file binding protocol with exclusive mutation too.
The new single-source entry does not require it.

Activate that broader work only if measured compatibility-path occupancy/cost
is material and the existing path prevents a gate from passing or necessary
production behavior from working. First resolve already escaped parsed views,
multi-source shared arenas, independent sibling failure/initialization,
committed lazy nodes, imported retention and post-failure parsed observations.

A shared OnceLock around mutable construction cannot revoke an existing read.
An undo journal, partitioned ownership or revised read protocol must be specified
and charged if needed. Reject a design that merely recreates the original overlay
cost under another name. A real observable divergence follows the project's
owner-approval process; the delegated engineering choice alone does not approve
changed source semantics. Do not infer a divergence from an internal API change
that preserves all promised observations.

## 6. What is adopted from Claude

| Suggestion | Revised treatment |
| --- | --- |
| A: ordinary binding before publication | Activated as A0 before CP2; existing published-file path retained |
| B: compact nodes, text and applicable inline fields | Selected shared implementation after A0; 24-byte header is the leading CP0 candidate |
| C: validation deduplication | Proof-driven experiment with full mutation/import failure coverage |
| D/E: hashing and capacity | Measured byte-semantic keyed hashing and exact/bounded reservation experiments |
| Model temporary traffic at CP0 | Adopted with native-counter identity and an explicit 350 MB provisional traffic ceiling; preload prevents treating the allowance as simply 200 MB |
| Phase CPU exits | Adopted as early continuation decisions; 1 s bind + 2 s parse does not yet satisfy the combined pipeline gate |
| 39% routing / +0.75 GB removable / duration in days | Not established by current measurements; A0 and later integrated captures determine actual costs and savings |
