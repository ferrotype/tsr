# S10: WebAssembly and Rust embedding

Policy update, 2026-09-20: [ADR 0020](adr/0020-phase-0-gate-decision.md)
changes the parser-size, throughput and Node-latency limits to 0.30, 1.5 and
0.40 respectively. This original implementation plan and its initial results
retain the former limits as history. The owner accepted these historical
results for the dated [S12 closure](S12-closure-plan.md); normal current-source
evidence checks remain unchanged.

Original status: interfaces and full correctness/ownership captures implemented; three
performance gates remain unmet. See [S10-results.md](S10-results.md). Base:
S09 `07156df`. Branch: `codex/s10-wasm-embedding`. The shared checkout is used
directly. S09 was subsequently merged; the S10 PR targets `main`.

## 1. Contract and authority

Implement S10's four items without changing its acceptance thresholds or the
frozen S08 denominator. E7 requires a portable wasm32 parser and checker,
complete frozen E2 type/diagnostic parity, a parser artifact at most 0.25 times
the pinned Go GOOS=js artifact, and parse throughput at least twice Go's. E8
requires full subset parity from a separate Rust consumer, applicable ownership
checks, and Node in-process parse-and-encode latency at most 0.10 times the real
`--api` socket path on a 10 KB input. The ledger remains the threshold authority.

The pin, native E2 requests, query schedule, diagnostic selection, baseline
comparators, and informational exclusions remain authoritative. A wasm trap,
unsupported host operation, or missing result is a failed observation, never a
disabled test. S10 does not implement the later full-compiler/emit wasm phase.

Existing components to reuse:

* `tsr_vfs::MemorySnapshot` and `tsr_bundled::BundledFs`: all files, libraries,
  symlinks, directory enumeration, and package metadata supplied in memory.
* `tsr_compiler::Program` / `FileCache`: loading and exclusive parse/bind.
* `tsr_checker::CheckerOwner` / `Operation`: generation validation, exclusive
  operation scope, retained results, reentry rejection, panic retirement.
* `tsr_encoder`: protocol-8 output, source indexing, lazy JSDoc materialization.
* S08's pure Rust corpus executor, native baseline walker, and comparators:
  reuse these with an explicit embedding construction policy; do not copy the
  checker or build a second test-only type-query API.

## 2. Public boundaries and ownership

`tsr_embed` supplies a Rust session around an immutable program and its checker.
Loading takes explicit program options and host inputs. Checker initialization
is lazy and fallible; query scopes borrow the session's checker owner. Retained
type/symbol/signature results use the existing owner-carrying handles. A handle
must keep its exact program/checker alive and be rejected by a different owner.
Dropping a session cannot revoke a valid separately retained result; retiring a
generation must reject all subsequent operations. No per-node `Arc`, callback
under a publication lock, unchecked identity conversion, or copied type graph.

Parser-only use must not depend on the checker, bundled libraries, or the project
service. `tsr_wasm` therefore has a parser build and an explicit checker feature;
the artifact-size numerator is the actual shipped parser build. Both artifacts
and their feature lists are recorded. The JS boundary accepts byte buffers for
source text, preserving WTF-8/invalid-byte behavior. Results are owned bytes or
explicitly disposable sessions, never borrowed views into movable wasm memory.

Start with wasm-bindgen for the safe JS ABI and generated ownership glue. Pin
the library and CLI to the same version and record their licenses/MSRV. Do not
handwrite unsafe pointer/length exports merely to avoid glue. The Node adapter
initially uses this parser module; a native Node-API leaf is a measured fallback
if the in-process gate requires it, not a premise of the size comparison.

Keep JSON request adaptation and baseline decoration in `tools/s10`, outside
the production embedding API. The external Rust consumer owns its own Cargo
workspace and lockfile and depends on `tsr_embed` by path. It must load and query
through that API. Sharing the existing baseline visitor is allowed; secretly
using the old native executable as the consumer is not.

## 3. Platform audit and failure behavior

First compile the existing dependency closure for wasm32 and investigate actual
errors before changing architecture. In particular:

* Parser entry currently spawns a reserved native-stack worker. On wasm it must
  execute synchronously on the invoking instance; it cannot spawn a thread.
  Native worker behavior and unwind payloads must stay intact.
* Inspect `stacker`/`psm` target support and execute recursion fixtures. A native
  stack reservation is not a wasm stack guarantee. Record linear-memory stack
  size and JS engine version, and test both deep grammar and post-parse visitors.
* Audit `usize`/`isize`, atomics, thread identity/TLS, locks, hash randomness,
  filesystem entry points, clocks, and allocator imports at the actual target.
  Preserve Go width semantics rather than silencing narrowing errors.
* Native sessions keep the existing unwind/retirement contract. A trapping wasm
  instance cannot promise Rust destructors or `catch_unwind` recovery. Its JS
  wrapper must become terminal and discard the instance. A fresh instance must
  work afterward; no result from the failed instance may be reused.
* Enumerate wasm imports. The compiler instance receives only memory-backed
  host state and JS ABI support; the surrounding test runner may read manifests
  and write results. No WASI, filesystem, process, or network import is accepted
  as a substitute for the portable host.

## 4. Implementation sequence

### P0: freeze interfaces and obtain a runnable target

1. Record the plan and review below. Install the pinned wasm standard library.
2. Build the parser/compiler closure for wasm32; resolve concrete target issues.
3. Add parser-only `tsr_wasm`, JS loader, and native `tsr_embed` session API.
4. Run a tiny TS and JS parse/encode comparison against native protocol output,
   including non-ASCII text and lazy JSDoc. Establish actual import inventory,
   byte ownership, explicit disposal, repeated calls, and terminal trap behavior.

Exit: real wasm execution, matching bytes, native embedding operation/retention
tests, and a documented remaining portability list. A successful compile alone
does not satisfy S10-1.

### P1: independent Rust consumer and ownership

1. Add `tools/s10/rust-consumer` as an isolated Cargo workspace. Use a shared
   corpus construction policy so loading and checker initialization pass
   through the public session without moving initialization outside existing
   checkerbench intervals or changing S08's default execution path.
2. Run representative frozen requests: multiple files, libraries, JS/JSDoc,
   declaration diagnostics, symlinks, ordered `paths`, and recursive displays.
3. Run the full frozen acceptance set and informational set. Compare the same
   type/symbol/error/public-display observations as E2; record per-domain counts.
4. Apply E3 scenarios that exercise the embedding boundary: repeated
   create/query/drop, retained results outliving a session, wrong-owner handles,
   reentry, retirement after panic, lazy caches, and program replacement.
   Use counters and `Weak` reachability, not process RSS, to assert disposal.
5. Run applicable tests under Miri and ASan with existing instrumentation rules.
   Native thread contention is not claimed as a wasm capability.

Exit: reproducible external consumer parity and concrete lifetime observations,
not booleans derived from successful compilation.

### P2: portable checker corpus

1. Add the checker feature and memory-backed request adapter to `tsr_wasm`.
2. Use the same construction policy and walker as P1; compile no native runner
   or allocation/profiling instrumentation into the wasm instance.
3. Run representative requests, then all frozen acceptance variants. Preserve
   ordered maps in request serialization. Preflight the complete request hash
   inventory before any long capture; exercise malformed-request rejection.
4. Report traps/failures by variant, stage, and runtime. Verify counts, request
   identities, output hashes, source stability, and wasm imports at replay.

Exit: complete portable-host and checker-parity evidence. A native comparison
cannot substitute for a wasm result or erase a wasm stack failure.

### P3: paired parser experiment (E7)

1. Build a small pinned-Go parser entry point with `GOOS=js GOARCH=wasm`, using
   the exact toolchain's `wasm_exec.js`. Include equivalent parser/encoder
   capabilities on both sides; exclude checker and native command-line code.
2. Freeze parser inputs and operations under `tools/s10`. Compare node/diagnostic
   and encoded output observations before timing. The timed operation parses a
   fresh source each iteration; it cannot return a previously cached AST.
3. Record raw `.wasm` byte size for both release artifacts, flags, features,
   toolchain/glue hashes, and optional compressed sizes as informational only.
4. Measure warmed parsing on one Node version/host in interleaved runtime order.
   Keep initialization, fixture I/O, and baseline decoration outside intervals;
   include equivalent allocation/lifetime work, account for Go GC, and retain
   raw samples. Use the repository's bootstrap/stability conventions. Read
   thresholds from the ledger; never silently substitute native Go timings.

Exit: replayable measured size and throughput, whether passing or failing.

### P4: Node adapter/socket comparison (E8)

1. Freeze an exactly 10,240-byte TS fixture and pinned parse options. Compare
   encoder bytes before timing, including source name/hash and lazy nodes.
2. Exercise the real pinned `--api` server. Its API exposes project snapshots
   and `getSourceFile`, not a standalone parse RPC. Define and record the
   shortest real request sequence that forces a fresh parse then encoding;
   include required update/create requests in the socket interval. Never time
   a cached `getSourceFile` response and label it parsing. Record unavoidable
   loading/binding work so the ratio's scope is explicit.
3. Time the in-process parse-and-encode call from JS including ABI transfer and
   result ownership. Exclude server/module startup on both sides; measure it
   separately if useful. Alternate samples, verify output outside intervals,
   record sample sizes and confidence/stability results.
4. If wasm misses the gate, attribute transfer/parser/encoder cost with a bounded
   diagnostic before deciding on the native Node-API fallback. No threshold
   change or claimed completion based on a projection.

### P5: producers, CI, and closure

Register `e7` and `e8` with exact source/toolchain/manifest inputs and strict
capture replay. Include generated JS, dependency locks, Go entry points, host
adapters, comparators, and threshold ledger in fingerprints. Invalid, missing,
partial, or stale captures withhold metrics. Producer tests cover source drift,
missing rows, zero/negative timings, output mismatches, reordered requests,
foreign handles, and mismatched artifacts.

CI builds wasm and the outside consumer, executes bounded correctness and
ownership tests, and uploads failed observations. Long measurements remain
explicit captures, not unbounded work on every CI runner. Run MSRV, deny, fmt,
Clippy, focused native regressions and the workspace checks warranted by actual
changes. New dependencies can stale S07/S08 evidence; preserve it and refresh
required evidence at the final stable revision rather than weakening freshness.
Regenerate status, run `check S10`, and report each unmet gate explicitly.

## 5. Review before implementation

Reviewed against S10, E7/E8, ADRs 0003/0012/0016/0017, the Rust guide, and the
current parser worker, memory host, checker owner, encoder, and S08 executor.

Resolved planning issues:

* Separate artifact measurements from checker correctness: stripping the
  checker for the size numerator must not strip it from the corpus run.
* Keep checker construction inside the measured initialization hooks when the
  corpus executor gains an embedding policy; eager construction would corrupt
  existing checkerbench accounting.
* Keep external-consumer evidence honest: an isolated manifest plus genuine
  public API construction is required, not an example in the root workspace.
* Treat stack support and panic retirement as target-specific contracts, and
  verify imports/execution rather than equating a successful build with a
  portable host.
* Verify every frozen request before expensive captures. Ordered `paths` and
  config maps are load-bearing, as the S08 canonicalization failure demonstrated.
* Do not invent a standalone socket parse endpoint. The measured sequence and
  any extra native work must be visible in the E8 record.

Open engineering risks, with bounded decision points: wasm stack/32-bit behavior
at P0/P2, parity runtime at P2, parser artifact/GC cost at P3, and ABI/socket cost
at P4. None authorizes a smaller acceptance corpus or a changed threshold.

## 6. Implementation decisions and reproducible commands

The implementation uses `tsr_embed` with a parser-only base and a default
`checker` feature. `tsr_wasm` builds three closures: shipped parser, shipped
checker, and a capture-only `corpus` feature that adds the existing S08 walker.
The corpus executable and the shipped checker are both exercised; a successful
walker run does not stand in for testing the public `MemoryHost` API.

Dependencies: wasm-bindgen/CLI 0.2.126 generates the JS ABI; xxhash-rust 0.8.18
implements the pinned protocol header's XXH3-128; napi 3.12.2, napi-derive 3.6.3
and napi-build 2.4.1 provide the native Node leaf. They avoid handwritten FFI.
Cargo.lock deliberately retains napi-derive-backend 6.1.2: 6.1.3 changed its
convert_case argument type incompatibly with the pinned derive crate. The ABI
macros require syn 2.0.119 while the existing serde macros use syn 3.0.5. The
exact syn-2 build-time exception in deny.toml is recorded here rather than
rolling back existing serde or broadly allowing duplicate versions. Runtime
crate duplication remains denied. MSRV, license and advisory checks still apply.

The parser throughput workload is S07's complete frozen VS Code source set:
13,094 files, 161,740,237 raw bytes, the recorded script kinds and external-module
indicator options. Every file is hash-checked, and each runtime must produce the
same counts and protocol bytes before timing. There are no BOM files in that
inventory; the runner asserts this instead of timing different loading policies.
The separate 10,240-byte Node input remains `tools/s10/parser/ten-kib.ts`. It is
synthetic and dense; it is not substituted with a more favorable file after
seeing results. The socket interval is updateTemporarySnapshot, getSourceFile,
and release; it includes loading/binding and JSON/base64 transport beyond the
native adapter's parse-and-encode operation.

Both wasm artifacts exclude name/producers custom sections. Rust uses the
workspace release optimization, fat LTO, and panic=abort; the 16 MiB initial
linear stack is recorded in toolchains.json. Native embedding keeps unwinding.
The pinned stacker/psm support wasm linear-stack segments; this does not extend
the JS engine's independent call-stack limit. Exact wasm imports are limited to
two parser or three checker string/exception-reference ABI functions; new imports fail the
portable check until reviewed. No filesystem/network/process import is allowed.

Run setup/builds before captures (the wasm-bindgen CLI path is explicit):

```sh
python3 scripts/s10_build.py --wasm-bindgen /path/to/wasm-bindgen --wasm-opt /path/to/wasm-opt
python3 scripts/s10_build.py --checker --wasm-bindgen /path/to/wasm-bindgen
python3 scripts/s10_build.py --corpus --wasm-bindgen /path/to/wasm-bindgen
python3 scripts/s10_build.py --consumer
python3 scripts/s10_build.py --node
python3 scripts/s10_go.py
cargo build --locked --release -p tsr_wasm --example parser
python3 scripts/s10_measure.py capture --kind portable --output target/s10/portable
python3 scripts/s10_corpus.py capture --runtime wasm --output target/s10/wasm-corpus
python3 scripts/s10_corpus.py capture --runtime rust --output target/s10/rust-corpus
python3 scripts/s10_lifetime.py capture
python3 scripts/s10_measure.py capture --kind parser --samples 21 --output target/s10/parser-measurement
python3 scripts/s10_measure.py capture --kind node --samples 21 --output target/s10/node-measurement
cargo xtask run e7
cargo xtask run e8
```

Do not compile or run other tests during timed batches. `--smoke` (corpus) and
`--limit` (parser) are diagnostic and cannot emit acceptance metrics. A final
21-sample capture retains all interleaved samples and warmup; a seven-sample
screen is also supported. A nominal passing median must satisfy the existing
bootstrap upper-bound and 5% relative-MAD rules; otherwise that timing metric is
withheld. A measured failing ratio remains visible as a failure. The Go parser's
final GC is included in each batch to account for AST disposal, as Rust drops
its AST after each call. No independent component savings are added together.

Ownership uses the external consumer's exact six-case inventory in
`tools/s10/lifetime-cases.json`, under debug, release, Miri and ASan. It covers
lazy checker initialization/caches through actual semantic queries, repeated
owner/allocation return to baseline, a retained type surviving its session,
wrong-generation imports even over the same program, reentry, explicit
retirement, panic retirement and concurrent initialization. A replacement
session is a distinct generation. Internal fault injection, project-service
replacement and socket-queue contracts remain in their existing E3 suites;
this API exposes no mutable project or request queue. The portable API separately
checks instance/session disposal and terminal traps, not native unwind/thread
behavior.

Further boundary review fixed two concrete lifecycle issues before full capture:
wasm-bindgen finalizers are explicitly detached after a trap, and exported JSON
uses owned byte arrays so generated string-return cleanup cannot reenter a
failed instance. The import inventory and generated detach method are bound to
the pinned ABI version. Dependency review also records a version-specific
BSL-1.0 allowance for xxhash-rust 0.8.18; its Boost license text is permissive and
requires retaining notices in source distributions.

The native Node adapter also offers a disposable `Parser` object with a
persistent reserved-stack worker. Each synchronous request owns its inputs and
AST; close/drop disconnects and joins the worker. The benchmark measures this
object, including the per-request channel/buffer transfers, and excludes its
initialization just as it excludes API-server initialization. A separate
four-lifecycle JS test checks both convenience and worker APIs on all ten parser
fixtures, rejects calls after close, and keeps all forty output buffers valid
after worker disposal. Initial standalone/worker microbenchmarks are diagnostic;
the paired capture, not cross-batch ratio arithmetic, decides the E8 result.

The final parser release adds Binaryen 132 `wasm-opt -Oz` after wasm-bindgen.
Published archive checksums for the four supported native build hosts are pinned
in toolchains.json and provisioned by `scripts/s10_toolchain.py`. The original
Rust module, pre-optimization module and shipped module are all authenticated.
The seven-sample full-workload screen measured 1.749× throughput and 0.268×
artifact bytes for the combined configuration; LLVM opt-level 2 plus the same
post-link pass measured 1.706× and produced a larger artifact, so it was rejected.
These are diagnostic screens; final metrics come from the recorded 21-sample
capture. No gate values changed.

The default Node engine stack completed 9,367/9,369 acceptance variants; the
remaining two (`binderBinaryExpressionStressJs` and `largeControlFlowGraph`)
trapped in subtree-fact traversal and checker flow analysis. Both execute with
Node's engine stack set to 4096 KiB. This host requirement is now explicit in
toolchains.json and recorded for every capture, separate from the 16 MiB wasm
linear stack. The final full capture proves all 9,369 acceptance cases under
that configuration; see the results record. Default-browser execution of these
stress cases is not certified by this result.
