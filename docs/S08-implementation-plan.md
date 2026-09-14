# S08 implementation plan: checker, type display and measured storage

Prepared 10 September 2026 on `codex/s08-plan`, based on merged `main`
`4c0818d` (S07-bis PR #12). Status: planned; no S08 implementation or passing
checker evidence is implied. The accompanying [review](S08-plan-review.md)
records the source checks and amendments made before implementation, and the
independent review of 10 September 2026 whose amendments are folded in below.

Source authority: TypeScript/Corsa
`1f70213d4922b434345f639b441681e470c7cfc1`. Use the existing toolchain pins and
the [Rust guide](CODEX-RUST-GUIDELINES.md). The requirements are
[S08](../sprints/S08.toml), the accepted
[symbol contract](design/symbols.md), [ownership contract](design/ownership.md),
[text contract](design/text.md), and ADRs
[0004](adr/0004-the-owner-approves-baseline-divergences.md),
[0007](adr/0007-symbol-ownership--file-owned-binding--checker-local-merges.md),
[0008](adr/0008-checker-mutation-model.md),
[0010](adr/0010-order-sensitive-outputs-are-enumerated--comparators-are-port.md),
[0011](adr/0011-deep-recursion--reserved-stacks-and-growth-guards.md), and
[0012](adr/0012-panics--recovery-and-generation-retirement.md).

## 1. Deliverable and scope

Implement a usable checker over S07's immutable bound programs, with exact
`.types` and `.errors.txt` observations for the frozen subset. Type display must
go through the production node builder and printer. Complete the required
checker-local merge, recursion, text integration and measurement obligations.
This is a substantial semantic port, not a type-annotation evaluator.

The workspace holds the S08 scaffold and nothing more of the checker:
`ts_checker` (owner and operation scope, the resolution-cycle guard, checker-local
link stores, the type record and alias store, flags checked against the pinned Go
package, and the program-host contract), `ts_printer` (the two text writers) and
`ts_nodebuilder` (flags and the symbol-tracker contract). None contains a type
construction, relation or checking algorithm. `ts_compiler` loads programs and
verifies their options; its public module explicitly excludes checker/emitter
construction. `run.e2` currently verifies only the source-selected
denominator. `CheckerIdentity`/`CheckerLease` are generic identity and permit
primitives, not a concrete checker or retained-type API.

The source inventory has **10,728 effective variants**, selected from 12,721
physical cases. The owner-approved S07-3 amendment makes **9,369 variants E2
acceptance** and retains **1,359 as informational differentials without an E2
gate**; see [the accepted amendment](S08-acceptance-amendment.md). The source
closure retains **111 loaded library identities and 675 checker obligation
records**. The obligation records are syntax/dependency observations, explicitly
not evidence of executed checker operations. They include generics, overloads,
mapped and conditional types, inference, indexed access, template literal types
and JSDoc in loaded declarations. A shallow implementation of literals, objects
and unions cannot certify this denominator.

What the complete source inventory holds, before the E2 acceptance partition: nine syntax
families have no eligible variant at all, because the rule excludes every variant
containing them in test sources: explicit type parameters (2,628 excluded
variants), decorators (765), JSX (455), indexed-access types (393), mapped types
(313), conditional types (279), import types (124), `infer` types (122) and
template-literal types (66). Everything else is in: control flow appears in 9,587
eligible variants, declarations and merging in 9,931, declaration members in
5,888, expressions and bindings in 7,654, imports and exports in 2,183, JavaScript
sources under `allowJs` in 895 and JSDoc in 558. Effective frozen options set
`declaration=true` in 1,450 variants, `composite=true` in nine and
`emitDeclarationOnly=true` in 109. Pinned Go requests declaration diagnostics
for 1,459 variants; option-key presence alone overcounts the obligation. The exclusions remove
generic *syntax in test sources*, not generic *semantics*: the 111 loaded
libraries bring `Array<T>`, `Promise<T>`, the lib's mapped and conditional helper
types and generic signatures, so instantiation, type-argument inference at every
call to a generic library signature and the relations they need are required work.

The following restriction applies within the owner-approved acceptance boundary.
Informational variants do not create required checker implementation work.
Do not change the source selection, effective options, library closure, malformed
cases, `skipLibCheck`, `noCheck`, or baseline eligibility to accommodate Rust.
Port the operations these programs and their actual queries require. A selected
case that reaches an unimplemented operation remains a named failure. If the
dependency audit exposes work beyond the initial module estimate, expand the
implementation worklist; changing sprint scope or acceptance requires a separate
owner decision.

| Requirement | Concrete completion evidence |
| --- | --- |
| S08-1 | Real checker-owned symbols/types/signatures and independent declaration merges over the same bound files; required E3 scenario in debug, release, Miri and ASan |
| S08-2 | Every frozen variant has the same type and error baseline outcome as pinned Go |
| S08-3 | Source comparators, actual union-ordering checks and named residual ID-sensitive fixtures |
| S08-4 | `TypeToString`/`TypeToStringEx` through generated type nodes and production printing |
| S08-5 | Actual checker, relation and printing recursion/reentrancy paths exercised with reserved stacks and growth guards |
| S08-6 | No accepted mismatch without an exact applicable owner-approved divergence; the current allow-list is empty |
| S08-7 | Measured per-type footprint on the subset, Rust/Go **≤0.80** |
| S08-8 | E4 literal-type bytes and original-source/regenerated literal printing, including malformed bytes and WTF-8 |
| S08-9 | Fixed checker workload: throughput, allocation traffic and retained bytes against Go, excluding parse/bind |
| S08-10 | Real arena-reference/interior-mutability relater prototype, fixture parity and measured comparison with the production ID-based relater |

S08 includes the declaration-transform and emit-resolver operations required to
produce declaration diagnostics on the frozen subset (decision below). It does
not claim general `.js`/`.d.ts` emit parity, the full language service, API
registries, shared-pool response commitment, WebAssembly or embedding. S09 owns
the complete registry/retirement integration and E3 completion; S10 owns the
portable checker/consumer gates. S08 must supply real reusable owners and host
interfaces for those integrations. It cannot make their metrics pass through
smaller S08 tests.

## 2. Selected implementation approach

Use the accepted **ID-based, single-threaded `&mut self` checker** for production.
Keep upstream resolution, relation, inference and narrowing control flow and
side-effect order recognizable. Copy small descriptors before recursive calls;
do not retain arena borrows across mutation. Use `Cell` only for scalar lazy
fields where its lifecycle warrants it, and immutable independently owned lists
where they must survive a recursive operation. Avoid pervasive `RefCell` and
per-type `Arc` ownership.

Start type storage with a small common record and separate typed payload storage.
Do not repeat the S07 uniform-largest-payload node layout. This selects an access
contract, not an unmeasured byte size or page policy. P1 below measures the first
real type families, including payloads, lists, directory slack and allocation
traffic, before thousands of callers depend on their representation. Keep one
production representation. The required relater alternative is an isolated
experiment, not a second whole checker.

Use one validated checker scope per operation and private same-checker slots
internally. Raw imports, retained results and callbacks keep release-mode owner,
generation and bounds validation. Repeated same-checker reads should compile to
safe indexed access without a lock, owner lookup, `Arc` clone or `Result` at every
leaf. File AST access is a separate problem: published program files are
immutable, may span owners and may materialize lazy nodes. Reuse the compact
read machinery through an immutable retained-file resolver; do not expose the
exclusive binder writer to the checker or assume one file-local brand covers
an entire program.

Keep the S07 production compact storage and publication contracts. File symbols,
binding fields and flow graphs stay immutable. Checker-owned link stores and
merged symbols must not become an overlay that changes other checkers' reads.

## 3. Source audit and production boundaries

Create `scripts/s08_inventory.py` and `tools/s08/inventory/` using the existing
Go-typed inventory tools. The proposed command is
`python3 scripts/s08_inventory.py check`; `prepare --output <dir>` writes
reviewable candidates. Keep generated inventories under `data/s08/` and their
derivation instructions in `data/s08/README.md`. This is separate from
`cargo xtask gen`; do not widen AST generation with checker-harness work.

Start the operation closure at `checker.NewChecker`, `GetDiagnostics`,
`GetTypeAtLocation`, `GetTypeOfSymbol`, declared-type/symbol queries used by the
baseline walker, `TypeToStringEx`, the five relation modes, and the node-builder
and printer operations they invoke. Include actual `checker.Program`/`Host`
callbacks, evaluator, grammar checks, diagnostic formatting and source helpers.
Resolve named function values, interface calls, callbacks and initializers;
neither a text call search nor a `// port:` marker is a complete dependency audit.

Produce an obligation map joining each of the 675 existing IDs to its capability,
source anchor, loaded-library witness, implementation and testing status.
Supplemental fixtures should directly exercise capabilities that the standard
query sequence leaves lazy. They do not enlarge the E2 baseline denominator or
turn syntax counts into execution counts. Do not rewrite S07's immutable frozen
obligation artifact merely to mark implementation progress.

| Home | Responsibility and dependency boundary |
| --- | --- |
| `crates/ts_checker` | Checker state/host contract, owner operations, types/signatures, links, merges, resolution, relations, inference, narrowing, diagnostics and checker-dependent node building |
| `ts_checker::{owner,identity,storage,links}` | One ownership/mutation contract; checked external imports and private local access |
| `ts_checker::{symbols,resolve,types,signatures,instantiate,inference,relater,flow,check,grammar}` | Cohesive algorithm families; split large families further without opaque forwarding layers |
| `ts_checker::{node_builder,type_display,accessibility}` | Checker-dependent type-to-node traversal, caches, name accessibility and display flags |
| `crates/ts_printer` | Checker-independent AST printing, text writer, parenthesization, emit metadata and original-source text reuse |
| `crates/ts_nodebuilder` | Node-builder flags and the `SymbolTracker` contract, kept as its own crate exactly as upstream keeps `internal/nodebuilder`, so the Phase 3 declarations transformer never depends on `ts_checker` |
| `crates/ts_evaluator` | Constant evaluation (`internal/evaluator`, 168 lines): enum members, computed names and template-literal folding; the checker constructs it in initialization with its own `evaluateEntity` |
| `ts_binder::name_resolver` | `resolveName` is already ported as the hook-driven resolver; the checker implements `NameResolverHooks` and `ResolverHost` over its state instead of porting `resolveName` a second time |
| Collections (`ts_core` or a `ts_collections` crate, decided at P1) | The checker uses `collections.Set` at 24 sites plus `OrderedSet`, `OrderedMap`, `MultiMap` and the copy-on-write set and map; the ordered ones are output-order structures (deferred nodes, for one) and must keep insertion order |
| `ts_compiler` | Implements the checker host/program interface over the existing loader; convenience entry points retain complete bound-file dependencies |
| `ts_ast` / `ts_arena` | Only the concrete symbol/AST access and ownership primitives needed by both consumers; no dependency on `ts_checker` |
| `ts_diagnostics` plus `ts_compiler` diagnostic formatting module | Structured diagnostics and pinned diagnostic-writer behavior; baseline decoration stays in the harness |
| `scripts/s08_*.py`, `tools/s08/oracle/`, Rust examples/binaries | Strict requests, pinned Go access bridges, baseline comparison, ownership and measurements |

The dependency direction is compiler → checker → printer → AST, with
`ts_nodebuilder` beside `ts_printer` below the checker and the checker host trait
(`ts_checker::CheckerHost`) defined below the compiler. If shared flags/data need a lower home,
put only that common contract there. Upstream `internal/nodebuilder` contains
flags; the actual checker-dependent builder is in `internal/checker/nodebuilder*`.
Do not create a circular checker/node-builder crate dependency.

Inventory and test the additional `checker.Program` methods before supplying
defaults. S07's loader interface is not the full Go checker interface: module
specifier generation, import helpers, JSX runtime imports, module format and
source-file selection have observable behavior. Required callbacks get actual
implementations; excluded project/reference or mapper execution stays explicit,
not a fabricated `None`, empty map or successful no-op.

## 4. Ownership, identity and failure contracts

### 4.1 One checker owner, one operation

`CheckerOwner` retains the immutable program/file/bundle set, checker state,
checker AST storage and builder context. An exclusive permit protects its mutable
state. Keep `CheckerState: Send`; it need not be `Sync`. Interior scalar cells
remain inside the protected state. Use the existing generation primitive and
consolidate permit/state access so an ordinary query does not take two unrelated
locks. No lock is taken per node, type, symbol or relation edge.

The current `CheckerIdentity` reserves a future symbol-arena identity, while
`SymbolArena::new` independently allocates one. Add a private ownership-preserving
construction path so the concrete checker symbol arena uses that reservation
exactly once. Do not accept an arbitrary public arena number or silently give a
checker two inconsistent identities. Retain the full 32-bit arena namespace,
zero-slot rules and checked exhaustion.

The scaffold implements this. `CheckerIdentity::adopt_symbol_arena` hands the
reserved arena number to one `SymbolArena`; a second call fails with
`IdentityAdopted`, and `CheckerOwner::new` is its only caller. `CheckerOwner::
operation` takes the identity's permit and then the state lock, refuses same-thread
reentry through a thread-local set of active owners before waiting, lets ordinary
contention wait, and a panic inside an operation retires the generation before the
permit is released. Two lock acquisitions per operation is the known cost of
reusing the `ts_arena` permit unchanged; operations are per query, not per node,
and P1 measures whether folding the state into the permit is worth a `ts_arena`
change.

Checker-local type/signature slots are private. Owner-internal lists and cache
values may contain them because they cannot escape or move to another checker.
Imported file symbols use the retained file/bundle set; checker symbols use the
exact checker identity, not just a shared pool generation. Add compile-fail cases
for external construction, cross-scope mixing and escaping local handles.

An escaped type, signature, merged symbol or generated node retains the exact
owner and dependencies. Internal caches use non-owning IDs and cannot hold an
`Arc` back to their enclosing owner. The normal query path borrows/copies small
records; explicit result retention pays the owner reference increment.

### 4.2 Merging, links and lazy resolution

Port `cloneSymbol`, `recordMergedSymbol`, `getMergedSymbol`, `mergeSymbol` and
unidirectional merging in source order. Clone shared file symbols before a merge
would mutate them. Copy declaration backing and member/export tables with the
same aliasing rules as Go; a shallow Rust map clone must not accidentally share
mutable table state. Preserve nil versus allocated-empty tables and lists.

Merge fixtures compare declaration order, flags, value declarations, member/export
tables and `.symbols` observations with actual Go checkers. Include two checkers
with different additional declarations over shared files, reverse operation order,
concurrent independent checking and disposal of either checker. Snapshot the
shared file symbol graphs before/after so two equally wrong mutations cannot pass
merely because both checkers return the same answer.

Create only the link families required by the closure, with separate meanings
for absent, computing, resolved-empty and resolved-value states. Node/file-symbol
links are checker-local, keyed by validated arena metadata and slot; transient
symbol links index the checker arena directly. Allocate directories/pages on
first use, not eagerly for every program node. Measure capacities rather than
assuming a dense vector is cheap.

Port `pushTypeResolution`, `findResolutionCycleStartIndex`,
`typeResolutionHasProperty` and `popTypeResolution` together. The source guard
keys on entity **and property**, invalidates an affected stack suffix on cycles,
and stops its search when an intermediate resolution has produced a value.
A generic `HashSet<TypeId>` or a one-bit "busy" flag is not equivalent. The
guard is ported in `ts_checker::ResolutionStack`, with the produced-property
predicate supplied by the checker because it reads the links; the link stores are
`ts_checker::LinkStore`, keyed by arena and slot, paged per arena on first use,
with a page count for the census.

Distinguish algorithmic recursion, which continues under the current `&mut self`,
from external callback reentry, which releases/reacquires an operation. Prohibited
same-thread permit reentry must fail diagnostically before waiting; legitimate
contention waits. Do not use `try_lock` failure as a reentry detector.

On panic retire the shared generation before releasing the operation permit;
discard partial unpublished results and never resume the checker. A normal
TypeScript diagnostic or successful circularity fallback does not retire it.
An external callback that may release the permit ends the validated local scope;
resumption reacquires the same owner and revalidates identity and generation.
S09 still owns the generation-gate/registry/response-commit integration; test
the real S08 operation boundary without claiming those server guarantees exist.

Install native stack guards when each recursive algorithm is introduced. P5
completes the stress inventory; it is not the first point at which recursive
production queries become protected. Keep an in-thread checker entry below the
native dispatch layer so S10 can supply a different stack strategy.

### 4.3 Builder and printer lifetime

The checker AST factory and type-display builder are different storage roots.
Synthetic signature declarations and synthetic expressions must survive a
builder release. AST-to-type and type-to-AST links stay non-owning inside the
checker owner, avoiding an AST/checker dependency cycle.

`getNodeBuilder` reuses a builder and its caches. `ReleaseArenas` rotates active
allocation storage; it does not clear persistent caches or invalidate original
links. Retention must cover both side-table keys and values, cached nodes,
cross-generation originals and returned nodes. Outer retention sets own these
dependencies; arenas do not retain one another through cycles. Add direct tests
of generation rotation before relying on this behavior in type display.

Do not turn every print into a fresh uncached builder to evade the lifetime
contract. Conversely, do not retain every historical generation forever. The
complete S09 cache/registry fixture inventory remains separate, but production
S08 storage must already permit correct reclamation.

## 5. Semantic port and frozen observations

### 5.1 Baseline authority

**Owner decision (2026-09-11):** [S07-3 is amended](S08-acceptance-amendment.md).
Correct the duplicate-file input ordering. The 9,369 native-eligible variants
remain required for E2. Native option-policy skips, rejected-option variants and
filename skips form an explicit 1,359-variant informational partition. Real
outputs may be collected beyond selection guards, but their mismatches, failures
or unavailable output never affect E2. Rejected options remain explicit; do not
project them into ordinary required semantic cases. The original review record
is retained as history, superseded by this decision.

Build an oracle around the pinned compiler runner and
`internal/testutil/tsbaseline/{type_symbol_baseline,error_baseline}.go`. Use
access-only bridges in an exported source closure, leaving `upstream/` untouched.
Keep raw Corsa baseline output separate from its comparisons with older Strada
baselines: old-baseline fixups are not permission to normalize Rust/Go output.

Freeze every variant, ordered input/option identity, query request and expected
baseline eligibility. The source runner's `NoTypesAndSymbols` and
`baseline.NoContent` behavior must have explicit outcome records. All 10,728
variants remain represented with an explicit tier. Every acceptance variant must
have its required outcomes; informational execution may be unavailable. Within
acceptance, an expected absent baseline is not a skipped row,
and absence must not be confused with an empty file or an unimplemented query.
Report actual emitted-baseline and query counts alongside the fixed denominator.

The full type walker has a specific traversal and filtering rule, including
reparsed `as`/`satisfies` nodes, source trivia positions and declaration meanings.
Reproduce it, including the diagnostic/checking phase that precedes it. Freeze
direct pull-query traces separately; query order can change lazy identities and
must not be altered to match output. Compare structured type/symbol observations
for localization and final baseline bytes for acceptance.
Native numeric type IDs are capture-local diagnostics, not expected cross-run
values. The two amendment captures already differ in twelve ID fields while
their ordered actions and semantic outputs match. Freeze the actions; compare
identity relationships within each owner without rewriting raw observations.

For errors include configuration, options, program, syntax, binding, grammar and
semantic diagnostics in the source-prescribed selection/order. Preserve message
chains, related information, ranges, categories, codes, deduplication and exact
baseline writer behavior, including newline/path handling. Port missing
diagnostic-writer operations rather than making a Go-shaped string in Python.
Existing diagnostic comparators can be reused only after auditing the full
checker caller contract.

Unknown operation, panic, timeout, missing result, duplicate row, reordered query,
wrong input digest, non-finite counter and malformed protocol are explicit
failures. Compare named Go contract panics separately from unknown/assertion or
overflow panics. A Go panic does not become a successful `.types` baseline.
Retain complete per-case stderr and actual/expected output.

**Declaration diagnostics.** The Go harness's error set is not the checker's
alone. `harnessutil.go` collects config-file, program, syntactic, semantic and
global diagnostics and, whenever `GetEmitDeclarations()` is true (`declaration`
or `composite`), appends `GetDeclarationDiagnostics`. That call runs the
declaration transformer (`internal/transformers/declarations`) over every
non-declaration file through an emit host, the checker's emit resolver
(`emitresolver.go`, `symbolaccessibility.go`) and the node builder's symbol
tracker. The P0 policy capture identifies 1,459 eligible variants that require
this phase (1,450 with `declaration=true`, nine with `composite=true`). Across
the whole reference directory 65 of 7,301 `.errors.txt` baselines carry
declaration-emit codes (TS4xxx, and TS9xxx under `isolatedDeclarations`); some
declaration-emit diagnostics use TS2xxx and TS7xxx codes, so 65 is a floor, not
the count. The implementation choice on takeover is to **port the required declaration-
diagnostics path inside S08**. The user delegated this decision to Codex; this
preserves the existing error-parity requirement and authorizes no divergence.
P0 lists every variant for which `GetEmitDeclarations()` is true, distinguishes
phase requested/executed/failed and records per-file selection. Until Rust
executes the required path, those outcomes remain named failures even if an
existing reference baseline happens to contain no declaration error.

Add the required `internal/transformers/declarations` slice under
`crates/ts_transformers/src/declarations/`, below compiler and separate from the
checker. Its emit host/resolver contracts must remain cycle-free through the
existing AST, node-builder and printer layers; implement their checker-specific
callbacks in `ts_checker::{emit_resolver,accessibility}`. P0 traces
`Program.getDeclarationDiagnosticsForFile`, `declarations.GetDeclarationDiagnostics`
and the callbacks they execute, including isolated declarations. P4/P5 complete
this path before E2 semantic acceptance. Emitted JavaScript/declaration file
baselines remain Phase 3 work.

The harness constructs separate pre-emit and post-emit programs. Contrary to the
initial review's wording, it detects **unequal diagnostic counts**, not every
changed diagnostic payload when counts are equal (`harnessutil.go:690`). The Go
oracle must compare the complete sorted pre/post sets independently and retain
both. Any emit-dependent difference is a named scope dependency to resolve; an
equal count alone cannot establish that pre-emit collection suffices. Preserve
`CaptureSuggestions`, diagnostic directives and no-emit filtering as well.


### 5.2 Type-system worklist

Implement in dependency order, with Go observations for each completed vertical
slice; do not wait until the whole checker exists to run production queries.

1. **Construction and lookup:** intrinsic creation order, literal interning and
   freshness/widening, special/error types, symbols, global/module lookup,
   aliases, declarations, imports, exports and checker-local merges.
2. **Compound types:** object members/index signatures, arrays, tuple flags and
   rest/optional elements, unions and intersections, reduction, aliases and
   constituent ordering. Preserve identity-sensitive caches and nil states.
3. **Signatures and instantiation:** parameters/return types/predicates, overload
   selection, contextual typing, type parameters/defaults/constraints, mappers,
   inference and variance required by loaded generic declarations. Include
   mapped, conditional, indexed-access, infer and template-literal operations
   from the obligation inventory, rather than treating declarations as inert.
4. **Relations:** identity, assignability, subtype, strict subtype and
   comparability; source ternary outcomes, assumed recursive relations, cache
   keys, variance state, depth/complexity limits and diagnostic elaboration.
   A recursive encounter is not unconditionally "true" and the modes cannot
   share an undifferentiated result cache.
5. **Bodies and flow:** expression/statement checking, declaration checks,
   destructuring, assignment, overload calls, property access, return checking,
   control-flow narrowing over immutable binder graphs, predicates, loops,
   switches, discriminants and reachability. Include JSDoc, JS, JSX, enums,
   classes and namespace behavior wherever selected inputs or callbacks require
   it; feature titles alone do not establish an exclusion.
6. **Display and diagnostics:** type-node construction, symbol accessibility and
   qualification, printer precedence/parenthesization, literal spellings,
   formatting flags, truncation and error chains. Bring up a minimal real
   printer in the first vertical slice and extend it with each new type family.

Use S04/S05's `JsString`, scanner bytes and `ts_jsnum` for names/literals. Keep
signed node positions and deliberate Go numeric semantics. Do not coerce a
JavaScript string to Rust `str`, normalize numeric spellings by Rust formatting,
or copy source-backed strings unnecessarily.

### 5.3 Ordering and type display discriminators

Port the complete helpers behind `CompareTypes`, `compareSymbolsWorker`,
`compareNodes` and `CompareDiagnostics`, with markers at their actual
implementations. The type comparator rejects foreign checkers, compares named
and anonymous types, aliases, mapper structure and deferred references, and only
then falls back to IDs in the source's residual cases. Program file order is
the source order for node comparison, not alphabetical path order.

Freeze named fixtures for intrinsic creation order; duplicate-name symbols with
no declarations; reverse-mapped types lacking symbols/mappers; deferred and
instantiation-expression types; tuple flags; nil ordering; numeric literal edge
values; recursive aliases and mapper comparisons. Verify actual union lists by
reversing and ten seeded shuffles followed by the source comparator, as the Go
runner does. Record both pairwise comparator outcomes and resulting lists.
Creation traces are opt-in diagnostics for a named ID-sensitive mismatch, not a
whole-checker capture/replay project.

`TypeToStringEx` builds a type node and prints it; a direct recursive formatter
is not the planned implementation. Match its default flags, enclosing scope,
unresolved-type comment, `serializationLevel` bound (currently two), verbosity
restoration, multiline handling and byte-counted truncation. Diagnostic
serialization can recursively request type serialization. Test that path, not
only explicit calls to a recursion helper.

E4 must include parsed original-source literals and regenerated/synthetic literal
nodes, escaped/raw text, lone surrogates, malformed UTF-8, BOMs, quote choices,
numeric/bigint edge spellings and truncation through the production checker and
printer. Transport arbitrary output bytes with explicit encoding. Preserve the
existing S04 leaf, S05 scanner and S06 encoder evidence scopes.

## 6. Measurement design and experiment limits

Use the acceptance partition for the required checker query/workload and type
census manifests. Informational differentials do not enlarge those gates. The
S07 parse/bind workload and E7/E8 selection remain unchanged by this amendment.
Freeze the methodology and request manifests in P0, before Rust results can
influence case selection or endpoints. Implement one Go-first census and one
small production storage pilot. The definitive measurements use the completed
production slice; a storage microbenchmark cannot pass a sprint gate.

### 6.1 Per-type footprint

Add `data/s08/type-footprint.json` and a documented allocation/census adapter for
both runtimes. Run the subset's exact semantic query sequence, then inspect the
declared live checker/result roots. Record type headers, every concrete payload,
aliases, type-owned strings and list backing, indexing/directory capacity and
unused retained type-storage slots. Count shared backing once and handle cycles.
Do not compare `size_of::<Type>()` with Go's common header alone.

The Go side of that census is measured, not modeled
(`data/s08/checker-flag-observations.json`, `unsafe.Sizeof` at the pin on
darwin/arm64, go1.27.1). The `Type` header is 56 bytes and is embedded in every
payload struct, so a Go type costs its payload struct plus its alias record and
owned lists and maps:

| Go record | Bytes | Go record | Bytes |
| --- | ---: | --- | ---: |
| `Type` (header) | 56 | `TypeReference` | 216 |
| `IntrinsicType` | 72 | `InterfaceType` | 376 |
| `LiteralType` | 88 | `TupleType` | 424 |
| `UniqueESSymbolType` | 72 | `UnionType` | 272 |
| `TypeParameter` | 104 | `IntersectionType` | 240 |
| `ObjectType` | 184 | `MappedType` | 248 |
| `IndexType` | 80 | `ConditionalType` | 144 |
| `IndexedAccessType` | 88 | `Signature` | 128 |
| `TypeAlias` | 32 | `IndexInfo` | 64 |
| `ast.Symbol` | 96 | | |

Of the 56-byte header, 24 bytes are the checker back-pointer and the `data`
interface's self-reference, which no Rust layout carries. This suggests room for a smaller common record; it does not establish the
whole-storage ratio. Rust still needs payload tags/indices and owner metadata.
These are fixed record-size observations, not a live per-type census: lists,
member maps, caches and page slack remain unmeasured and must be charged.

The proposed gate statistic is the ratio of aggregate mean bytes per retained
type record: `(sum Rust type-storage bytes / sum Rust retained type records)`
divided by the equivalent Go mean over the same variants/checkpoints. P0 must
freeze the exact census membership and shared-allocation attribution, including
which intern/cache structures are charged to type storage. Shared storage may
not be excluded from both categories. Report actual type counts and total bytes
beside this statistic; explain differing counts from actual creation/cache
semantics and matched observations, not by inventing records to lower the mean.

Also report signatures, merged symbols, links, relation caches, builder storage
and whole-checker live bytes separately. Type-specific cache/indexing cost cannot
be hidden in an uncharged "other" bucket. An adapter unable to account for a
required family yields unavailable evidence and a named gap, not a zero charge.
The early model must include both retained and transient traffic. The final
threshold remains **0.80**, and layout changes remain subject to complete-slice
CPU/allocation measurements even though checker CPU has no numerical target.

### 6.2 Checker workload

Create `data/s08/checker-workload.json` containing a fixed, source-derived ordered
query schedule over the frozen subset. Its acceptance schedule includes checker
creation, program diagnostics, type/symbol queries and type display. Record
normal source semantics for options that suppress an operation. Supplemental
query fixtures add diagnostic coverage but are separately counted. Do not select
only cases that already pass or amortize initialization with arbitrary repeats.

Preload, parse, bind and validate inputs before the measured checker interval.
Retain those bound roots unchanged. The normal elapsed interval includes checker
initialization, lazy resolution, diagnostics and type display; it excludes input
loading, parsing/binding, JSON transport and baseline decoration. Store an output
digest and executed-query count so a timed child cannot skip work. Capture phase
elapsed times for initialization, checking/query work and display in a separate
diagnostic mode from the start; sampler self weights are not phase elapsed time.

Use fresh processes with an identical ordered workload, pinned options and exact
executable identity. Freeze seven measured samples per runtime with deterministic
alternation and separate excluded warmups; predeclare any instability reporting
rule, and do not repeat until a preferred number appears. Throughput is completed
fixed work per second, so **Rust/Go throughput >1 means faster**. Elapsed ratios
are its separately labeled inverse.

Allocated bytes are traffic during the checker interval, including temporary
lists, growth, diagnostics and builder work. Measure in a separate allocation
variant, not the normal timing executable. Retained bytes use the same live-root
checkpoint after queries, with a declared pre-checker baseline: Go after explicit
GC outside timing, Rust after temporary values are dropped. Record raw endpoints,
not only their subtraction, and describe the allocator's accounting units
(Go heap allocation versus Rust live requested allocations) and any class/slack
limitations. Report the structural census alongside the runtime heap endpoint;
neither owner counts nor peak RSS substitute for retained bytes. Unusable or
non-positive Go denominators remain unavailable.

`checkerbench` reports throughput, allocated-byte and retained-byte ratios. These
are measurement-presence gates, **not new speed or memory targets**. Report the
numbers even when unfavorable. No parse/bind sample or S07 ratio may supply a
checker metric. Use a designated quiet native host for timing, not hosted CI;
retain effective configuration, input digests, raw samples, compiler artifact
paths/hashes and the fixed protocol in a replayable archive.

### 6.3 Required alternative relater

Freeze `data/s08/relater-fixtures.json` from source contracts and Go-observed
cases before comparing implementations. Include all five relation modes,
recursive objects, unions/intersections, tuples, generic instantiation,
signature variance, cache reuse, diagnostic side effects and cycle/limit paths.
Freeze both cold and repeated-query state transitions, not just input graphs.

Build an isolated arena-reference/interior-mutability relater in
`tools/s08/relater-prototype/`. It must execute the relation algorithm and its
required lazy-resolution work; a precomputed relation matrix or an adapter back
to the production relater does not fulfill S08-10. Both implementations compare
against the same Go observations and each other before measuring.

Stable arena references and interior-mutability scopes need an explicit safe
construction proof. Prefer a safe existing arena dependency if necessary under
the dependency policy; no new `unsafe`, lifetime transmute, leaked storage or
unchecked self-reference. Sealing every graph before querying is permitted only
for fixture operations whose source protocol also separates construction;
fixtures that allocate/resolve during relations must still execute that work.
Prototype feasibility is addressed at P1, before the main port grows, so a late
unbuildable reference API cannot strand the required experiment.

Measure setup and relation work separately and include all retained roots and
allocation traffic in the declared scope. The primary throughput ratio is
**reference prototype / ID implementation**, unlike checkerbench's Rust/Go ratio.
Keep exact requests, cache conditions, ordering, diagnostics and output digests
identical. A production memory/CPU win is not inferred by adding these numbers
to other experiments. Run one complete comparison; promote the alternative only
through a separate recorded design decision. An unfavorable but valid measured
prototype still satisfies the measurement obligation.

### 6.4 Stop rules

Do not repeat the S07-bis sequence of extended diagnostics before an integrated
candidate exists. P1 resolves ownership feasibility and a gross footprint miss;
later profiling needs a named unexplained cost in a completed query path and a
specific removable mechanism. Use one bounded diagnostic to choose a candidate,
then measure the combined implementation. Preserve negative results.

Component screens provide attribution, not independent promotion or rejection
when their CPU/memory costs depend on one another. Report absolute milliseconds,
bytes, capacities and allocation calls before ratios. Do not forecast acceptance
by summing independently measured savings, demand a 5% gain for every useful
correctness-preserving change, or introduce an unapproved threshold adjustment.

## 7. Producer routing and downstream audit

Register producers only when they execute their stated work. Proposed entry
points are `scripts/s08.py` (prepare/capture/verify), `scripts/s08_producers.py`
(tracker output), and separate baseline, ownership, text, memory, benchmark and
relater modules. Reuse strict JSON, subprocess, compiler-artifact and pinned
oracle helpers instead of creating another subtly different framework.

| Producer | S08 change | Preservation obligation |
| --- | --- | --- |
| `e2` | Extend source-only verification with baseline parity, comparators, type display, recursion and divergence validation; one record contains all required metrics | Preserve the exact S07 source/partition validator; require every one of the 9,369 acceptance IDs; keep informational outcomes outside E2 aggregation |
| `e3` | Add `data/s08/ownership-cases.json` and real checker-merge scenarios to the existing instrumented orchestration | Aggregate existing S04/S06/S07 scopes; retain all earlier assertions; only publish a shared criterion when every implemented required contributor passes |
| `e4` | Compose existing leaf results with a new exact S08 literal/printer inventory | Keep leaf/scanner/encoder consumers and denominators intact; new metrics require production integration, not old helper observations |
| `e5` | Compose independently validated parse/bind and type-footprint captures | Keep both S07 memory ratios and provenance; add `type_footprint_ratio` from the new subset census, not a guessed value |
| `checkerbench` | Register an actual checker capture consumer with the frozen workload/methodology | Require positive finite throughput and positive Go byte denominators; retain raw zeros only when measured and valid |
| `relater` | Register an actual parity/capture consumer with frozen requests | Missing parity, a shortened fixture inventory, or a wrong reference denominator cannot pass |
| `e6` / `bindworkload` | Keep existing algorithms and ADR 0021 rules | Refresh if their source/artifact inputs become stale; do not broaden their performance claims to checking |

Include the divergence ledger, query/fixture manifests, comparison rules,
toolchain pins, actual compiler closure and validator code in relevant evidence
inputs. Scope source globs to the actual closure; avoid `crates/**` where unrelated
leaf edits would force a checker replay. Cache raw captures only with exact source,
configuration, request and executable identity checks. A standalone S08 component
run cannot replace `.latest` with a record that silently drops older fields.

Before changing routing, enumerate every consumer in `status/experiments.toml`,
`sprints/*.toml`, CI and tracker tests. Specifically test that S09-5 remains
incomplete with only S08 merge instrumentation, and that E5 cannot pass with only
parse/bind metrics or only the type census. Keep later E3 criteria unavailable
until their complete inventories execute.

Do not modify `data/divergences.toml` to resolve a mismatch without explicit
owner approval for that difference. The comparator must validate the pin, exact
scope, approval metadata and accepted difference; an allow-listed case must not
accept an unrelated new diagnostic or output corruption. Retain unapproved,
approved and stale/unused-entry counts separately.

## 8. Implementation checkpoints

These are work checkpoints, not prescribed commit ordering. Each checkpoint
keeps named unfinished operations and a reviewable record. New commits and normal
pushes preserve history. Full-corpus and expensive instrumented captures occur
when their production paths exist, not after every module edit.

| Checkpoint | Work and exit evidence |
| --- | --- |
| **P0 — Freeze execution contracts** ([complete](S08-P0-completion.md)) | Regenerate/verify S07 inputs without edits. Audit the typed source closure and checker host. Freeze baseline/query requests, missing-output semantics, supplemental fixtures, memory accounting, benchmark endpoints and relater fixtures. Freeze declaration-phase obligations for every eligible variant under the chosen full-parity path (§5.1). Run the actual Go oracle and one adversarial validator test for each output class. Rust gaps remain explicit. |
| **P1 — Ownership and storage feasibility** ([record](S08-P1.md)) | Concrete checker owner/permit, private local IDs, symbol-arena identity adoption, types/signatures/lists/links, basic synthetic AST retention. Measure real intrinsic/literal/object/union/tuple storage and compare with the source census, with total and transient budgets. Prove the reference-relater allocation/recursion API on a real recursive fixture. Exit with debug/release foreign-owner, retention, exhaustion, reentry and first-query tests; no full parity claim. |
| **P2 — First complete semantic slice** ([record](S08-P2.md)) | Checker host, initialization, globals, symbol lookup/merge, intrinsic/literal/object types and basic queries. Add minimal production node building/printing and structured/baseline errors. Run real checker-local merge fixtures in all four execution modes. Obtain end-to-end Go/Rust observations for named small programs, including a failing program, before expanding the port. |
| **P3 — Types, signatures, relations and ordering** ([increment record](S08-P3.md)) | Compound types, declarations, signatures, inference/instantiation, the relation operations they require, required loaded-library operations and source comparators. These mutually dependent algorithms grow together in complete query slices. Extend the printer alongside the types. Direct comparator, residual-identity and recursive relation fixtures pass; all remaining obligation families are mapped to concrete pending work. |
| **P4 — Body checking and full dependency closure** | Relations, expression/statement/grammar checks, flow narrowing and declaration/module diagnostics across all frozen options and loaded dependencies, including the required declaration-transform/emit-resolver path. Run all primary variants and retain every failure bucket; finish required missing operations without changing eligibility. Complete direct fixtures for lazy library capabilities. |
| **P5 — Display, bytes and recursion** | Finish builder/accessibility/emit-metadata/printer closure, exact type/error baseline decoration, E4 production integration and actual nested serialization/resolution tests. Stress left/right binary chains, nested parentheses/JSX, recursive object relations, conditional/instantiation limits and printing on small stacks that prove growth. Panic cleanup and subsequent owner retirement are observed; native tests do not certify wasm stack behavior. |
| **[P6 — Semantic acceptance](S08-P6.md)** | Exact E2 inventory, baseline bytes, ordering and type-display parity; measured recursion and divergence checks. Complete S08 ownership contributors and rerun applicable E3 instrumentation. Review algorithms, identity/lifetimes and ordinary-path costs separately. All nonmeasurement S08 requirements pass with current inputs. |
| **P7 — Required measurements** | Finish and parity-check the isolated relater. Run the full type census, checkerbench and relater comparisons with frozen methodology. If type footprint misses 0.80, select one integrated candidate from actual attribution and record its complete comparison; no declaration of success based on a model. Unfavorable checker/relater throughput is reported, not hidden. |
| **P8 — Final evidence and delivery** | Stage the final tracked inventory. Refresh all affected correctness producers and S07 native graph/capture evidence once as needed for changed Cargo/source fingerprints. Verify captures, compose E2/E3/E4/E5/checkerbench/relater, regenerate status, require `cargo xtask check S08` and `status --check-committed`. Archive raw requests/results, policies, logs and replay code; push and run four-target CI. |

Size of the work, so no checkpoint is mistaken for a sprint: the checker package
is 60,678 non-test Go lines. The exclusions take out `jsx.go` (1,488) and parts
of the node builder and grammar checks; they leave the bulk of `checker.go`
(32,523), `relater.go` (5,044), `flow.go` (2,761), `inference.go` (1,684) and
`grammarchecks.go` (2,230). S08 is the entry to the Phase 2 critical path and is
not time-boxed the way S07 was. Each checkpoint reports the pass count over the
9,369-variant acceptance denominator and the named families still failing;
informational results are reported separately; a partial
increment ends with explicit remaining work. The scaffold committed with this
revision is P1's starting point, not a P1 result.

P1 must not become a storage research program: one measured production candidate
and the minimum alternative-relater feasibility proof. P3/P4 may take several
implementation increments because the dependency closure is large. A partial
increment ends with explicit remaining work, not a finished checker claim.

## 9. Verification and CI

Run focused Rust debug/release tests while each family is introduced. Counterexample
fixtures include same numeric slot in two checkers of one generation, foreign
cached type lists, source symbols mutated by a second merge, a result surviving
lease release, checker-created nodes surviving builder rotation, and a generated
node whose original belongs to a released older builder generation. Verify final
drop counts only after all declared result/cache roots are released.

Recursion tests run actual checker/relater/printer entry points with the existing
reserved-stack/growth strategy. Preserve Go's semantic depth limits separately
from host stack protection. Source `pushTypeResolution` cycles, circular aliases,
return inference and diagnostic-triggered serialization get different named
fixtures. Add a small-stack run that demonstrates growth and an injected unwind
after growth. Keep the native stack dependency/platform compile checks on all
four native targets; wasm execution remains S10.

Challenge producers with missing/extra/duplicate/reordered cases and queries,
wrong flags or same-size source changes, unexpected absent baselines, wrong panic
classes, omitted diagnostics/related info, forged success counts, zero benchmark
denominators, NaN/infinity and stale binaries. Assert that unrelated criteria and
later sprint items cannot turn green after a partial producer result.

Final local checks include workspace tests, fmt, Clippy with existing warnings
policy, deny, tracker self-tests/validation, regeneration drift and original
client parity, applicable scanner/parser/binder/program regression evidence,
S08 exact corpus and ownership runs, and the declared MSRV. Do not add a generic
all-function percentage gate that S08 never requested; keep handwritten port
traceability and generated provenance distinct.

Extend `.github/workflows/status.yml` for the four native targets:
`macos-15`, `macos-15-intel`, `ubuntu-24.04`, `ubuntu-24.04-arm`. Each builds the
new production code and runs the required correctness comparisons. Preserve
upstream checkout/assets, caller Cargo/Rustup homes, reusable Go/Miri/toolchain
caches and `GOTOOLCHAIN=local`. Plan deterministic shards if the complete E2
inventory needs them; the merger must reject missing/overlapping shards and may
not report a partial denominator as complete.

Independent captures and artifact upload remain reachable after another producer
fails when their own prerequisites hold (`!cancelled()` plus explicit prerequisite
outcomes). A nightly install failure must not suppress ordinary checker parity
or its diagnostics. Hosted runners do not supply native performance acceptance.

The last S07 capture may become stale when new crates change the Cargo closure.
Schedule its required refresh after final source changes, not repeatedly during
porting. Retain ADR 0021's 0.85 memory and 1.25/1.45 CPU limits and host scope.
Replaying an archive confirms its recorded inputs; it does not make old evidence
current for a changed compiler. Adding crates changes `Cargo.lock`, which is in
every S07 native fingerprint and in the `sources` of every host producer; from the
scaffold commit onward S07's E5/E6 evidence shows stale until the P8 refresh.
That is expected and recorded, not a regression to chase before then.

## 10. Completion record

Publish `docs/S08.md` with the final mapped scope, all primary/supplemental counts,
baseline mismatches and approved divergences, ownership/recursion coverage,
per-type footprint and its accounting definition, checkerbench results, the
relater comparison and limitations. Include source/toolchain/executable and
manifest identities plus instructions to replay the retained evidence.

Completion means every required S08 item and exit expression passes from current
evidence. It does not mean full TypeScript checking/emit parity, E3 completion,
WebAssembly success, or a demonstrated full-compiler speedup. If the footprint
gate or semantic closure remains unfinished, the record states the measured gap
and next concrete work; thresholds and frozen inputs remain unchanged.
