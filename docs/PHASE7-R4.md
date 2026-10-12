# R4 — embedding acceptance progress

Status: full native compiler/transpile correctness and public ownership checks
captured; R4 remains incomplete because its resource budgets are not met.
This record tracks [R4 and architecture item 4](PHASE7-plan.md), rather than
changing their exit criteria. Benchmark and retained-memory results remain
separate obligations and are not inferred from API tests.

## Public boundary contract

The facade intentionally exposes a compiler-level API, as architecture item 4
allows: public programs, scoped checker operations, retained results and host
services. It does not provide an independently stable opaque wrapper around
those types. `QueryOperation`, requests, errors, `TypeRef`, program/value types
and transpile values alias their production definitions; their public methods
and concrete return types are consequently part of the exposed surface.
The [facade README](../crates/tsr_embed/README.md#compiler-level-api-boundary)
enumerates the families and lifetime, cancellation and compatibility limits.

Program and checker-owner access predates R4: before this work, `Session`
already returned `Arc<Program>`, `Arc<CheckerOwner>` and `Operation`, and retained
results exposed their owner. The new aliases make those types nameable without
direct implementation-crate dependencies. Compiler-pool diagnostics/queries,
incremental services and single-file transpilation add actual capabilities.
Direct owner operations retain exact-owner/generation checks; they do not apply
the session wrapper's phase schedule or combined pool unwind policy.

The consumer audit proves imports/dependencies route through the declared
public facade and allowed host/value crates, with no direct internal fallback.
It does not prove an opaque or smaller capability surface: an alias retains
all public methods of its implementation type. Public facade and implementation
API compatibility must therefore be reviewed together before release.
The acceptance and lifetime receipts cover the consumer's executed operations
and obligations; they are not exhaustive public-entry-point tests for every
method reachable through a reexported program, operation or builder.

## Public transpile operations

`tsr_embed` exposes production `transpile_module` and
`transpile_declaration` operations under its default `checker` feature:

```rust,ignore
fn transpile_module(
    request: &TranspileRequest,
    input: &[u8],
    options: &TranspileOptions<'_>,
) -> Result<Option<TranspileOutput>, TranspileError>;
```

The declaration operation has the same signature. Request, options, output
and error types alias the production types; no transpilation algorithm or
port marker is duplicated. The output owns emitted text and source-map text
and retains the program needed to resolve diagnostic file identities. Dropping
input/options does not invalidate it; cloning a diagnostic alone does not
retain that program. Module cancellation returns `Ok(None)`; declaration
transpilation preserves the pin's forced-emit behavior and skips that gate.
Parser-only builds omit the transpile dependency and surface.

`QueryError` and `SessionError` expose the existing error value types for
public lifetime assertions. Session query results follow the established
scoped-operation/explicit-retention contract. Retirement invalidates future
queries and emits, including queries through retained results; dropping a
session alone does not revoke explicitly retained query results.

## Whole-program facade observations

The public session now provides compiler-pool diagnostic phases through
`diagnostics(SessionRequest, DiagnosticKind, target)` and scoped queries through
`with_compiler_operations`. The scope holds every compiler checker and selects
a file's actual checker with `for_file`; diagnostics, emission and these queries
therefore share compiler state. The existing `operation()` keeps its independent
interactive checker. The new scopes reject session reentry and retire both
checker roots when a callback unwinds. Semantic-check cancellation preserves
the native checker's poisoned state when the phase reaches a checker, separately
from retirement; incremental gates can return before checking.

`emit_with_request` exposes the production cancellation result. `EmitOutput`
preserves the emitter's source-map records and ordered callback metadata;
callback records index the existing emitted files instead of copying their
bytes, and explicitly retain any unlisted callback output. The output now
retains its program for diagnostic and metadata source identities after session
drop. Its retention ends when the output and any clones are released.

Configured session loading now wraps the production incremental program when
`incremental` is enabled, including previous build-info reads and build-info
write metadata. `SessionServices` accepts a caller-owned mapper project and a
construction-time build-info reader; production mapper host/spawner contracts
are available through `tsr_embed::content_mapper`. Plain `from_program` adoption
is preserved; configured adoption is explicit and fallible. Production version
validation is unchanged. The live package publish order places incremental
before embed, and its dependency graph/order check passes.

Five separate external `services.rs` tests were added for build-info reuse,
version rejection/plain adoption, canonical and supplemental mapped sources,
production spawner failure reporting, cancellation and query ownership. All
five passed along with the unchanged 14 external lifetime tests. The mapped
source witness requests declarations: production suppresses JavaScript output
for content-mapped sources. External consumer clippy passed with warnings
denied. The embedding crate's 17 integration tests and one doctest passed;
focused embedding crate clippy also passed with warnings denied after the
concurrent printer lint and one facade style lint were corrected. These methods supply the API
foundation used by the bounded compiler integration below; they do not by
themselves establish compiler-suite acceptance.

## Independent consumer and suite foundation

The separately manifested `tools/s10/rust-consumer` compiles through public
`tsr_embed` operations and no longer delegates the acceptance path to
`s10_corpus`. Its test dependencies obey the same boundary. Manifest and
source guards reject direct compiler/checker/transpiler/test-runner imports
and dependencies, including renamed or target/dev/build dependencies and
shared modules included by the consumer. Compiler crates behind `tsr_embed`
remain expected transitive dependencies.

The historical S10 corpus protocol remains available through the separate
`s10_corpus` package binary `legacy-rust-consumer`
(`tools/s10/corpus-adapter/src/bin/legacy-rust-consumer.rs`). Its S10 build and
capture scripts select that binary. The new public consumer serves the Phase 7
single-file protocol, so changing its request/response boundary does not replace
or break the old S10 protocol. The legacy internal adapter is not an R4 public
API acceptance witness.

The existing runner owns case expansion, rendering and comparison.
`transpile-embed` and `transpile-wasm` select an external executable and use
the native transpile roster, whose current expectation contains 28 variants.
Each portable suite has its own expectation file. Returned diagnostics carry
owned source snapshots; the host reconstructs source-only frames for the
existing diagnostic renderer and never compiles a missing result on behalf
of the consumer. Unsupported protocol/source cases and child failures fail
the observation. The native `compiler-embed` route and full-roster correctness
capture are recorded below.

The [portable adapter README](../tools/phase7/portable/README.md) records the
protocol, native/wasm commands and checks. Wasm runs require an explicit host
executable and artifact directory. List/check/accept never build a consumer;
native embedding runs may auto-build the independently manifested consumer.
An explicit runner does not bypass consumer selection. Process deadlines
also terminate external consumer descendants on the tested POSIX route.

## Focused witnesses and remaining work

The actual local full-roster captures on 2026-10-11 used pin
`1f70213d4922b434345f639b441681e470c7cfc1` and the existing baseline comparator:

| Suite | Variants | Passing subtests | Failing | Skipped | Retained capture |
| --- | ---: | ---: | ---: | ---: | --- |
| `transpile-embed` | 28 | 41 | 0 | 0 | `/Users/cristian/git/ts-rust-benchmarks/2026-10-11/r4-transpile-embed` |
| `transpile-wasm` | 28 | 41 | 0 | 0 | `/Users/cristian/git/ts-rust-benchmarks/2026-10-11/r3-transpile-wasm` |

The embedding capture invokes the independently linked public consumer; the
wasm capture invokes the committed Node shim over the actual production
checker-enabled wasm artifact. Each capture is complete (`partial: false`),
with its own `meta.json`, `results.ndjson` and local baselines retained outside
`target`. Each suite exercises 19 module and 22 declaration subtests. Both
expectation files now record 28 variants with no failures. These are
correctness captures, not elapsed/RSS/retained-byte or size budget measurements.
Their concurrent jobs and debug native consumer must not be used as R4
performance acceptance evidence.

The public transpile integration tests cover JavaScript and declaration
operations, option overrides that differ from whole-program emit, source-map
bytes, diagnostic ownership, and the pin's cancellation asymmetry. They passed
locally (four tests). The external lifetime tests independently exercise
create/check/query/emit/drop, retained query roots, retirement/reentry, native
query unwind invalidation and fresh-session recovery, foreign emit targets,
concurrent initialization, malformed bytes, and transpile results. These are
named witnesses whose public applicability is mapped below.
All 14 external public lifetime tests passed locally, including source-map
metadata/inline maps, differing callback and emitted-file order, shared compiler
state, positive suggestion/declaration diagnostics, and compiler-scope
reentry/cancellation/panic cases. The crate's 17 integration tests and one
doctest passed, as did focused crate and external-consumer clippy with warnings
denied. Seven native wasm
schema tests also passed, validating production serialization against the
portable DTO/public consumer; those native tests do not themselves execute
wasm. The separate full-roster wasm capture above supplies actual wasm
execution for this single-file corpus.

Fake-command script tests cover portable/native roster identity, explicit
consumer routing, Cargo artifact selection, native/wasm expectation isolation,
missing consumers, consumer failures without fallback, and timeout cleanup of
actual child descendants. The dependency guard has injected manifest/import
failure cases. These checks validate the producer and public boundary; they
are not full upstream corpus captures or performance measurements.

R4 still requires final CI lanes, facade/host documentation closure, and the
specified
elapsed/RSS/retained-byte/executable-size budgets
or an owner-approved ADR amendment. R3's wasm/browser/trap and budget criteria
also remain independent; native tests cannot certify them.


## Full native compiler embedding correctness

The independently linked native consumer now has a tested `--compiler`
execution path with persistent compilation IDs. It mirrors pre-diagnostics,
fresh post-emit, post-diagnostics with one shared file cache and keeps both
source graphs until release. Its DTOs carry owned diagnostic source identities,
actual program facts, ordered write metadata, production source-map records,
and trace inputs. The host retains count settlement and every baseline writer.
The shared type/symbol walker and assertion observers query through the public
facade; host callbacks preserve per-domain stopping and decoration order.

A public content-mapper `Project` proxy relays fixture work to the production
mapper host on the host side. The consumer has no fixture implementation or
direct compiler/checker dependency. Shared byte codecs preserve configuration snapshots,
mapped/supplemental source identities and compiler-visible typed mapper errors.
The public incremental reader is decorated only by this test caller to match
native fixture version normalization. Production load policy is unchanged.

Focused external binary tests passed (nine total, including four compiler
executor witnesses), as did external all-target clippy and shared test-runner
all-target clippy with warnings denied. A real `--compiler` stdin/stdout smoke
compiled and released one program in one process. The boundary guard passes
five focused tests and explicitly traverses the new executor, schema, codecs,
assertion and walker modules. The paired development selection contains 18 variants from the actual
13,432-variant roster; its exact comparison precedes the full capture below.
This does not establish production wasm/browser, lifetime/E3 or budget closure.

The external compiler receives the exact ordered host snapshot. A physical-file
boundary preserves native insertion order around symlinks and test libraries.
The public `BundledFs::with_libraries` overlay applies the existing bundled
filesystem behavior to explicit ordered loaded bytes, including exact case
lookup, BOM handling, stat/directory/realpath policy and missing libraries. It
never falls back to built-in libraries. Explicit immutable overlays have distinct
cache identities; a live inner host remains uncached. Four bundled filesystem
tests, all 19 VFS tests, and focused bundled/VFS/transformer clippy passed.

The owner decision on 2026-10-11 permits the separate test-only wasm compiler
consumer in `tools/phase7/portable/wasm-compiler`, sharing the public executor,
protocol, codecs and walker with the native consumer. Its explicit artifact
and synchronous callback transport are documented in the portable README.
The test artifact is not eligible for production performance budgets; production
wasm/browser acceptance remains independent.

The final paired development captures on 2026-10-11 each passed all 159
subtests across 18 variants, with zero failures or skips and exactly equal
result rows. Retained captures are
`/Users/cristian/git/ts-rust-benchmarks/2026-10-11/r4-compiler-native-focused-v3`
and `r4-compiler-embed-focused-v3`; selection and comparison JSON live beside
them. They cover errors, content mapping, output, source maps/map records,
types/symbols, resolution trace and both assertions, including incremental
build info, config errors, symlinks and supplemental sources. Existing host
writers compare every emitted baseline against the pin; successful baseline
files are removed by that comparator. These partial captures cannot be accepted
as suite expectations and do not establish full compiler parity or performance.

The native consumer release build passed with all 1,003 local dependency and
shared tool source hashes unchanged before/after compilation. Its executable
and identity records are retained in
`/Users/cristian/git/ts-rust-benchmarks/2026-10-11/r4-compiler-embed-release-artifact`.
The build elapsed time and executable size are not production budget
measurements.

The complete native `compiler-embed` correctness capture on 2026-10-11 passed
119,615 subtests across all 13,432 variants, with zero failures and eight skips
from the existing shared native output rules. Four skips cover output filename
collisions; the others cover no emitted files, one declaration-order case, and
two type-only merge ordering cases. They do not represent unsupported consumer
operations. The capture is `partial: false`, uses the pinned baseline comparator
for every existing domain, and retains `meta.json`, `results.ndjson` and an
invocation record at
`/Users/cristian/git/ts-rust-benchmarks/2026-10-11/r4-compiler-embed`. Results
contain unique subtest IDs and match their recorded counts. The invocation binds
the retained release runner and public consumer hashes. This four-worker run is
correctness evidence only; no elapsed/RSS/allocation/retained-byte or executable
size budget is accepted from it. Full compiler-wasm acceptance remains separate.

The final public ownership capture passed all 14 named cases in each of debug,
release, Miri and AddressSanitizer (56 successful case executions). The retained
capture is
`/Users/cristian/git/ts-rust-benchmarks/2026-10-11/r4-lifetime-final`, with capture
SHA-256 `ed48d8ef6a2d98a76b181710cd74114fdcea36f95654e9af11d2fcb3d51400fc`.
These are the enumerated public lifetime, failure and release routes described
above. This historical receipt covers its fourteen-case source roster; the
additional public categories and their focused followup are mapped below.
Native ownership receipts do not certify wasm trap behavior. The complete native compiler expectation now records 13,432 variants
with an empty failure set; the actual capture remains the correctness witness.


## Public ownership applicability

The applicability audit compares [S06](../data/s06/ownership-cases.json),
[S07](../data/s07/ownership-cases.json) and
[S09](../data/s09/ownership-cases.json) with the public `Session`, `ProgramFile`,
query-result and emit contracts. Applicability concerns an exposed ownership
transition; it does not require a second facade test for each private mechanism
that implements that transition. The original suites still test those mechanisms.

| Public transition | Inventory obligation | External witness |
| --- | --- | --- |
| Source bytes, node/list/text backing and checked imports survive their owners | S06 retained node/text/imported metadata; S07 completed nodes/symbols | malformed-source, transpile-result and emitter-record tests; generated syntax below |
| Canonical and supplemental mapped sources stay alive together | S06 `storage_retained_mapped_node_keeps_both_files_auxiliary_data_and_metadata`; S07 mapped-member retention | `services::caller_owned_mapper_project_loads_and_emits_canonical_and_supplemental_sources` retains both source texts through output, then checks final disposal |
| Identical snapshots share bound-file identity; an edited snapshot cannot reinterpret old IDs | Both S07 `groups` (`shared_bound_file`, `retained_snapshot_edit`) | `public_ownership_shared_file_survives_concurrent_snapshot_edit` checks shared `Arc`, concurrent retained reads, old/new node and symbol rejection, and final release |
| Every escaped result imports only into its exact checker and keeps its dependencies | S09 `results`, `ast` | `public_ownership_retained_results_and_generated_syntax_keep_exact_owners` covers symbol/signature/type-list/node, typed AST parent/return links, transferred builder syntax and clone; existing retained-type test covers type |
| Retained access reacquires a permit; retirement closes access without freeing live roots prematurely | S09 `results`, exposed `generation`/`pool` effects | retained-type, retirement/reentry, unwind and compiler-pool tests below |
| Compiler operations and emit share the correct pool and release its owners | S09 `compiler_pool` release, wrong-owner and panic effects | shared compiler diagnostics/query test, compiler-scope panic test, emit metadata/final-drop tests |
| Concurrent first requests share initialization; reentry fails before waiting; native panic preserves payload and retires both pools | Exposed S09 `generation`/`pool` effects | concurrent-first-query, reentry, native-unwind and compiler-scope-panic tests |

The original fourteen-case receipt maps completely as follows. Names refer to
[the external test target](../tools/s10/rust-consumer/tests/lifetime.rs).

| Case | Public obligation |
| --- | --- |
| `callback_order_and_inline_maps_are_observed_independently_of_output_files` | Output ordering, callback index identity and source-owner retention |
| `compiler_diagnostics_and_queries_share_the_emit_pool` | Compiler-pool identity, retained query access, foreign files and reentry |
| `compiler_request_cancellation_preserves_native_emit_and_checker_contracts` | Cancellation remains distinct from retirement; unaffected routes stay usable |
| `compiler_scope_panic_retires_both_checker_roots` | Native panic invalidation across both public pools |
| `concurrent_first_queries_share_a_checker_identity` | First-use contention publishes one checker identity |
| `emit_refuses_a_target_file_from_another_program` | Foreign source-owner rejection |
| `emitter_records_and_write_metadata_retain_their_source_owners` | Source-map/write metadata and final output-root disposal |
| `malformed_source_keeps_original_bytes_and_resolvable_diagnostics` | Original byte backing and diagnostic node resolution while its Program is retained |
| `native_query_unwind_preserves_payload_and_invalidates_the_session` | Initiating panic payload, retirement and fresh-session recovery |
| `reentry_fails_and_retirement_invalidates_retained_queries_and_emit` | Reentry, query/emit retirement and retained-root disposal |
| `repeated_load_check_query_emit_drop_releases_tracked_storage` | Repeated request lifecycle, escaped output and final Program/checker release |
| `retained_type_survives_session_drop_and_rejects_a_foreign_session` | Exact-owner type retention and post-session access |
| `retirement_before_initialization_refuses_a_query` | Retired generation cannot initialize a checker |
| `transpile_results_retain_sources_and_follow_pinned_cancellation` | Transpile result backing, diagnostics and pinned cancellation asymmetry |

The two new `public_ownership_*` cases close the public snapshot-edit and
non-type result/AST categories missing from that roster. The mapped/supplemental
service case is included separately in the focused instrumentation followup.
The [manifest](../tools/s10/lifetime-cases.json) inventories sixteen lifetime
cases and identifies this supporting service case. Evidence is composed from the
historical fourteen-case receipt and focused new-case/service executions; it is
not described as a new full sixteen-case capture.

The complementary capture passed those two new lifetime cases and the supporting
service case in each of debug, release, strict-provenance Miri and
AddressSanitizer: twelve successful focused executions, with the compiled-source
fingerprint unchanged. It is retained at
`/Users/cristian/git/ts-rust-benchmarks/2026-10-11/r4-lifetime-public-followup-01`,
with capture SHA-256
`24db8b12f740dcdc41ac91721e4cdcc24e8ffbd6b2a6f0fda23cf0521ccea338`.
The receipt records exact test names, commands, toolchain, source revision hashes
and log hashes; `run.py` preserves the focused runner. Focused external lifetime
and services clippy also passed with warnings denied. Together with the original
four-mode receipt, this accounts for all public ownership categories identified
by the audit; the fourteen historical cases were not rerun. The two receipts
retain their distinct source fingerprints and do not certify unrelated later
implementation changes.

The following inventory mechanisms have no corresponding facade operation:
S06 exclusive construction, arbitrary storage/header replacement, validation
fault hooks, burned staged identities and injected foreign cache publication;
S07 local binding backend selection, exclusive mutating binder callbacks and
invalid staged flow/symbol graphs; S09 generation commit gates, forced retirement
between compiler files, initializer fault injection, pool idle eviction/project
replacement, API socket registry/queue commitment, decoded printer scratch and
formatting insertion scratch. Their original E3 suites remain authoritative.
Ordinary public generated syntax and mapped-source retention are included above;
these exclusions do not exclude those results merely because private factories
implement them. This audit identifies no additional public ownership category
requiring a synthetic hook or a new facade capability. Native unwind/contention
and sanitizer execution remain separate from wasm instance/trap evidence.

## First serial resource comparison

`r34-native-xstate-check-pairs-01`, under the durable
`../ts-rust-benchmarks/2026-10-11/` capture root, compares the ordinary public
embedding consumer with the pinned Go consumer at one checker. One discarded
warmup pair precedes seven fresh, serial, alternating pairs; complete owned
observations match throughout.

| Metric | Go median | Rust median | Rust / Go | Limit |
| --- | ---: | ---: | ---: | ---: |
| Execute request through acknowledgement | 0.888 s | 1.769 s | 1.9913 | 1.0 |
| Process peak before observation | 668,450,816 B | 702,726,144 B | 1.0513 | 0.70 |
| Linked consumer bytes, both stripped (corrected 2026-10-12) | 20,017,168 B | 25,920,720 B | 1.2949 | 1.0 |

Elapsed excludes preparation, checkpoint GC and later observation. The 95%
ratio interval is 1.9236–2.0390; Go/Rust relative MAD is 1.61%/1.54%. Every pair
was under background host load, so this timing/RSS capture is diagnostic and
is not entered as a qualified performance run. It establishes matching work
and gives a starting measurement, not a quiet-host acceptance claim.

The size row is separately recorded under `status/perf/embed-size`. The original
1.4436 ratio compared Go stripped by `-s -w` with an unstripped Rust release
consumer and is withdrawn as a comparable size result. Its raw record remains
historical. On 2026-10-12, `size.py` copied both retained executables and applied
the same `llvm-strip --strip-all` to both. The corrected ratio is **1.2949**,
still above the 1.0 target, recorded in
`status/perf/embed-size/2026-10-12T03-35-17.387571+00-00-a69cdeb6.json`.
The durable capture is
`/Users/cristian/git/ts-rust-benchmarks/2026-10-12/r4-native-size-stripped-04`.
It preserves the original identities, build manifests, strip-tool identity and
commands, and stripped binaries; both stripped executables launch successfully.
Rust still uses fat LTO, one codegen unit and mimalloc. No code was rebuilt, no
timing was rerun and the original timing artifacts remain unchanged. There is
no allocator instrumentation in these executables. Native size replay now
rejects captures without the matching stripping step.

The separate native allocation path uses the same Rust sources, release
profile and allocator, with allocation instrumentation added. It authenticates
the ordinary reference and checks each sample's complete work. Go's collected
`HeapAlloc`, Rust's live requested layout bytes, allocation traffic and process
RSS retain distinct names and accounting authorities. The production budget
commands and readonly `perf.py` recording route are documented in
[the budget README](../tools/phase7/budget/README.md). All captures are outside
`target`; cleaning Cargo outputs does not remove them.

The seven-pair instrumented follow-up
`r34-native-xstate-check-allocation-01` completes with identical observed work.
Retained Go `HeapAlloc` has median 559,392,448 bytes; Rust live requested bytes
have median 573,931,160 bytes: **1.0260×**, against the 1.0 retained-byte limit.
Cumulative allocation traffic is 1,588,245,328 versus 2,858,454,476 bytes
(**1.7998×**). Traffic includes preparation and is not presented as compiler-only
allocation or a retained-byte substitute. Instrumented elapsed/RSS numbers are
not used. This allocation result can accompany a future ordinary qualified
comparison only under the tool's exact source/artifact/work binding; the busy
ordinary run is not made timing-qualified by the allocation result.
