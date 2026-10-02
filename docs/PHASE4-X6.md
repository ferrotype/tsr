# Phase 4 X6: tracing and statistics

Implementation record for the pinned Corsa revision
`1f70213d4922b434345f639b441681e470c7cfc1`, on `phase4-x0`.
This records development witnesses; it does not publish status evidence or
claim that the two raw trace baselines match byte for byte.

## Implemented scope

`crates/tsr_tracing` implements the production `TraceSink`: the pinned trace
header and process/thread metadata, stable file thread IDs using XXH3,
checker thread IDs, instant events, separate begin/end events, complete
events crossing a real 10 ms sampling boundary, deterministic counters,
256 KiB buffered appends, first-error retention, the legend, and complete
per-checker type dumps. The sink snapshots the recorded IDs and asks the
original checker to describe them after releasing its own mutex. Type display
can therefore reenter tracing without deadlocking. Types created by display
are outside that snapshot, as in the pin. No synthetic subset type table or
inferred event timestamps are used.

The compiler owns an optional trace sink and forwards it to its checker pool.
The loader records its actual source parse and eager bind operations. It also
records the pinned sampled operations around root processing, project config
parsing, source-file lookup, module/library resolution and automatic/explicit
type references. Resolution arguments retain JSON boolean values.
The common emit path records bind/check phase spans; the existing checker and
emitter event hooks use the same production session.

The standalone command starts and stops tracing, with the pinned warning
strings on file failures. Build mode does not independently start tracing:
the pinned build driver has no `startTracingIfNeeded` call.

`crates/tsr_tsc/src/statistics.rs` ports the table, conditional duration rows,
sorted content-mapper rows, testing callbacks, and build aggregation.
`crates/tsr_compiler/src/statistics.rs` supplies actual line, identifier,
symbol, type and instantiation counts. Per-checker symbol/type/instantiation
counts retain the pin's `uint32` arithmetic; aggregation sums program counts
as signed 64-bit integers. Aggregate total and content-mapper times are not
summed, as in the pin. The caller supplies aggregate elapsed time.

The native `tsrust` binary provides a counting allocator for allocation count
and live requested bytes. These are Rust allocator observations, not Go heap
arena or RSS measurements. Embedded/fake systems without an allocator hook
render `unavailable`, rather than inventing zero memory use. The pinned
baseline sanitizer discards the statistics block, so table fidelity is
witnessed separately below. Profiling remains under plan decision 6.

## Deterministic trace witnesses and approved exception

The bounded development run was:

```sh
python3 scripts/phase4_corpus.py run --output target/phase4/x6-trace-02 --jobs 1 \
  tsc/generateTrace/generateTrace-generates-types-file.js \
  tsc/generateTrace/generateTrace-with-multiple-files-and-complex-types.js
python3 scripts/phase4_compare.py report --rust target/phase4/x6-trace-02
```

Both selected scenarios completed, with no harness or production failure.
The source snapshot stayed stable during capture. Build took 3.569 s, native
replay 0.149 s, and Rust execution 1.244 s. This is a two-row partial capture;
the other 514 scenarios were not selected. The preceding `x6-trace-01` attempt
failed during a concurrently unfinished mapper build and produced no rows.

| Observable | Simple scenario | Complex scenario |
| --- | --- | --- |
| Console and emitted JavaScript | exact | exact |
| `types_0.json`, complete checker table | exact | exact |
| `legend.json` | exact | exact |
| Trace through parse/bind | raw difference | raw difference |
| Trace after `bindSourceFiles` ends, including check and emit | exact | exact |

The raw comparison correctly reports **2 different**, not 2 matches. The
only differences are the parse/bind prefix of `trace.json`: reference
transcript lines 61–72 for the simple case and 81–98 for the complex case.
The pin parses all files, ends `createProgram`, then binds them in reverse
order inside `bindSourceFiles`. The existing S07 immutable completed-file
architecture binds each file while loading it, so the Rust trace places each
`bindSourceFile` inside the corresponding `createSourceFile`, in load order.
The later `bindSourceFiles` span covers the actual bind-diagnostic operation.
After its end (deterministic timestamp 13/17), event bytes match.

The owner approved this narrow trace-order exception during implementation:
preserve the existing storage architecture and report the operations when
they happen. It does not permit fabricated timestamps, reordered events,
weakened type tables, or differences in subsequent checker/emitter events.
Raw captures and raw comparison results remain intact. Any acceptance rule
must name this exception separately instead of silently normalizing it away.

The sampled hooks were completed after this snapshot; deterministic sessions
omit those events. Subsequent shared production changes also stale the
capture's source binding. This record is therefore a development witness,
not a replacement for the final fresh Phase 4 capture.

## Statistics native observation

A temporary overlay added this test to the pinned
`internal/execute/tsc` package, without editing upstream:

```go
package tsc

import (
    "bytes"
    "encoding/base64"
    "fmt"
    "testing"
)

func TestPhase4StatisticsWitness(t *testing.T) {
    stat := Statistics{
        files: 3, lines: 10, identifiers: 21, symbols: 31,
        types: 41, instantiations: 51, memoryUsed: 2049, memoryAllocs: 9,
        compileTimes: &CompileTimes{},
    }
    var output bytes.Buffer
    stat.Report(&output, nil)
    fmt.Println("P4STAT:" + base64.StdEncoding.EncodeToString(output.Bytes()))
}
```

The overlay maps virtual
`upstream/tsc/internal/execute/tsc/phase4_statistics_witness_test.go` to the
temporary test. With Go 1.27.1 on darwin/arm64, this command passed in 0.556 s:

```sh
cd upstream/tsc
go test -overlay /private/tmp/phase4-statistics-overlay.json \
  ./internal/execute/tsc -run '^TestPhase4StatisticsWitness$' -count=1 -v
```

The decoded output, including spaces and final newline, was:

```text
Files:               3
Lines:              10
Identifiers:        21
Symbols:            31
Types:              41
Instantiations:     51
Memory used:        2K
Memory allocs:       9
Parse time:     0.000s
Total time:     0.000s
```

The Rust fixed-input assertion in
`crates/tsr_tsc/src/statistics/tests.rs` matches these observed bytes.
The other statistics tests cover mapper identity ordering, operation-count
gating, and aggregation without summing mapper/total times.

## Focused validation

- All eight `tsr_tracing` tests passed: exact deterministic wire bytes
  and pinned file thread ID, stable per-key thread IDs, matching duration-end
  IDs and metadata with per-thread nesting checked from written JSON, deterministic sample
  suppression and real sampled duration, full type snapshots with reentrant
  display, retained first flush error, retry after final append failure, and
  boolean argument wire types.
- All three statistics tests passed, including the independent native table
  observation above.
- The compiler `phase4_tracing` integration test passed, covering the actual
  sampled loader operations, including a missing explicit type reference,
  a resolved automatic type reference, project config parsing and library
  replacement lookup. These are real resolver operations with an in-memory
  observation sink, so the test is independent of timing/sampling luck.

The final corpus/status update and any ledger regeneration remain coordinated
Phase 4 closure work; this document does not change their results.
