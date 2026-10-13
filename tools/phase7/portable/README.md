# Portable public API acceptance

The existing `tsr_testrunner` expands the pinned transpile cases, invokes an
external consumer for each module or declaration operation, and renders and
compares the same baseline bytes as the native `transpile` suite. The
`transpile-embed` and `transpile-wasm` suites use that roster but keep separate
expectations in `status/parity/<suite>.json`. This is the single-file portion of
[Phase 7 architecture item 4](../../../docs/PHASE7-plan.md). The native
`compiler-embed` route now uses the persistent public consumer foundation
described below. Its complete 13,432-variant correctness capture passes
119,615 subtests with zero failures and eight existing native skips.
`compiler-wasm` acceptance remains separate.

## Execution boundary

`protocol.rs` defines versioned NDJSON requests and responses. Source text,
filenames, emitted output, source maps and diagnostic strings travel as byte
arrays; compiler options travel as ordered JSON bytes with numeric enum values.
One request selects either module or declaration transpilation. Each response
contains emitted bytes, diagnostic records and the source snapshots needed to
render those diagnostics. Arena IDs never cross the process boundary.

`consumer.rs` performs compiler work only through
`tsr_embed::transpile_module` or `tsr_embed::transpile_declaration`. It freezes
diagnostics and their source bytes before dropping the output's retained
program. The independently linked native executable is
`tools/s10/rust-consumer`; it has its own manifest and lockfile. Public
host/value crates are allowed dependencies. Direct compiler, checker,
transpiler, test-runner and `s10_corpus` dependencies or imports are prohibited,
including in tests. Compiler dependencies reached transitively through
`tsr_embed` are expected. The source and manifest guard is
`scripts/tests/test_phase7_consumer_boundary.py`.

This is an entry-point/dependency audit, not a capability restriction on
reexported types. The facade intentionally exposes the production compiler-level
`Program` and `QueryOperation` APIs, their returned handles and public host/value
contracts. The shared walker names those types through `tsr_embed`; it still
executes their production methods. The facade README documents the
[exposed surface and compatibility limits](../../../crates/tsr_embed/README.md#compiler-level-api-boundary).
Passing this audit proves neither a narrow opaque API nor independent facade
semver stability.

PR CI builds each consumer once, then runs `compiler-embed` in four shards and
`compiler-wasm` in eight; each transpile roster remains one shard. The existing
merge rejects missing/duplicate shards or variants. Embed jobs have a 60-minute
timeout (45 for execution), wasm jobs 90 minutes (75 for execution), leaving
time to publish partial results. The shared build jobs also have 90-minute
timeouts. These bounds replace the former unsharded 39/254-minute roster runs;
the new CI wall time still needs observation.

The wasm build runs browser smoke once, with a 15-minute limit. Browser binaries
and npm downloads use an exact OS/architecture/Playwright/toolchain cache key.
Fresh runners still install OS libraries with `install-deps`; a cached browser
does not supply system packages. Cache hits skip the three browser downloads.

The runner reconstructs source-only frames from returned snapshots so its
existing diagnostic writer can render filenames, spans and source lines.
These frames contain source metadata and text; the host does not parse, bind,
check or emit a missing observation. Case expansion, baseline composition and
comparison remain in the host. Consumer failure, invalid protocol, unavailable diagnostic sources and the
single-file protocol's unsupported mapped/supplemental sources fail the
observation. There is no native compilation fallback.

Historical S10 corpus captures use `s10_corpus`'s separate
`legacy-rust-consumer` binary. S10's build/capture scripts retain that protocol;
the new independently manifested public consumer serves this Phase 7
single-file protocol. The legacy internal adapter is not a public API
acceptance witness.

`wasm.mjs` hosts the same byte protocol through the production checker-enabled
`tsr_wasm` artifact in Node. `wasm-consumer` is its executable shim and fixes
Node's stack size at 4096 KiB. `TSR_WASM_ARTIFACT_DIR` must name a directory
containing `checker_bg.wasm` and `bindings.mjs`; an artifact path is explicit
input, never a signal to substitute the native consumer. The shim and Rust
consumer are transport adapters, not independent comparators.

## Persistent compiler foundation

The external native consumer also accepts `--compiler`. Its versioned
`compiler_protocol.rs` commands are `compile`, `observe_types_and_symbols`,
`assertions`, and `release`, keyed by compilation ID. `compiler_consumer.rs`
loads and checks a pre-emit session, then loads a fresh post session with the
same file cache, emits, and checks again. It retains both source graphs until
release. The host chooses diagnostics using the existing count-mismatch rule.
The child returns diagnostic records, source snapshots, actual program facts,
ordered write metadata, source-map records, and resolution trace inputs.

The shared walker runs type observations for all selected files before symbol
observations, under one public compiler-operation scope. Each owned row batch
requires a host callback reply before proceeding, so host decoration can stop
a domain at the same point as the native path. Shared assertion observers use
the same public operations and AST value views. No checker handle crosses the
wire, and the host keeps baseline rendering and comparison.

Content-mapper fixture execution stays in the host's production mapper host.
The child injects a public `Project` proxy through the embedding facade; nested
callback frames relay identities, options diagnostics, transforms and typed
errors. The fixture project and host close immediately after compilation,
before later query commands, matching the native scope. The consumer has no
test mapper dependency. Missing callbacks, malformed replies and unavailable
source identities fail the observation. Physical files, decoded bundled files,
and symlinks arrive explicitly in host order: physical inputs, symlinks, then
test libraries. Loaded bundled assets use the public production
`BundledFs::with_libraries` overlay, preserving exact prefix/case lookup,
directory/stat/realpath policy, ordered entries, and loaded BOM bytes. Missing
snapshot libraries stay missing; no built-in assets are substituted.
Acceptance fixture version normalization decorates the public
build-info reader in this tool, outside production loading policy.

The executor and DTO modules are tool acceptance code. A production wasm API
must remain independent of them; linking or executing this native foundation
does not certify production wasm or browser behavior. The complete native
compiler embedding capture is retained at
`/Users/cristian/git/ts-rust-benchmarks/2026-10-11/r4-compiler-embed` with
`partial: false`, exact counts and retained executable identities. It exercises
every existing domain; its eight skips are shared native output rules, not
unsupported consumer operations. The concurrent correctness run supplies no
performance or retained-memory budget evidence.
The native and embedding routes share the existing baseline writers for errors,
content mapping, JavaScript/declarations, source maps and map records, resolution
traces, types/symbols, union ordering, and parent pointers. Child failures are
reported through those subtests and cannot select the native backend.

## Test-only wasm compiler consumer

The owner decision on 2026-10-11 permits a separate test-only wasm compiler
consumer of public `tsr_embed`. The crate in `wasm-compiler` shares the exact
native `compiler_consumer.rs`, protocol, codec, assertion and walker modules.
Its synchronous JavaScript import emits a callback frame and reads the reply
while the Rust operation is suspended. Compilation owners persist until
`release`; an error or trap terminates the child, and the next variant gets a
fresh instance. There is no native fallback.

Build this separate artifact into a new directory outside `target`:

```sh
python3 tools/phase7/portable/wasm-compiler/build.py --wasm-bindgen "$TSR_WASM_BINDGEN" --output /tmp/tsr-r4-compiler-wasm-artifact
TSR_WASM_COMPILER_ARTIFACT_DIR=/tmp/tsr-r4-compiler-wasm-artifact tools/phase7/portable/wasm-consumer --compiler
```

`wasm-consumer --compiler` selects `compiler-wasm.mjs` and requires the explicit
`TSR_WASM_COMPILER_ARTIFACT_DIR`. The artifact records
`performance_eligible=false`; it cannot satisfy production performance budgets.
Production `tsr_wasm` browser and single-file transpile artifacts remain separate
acceptance subjects. Creating the test artifact does not establish full
`compiler-wasm` corpus parity.

## Run the suites

From the repository root, list the native and portable rosters:

```sh
python3 scripts/parity.py list transpile
python3 scripts/parity.py list transpile-embed
python3 scripts/parity.py list transpile-wasm
```

Listing needs the runner but never builds or executes a consumer. Native
embedding runs build the independent consumer in release when `--consumer`
is omitted, using Cargo's reported executable path:

```sh
python3 scripts/parity.py run transpile-embed --jobs 1 --output /tmp/tsr-r4-transpile-embed
python3 scripts/parity.py check transpile-embed --results /tmp/tsr-r4-transpile-embed
```

`--runner PATH` selects a prebuilt runner. It does not bypass consumer
selection; pair it with `--consumer PATH` to use an already built native
consumer. Embed captures require its `--identity` response to identify the
embed runtime before hashing the executable. Wasm captures verify the launcher
and transport, artifact bytes and ABI, and record the actual Node executable,
version, command-line flags and inherited `NODE_OPTIONS`. The identities must
agree before and after capture. A wasm run always requires an explicit `--consumer` and never builds
the native consumer. Set `TSR_WASM_BINDGEN` to the CLI matching the version in
`tools/s10/toolchains.json`, then build and run:

```sh
python3 scripts/s10_build.py --checker --wasm-bindgen "$TSR_WASM_BINDGEN"
TSR_WASM_ARTIFACT_DIR="$PWD/target/s10/checker" python3 scripts/parity.py run transpile-wasm --consumer tools/phase7/portable/wasm-consumer --jobs 1 --output /tmp/tsr-r3-transpile-wasm
python3 scripts/parity.py check transpile-wasm --results /tmp/tsr-r3-transpile-wasm
```

The native compiler route uses the same independent executable in persistent
`--compiler` mode. Its backend selection matches the pinned test harness:
only explicit `incremental: true` selects the incremental wrapper. Public
`tsr_embed::Session` continues to honor composite configuration as incremental.
For a bounded capture with a prebuilt runner and consumer:

```sh
python3 scripts/parity.py run compiler-embed --runner target/debug/tsr-testrunner --consumer tools/s10/rust-consumer/target/debug/s10_rust_consumer --jobs 1 --id compiler/2dArrays.ts --output /tmp/tsr-r4-compiler-embed
```

Use `--id VARIANT` for a development capture. Such partial captures cannot be
checked or accepted as a suite expectation. Full transpile captures exercise both
transpile kinds for each applicable native case; compiler captures use the
compiler roster and its existing baseline domains. `check` and `accept` consume
recorded results without building a runner or consumer. Acceptance requires
review of the actual capture; adding a suite does not establish parity.
Per-variant timeouts terminate the runner and its consumer descendants. The
process-group cleanup has a POSIX regression witness; its Windows `taskkill`
path requires separate execution evidence.

## Focused checks

```sh
python3 -m unittest discover -s scripts/tests -p test_parity_embed.py
python3 -m unittest discover -s scripts/tests -p test_phase7_consumer_boundary.py
cargo test --manifest-path tools/s10/rust-consumer/Cargo.toml --bin s10_rust_consumer --test lifetime --test services
cargo test -p tsr_embed --test transpile
cargo test -p tsr_embed --doc
node --test tools/phase7/portable/test-wasm.mjs
```

The external lifetime tests cover their named public routes: program creation,
checking, scoped queries, in-memory emit, retained results after session drop,
retirement, reentry, native unwind invalidation and recovery, foreign emit
targets, concurrent initialization, malformed source bytes, and transpile
ownership/cancellation. The [R4 ownership applicability map](../../../docs/PHASE7-R4.md#public-ownership-applicability)
relates every original fourteen-case witness and the two additional snapshot-edit
and retained-result/AST cases to S06/S07/S09. A separate public service witness
covers canonical/supplemental content-mapper retention. The original fourteen
cases have debug/release/Miri/ASan evidence. The two new lifetime cases and the
supporting service case also pass all four modes, with a separate twelve-execution
receipt recorded in the applicability map. This is complementary evidence, not
a new full-roster capture.
These ownership tests do not establish compiler corpus acceptance, browser/trap
behavior or performance/retained-memory/size budgets. The full native corpus
capture above supplies the separate compiler correctness witness.
