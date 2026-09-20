# Corsa in Rust

Engineering plan, draft 3.2, 5 September 2026. Measured against microsoft/TypeScript at commit `1f70213d49`. An independent repository with upstream pinned as a dependency, four native targets, required spike experiments for memory, WebAssembly and embedding, divergences approved by the repository owner, and sequencing by dependency and parity gates rather than by calendar or headcount. The spike validates bounded prototypes; full compiler WebAssembly and Rust embedding are required before cut-over.

A plan to replace the Go implementation of the TypeScript 7 compiler and language server (the `tsc/` module of microsoft/TypeScript, codename Corsa) with a Rust implementation that ships as a drop-in `tsc` binary for macOS and Linux: the same 48,075 baseline files, the same LSP and JS-API wire protocols, usable by the upstream JS client and VS Code extension without changes.

| | |
|---|---|
| Source to replace | 317,820 lines of Go across 82 packages, of which 47,442 are generated; 5,082 Go files; no cgo; twelve files of `unsafe` confined to file watching and paths |
| Tests to satisfy | 12,721 compiler and conformance cases producing 48,075 baselines; 4,356 fourslash tests (761,930 lines of Go); tsc, build, watch, project, LSP and JS-API suites; 342 MB of test data |
| Wire contracts to keep | LSP 3.17 with negotiated UTF-16 or UTF-8 positions; the `--api` protocol (msgpack framing, binary AST encoder version 8 with WTF-8 strings and UTF-16 positions, 144 methods); the content-mapper plugin RPC |
| Upstream | microsoft/TypeScript, pinned at a commit and consumed for test data, schemas, lib files, the JS client, the extension and the Go oracle binary; 113 to 237 commits per month to absorb by moving the pin |
| Native targets | macOS arm64 and x64, Linux x64 and arm64 with glibc. WebAssembly is the additional library entry point described below. |
| Pace and gates | Prototype dependencies are explicit; full parity gates close each phase. The spike must pass memory, WebAssembly and embedding experiments; full deliverable acceptance blocks cut-over. No calendar or staffing model. |

## Contents

1. [The short version](#1-the-short-version)
2. [Why do it](#2-why-do-it)
3. [What is in the upstream repository](#3-what-is-in-the-upstream-repository)
4. [What makes this hard](#4-what-makes-this-hard)
5. [Goals and exit criteria](#5-goals-and-exit-criteria)
6. [Architecture decisions](#6-architecture-decisions)
7. [Compatibility contracts](#7-compatibility-contracts)
8. [Strategy and crate map](#8-strategy-and-crate-map)
9. [Sequence and gates](#9-sequence-and-gates)
10. [Risk register](#10-risk-register)
11. [Consumed from upstream, not rewritten](#11-consumed-from-upstream-not-rewritten)
12. [Alternatives considered](#12-alternatives-considered)
13. [Open questions and pending technical decisions](#13-open-questions-and-pending-technical-decisions)
14. [First steps](#14-first-steps)

## 1. The short version

1. **This is a rewrite in the hard third and a port in the easy two thirds.** The Go code is a near line-for-line translation of the original TypeScript source kept alive by a garbage collector: nodes, symbols, types and the checker form cyclic, lazily mutated pointer graphs, and the checker keys dozens of caches by pointer identity. Rust cannot express that shape. The checker's data layer and the project system's snapshots must be re-architected onto arenas and integer ids while their control flow is preserved. Scanner, parser, binder, printer, transformers, module resolution, the command line and the formatter port mostly mechanically once the AST design exists.
2. **The TypeScript team evaluated Rust for this codebase in 2024 and chose Go for exactly that reason.** Their reasoning still holds, and the spike exists to settle with evidence what it costs to do it anyway.
3. **The test corpus makes it tractable.** The inputs and baselines are language-agnostic text. The Go binary built from the pin is the oracle: binary AST encoding supplies the first parser comparisons, a test-host protocol validates transport and filesystem behavior before semantic LSP tests, and the unchanged JS-API suite supplies full API parity once that server exists.
4. **The critical path is the checker** (60,703 lines, one 32,523-line file, 2,885 functions, a struct with about 320 fields), followed by the language service (41,500 lines) and the project system (12,300 lines). Emit, the command line and the API server run as parallel workstreams.
5. **Order-sensitive output is enumerated, not assumed.** Corsa orders union constituents, members and diagnostics by sorting at the point of observation with structural comparators: type flags, names, declaring file and source position, tuple shape, type arguments. It falls back to ids only for intrinsic types and rare residual ties. The Rust checker must port those comparators exactly and build the intrinsic types in Corsa's order; it does not have to reproduce Go's allocation sequence.
6. **Independent repository, pinned upstream.** This repository is the Rust workspace. microsoft/TypeScript is pinned at a commit and consumed for test data, schemas, lib files, locales, the JS client, the extension and the Go oracle. Upstream changes are absorbed by moving the pin and porting the diff against a file ledger. Contributing upstream is a later option, not a precondition of anything here.
7. **Four targets.** macOS arm64 and x64, Linux x64 and arm64 with glibc. That removes the Windows host layer (named pipes, directory-change watching, Windows realpath), musl, and every best-effort target from scope. Windows-style path semantics in the path utilities stay, because baselines exercise them.
8. **Three contracts come before the AST is generated.** Node ownership includes file-owned lazy storage, content-mapper bundles, checker AST, transform and builder arenas, plus request scratch storage, and checked owner/generation identities. Text handling separates source bytes after BOM decoding, JavaScript string bytes, and wire positions, preserving malformed input behavior. Binding symbols belong to files; merged and transient symbols and resolved types belong to checkers. The spike exercises sharing, lazy allocation, disposal, stale handles and malformed input.

## 2. Why do it

Corsa already captured the large win of leaving JavaScript; Rust competes with Go, not with the old compiler. The three benefits below are all primary. The spike measures each on a bounded workload and must pass all three probes before the rest of the plan starts. Parser results do not establish whole-checker performance or whole-compiler WebAssembly support; those require the acceptance tests at cut-over.

- **In-process embedding.** The Rust tools that now dominate the JavaScript toolchain (oxc, rolldown, rspack, turbopack, biome, deno) can link a Rust checker directly instead of driving the `--api` server over a socket. Today anything that wants in-process access must itself be written in Go, which is why the typescript-eslint native linter is a Go program. Experiment E8.
- **WebAssembly.** A `wasm32` build of the compiler for the playground, browsers and hosts that cannot spawn native processes. Go's WebAssembly output is large and slow, so Corsa has no such story and the JS API requires a native binary. Experiment E7.
- **Memory and tail latency.** Arena-owned type universes free wholesale, there is no collector headroom (Go's collector by default lets the heap grow to twice the live set between collections), and no collection pauses inside editor requests. Expect 30 to 50 percent lower peak memory; CPU gains of 1.2 to 2 times are plausible from data layout but not guaranteed. Experiments E5 and E6.
- **Concurrency and identity rules.** Rust's `Send` and `Sync` constrain sharing and mutation across threads. Checker identity and stale-handle rejection are separate contracts: checked owner/generation identities enforce them at arena and registry boundaries. An in-range integer index from the wrong checker is an error even though it causes no memory violation.
- **Portability without cgo tricks.** Corsa reaches FSEvents on macOS through a hand-rolled, cgo-free foreign function layer. Rust reaches system APIs natively.

## 3. What is in the upstream repository

Non-test Go lines per area, measured on the pinned commit. Generated code is counted separately because it is regenerated from a schema rather than rewritten.

| Area | Go packages | Lines | Notes |
|---|---|---:|---|
| Type checker | `checker` | 60,703 | `checker.go` alone is 32,523 lines; `relater.go` 5,044; `flow.go` 2,761; node builder 3,705. 2,885 functions. |
| Language service | `ls`, `ls/autoimport`, `ls/lsutil`, `ls/change`, `ls/lsconv` | 41,536 | Completions 6,900 lines, string completions 2,247, find-all-references 2,774, auto-import registry 5,400. |
| Emit | `printer`, `transformers/*`, `sourcemap` | 37,163 | ES transforms 11,251 (class fields 3,618, decorators 2,751); declaration transform 4,212; TS, module and JSX transforms 7,813. |
| AST | `ast` | 21,219 | 10,058 lines generated from `tools/scripts/tsc/ast.json`; utilities 4,631; pointer-based nodes with parent links and per-kind arenas. |
| Program, options, resolution | `compiler`, `tsoptions`, `module`, `modulespecifiers`, `packagejson`, `outputpaths` | 19,236 | Program construction, checker pool (FENNEL partitioning), about 160 compiler options, node and bundler module resolution. |
| LSP protocol types | `lsp/lsproto` | 18,325 | 17,566 lines generated from the LSP 3.17 metamodel by `_generate/generate.mts`. |
| Syntax | `parser`, `scanner`, `binder` | 17,002 | Includes the JSDoc reparser that rewrites JSDoc into synthetic TypeScript nodes; positions are UTF-8 byte offsets. |
| Command line, build, watch | `execute/*`, `fswatch`, `diagnosticwriter` | 14,656 | `tsc`, `-b` orchestrator, incremental `.tsbuildinfo`, native watchers for FSEvents, inotify, fanotify, kqueue and Windows. Only FSEvents, inotify and fanotify are in scope. |
| Project system | `project`, `project/ata`, `project/dirty`, `project/logging` | 12,309 | Immutable, reference-counted snapshots built copy-on-write; a reference-counted parse cache that binds files on entry; automatic type acquisition spawns npm. |
| JS API server | `api`, `api/encoder`, `ipc`, `jsonrpc` | 12,168 | 144 methods, msgpack framing, binary source-file encoder (protocol 8), unix sockets and named pipes, callback file system. Only unix sockets are in scope. |
| Diagnostics | `diagnostics` | 9,658 | 8,854 generated from 2,135 messages plus 85 extra; 13 embedded gzipped locales. |
| Formatter | `format` | 4,259 | Rule-based formatter with its own baselines. |
| LSP server | `lsp`, `lsp/lspwatcher` | 3,734 | Request queue with cancellation, progress, watcher registration, API-over-LSP connection, stack sanitizer. |
| Content mappers | `contentmapper`, `spanmap` | 2,795 | Child-process plugins (for example `.vue`) that produce virtual TypeScript over JSON-RPC. |
| Utilities and hosts | `core`, `collections`, `stringutil`, `tspath`, `jsnum`, `vfs/*`, `glob`, `semver`, `json`, `locale`, `bundled`, `astnav`, `evaluator`, `pseudochecker`, `transpile`, `tracing`, `pprof`, others | about 26,000 | Unicode 15.1 identifier tables (3,496 generated lines), WTF-8 string helpers, JavaScript number semantics, virtual file systems, 108 embedded `lib.*.d.ts` files. |
| Test harnesses (Go, not counted above) | `fourslash`, `testrunner`, `testutil/*`, `execute/tsctests` | about 17,000 | Compiler runner with directive parser and baseline diffing, fourslash harness with an in-memory file system and an in-process server, fake system with clock for tsc tests, project test utilities. |
| **Total non-test Go** | **82 packages** | **317,820** | 47,442 generated; about 270,000 hand-written. |

Alongside the Go module: `packages/typescript`, the npm package and JS API client (53,200 lines of TypeScript, 12,863 generated, including a WTF-8 decoder), `packages/vscode-typescript` (4,184 lines), the hereby build file, generators, and the release pipelines. None of that is rewritten; the client and the extension are consumed from the pin and pointed at the Rust binary.

## 4. What makes this hard

### The Go code is a port, and Rust cannot be ported to

Every load-bearing structure in Corsa is a pointer graph with cycles and late mutation. `ast.Node` carries a `Parent` pointer and an interface-typed payload; the binder writes symbols and flow nodes into nodes after parsing; `ast.Symbol` points at its declarations, members, parent and export symbol; `checker.Type` points back at its checker and symbol. The checker itself is a 320-field struct whose caches are maps keyed by `*ast.Node`, `*ast.Symbol` and `*Type`, plus 26 `core.LinkStore` side tables keyed by pointer, plus function-valued fields that swap behavior at runtime. Go's collector frees the whole tangle when the checker is dropped.

None of that survives the borrow checker unchanged. The data layer becomes arenas and ids (which Corsa already half-uses through `core.Arena` and `LinkStore`), and every function that today mutates through a pointer while holding other pointers must be rewritten to work with ids. The control flow can and must stay the same; the plumbing cannot.

### Order is made deterministic by sorting, and the comparators are the contract

Symbol tables in Corsa are plain Go maps, which iterate in random order, so the checker sorts wherever order is observable. `CompareTypes` in `checker/utilities.go` orders union constituents by type flags, then names and alias arguments, then per-kind structure: declaring file and source position through `compareSymbols`, tuple element shape, type-argument lists, deferred-reference location and type mappers. Members are sorted with `compareSymbols` (first declaration position, then name), diagnostics with `CompareDiagnostics`. The compiler runner's `union ordering` sub-test shuffles unions and re-sorts them to check consistency on the exercised types; it is not a proof of total ordering for every possible type graph. `compareTypeIds` still exists in `checker.go` but has no callers.

Type ids remain the last resort in three places: intrinsic types, which are created in a fixed order when a checker is constructed; reverse mapped types with neither symbol nor mapper, which the source itself calls unstable and rare; and symbols with no declaration and duplicate names. Draft 1 of this plan read the ordering as id-based and demanded that the Rust checker reproduce Go's allocation sequence. That was wrong. The real requirement is narrower and testable: port the comparators line by line, construct intrinsics in Corsa's order, and treat the three residual cases as known instabilities with their own tests.

### Source bytes and JavaScript strings need different contracts

JavaScript strings can contain lone surrogates. Corsa keeps them in Go strings as three-byte WTF-8 sequences (`EncodeJSStringRune` in `stringutil/util.go`), the relater and the JSX transform special-case them, the API encoder's string section is documented as UTF-8 with WTF-8 for such values, and the JS client ships its own WTF-8 decoder. A Rust `String` cannot hold `"\uD800"`. Positions come in three encodings as well: UTF-8 byte offsets inside the compiler, UTF-16 code units in the API encoder (it converts every node position through a per-file position map), and UTF-16 or UTF-8 for LSP depending on what the client negotiates.

Source input is not guaranteed to be valid UTF-8 either. `vfs/internal/internal.go:decodeBytes` performs BOM handling, including UTF-16 decoding, but otherwise preserves bytes. The scanner diagnoses malformed bytes outside strings, while `scanString` can return an unvalidated raw substring. Strict UTF-8 decoding or lossy replacement at the file boundary changes diagnostics, positions or literal bytes. The Rust port preserves this distinction and matches the oracle's success or error at each later boundary.

### Files are bound before any program exists

The project system's parse cache binds a file as it is created (`project/parsecache.go`) and hands the same bound file, symbols included, to every program and snapshot that references it while its reference count is positive. Incremental builds, auto-import and source-definition code also bind files outside any program. Binding symbols are therefore owned by files. Merged symbols are different: `Checker.mergedSymbols`, `cloneSymbol` and `mergeSymbol` allocate and mutate checker-owned transient symbols. Their overlays, like resolved types, must be disposable without touching the shared file or another checker.

### Nodes have several owners

Source-file arenas are only the first kind. Transforms allocate synthetic nodes through their own factory, and the emit context records each synthetic node's original in a side map (`printer/emitcontext.go`). The checker's node builder, which produces the trees behind `typeToString` and hover, allocates in a factory whose `ReleaseArenas` call drops everything not still referenced by a cache (`checker/nodebuilder.go`). The API session, the options parser and the formatter create scratch factories of their own. Go's collector makes those retention rules implicit; Rust has to state them.

Binding does not finish file-owned allocation. `SourceFile.resolveJSDoc` and `GetOrCreateToken` allocate and cache nodes on demand under locks; those nodes survive individual requests and checkers. Content mappers also link a canonical source file to supplemental files and each supplemental back to the canonical file (`contentmapper/transform.go`). Upstream's parse cache owns that set as one bundle. An immutable file arena and a rule allowing references only toward older arenas cannot represent these paths.

### Deep recursion on fixed stacks

Go stacks grow on demand up to 1 GB, and Corsa never had to think about recursion depth beyond TypeScript's own instantiation limits. It even removed the trampolines the TypeScript-in-TypeScript compiler used: the binder, `checkBinaryLikeExpression` and the printer's binary-expression emit all recurse on the left operand, so a long concatenation chain recurses once per term in each of them. Rust threads have fixed stacks and a stack overflow aborts the process. Long binary-expression chains, deeply nested JSX and recursive conditional types will find this on day one unless the design reserves large stacks and guards the deepest paths.

### The target moves

The Go tree received between 113 and 237 commits a month this year from about fifteen regular contributors, with roughly a quarter of commits authored by coding agents. This repository absorbs that by moving its upstream pin and porting the diff between two pins against a file ledger, which is why the Rust code keeps Go's file, function and variable names wherever possible: the diff between two pins must be transplantable.

### Calibration

The Go port's first commit landed on 30 September 2024. The upstream repository switched to the Go-only layout on 19 August 2026, after 2,546 commits to `tsc/`. That was a one-to-one translation by the people who wrote the original, with the original test suite, and with heavy agent assistance. A Rust rewrite has the same suite and can use the same assistance, but it pays for the re-architecture and for tracking. This plan does not restate that history as a schedule; it orders the work and gates it.

## 5. Goals and exit criteria

### Goals

- A `tsc` binary for the four targets that the upstream `packages/typescript` client and the VS Code extension, built from the pin, can be pointed at without changes.
- Identical baselines across all suites, with intentional differences recorded in an allow-list approved by the repository owner, the way `testdata/submoduleAccepted.txt` records Corsa's accepted divergences from TypeScript 6.
- Wire compatibility for LSP, the API protocol and the content-mapper protocol, verified by running the existing clients and test suites unchanged.
- Performance at or above Go on the TypeScript-benchmarking scenarios (vscode, self-compiler, mui-docs, xstate, bluesky) at 2, 4 and 8 checkers, with at most 70 percent of Go's peak memory.
- A `wasm32` compiler library and an embeddable Rust crate API, with parser/checker prototypes tested in the spike and full type-checking and emit acceptance required in Phase 7 before cut-over.
- Upstream parity maintained by moving the pin, with a ledger that maps every Go file to its Rust counterpart and the pin it was last synchronized to.

### Non-goals

- Changing TypeScript semantics, diagnostics text, emit or the differences documented in `tsc/CHANGES.md`. The Rust compiler targets Corsa, not TypeScript 6.
- Rewriting the JS API client, the VS Code extension or the test data; they are consumed from the pin.
- Adopting a third-party AST or parser. The AST shape is pinned by `ast.json`, the binary encoder and the generated TypeScript AST package.
- Windows, musl, and every best-effort target. The Windows host layer is not built.
- Publishing under upstream package names, code signing, and upstream's release pipelines. Whether and when any of this goes upstream is a later question.
- New features before parity. Rust follows Go until the checker gate.

### Exit criteria for cut-over

- 100 percent of compiler, conformance, transpile, config, tsc, build and watch baselines, modulo the allow-list.
- At least 99.5 percent of fourslash tests, with the remainder triaged and listed.
- The `packages/typescript` sync and async suites, the project and LSP suites and the replay corpus pass against the Rust binary.
- Benchmark and memory targets met on every scenario for four consecutive weekly runs.
- Four weeks of the owner's own projects and the VS Code extension running on the Rust binary with no open P1 crash.
- The WebAssembly library runs the compiler/conformance and transpile corpus through an in-memory host, matching the pinned oracle's applicable diagnostics, type/symbol and emit baselines, including `.js`, `.map` and `.d.ts`, modulo the same approved allow-list. It must not require native processes or native filesystem access. Node runs the corpus; a browser smoke test covers parse, type checking, emit and deep input.
- A separate Rust consumer links the embedding crate, supplies its host, and runs the same compiler/conformance and transpile comparisons in process. E3 ownership, malformed-input, cancellation, panic-invalidation and repeated-create/drop tests pass through the public API. Required crate operations and host callbacks are documented and exercised by the consumer, not just a Node parse binding.
- WebAssembly and embedding size, latency and retained-memory budgets are fixed for named workloads before Phase 7 and met for four consecutive weekly runs. E7/E8 parser thresholds remain parser measurements; they are not substituted for full-compiler acceptance. Both deliverables block cut-over.

## 6. Architecture decisions

Each decision names what it replaces. Corsa's observable behavior is preserved while ownership and identity become explicit. This section is the active contract; section 13 records unresolved implementation choices and references settled decisions rather than supplying conflicting alternatives.

### 1. AST: index-based, generated from `ast.json`, with explicit arena ownership

- **Decision.** A file owner retains the immutable parsed/bound core and synchronized append-only storage for lazy JSDoc and language-service tokens. Lazy nodes are published only after initialization, keep stable ids, and live until the file owner is released. A content-mapper bundle owns its canonical and supplemental files together; cyclic links inside the bundle are non-owning ids. Checker, transform and builder owners retain their synthetic arenas; options, formatting and API print/format factories use operation-scoped scratch storage. Generated accessors hide node layout, which is measured under section 13, item 6. Children are node ids and node-list ranges; positions are byte offsets into `SourceText`.
- **Identity.** A node id identifies an arena generation and slot. Resolution requires a live owner and validates the arena generation and bounds; recycled slots cannot make an old id valid again. The intended compact representation is 64 bits, with its packing, overflow behavior and registry protocol fixed in the ownership design note before generation. Ids do not retain storage: snapshots, bundles, caches and returned owning handles retain the necessary owners explicitly.
- **Check elision.** Checked resolution is the release-build default. Repeated owner/generation checks may be elided only within a validated ownership scope whose non-forgeable handles prove provenance and whose lease prevents storage replacement or slot reuse. Internal caches, cross-arena links, retained ID lists and callback/reentrant paths must preserve that proof or validate before use. Fresh invariant owner brands or a private validated local-handle API may establish the scope; ordinary ID newtypes, shared lifetimes and `Send`/`Sync` do not. Bounds safety remains separate, and elided checks retain debug assertions. The design note states the proof for each elided path rather than requiring all internal accesses to skip checks.
- **Rules.** Logical links and owning references are separate. Parent, original-node and bundle links may be cyclic; reference-counted ownership between owners must be acyclic, with any mutually dependent file set grouped under one bundle owner. File-owned lazy storage belongs to the same owner as its core, so an older file can retain newly allocated nodes without a new owning cycle. Checker, transform and builder owners retain the file/bundle owners they reference. Builder caches, persistent emit side tables and returned handles retain every arena dependency of their nodes and metadata; release rotates allocation, and an arena is reclaimed only after its final retention root drops. The design note records each owner edge and its disposal rule; arena age is not a substitute for that graph. API encoding walks the tree and converts positions through the file's position map.
- **Verified by.** E3 includes concurrent first-use JSDoc/token requests through shared snapshots, a mapper producing multiple supplemental files, release of owners in different orders, and release-mode wrong-owner/stale/reused/retired-id rejection through caches, cross-arena links and callback reentry. Debug and sanitizer runs supplement these required semantic assertions. Evidence: `ast/ast.go:resolveJSDoc`, `GetOrCreateToken`, `contentmapper/transform.go`, and `project/parsecache.go:ContentMappedParseCache`.
- **Instead of.** `Rc<RefCell<Node>>` graphs (slow, unergonomic); bump-arena references in the oxc style, `&'a Node<'a>`, which are excellent for parse and transform but push a lifetime into the checker, the language service and long-lived snapshots; adopting oxc's AST outright, whose shape does not match `ast.json`; a single arena per file, which was draft 1's design and has no place for synthetic nodes.

### 2. Symbols, types and signatures: file-owned binding, checker-owned types

- **Decision.** The binder allocates binding symbols in the file owner's arena before any program exists. Merged symbols, their `mergedSymbols` mapping, and all other transient symbols remain checker-local, matching `Checker.cloneSymbol` and `mergeSymbol`; programs share bound files, not mutable merged-symbol overlays. A symbol id distinguishes a file owner from a checker owner and validates its generation and slot. Types and signatures also carry checked checker provenance: a resolver rejects a handle from another checker even when its slot is in range. Owner identity is retained in compact ids or an enclosing owning handle, never inferred from the slot alone. The precise packing is settled with the node-id contract. Pointer-keyed maps and `LinkStore` instances become side tables keyed by these identities. Symbol tables remain unordered hash maps keyed by `JsString`, with ordering at the same observation points as Go; node/type payload layout is selected by measurement.
- **Check scope.** Decision 1's conditional check-elision contract applies to symbols, types and signatures. A cache result or independently retained `Arc<[TypeId]>` list does not establish arena ownership merely because its allocation is live. It must preserve a validated checker/generation scope or validate on import; the list retains its own bytes, not automatically the checker. Required wrong-checker and retired-generation rejection runs in release mode as well as debug/sanitizer configurations.
- **Because.** This is the memory model Corsa already uses through `core.Arena` and `LinkStore`, minus the pointers. Releasing the final checker-owner reference frees its type universe; retained results can keep that owner alive after its pool slot is released. Dropping a program does not dispose a file still retained elsewhere.
- **Verified by.** E3 checks two programs sharing one bound file; disposal of one while the other keeps checking; an edit while an old snapshot answers requests; and two checkers independently performing declaration merging over shared files. Wrong-checker, stale and recycled-id lookups must fail explicitly. Miri and AddressSanitizer check memory misuse in addition to these semantic identity assertions.
- **Instead of.** `Arc<Type>` graphs (cycles leak, an atomic on every access); a program-wide symbol arena, which was draft 1's design and cannot hold a symbol that a cached file carries into a second program.

### 3. Mutation inside the checker

- **Decision.** The checker is a single-threaded state machine that takes `&mut self` and never holds a borrow into its arenas across a call: accessors copy small values out, scalar lazily-resolved fields use `Cell`, collections are addressed by id. `isRelatedTo`, instantiation, inference and control-flow analysis keep Corsa's structure, and keep its order of side effects wherever decision 5 lists the output as order-sensitive.
- **Because.** The port ledger depends on Go and Rust having the same shape so that upstream diffs transplant, and the sort points in decision 5 depend on the same values being present when the sort happens.
- **Instead of.** `RefCell` on every table (runtime borrow panics inside 32,000 lines of recursion); splitting the checker into pure passes, which does not match TypeScript's demand-driven semantics.

### 4. Concurrency: keep Corsa's model exactly

- **Decision.** Parse and bind in parallel with scoped threads. Partition files across checkers with the same FENNEL heuristic and the same tuned constants from `compiler/checkerpool.go`; one checker per thread, `Send` but not `Sync`. Emit per file in parallel. The language server serves requests on `Arc<Snapshot>` values built copy-on-write, with `project/dirty`'s boxes becoming persistent maps. Cancellation is a token polled wherever Corsa polls its context.
- **Because.** These behaviors have baselines and a race-mode CI job, and the partitioning constants were swept on real projects. There is nothing to gain from a different model before parity.
- **Instead of.** A query-based incremental architecture in the rust-analyzer style. Attractive later; incompatible with landing a drop-in replacement.

### 5. Order-sensitive outputs are enumerated, not assumed

- **Decision.** Port `CompareTypes`, `compareSymbols`, `compareNodes` and `CompareDiagnostics` line by line, and sort at exactly the points Corsa sorts. Construct the intrinsic types in Corsa's order so their ids compare the same way. List the residual id-sensitive cases (reverse mapped types without symbol or mapper, symbols without declarations and with duplicate names) and give each a test. Keep a creation-trace mode in both binaries for diagnosing those residual cases, scoped to them rather than imposed on the whole checker.
- **Because.** Corsa's own comment on reverse mapped types calls their id ordering unstable and accepts it as rare. That is the size of the real problem. Demanding identical allocation traces, as draft 1 did, would have constrained every allocation in the checker to fix a problem that the comparator already solves structurally.

### 6. Deep recursion

- **Decision.** Run parsing, checking and emit on threads created with large reserved stacks (256 MB to 1 GB; reservation is virtual and committed lazily on both target operating systems), and add growth guards in the deepest recursive paths.
- **Because.** Go grows stacks on demand up to 1 GB by default (`core.ApplyDebugStackLimit` only lowers it for debugging). A fixed 8 MB stack would introduce a new crash class that Corsa never had. Section 13, item 11 describes the trampoline audit and native/WebAssembly stress tests.

### 7. Panics and errors

- **Decision.** Invariants stay `panic!` and `debug_assert!` (Corsa has about 600 panic sites and twelve recovers). Each LSP request, API call and test step has a `catch_unwind` boundary with the same stack sanitizing as `lsp/stack_sanitizer.go`. I/O and configuration failures are `Result`. Locks are `parking_lot`; absence of poisoning is not evidence that state remains valid after a panic.
- **Invalidation.** A caught checker panic retires the checker/pool generation before the checker can be acquired again. All snapshots, projects, in-flight leases and API registries referencing that generation observe the retirement, not only the requesting snapshot: `Project.Clone` shares programs and checker pools while `api/session.go` stores separate registries. Registries validate generation before resolving handles; old registries cannot attach to a replacement checker with reused sequential ids. Retired storage stays owned until all leases, retained results and registry roots release it. If mutated shared state cannot be isolated to that generation, retire the affected session rather than continue with potentially inconsistent file or project caches.
- **Lease validity.** Retaining storage is distinct from retaining permission to use an active generation. The ownership note defines checked lease boundaries and a shared generation gate that serializes retirement with registry insertion and success-response commitment. Computation may finish on retained storage, but uncommitted results from a retired generation are discarded. Callback/reentrant paths that can retire or replace an owner must preserve the active-scope proof or revalidate before resuming. Ordinary recursion within an uninterrupted valid scope need not repeat generation checks.
- **Wire contract.** Generation metadata stays server-side; the pinned request/response shapes and handle syntax remain unchanged. Requests through invalidated snapshots or handles return the existing protocol error form. The recovery fixtures pin those errors and subsequent refresh/reconnect behavior against the unchanged client. E3 injects a panic through one of two retained snapshots sharing a pool and verifies that both reject old handles; Phase 6 repeats this with the real API client. Rebuilding initializes globals and demand caches again; only disposal is cheap.

### 8. Source bytes, JavaScript strings, wire positions and small semantics

- **Decision.** `SourceText` owns source bytes after the same BOM removal and UTF-16 decoding as Go. Ordinary UTF-8 input has a validated `str` fast path, but malformed input remains byte-backed and keeps the scanner's byte offsets. `JsString` owns cheaply shared immutable bytes: ordinary JavaScript values use UTF-8/WTF-8, while raw malformed literal bytes accepted by Go remain representable without lossy decoding. Operations explicitly distinguish validated Unicode, WTF-8 surrogate handling and raw malformed input; a `JsString` is not an unchecked Rust `String` or a promise that every byte sequence is valid WTF-8. E4 pins scanner, literal and encoder success/error behavior for each class.
- **Positions.** Convert byte offsets at the edge: UTF-16 code units for the API encoder, UTF-16 or UTF-8 for LSP as negotiated, through per-file position maps that preserve the oracle's handling of malformed sequences. Preserve the distinct API, LSP and scanner rules for partial-character offsets, clamping and line breaks; converted offsets may require byte slicing even for valid UTF-8. Do not replace invalid source bytes before scanning or silently normalize literal values to make serialization succeed. Helpers and printers still perform the explicit replacements and escaping that Go performs.
- **Small semantics.** Port `jsnum` with a shortest-round-trip printer and golden tests against V8. Regenerate identifier tables from Unicode 15.1. Port the organize-imports comparer as written: NFD normalization, natural-number keys and its case-first/accent rules; the locale preference is ignored and no collation library is used.
- **Because.** The encoder's string section is documented as UTF-8 with WTF-8 for such values, the JS client decodes it accordingly, and the relater and JSX transform special-case lone surrogates. "UTF-8 everywhere", draft 1's wording, cannot represent `"\uD800"` at all.

### 9. Host seams

- **Decision.** Object-safe, `Send + Sync` traits with the same shapes as `vfs.FS`, `compiler.CompilerHost`, the tsc `System` with its fake clock, `checker.Program` and the content-mapper host.
- **Because.** Every harness injects through these seams. Keeping them identical makes the harness ports mechanical and lets one test corpus drive both implementations.

### 10. Generated code stays single-source, generated here from the pinned schemas

- **Decision.** The pinned upstream resolvers and extractors remain authoritative throughout the port. A local adapter runs `tools/scripts/tsc/schema.ts` and exports normalized AST data for the Rust emitter; the LSP metamodel uses its pinned resolver in the same way. Diagnostics come from the pinned messages. The Go API dispatch code, annotations, serialization tags and special mappings are interpreted by the pinned `tools/gen-proto` extractor with a carried patch adding JSON export. `api.json` is a generated artifact refreshed on every pin bump, never a separately edited source of truth. The pinned enum extraction logic likewise supplies Rust's enum inputs.
- **Generated surface.** Rust DTOs, method identifiers, dispatch metadata and ordinary field serialization are generated from that export. Special codecs such as `DocumentIdentifier`, number/tristate conversions and custom omission/null rules retain explicit adapters matching Go; each exception is inventoried beside the export and covered by wire fixtures. The Go implementation is not regenerated or semantically patched for this migration.
- **Verification.** Generate `proto.generated.ts` from the exported contract into a separate output directory and require byte identity with the untouched pinned client's file before accepting the exporter or moving the pin. Build and test the original pinned client against Rust; substituting a changed client is not a parity check. Check generated Rust output for drift, and compare golden wire fixtures for both ordinary fields and handwritten codecs. Patches that expose schemas or test transports live here and are applied in a disposable tooling worktree; the oracle uses the unmodified pin. The Go contracts remain authoritative after a successful equality check; any future transfer of schema ownership is a separate decision.
- **Data.** Embed the pinned libs with `include_str!` behind a `noembed` feature; keep the 13 locale files gzipped and inflate on demand. Reusing upstream tooling locally does not require upstream cooperation.

### 11. This repository is the workspace; upstream is a pinned submodule

- **Decision.** A Cargo workspace at the root of this repository, one crate per Go package where the boundary carries meaning, tiny leaves merged into `tsr_core`. Test-only packages become test crates. microsoft/TypeScript is a git submodule at `upstream/`, pinned to a commit (1f70213d49 today), providing test data, schemas, libs, locales, the JS client, the extension, and the Go source from which the oracle binary is built and against which the ledger is kept. See the crate map below.
- **Because.** Cargo compiles and caches crates independently; a monolithic crate makes the 60,000-line checker the compile-time bottleneck of every change. A submodule keeps 342 MB of test data out of this repository's history while pinning exactly which upstream commit every baseline and schema came from.

### 12. Toolchain and lints

- **Decision.** Stable Rust, minimum supported version one behind current stable. `clippy` at pedantic minus an allow-list. `rustfmt`. `cargo nextest` for the suites, `lld` or `mold` plus `sccache` in CI. `#![forbid(unsafe_code)]` everywhere except the same leaf crates where Corsa uses `unsafe` today (file watching, paths, platform FFI). Release profile: `panic = "unwind"` (required by decision 7), fat LTO, one codegen unit, mimalloc. Section 13, item 14 decides how the custom Go lints are replaced; any compiler-private lint tool has its own pinned toolchain and does not change the compiler's stable-build requirement.

### 13. Targets and distribution

- **Decision.** Four targets: `aarch64-apple-darwin`, `x86_64-apple-darwin`, `x86_64-unknown-linux-gnu`, `aarch64-unknown-linux-gnu`. CI runs on macOS and Linux runners and cross-compiles the second Linux architecture. The output layout mirrors upstream's platform packages (`lib/tsc`) so the pinned JS client's `getExePath` and the extension resolve it. Not built: the Windows host layer (named pipes in `ipc`, the Windows watcher and realpath), musl, and every best-effort target. Windows-style path semantics in `tspath` stay because baselines exercise them. Code signing and publishing under upstream names are out of scope. The Linux binaries link against glibc 2.28, the floor of Node 18 and later, and CI rejects any binary whose ELF requires a newer symbol version (item 15 of the pending decisions).

### 14. Dependency policy

- **Decision.** Few, vetted crates, each recorded with the reason it exists. `cargo-deny` enforces an Apache-2.0-compatible license list, bans duplicate versions and known advisories; `cargo-vet` records an audit for every crate; `Cargo.lock` is committed; no build script touches the network; the minimum supported Rust version is checked in CI. The expected direct dependencies mirror the roles of Corsa's twelve Go modules: scoped threads or rayon, parking_lot, mimalloc, serde with a JSON crate matched to the Go JSON v2 usage, an msgpack crate, an xxh3 implementation that produces the same hashes the API encoder header carries today, a patience diff for baselines, flate2 for the locale files, a Unicode normalization crate for the organize-imports comparer, stacker, and the platform crates for file watching on macOS and Linux.
- **Because.** The Go module builds without cgo, without Node and with twelve direct dependencies, and CI proves it. A Rust tree that pulls in hundreds of transitive crates would be a regression in auditability.

## 7. Compatibility contracts

What must hold at cut-over, what pins it, and whether the existing verification can be reused as-is, bridged to the Rust binary, or must be ported.

| Contract | Pinned by | Verified with | Reuse |
|---|---|---|---|
| Type-check results: `.errors.txt`, `.types`, `.symbols` | 12,721 cases; 45,447 baseline files under `compiler` and `conformance`; `union ordering` and parent-pointer sub-tests | Ported compiler runner; Go oracle built from the pin; exact comparator behavior and consistency checks on the exercised types | Data as-is; runner ported |
| Emit: `.js`, `.map`, `.d.ts`, source-map records | Same corpus; 41 transpile baselines | Runner sub-tests | Data as-is |
| Command line: `tsc`, `--watch`, `-b`, `--incremental`, pretty output, locales | 517 baselines across `tsc`, `tscWatch`, `tsbuild`, `tsbuildWatch`; `.tsbuildinfo` JSON | Ported `tsctests` harness with fake system and clock | Data as-is; harness ported |
| Configuration parsing | 309 baselines under `config` and `tsoptions` | Ported unit tests | Data as-is |
| Language server over LSP 3.17 | 4,356 fourslash tests; 1,749 fourslash baselines; project and LSP suites; replay corpus | Phase 1 transport/filesystem contracts; Phase 5 semantic assertions from the pinned Go harness through the test-host patch, followed by full-suite coverage | Retain Go executable tests; patch transport only |
| JS API: msgpack framing, encoder protocol 8 with WTF-8 strings and UTF-16 positions, 144 methods, snapshots, batches, callback FS | `packages/typescript` sync and async suites and benchmarks; `api` baselines; `proto.generated.ts` | The suites spawn whichever binary `getExePath` resolves | Unchanged |
| Content-mapper plugins | JSON-RPC child-process protocol; `spanmap` fidelity; 15 contentmapper baselines | Ported `contentmappertest` | Harness ported |
| Diagnostic text and localization | 2,135 + 85 messages; 13 locales | Baselines; message-format unit tests | Regenerated |
| Semantics relative to TypeScript 6 | `tsc/CHANGES.md` | The corpus; Strada divergences are not "fixed" | Spec as-is |
| Small semantics: source-byte/BOM handling, Unicode 15.1 identifiers, WTF-8 and malformed literal bytes, JS numbers, organize-imports ordering, paths and symlinks | Pinned Go unit tests and client WTF-8 tests; E4 malformed-input fixtures | Ported unit tests plus differential fuzzing against the Go oracle, comparing diagnostics and success/error behavior as well as output bytes | Tests ported; boundary fixtures added |

## 8. Strategy and crate map

The Go oracle is built from the pinned upstream. Rust grows through explicitly scoped, tested dependency slices; a component may use a supported slice of another component before that dependency reaches full package parity. Phase gates distinguish this integration readiness from complete baseline coverage. Unsupported operations fail explicitly and are recorded in the slice's manifest; stubs do not count as passing parity tests.

- **Separate repository, pinned upstream.** This repository holds the workspace; `upstream/` is the submodule. Moving the pin is a deliberate operation: bump, rebuild the unmodified oracle, reapply tooling/test patches in a disposable worktree, re-export schemas, verify the untouched client's generated protocol file, list changed Go files against the ledger, port those diffs, and re-run every suite before the pin bump lands.
- **Every crate lands with its tests.** The Go unit tests for the package are ported, and where output is observable a differential test runs both binaries.
- **Oracle seams arrive with their dependencies.** Parser parity begins in Phase 0 by encoding each test input and lib file with both implementations. The Go fourslash harness injects an in-memory filesystem, symlinks and configurable case sensitivity, a parse cache and plugin spawners, inferred-project options and an initialization signal. Phase 1 therefore builds a test-host endpoint with callback filesystem operations (`readFile`, `fileExists`, `directoryExists`, `getAccessibleEntries`, `realpath`) and test-only initialization, options and plugin controls. Transport contract tests use representative fixtures to verify these operations, including cancellation and callback progress, without asserting language-service results. Phase 5 connects the same transport to the actual project system and service; only then do semantic fourslash passes contribute to coverage. The unchanged JS-API suite supplies full API parity in Phase 6. Harness and schema patches are carried against the pin separately from the oracle build.
- **A port ledger.** `PORTS.toml` maps each upstream Go file to its Rust module and the pin it was last synchronized to. Bumping the pin produces the list of files to re-port before the bump lands.
- **Agent-assisted translation with a hard oracle.** The mechanical tiers are good candidates for agent-driven translation, exactly as the Go port used. The rule is that nothing merges without its baselines, and the checker's data-layer redesign is human-led.
- **Gates instead of a calendar.** Section 9 distinguishes tested prototype dependencies from full parity milestones. The spike is the initial go/no-go point; later phases cannot close without their stated acceptance tests, and full native/WebAssembly/embedding acceptance blocks cut-over.

### Progress and evidence

The [tracking specification](docs/TRACKING.md), [current status](STATUS.md), [dashboard](docs/status.html) and [ADR index](docs/adr/README.md) accompany this plan. The Cargo tracking scaffold and the S04 text/ownership contract leaves are implemented; the compiler pipeline remains pending. See the [S04 synthesis plan](docs/S04-synthesis-plan.md) and [implementation/verification notes](docs/S04.md). The canonical `upstream/` submodule is registered, initialized and clean at `1f70213d4922b434345f639b441681e470c7cfc1`; its actual Go oracle build and `--version` smoke have passing execution evidence. ADRs 0006, 0007 and 0013 and their design notes are accepted; S01 passes against the current bootstrap evidence. The S04 leaf portions of E3 and E4 have executable producers; full E1–E8 verification remains pending. A status workflow for macOS and Linux is installed under `.github/workflows/status.yml`; no run of it is recorded here, and a rendered report never establishes that a job ran. The Phase 0 implementation plan is the sprint set under [sprints/](sprints/README.md). Bootstrap success does not establish Rust compiler parity.

Editable ledger states describe implementation work; `port:` markers describe function mappings. Neither establishes semantic parity or complete behavioral coverage. Verified states and experimental results are generated from validated run evidence, with every required test, baseline, generation and experiment gate enforced. Required sprint items have machine checks in addition to the sprint's exit criteria; missing checks cannot silently close an item.

`cargo xtask run <run-id>` captures typed metrics from a reviewed command specification in `status/runs.toml`, bound to the upstream pin, command, source digest and declared corpus/configuration input digests. Relevant edits invalidate earlier evidence. Producers require reviewed contracts defining their workload, denominator, units and assertions, because a valid evidence envelope cannot prove a metric producer implemented the intended test. `cargo xtask status` validates evidence and writes the reports; `cargo xtask check <sprint-id>` enforces the sprint gates. CI also uses `check-metrics` to require live build and quality results independently of unfinished sprints, and compiles with the declared minimum Rust version on macOS and Linux. `status --check-committed` checks all four generated views, including the unmapped-function worklist, in their recorded context without rewriting them; live gates retain current-host and toolchain validation. The tracking specification owns the detailed schema.

### Crate map, leaves first

Tiers group related crates; the actual dependency edges are in `data/import-graph.txt` and the package order in `data/topological-order.txt`. The phase column identifies full parity, except for the explicit Phase 0 parser gate of 99.9 percent; earlier tested slices are listed below. It is not a prohibition on using a dependency before that phase. For example, `checker` imports `printer` and `api/encoder` imports `spanmap`, so both need usable slices in the spike. Crates marked "generated" are emitted from a schema.

| Tier | Crates | Phase |
|---|---|---|
| 0, leaves: no compiler knowledge | `tsr_core`, `tsr_collections`, `tsr_tspath`, `tsr_stringutil`, `tsr_jsnum`, `tsr_json`, `tsr_locale`, `tsr_glob`, `tsr_semver`, `tsr_packagejson`, `tsr_vfs`, `ts_vfs_os`, `ts_vfs_match`, `tsr_diagnostics` (generated), `tsr_bundled` (generated) | 1 |
| 0, leaves: contracts the AST depends on | `tsr_jsstring` (source/string bytes and position maps), `tsr_arena` (checked owner/generation ids, lazy storage, bundle ownership and leases) | 0 |
| 0, leaves: platform | `tsr_fswatch` (FSEvents, inotify, fanotify), `tsr_nativepath` | 4 |
| 1, syntax: source text to parsed trees | `tsr_ast` (generated), `tsr_scanner`, `tsr_parser`, `tsr_encoder` (generated) | 0 (E1); 1 for full parse/bind coverage |
| 1, syntax | `tsr_binder`, `tsr_astnav`, `tsr_evaluator` | 1 |
| 2, semantics: resolution | `tsr_module`, `tsr_tsoptions` | 1 |
| 2, semantics: types | `tsr_modulespecifiers`, `tsr_checker` | 2 |
| 2, semantics: output trees | `tsr_pseudochecker`, `tsr_sourcemap`, `tsr_printer`, `tsr_transformers`, `tsr_declarations` | 3 |
| 3, programs: compile, build, watch | `tsr_outputpaths`, `tsr_transpile` | 3 |
| 3, programs | `tsr_compiler`, `tsr_incremental`, `tsr_build`, `tsr_execute`, `tsr_diagnosticwriter`, `tsr_tracing`, `tsr_pprof`, `tsc` (binary) | 4 |
| 4, editor: language service and projects | `tsr_format`, `tsr_ls`, `tsr_autoimport`, `tsr_lsproto` (generated), `tsr_project`, `ts_ata`, `tsr_contentmapper`, `tsr_spanmap` | 5 |
| 5, servers: processes and protocols | `tsr_lsp`, `tsr_testhost` (transport contracts in 1; semantic server integration in 5) | 5 |
| 5, servers | `tsr_ipc` (unix sockets), `tsr_jsonrpc`, `tsr_api` | 6 |
| 7, additional entry points | `tsr_wasm`, `tsr_embed` (parser/checker prototypes in 0; full compiler acceptance before cut-over) | 7 |
| Test crates, not shipped | `tsr_testutil`, `tsr_testrunner` | 1 |
| Test crates | `tsr_tsctests` | 4 |
| Test crates | `tsr_fourslash`, `ts_projecttest` | 5 |

## 9. Sequence and gates

Phases are ordered by dependency, not by time. The table names the tested slices needed to start and the complete coverage needed to finish. A phase can start once those slices exist; it need not wait for unrelated work in their owning phases. The spike is the initial go/no-go point, and the final acceptance gates remain mandatory.

| Phase | Needs first | Can run alongside | Gate |
|---|---|---|---|
| 0. Spike | The three contracts | Nothing; it goes first | E1 to E8 |
| 1. Foundations | 0 | Its own tail overlaps 2 | All cases parse identically; binder dumps match; config baselines; test-host transport/filesystem contracts |
| 2. Type checker | 1's AST, binder, module resolution and options | 3, 4, 5 | 100 percent of `.errors.txt`, `.types`, `.symbols` |
| 3. Emit | 1's parser and binder; 0's printer slice; declaration completion needs 2's emit resolver | 2, 4, 5 | 100 percent of `.js`, `.map`, `.d.ts`, transpile |
| 4. Programs and CLI | 1 for the driver, build and watch; 2 for full checking | 2, 3, 5 | All tsc, watch, build baselines; smoke test; sanitizer run |
| 5. Editor | 1's transport contracts and framing; 0's ownership components; 2's tested checker/service operations | 2, 3, 4 | 99.5 percent semantic fourslash; project, LSP, replay suites |
| 6. JS API server | 2 and 5's project system | 5's tail | `packages/typescript` suites pass unchanged |
| 7. Hardening, WebAssembly, embedding, cut-over | All parity phases; 0's WebAssembly/embedding prototypes | Nothing | Native, WebAssembly and Rust-consumer acceptance; every exit criterion for four consecutive weeks |

### Phase 0: spike

- **Scope.** Workspace skeleton with the `upstream/` submodule, CI on macOS and Linux, lint and format. Write the three contracts from section 1, item 8 before generation. Build the AST, scanner, parser/JSDoc and encoder plus the dependency slices below. The checker slice includes literal, object, union, intersection, array and tuple types; `getTypeOfSymbol`; `isRelatedTo`; narrowing; `typeToString`; the comparators; and declaration-merging/reentrancy fixtures that exercise ownership. Eight experiments have explicit thresholds. Their results apply to these slices, not to unimplemented compiler or editor behavior.
- **Dependency slices.** The following components must be implemented and tested during Phase 0, even when their full parity belongs to a later phase. The manifest records supported operations and dependencies; none is counted as a fully ported package just because the spike uses it.

| Component | Phase 0 implementation and use | Full parity |
|---|---|---|
| Core, diagnostics, options, libs and host | Parser/encoder dependencies, test-directive handling, pinned lib loading, and an in-memory `checker.Program` host for the frozen subset | 1 and 4 |
| Binder and resolution | Real binding and the resolution needed by the subset and parse/bind benchmark, with file-owned symbols and checker-local merges; unsupported scenarios listed explicitly | 1 and 2 |
| Span maps and encoder support | Data model, position conversion and serialization used by protocol 8; multi-output mapper ownership fixtures | 5 for full mapper integration |
| Printer and node builder | Actual `typeToString` paths for E2, synthetic-node retention and release; no claim of full JS/declaration emit | 2 and 3 |
| Ownership and registry harness | Production arena/resolver/lease components under minimal program, snapshot and API-registry owners; sharing, lazy allocation, bundle disposal and panic-generation tests | 5 and 6 repeat these through actual servers |
| WebAssembly and Rust embedding | Parser and checker-slice entry points with an in-memory host; a separate Rust consumer and the Node parse benchmark adapter | 7 for full checking and emit acceptance |

- **Experiments.**

| | Measures | Threshold | Nature |
|---|---|---|---|
| E1 Parser parity | Encoder-identical parses of 12,721 cases and 108 lib files | At least 99.9 percent | Measured |
| E2 Checker slice | `.types` and `.errors.txt` on the frozen subset; comparator consistency under the `union ordering` sub-test; checker throughput, allocations and retained memory reported separately from parser results; arena-reference relater prototype compared with the ID-based implementation | 100 percent baseline parity on the subset; comparator and prototype parity checks pass; usable checker and relater measurements recorded in S08 | Measured for the slice; not a proof of total ordering for all future types |
| E3 Ownership | The ownership and symbol design-note scenarios: shared files/bundles, checker merges and retained results, checker AST nodes, lazy page publication, builder cache/emit dependencies, API scratch disposal, id exhaustion, wrong-owner imports and shared-pool retirement/publication interleavings | Correct surviving queries; every invalid lookup rejected in release mode; every dependent registry/lease observes retirement and no stale result is published; owner/allocation counters return to the pre-scenario level after all owner, lease, cache, emit-table, retained-result and registry roots drop; no debug-assertion or Miri/AddressSanitizer violations | Measured in the ownership harness; repeated through real servers in 5/6. Debug and sanitizer runs supplement release-mode identity assertions; RSS is reported, not required to fall immediately after each drop |
| E4 Strings | The text design-note fixtures: corpus literals, malformed bytes/BOMs, helper replacements and truncation, original versus regenerated printing, byte-slice validity, separate API/LSP/scanner partial offsets and ECMAScript/LSP line maps | Match Go diagnostics, token/literal bytes, helper/printer behavior, per-path positions and encoder success/error; byte-identical output wherever Go encodes successfully | Measured; malformed input is not silently repaired to satisfy a Rust text type |
| E5 Memory | Peak RSS and bytes allocated for parse and bind of the VS Code repository, Rust against Go; the slice's per-type footprint on the subset | Parse and bind at most 85 percent of Go (re-based from 70 percent by ADR 0021 on measured 74 percent); per-type at most 85 percent (re-based from 80 percent by owner decision ADR 0022) | Measured for parse and bind and the checker subset's per-type footprint; full checking remains extrapolated |
| E6 CPU | Parse and bind wall time on the VS Code repository at 1 and 8 threads | At most 1.25 times Go at 1 thread and 1.45 times at 8 (re-based from parity by ADR 0021 on measured 1.21 and 1.40) | Measured for parse and bind; extrapolated for checking |
| E7 WebAssembly | Parser benchmark against Go `GOOS=js`, plus the E2 checker slice built and run with an in-memory host without native process/filesystem dependencies | Parser artifact at most 30 percent of Go's size and at least 1.5 times its parse throughput (re-based from 25 percent and 2 times by owner-approved ADR 0020); checker subset diagnostics/types match the oracle | Parser performance and checker-slice feasibility measured separately; no claim about full checking/emit parity or speed |
| E8 Embedding | A separate Rust consumer supplies the host and runs the E2 checker slice with repeated create/query/drop operations; a Node adapter separately benchmarks parse-and-encode against `--api` | Rust consumer matches the subset and passes applicable E3 lifetime tests; Node parse latency at most 40 percent of the socket path for a 10 KB file (re-based from 10 percent by owner-approved ADR 0020) | Rust embedding feasibility and Node parse latency measured separately; full crate surface and workload budgets remain Phase 7 gates |

E4 progresses through leaf helpers in S04, scanner values in S05 and encoder behavior in S06; its full literal-type and printer integration criteria close in S08. The S08 `checkerbench` and `relater` producers enforce the separate measurements and alternative ownership-model prototype required by section 13, items 6 and 7. These measurement gates require usable samples without adding a performance target.

- **Gate.** E1 to E4 pass; E5 to E8 meet their thresholds for the parts they measure, with the extrapolations written down as extrapolations. Otherwise stop.

### Phase 1: foundations

- **Scope.** Complete the foundational slices: `core`, `collections`, `tspath`, `stringutil`, `jsstring`, `jsnum`, `json`, `locale`, `glob`, `semver`, `packagejson`, the `vfs` family including `vfstest`, diagnostics, libs, the AST's owner/lazy/bundle mechanisms, scanner, parser, file-owned binder, `astnav`, `evaluator`, module resolution and options. Extend the compiler runner through complete parse/bind and syntactic `.errors.txt` coverage. Build test-host framing, initialization, server-owned option completion, filesystem callbacks and plugin byte streams in a transport endpoint that does not depend on the language service. Complete generator drift and untouched-client equality checks.
- **Gate.** All 12,721 cases parse identically; binder symbol-table/flow-graph dumps match; the 309 config/options baselines pass. Dedicated transport tests verify case sensitivity, symlink resolution, plugin byte streams, initialization, cancellation and progress while reverse callbacks are outstanding, using representative fourslash fixtures. The synchronous filesystem bridge with blocked workers is a Phase 5 gate (ADR 0019). They assert transport/filesystem contracts only; semantic fourslash passes are a Phase 5 gate.

### Phase 2: type checker, the critical path

- **Scope.** In an order where each step unlocks a slice of baselines: symbol resolution, declared and inferred types, literal, enum, tuple and array types; the relater (assignable, subtype, strict subtype, comparable, identity) and variance; signatures, overloads, inference and contextual typing; control flow and narrowing from `flow.go`; mapped, conditional, template-literal and indexed-access types with instantiation, mappers and depth limits; classes, interfaces, enums, namespaces, late binding, declaration merging and the JavaScript semantics in `CHANGES.md`; JSX; grammar checks and all 2,220 diagnostics; the node builder behind `typeToString`, `typeToTypeNode` and hover, with its builder arena and release protocol; the emit resolver and the services surface in `services.go` and `symbolaccessibility.go`; the checker pool, parallel checking, cancellation and `--generateTrace`.
- **Tooling.** A burn-down harness that runs both binaries per case, aligns `.types`, `.symbols` and `.errors.txt`, buckets differences by diagnostic code and checker area, and publishes a pass-rate dashboard per area. The creation-trace diff from decision 5 for the residual id-sensitive cases.
- **Gate.** 100 percent of compiler and conformance `.errors.txt`, `.types` and `.symbols` baselines with an approved allow-list; `union ordering` and `source file parent pointers` sub-tests green.

### Phase 3: emit

- **Scope.** `printer`, `sourcemap`, the TypeScript, ES, module, JSX and inliner transforms with their transform arenas and original-node side tables, the declaration transform (which depends on the emit resolver and `pseudochecker`), `outputpaths` and `transpile`; the `.js`, `.map` and `.d.ts` sub-tests and the transpile runner. Starts once the parser and binder are stable and finishes only after the emit resolver exists.
- **Gate.** 100 percent of `.js`, `.map`, `.d.ts` and source-map-record baselines and the 41 transpile baselines.

### Phase 4: programs, command line, build and watch

- **Scope.** `compiler` (program, file loader, project references, checker pool, emit orchestration); `execute/tsc` with pretty diagnostics, colors and locales; the `-b` orchestrator; `incremental`, reading and writing Corsa's `.tsbuildinfo` so both binaries can share build state; the watch manager and `fswatch`, porting the FSEvents, inotify and fanotify backends rather than substituting a generic crate, because their behaviors are baselined; `diagnosticwriter`, `tracing`, `pprof`; the `tsctests` harness with its fake system and clock.
- **Gate.** All 517 tsc, watch, build and build-watch baselines; the smoke test compiles upstream's fixture project single- and multi-threaded; a ThreadSanitizer build mirrors upstream's race-mode job; the `--singleThreaded` and concurrent-test-programs configurations both pass.

### Phase 5: language service, project system, LSP server

- **Scope.** Connect Phase 1's test-host transport to the project system, request queue and checker service operations. Implement the synchronous filesystem-to-reverse-request bridge and prove cancellation/progress while workers are blocked; supply real options completion and parse-cache injection (ADR 0019). Implement real snapshots, synchronized file-owned lazy storage, mapper bundles, and pool-generation invalidation; repeat E3 through these components. Then implement the language service in order of fourslash weight: completions including string completions and auto-imports; hover; definitions; references, highlights and rename; signature help; diagnostics; code actions/fixes; organize imports; inlay hints; call hierarchy; code lens; folding; selection ranges; semantic tokens; linked editing; document/workspace symbols; JSDoc snippets and format. Complete config-file registry, inferred projects, cross-project references, automatic type acquisition, overlay FS, parse cache, checker pool, background work, logging, progress, watchers and stack sanitizing. API-over-LSP integration completes with Phase 6.
- **Testing.** The pinned Go fourslash suite remains the canonical executable corpus and drives the Rust service through the carried harness patch. Validate semantic results first on representative case-sensitivity, symlink and plugin tests, then report passes against all 4,356 tests with unsupported/failing cases visible. Phase 1 transport checks are not counted as these passes. Replay files run against both servers. Any later test-language conversion must preserve the current Go cases and prove equivalent assertions; it is not a dependency of the editor gate.
- **Gate.** At least 99.5 percent fourslash with a triaged allow-list; project and LSP suites; synchronous filesystem bridge cancellation/progress with blocked workers; replay corpus; no request-latency regression against Go on the benchmarking editor scenarios.

### Phase 6: JS API server

- **Scope.** `ipc` over unix sockets with both sync and async connections; msgpack framing; the generated encoder at protocol 8 with WTF-8 strings and UTF-16 positions; the 144 methods with snapshot, temporary-snapshot, batch, pagination and disposal semantics; request-scoped scratch arenas for API printing and insertion formatting; the callback file system; the schema-driven `proto.generated.ts` and enum generation for the pinned TypeScript client.
- **Gate.** The untouched pinned `packages/typescript` sync and async suites and benchmarks pass against Rust; API baselines and codec wire fixtures match. Through the real client, inject a panic in one of two retained snapshots sharing a checker pool: every affected registry rejects old handles, valid unaffected pools continue working, and refresh/reconnect cannot alias retired ids. These tests validate the existing wire error forms rather than changing the client.

### Phase 7: hardening, WebAssembly, embedding, cut-over

- **Scope.** The TypeScript-benchmarking scenarios at 2, 4 and 8 checkers; allocation/layout tuning; differential fuzzing; crash replay; four native targets and a binary-size budget. Expand the E7/E8 entry points to full compiler checking and emit with explicit in-memory/callback hosts. The WebAssembly library does not include native CLI/watch/process services; it must execute compiler work without them. A separate Rust consumer exercises the documented crate API. Run the corpus, lifetime/failure cases and the pre-agreed WebAssembly/embedding workload budgets through those entry points before dogfood and cut-over.
- **Gate.** Every exit criterion in section 5, including full WebAssembly and Rust-consumer acceptance, holds for four consecutive weeks. Neither deliverable can be deferred until after cut-over under this plan.

The owner-approved [full acceptance matrix and workload budgets](docs/S12-acceptance.md)
are recorded by ADR 0020. These retain the native cut-over goals and set separate
full-compiler WebAssembly and Rust-consumer budgets. The amended E7/E8 parser
limits do not replace them.

## 10. Risk register

| Rank | Severity | Risk | Evidence in the repository | Mitigation |
|---:|---|---|---|---|
| 1 | High | The checker cannot be made to fit the borrow checker without changing behavior | 320-field `Checker`, 26 pointer-keyed link stores, function-valued fields, mutation anywhere through `*Checker`; the reason Go was chosen | Decisions 2, 3 and 5; the Phase 0 slice before committing; structure-preserving translation; comparators ported line by line |
| 2 | High | Upstream outruns the port | 113 to 237 commits a month; thousands of commits over the life of the port | Pin bumps at a cadence; the ledger lists exactly what to re-port; Go names kept in Rust; agent-assisted diffs with baselines as the gate |
| 3 | High | Ownership or identity fails under real sharing | Shared files allocate lazy nodes; mapper bundles have cyclic logical links; merged symbols are checker-local; several snapshots can share a checker pool | Decisions 1, 2 and 7; E3 owner/generation assertions, concurrent lazy allocation, bundle disposal, two-checker merges and shared-pool panic tests; Miri/AddressSanitizer supplement them; repeat through actual servers |
| 4 | High | The benefits do not materialize | Corsa already took the large win; Go's checker pool and arenas are tuned | Required E5 to E8 probes with bounded claims; full checking/emit acceptance and named workload budgets for WebAssembly and embedding block cut-over |
| 5 | Medium | Stack overflow as a new crash class | Go stacks grow to 1 GB; Corsa removed the old trampolines and has no depth guards beyond TypeScript's own limits | Decision 6: large reserved stacks and growth guards; section 13, item 11 on trampolines; native and WebAssembly deep-input tests |
| 6 | Medium | Editor parity stalls on the long tail | 4,356 fourslash tests, 41,500 lines of service code, many features with few tests each | Phase 1 transport contracts; semantic pass rates from Phase 5; retain the pinned Go executable corpus and a visible owner-approved allow-list |
| 7 | Medium | The test-host protocol underestimates the harness | The harness injects an in-memory file system with symlinks and case sensitivity, a parse cache, plugin spawners, an inferred-options hook and an in-process init signal | Separate transport from semantic assertions, validate representative fixtures first, carry and revalidate the harness patch on each pin bump |
| 8 | Medium | Source or string bytes are corrupted by Unicode assumptions | Go preserves malformed source bytes and some raw literal substrings as well as WTF-8 lone surrogates | Decision 8; byte-backed `SourceText` and `JsString`, explicit validation/conversion boundaries and E4 malformed-input/BOM fixtures; no implicit lossy conversion |
| 9 | Medium | Small-semantics drift | JS number formatting, Unicode 15.1 tables, the organize-imports comparer, path and symlink rules each have baselines | Decision 8; port the Go unit tests first; differential fuzzing |
| 10 | Low | Compile times slow iteration | 60,000-line checker crate; generic-heavy code | Crate split per decision 11; sccache; lld or mold; a compile-time budget in CI |
| 11 | Low | The generators fall behind upstream schema changes | AST/LSP/diagnostic inputs and Go API contracts change upstream; exporter patches may stop applying | Re-export from the pinned authoritative tooling on every bump; verify byte identity with the untouched client's protocol file, Rust output drift and codec wire fixtures |

## 11. Consumed from upstream, not rewritten

The runtime client, executable tests and data below come from the pinned submodule. Exporter and test-transport patches are explicitly separated from the unmodified oracle and client.

- **Test data and baselines** under `tsc/testdata`: 342 MB of language-agnostic inputs and expected outputs, reused byte for byte.
- **`packages/typescript`**, the unchanged pinned npm package and JS API client, pointed at the Rust binary. Regenerated protocol output is compared against its original file, not substituted to hide a mismatch.
- **`packages/vscode-typescript`**, pointed at the Rust binary.
- **Schemas, resolvers and data**: `ast.json` and its TypeScript resolver, the LSP metamodel and resolver, `diagnosticMessages.json`, the 108 lib files, the 13 locale files. Local adapters expose normalized data for Rust emitters.
- **API and enum extractors**, reused from the pin. A maintained patch adds neutral API-schema export without changing Go runtime semantics. Patches run in a disposable tooling worktree and are checked on every pin bump.
- **The Go fourslash suite**, retained as the executable specification, with a separate transport patch for the Rust test endpoint. That patch does not change expected semantic results.
- **The Go source**, from which the oracle binary is built and against which the ledger is kept.
- **`CHANGES.md`** as the specification of Corsa's intentional divergences from TypeScript 6.

Upstream's release pipelines and signing tools are not used. Local tasks build the oracle/client and invoke the needed pinned tooling; `tools/gen-proto`, the AST/LSP resolvers and enum extraction logic remain dependencies. The custom Go linters inform Rust's replacement checks under section 13, item 14. A successful generation check does not transfer API-schema authority away from the pin.

## 12. Alternatives considered

- **Hybrid: Rust only for scanner, parser and emit behind an FFI.** The parser is not the bottleneck, the FFI boundary would cross on every node access, and two toolchains would ship in one binary. Rejected.
- **Adopt an existing Rust AST and parser** such as oxc's. The AST shape is pinned by `ast.json`, the binary encoder and the generated TypeScript AST package, and thousands of checker and service code paths are written against it. Borrow the allocator and layout ideas; do not adopt the types. Rejected as a dependency.
- **Rewrite from the TypeScript 6 source rather than from Go.** The Go tree is the current semantics, the baselines are Corsa's, and the Go port already answered every question about JavaScript-specific idioms. Rejected.
- **A query-based incremental architecture** in the rust-analyzer style. The right long-term shape for an editor service, and incompatible with landing a drop-in replacement on a moving target. Deferred until after cut-over.

## 13. Open questions and pending technical decisions

Remaining choices are resolved by measurement or owner review. None may override the active contracts in section 6 or waive section 5's cut-over gates implicitly:

1. **The spike's curated subset.** Pending item 16 below.
2. **Performance budgets per benchmark scenario.** Settled by owner-approved ADR 0020 and [the full acceptance matrix](docs/S12-acceptance.md). Native targets retain section 5's Go-relative goals. Separate full WebAssembly and embedding size, latency and retained-memory budgets apply to the named release workloads; parser measurements are not used as full-checker thresholds.
3. **WebAssembly and embedding API details.** The host callbacks, public Rust types and browser adapter are refined from the E7/E8 prototypes. Both entry points must support full checking and emit and pass section 5 before cut-over; delivery timing is not deferred beyond that gate.
4. **The residual id-sensitive cases in practice.** Whether reverse mapped types without symbol or mapper, and undeclared duplicate-name symbols, ever change observable output in the corpus. Measured in the spike.
5. **The fourslash harness patch — settled.** Carry a transport-only patch against the pinned Go harness, retaining its executable assertions; do not fork the harness. Accepted in ADR 0019 on 2026-09-19.

Implementation choices and status, including references to the settled contracts:

6. **Storage layout for nodes and types.** An experiment, not a decision. Generated accessors are independent of the representation, so the layout can change without touching callers. A fully inline enum sizes every slot to its largest variant, so large payloads (source files, JSDoc, classes) are boxed or split out from the start. E5 and E6 measure parsing and binding; the layout of types is chosen only after the checker slice reports throughput, allocations and retained memory. One generator with a representation switch, one production output at a time.
7. **Lists, borrows and reentrancy.** Mutable builders, immutable published lists: Go sorts, compacts and modifies type and signature lists while constructing them, then never again. Published lists are independently owned handles, `Arc<[TypeId]>` rather than `Rc`, because the checker must be `Send`; a slice borrowed from `self` would still block a recursive `&mut self` call. Lazy resolution keeps Corsa's `pushTypeResolution` and `popTypeResolution` guard, since `getTypeArguments` and its peers can re-enter themselves. The arena-references-with-interior-mutability alternative is prototyped on the relater core in the spike, so that it is a measured fallback rather than an assumed one.
8. **Interning.** Deferred, with a corrected rationale: files outliving programs does not force an immortal process-global interner; file-owned interners, session-scoped interning and reclaimable shared entries are all available later. Start without interning, but never deep-copy bytes where Go copies a string header: `JsString` values are cheaply clonable shared immutable buffers. Measure string duplication separately from interning before deciding.
9. **Threads versus async.** Synchronous compiler and service APIs. Not a thread per goroutine: bounded worker pools for parse, bind, check and emit, and transport readers that progress independently of workers, because the API transport handles responses while requests are executing and a blocked worker must not stall cancellation or callback traffic. An async transport is allowed; it does not spread into the service.
10. **Panic policy: settled contract.** Decision 7 retires the failed checker/pool generation across every referencing snapshot, project, lease and registry, preserving the pinned wire format. The spike prototypes retirement and draining; Phase 6 verifies actual client error and refresh/reconnect behavior. Recovery cannot resume through an old registry merely because a replacement checker has been allocated.
11. **Deep recursion.** Reserved stacks and growth guards in general, plus trampolines at every site that recurses on the left operand: the binder, `checkBinaryLikeExpression`, the transforms and the printer. The stress tests cover the full parse, bind, check, transform and emit path, left- and right-associative chains, and deep nesting of parentheses, JSX and conditional types; native and `wasm32` stack strategies are validated separately, because WebAssembly stacks do not grow.
12. **Generator language: settled contract.** Decision 10 reuses the pinned AST/LSP resolvers as the only schema interpreters; local adapters export normalized data and Rust emits the implementation. Representation experiments change Rust output, not schema interpretation.
13. **API schema: settled contract.** Decision 10 keeps the pinned Go extractor authoritative on every pin bump. `api.json` is generated; DTO/dispatch output and codec exceptions are explicit. Equality with the untouched TypeScript client and wire fixtures are acceptance checks, not permission to start maintaining a second contract.
14. **Custom lints.** Upstream registers seven analyzers. `bitclear`, `shadow` and `cleanup` are Go- or Go-testing-specific and moot; `unexportedapi` is rustc's private-interfaces lint; `emptycase` is clippy configuration. `forbidparentaccess` becomes structural, transform crates get a node API without a parent accessor, with the declarations transform exempt as upstream exempts it. `checkchildren` is a compiler-specific analysis that flags returns which skip child checks and would make diagnostics depend on traversal order; clippy cannot express it, so a replacement is budgeted, a `rustc_driver`-based lint or dylint, chosen in the spike.
15. **Linux glibc floor: decided, 2.28.** musl is out of scope, so each Linux architecture ships one glibc binary linked against glibc 2.28, the floor of Node 18 and every later release line, which excludes Node 16's 2.17 even though the pinned client still declares Node 16.20 as its minimum. In distribution terms that is Debian 10, Ubuntu 20.04 and RHEL 8 or newer, and it excludes Amazon Linux 2. CI inspects the versioned symbol requirements of the final ELF and fails on anything above 2.28, and the suites run on a glibc 2.28 image. Static musl remains the measured alternative if a single universal Linux binary is ever wanted.
16. **The spike subset.** A checked-in rule applied across directories by feature criteria, including `conformance/controlFlow`, produces a frozen manifest of inputs, options and exclusions. It also records the dependency operations needed by E2/E7/E8. Mandatory ownership, lazy-allocation, mapper-bundle, declaration-merging, malformed-input, recursion, reentrancy and panic-retirement fixtures sit alongside it; changing eligibility cannot hide a failing case.

## 14. First steps

1. Use the accepted design notes behind ADRs 0006, 0007 and 0013 to implement the Phase 0 ownership and text contracts. Turn their E3/E4 scenarios into fixtures before AST generation is relied on: retained checker results and synthetic nodes, lazy publication, builder emit-table dependencies, retirement versus result commitment, and per-path string and position behavior. Contract acceptance completes S01; it does not satisfy these implementation gates.
2. Keep the initialized `upstream/` pin, generated provenance and workspace/oracle evidence current. The pinned toolchain, lint and format configuration, dependency policy and the macOS/Linux status workflow are S02; generation tasks are S03 (sprints/). The actual bootstrap build does not satisfy the design-note or compiler-implementation gates.
3. Use the oracle built from the unmodified pin. In a separate tooling worktree, run the pinned resolvers/extractors through the carried adapters, emit Rust, and compare regenerated TypeScript against the untouched client. Implement the Phase 0 dependency slices on top of `jsstring` and `arena`.
4. Build parser parity over the corpus and libs plus malformed-byte/BOM fixtures. Compare encoded bytes when Go succeeds, and scanner/encoder errors and positions when it does not (E1/E4).
5. Implement the frozen checker slice, its binder/resolution/printer dependencies and the comparators. Add the E3 identity, lazy-allocation, bundle, shared-pool panic and disposal scenarios as permanent assertions alongside sanitizer runs.
6. Design the test-host protocol and prototype callback filesystem/plugin operations over the connection using case-insensitive, symlink and mapper fixtures. These are transport contract tests; defer semantic fourslash assertions to Phase 5 and keep their coverage separate.
7. Create the allow-list with the owner as approver and maintain the existing ledger under the [tracking specification](docs/TRACKING.md). Review metric-producer contracts and gate definitions, require checks on every required sprint item, and keep implementation/function mapping separate from generated verified parity.
8. Run E5 to E8, including the actual Rust consumer and WebAssembly checker slice, and record numbers with their workload limits. Define the full checking/emit acceptance matrix and fix its remaining performance budgets before Phase 7.

---

Numbers were measured on commit `1f70213d49` of microsoft/TypeScript (5 September 2026), excluding `testdata`, `node_modules` and test files unless stated. The package dependency order was produced by `go list` with Go 1.27.1 and is stored in `data/`. Draft 3.2 retains the independent repository, four native targets and owner-approved divergences. It makes file-owned lazy storage, mapper bundles, checker-local merges and shared-pool panic retirement explicit; adds source-byte and semantic id validation; separates prototype dependencies from full parity gates; keeps the pinned schema extractors authoritative; and requires full WebAssembly and Rust embedding acceptance before cut-over. These are implementation contracts and planned checks, not claims that a Rust implementation has already passed them.
