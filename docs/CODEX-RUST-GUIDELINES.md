# Rust and Go-to-Rust working guide

For Codex/Astra. The [loading conditions](../AGENTS.md) apply. This guide
supplements the accepted design notes, ADRs and the user's instructions; it is
not a style mandate for other models or contributors.

Before writing code, answer these questions:

1. What observable behavior must this API preserve, including invalid input?
2. Who owns the data, who only borrows it, and what makes an escaped result live?
3. What scans, allocations, reference-count operations and locks occur on the
   ordinary path, cache hit and first-use path?
4. What happens after an error, panic, retry, reentry or owner retirement?
5. What independent observation could prove the implementation wrong?
6. Which other claims or sprint items will consume the emitted evidence?

## Implementation checks

- Split by responsibility and invariant before the implementation grows. Use upstream files as a starting point, not an inflexible module rule.
- Make the normal result a borrow; require explicit retention when it escapes. Account for atomics as well as allocations.
- Use a small branded API where repeated checks justify it, while retaining checked raw imports and safe bounds access.
- Use byte/character literals and meaningful constants without losing the operation's integer or byte semantics.
- Analyze reentry and wait cycles; detect prohibited same-thread reentry without rejecting ordinary contention.
- Decide storage ownership and initialization timing separately. Verify unused derived data stays uncomputed.
- Inspect every consumer of shared metrics, including later sprints. A workload subset must not certify the whole experiment.
- Compare failure classes and contract messages; label modeled checks separately from upstream truth; freeze requests as well as scenario names.
- Preserve the failed scenario and diagnostic. Failing closed does not replace useful failure reporting.
- Verify the effective toolchain, configuration and failure flow of the complete producer, not just its command in a local environment.

## General Rust rules

### Design ownership and costs together

Start with the smallest truthful access contract: `&T` for a borrow, an owned
value for transfer, and a retained handle for an explicit lifetime extension.
Do not add `Arc`, `Mutex` or cloning merely to make a lifetime problem disappear.
Work out what must outlive what before choosing the storage representation.

An owned result does not necessarily require a byte copy. When replacing a
contextual string reader with `&[u8]`, preserve its explicit owned conversion:
`JsString::from_bytes` allocates, while a source slice or pooled-string clone
can retain existing backing. Carry that operation through the local reader
instead of reconstructing it from borrowed bytes. Check backing sharing and
post-owner-drop validity, then charge any new requests in the full pipeline.

Keep mutable construction separate from publication when that matches the
lifecycle. A consuming `finish(self)` can make invalid transitions unavailable.
Keep identity constructors and mutation capabilities private to the layer that
can uphold their invariants. Add generics for real payload variation, not for
every hypothetical future owner or algorithm.

For a common operation, write down its structural cost during design:

| Operation | Intended leaf behavior |
| --- | --- |
| Core lookup | Borrow; no lock, owner `Arc` clone or node allocation. |
| Escape a node from its scope | Explicitly retain the complete file/bundle root. |
| Lazy lookup | Synchronize lookup and retain a stable page; do not describe it as the same cost as core access. |
| Publish a file without requesting its map | Publish ownership without constructing the derived position map. |
| Unchanged ASCII casing / truncation | Return borrowed `Cow` / a byte prefix where the API permits it. |

These are operation-level claims, not compiler benchmarks. Storing bundle
members as `Arc<FileOwner>` instead of inline file owners also changes allocation
costs; a cheaper lookup does not establish fewer allocations everywhere. Measure
actual throughput or heap behavior before making that broader claim.

### Make module and API boundaries reviewable

Keep one cohesive invariant together. Separate unrelated responsibilities even
when they share an owner type. `lib.rs` should expose the intended API; private
implementation details need not become public to connect modules. Small helpers
should name an operation or remove a repeated rule, rather than just move lines
elsewhere. Reuse a helper only after confirming its edge behavior matches.

Use explicit units in names: `byte_offset`, `utf16_units`, `slot`, `arena_id`.
Short conventional loop indices are fine locally; do not mechanically preserve
Go's short names when they hide which representation is being manipulated.
Keep control flow direct. Neither iterator chains nor manual loops are a goal
on their own.

Document non-obvious behavior, ownership, panic boundaries and concurrency
restrictions at the API. Link to the design note for the architecture instead
of copying its paragraphs into module headers. A UTF-16 decoder comment about
discarding an odd trailing byte is useful because it explains an otherwise
surprising compatibility choice.

### Use Rust conventions when their semantics fit

Derive traits when field semantics and generated bounds match the abstraction.
Do not replace manual implementations just to reduce a report's count:

- `JsString` equality, ordering and hashing observe its selected bytes, not its
  backing allocation, range representation or cached validity tag.
- Cloning an `Arc`-backed generic handle should not require its payload to
  implement `Clone`; a manual implementation can preserve that API.
- `Debug` should expose useful identity or state without recursively dumping an
  entire retained graph.
- Conventional methods should read conventional state. For a bundle with a
  mandatory canonical file, `is_empty() == false` is consistent with the invariant,
  but delegating to the collection states that relationship more directly.

Distinguish recoverable errors from violated invariants. Use typed errors at
fallible boundaries. Use `expect` only where construction or prior validation
establishes its premise, with a message that identifies that premise. An
upstream contract panic, a poisoned lock and malformed external input are
different cases; neither banning panics nor turning all of them into `Result`
is a sound rule.

Start with safe Rust and the standard library. In this repository, inherit the
workspace's unsafe prohibition and dependency policy. A dependency needs a
specific capability, maintenance and validation reason; “fewer dependencies”
alone is not proof of better code. Standard locks let production synchronization
run under the selected strict-provenance Miri configuration. That is a scoped
choice, not a claim that another lock library is universally incorrect.

### Treat concurrency as a state machine

Before adding a lock or callback, identify the protected state, publication
point, lock order, callback behavior and unwind policy. Include cache hits,
misses that race, partial construction and retry in the design.

Reservation and publication are separate: a failed attempt burns its
provisional IDs, drops private staged payloads and publishes no partial graph.
Page addresses remain stable while the directory grows. The cache miss is
rechecked under the same write lock that protects publication.

Panic recovery needs a reason that the surviving state is valid. Lazy
initialization catches the unwind while the guard remains held, drops staging,
then unlocks and resumes the panic. Checker-lease unwind instead retires the
shared generation before releasing its permit. Do not blindly ignore poison or
assume a non-poisoning mutex establishes recovery safety.

Do not use `try_read` as a reentry detector: it also fails during legitimate
contention. The current thread-local guard detects same-file reentry, but does
not solve cross-thread wait cycles. Initializers must not wait on another
thread that needs their lazy storage, including indirect cycles through other
files. Moving construction outside the lock would require a new publication
protocol; it is not a mechanical cleanup.

## Go-to-Rust rules for this project

### Specify the source semantics before choosing Rust types

For each ported boundary, record the source type, unit, valid and invalid ranges,
overflow behavior, clamping, rounding and panic behavior. Use the pinned Go
implementation and accepted project design together. If they conflict or an
intentional change is required, make that decision explicit through the
project's divergence process; do not silently “improve” observable behavior.

Use conversions according to their purpose:

| Purpose | Default |
| --- | --- |
| Lossless widening | `From` / `into` when available. |
| Validation of a Rust ownership or external-input boundary | `TryFrom` / checked arithmetic with a meaningful failure. |
| Required Go narrowing or signed reinterpretation | A deliberate `as` at the corresponding boundary, with edge tests. |
| Required source wraparound | Explicit wrapping arithmetic at the operation where Go wraps. |
| Rust slice access | Convert after establishing the appropriate bounds or faithfully reproducing the required failure path. |

Do not change signed positions to `usize` merely because they eventually index
a slice. Do not replace wrapping with checked or saturating arithmetic without
a contract change. Conversely, never apply the text port's wraparound policy to
arena identity allocation: IDs must reject exhaustion before wrap or reuse.
Go's machine-sized `int` and `int32` are distinct; the current supported native
targets are 64-bit. A new target requires revisiting those assumptions.

Preserve the distinction between API, LSP and scanner conversion. UTF-16 offset
1 inside `😀` maps to byte offsets 1, 0 and 4 respectively in the tested paths.
A single “correct UTF-16 converter” would erase those contracts. Keep byte
positions, UTF-16 counts and line indices distinguishable at call sites.

### Preserve bytes and choose decoding per operation

Go strings can carry arbitrary bytes; Rust `str` cannot. Keep malformed bytes
and lone surrogate encodings representable. Expose `&str` only when valid;
never insert lossy conversion to satisfy a Rust API unless that operation's
contract explicitly requires replacement.

Choose the decoder used by the upstream operation, not one inferred from the
container's validity tag. Standard Go UTF-8 decoding and the JavaScript
sentinel decoder differ. For example, truncating `ED A0 80` to one Go-decoded
rune returns `ED`; preserving the full sentinel there would change behavior.
Slices classify their own bytes and may cut a multibyte sequence.

Pin semantic data as well as source. JavaScript casing uses upstream generated
tables; `LowerFirstChar` follows the pinned Go toolchain's simple lowercase
table. Rust's current Unicode behavior is not a substitute even if today's
examples match. Retain a discriminator such as Garay U+10D50 when testing this
distinction. Do not replace simple mappings with full casing or add
normalization unless the source operation does so.

### Replace GC reachability with explicit, sufficient ownership

Keep identity, validation, borrowing and retention separate. Raw `NodeId` and
`SymbolId` values do not retain storage. Imported IDs receive owner, generation
where applicable, and published-slot validation in release builds. Branded
local handles justify eliding repeated owner checks only within the scope that
minted them; keep compile-fail tests for escape and cross-arena misuse.

A node that escapes from a mapped file must retain its complete bundle, not
just the page containing that node. Graph links use non-owning IDs to avoid
ownership cycles. Generic payloads can still contain interior mutability or
owning links: the storage API does not prove arbitrary AST payloads immutable
or cycle-free. Future AST and checker types must uphold their own contracts.

Retirement, disposal and response commitment are separate events. A retained
object may remain allocated after its generation becomes unusable. The current
lease primitive does not establish the later server's atomic
retirement-versus-publication gate. Do not implement or claim those later
systems through placeholder success flags.

## Verification rules

### Test the assertion, then test the measurement

For each important claim, identify a concrete counterexample before writing its
test. Use runtime regressions for behavior, compile-fail cases for type-system
guarantees, and appropriate instrumentation for executed memory/concurrency
paths. Add failure-path tests for the producer itself. More assertions that
repeat the implementation do not provide an independent reference.

| Claim | Counterexample the check should detect |
| --- | --- |
| Core reads borrow cheaply | A hidden owner `Arc` clone or allocation appears on repeated lookup. |
| Failed IDs stay invalid | A retry makes a previously exposed provisional ID resolve to a new node. |
| Panic parity | Rust overflows or hits a wrong assertion where Go raises a bounds error. |
| Complete differential coverage | One request disappears while the same scenario name remains. |
| Honest experiment scope | Seven instrumented leaf scenarios complete an item requiring every future E3 scenario. |
| Diagnosable failure | One scenario panics and the report loses its identity or manufactures zero aggregate counters. |
| Independent CI capture | A failed E3 command or nightly install prevents E4 evidence or artifact upload. |

The oracle should execute the pinned upstream functions, with access-only
bridges where necessary. If upstream has no equivalent, label a model as a
model and test the Rust contract separately. A modeled classifier in the
differential payload must not imply upstream validation of a Rust-only validity
tag. Agreement between two versions of the same algorithm is not upstream
validation. Slice probes can compare bounds, bytes and Go UTF-8 views while
Rust classification invariants are tested separately.

Validate the protocol before treating any failure as library behavior. Reject
duplicate keys, non-finite values, wrong types, duplicate/missing/extra/reordered
results and malformed probes. Freeze the actual request inventory as well as
scenario IDs; regenerate it only after reviewing a corpus change. Hashes bind
content but cannot establish independence or coverage by themselves.

Retain every panic payload for diagnosis. Compare stable contract messages
exactly and recognized runtime bounds failures by a narrow class. Unknown,
assertion and overflow panics must not pass merely because both programs
panicked. Do not loosen the classifier to make a new mismatch disappear.

Repeated source output can vary even under an exact pin; a dependency can
deliberately alternate between two diagnostic prefixes. Establish that behavior
from the dependency's code and tests before adding a qualification; limit it to
the demonstrated field and prefix, retain the raw diagnostics, and test that a
changed message body or unknown prefix still fails. A diagnostic-only field is
not a reason to silently discard all source-check differences.

Exercise a producer helper through its caller as well as on its own. A helper
can pass its checks yet print progress into the containing producer's JSON
channel, invalidating the capture. Reserve stdout for the protocol and test
embedded calls with captured stdout/stderr before running the complete corpus.

### Keep claims no broader than the execution

A successful capture is not a passing metric. Inspect the metric gates after
running producers. A failed scenario should retain its name and diagnostic;
unavailable counters or instrumentation should remain absent. A measured
instrumentation failure is false; missing prerequisites or malformed output
cannot become success.

Read every consumer of a shared metric before changing its producer.
Full E3 must remain incomplete while its later scenarios are absent. Likewise,
owner/storage-unit counters establish their measured disposal properties, not
total heap bytes, RSS or the absence of every possible leak.

Miri and ASan cover the paths they execute; neither proves all ownership or
deadlock behavior. Test the actual production synchronization. Do not replace
it with a simpler test-only implementation or weaken instrumentation to obtain
a green result without explicitly assessing the lost coverage.

### Review the environment and failure flow

Use one machine-readable source for toolchain pins and verify the compiler
actually invoked. Keep `GOTOOLCHAIN=local` when the evidence names a fixed Go
version. Check the declared MSRV, not just the current compiler; review warnings
on the pinned instrumentation toolchain as well.

Preserve caller Cargo/Rustup homes and registry/offline configuration. Scrub
compiler-related environment overrides deliberately; this does not make
arbitrary Cargo configuration hermetic. Keep reusable toolchains and sysroots
outside disposable build output and explicitly cache them in CI. An offline
run still needs its dependencies and components provisioned.

Export only the pinned source closure needed by the oracle, including embedded
resources and module metadata. Check archive command failures before decoding
the archive. Use stable build inputs such as `-trimpath` so temporary paths do
not cause cold recompilation. Share protocol/subprocess helpers when they
implement the same contract, and register them as evidence inputs.

A successful Cargo command does not identify the executable to measure. Consume
Cargo's `compiler-artifact.executable` and check its features/target; a caller's
`build.target-dir` can otherwise leave an older binary at a guessed path. A
configuration hash records the configuration but does not establish a claimed
profile. Enforce or observe the actual optimization, LTO, unwind and codegen
settings while preserving registry and offline configuration.

Fresh builds need not produce identical bytes: native dependencies can embed
compilation timestamps, and linkers can retain temporary output paths. When
semantic validation is bound to an executable hash, time that exact executable
after checking its source and build configuration. Rebuilding the same source
is not a replacement for its artifact identity, and a mismatch is not a reason
to weaken the hash check. Build separately instrumented variants explicitly.

Bind each measured child to the bytes and options it actually loaded. Equal
file, byte, node, symbol and diagnostic counts cannot distinguish a same-size
literal edit. Compute the ordered input identity during preload,
outside the measured phase, and compare it with the independent graph
obligations. Checking mutable input files only before a batch cannot prove what
later children executed.

Trace CI from a failed install or producer through the remaining steps.
Independent captures and artifact publication should remain reachable after
unrelated failures while their prerequisites still hold. A successful run alone
does not test that failure path or demonstrate a warm cache hit.

When a crate adds compile-time assets, audit every workspace build job's checkout
inputs. A local MSRV check with an initialized `upstream` does not establish that
CI retrieves the embedded library files. Reproduce the job from a fresh checkout;
an installed compiler and a passing warm workspace check do not establish that
CI retrieves all required files.

## Review and delivery

1. **Plan from contracts.** Read the relevant design notes, source functions and
   callers. Record the ownership transitions, semantic traps, scope and common
   path costs. Start with module boundaries and a narrow public API.
2. **Implement a complete slice.** Keep upstream mappings traceable. Preserve
   existing current-main policy and CI changes when borrowing from another
   branch. Do not import its historical evidence as proof of the new code.
3. **Review behavior and costs separately.** First inspect bytes, arithmetic and
   failure semantics; then inspect retention, scans, allocations and locks.
   Passing parity does not establish a cheap or maintainable API.
4. **Challenge the harness.** Inject representative missing/duplicate requests,
   wrong panic reasons, failed scenarios and partial metrics. Inspect downstream
   gates and fresh-worktree prerequisites. Ask an independent reviewer for
   counterexamples in a named area, not just “another review.”
5. **Validate proportionately.** Run relevant debug/release, type-level, MSRV,
   lint and instrumentation checks for the change. Reuse current evidence only
   where its declared inputs remain unchanged. Documentation-only edits need
   link/content checks and any affected tracker validation, not a ritual rerun
   of every expensive producer.
6. **Report precisely.** State what changed, what ran, what failed or remains
   unmeasured, and which target the evidence covers. Use new commits and normal
   pushes for published branches; do not rewrite remote history without the
   user's specific authorization.

Stage the final file inventory before evidence captures when moving or deleting
files. The tracker currently hashes an unstaged tracked deletion as `deleted`,
but omits the path after staging. Verify current gates again after staging and
before committing; source-byte stability alone does not establish stability of
this tracker's input inventory.

For quantitative acceptance targets, estimate the representation budget from
representative counts and measure a storage prototype early. Include replacement
storage in any proposed savings, and use separate
CPU profiles before attributing time to a suspected cost. Safe ownership and
semantic parity do not establish acceptable memory use or speed.

Judge performance tradeoffs at the scale of the actual requirement. Report
absolute milliseconds and MB before relative percentages, with explicit sweep
counts, production invocation frequency and allocation/live/RSS endpoints. A
large slowdown inside a tiny microbenchmark is not a whole-pipeline veto, and a
microbenchmark saving is not automatically available in production. If the real
frequency or caller contract is unmeasured, show conditional arithmetic and name
the missing observation. Apply pipeline thresholds only to pipeline measurements;
keep a materially smaller storage candidate in contention while testing its
integration. Do not infer a rigorous upper bound from a cheaper synthetic control.

Separate performance acceptance from experimental retention and investment.
Components that trade CPU for memory can belong in one combined implementation;
measure that complete implementation against the same control before promoting
it. Preserve component results for attribution, including regressions. Never add
separately measured savings or use a weighted score to excuse a failed final
gate. An investment threshold is not a reason to discard a reviewed small win
already supported beyond noise: weigh its absolute benefit, maintenance cost
and whether its implementation surface will survive the next planned change.
Keep phase elapsed clocks distinct from sampled CPU; retain matched phase
attribution when a storage tradeoff could improve binding while slowing parsing.

Do not use report grades or counts as acceptance criteria. Shorter files,
fewer casts, fewer `expect`s, more derives, more comments and more probes can
each make a port worse if pursued without examining the contract. Identify
hidden costs and unsupported claims early while retaining exact semantic work.

An output-map entry must not make `ast_schema` a success sentinel instead of a
separately observable resolver result. Unique staging directories prevent reuse
but accumulate without a retention limit. Broad source globs force unrelated
leaf edits through an expensive generator. Test a failed frontend, a second
attempt and an unrelated source edit explicitly. Compare CI step durations and
actual cache-hit logs before attributing an entire slowdown to one tool or cache;
a clean independent review does not substitute for those observations.

`rust-cache` can retain a tooling directory under `target/` while removing its
Git files before saving the cache, leaving a restored directory without a usable
repository. Keep non-Cargo repository state outside Cargo's cleanup domain and
test generation after target cleanup as well as from a clean checkout.

The tracker restricts source-function coverage thresholds to handwritten source.
Read the actual metric collector before calculating scope; keep generated
provenance and source coverage separate. Broad parser parity does not establish
utility behavior on constructed graphs and unusual kinds; verify those families
with independently regenerated Go observations.

For Go slices, test nil, allocated empty, nil elements and copied backing
separately. Copying a slice header can require shared mutable backing during
construction; cloning a Rust vector changes that behavior. For caches reached
through imported owners, validate stored IDs against the cache owner's retained
graph. The caller's broader lookup context is not sufficient proof that the
cached result will remain live after that caller disappears.

## Code and contract references

Implementation examples: [borrow/retention](../crates/tsr_arena/src/refs.rs),
[brands](../crates/tsr_arena/src/scope.rs),
[publication and reentry](../crates/tsr_arena/src/lazy.rs),
[lazy position maps](../crates/tsr_arena/src/file.rs),
[panic comparison](../scripts/s04.py),
[ownership measurement](../scripts/s04_ownership.py), and
[metric scope checks](../xtask/src/tests.rs).

Use the relevant [text](design/text.md) and [ownership](design/ownership.md)
designs, [tracking contract](TRACKING.md), [divergence policy](adr/0004-the-owner-approves-baseline-divergences.md),
and [ADRs](adr/README.md).
