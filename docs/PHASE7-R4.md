# R4 — embedding acceptance progress

Status: single-file and native compiler embedding correctness captured; R4 remains incomplete.
This record tracks [R4 and architecture item 4](PHASE7-plan.md), rather than
changing their exit criteria. Benchmark and retained-memory results remain
separate obligations and are not inferred from API tests.

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
named witnesses for a subset of the lifetime obligations, not full E3 closure.
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

R4 still requires complete applicable public lifetime/E3 coverage with
release/Miri/ASan execution, final CI lanes, facade/host
documentation closure, and the specified elapsed/RSS/retained-byte/executable-size budgets
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
above; they do not establish every applicable E3 obligation or wasm trap
behavior. The complete native compiler expectation now records 13,432 variants
with an empty failure set; the actual capture remains the correctness witness.
