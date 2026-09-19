# S10 initial acceptance results

The embedding interfaces and correctness captures are complete on the source
tree committed as `09e6016`. S10 remains open: all three performance criteria
miss their unchanged thresholds. The plan and API contracts are in
[S10-implementation-plan.md](S10-implementation-plan.md) and [S10.md](S10.md).

The review fixes below change the source scope and adapter code. The initial
captures remain preserved historical results; they are not evidence for the
amended revision, and E7/E8 must be refreshed before claiming current acceptance.

## Recorded gates

| Criterion | Observed | Required | Result |
| --- | ---: | ---: | --- |
| E7 parser artifact bytes, Rust / Go | 0.268019 | ≤0.25 | Fail |
| E7 parse throughput, Rust / Go | 1.676912 | ≥2 | Fail |
| E7 checker parity | 1.0 | 1.0 | Pass |
| E7 portable in-memory host | true | true | Pass |
| E8 Node latency, in-process / socket | 0.341848 | ≤0.10 | Fail |
| E8 separate Rust consumer parity | 1.0 | 1.0 | Pass |
| E8 applicable consumer lifetime checks | true | true | Pass |

S10-1 and S10-3 have their required observations. S10-2 and S10-4 remain open.
The Cargo/source changes also stale the older S08 prerequisite evidence. That
freshness rule is unchanged; these S10 captures do not relabel older E1–E6
records as current. Refresh prerequisite captures at the eventual stable
completion revision, after resolving the S10 performance work.

## Correctness and ownership

Both the bare-wasm adapter and the independent Rust consumer executed all
10,728 frozen variants. Each passed all 9,369 acceptance variants: 9,369 exact
diagnostic comparisons, and 9,171 exact type/symbol and public-display
comparisons with the existing 198 native-disabled baselines. Every available
informational result also matched: 1,325 variants, with the existing 34 native
unavailable cases reported separately. No acceptance failures or differences
remain and all captures report stable sources.

The Node host is v24.20.0 with `--stack-size=4096`. Default Node completed
9,367 acceptance variants; the two remaining stress cases trapped in
subtree-fact traversal and checker flow analysis. The final capture includes
both and passes with the explicit 4 MiB engine stack. This is separate from the
16 MiB initial wasm linear-memory stack. Browser-default stress execution is
not certified. Import checks allow only the pinned string/reference ABI
functions, with no process, filesystem or network imports.

The shipped checker feature also passes the public memory-host API tests,
including independent instances, owned output buffers, disposal and terminal
trap handling. The larger corpus feature is a capture adapter and is not the
parser artifact used for the size metric.

All six external-consumer lifetime cases pass in debug, release, Miri and ASan.
They cover repeated create/query/drop with owner/allocation baseline checks,
retained results and wrong-generation rejection, reentry, retirement before
initialization, panic retirement with payload preservation, and concurrent
first queries. The native Node adapter separately passes four object lifecycles
over ten parser fixtures, with all forty outputs surviving disposal.

## Performance

The paired captures ran on an 18-CPU macOS 26.6.1 arm64 host, using Rust 1.97.1,
Go 1.27.1 and Node v24.20.0. Each retains a warmup and 21 interleaved samples.
No builds or other task tests ran during timing. Ordinary desktop applications
were active; one-minute load averages ranged from 4.70 to 7.53 during parser
samples and 4.50 to 5.34 during Node samples. This is not described as an idle
measurement host.

The parser workload contains 13,094 files and 161,740,237 source bytes. Counts
and encoded bytes matched for every file before timing. Rust's median was
3.632273458 s versus Go's 6.091004750 s. The 95% bootstrap interval for the
Rust/Go time ratio is 0.589461–0.602315, above the 0.5 limit; relative MAD is
0.742% for Rust and 0.755% for Go. More samples are not requested by the existing
stopping rule. Its `stable` field is false because it includes the gate-bound
test, not because these samples have high relative MAD.

The shipped parser is 3,472,720 bytes versus Go's 12,956,989 bytes. It uses the
workspace release optimization, fat LTO, wasm-bindgen 0.2.126, and Binaryen 132
`-Oz`. The pre-optimization module and optimizer identity remain in the build
record. The rejected opt-level-2 configuration produced a larger artifact and
a lower Rust/Go throughput ratio in its seven-sample screen. Earlier screens
are preserved; their ratios are not
combined with this final batch or substituted for it.

Each Node sample contains 100 fresh 10,240-byte sources. The median batch took
36.230250 ms in process versus 105.983500 ms through the real `--api` socket:
0.362303 ms versus 1.059835 ms per call. Every encoded output matched. The 95%
ratio interval is 0.337616–0.346078, above 0.10; relative MAD is 0.497% for Rust
and 1.335% for Go. The in-process interval includes the persistent worker's
request/result transfer. The socket interval includes snapshot update,
loading/binding, encoding, JSON/base64 transport and snapshot release.

## Evidence and remaining work

[The compact machine-readable record](../data/s10/initial-acceptance.json)
contains counts, statistics, artifact hashes and all six capture identities.
Raw inputs, binaries, glue, observations and logs remain under `target/s10` in
`parser-measurement`, `node-measurement`, `portable`, `wasm-corpus`,
`rust-corpus` and `lifetime`. Corpus replay also authenticates the preserved
native E2 observations named in that record. Preserve that native file when
exporting the captures. Earlier configurations remain under
`target/s10/previous` and the named parser screen directories.

Recorded quality checks include workspace Clippy with warnings denied,
formatting, dependency policy and tracker self-tests. The whole locked
workspace checks on MSRV 1.96; relevant script tests and parser-worker
regressions pass. CI adds Linux wasm builds/public-API execution and the
outside-workspace consumer's debug/release lifetime tests.

The next performance work must address the shipped configurations: attribute
parser code size and parsing cost, and attribute native parse/encode/transfer
cost on the fixed Node input before choosing a candidate. Artifact size needs
about 6.7% reduction; parser elapsed time about 16.2%; Node elapsed time about
70.7%, if their paired Go denominators remain unchanged. These are distances,
not predicted savings. Any retained change needs a fresh paired result and the
same correctness/ownership gates. No thresholds, inputs or exclusions changed.

## Review fixes and Node worker attribution

Recoverable wasm `Result::Err` responses now become `WasmApiError`; independent
sessions survive invalid paths, positions, options and retired-session queries.
Unclassified exceptions and traps remain terminal, including engine stack
exhaustion reported as `RangeError`. JSON serialization happens before consuming
a host. Diagnostic file lookup uses the program's existing owner index rather
than scanning all files for every record.

Selftest now discovers both Python test directories, including S10 and the
previously omitted S08 root suites, without executing legacy shim registrations
twice. E7/E8 fingerprints share an explicit dependency manifest and tests verify
closure coverage, ledger agreement and immunity to unrelated edits. Node's two
entry points accept JSX/force-module options; the socket benchmark removes its
own temporary directory even on failure.

Validation: 65 tracker tests and 565 Python tests (one existing skip) pass through
the registered selftest producer. Workspace Clippy with warnings denied passes with all features and targets. The rebuilt checker wasm passes
ten parser fixtures plus recoverable-error, sibling-session, disposal and
terminal-exception checks. The normal release Node module passes forty retained
outputs over four lifecycles, plus JSX and forced-module byte comparisons with
the native parser. A one-input socket benchmark smoke matched the encoded
bytes and verified removal of its temporary directory.

The optional `ts_node/worker-probe` feature timestamps the existing request
path without changing the worker's reserved stack. The outer owner thread only
joins the parser worker; it does not forward each request through another queue.
Twenty-one alternating normal/instrumented batches used 100 fresh 10 KiB inputs
each after one warmup. Every encoded output matched. No task builds or other
tests ran during timing; one-minute host load was 5.89. Sources stayed stable.

| Median per call | Microseconds |
| --- | ---: |
| Normal JS wall interval | 370.695 |
| Instrumented JS wall interval | 373.260 |
| Input preparation | 0.693 |
| Channel creation, dispatch and worker wakeup | 1.990 |
| Worker parse, encode and request disposal | 364.235 |
| Reply and caller wakeup | 2.950 |

The paired median instrumented/normal ratio was 1.00488. Dispatch plus return
took 4.940 µs, about 1.3% of normal elapsed time. Eliminating those boundaries
alone cannot explain or close the roughly 70% reduction needed in the initial
paired E8 result. This is attribution, not an inline implementation experiment
or an acceptance measurement; removing channels could also change scheduling
and cache behavior. Next CPU work should inspect parsing/encoding on the worker.

[The compact probe record](s10-node-worker-probe.json) retains every batch's
timing and load, hashes and boundary definitions. Full input/output hashes,
source hashes and the instrumented binary remain in
`target/s10/review-worker-probe`. Reproduce by building `ts_node` in release
with `--features worker-probe`, copying the library to a separate `.node` file,
and running `node tools/s10/node/worker-probe.mjs <binary.node> 21 100`.
The feature is absent from the shipped default Node build.

## CI capture isolation

Run `35437940348` failed before portable-host execution because `rust-cache`
restored `target/s10/portable`; the capture refused to overwrite that directory.
CI now writes to a run-and-attempt-specific directory under `RUNNER_TEMP`,
outside the Cargo cache, and uploads the complete replay bundle. Existing
captures remain untouched. A regression exercises a restored target, two run
attempts and duplicate-output refusal. A fresh local portable capture using the
workflow command passes with `portable_host: true`. This fixes capture setup;
it does not close the performance criteria or refresh the full corpus evidence.
