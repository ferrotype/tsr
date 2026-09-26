# C2 implementation record

## Starting point

C1 review fixes are committed as `cc03e64`. The full reviewed-source capture
at `target/phase2/c1-reviewed` is the authenticated C2 baseline, retained in
`data/phase2/c2-baseline.json.gz`: 13,432 executed variants, no harness errors,
12,459 full-domain matches, and all 9,367 S08 regression rows matching.
The source inputs were stable. The native capture is reused unchanged.

The original `target/phase2/rust` capture is preserved as
`target/phase2/rust-c1-historical-0ab79c6`. The default capture path now points
to the reviewed C1 capture. No capture was deleted or rewritten to absorb a
code change. Its raw snapshots remain usable after C2 edits make it historical.

The C2-start worksheet contains all 191 C2-owned rows with an open domain.
It records the baseline outcomes and exact row reproduction commands. The
declaration-transform panic has a traced C5 handoff from C1. The remaining
rows stay open until their attribution and validation are completed; the
worksheet is not a complete C2 function audit or an exit claim.

## Plan review

- C1's variance measurement handoff is explicit and remains C2 work.
- B03 and B08 need node-builder algorithms over type state that already
  exists. A storage redesign requires a demonstrated missing fact.
- B08 preserves distributivity when an instantiated conditional check is no
  longer a type parameter; its guard is not merely a name collision.
- B10's native template cross-product limit is distinct from the union
  reduction limit.
- Claims and exit accounting must not hide unclassified failures or ignore
  a regressed row previously marked closed.
- Corpus and benchmark binaries are different artifacts. Measurement joins
  their shared production source identity and pin, while authenticating each
  binary's own build. It must not require equal executable hashes.
- Intermediate validation remains the frozen 300-row sample plus affected
  rows. The full function audit grows with the implementation; an attributed
  small refusal fix can proceed before every unrelated row is understood.

## Posted PR #62 review (head `61140fd`)

Reviewed every finding against the scripts, current contract and pin:

- The `result_recorded` mismatch and repeated capture move were valid at the
  reviewed head. The C1 fixes already separate acceptance from optional
  comparison history and preserve captures. `--previous --record` remains
  supported; the reviewed full C1 record reports `result_recorded: true`.
- Blocker handoffs must survive rebuilding. C2.12 now requires validated
  per-variant/cause/domain ownership, stable operation identities rather than
  B-numbers, and retention of mixed groups' unresolved C2 shares. This builder
  change is scheduled work, not implemented by editing the register.
- Failure accounting covers every C2 claim, including failures in AST/arena
  code. Exemptions bind the full raw observation, failure site, request and
  traced artifact. A changed failure or uncovered domain reopens the row.
- The exit table now requires all seven applicable domains and the underlying
  capture-integrity predicates. Declaration diagnostics belong to `errors`;
  there is no separate declarations domain to exclude.
- Receipts bind embedded assets as well as dependency sources, manifests,
  configuration, lockfile and toolchain. This reuses the repaired C1 closure.
  The variance handoff and fixed package-metadata row retain their established
  owners. All three B09 rows are inventory-C2, with the emit cause owned by C5.
- Creation tracing must observe actual fallback branches, actual birth
  origins and natural semantic-ID assignment separately. Go symbol IDs are
  lazy: requesting one for logging would change the result. Equal first
  declarations also reach the symbol fallback. A separately fingerprinted
  diagnostic overlay can instrument these sites without modifying the pin or
  invalidating the canonical native capture. If canonical inputs do change,
  fresh capture/verification and explicit contract equivalence are required.
- Complexity witnesses name `removeSubtypes` and `checkCrossProductUnion`,
  including their callers. The depth contract must prove the actual limit,
  avoid identity shortcuts, compare native diagnostics/results, execute on a
  grown stack and answer a later query in debug and release.

The accepted comparator policy is unchanged. ADR 0010 only gains a factual
clarification of the pinned fallback and lazy-ID behavior. No divergence or
benchmark threshold was introduced.

## First implementation slice

B04, `Checker.extractRedundantTemplateLiterals`, is independently attributed:
the intersection constructor returns `Unsupported` at the exact pinned call
site. Thirty C2 rows expose it. The port uses the existing subtype relation
and pattern-literal predicate, preserving reverse removal order and the
position before `any` handling. No new type representation is needed.

Direct native probes cover matching templates, incompatible patterns,
`Lowercase<string>`, generic templates/mappings, retained constituent order,
and incompatible patterns combined with `any`. The public regression test
failed with the named refusal before the port, then passed against eight
complete native diagnostics. The test binds the pin and fixture source hash.
Targeted clippy with warnings denied passes. A separate source review found no
subtype-direction, ordering or cache-mutation defect.

The frozen 300-case sample plus all 30 affected cases selected 327 distinct
variants. The source-stable capture at `target/phase2/c2-b04` completed all of
them with no harness error or execution failure. Full-domain matches rose
from 228 to 235. All 100 selected S08 regression rows still match. There are
zero matching-domain regressions and zero changed observations in domains
that remained `different`.

The B04 refusal is absent from all 30 target rows. Seven now match completely;
23 remain open: 20 expose ordinary diagnostic/type/display differences and
three reach further refusals (`addIntraExpressionInferenceSite: array element`,
`conditionalTypeToTypeNode: shadowed distribution parameter`, and
`getTypeFromIntersectionTypeNode: pattern literal`). Removing this refusal
does not close those rows. Claims retain their starting status until the
current full exit report validates closure or an attributed handoff.

[data/phase2/c2-b04.json](../data/phase2/c2-b04.json) records the exact
selection/reproduction commands, capture identities and per-target outcomes;
its linked compressed comparison retains all 327 domain results. This is
intermediate evidence, not a replacement for the C1 record or C2 acceptance.
Later C2 operators, the full audit, creation-trace contracts, producer wiring
and quiet-host measurement remain open. No benchmark was run.

## Refusal and diagnostic slice

The next batch ports the remaining immediately attributed refusal paths using
existing type and inference storage:

- B03 groups applied outer type arguments by declaring container and builds
  the corresponding reference or instantiation-expression chain. B08 builds
  the conditional wrapper that preserves distributivity after instantiation.
  A 16-query native display fixture covers grouped arguments, default groups,
  shadowed names, flags and conditional inference scopes.
- B07 registers context-sensitive array elements as intra-expression inference
  sites, before optionality changes the inferred type. The existing fixing
  mapper drains those sites. Its direct fixture compares five native errors.
- B10 reports TS2590 and returns each caller's native recovery result. The
  intersection caller now honors the failed size check. Template, intersection,
  spread and tuple probes compare native diagnostics and recovery; tuple
  normalization deliberately differs from the other recovery paths. The
  two-constituent pattern-literal intersection with `{}` also preserves the
  native no-supertype-reduction flag.
- B11 distinguishes CommonJS typedef exports from ordinary JavaScript value
  exports and keeps `typeof` and type meanings separate. B01's malformed
  `import<T>` expression recovers with the builtin error type while preserving
  grammar and type-argument errors.
- B16 searches binding-pattern names in source order and reports TS1230 or
  TS1225 at the predicate name. Renames, omissions, rest elements, defaults and
  nested patterns are covered by twelve native diagnostics.
- B18 uses each operator's native compatibility predicate when deciding whether
  to suggest `await`, preserves original versus base literal types accordingly,
  and reports the native related-information range. Equality now shares this
  reporter instead of carrying a second implementation. Its fixture compares
  all 22 native diagnostics, including nested related information.
- Intrinsic alias validation follows the pin's name/arity pairs, including
  `NoInfer` and `BuiltinIteratorReturn`. These forms are allowed outside the
  libraries too. Invalid names or arities report TS2795; valid constraints
  still get checked. The plan's earlier library-only wording was corrected.

The native-backed C2 tests share a small diagnostics adapter where their
contracts coincide. Display and related-information tests retain their own
observers. All thirteen focused tests pass, and targeted clippy with warnings
denied passes. Independent source reviews of the array inference, reference
and conditional display, type-predicate and awaited-operator changes found no
additional defect. These tests do not replace the sampled corpus comparison
or the outstanding C2 audit and exit contracts.

The combined sample selected 385 distinct rows: the fixed 300 plus 91 affected
rows and controls. Every process completed, source inputs stayed stable, and
there were no harness errors. Full-domain matches rose from 230 at C1 to 293;
65 of the 91 targets now match completely. All 100 selected S08 regression
rows still match. No matching domain regressed against C1 or the earlier B04
sample, and no observation changed in domains that remained `different` in either
overlap.

None of the targeted C2 refusal messages remains in this sample. This exposes
one further failure in `recursiveConditionalCrash3`: public display fails
after the conditional wrapper is built. It was previously hidden behind the
B08 refusal and remains C2 work, not an accepted divergence or closed claim.
Other targets still have ordinary semantic or display differences.

The exact selection, capture identities, reproduction commands, target outcomes
and compressed 385-row comparison are retained in
[data/phase2/c2-refusals.json](../data/phase2/c2-refusals.json). As with B04,
this is intermediate evidence. The reviewed C1 baseline and C2-start claims
are preserved.

## Alias keys, defaults and wrapper semantics

The alias-instantiation cache now keys the supplied argument list and alias
metadata before filling defaults. Intrinsic dispatch also sees the supplied
argument count. Defaults are filled only on a miss, using the alias symbol's
value declaration for JavaScript mode, exactly as the pin does. A JSDoc
alias's source file alone does not establish that mode.

The access-only native observer records six query sequences without assigning
Go IDs or warming the cache. Both query orders show separate cache work for
omitted and explicit defaults: cache-entry deltas `1, 1, 0, 0`, instantiation
deltas `3, 3, 0, 0`, but equal returned type identities and displays. Dependent
defaults have their own control. For a module-local defaulted `Uppercase`,
the omitted form remains `intrinsic`; the explicit form maps the argument.
The public difference includes a TS2322 only for the explicit form. Four
focused Rust tests compare native work, equality matrices, display and
diagnostic ranges, and verify that the observer is read-only and rejects a
foreign checker. There is no production hot-path instrumentation.

A separate missing branch in `fillMissingTypeArguments` converts defaults
identical to `unknown` or the empty object to `any` in JavaScript implicit-any
mode. The conversion occurs before instantiating dependent defaults. Four
previously wrong conversions now match; fourteen native diagnostics cover
explicit arguments, structural identity, dependent defaults and TypeScript
controls. This is distinct from raw alias keying.

The `instantiationExpressionErrors` bucket also needed attribution. Both
TS1477 parser diagnostics and their ranges already match native. Its extra
TS2869 came from a private outer-expression helper that omitted instantiation
expressions. It now uses the shared `SkipOuterExpressions(OEKAll)` port.
The native test retains ordinary never-nullish and always-nullish errors plus
a later assignment diagnostic, while rejecting the five spurious errors on
wrapped instantiation expressions. The row's declaration accessibility errors
remain separate work; fixing TS2869 does not close the whole row.

## Retained instantiation nodes and elision dependency

Instantiation-expression types now use their retained syntax node when
collecting outer type parameters, apply the pin's reference filter, and retain
that node when instantiated again. Previously they used the original function
symbol's declaration, losing the caller's generic scope. Fourteen native
display observations cover inferred string/number calls, a generic wrapper,
re-instantiated boolean arguments, repeated calls and explicit controls.

The B08-dependent display failure was the pre-existing synthetic-elision
refusal in the shared node builder. Type-list elision now creates the native
`any` node and synthetic leading comment under `NoTruncation`; conditional
elision uses the native placeholder length accounting. Ten access-only native
branch cases compare output, length increments and comment metadata. The
large corpus query now executes and a subsequent ordinary query still matches.

Its full-byte display parity remains open: Go emits 790,211 bytes; Rust emits
789,025. The first 788,929 bytes agree, then Rust reaches the approximate-length
truncation budget earlier. Native SHA256 is
`f401efcd5b97621a0019f1e9465d1228b8df7a18db17c442bd14fe9e333ded0c`;
Rust SHA256 is
`5d8999cfbf9e9c8d25ff5bece3478facce62ad8fa8ccf323fe1661ae611e1a90`.
The integration test explicitly claims completion and recovery, while retaining
the native hash; it does not assert or manufacture full parity. This shared
display dependency does not transfer all B13 work from C5 to C2 or approve
this remaining difference.

The binding-pattern predicate search also uses an explicit depth-first stack
instead of unbounded Rust recursion, retaining source order and the first
matching-name diagnostic. Its native diagnostic inventory is unchanged.

The final combined sample selects 393 distinct rows: the frozen 300 plus 99
affected cases and controls. All processes complete with stable sources and
no harness failures. Full-domain matches rise from 233 at C1 to 300; 72 of
99 targets now match. All 100 sampled S08 regression cases remain matches.
There are no matching-domain regressions against C1 or the preceding refusal
batch. The sole changed observation in a domain that remains different removes
only the extra TS2869 from `instantiationExpressionErrors`, with no added or
changed diagnostic.

`circularInstantiationExpression` and `defaultPropsEmptyCurlyBecomesAnyForJs`
now match every enabled domain. The three selected `genericDefaults` controls
retain their full matches.
`recursiveConditionalCrash3` matches every domain except public display; the
remaining difference is recorded above. The other 26 targeted open rows and
the broader C2 audit/contracts are not closed by this partial sample.

[data/phase2/c2-instantiation.json](../data/phase2/c2-instantiation.json) retains
the exact selection, reproduction commands, comparison identities and all
target outcomes; its linked compressed comparison contains all 393 rows.
Focused tests and targeted clippy with warnings denied pass. No benchmark or
full C2 acceptance capture is claimed.

## Native contracts and the source audit

The C2 audit enumerates 231 pinned functions, including every function in
`inference.go` and `mapper.go`. Inline Rust implementations are described at
an actual call site rather than credited by file-level coverage. Two inference
predicates depend on state populated only by Phase 5 signature-help services;
that dependency remains explicit. Decorator contextual typing belongs to C4.
The audit also checks semantic `open_issues` on functions that already carry
port markers: an annotation alone cannot close an identified implementation gap.

The contracts now exercise source-based inference fixing and cloning, mapper
kinds, syntactic defaults, generic tuple inference, contextual restoration,
instantiation/cache identities, higher-order inference, reverse mappings and
library overload diagnostics. The variance fixture observes all seven flag
values and recursive entry-order restarts. Limit fixtures execute the actual
instantiation depth and five-million-call thresholds, the subtype-work estimate,
base/conditional constraint depths and mapped/conditional recursion identities.
They do not seed counters or lower thresholds to force the branches.

These fixtures found differences in mapped inference, enum-index filtering,
generic tuple intersections, cached contextual absence, constraint resolution,
deprecated type suggestions and mapped recursion identities. Their regression
expectations come from independently captured pinned Go operations. A source
witness also exposed binary fallback contextual typing: for a destructuring
parameter initialized with `values || [1, "hi"]`, with `values: number[] |
undefined`, Go gives both elements `string | number`; Rust previously gave the
first `number`.

The mapped recursion fix may allocate a stack snapshot once the relation's
native depth threshold is reached. The snapshot preserves the active relation
frame if mapped-modifier resolution reenters the checker. This is a correctness
fix with a potential allocation cost, not an allocation-neutral optimization;
the final measurement must include it.

## Creation-order witnesses

`tools/phase2/order-trace/witnesses.json` declares four source programs for
intrinsic types, reverse-mapped types, declaration-less equal-named properties
and properties with equal first declarations. The diagnostic overlay observes
object birth, natural semantic-ID assignment, the actual comparator fallback
and enclosing sort inputs/outputs. Its tokens are independent of semantic IDs;
logging never calls a lazy ID getter or display function to label an event.
The frozen native trace is `data/phase2/c2-order-traces.json`.

The symbol witnesses found a production difference. Go's `valueSymbolLinks.Get`
assigns a lazy symbol ID when instantiated, tuple and compound property links
are read or written. Rust had left those IDs unassigned until sorting, so the
same source properties sorted in the opposite order. The fix assigns the ID
at the corresponding production link operations. It does not replace the
comparator's semantic ID with allocation order. The four native/Rust witnesses
now agree on ordinary output and the relative order of the actual fallback
operands, without requiring equal whole-program allocation sequences.

The diagnostic runner keeps parse, bind and checker work on the same parser
worker so its thread-local observer sees binder births. It builds ordinary and
instrumented executables separately. Tracing-enabled and tracing-disabled
outputs must agree, and provenance binds the observer, request manifest,
replacement bytes, compiler flags, pin and executable. These diagnostic builds
are excluded from performance measurements.

## Declaration/display fixes shared with C5

The user authorized working across C2–C6 where a shared implementation cause
is involved. The queued augmentation declaration now retains its actual source
before the declaration transformer reads it through the output factory; the
active transform source remains unchanged. The exact corpus request now matches
native diagnostics, type/symbol baselines and public display without the former
`WrongOwner` panic.

The large recursive conditional display is now compared against the full
native output digest, not merely successful completion. A temporary diagnostic
trace localized truncation drift to type-parameter names and infer-type length
accounting. Pinned `symbolToName` adds no type-access length charge; an infer
node charges its name and the optional constraint wrapper. After those fixes,
the 790,211-byte output matches native SHA-256
`f401efcd5b97621a0019f1e9465d1228b8df7a18db17c442bd14fe9e333ded0c`.
The temporary per-node trace hooks were removed after the comparison.

The combined 15-test C2 contract suite currently passes in debug. This is an
implementation checkpoint, not the full C2 exit: the current-source corpus,
release contracts, relater/obligations and measurement still have to be recorded.


## Remaining-row native fixes

The 482-row audit sample (the fixed 300 plus the C2 claims) exposed 24 C2
rows still open. The next fixes are checked against retained native requests
and complete observations, rather than Rust-authored expected diagnostics:

- Bigint declaration names report TS1539; branded intersection index keys use
  the native any-constituent test; mapped display resolves the original
  `keyof` operand; mutable expression inference finds the enclosing inference
  scope instead of requiring an exact contextual-node cache entry.
- Context-sensitive functions contribute their contextual signature parameters
  to nested generic declarations. Uninitialized-reference checks use the
  native top-level undefined predicate instead of descending intersections.
- Empty-object checks reject generic mapped types before resolving their
  members. The reversed order had reentered constraint inference and cached a
  circular constraint, truncating both Redux diagnostic chains.
- Entity-name resolution follows aliases until the requested meaning is
  present. JSDoc full signatures supply their return and `this` types, and
  structured members are published before named-member enumeration can reenter
  resolution. CommonJS declaration exports preserve their annotated type.
- Import-type reuse tracks the full display symbol chain, including its module;
  reparsed JSDoc modules use their reparsed declaration symbol.
- Synthetic property elision now preserves native empty-object and entry
  shortcuts, omitted-property comments, final-property ordering and approximate
  length accounting. Index-signature names, readonly modifiers and the eagerly
  evaluated elided placeholder contribute their native charges.

The exact corpus fixtures establish diagnostics, type/symbol baselines and
public display. They do not claim declaration-text parity. The separate native
property observer establishes node kinds, synthetic-comment metadata, length
increments, restored flags and printer text for 14 branch schedules. Large
naturally truncated corpus displays are verified by their full canonical byte
length and SHA-256, without committing another 30 MB of rendered text.

The contextual-node cache no longer duplicates inference identities. Inference
scopes live in their dedicated stack; the reachability census follows that
stack. This removes an unused field after mutable-expression inference started
using the native ancestor search.

These are focused results. The final current-source full corpus and exit
receipts below, when recorded, remain the acceptance authority.


## Full correctness exit at `35661af`

The fresh `target/phase2/c2-exit-01` run completed all 13,432 variants with
unchanged sources, no timeout or execution failure, and no harness error.
12,647 match every enabled domain. All 9,367 S08 regression variants remain
full matches; no previously matching domain regressed against reviewed C1.
The complete comparison is retained in `data/phase2/c2-exit-comparison.json.gz`.

C2 has 2,951 fully matching rows and three errors-only differences. Native
stacks prove that these three are caused by missing JavaScript emit callers:
constant-enum inlining for `incorrectRecursiveMappedTypeConstraint`, and import
elision for `typeParameterWithInvalidConstraintType` and `recursiveMappedTypes`.
The native harness creates separate programs for pre- and post-emit diagnostics;
the latter runs JavaScript transforms before semantic diagnostics. Rust matches
the pre-emit results. The exact operations, 14 diagnostic-production stacks,
retained pre/post output and reproducer are in
[`c2-emit-handoffs`](../data/phase2/c2-emit-handoffs/README.md).

The claims close 188 baseline-open rows and bind three C5 handoffs to the
current request, full raw observation and capture hashes. These handoffs do
not accept or erase the raw differences. The rebuilt blocker register has
no remaining C2-owned entry. C3 has 15 open rows and C4 has 767.

All 16 C2 contract tests pass in both debug and release. Their current-source
receipt is `data/phase2/receipts/c2-contracts.json`. The 133 Phase 2 Python tests
and 115 subtests pass; clippy with warnings denied passes for the four changed
production crates and their targets/features. Remaining runtime/measurement
receipts are recorded separately as they finish; this subsection does not
claim `c2_complete` before those authorities exist.


The remaining correctness witnesses have now passed on the same source bytes:
E2 comparator/recursion obligations, all 105 production and 105 reference
relater groups (including creation/cache state and structured diagnostics), and
all four standalone creation-order witnesses in both debug and release.
Ordinary, instrumented-but-disabled and enabled tracing preserve output.
`data/phase2/c2-runtime.json` binds the retained artifacts. Workspace formatting
and clippy with warnings denied pass, as do the final 133 Phase 2 Python tests
and 115 subtests after recording the exit data.

The checker producer now reports `c2_open = 0`, `c2_regressions = 0`,
`c2_failures = 0`, `c2_blockers_open = 0`, `c2_handoffs = 3`,
`c2_audit_complete = true` and `c2_contracts = true`. At this checkpoint,
`c2_measured` and therefore `c2_complete` are still false until the required
current-source checkerbench capture is verified and recorded. No performance
threshold is changed or inferred from the correctness capture.

Ledger validation also passes on this recorded correctness checkpoint.

## Completed measurement and final recording

The full checkerbench capture is retained at
`target/s08/c2-checkerbench-01`, with its replayed report committed as
`data/phase2/c2-benchmark.json`. All 48 processes completed: both runtimes,
normal/phase/allocation modes, one warmup and seven measured samples each.
Every one of the 9,369 variants has identical action counts and output digests
across all samples and runtimes. Sources stayed unchanged; every census metric
is available.

| Metric | Rust / Go |
| --- | ---: |
| Checker elapsed time | 2.135771 |
| Requested bytes | 0.573941 |
| Retained bytes | 1.414528 |
| Type-storage bytes per reachable type | 0.812580 |

Normal interval medians are 12.947 seconds Rust and 6.062 seconds Go. Their
max/min ratios are 1.013 and 1.041, respectively, within the existing stability
rule. The report nevertheless marks `host_busy: true`: observed one-minute
loads exceed its 2.0 threshold in every mode. This is a current-source measured
result with that qualification, not certification of a quiet host or a claim
of meeting the later CPU/memory budgets. No sample or threshold was changed.
The C2 exit requires an authenticated measurement with available metrics; it
does not turn the later performance budgets into a C2 gate.

The three handoff claims now hash the actual native stack artifact directly.
The checker evidence fingerprint includes the entire handoff/reproducer bundle,
and a regression checks every recorded handoff trace against that closure.
The 31 C2 script tests and 27 subtests pass after this final metadata change.
The S07 operation inventory is also refreshed for moved Rust mapping lines;
its operation set and pinned native source are unchanged.

`cargo xtask run checker` now records `c2_complete = true`,
`c2_measured = true`, `c2_contracts = true`, and `c2_audit_complete = true`.
Open C2 cases, regressions, execution failures and C2-owned blockers are all
zero; the three validated C5 handoffs remain reported. `P2B-C2` is complete.
Whole Phase 2 remains open for C3–C7; older checkpoint receipts retain their
own source freshness rather than being relabeled as current.

## PR #62 review follow-up

The bounded review fixes preserve the preceding full captures as historical
observations of their recorded source revision. They do not relabel those
captures as evidence for these production changes.

- The measurement receipt locates its capture relative to the checkout. Its
  capture, build and production-source digests are unchanged by that locator
  migration. Source filtering now excludes repository-relative build caches;
  a checkout whose parent is named `target` no longer produces empty hashes.
- Outgoing handoffs and `later` audit dispositions must name a later owner.
  C2 cannot exempt a row by assigning it back to C1, whose claim accounting
  does not consume such incoming transfers.
- Value-symbol links require an observed key. This reproduces
  `symbolArenaLinkStore.Get/TryGet/Has` assigning the lazy comparison ID even
  on a miss. Go's other symbol stores use pointer keys and remain unchanged.
  Rust-only snapshots, censuses and diagnostic inspection use passive reads.
  Reverse mapping, spread construction and symbol cloning preserve the native
  new-symbol-before-source access order; the compound-property clone also
  retains the native transient-symbol guard. The mapped-property cycle check
  uses the shared resolution predicate, including its native link reads.
- Generic checking and inferred constraints now share
  `getTypeParametersForTypeAndSymbol`: absent or empty alias parameters fall
  through to the reference target's local parameters. Its direct test covers
  that internal branch without claiming a reachable TypeScript counterexample.
- The checker producer authenticates one run-local capture context and shares
  it among comparison, blocker construction, handoff validation and measurement
  joining. No context is loaded from recorded JSON or reused across producer
  invocations. Blockers and metrics use one handoff-authority loader.

Focused validation passed 74 checker unit tests, the 16 C2 contracts (including
the frozen native creation-order observations), 129 Python tests with 118
subtests, and checker Clippy with all targets/features and warnings denied.
Each command was bounded below ten minutes. No corpus or benchmark was run.
The C1 contract receipt was refreshed on these sources in 103 seconds: all
seven contracts passed in both debug and release, and `receipt_current` is
true. This refresh does not refresh the full correctness or measurement
captures above.
The relation-stack allocation optimization from review item 7 remains separate:
its reentrant snapshot requirement has not changed.
