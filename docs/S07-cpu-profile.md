# S07 CPU profile: parse and bind

This diagnosis profiles the implementation at `5430ce6`, using the frozen S07
workload on the same macOS arm64 host as the existing measurements. Binding is
the larger elapsed-time gap. The next optimization should target measured AST
access and traversal costs; publication is too small to explain the regression.
The ranked CPU findings below determine which paths to prototype first.

This is diagnostic evidence. No compiler implementation, acceptance binary,
E5/E6 capture, threshold or sprint status changed. S07-4 remains open. The
existing Rust/Go wall ratios remain **1.789 at one worker and 1.931 at eight**,
with the uncertainty qualification described in [S07](S07.md).

## Work and capture

There are three fresh CPU captures per runtime and worker count: twelve
profiles, plus eighteen fresh controls. Every one of the thirty invocations
matches the independently frozen work:

| Obligation | Observed in every invocation |
| --- | ---: |
| Files | 13,094 |
| Loaded source bytes | 161,740,237 |
| Nodes | 19,593,488 |
| Symbols | 2,459,867 |
| Parse diagnostics | 423 |
| Bind diagnostics | 5,250 |

The loaded input/options digest is
`d4ff2ad3b0d590babb596522a7105d961dbb3c4f2ebc987488d88b7ebd8e0c88`.
The compiler/benchmark source fingerprint remains
`17b7559a34741eb8fcf73932c4af928d264981dc2607c154143bf69567152fa3`.
Rust is 1.97.1; Go is 1.27.1 with `GOTOOLCHAIN=local`, `GOGC=100` and one/eight
`GOMAXPROCS`. Upstream is pinned at
`1f70213d4922b434345f639b441681e470c7cfc1`. The host has eighteen physical CPUs,
64 GiB RAM and macOS 26.6.1. Ordinary desktop load remained present; the initial
load averages were 6.95, 8.54 and 8.48. The capture rejected concurrent detected
compiler, benchmark and profiling processes.

Inputs are preloaded before the pipeline. Precreated workers receive bounded
round-robin queues; each file is parsed, published and bound before that worker
moves to its next file. Completed owners stay alive through the endpoint. This
preserves the per-file interleaving and retained-root endpoint; instrumentation
may still perturb allocation timing and GC. It does not
measure program loading, checking, dependency installation or generated outputs.

The Rust adapter is a separate optimized executable with mimalloc, fat LTO,
one codegen unit and packed debug symbols. Its registry versions and checksums
match the production lock. The Go adapter is built from an exact git archive of
the upstream pin. Build commands, input/configuration hashes, executable hashes,
debug-symbol hashes and Cargo's selected artifact are retained in the capture.

## Measurement domains

Rust uses Instruments Time Profiler, with a one-millisecond sampling interval
and waiting-thread capture disabled. The denominator is the sum of recorded
weights for the target process's **Running** samples across all its threads.
The complete trace includes preload, input hashing, reporting and retirement;
those costs are kept separate from parse/publish/bind.

Non-inline wrapper frames delimit the Rust operations. With DWARF, Instruments
can display an inlined callee at the wrapper's physical address. The analyzer
therefore retains both the displayed source frame and the enclosing physical
symbol, verifies the Mach-O UUID, and resolves the latter by binary-relative
address. A generic type merely mentioning a wrapper is not a phase marker.
Missing stacks remain in the denominator. Inclusive function/group weights count
a sample once per selected group and overlap between groups; summing them would
inflate the CPU total.

Go uses `runtime/pprof` at ten milliseconds per sample. Capture starts immediately
before releasing work and stops after all workers finish. `phase=parse` and
`phase=bind` labels retain attribution on runtime system stacks. Unlabelled
background GC/runtime samples remain visible. GC is a subset of those phase
partitions, not additional CPU. Rust whole-process percentages and Go pipeline
percentages have different scopes and sampling resolution.

Both adapters also report per-file elapsed worker timers. They include
preemption/descheduling, and their sum can exceed pipeline wall time with eight
workers. They are useful phase observations, not CPU measurements. Profiler
command wall time additionally includes startup and trace processing.

## Controls and phase timers

These are medians of three fresh diagnostic observations, in seconds. They are
not new acceptance results or an improvement over the earlier capture.

| Pipeline configuration | One worker | Eight workers |
| --- | ---: | ---: |
| Original Go benchmark | 2.821 | 0.647 |
| Go adapter with labels, timers and pprof | 2.832 | 0.648 |
| Original Rust benchmark | 5.306 | 1.200 |
| Rust adapter without Time Profiler | 5.171 | 1.214 |
| Rust adapter with Time Profiler | 5.241 | 1.213 |

The close medians do not prove negligible overhead. Controls always followed
profiles in Go → Rust → Rust-adapter order, the sample size is small, and all
outliers remain included. The final eight-worker Rust profile took 1.400 seconds,
versus 1.214 seconds for its unprofiled adapter control. The Go comparison combines
label, timer and profiler effects; it cannot isolate profiler overhead.

| Median summed elapsed worker time | Go: one | Rust: one | Go: eight | Rust: eight |
| --- | ---: | ---: | ---: | ---: |
| Parse (Rust includes completion validation) | 1.744 | 2.535 | 2.228 | 3.019 |
| Rust publication/root lookup | — | 0.002 | — | 0.003 |
| Bind (Rust includes graph validation) | 0.726 | 2.689 | 0.871 | 3.050 |

Go publication is part of its parser API. Rust `ParsedFile::publish_unbound`
consumes an already validated result and transfers ownership; it does not repeat
the full core scan. Parsing includes grammar work, final metadata, JSDoc/import
work and `AstBuilder::complete`. Binding includes private construction, deferred
expando handling and validation before publication.

## CPU findings

All six Rust traces have nonzero, valid running-thread samples. Pooled CPU
weights across the three processes in each worker mode are:

| Rust phase | One worker: CPU seconds | Eight workers: CPU seconds |
| --- | ---: | ---: |
| Parse | 7.552 | 8.646 |
| Publish/root lookup | 0.002 | 0.015 |
| Bind | 7.956 | 8.921 |
| Retirement after the measured endpoint | 1.030 | 1.149 |
| Preload and other driver work | 2.094 | 2.063 |
| Main-thread pipeline dispatch | 0.068 | 0.027 |
| Worker samples without an assigned phase | 0.061 | 0.061 |
| Other unknown samples | 0.313 | 0.296 |
| **Complete process CPU** | **19.076** | **21.178** |

The phase assignment covers 99.61%/99.65% of worker CPU at one/eight workers.
Missing stacks account for 0.220/0.227 seconds of the complete process totals;
they remain in the unassigned/unknown partitions above. Leaf source metadata
covers 86.5%/87.8% of process CPU; requiring a nonzero line number reduces that
to 79.7%/83.7%. DWARF line zero remains unknown. The physical outer function and
displayed inline frame do not reconstruct every intermediate inline ancestor,
so a query can miss a hidden ancestor. These are sample weights, not invocation
counts or exact continuous CPU timers.

| Measured path | One worker | Eight workers | Denominator |
| --- | ---: | ---: | --- |
| AST lookup union during binding | **38.9%** | **36.4%** | Binding CPU |
| Final `BindBuilder::validate` | 2.6% | 2.4% | Binding CPU |
| AST lookup union during parsing | 12.2% | 11.6% | Parsing CPU |
| Final `AstView::validate_core` | 10.1% | 8.8% | Parsing CPU |
| Edge validation inside new-node construction | 4.4% | 4.4% | Parsing CPU |
| Remaining new-node hook work | 8.7% | 14.0% | Parsing CPU |

The lookup union counts a CPU sample once if its stack passes through
`AstView::node`, `StorageView<Node>::node` or `node_here`, using both physical and
displayed inline names. It therefore includes work below those accessors.
Validation paths can also perform lookups: **the rows overlap and must not be
added**. New-node edge validation and remaining hook work are explicitly
partitioned; the entire construction hook is not validation.

The main access path is in [AST storage](../crates/tsr_ast/src/storage.rs) and
[arena storage](../crates/tsr_arena/src/file.rs). It selects the binding overlay,
resolves the arena and retrieves a checked page/slot. Binding leaf samples also
land in `binding_for_node`, node-ID extraction, result branching, page-directory
access and hash-table probing. Their repeated execution is a measured cost;
this profile does not isolate how much is arithmetic, cache misses or branch
behavior.

Binding traversal reaches these reads through
[`Binder::n`, symbol/locals access](../crates/tsr_binder/src/state.rs),
[`BindBuilder`](../crates/tsr_ast/src/bind_result.rs) and AST utilities. Final
binding validation is a much smaller measured path. In parsing, both core
validation and new-node construction have measurable costs that warrant a
separate experiment after the binder access path.

Whole-process destructor samples include retirement after the benchmark
endpoint, so their prominent leaf names do not by themselves explain E6.
Similarly, large inclusive `OnceLock` and worker-entry totals include the work
inside their callbacks. These totals do not measure lock contention; waiting
threads were excluded from this CPU capture.

## Go comparison

Across the three one-worker profiles, Go records 821 samples / 8.21 seconds of
pipeline CPU: 4.93 seconds labelled parse, 2.09 labelled bind and 1.19 unlabelled.
At eight workers it records 1,151 samples / 11.51 seconds: 6.00 parse, 2.27 bind
and 3.24 unlabelled. These are pooled CPU totals across three processes.

The most frequent sampled PC is `runtime.madvise`: 58.0% and 49.8% of the pooled
one/eight-worker CPU totals. Every observed instance has the path
`sysUsedOS → sysUsed → mheap.allocSpan → mheap.alloc.func1 → systemstack`.
The pinned Darwin runtime calls `MADV_FREE_REUSE` there for kernel accounting.
This identifies the sampled PC and allocation path; it does not establish how
much time belongs to a particular kernel operation, and is not a Rust hotspot.

GC assist, background marking and other identified GC frames together account
for 16.1% of Go CPU at one worker and 27.6% at eight. They are already included
in the phase totals above. In particular, the large unlabelled background-GC
cost must not be dropped when comparing Go's application phases. Labels are
necessary here: `systemstack` samples may lack the user-level caller frames.

## Next bounded experiments

1. **Reduce repeated binder node access.** Start with repeated reads of the same
   node in dispatch and shared helpers. Carry already resolved borrows or small
   immutable observations across adjacent operations where their lifetime and
   mutation boundaries permit it. Avoid cloning entire payloads to work around
   borrowing. Measure lookup frequency and pipeline time before and after.
2. **Prototype a cheaper checked access path for a known owner.** Use the existing
   [branded core-arena pattern](../crates/tsr_arena/src/scope.rs) as an invariant
   reference. A binder-facing path must still observe binding overlays and
   support lazy/mapped/foreign-owner fallbacks. Inspect optimized code for
   repeated arena selection, ID decoding and result/borrow dispatch; retain safe
   bounds access and public import validation. An `inline` attribute alone is
   not a measured solution.
3. **Then investigate node construction and validation together with layout.**
   The new-node hook's non-validation work grows from 8.7% to 14.0% of parsing
   CPU across these worker modes. Final core validation is another 8.8–10.1%.
   Identify duplicate work that a construction proof can replace while keeping
   publication obligations intact. Coordinate this with the retained-layout
   work required by E5; the CPU samples alone do not prove a cache, allocator or
   synchronization cause for the worker-count difference.

The lookup union is about 25.9%/24.2% of combined parsing and binding CPU. That
includes useful data access and is not an eliminable overhead budget. These
priorities identify the first experiments; they do not forecast closing the
much larger wall-time and memory gaps.

Each candidate must preserve checked public IDs, borrowed core reads, lazy and
merged-owner behavior, parsed/bound view separation, full retained ownership,
and the existing failure/reentry contracts. A CPU profile is not permission to
remove an ownership check or validation obligation. Prototype one change at a
time, retain an unchanged control, and rerun the full relevant semantic and
ownership evidence plus paired E5/E6 measurement before claiming an improvement.
The separate allocation/RSS gap in [S07](S07.md) remains; a CPU gain alone cannot
close S07-4.

## Reproduction and review

Use the [profiling tools](../tools/s07/cpu-profile/README.md). The compact
[artifact archive](../tools/s07/cpu-profile/results/2026-09-08/) retains the thirty
raw reports, build/capture provenance, six Go profiles, and targeted Rust CPU
exports and summaries. Native Instruments traces and executable/debug bundles
remain under `target/s07-cpu-profiles` and `target/s07-cpu-build` locally.

The capture review checked build/binary provenance, fixed-work validation,
control stability and failed-capture handling. The analysis review independently
reconciled a real Rust XML export with its 6,291 running samples, checked the
physical symbol ranges against `nm`, and verified disjoint totals and union
accounting. All six Go raw denominators also match native `pprof -top` totals.

Thirteen Rust-analyzer countertests and eight Go-analyzer tests cover malformed
weights/references, missing/duplicate inputs, wrong UUIDs, misleading generic
names, phase precedence, recursion and CPU partitioning. Rust adapter formatting
and `cargo xtask status --check-committed` pass. The archive review checks all
thirty work reports, raw profile hashes, offline Rust replay and reported totals.
Device names and device UUIDs are omitted from archived TOCs; original hashes
and the limited redaction are recorded in the archive manifest.
