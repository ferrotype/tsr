# S07 implementation plan

Prepared 8 September 2026. This plan is stacked on S06 PR #10 and follows
[S07](../sprints/S07.toml), [PLAN §13](../PLAN.md#13-open-questions-and-pending-technical-decisions),
and accepted ADRs
[0006](adr/0006-node-ownership--arenas--lazy-file-storage--bundles-and-check.md),
[0007](adr/0007-symbol-ownership--file-owned-binding--checker-local-merges.md),
[0009](adr/0009-concurrency-model-kept-from-corsa.md),
[0011](adr/0011-deep-recursion--reserved-stacks-and-growth-guards.md),
[0012](adr/0012-panics--recovery-and-generation-retirement.md),
[0013](adr/0013-source-bytes--javascript-strings-and-wire-positions.md),
[0014](adr/0014-host-seams-as-traits.md), [0016](adr/0016-toolchain-and-lints.md)
and [0017](adr/0017-dependency-policy.md). The exact accepted
[symbol](design/symbols.md) and [ownership](design/ownership.md) contracts take
precedence over an implementation shortcut in this document.

The source authority remains TypeScript/Corsa commit
`1f70213d4922b434345f639b441681e470c7cfc1`, with shared toolchain pins in
`data/s04/toolchains.toml`. No canonical submodule edits are part of this work.

## 1. Outcome, scope and existing gaps

S07 completes binding, the dependency operations needed to load a frozen checker
subset, and measured parse-and-bind performance. It does not implement the
checker merely because later checker consumers need binding APIs.

| Existing sprint requirement | Concrete deliverable | Acceptance |
| --- | --- | --- |
| S07-1 | File-owned symbols, tables, flow graphs, bind diagnostics and both binder resolver helpers | Exact binder corpus parity plus separate resolver observations |
| S07-2 | Object-safe in-memory host, options, paths, resolution and bundled-library closure in `tsr_compiler` and its dependency crates | Every frozen subset variant loads the same ordered files, options, references and diagnostics as Go |
| S07-3 | Feature rule, primary manifest, effective options, exclusions and future-consumer operation inventory | Independently regenerated frozen subset; no skipped required rows |
| S07-4 | Identical pinned VS Code parse+bind runs with retained results, one/eight workers, actual allocation and RSS samples | RSS and allocated bytes each <=0.7 of Go; wall time <=1.0 at each worker count |
| S07-5 | Real bound files shared between program owners and retained through snapshot replacement | Two separate E3 criteria, with release import checks and final-drop evidence |
| S07-6 | Handwritten Go function mappings to actual implementations | At least 176/195 binder functions; target all 195 |

The source-only denominator is 167 functions in `binder.go`, 13 in
`nameresolver.go` and 15 in `referenceresolver.go`. `binder.go` alone cannot meet
90%. Generated functions remain separate provenance; mapping a wrapper does not
establish its behavior. Scope all three files explicitly in `PORTS.toml`.

Several advertised prerequisites do not yet exist: `data/workloads.toml`, a
frozen E2 subset, concrete symbol/flow payloads, a file binding cell, and a
program/snapshot owner. S06 has generic prepublication symbol primitives but
uses unit symbol payloads, and `ParsedFile` retains exclusive mutable AST
construction. These are implementation work, not presumed completed contracts.

S06's independent follow-up review also found shared-kind decoder construction
and a generic-storage escape from the concrete AST API. Those fixes must be on
PR #10, with current evidence, before this branch's implementation depends on
its retained-node or decoder APIs. The S07 PR stays stacked on PR #10 while that
base is open; no published history is rewritten.

## 2. Source audit and component boundaries

Before algorithm porting, generate a machine-readable dependency inventory from
the three pinned binder files, resolving each called AST, scanner, core and path
function against existing Rust mappings. Review actual semantics and call sites,
not marker presence alone. The initial audit found 126 direct AST helpers and
six scanner helpers. Missing families include full assignment-declaration
classification, module-instance state, expando initialization, declaration
containers, symbol-table operations, declaration-name formatting and token/error
ranges. Parser-private partial helpers do not cover the full binder contract.

| Component | Home | Responsibility |
| --- | --- | --- |
| Symbols/flow types and immutable bind result | `tsr_ast::{symbols,flow,bind_result}` | Shared types usable by AST, binder and later checker without a dependency cycle |
| File binding lifecycle | `tsr_ast` plus generic owner primitives in `tsr_arena` only where needed | One initialization, stable identities, exclusive writes, publication, retention |
| Binder | `tsr_binder::{state,declarations,containers,flow,expressions,statements,jsdoc,diagnostics,expando}` | Pinned bind traversal and state transitions |
| Name/reference lookup | `tsr_binder::{name_resolver,reference_resolver}` | Real hook-driven source algorithms; no fabricated checker answers |
| Source-facing helpers | `tsr_ast`, `tsr_scanner`, `tsr_core` | Correct shared dependency slices rather than binder-local copies |
| Options/configuration | `tsr_core` option values; `tsr_tsoptions` interpretation | Pinned defaults, configuration diagnostics and option projection |
| Paths/filesystem/resolution | `tsr_core::path`, `tsr_vfs`, `tsr_module` | Canonical identity, immutable in-memory files, supported resolver operations and caches |
| Libraries/programs/snapshots | `tsr_bundled`, `tsr_compiler` | Embedded source, file inclusion, bound-file sharing, immutable snapshot roots |
| Producers | `scripts/s07_*.py`, `scripts/s07_oracle/`, Rust examples | Frozen observations, strict comparison, measured benchmark runs |

Keep `lib.rs` as the public boundary. Do not add a new crate merely for each Go
file. Implement safe Rust under the existing unsafe prohibition; use typed
ownership errors separately from upstream panic contracts.

## 3. Binding lifecycle and ownership

### 3.1 Initialization before immutable publication

S07-bis implementation update: ordinary eligible `ParsedFile` inputs now use
the consuming parse→bind→publish path described in the
[A0 record](S07-bis-A0.md). Its `CompletedFile` capability exposes bound state
without promising a pristine parsed snapshot. The publication-first contract
below remains the supported path for already published inputs and for files
rejected by exclusive eligibility. This preserves the independent mapped-sibling
initialization requirement that motivated the original S07 implementation.

Binding changes more than a symbol side table: it writes parents/container
links, local/export symbols and tables, flags, CommonJS indicators, flow links,
source symbol counts and diagnostics. Inventory every assignment in the Go
binder before freezing the Rust storage shape.

Parsed AST cores and mapped-bundle membership are published before binding.
Each logical SourceFile owns its own `OnceLock<BindCompletion>` under the
canonical file/bundle retention root. Binding receives shared file access;
only the winning closure constructs that file's result, while competing callers
for the same file wait. Binding one mapped sibling must not initialize another,
surface its diagnostics, or consume its once state. A sibling failure leaves
healthy siblings independently bindable. No group-wide bind initializer or
failure state is introduced.

The winner reads the immutable parsed core and records writes in a private,
file-owned `BindBuilder`. This produces the accepted immutable `BindResult` plus
typed overlays for every binder-visible Node/SourceFile field that Go mutates.
The completed result is installed atomically in that file's binding cell.
There is no clone of the whole parsed graph and no mutation through a published
raw Node reference. Sparse staged node records may share immutable payload/list/
text backing; field-specific side tables hold symbol, local and flow links.
The prototype must audit every binder write and ensure all bound reads resolve
the corresponding overlay, including flags, parent/container links, source
metadata and diagnostics. A helper that bypasses the bound view and reads a
stale parsed field is a correctness defect, not an allowed representation detail.

Provide explicit parsed and bound views. Operations that require binding first
acquire the completed bound view; previously borrowed parsed views do not claim
to show binding state. NodeIds remain the same identities across these views.
Retained node/symbol access resolves through the applicable view and file owner,
rather than caching a raw prebind location for a later bound read. Integrate and
test encoder/helper/visitor reads of bound fields before porting binder algorithms.
Public bound queries cannot accept an unbound view accidentally.

The current checked binding root contract covers ordinary parsed logical sources
whose syntax belongs to that source. It does not cover rebinding a shallow
`SourceFile.Clone` or `UpdateSourceFile` transformation result that shares children
still parented to another logical source. Pinned Go can bind such a clone and
mutate the shared children while the original source remains unbound; Rust does
not represent that operation as an upstream panic or a passing parity case.
Source cloning retains its separate S06 AST contract. Transformer integration
must resolve shared-child binding semantics before claiming clone-plus-rebind
support; see [binding operations and the pinned witness](S07-binding-operations.md).

`publish_unbound` remains the S06 parser-tool path. It does not certify binding;
its file-owned cell can subsequently initialize a bind result without reopening
mutable core access. The cell remains inside the file owner after publication;
only private initializer staging is dropped on completion. Published file handles
retain their canonical file or mapped bundle, and mapped member handles retain
that bundle plus an index. There is no owning back-reference from the bind
result to its own file/cell. This same root owns parsed storage, lazy AST storage,
symbols, flows, overlays and diagnostics until the final retained result drops.

Freeze per-member bind traces for a mapped bundle: bind A only (B stays unbound),
bind B later, concurrent first requests for A and B, and a B panic followed by
successful A queries and first binding of a third healthy member. Compare each
member's initializer count, bound state and diagnostic timing with Go. Verify
sibling retention and final disposal independently of initialization timing.

No user-supplied initializer receives raw generic storage authority. Retained
AST/symbol handles expose checked concrete reads and explicit retention; they
cannot reopen generic lazy writers. The S06 foreign-edge discriminator applies
also to new symbol, table, declaration-list and flow edges.

### 3.2 BindResult and links

The immutable result contains, or resolves through its file owner:

- Its own symbol arena with a fresh process-global `ArenaId`, declaration lists,
  symbol tables and ordered member/export relationships.
- Flow nodes/lists and synthetic flow payloads, including cycles and shared
  nodes. A checked `FlowId` names its file and slot; private slots cannot escape
  without their owner. Sentinel/unreachable and incomplete loop-label behavior
  must match Go.
- Slot-indexed declaration/local/export symbol maps, locals maps, container
  chains, flow/return/end/fallthrough links and all binder-written node/source
  flags. Resolve Node-contract writes through typed bound overlays and use side tables
  for fields the accepted design replaces; do not mutate the parsed core.
- Source symbol count, bind diagnostics and deferred-expando state required by
  later queries. Temporary worklists disappear after completion.

Logical links use non-owning IDs. Ordinary same-file reads borrow; no symbol
lookup should allocate, clone a retention root or acquire the binding lock.
Cross-file imports validate identity, owner kind and published bounds in release.
File symbols stay stable across programs retaining that file. A retained symbol
keeps the complete mapped bundle if applicable, not only the symbol page.

Symbol names are `JsString`: upstream internal names use byte `FE`. Preserve nil
versus allocated-empty tables and lists, nil entries where the source permits
them, declaration order, duplicate suppression and slice aliasing during
construction. Published maps remain unordered; sort only at upstream observation
boundaries or in an explicitly canonical comparison representation.

### 3.3 Failure, contention and retirement

Freeze the successful, contended and failing state transitions before choosing
lock nesting. The winning binder runs once; repeated calls return the same
symbol identities and do not replay diagnostics or deferred work.

Go's `BindOnce` consumes its `sync.Once` after a panic but sets `isBound` only on
success. A bare Rust `OnceLock` retry would differ. Catch the initializer unwind
inside the one-time completion closure, discard unpublished staging and publish
a terminal failed completion, then resume the original unwind outside the
closure. Subsequent accesses must not rerun the initializer, expose a partial bound
result or report `is_bound = true`. The initiating caller resumes the original
panic on its own thread after the worker returns its Send unwind payload;
contending waiters and later callers receive a typed terminal `BindFailed`
ownership error. This asymmetry is intentional and tested for both direct worker
entry and dispatch from another thread. Go comparisons cover initializer count,
bound state and initial panic outcome; the extra Rust failed-file result is not
invented as a Go returned error. Parsed storage remains readable after failure,
but no caller can obtain a successful bound view for that failed file.

Detect same-thread initialization reentry before waiting; ordinary contention
waits. Reuse S04's thread-local initialization-guard mechanism, extended with a
binding/lazy operation discriminator so legitimate bind-to-JSDoc nesting remains
possible while reentering the same binding cell is diagnosed. Dispatch to a worker before acquiring the binding guard. Initializers
must not wait for work that needs the same binding cell. Test panic cleanup,
retry prevention, reentry diagnostics and two racing first callers. Do not claim
checker generation retirement from a failed binder; that remains a separate
later contract.

## 4. Binder and resolver semantics

Port all three source files with real operations, naming Rust helpers by
behavior while retaining exact source mappings. Preserve at least:

- Declaration conflict tests and includes/excludes masks; default exports,
  replaceable JavaScript properties, ambient merges, enum and namespace state.
- Value-declaration preference for nonassignment/nonnamespace declarations,
  declaration append order and uniqueness, local/export symbol relationships.
- Function-first statement traversal; container and block-container save/restore;
  strict-mode checks, labels, unreachable diagnostics and parse-error suppression.
- Assignment/destructuring state, narrowing flow labels, antecedent order,
  exception/finally/switch paths, short-circuit and optional-chain effects.
- JavaScript CommonJS/export/expando rules; delayed expando binding after the main
  walk; JSDoc namespace/type-alias/import declarations and their source identity.
- Signed source positions and open integer kinds/flags at the same widths as Go.
  Reuse S04/S05 text operations by their actual decoder/formatting contract.

NameResolver and ReferenceResolver are not module resolution and must not be
substituted by a table lookup. Preserve globals/excludeGlobals, meaning masks,
parameter/type-parameter scopes, use-before-declaration callbacks, arguments and
require fallback, deferred contexts, export-default locals and type-only aliases.
Define object-safe hook traits matching the source's injected operations. Test
absent hooks, hook result precedence, nil inputs, callback sequence and arguments
against Go. Future checker integration supplies real checker hooks; S07's test
hooks are identified as fixture hosts, not checker implementations.

Reuse or extract a helper only after proving the full source domain matches.
In particular, full `GetAssignmentDeclarationKind` cannot simply reuse the
parser's private binary-only branch. Add independent discriminators for every
new helper family, including invalid bytes, synthetic locations, malformed
payloads and exceptional results.

## 5. Binder observation protocol and corpus

Freeze `data/s07/binder-cases.json`, `binder-requests.json` and a probe manifest
from the existing authoritative S06 input expansion. Use the same 12,829 primary
physical/library IDs and 22,343 primary parser requests unless independent Go
preflight exposes a necessary, reviewed binding-specific representation change.
A source panic is an observed outcome, not grounds to remove a case. Every
required variant must match for its physical row to pass. The gate is explicitly
`run.binder.parity == 1`: a parse-divergent row fails S07 even if a future S06
capture could pass E1's 0.999 threshold. No E1 allowance transfers to binding.
The current S06 base has exact 1.0 primary parity.

All 22,343 primary requests are parser-entry inputs: 22,075 virtual-file requests,
160 initial-tsconfig requests and 108 library requests. Reuse their bytes,
loading route, script kind and parse options, then call BindSourceFile after a
successful parse. None of S06's supplemental encode/decode action requests enters
this binder denominator. Matching parse-terminal panics retain their rows with
bind-not-reached recorded; independently freeze reached-bind counts and require
every Go-reached bind to execute in Rust.

The clean pinned Go adapter parses, binds and dumps actual results. Rust calls
production APIs. Dump:

1. Node structural identities and every binder-mutated observable field.
2. Symbol identity graph: raw name bytes, flags, declarations, value declaration,
   parents, export symbols, members/exports/locals and table nil/empty state.
3. Flow graph: variant/flags, linked node identities, shared labels, ordered
   antecedents, synthetic payloads and nil edges.
4. Source counts/indicators and ordered bind diagnostics with byte-preserving
   arguments, range, code/category and related information.
5. Named resolver hook traces and repeated-bind/concurrency observations in
   separate supplemental groups.

Use cycle-safe canonical IDs derived from a declared traversal of roots and
ordered edge labels, not allocation addresses or global runtime IDs. For map
edges, canonicalize by raw key bytes in the dump only. Preserve edge multiplicity,
list order and graph aliasing. Diagnostic scoring is exact by default: a different order, argument or outcome
fails the row. Before freezing, an ambiguity may receive a named request/stage/
field qualification only after repeated independent Go processes demonstrate
its bounded alternatives. Record those observations and the exact deterministic
Rust alternative in `data/s07/diagnostic-ambiguities.json`. Such a row passes only
if both runtimes satisfy that named policy and all remaining fields/graphs match
exactly; it remains in the denominator. Preserve raw outcomes, exactness bits
and qualification counts. Unknown ambiguity fails closed and cannot expand the
policy automatically. Do not sort production operations or grant a blanket
exemption.

Strict records include request ID, stage, sequence and terminal outcome. Reject
missing/duplicate/extra/reordered records, unknown stages, non-finite metrics and
unclassified panics. Compare stable source panic messages exactly and runtime
bounds only through narrow classes. Use absolute per-request deadlines; a
hang/aborted request cannot pass as a matching error. Preserve raw stderr and the
first divergent graph path. Add negative tests that delete a flow edge, change
an alias target, reorder declarations, drop a variant or manufacture a panic.

Publish `run.binder.parity` only from the frozen primary test map. Require
separate `run.binder.resolvers`, `run.binder.graph_contracts` and
`run.binder.depth` metrics in S07 and the relevant ledger consumers. Supplemental
probes never inflate primary parity.

## 6. Frozen E2/E7/E8 subset

The selection rule must exist before Rust binding/loading results select any
case. Implement a pinned-Go syntax/configuration classifier under
`tools/s07/subset/`, with `python3 scripts/s07.py freeze-subset` checking and an
explicit `--write-manifest` operation writing the reviewed outputs. Do not widen
S03's generator command.

Version 1 is a feature rule across the entire compiler/conformance corpus,
including `conformance/controlFlow`; it is not a hand-picked list of passing
files. Before freezing, check in a total syntax/option classification table:
every encountered pinned SyntaxKind and directive has an explicit allow, reject,
or irrelevant-to-this-phase disposition with a reason. Missing classification
is a classifier error, never implicit eligibility or silent exclusion.

The proposed allowed feature families are primitives/literals, variables,
functions and overloads, object/array expressions, classes/interfaces,
enums/namespaces and their declaration merges, direct imports/exports,
unions/intersections, named type references without type arguments, simple
`typeof` type queries, ordinary control flow and narrowing. Explicit type
parameters and nonempty type-argument lists are rejected in source cases;
conditional/infer/mapped/template-literal types, indexed-access types, import
types, decorators and JSX are rejected by their exact node kinds. Project
references, content-mapper execution and emitted-output-only configurations are
rejected by explicit option/baseline predicates. All other syntax/directives
must be assigned deliberately in the table at checkpoint B; the rule is not
frozen until that table is complete and independently reviewed. Keep malformed
input whose recovered tree otherwise fits; do not exclude it for diagnostics.

The classifier records actual required dependency operations, including operations
introduced through declarations and libraries. Library dependency features are
an explicit future checker obligation, not silently erased from the inventory.
Checkpoint B creates `data/s07/checker-obligations.json`: each library-induced
operation (including generic instantiation and conditional types reached through
library declarations) names its input witness, pinned source anchors, owning
future sprint S08, required checker capability and expected E2/E7/E8 consumer.
This is the concrete S08 dependency-scope artifact produced by S07. It records
planned obligations, not checker implementation or passing evidence. Completeness
of this inventory blocks the freeze; implementation of its S08 entries does not.
Source-case exclusions cannot disguise library obligations.
Freeze every qualifying case and all qualifying effective variants; a feature
exclusion applies at the documented physical/variant boundary and records why.
Do not cap counts or shrink eligibility after observing Rust failures.

Before accepting version 1, independently inspect resulting counts by directory
and feature, ensure `conformance/controlFlow` contributes cases, and ensure all
required operation families and mandatory stress fixtures are represented. An
empty family or unsupported program operation found in an otherwise eligible
case requires implementing that operation or revising the proposed rule before
freezing; it cannot become a per-case success-based exclusion.

`data/s07/subset-rule.json` and `subset.json` retain raw/loaded hashes, virtual
files, options, exclusions, effective lib closure, baseline identities and the
E2/E7/E8 operation matrix. Keep original diagnostics and unsupported settings;
the seven S06 legacy `module:none` parser qualifications are not an E2 permission.
An unsupported option must be recorded as such with its source diagnostic, not
silently changed to a successful configuration.

Mandatory ownership, lazy storage, mapped bundle, declaration merging, malformed
input, recursion, reentry and panic-retirement fixtures accompany the selected
compiler cases with separate identities and later-sprint completion status. S07
freezes future checker fixtures without marking their behavior implemented.

The `e2` producer initially publishes only independently measured
`frozen_subset`; no checker parity, errors, types or performance metric appears
until those implementations execute. All consumers must continue to see the
other E2 criteria as absent.

## 7. Options, paths, module resolution and in-memory program

### 7.1 Host and operation matrix

Implement object-safe `Send + Sync` host traits matching the relevant source
seams. In-memory file entries contain immutable byte owners, canonical and
original paths, directory entries and declared symlink/case behavior. Host methods
return typed missing/unsupported outcomes; no hidden disk/network fallback.

The initial source-only package inventory is 178 core functions (22 files),
196 tsoptions (18), 88 tspath (3), 105 module (4), zero function bodies in the
vfs interface file, 28 bundled functions (2), and 340 compiler functions (14).
These are audit denominators, not a commitment to complete every package in S07.
Checkpoint B records the exact required function-ID closure and its exclusions
in `data/s07/dependency-functions.json`. Each touched ledger file stays
`in-progress` unless every function in that file is implemented; `ported` and
its nonempty verifier set require a complete real file scope. Untouched files
remain planned. No new package-ratio gate is inferred, and interface-only files
receive API/host tests rather than invented function credit.

Freeze `data/s07/operations.json` mapping required host/config/resolver/program
operations to source functions, supported inputs, observations and exclusions.
Support every operation used by the frozen subset and its library/dependency
closure, with negative tests for explicitly unsupported calls. This matrix is
also the dependency contract for E2/E7/E8.

### 7.2 Required semantic slices

- Preserve path normalization, drive/root and URL handling, case sensitivity,
  relative path rules and canonical cache keys using pinned `tspath`/core code.
  Both case-sensitive and case-insensitive virtual hosts get fixtures.
- Preserve compiler-option tristates/defaults and config diagnostics. At this
  pin, target defaults to LatestStandard and Unknown/Classic/Node10 module
  resolution settings are mapped according to module mode; do not import an
  older TypeScript default table.
- Support relative/absolute and package imports, extension substitution,
  package.json type/main/types/exports/imports and applicable conditions, type
  directives, `types`/`typeRoots`, `baseUrl`/`paths`, rootDirs, moduleSuffixes,
  allowJs and resolution mode wherever the frozen operation closure needs them.
  Freeze successful and failed lookup traces, not only the final filename.
- Resolver caches include containing directory, name, mode and redirect inputs;
  tracing bypasses cache hits as in Go. Do not reuse a cache across immutable
  filesystem versions unless its inputs prove equivalence.
- Load roots, references, imports, automatic types and lib references in the
  source's inclusion order. Preserve missing/duplicate/case-conflict diagnostics,
  source reasons and pinned default-library handling. At this pin,
  `parser/parser.go:6654` explicitly ignores `no-default-lib="true"`; preserve that
  behavior and freeze a loader discriminator showing that only the `noLib`
  option suppresses default libraries. `bundled.LibNames`
  alphabetic listing is different from the compiler's lib dependency/load order.
- Embed the pinned library bytes under `bundled:///libs` with their license.
  These bytes bypass filesystem BOM decoding. Source filesystem loading retains
  the S06 physical/virtual parser-text boundary.

`tsr_compiler::Program` owns its immutable host/options view and retained bound
files/bundles. It exposes source lookup, ordered files, resolution results, bind
results and the eventual checker.Program dependency seam without a fake checker.
Equivalent file requests reuse the same parsed/bound owner through an explicit
cache key including canonical filename, source bytes, parse options, script kind and applicable
mapper context. Separate programs may share owners while keeping their own
resolution/global/checker state.

The Go loader comparison executes actual pinned program creation/file-loading
operations, with checker work disabled at the source seam where possible. It
must not replace the loader with a second handwritten model. Compare file order,
bytes, effective options, references, module modes and diagnostics. Keep source
inclusion parity separate from binder parity.

## 8. Program and snapshot ownership evidence

Extend E3 with real S07 payloads and two independently gated scenario groups:

- `shared_bound_file`: two program owners share one completed bind result and
  the same SymbolIds; source/binder-name lookup agrees before and after either
  program drops. A retained symbol and mapped sibling survive their intermediate
  roots; final drops return measured counters to baseline.
- `retained_snapshot_edit`: an old immutable snapshot answers using old bound
  files while a new snapshot owns edited files. Cross-import of old/new file and
  symbol identities fails in release, even for matching numeric slots. Dropping
  the active snapshot does not invalidate retained old results.

Use deterministic barriers, not sleeps, for contended tests. Include concurrent
bind-once, bind panic and reentry, malformed symbol/flow references, mapped bundle
roots and a retained symbol outliving a program. Run the exact inventory in
debug, release, Miri strict provenance and ASan with rebuilt std.

These scenarios exercise actual program/file ownership and binder queries. Do
not manufacture checker instances merely to name a test: independent checker
merges, checker-result retention, pool retirement and full E3 remain later work.
The future checker-specific fixtures retain their separate pending metrics.

## 9. Pinned VS Code workload

There is no existing workload pin to reuse. Freeze the stable VS Code 1.136.1
source revision `a44adf7f53e00964ab890f9f8758a334f1fc15bc`, resolved from its
[release tag](https://github.com/microsoft/vscode/releases/tag/1.136.1) before
performance measurements. Record the full commit, repository URL, license,
archive digest, selection rule, options, library pin and ordered source hashes
in `data/workloads.toml` and `data/s07/vscode-files.json`.

The measured operation is parse and bind of all tracked `.ts`, `.tsx`, `.mts`
and `.cts` files in that tree, including declaration/test sources, with each
physical file included once. No node_modules/build output download, dependency
installation, module loading, checking or emit runs in this workload. These are
separate source files with the pinned parser's extension-derived script kind,
ESNext target and preserved declaration-file status; the exact effective parse
options are frozen and shared by both adapters. This explicit source-tree
workload avoids giving either implementation a different project-resolution
closure while still measuring the actual VS Code repository's parser/binder
work. Report its file count and byte count after independent export, before any
Rust result is used. The untruncated Git tree for this commit independently
reports 13,094 selected regular files and 161,740,237 source bytes: 12,750 `.ts`,
267 `.tsx`, 77 `.mts` and no `.cts` files. Archive reconstruction must reproduce
those totals and then freeze SHA-256 content hashes; a Git tree listing alone
is not a runtime workload capture.

Do not call upstream `BenchmarkBind` an E5/E6 measurement: it binds a handful of
preparsed optional fixtures. Both S07 binaries must parse and bind the frozen
VS Code inputs from the same bytes/options and retain every bound file until the
measurement endpoint. Require an independent, untimed Go/Rust comparison of the complete per-file
binder-observable graph representation from section 5 over this exact VS Code
workload before accepting any performance result. Hash the canonical graph
records with a cryptographic digest, preserve counts and first differences, and
require every file digest/outcome to agree. This is a separate required
`run.bindworkload.parity == 1` gate; counters alone cannot satisfy it.

Run that graph validation in separate processes. Measured RSS children do no
postphase graph dump or hashing allocation: after retaining the results they
read existing counters into fixed-size accumulators, emit a bounded scalar
report and exit. The process high-water mark includes that bounded report
cost, as it includes startup; it cannot be inflated by an unbounded validation
buffer. Input/work counters and frozen binary/source fingerprints connect the
measured operation to the independently validated operation.

Exports and reusable toolchains/caches live outside disposable Cargo cleanup.
Every download verifies its frozen source identity; offline runs use an already
provisioned archive/cache or fail with a prerequisite message. A moving branch,
missing source, partial export or unknown file never becomes a smaller workload.

## 10. Measurement contract and performance work

### 10.1 Allocator and worker integration

Install ADR 0016's pinned mimalloc allocator in native driver binaries, not as a
global allocator inside reusable compiler libraries. Proposed dependency is
`mimalloc = 0.1.48` with default features; the cached release and its
`libmimalloc-sys` closure must pass license/advisory/MSRV checks and all four native
builds. The native C build requirement is explicit. Miri/ASan continue exercising
the production compiler/owner code under their compatible instrumented allocator;
they do not certify mimalloc internals.

Allocation measurement uses a separate instrumented driver with
`cap = 0.1.2` and its `stats` feature wrapping mimalloc through its safe const
API. It reports original requested bytes across all worker threads, including
the full new size of successful reallocations, and adds no per-object metadata.
Keep counter snapshots and arithmetic checked and treat missing events as invalid
evidence. Instrumentation affects allocator calls, so neither E6 timing nor peak
RSS is taken from this allocation-instrumented binary. An eight-worker preflight
on the MSRV passed alloc/zeroed/realloc/dealloc byte accounting in debug/release;
the final adapters repeat those discriminators and record actual build identity.

The capability reason is actual allocator-request measurement without custom
unsafe Rust in compiler crates. Arena counters are rejected as a substitute;
`allocation-counter`'s thread-local System-allocator model would not match the
chosen allocator and eight-worker contract; `stats_alloc`'s generic const
constructor currently requires its nightly feature and its growth-delta counter
is not full requested-byte accounting. The initially proposed
`tracking-allocator 0.4.0` was rejected during preflight because MPL-2.0 is outside
the project's license allow-list. The selected wrapper's
[published source](https://github.com/alecmocatta/cap) uses MIT/Apache-2.0 and
records successful full requests. No dependency-policy exception is introduced.


Use one reusable bounded worker pool for a whole batch with exactly one or eight
workers. Both adapters use N persistent workers consuming a bounded queue of
file requests; Go uses N goroutines, not a hidden parallel compiler loader. Each Rust worker enters the reserved-stack/growth-guard environment once;
per-file `on_parser_worker` must not create additional threads. Go uses GOMAXPROCS=N, and the record states explicitly that this caps Go GC and
runtime work as well as binder goroutines. This measures each implementation
within the named Go processor budget, not equal GC scheduling costs; Rust's
mimalloc has its own documented runtime behavior. Record effective worker count,
GOMAXPROCS, GOGC and runtime thread/capacity observations alongside results.

The initial evidence-of-record host is this local macOS arm64 workstation,
observed before implementation with 18 physical/logical CPUs and 64 GiB RAM.
Record capacity again at capture, reject fewer than eight available CPUs, and
run without concurrent builds/other benchmark sessions. Local captures are
reviewed ledger evidence. Ordinary four-target CI runs correctness/ownership and
archived-view checks; only a designated capacity-qualified measurement runner
runs the full performance-dependent S07 gate. Do not fail or certify eight-worker
performance by pretending an underprovisioned hosted runner is this host. Other
native performance targets remain explicitly unmeasured until their own qualified
captures exist.

### 10.2 Raw samples and statistics

Freeze the configuration before sampling: one unrecorded process warm-up per
runtime, then seven fresh-process samples per runtime/mode, alternating Go/Rust
order. Each process parses/binds the full frozen workload once and retains the
same logical roots. The median is the fixed aggregation rule; retain all samples,
not just the fastest or passing subset.

Report median absolute deviation divided by the median (relative MAD) for each
runtime/mode. For timing, compute a fixed-seed 10,000-resample bootstrap 95%
interval for the ratio of medians, retaining the seed and algorithm version.
If the interval spans 1.0 or either relative MAD exceeds 5%, add one batch of
seven samples per side, up to 21 total; keep every sample. A remaining ambiguous
interval or excessive dispersion is measured uncertainty, not success.
`run.e6.stable` is required by S07 in addition to the unchanged median ratio
gates; it requires relative MAD <=5% on each side and an upper 95% timing-ratio
bound <=1.0 for both modes. Clear regressions may stop after the initial seven
samples and retain false gates. Do not repeat complete captures until random
noise produces a pass. A process failure, mismatched workload
count or invalid numeric sample invalidates that capture. A valid slow/high-memory
sample remains a measured failure.

Preload source bytes/options identically before the phase boundary. Report
preload size and process startup separately. Start wall time and allocation
snapshots at a barrier immediately before parse+bind; stop when all bound roots
are retained and workers have completed. The same uninstrumented child supplies phase wall time and lifetime peak RSS;
result serialization is outside wall time but its bounded cost remains in RSS.
Workload graph validation uses this same uninstrumented binary in an explicit
reporting mode in separate processes, with no change to its parse/bind/pool code.
The allocation driver is a distinct instrumented build with recorded feature and
source fingerprints and its own correctness smoke check. Preserve Go's pinned normal GC behavior (GOGC=100, no imposed memory limit)
and report effective values; do not disable GC to make Rust look smaller.

Measure allocation passes with Go runtime.MemStats.TotalAlloc deltas and Rust
original-layout allocation request bytes, recording their precise measurement
domains and realloc accounting. Include worker setup inside the measured phase
or exclude it on both sides by a pre-created-worker barrier; use the latter
consistently. Keep preload bytes outside both allocation deltas. Report Rust
wrapper overhead separately and never subtract real compiler allocations.

Measure peak RSS in fresh, uninstrumented processes using OS child resource usage
with macOS byte/Linux KiB normalization. Peak RSS includes startup and preload;
report those components but do not subtract an incomparable guessed baseline.
The launcher measures one child per resource-accounting process so a previous
child's high-water mark cannot contaminate a sample. Record the allocator,
architecture, OS, toolchains, build profile, GC settings, worker count, commit
identities and exact source/benchmark fingerprints.

Compute each ratio as the Rust sample median divided by the Go sample median
for the same metric and worker count. `e5` records separate one/eight-worker RSS
and allocation ratios; its existing scalar `peak_rss_ratio` and
`allocated_bytes_ratio` are each the maximum of their two per-mode ratios.
Thus both worker modes must meet the memory gate, with no favorable-mode
selection. `e6` emits each mode's median wall-time ratio separately. Both
producers include every numerator/denominator, raw sample, sample count and
workload completeness result. The existing thresholds remain <=0.7 for both
memory metrics and <=1.0 for both time metrics. S07 does not emit
`type_footprint_ratio`, and therefore does not complete full E5.

### 10.3 Responding to measured failures

Measure an early complete parser/binder vertical slice before polishing all
interfaces. Profile allocations, node/payload sizes, map growth, copied bytes,
lookup costs and lock/atomic counts. Optimize the observed costs: per-kind arena
layout, large payload separation, reserved vectors, borrowed text/lookup paths
and removal of duplicated metadata are candidates, not predetermined wins.
Change authoritative generators when representation changes; retain one
production representation and rerun semantic/ownership evidence after changes.

Do not pool live bound results across samples, remove source files, compare
parse-only Rust against parse+bind Go, change GC defaults asymmetrically or
relax thresholds. Failure to meet the performance gates means S07 is not done;
record the failure and continue concrete optimization or report the remaining
measured limit truthfully.

## 11. Recursion, portability and validation

Extend the cycle audit to binder recursion and combineFlowLists. Implement the
binder's explicit binary-expression trampoline required by PLAN §13.11, plus
production stack guards for other recursive components. Fixtures include long
left/right binary chains, nested blocks/functions/types/JSX, destructuring,
namespace/JSDoc nesting, deeply shared/cyclic flow labels and exceptional paths.
Small-stack optimized tests must prove actual segment growth; a large test stack
alone is not evidence. Avoid recursive graph serialization/drop paths that
circumvent the binder's protection.

All four supported native targets build and run binder/host tests, generator and
corpus producers, instrumented ownership and MSRV checks. Preserve independent
capture and artifact-upload paths after a measured failure. Benchmark captures
must identify host capacity and availability; do not assert portable performance
from one machine. WebAssembly growth strategy, browser execution, embedding and
full checker/emit acceptance remain E7/E8 and later phases.

Required checks: workspace debug/release tests and compile-fail docs; strict
Clippy and formatting; dependency/MSRV checks; generation; S04 text and S05
scanner regressions where their selected inputs change; S06 E1 and E3 current;
all new producers, negative harness tests, traceability and generated views.
Inspect metric truth and freshness after capture, not just command exit status.

## 12. Evidence registration and consumers

| Producer | Declared input groups | Measured output and consumer |
| --- | --- | --- |
| binder | Binder/AST/scanner/core/owner implementation closure, clean upstream pin, exact corpus and supplemental manifests | Derived corpus parity; graph/resolver/depth gates; S07-1/6 and source-file ledger |
| bindworkload | Production parse/bind and benchmark-driver closure, VS Code pin/options/file manifest, clean upstream pin and strict graph protocol | Separate exact full-workload parity required before E5/E6; no corpus-row contribution |
| program | VFS/options/module/bundled/compiler closure, subset rule/manifest/options/operations and pinned libs | Complete load/resolve parity and `subset_loads`; S07-2 |
| e2 | Independent subset classifier, clean oracle preprocessing, frozen rule/manifest/baseline and operation identities | Only `frozen_subset` in S07; S07-3 and future E2 consumers |
| e3 | Existing real runtime closure plus binder/program owners and exact added scenario inventory | Actual shared-bound-file and retained-edit outcomes in all required modes; S07-5 |
| e5/e6 | Both benchmark adapters, complete production closure, allocator/pool integration, sample/aggregation configuration, immutable VS Code workload and toolchain pins | Actual ratios and completeness; S07-4 |

Audit every existing consumer before adding a metric. Add regressions for absent,
false, partial, stale and wrong-workload evidence; a seven-scenario S04 capture
must still not close full E3 or S09. The separate `bindworkload` capture must match the VS Code workload/options
fingerprints and the corresponding production driver/source fingerprints used
by E5/E6; a parity result for an earlier workload or implementation is not a
performance prerequisite. A subset-manifest success cannot close full
E2, and parse/bind measurements cannot close per-type E5 or E7/E8. Source-set
selection must include every helper read by a producer without invalidating
unrelated leaf evidence needlessly. Generated output-scope files are inputs too.

Keep raw reports, failures, effective toolchains and input hashes in immutable
artifacts. Registered producers write no tracker status by hand. Update editable
ledger fields only for real implemented scopes, then regenerate STATUS.md,
status/status.json, the unmapped-function worklist and docs/status.html.

## 13. Work checkpoints and exits

| Checkpoint | Work | Required exit before dependent work |
| --- | --- | --- |
| A | Fresh S06 review, valid fixes and source/API baseline | PR #10 pushed, affected evidence current; concrete review disposition recorded |
| B | Dependency inventory, frozen subset proposal, VS Code pin and measurement preflight | Independent Go-derived manifests/counts, named operation closure; allocator/counter/worker prototype verified |
| C | File binding cell, Symbol/Flow storage, publication and failure state machine | Real concurrent bind-once prototype, stable IDs, import/retention and panic tests; Miri/ASan on actual payloads |
| D | Binder traversal/declarations/flow plus missing helper families | Frozen binder graph/diagnostic parity over full corpus, exact failure records; no unimplemented grammar branches; an informational early parse+bind layout/allocation measurement precedes interface polish |
| E | Name/reference resolvers | Independent callback and lookup traces, all 195 source functions audited |
| F | VFS/options/module/bundled/program and immutable snapshots | Every subset variant loads with Go-equivalent file/order/options/diagnostics; both E3 program scenarios pass |
| G | Full VS Code benchmark and optimization | Usable frozen samples, ratios meet existing gates; final source layout retains semantic parity |
| H | Independent implementation review and final verification | Valid findings fixed, all affected captures current; S01–S07 pass locally, native CI results stated per target |

Useful parallel boundaries are binder control-flow versus resolver algorithms
once the shared API is fixed; host/module loading versus graph-observation tools;
and independent Go fixture construction versus Rust implementation. They share
one authoritative contract and frozen request identities. Avoid concurrent edits
to common owner/generator APIs or regenerating evidence while source changes.

## 14. Plan review protocol and unresolved prototype decisions

This is a plan, not evidence of implementation. Before implementation, post this
plan in a PR based on S06 PR #10. Resume the Claude session that reviewed the S06
plan, using Fable 5.1 at high effort. Ask it to read this plan only, without source
code review, subagents or extra model calls, and post concrete plan findings to
the new PR. Wait for its completed review, verify each finding and amend the
plan where valid before starting S07 code.

The structural choices needing prototype validation are the file-owned binding
cell and per-file mapped initialization, exact symbol/flow storage sizes, the
complete binder-write-to-overlay/side-table mapping, lock nesting and the shared
reentry guard, source-derived subset counts/operation closure and future checker
obligations, and allocator instrumentation preflight. Diagnostic scoring is
fixed above; any concrete named ambiguity still needs independent observations
before policy freeze. Each has a concrete checkpoint above. None permits
invented success metrics or a silent change to an accepted contract.

After Claude's review, append a disposition table linking its comment and
recording the accepted/rejected findings with reasons. During implementation,
record new contract corrections separately from the historical plan review.

### Internal plan review before publication

An independent plan-only review and bounded reread found the following issues;
the author checked them and applied these corrections before Claude's review.

| Finding | Disposition |
| --- | --- |
| E5 had no rule combining one/eight-worker memory results | Both per-mode median ratios are retained; each scalar gate is their maximum |
| Postphase graph hashing could inflate lifetime RSS | Complete workload graph parity runs separately; measured children emit only bounded scalar reports |
| Mapped siblings could lose their shared retention root | The initial group-initialization proposal was superseded by Claude finding 2: retain one bundle while initializing each member independently |
| Feature families did not form a total subset rule | Complete syntax/directive classification is required before freeze; unclassified input is an error and library obligations must map to S08 |
| Workload counts were not a concrete correctness gate | Exact full-workload graph parity is a separate required `bindworkload` producer |
| Workload parity could stay current after benchmark inputs changed | Its source/input declaration includes the VS Code pin, options, manifest and adapters; fingerprints must agree with E5/E6 |

Claude's completed review and the dispositions below supersede the original
pending-review status. No S07 runtime implementation or measured success is
implied by plan acceptance.

### Claude Fable 5.1 review disposition

The original S06 review conversation was resumed as session
`99dbdfb6-0388-4124-b52f-ff5cd7c851b2`, explicitly configured with
`claude-fable-5-1` and high effort. The completed
[plan-only review](https://github.com/iantocristian/ts-rust/pull/11#issuecomment-5576522620)
used no subagents or implementation/source review. The author verified the
findings against this plan and the governing sprint definitions before editing.

| Review item | Disposition |
| --- | --- |
| 1: Exact binder threshold / E1 allowance | Clarified `parity == 1`; parse-divergent rows fail S07 and no E1 allowance transfers |
| 2: Group binding changes observable behavior | Accepted; replaced group initialization with per-file once cells and immutable bound overlays under one retained bundle; sibling timing/failure traces are required |
| 3: Undefined S08 scope blocks subset freeze | Accepted; S07 creates a concrete checker-obligation inventory owned by S08; implementation remains deferred |
| 4: Panic versus waiter outcome | Accepted; initiator resumes original panic on its calling thread, waiters/later callers receive terminal typed failure; both routes are tested |
| 5: Diagnostic nondeterminism scoring | Accepted; exact by default, only independently demonstrated named bounded qualifications can pass, with raw data and denominator retained |
| 6: Measurement host | Accepted; designated local 18-CPU/64-GiB macOS arm64 host, capacity rechecked per capture; ordinary CI does not certify unavailable performance |
| 7: Noise rule | Accepted; fixed relative-MAD/bootstrap policy, bounded additional samples, separate required timing-stability gate |
| 8: Go concurrency / GC budget | Accepted; N bounded-queue goroutines and GOMAXPROCS=N explicitly include Go runtime/GC scheduling in the recorded budget |
| 9: Corpus and workload evidence coupling | Accepted; separate `bindworkload` producer and inputs |
| 10: Retention lifetime | Accepted; file-owned binding cells remain with the canonical file/bundle root; only private staging is discarded |
| 11: Unresolved decisions inventory | Completed for field overlays, lock nesting/reentry and concrete ambiguity observations |
| 12: Dependency slice size/ledger treatment | Added source-only package counts, required-ID closure artifact and truthful partial-file ledger states |
| 13: S06 request carry-over | Specified parser-entry categories and excluded supplemental codec actions; bind-reached counts are frozen separately |
| 14: Measurement pairing/builds | Same uninstrumented child provides wall time/RSS; same binary reporting mode validates workload graphs separately; allocation build is distinct and identified |
| 15: Early measurement checkpoint | Added informational vertical-slice measurement to checkpoint D |
| 16: Reentry mechanism | Reuse S04 guard mechanism with binding/lazy domain keys |

No divergence approval is inferred from the request to proceed. The mapped-file
proposal was corrected to preserve per-file initialization, rather than approved
as a new observable behavior. Implementation may now begin at checkpoint B/C;
measured acceptance still requires every stated gate.

### Implementation contract corrections

The typed binder-write inventory found two source-defined symbol names that
embed process-global numeric identities: ambient modules with import-attribute
patterns (`binder.go:314`, Node identity) and private class identifiers
(`binder.go:375`, Symbol identity). Their observation records must retain the
literal name bytes plus a structured reference for precisely that embedded
identity component. Compare the reference by canonical graph identity; retain
the raw numeric name for diagnosis. Ordinary names and all other bytes remain
exact comparisons. This is graph-identity canonicalization, not a diagnostic or
name waiver, and its discriminator fixtures must reject changed delimiters,
descriptions and referenced identities.

Allocator preflight rejected `tracking-allocator 0.4.0`: its MPL-2.0 license is
outside the accepted dependency allow-list. The implementation instead selects
`cap 0.1.2` with its `stats` feature, wrapping `mimalloc 0.1.48`. The published
MIT/Apache-2.0 source exposes a safe const constructor and counts original
requested bytes across all threads, including the full new size on successful
reallocation. It adds no per-allocation metadata. An eight-worker preflight on
Rust 1.96.0, both debug and release with `unsafe_code = forbid`, observed exactly
9,600,400 requested bytes and zero final live-byte delta, and checked zeroed
contents and preservation through reallocation. Allocation-instrumented builds
remain separate from wall-time/RSS measurement. The future benchmark's locked
transitive allocator version is recorded with its actual build, rather than
assuming a cached `libmimalloc-sys` version. No license-policy exception is made.
