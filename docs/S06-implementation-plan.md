# S06 parser, AST runtime and protocol-8 implementation plan

Planning branch: `codex/s06-parser-plan`, based on PR #9 at `6491399`.
Upstream authority: `1f70213d4922b434345f639b441681e470c7cfc1`.
This document defines implementation and verification work. Implementation began
after the plan review; completion is measured by the checkpoints below, not by
the presence of this document. The review records distinguish source findings,
proposed decisions and measured implementation results.

## 1. Acceptance, scope and source inventory

Follow the accepted [ownership](design/ownership.md) and [text](design/text.md)
contracts and ADRs 0006, 0010–0018. The pinned functions and their callers determine behavior;
comments and protocol diagrams must be checked against executable code.

| S06 requirement | Deliverable and acceptance |
| --- | --- |
| S06-1 frozen denominator | 12,721 compiler/conformance physical cases plus 108 lib files; freeze virtual units, effective parse options, auxiliary assets and every eligibility reason before Rust results exist |
| S06-2 parser/JSDoc/reparser | Real `tsr_parser` over the current scanner and owning AST; grouped case results compare protocol-8 output against the clean Go pin |
| S06-3 AST runtime | Construction, list identity, parents, visitors, updates/clones, precedence, subtree facts and SourceFile services; independent runtime observations beyond encoded bytes |
| S06-4 encoder | SourceFile and fragment encoding, string/structured data, position conversion and node-index tables; exact Go success/error outcomes and successful bytes |
| S06-5 traceability | At least 463/514 parser, 61/67 encoder-package and 420/839 AST source functions mapped to real implementations. Generated functions have separate provenance and earn no source-function coverage. |

The package called `internal/api/encoder` includes the decoder. Encoding and its
string table cover 38/67 handwritten functions and cannot meet the 90% gate.
Include all 29 handwritten decoder functions and the eight generated
encoder/decoder dispatch functions in S06. Generated dispatch earns no credit in
the 67-function denominator; any omission must be justified by exact ID rather
than an unimplemented wrapper.

| Package/file | Inventoried functions |
| --- | ---: |
| parser/parser.go | 431 |
| parser/jsdoc.go | 54 |
| parser/reparser.go | 21 |
| parser/references.go | 2 |
| parser/utilities.go | 6 |
| encoder/encoder.go + encoder_generated.go + stringtable.go | 33 + 5 + 5 |
| encoder/decoder.go + decoder_generated.go | 29 + 3 |
| ast/ast_generated.go | 1,386 |
| ast/ast.go + utilities.go | 307 + 418 |
| Remaining AST files | 116 |

Keep `data/go-functions.tsv` as the inventory authority. Generated shape/file
markers and S03's optional casts do not automatically implement Go factory,
update, cloning or assertion-cast semantics. Required source-grammar paths may
not be deferred merely because the mapping threshold permits 51 omissions.

E1 retains its published 99.9% threshold. E4 requires exact encoder success/error
and bytes over the same integrated corpus: the conjunction is stricter wherever
Go encodes. Do not use E1's numerical allowance to waive an E4 mismatch, select
only matching parses for E4, or shrink the frozen denominator after failure.

S06 does not implement binding/program resolution, checker state, printer/emit,
content-mapper execution, incremental editor updates, full E3/E4, WebAssembly,
embedding or performance gates. `reparser.go` means the real JSDoc reparser,
not a promise of an incremental source-file parser. Dependency slices and
explicit mapper-metadata fixtures do not mark those later packages complete.

## 2. Public boundaries and module structure

Create `tsr_parser`; extend `tsr_ast`, `tsr_encoder`, `tsr_arena`, the narrow core/path
slice and the authoritative S03 Rust emitters. Add `tsr_spanmap` only for the
protocol-8 data/serialization operations and explicit metadata fixtures needed
here. Do not build the later compiler host or general API server to run E1.
PR #9 remains the stacked dependency at `6491399`. The buffered-diagnostic,
retained-value and spelling-helper APIs below are scanner follow-ups in S06;
the parser must consume their verified interfaces rather than redefine S05's
accepted callback, checkpoint or byte contracts.

Proposed private modules follow invariants rather than copying one very large
Go file:

| Area | Modules and responsibility |
| --- | --- |
| Parser | `lib`, `worker`, `state`, `diagnostics`, `lists`, `expressions`, `types`, `declarations`, `statements`, `jsx`, `json`, `jsdoc/*`, `reparse`, `references` |
| AST | `node`, `lists`, `factory`, `source_file`, `parse_options`, `visitor`, `clone`, `precedence`, `subtree_facts`, `diagnostics`, `utilities/*`; generated dispatch remains visibly generated |
| Encoder | `encode`, `decode`, `node_table`, `strings`, `structured`, `positions`; generated field/common-data and decoder/factory dispatch stay separate from handwritten special codecs |
| Evidence | `scripts/s06.py`, purpose-specific build/corpus/protocol helpers, `scripts/s06_oracle/*`, and thin Rust adapters over the production crates |

The source-file parse boundary accepts owned `SourceText`, a separate ScriptKind,
and SourceFile parse options: normalized filename, path, and external-module
indicator booleans `JSX`/`Force`. These are the actual pinned Go fields. Do not add
compiler target or a JSDoc-mode argument to pretend they are parser options.
Go `initializeState` leaves the scanner target at its default. Freeze explicit
witnesses for default target and for language-variant selection: Go maps JS and
JSON, as well as JSX/TSX, to its JSX scanner variant.
Keep ScriptKind's open `i32` domain: zero triggers the pinned
`ScriptKind must be specified when parsing source file: <filename>` panic;
unknown nonzero values take the ordinary default branches. A closed enum or
blanket unknown-value rejection changes this boundary. The corpus's 29 assets
classified as zero are retained with an explicit non-parser disposition.

Cursor/getter/diagnostic positions retain S05's `i64` Go-int domain on supported
64-bit targets. Existing S04 helpers remain `isize`; node/list ranges remain
`i32`. Narrow through `TextRange` at the Go boundary, not when storing the scanner
cursor. API encoder positions use the existing API PositionMap; scanner or LSP
rounding is not interchangeable. Wire node indexes are not arena slots.

`ParseIsolatedEntityName` returns an owning fragment or an ID together with its
explicit owner. Its root retains a nil parent; do not invent a SourceFile parent
or return a dangling bare ID to make the API convenient.

## 3. Construction, ownership and publication

Resolve these seams before porting grammar bodies.

**One node header.** Do not wrap the current `tsr_ast::Node` unchanged inside
`tsr_arena::Node<N>`: kind and reparsed flags would have two authorities. Define a
small metadata interface in `tsr_arena` so its storage can hold the AST's concrete
record with typed kind, flags, range and parent ID. Keep the current generic
arena node as the S04 adapter. The storage crate must not depend on `tsr_ast`.
The runtime header uses an open signed `NodeKind(i16)`: Go factories accept
unknown kinds and even known kind/payload mismatches. Keep the scanner's closed
`SyntaxKind` separately. Node.ForEachChild dispatches by kind, whereas Clone,
VisitEachChild, Name and subtree facts dispatch through the payload interface;
do not turn every operation into the same generated kind switch.
Heavy payloads remain split/boxed as appropriate; this is not the later measured
node/type layout decision.

**Lists are objects as well as edges.** Replace runtime `NodeListRange` use with
owner-qualified list identity resolving to a header and immutable edge range.
The header carries its location and modifier data where applicable. Parser
missingness follows a private empty backing-slice sentinel, as upstream does;
replacing the slice changes missingness and cloning a header preserves it.
Preserve absent, present-empty and missing lists, distinct
empty-list identities, and raw slices separately. Go's `core.Same` compares raw
slice length/backing identity, except all zero-length slices compare equal;
element equality is not a replacement. A cloned list gets a new header while
it may share edges. Public factory list elements retain `Option<NodeId>` where
Go accepts nil; do not silently filter them at construction.
Raw-slice identity also applies to the four generated JSDoc text-slice payloads,
currently `Box<[JsString]>`: Update compares backing identity and Clone shares
the slice. Use owner-qualified shared slice ranges for these strings too;
copying a Box or comparing its elements would lose the Go operation's identity.

**Retention includes everything reachable.** Lists, source metadata, eager and
lazy JSDoc roots, reparsed clones and source bytes must be owned below the same
file/bundle retention root as their nodes. An outer AST wrapper with a separate
list vector is insufficient because an escaped `RetainedNode` currently keeps
only its FileHandle alive. Parent/child/original/list links stay non-owning IDs.
Diagnostics and source-file cross-references also use non-owning identity under
that root; a diagnostic must not retain its own containing file in a cycle.
Foreign-owner and unpublished list/node imports are checked in release builds.
Validate every stored reference, including fields intentionally omitted from
ForEachChild. Mutable construction can introduce invalid edges after factory
allocation, so completion and shared publication validate again. These scans
are an explicit O(nodes + auxiliary storage + metadata references) boundary
cost, separate from ordinary borrowed lookup. Mapped publication validates the
whole consumed group before creating its shared root.

A factory can contain several SourceFile nodes with distinct text and metadata.
Keep source-file services keyed by SourceFile identity, with lazy position maps
and encoder caches below that owner. JSDoc entries use both logical SourceFile
and parent identity, including when cloned SourceFiles share their children.
Storage-level parent-only caches cannot represent this boundary. Copied metadata
arrays have owner-qualified backing; exclusive mutation of an existing element
is visible through cloned headers, while header replacement is independent.
Text and parse options are construction-only, so a cached position map cannot
become stale through later text replacement. Cache installation validates against
the logical source's own retention root, including its mapped bundle and retained
imports, never an importing caller's broader lookup context. Cloning copies only
the fields named by
SourceFile.copyFrom after OnCreate; diagnostics, counts, hash and caches start
fresh. Storage ownership alone is not a source-file identity.

**Parse completion is not shared publication.** The accepted contract freezes
core storage after binding. Return an exclusively owned `ParsedFile` backed by
mutable construction storage, with borrowed read views for encoding and AST
operations. S07 must be able to consume or exclusively borrow that object for
binding, then consume it into shared FileHandle publication. Do not freeze all
parsed nodes now and silently replace later binder mutation with side tables.
Explicit unbound publication for parser tools/ownership fixtures is a distinct
operation and must not claim binding occurred. No shared mutable core access is
introduced by this distinction.

ParsedFile already owns the lazy arena, cache, list storage and their identities:
encoder node-table construction can request lazy JSDoc before binding. Shared
publication transfers that storage unchanged instead of allocating a fresh lazy
arena or rebuilding the cache. The source-owned PositionMap is likewise lazy
and transferred intact if already requested. Offer a common borrowed AST view
over exclusive and published storage, with mutation authority only in exclusive
builder/transaction paths.

Use move-and-return worker operations for an exclusive ParsedFile: move the
whole owner onto the worker, borrow its view there, then return the owner with
the successful result. The integrated parse/encode request can keep it on the
worker throughout. Do not send arbitrary borrowed views through a persistent
queue, force future binder payloads to become Sync, or publish early to obtain
a FileHandle. Panicking exclusive mutation discards that owner. Shared-file
requests use the explicit retained-file path below; same-worker operations run
directly. Validate these signatures and Send/lifetime bounds in the ownership
prototype before grammar porting.

**Lazy graphs are transactions.** Extend S04 staging to reserve and publish list
headers/edges together with lazy nodes under its existing single lock. Failed
attempts burn provisional identities, drop private staging and publish nothing.
Seed the eager JSDoc cache with validated core IDs before publication; the current
lazy-root-only API cannot represent that. Add lookup-only access for EagerJSDoc.
A transaction-local resolver may inspect staged and already published lazy data
without reacquiring its lock; core reads remain borrowed. Preserve the existing
reentry detector, contention behavior, unlock/rethrow policy and bundle retention.

AST defines the typed JSDoc-provider boundary and `tsr_parser` implements it.
Use an explicit provider/function entry point with no owning closure capturing
its own file; do not create an AST→parser crate dependency or global mutable hook.
Reparsed clones are core allocations made before publication, not lazy nodes.

Dispatch the complete lazy lookup/transaction to the parse worker **before**
acquiring the lazy lock. When already on that worker, execute directly instead
of synchronously queuing to itself. Initialization, the thread-local reentry
guard and publication stay on the same thread. An external request explicitly
retains its file/bundle while queued; a transaction must never send its borrowed
resolver or wait on another thread. Test external-thread requests, calls already
on the worker, prohibited same-file nested initialization, and recovery after
an initializer panic. This retains S04's publication protocol; it does not move
staging outside the lock or solve arbitrary cross-file wait cycles.

Review costs separately: core-node reads borrow without locks or owner clones;
core construction and list edge storage grow in owner-owned buffers; publication
transfers ownership once. Lazy lookup still locks and retains its stable page;
off-worker requests also retain their file/bundle while queued. This does not
establish per-node allocation or memory performance. Rerun E3 soundness, release
import and retention checks after storage changes, using actual AST/list
payloads as well as existing leaf fixtures.

## 4. Scanner integration without self-references

An outer parse operation owns `SourceText`. The scanner borrows it while the
builder receives one shared source clone. Parser state must not contain a
scanner borrowing another field of the same movable parser/builder object.

Distinguish file bytes from already loaded parser text. Current
`SourceText::from_bytes` performs file decoding; ParseSourceFile itself accepts
the supplied arbitrary Go string unchanged. Add an explicit already-loaded-byte
constructor that preserves all bytes, including a leading BOM or `FF FE`.
Requests name `file_bytes` versus `parser_text`; never run final Go-produced
parser text through file decoding again. The manifest names any earlier physical
and virtual-file loading stages separately. Freeze one-BOM/two-BOM, malformed
UTF-8, UTF-16 endianness and odd trailing-byte witnesses at the correct entry
point, retaining both raw and parser-text hashes.

Go assigns `p.scanError` as the callback of its own scanner field. Use an opt-in
scanner diagnostic buffer and one private parser scanner-operation wrapper:
execute the operation, drain errors into disjoint parser state, then return.
`scanError` only updates diagnostics and `hasParseError`; it does not influence
scanner control, so this preserves parser-observable timing. Drain before any
mark, rewind, node finish or parser error handling. The wrapper must not
implicitly overwrite `Parser.token`: several rescans deliberately handle their
return separately. Keep S05 callback behavior unchanged outside buffered mode.
Allocate diagnostic storage only when errors occur; do not add Arc/Mutex to the
normal token path. Pending buffered diagnostics are never checkpoint state.

The parser often saves a token value, advances, then constructs its node. Add
an explicit retained token-value view: borrowed source bytes or shared cooked
storage. Ordinary `token_value(&self)` cannot survive mutable advancement.
Construct AST JsString values at that ownership boundary, preserving raw bytes.
S03's JsString payload choice entails source Arc clones and slice classification
for stored strings; document that cost rather than claiming zero per-node
atomics/scans. Share cooked allocations where possible instead of forcing a copy.

Reuse the pinned spelling helper currently private in `tsr_scanner`; expose or
relocate it through a dependency-safe seam, without a core↔scanner cycle.
Complete node-aware scanner helpers needed by parser diagnostics through the
AST view interface, including missing nodes, trivia/JSDoc handling and reparsed
literal quoting. Do not duplicate S04 position algorithms.

## 5. AST runtime and authoritative generation

Extend the existing normalized schema adapter with resolver facts required for
runtime generation, including subtree-fact generation and factory aliases.
Use the pinned `schema.ts` resolver and `generate-go-ast.ts` rules. Do not build a
second ast.json interpreter or hand-edit S03 generated files. This runtime work
legitimately extends `cargo xtask gen`; scanner table generation stays separate.

Implement constructor masks, node/text counters, hook order, update comparisons,
kind aliases, child enumeration, visitor dispatch and subtree propagation.
Updates preserve identity when the Go comparison says unchanged; Rust structural
`PartialEq` does not encode that rule. Retain nullable children despite a schema
"required" property; DefaultClause's nil expression is a known discriminator.
Do not invent blanket required-child validation that rejects Go-valid factory
states. Keep ordinary optional accessors distinct from Go's assertion casts.

The generated inventory has 193 constructors, 166 updates, 191 clones, 232 kind
predicates, 167 child enumerators, 166 VisitEachChild methods, 38 subtree-fact
methods, 41 Name accessors and 192 assertion casts. The first eight categories
provide 1,194 candidate implementations, before handwritten runtime work.
Exclude SyntheticExpression's New, Update, VisitEachChild and Clone until its
checker type link has real ownership; 1,190 candidates remain. Its codec
rejection still needs coverage and does not justify a fake type-bearing factory.
Freeze an exact function-ID scope/omission list before adding provenance labels.
Generated Go methods use `upstream:` labels and do not contribute to the
tracker's handwritten source-function denominator. Source-kind AST utilities
and accessors must independently meet 420/839; generated runtime behavior still
requires authoritative regeneration and differential tests.

Runtime visitors must express deletion, SyntaxList flattening, hook ordering,
identity-preserving updates and single-result lifting assertions; the current
S03 ChildMapper cannot express all of these. `visitNodes` may forward nil list
elements while a singleton visit suppresses absence. Preserve that distinction.

DeepCloneNode assigns synthetic locations to cloned nodes/lists; its trailing
comma preservation sets the final child's range to `(-2,-2)`. DeepCloneReparse
preserves locations, fixes parents and marks the cloned root reparsed. Preserve
these operations separately. Parent fixing, clone traversal and subtree-fact
computation require stack-safe implementations, not just a protected parser.

Implement precedence and subtree exclusions from their pinned functions.
Composite subtree facts cache with an atomic Computed bit even before
publication; NodeDefault does not cache. `walkTreeForJSXTags` requests facts
during SourceFile completion, so exclusively owned ParsedFile views must support
the real query/cache path. Preserve fresh caches on new/clone allocations and
retained caches on identity-preserving updates. Upstream's mutable setters do
not reset facts: do not invent automatic invalidation, recomputation on every
query or a publication-time reset. Verify cache/query/mutation ordering against
Go explicitly. Publication must not eagerly compute unused derived fields.
SourceFile module indicators, references, imports/augmentations, pragmas and
diagnostics must use real runtime paths rather than encoder-only synthesized
answers.

## 6. Parser phases and state transitions

Port the public orchestration before filling grammar modules: initialization,
first token, JSON versus source-file path, source-file completion, reference
collection, eager JSDoc/reparse integration and parser release. Preserve missing
nodes/lists and recovery progress tests at each parsing context.

Implement expressions and ambiguity handling together: precedence/associativity,
assignment and conditional forms, `new`, optional chains, type arguments,
generic/async arrows, contextual keywords, `as`/`satisfies`, JSX ambiguities and
ASI. Then cover declarations/statements, type grammar, JSX and JSON with their
actual entry points. Do not use a generic combinator rollback rule for Go's
selective speculation or let an error production consume a different token.

A parser checkpoint contains the scanner checkpoint, context flags, selected
lengths (diagnostics, JS diagnostics, JSDoc infos, reparsed clones),
statementHasAwaitIdentifier and hasParseError. Preserve the exact source save
set. In particular, rewinding does not discard factory allocations/counters,
identifierCount, sourceFlags, notParenthesizedArrow or all JSDoc diagnostics.
Top-level-await reparsing changes the saved diagnostic length to preserve its
new errors. Failed speculation must not reuse IDs or truncate the node arena.

S05 checkpoints stay consuming, non-Clone and LIFO. Every successful speculation
must explicitly commit where Go simply discards its saved value. Audit keyword
and contextual-modifier probes, arrow/type-argument paths and JSDoc loops as well
as ordinary lookahead. On panic, discard the parser/builder and any scanner with
abandoned checkpoints; never put it back into a reuse pool.

`finishNodeWithEnd` sets ranges, applies context flags, consumes pending parse
error state and overwrites immediate children's parents. These transitions are
observable through counters, parents and flags even when the binary tree shape
looks correct. Preserve JS-only diagnostic collection and error deduplication
by its actual source rule, not sorting by location.

The parser calls slash rescan with the omitted reporting argument, so it does
not perform S05's full regexp grammar validation. Keep that parser behavior;
S05's explicit true/false regexp evidence remains independently required.

## 7. JSDoc and reparsing

JS/JSX comments populate eager JSDoc cache entries with core IDs. Other script
kinds set HasJSDoc and perform the cheap deprecated-tag check, deferring normal
JSDoc until first request but eagerly parsing `@see`/`@link` comments. This branch
is broader than TypeScript alone. JSON has the JavaScriptFile context flag while
`isJavaScript` is false; preserve the separate tests controlling eager parsing
and diagnostic transfer. Freeze trigger distinctions, parsed flags, cache
identity and duplicate/error behavior.

Implement comment text/link/name/type parsing, tags and their recovery, overload/
callback/typedef/property handling, parent overrides and the real JSDoc-to-TS
reparser. Preserve scanner state around text replacement: restore original text
and selected state in the source order, with explicit checkpoint commits.
`unicode.IsSpace`, scanner whitespace and JavaScript whitespace are different
predicates at different trim sites; retain pinned rune decoding and casing.

Reparse functions may mutate host nodes before publication. Deep-cloned reparsed
nodes keep source ranges, repaired parents and the Reparsed flag; the final
SourceFile sorts reparsed clones by the pinned position comparator. Requests
for tokens under reparsed parents retain the upstream rejection. Do not model
reparsed clones as an independent file or let lazy cache publication own them.

## 8. Protocol-8 encoder and decoder

Use current S03 layouts as inputs, adding real runtime dispatch and special
codecs. Compare against executable Go, including places where its comments are
inaccurate: protocol version 8 is written into byte 3 via `8 << 24`, not byte 0.

Encoding covers SourceFile and subtree entry points, nil node-table sentinel,
list pseudo-nodes, exact visitor order, sibling/parent indexes, child masks,
common-data bits, extended template/SourceFile data, structured tuples and
node-index lookup/cache behavior. List pseudo-nodes are wire parents; do not
confuse them with AST parent identity. Indexes must remain independent of arena
allocation order, including discarded speculative allocations.
Exercise BuildNodeIndexTable before encoding as well as encoding's table path;
freeze GetIndex absent/present results, lazy-JSDoc additions and cache behavior.
Equal bytes alone cannot prove that separately returned lookup tables work.

Port string-table behavior exactly: source text prefixes the string-data region,
matching source slices are reused, and other strings append in traversal order
**without deduplication**. Each add appends an offset pair, returning its word
index (0, 2, 4, …), not a string ordinal. Preserve the SourceFile special case,
raw/WTF-8 bytes and the source-slice bounds behavior. Use the S04 API PositionMap
for node/list/reference and span coordinates. Compare all output bytes, not just
a decoded semantic tree.

Port the small protocol-specific MessagePack writers directly; a general serde
codec is not authority for tag widths, tuple order or empty-data sentinels.
Implement span-map and diagnostic-directive normalization/serialization used by
these codecs, with canonical/supplemental multi-file ownership fixtures. Full
mapper plugin execution remains later work.
Header/node words are little-endian; MessagePack lengths and integer payloads
use their specified big-endian representation. Regions need not be aligned.
Keep nil SpanMap's `0xffffffff` sentinel distinct from a present empty map's
encoded empty array. The SourceFile extension has 19 words (76 bytes), including
fields patched after traversal. Exercise all nine extended layouts and the
SyntheticExpression rejection separately. The encoder rejects solely by kind
before payload access: a named raw/schema rejection fixture suffices in Rust,
while Go permits its factory's nil type for this fixture. Decoder rejection also
precedes construction. Freeze their exact `SyntheticExpression should never be
encoded` / `SyntheticExpression should never be decoded` messages; neither
fixture earns type-bearing constructor/update/clone credit.

SourceFile hash is supplied state. Direct ParseSourceFile does not compute it;
that oracle's header is zero unless a named setup stage explicitly supplies a
hash. Preserve Hi/Lo ordering and SourceFileHash formatting with injected-value
fixtures. Do not add a hashing dependency merely because the protocol describes
xxh3; host hash computation is a distinct later operation.

The pinned encoder currently always returns nil in its error return on its
normal encodeTree path. Invalid structures can panic; malformed source bytes
can successfully encode. Record observed outcomes and stage, and do not invent
an encoder-error witness simply to make a success/error metric look balanced.

Decoder implementation is required and independently tested. Go decoding is
lossy: most SourceFile metadata and NodeList trailing-comma flags are not
restored, and wire positions are assigned directly through signed TextRange.
Do not require decode→encode to equal the original bytes. Compare Go-decoded
observations and, where useful, Go's own roundtrip output.

Recognize exact raw `0xffffffff` as the list sentinel first, then apply Go's
signed-int16 Kind narrowing for dispatch **and both diagnostic wrappers**.
`0x10000` can alias KindUnknown; `0xffff` is Kind(-1), not a list. Preserve known
KindCount and unknown-kind formatting without assuming the Rust enum contains
every raw kind. For example, an unsupported narrowed kind reports
`at node 1 (kind Kind(-1)): unhandled node kind Kind(-1) with 0 children`.
Reserved data-type bits use Go's default children branch. Do not impose extra
alignment, minimum-region-offset or trailing-byte rules on Go-accepted input.

Valid encoding does not imply successful Go decoding. The generated SyntaxList
and JSDocTypeLiteral branches index a zero-length slice allocated with capacity;
any child triggers a bounds panic. Freeze these as recognized expected-panic
witnesses, preserving the pinned behavior rather than silently fixing or
dropping it. A root list sentinel can instead make DecodeNodes return `(nil,
nil)`; DecodeSourceFile then dereferences the nil root. Record entry point and
stage so these outcomes cannot become interchangeable errors.
For the paired nil-root request, compare a narrow, entry-point-specific
`nil_root` class against a named Rust contract panic, retaining Go's raw nil
pointer payload. Require the same wire to produce DecodeNodes' successful null
root first. An unrelated unwrap/assertion, bounds error or wrong-stage panic
must fail this classifier.

Preflight every required decoder fixture through Go before freezing it: it must
return success, an error or a recognized panic. Any timeout during required
capture invalidates that capture; it is never an expected matching error or a
row to drop. Known cyclic sibling links have a separate frozen subprocess
watchdog. The production decoder preserves Go's sibling traversal, including
nontermination for this malformed cycle. Each adapter must emit a valid begin
frame before the watchdog starts; an early exit, returned value or malformed
stream cannot count as the observed timeout. This measurement contributes no
primary or returned-decoder parity rows. The proposed finite Rust rejection was
not adopted, so this behavior needs no divergence entry. Any later intentional
compatibility exception still requires the owner's explicit ADR 0004 approval.

## 9. Frozen corpus, options and oracle

Use a clean export of the pin with access-only bridges, local pinned Go,
GOTOOLCHAIN=local, readonly modules and reusable caches. S03's patched tooling
worktree is not the behavioral oracle. Correct the stale sprint README sentence
that currently says later token/encoder tools use that worktree.
Export only the measured source/resource closure, handle archive errors as
producer diagnostics, and build with `-trimpath`. Read Go/MSRV/instrumentation
pins from `data/s04/toolchains.toml` and retain the existing stable/generator pin
authorities; preserve caller Cargo/Rustup homes and registry/offline
configuration. Reuse the existing strict-JSON and subprocess helpers where their
contracts fit, declaring those helpers as evidence inputs. Keep reusable Go,
generator and instrumentation state outside Cargo's disposable target cache.

Read the pinned Git tree rather than filesystem glob drift. The 12,721 physical
compiler/conformance sources contain two JS inputs; adding the 25 transpile
fixtures gives S05's 12,746. The compiler runner's narrower extension selection
and 42 explicit skips are not E1's denominator. Retain those 42 physical cases;
do not inherit compiler/baseline skips into a direct parser comparison.

Run the actual `ParseTestFilesAndSymlinks` and configuration-expansion helpers
through Go bridges; do not transcribe their regex/newline rules into Python.
Physical compiler fixtures first pass through pinned osvfs/Common.ReadFile
decoding, before compiler-setting extraction and virtual-unit splitting. That
loader consumes a UTF-16 BOM with its odd-byte rule, strips one UTF-8 BOM, and
otherwise preserves arbitrary bytes. Splitting also has its own directive,
newline and leading-blank rules. Ordinary virtual files then pass through the
test VFS's Common.ReadFile decoding before reaching the cached compiler host;
initial tsconfig parsing instead receives extracted text directly. Freeze the
actual route and physical-raw, physical-loaded, extracted-unit and final
parser-text hashes. Preserve those upstream loading stages in fixture production
without adding a decoder inside ParseSourceFile or on final Rust adapter input.
Canonicalize configuration IDs by sorted option-key/value pairs because Go map
iteration can reorder otherwise identical variants. Preserve the distinction
between raw directive settings and effective parse options, including the
separate trailing-semicolon behavior of compiler-setting extraction.
An empty configuration expansion means one default configuration, not zero
executions. A Go preprocessing fatal/error/panic invalidates preflight rather
than importing the compiler runner's skip policy.

The first preflight found a precise legacy exception to configuration expansion:
seven retained physical cases specify `module=none`, which the pinned option
map rejects even though `ModuleKindNone` still exists. Keep the raw setting and
the rejected helper outcome. For exactly the seven paths recorded in
`data/s06/corpus.json`, expand the remaining valid settings through the unchanged
helper, then use the real error-bearing `ParseCommandLine` result to obtain the
parser-only options, retaining diagnostic 6046. This input projection was chosen
before any Rust parse results. It is neither a claim of compiler-option parity
nor an E2 baseline exception. An eighth path or a changed raw value fails
preflight; the producer tests both rejection paths.

Initial read-only inspection expanded all physical cases into 17,264 virtual
units / 7,528,194 bytes before option expansion: 14,746 TS, 1,360 JS, 641 JSON,
472 TSX, 16 JSX and 29 Unknown-kind assets. There are 2,078 multiunit cases,
25 cases with symlinks and 50 explicit working directories. The implementation
freeze reproduced those totals from two clean Go exports. Its 15,206 option
variants produce 22,343 primary parser requests: 22,075 virtual-file loads,
160 direct initial-config parses and 108 embedded libraries. The frozen request
digest is `30e50b547b79c676de2870b08ce1b48f2d810b95badda54f205945af1bb450a1`.
These are corpus observations, not Rust parity results.
An independent manifest audit must assert those pre-option-expansion extracted
totals (17,264 units and 7,528,194 bytes), their ScriptKind breakdown and the
12,829 primary rows. Keep this check distinct from merely rerunning the same
bridge. Review the complete manifest diff, inspect named multiunit/config/BOM/
symlink/ancillary examples against pinned sources, and compare each recorded
loading hash stage. A discrepancy requires an explained source-derived change
to the planning expectation; it must not silently reset the expected totals.

Freeze the 108 libraries from the exact bundled LibNames list, one whole-file TS
request each at `bundled:///libs/<name>`, with false/false declaration module
options and the direct-parser zero hash. Default Go loading returns embedded
string bytes directly; it does not call the file decoder or the test-directive
splitter. These 108 files currently total 3,785,075 bytes and contain valid UTF-8
without BOMs or CR line endings; that observation does not test loader edge
cases. The disk/noembed loader and additional `@libFiles` test libraries are
separate named routes, not substitutes for the bundled denominator.

Freeze every physical case, virtual unit, raw/decoded hash, filename/path,
configuration variant, script kind, effective JSX/Force options, package-format
metadata inputs and eligibility reason. Auxiliary JSON participates through the
JSON parser where eligible. Retain the 29 assets in the manifest: 21 mapper-native
inputs and eight ancillary files are not silently coerced into TypeScript.
No physical case is currently empty after direct-parser selection. Record that
mapped-source transformation and external test scripts are not executed by this
parser-only corpus; explicit encoder metadata fixtures cover the required slice.
Do not describe this as running the full upstream compiler tests.

Use pinned option/path helpers to produce the fixture's effective parse inputs.
The harness passes already resolved ParseSourceFile inputs and records that
they are Go-produced fixture data, not Rust compiler-option parity. Port the
SourceFile-option and path operations required by the Rust AST/parser itself;
do not expand S06 into a raw compiler-configuration API.
Never infer package metadata from the developer's filesystem. Freeze virtual
cwd, path normalization and case sensitivity; parameterize explicit cross-host
path fixtures so macOS/Linux cannot change the denominator.

One tracker row represents each of 12,721 physical cases or 108 lib files:
12,829 primary rows. A case passes only if all its eligible units and frozen
option variants pass. Expanded units/options are reported as separate counts,
not extra favorable denominator rows. Every case needs a nonempty obligation;
unknown/missing/duplicate/skipped rows fail the frozen-denominator check.
Supplemental malformed/owner/codec/depth fixtures form a separate exact suite
and must never inflate primary E1 parity. Freeze actual request hashes and
subgroup membership, not only scenario names.

Planned artifacts are `data/s06/corpus.json`, `cases.json`, `requests.json`,
`fixtures.json`, `probes.json` and an exact function-scope list. A deliberate
`python3 scripts/s06.py freeze --write-manifest` records reviewed expansion;
verification reconstructs and checks it without writing. The registered
`cargo xtask run e1` is added only once the full harness exists.
Declare its actual production dependency closure, generator outputs, oracle
helpers, manifests, pins and workspace build configuration as evidence inputs.
Avoid a blanket `crates/**` source glob; an unrelated later crate must not force
this corpus through recapture. Test both a relevant-input invalidation and an
unrelated edit, alongside fresh-worktree prerequisites and cached reruns.

## 10. Observations, metrics and failure paths

One persistent Go process and one Rust adapter execute bounded requests through
production code. Use strict versioned framing with case/unit/config IDs, action
ordinals, begin/end counts and explicit parse/encode/decode stages. Reject
unknown fields, duplicate keys, nonfinite numbers, malformed hex, missing or
extra records and mismatched counts before comparing behavior. Bound request,
response and memory sizes from inspected largest workloads; freeze reviewed
limits with the protocol rather than inheriting S05's token-stream sizes blindly.
Use absolute I/O deadlines, drain stderr, serialize shared output paths and keep
complete mismatch/timeout context in CI artifacts.

For integrated cases observe parse completion, encoded bytes, node-index order,
selected AST metadata/counters and diagnostics as separately labeled data.
Binary equality is the E1 obligation. Codec-specific constructed AST actions
exercise fields absent from ordinary parsed files without replacing corpus
coverage. Missing parse output is not an encoder error. Unknown panics or absent
encoder observations cannot satisfy exact encoder metrics. Retain raw payloads;
compare stable contract messages and narrowly recognized bounds classes.

Go's parser diagnostics have a confirmed unordered-map source: keyword split
suggestions use the first prefix in a map-derived list. Across 40 fresh Go
processes, `constructorabcdefghij x;` reported code 1435 with either
`const ructorabcdefghij` (21) or `constructor abcdefghij` (19);
`typeofabcdefghijklmno x;` produced `type ofabcdefghijklmno` (25) or
`typeof abcdefghijklmno` (15). Do not sort the oracle or claim universal diagnostic
argument-byte parity. Stable diagnostic fixtures compare exactly. Named
nondeterminism fixtures compare stable code/range fields and report raw argument
variants, explicitly outside a diagnostic-byte claim. Retain S05's deterministic
lexically ordered Rust candidates: these choose the const/type split, each an
observed Go outcome. Variant membership must never be labeled byte parity.
In a separate 40-process run, the full 426-byte encoder buffers were identical
for each of these witnesses despite varying arguments. Diagnostic message text
is not serialized into protocol 8. Keep later diagnostic evidence unknown;
this plan introduces no baseline rewrite or divergence allowlist. Any later
intentional E2 baseline difference still follows ADR 0004.

Use one e1 producer with derived per-case rows and measured supplemental results:

| Metric | Non-vacuous obligation and consumer |
| --- | --- |
| `run.e1.parity` | Tracker derives the fraction of successful primary rows; E1's existing threshold remains 0.999 |
| `run.e1.frozen_denominator` | Exact primary IDs, units/options, library list and hashes match; E1's denominator criterion |
| `run.e1.encoder_success_error` | Exact staged encoder outcomes across the integrated corpus and encoder fixtures; E4's encoder outcome criterion |
| `run.e1.encoder_output_bytes` | Every successful Go encoding has matching Rust bytes, with no missing corresponding output; E4's encoder byte criterion |
| `run.e1.decoder_parity` | All frozen decoder fixtures match Go success/error/recognized-panic outcomes and decoded observations; add an explicit S06 gate and both decoder ledger consumers |
| `run.e1.ast_runtime` | Exact factory/list/visitor/clone/JSDoc/metadata observations in the frozen nonempty runtime suite; require in S06 and relevant AST runtime ledger entries |
| `run.e1.ast_utilities` | Exact named utility/accessor test inventory, rebuilt Rust tests and independently regenerated Go expectations; source-function coverage has its own behavioral gate |
| `run.e1.depth` | The named production entry points complete their frozen stress cases and unwind tests; require in S06, without claiming later full-pipeline recursion coverage |

Emit each supplemental result as a boolean with companion request/observation
counts and failed-ID records; gate it with `== true` and require a nonempty suite.
Missing instrumentation or malformed output is unavailable, not a passing
boolean. A measured mismatch is false and retains its scenario diagnosis.
Rust-only ownership/ingress safety cases remain separately labeled checks.
Require the three supplemental metrics in S06's exit and the corresponding
required items; fixtures never enter the 12,829-row primary test map. This is a
planned strengthening of missing acceptance coverage, not evidence produced by
this documentation PR.

Route only the two S06 E4 encoder criteria to `run.e1.encoder_*`, with their full
integrated-corpus scope. This avoids a second expensive parse and keeps S04's
text producer inputs narrow. Update S06's producer comment and the three encoder
output consumers in PORTS.toml; preserve stringtable's still-pending
`run.e4.token_literal_bytes` and AST diagnostic's `run.e4.diagnostics` obligations.
The current decoder ledger only consumes E1 parser parity: its new dedicated
metric must prevent a broken decoder from passing while parser encoding agrees.
Audit all consumers and add regressions proving full E3/E4, literal types/printing
and later sprints remain incomplete. Do not change S05's scanner-diagnostic
authority or manufacture the future diagnostic producer from a parser subset.

Required supplemental witnesses include nil/empty/missing lists, update identity,
factory hook/count changes, visitor deletion/flattening, parent overrides,
synthetic/reparse clone locations, top-level-await rewind, eager/lazy JSDoc,
module indicators/references, malformed bytes/BOMs/surrogates, UTF-16 partial
positions, template text/raw flags, SourceFile structured metadata, decoder
returned errors and classified panics. Include every pinned parser fuzz seed
and relevant upstream parser/encoder/decoder regression as named reviewed input.
A vacuous witness group fails even if both sides omit the operation.

Freeze at least these named counterexample families; expand individual requests
before implementation rather than treating one representative as full coverage:

| Fixture family | Failure it must expose |
| --- | --- |
| `parse-options-open-kind` | Zero-kind panic versus accepted unknown nonzero kinds; JS/JSON variant and JSX/Force module indicators |
| `source-loading-boundary` | A second BOM decode of virtual-unit text or loss of malformed bytes passed directly to ParseSourceFile |
| `speculation-state-and-cost` | Successful checkpoints left uncommitted; failed lookahead wrongly rolling back counts/allocations or keeping errors |
| `await-reparse-spans` | Adjacent/overlapping reparse spans, changed diagnostic length and inserted JSDoc declarations shifting statements |
| `recovery-list-context` | Absent/empty/missing confusion, zero-width nodes, unexpected separators and failure to make source-defined recovery progress |
| `jsx-parent-repair` | A child's recovery consuming its parent's closing tag, adjacent roots and repaired parents differing despite plausible text |
| `jsdoc-lifecycle-worker` | Eager versus deferred roots, nested tag/error behavior, self-queued worker deadlock and lazy publication after panic |
| `pragma-reference-metadata` | Duplicate triple-slash precedence, last check/nocheck directive, casing, ambient-relative filtering and node: module indicators |
| `factory-list-identity` | Structural equality replacing update identity; nil elements filtered; clones reusing headers or losing trailing-comma ranges |
| `subtree-facts-lifecycle` | Cached facts silently invalidated by mutation, reset on publication or copied into a fresh clone |
| `codec-source-string-offsets` | Accidental deduplication, ordinal indexes, source-slice reuse/bounds and wrong UTF-16 conversion |
| `codec-sourcefile-structured` | Endian/tag-width drift, nil/empty SpanMap, supplied hash ordering and post-traversal patch offsets |
| `decode-open-wire` | Kind narrowing/KindCount, reserved tags, nil root and lossy ranges/metadata treated as canonical roundtrip semantics |
| `decode-generated-list-panic` | SyntaxList/JSDocTypeLiteral children silently repaired or a different Rust panic accepted |
| `decode-cyclic-watchdog` | Preserve the source loop; separately measure both live subprocesses at the frozen deadline, with no returned-error or primary parity credit |
| `keyword-split-nondeterminism` | Unordered diagnostic arguments silently normalized or counted as exact diagnostic bytes |
| `producer-rejected-capture` | Duplicate/reordered/missing request, wrong panic class, one failed stage, timeout or partial metrics recorded as success |

Retain upstream's five named parser regressions covering heritage kinds, JSDoc
import parents, source survival/propagation through reparse and non-ASCII API
positions, plus its ten fuzz-crash seeds. The frozen manifest names every input;
an aggregate "upstream regressions" flag does not establish their execution.
For subtree caching, preflight a computed property containing `this`, query its
facts, replace its expression with an identifier, query again, then clone/query.
Freeze Go's actual results before asserting them in Rust; this sequence separates
retained caches from fresh clone computation.

## 11. Recursion and operation failure

Use a production native parse worker with a named 256 MiB reserved stack,
consistent with ADR 0011, and reuse it for the adapter's case sequence. Keep its
queue bounded and owning inputs explicit; do not create one thread per recursive
operation or cache a parser after panic. Parser APIs used on an existing worker
must document that boundary rather than silently depending on a test stack.
Catch request unwinds at the worker operation boundary, retain the panic payload
and discard the complete Parser/Builder/scanner state. Test a following
independent request on that worker; queue/worker failure is a distinct operation
failure, not a parser diagnostic or compatible encoder error. Use the same
worker boundary for synchronous AST/codec passes; a lazy transaction follows
the before-lock dispatch rule in section 3.
Growth guards (or equivalent order-preserving iterative traversal) provide the
required protection on unbounded paths; the reserved stack is the default
execution container. A test passing solely because its stack is large is
insufficient. Forced-growth tests start on modest stacks and observe actual
guard entry/growth, independently of the default worker stress run.

Add S05-style production growth guards to every unbounded parser cycle, not
only parseExpression/parseType. A conservative source/callback graph identified
these initial guard candidates: parseStatement, parseType,
parseAssignmentExpressionOrHigherWorker, parseBinaryExpressionOrHigher,
parseSimpleUnaryExpression, parseJsxElementOrSelfClosingElementOrFragment,
parseMemberExpressionOrHigher, parseModuleOrNamespaceDeclaration,
parseTypeOperatorOrHigher, parseJSDocType, parseJSDocTypeNameWithNamespace,
reparseJSDocTypeLiteral, wrapInJSDocNamespace,
convertEntityNameExpressionToEntityName, validateJsonValue, isStartOfType,
parseNestedTypeLiteral, parseTag, parseIdentifierOrPatternWithDiagnostic and
parseNewExpressionOrNewDotTarget. This lexical graph is planning evidence,
not a verified call graph: audit indirect callbacks and generated traversal
before treating this as sufficient. Document bounded cycles separately.

Also protect AST parent fixing, cloning, subtree computation, SourceFile searches,
encoder walks and decoder graph processing. Prefer explicit worklists where they
preserve order and callback behavior simply; otherwise guard recursion. Test
left/right binary chains, unary/new nesting, parentheses, qualified namespaces,
lookahead-only type nesting, destructuring, JSX, conditional/JSDoc types and
malformed JSON/recovery. Execute from modest stacks to force actual growth,
then test the normal worker path and callback unwind. Existing stacker/psm
native dependencies must compile and execute on all four targets. This does not
establish the separate WebAssembly strategy or future full-compiler depth gates.

## 12. Ordered implementation and review checkpoints

1. Freeze the function scope, corpus/options contract, protocol and known Go
   outcomes; resolve nondeterminism and decoder-termination classifications.
   Review these independently before any Rust output selects eligibility.
2. Prototype the authoritative node header, list identity/retention, exclusive
   ParsedFile/publication lifecycle and lazy transaction extensions. Prove them
   with real AST/list ownership tests and rerun S04 E3 before building on them.
3. Extend authoritative runtime generation; implement factories, updates,
   visitors/cloning/subtree facts and the minimum SourceFile/options/path slice.
   Keep generated TypeScript/client drift checks intact. Review actual marker
   credit and factory identity/counter witnesses before grammar work expands.
4. Add the scanner buffering/retention seams and parse worker; implement parser
   state, missing nodes/lists, diagnostics and the public entry points. Rerun S05
   unchanged corpus evidence after scanner/runtime dependency edits.
5. Port grammar in coherent vertical slices, then JSDoc/reparser/reference
   completion. Compare small named Go cases continuously; retain every discovered
   mismatch as a reproducer without weakening the full corpus contract.
6. Implement encoder/string/structured/span data and decoder over the real AST.
   Exercise special codecs and malformed input independently, then run the full
   grouped corpus and all exact supplemental suites.
7. Review bytes/arithmetic/recovery, ownership/costs, recursive cycles and the
   harness/consumer failure paths independently. Fix valid findings; audit
   source facts that diagrams or conventional TypeScript assumptions obscure.
8. Run workspace and relevant debug/release tests, formatting, strict Clippy,
   dependency policy, declared MSRV, generated-client drift, S04 E3/E4, S05 and
   the new E1 producer. Regenerate views only from current evidence; enforce
   S01–S06 plus exact supplemental metrics and retained later-sprint gates.
9. Execute those contracts on macOS arm64/x64 and Linux arm64/x64. Preserve
   independent captures after failures and upload raw encoder/parser logs.
   Inspect measured CI step durations and cache restoration before attributing
   cost or optimizing the evidence scope. Report complete results and limits.

Each checkpoint has an independently reviewable artifact and an explicit exit.
The ownership foundation is reviewed and its real-payload E3 results are current
before grammar consumes that API. Corpus preparation and generator source
analysis can proceed independently; neither may choose inputs from Rust results.

| Checkpoint | Exit evidence |
| --- | --- |
| 1: scope and corpus | Exact function scope and request manifests reviewed; independent count/hash-stage audit passes; each selected Go preflight outcome classified |
| 2: ownership | Documented Send/lifetime signatures compile; release imports, retained lists/text and eager/lazy publication witnesses pass; E3 passes on production storage with real AST/list payloads |
| 3: AST generation/runtime | Authoritative generator drift is clean; real factory/update/visitor/clone/subtree observations pass; exact implemented-ID audit meets its declared scope |
| 4: scanner/parser boundary | Buffered and callback observations agree; retained source/cooked values survive advancement; raw parser-text loading and worker panic recovery pass; S05 evidence remains current |
| 5: grammar/JSDoc | Every in-scope grammar path has its reviewed mapping; frozen recovery, speculation, JSX/JSDoc and metadata fixtures match Go with the stated diagnostic qualification |
| 6: codec/integrated corpus | All 12,829 primary rows captured without skips; exact encoder and supplemental decoder/runtime suites pass; cyclic timeouts cannot masquerade as parity |
| 7: adversarial review | Valid review findings resolved; injected wrong stage/class, missing/duplicate request and partial metric fail the intended consumer |
| 8: local acceptance | Required debug/release, generation, lint, policy, MSRV and instrumented runs pass; S01–S06 pass from current evidence while later incomplete scopes remain incomplete |
| 9: native acceptance | All four native targets execute the required contracts; raw artifacts and effective toolchains are available; any unavailable target remains explicitly unverified |

Tracker acceptance must also be concrete before parity is recorded: route the
two encoder metrics, require decoder/runtime/depth results and execute the
non-closure regressions while those producers are still absent. No absent
metric is a pass, and no registry placeholder stands in for the later harness.

## 13. Review record

Reviewed on 7 September 2026 against the pinned source, the current S03–S05
APIs, accepted ownership/text contracts and the personal coding guide. Three
independent reviews covered AST/storage/generation, parser/JSDoc/recursion, and
corpus/codec/evidence; they then challenged the written draft. The author checked
the findings, corrected the plan and requested a second read of the ownership
and acceptance changes. This is plan review, not validation of an implementation.

| Finding | Disposition in this plan |
| --- | --- |
| Encoding alone cannot meet the encoder-package coverage threshold | Include 29 handwritten decoder functions alongside 38 handwritten encoder/string-table functions; the eight generated functions have separate provenance; require separate decoder observations |
| Existing AST and arena headers duplicate authority; list ranges lose Go identity | One AST header through an arena-defined interface; owned list headers and shared node/text slices with nil/empty/missing distinctions |
| Encoder-triggered JSDoc needs lazy storage before shared publication | ParsedFile owns that storage from construction; publication transfers its cache/IDs; whole-owner worker operations preserve the exclusive lifecycle |
| Dispatching lazy parsing after taking its lock can deadlock or move thread-bound state | Dispatch before lookup/transaction; run directly on the existing worker; retain the transaction and reentry guard on one thread |
| Immutable-only subtree caching contradicts parse-time queries and mutable setters | Preserve Go's computed-bit lifecycle, retained update cache and fresh clone cache; add a query→mutate→query→clone witness |
| SyntheticExpression deferral was undercounted | Exclude its New/Update/VisitEachChild/Clone; 1,190 generated candidates remain; codec rejection still executes |
| Parser assumptions could change ScriptKind, scanner callbacks or rollback | Open signed ScriptKind, explicit zero panic, buffered diagnostics drained at source boundaries, exact saved fields and consuming checkpoint commits |
| Deferred JSDoc was described too narrowly as TypeScript-only | Distinguish JS/JSX from every other kind and keep JSON context-flag checks separate |
| String table and decoder formatting were misdescribed in the draft | No deduplication; word-offset indexes; signed Kind narrowing in both error layers; exact mixed-endian structured layouts |
| Valid encoded nodes are not always Go-decodable | Freeze SyntaxList/JSDocTypeLiteral bounds panics, nil-root paired entry points and lossy decode observations; no assumed canonical roundtrip |
| Cyclic wire inputs can make Go nonterminate | Timeout invalidates required capture; the separate watchdog measures preserved Go/Rust nontermination, with no parity-row contribution |
| Corpus count and compiler skips could silently change E1 | 12,721 physical cases plus 108 bundled libraries; all eligible units/configurations determine each row; retain ancillary assets and reasons |
| File decoding is not the parser-text boundary | Preserve physical and virtual loader stages, direct tsconfig text and embedded-library bytes; add a no-decode SourceText constructor for final parser text |
| Exact diagnostic arguments are not deterministic upstream | Two keyword-split witnesses varied over fresh processes while their encoder buffers stayed identical; qualify diagnostics without weakening encoder bytes |
| Parser encoding alone would not gate runtime/decoder/depth regressions | Name separate required supplemental metrics and ledger consumers; keep primary rows and full later-sprint claims unchanged |
| A large test stack would hide missing production guards | Reserved production worker, initial 20-method cycle audit, AST/codec traversal audit and modest-stack forced-growth tests on all four native targets |

The most consequential source anchors are the pinned
[parser state and entry points](https://github.com/microsoft/TypeScript/blob/1f70213d4922b434345f639b441681e470c7cfc1/tsc/internal/parser/parser.go),
[AST ownership and subtree cache](https://github.com/microsoft/TypeScript/blob/1f70213d4922b434345f639b441681e470c7cfc1/tsc/internal/ast/ast.go),
[generated factory behavior](https://github.com/microsoft/TypeScript/blob/1f70213d4922b434345f639b441681e470c7cfc1/tsc/internal/ast/ast_generated.go),
[string table](https://github.com/microsoft/TypeScript/blob/1f70213d4922b434345f639b441681e470c7cfc1/tsc/internal/api/encoder/stringtable.go),
[decoder runtime](https://github.com/microsoft/TypeScript/blob/1f70213d4922b434345f639b441681e470c7cfc1/tsc/internal/api/encoder/decoder.go),
[generated decoder](https://github.com/microsoft/TypeScript/blob/1f70213d4922b434345f639b441681e470c7cfc1/tsc/internal/api/encoder/decoder_generated.go),
[test configuration parsing](https://github.com/microsoft/TypeScript/blob/1f70213d4922b434345f639b441681e470c7cfc1/tsc/internal/testrunner/test_case_parser.go),
[virtual-file harness](https://github.com/microsoft/TypeScript/blob/1f70213d4922b434345f639b441681e470c7cfc1/tsc/internal/testutil/harnessutil/harnessutil.go)
and [embedded library loader](https://github.com/microsoft/TypeScript/blob/1f70213d4922b434345f639b441681e470c7cfc1/tsc/internal/bundled/embed.go).

Implementation still must freeze the concrete requests and resource limits,
prove the ownership prototype's Send/lifetime/publication signatures, document the
cyclic-input boundary decision, and execute the proposed parity/instrumentation
suites. No manifest, metric, mapping, dependency approval or four-target result
is implied by this plan. The 20-method recursion list remains a starting audit,
and planning probes are not committed sprint evidence.

### PR review follow-up

Verified the [external plan review](https://github.com/iantocristian/ts-rust/pull/10#issuecomment-5574904712)
against the repository on 7 September 2026. The source-behavior checks agree with
the preceding review. The inventory totals quoted by both reviews count all Go
file kinds; treating those totals as tracker denominators was incorrect. The
tracker has always excluded generated Go functions. Implementation validation
exposed this mistake: acceptance is 463/514 parser, 61/67 encoder and 420/839 AST
source functions, with generated provenance tracked separately. The existing
ratios and tracker policy are unchanged. The missing per-checkpoint exits, independent manifest
audit, explicit recursion primary and named divergence procedure are valid
planning gaps and are corrected above. The dependency on the open #9 and its
scanner follow-up seams are now explicit.

The ownership change is a substantial prerequisite with its own review and E3
exit; grammar must not build on an unverified storage design. S06 remains the
existing sprint scope, with bounded review artifacts rather than an assumption
that one aggregate parity run validates every component. The review's claim
that decoder.go has an empty `verify` is incorrect: it currently requires
`run.e1.parity >= 0.999`. The actual gap is the missing decoder-specific result,
which the acceptance changes address. No divergence approval is inferred from
the review or from a request to proceed with implementation.

### Implementation review corrections

The implementation review added independent checks for the following source
contracts before recording final acceptance:

- SourceFile OnCreate sees constructor metadata; copyFrom reads the original
  after that hook and before OnUpdate/OnClone. Copied metadata shares owner-backed
  slice headers without copying every vector twice.
- Cache ownership follows the source's canonical file/bundle root even when
  accessed through a broader importer. A foreign result remains a retryable Rust
  ownership error; a Go initializer panic completes sync.Once with its nil value.
- Go node comparison IDs are assigned lazily, separately from checked storage
  capabilities. The pinned Go pdqsort and binary-search midpoint order are
  observable for repeated JSDoc node identities; comparison traces test both.
- FullSignature is omitted from protocol properties but still traversed by the
  Go visitor for seven function-like payloads. Generated traversal follows that
  source-member inventory, not the wire property mask.
- The strict protocol rejects invented returned-error outcomes from operations
  such as ParseSourceFile that have no such return contract. Absolute request
  deadlines bound both record trickles and buffered output.
- Generated Go provenance earns no handwritten source coverage. Required source
  utility families have independent open-kind, graph, flag and failure witnesses.

See [the implementation record](S06.md) for the final public boundaries, measured
commands, corpus scope and remaining later-sprint work.
